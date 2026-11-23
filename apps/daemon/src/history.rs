use crate::{
    delivery::{Store, inspect_connection, sql},
    retention::{database_soft_limit, disk_reserve, free_bytes, result_usage},
};
use garde_config::{Config, Result};
use garde_contracts::{
    Event, EventCount, EventSummary, HistoryPage, HistoryStats, Id, InstallationHealth, Severity,
    SourceHealth, SourceReport, SshStatus, StorageReport, WireVersion,
};
use garde_entrypoint::message;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use std::fmt::Write;

pub enum Query {
    Recent {
        limit: u8,
        before: Option<(i64, Id)>,
    },
    Event(Id),
    Stats(u16),
    Health,
}

impl Query {
    pub fn parse(name: &str, args: &[&str]) -> Option<Result<Self>> {
        Some(match (name, args) {
            ("recent", []) => Ok(Self::Recent {
                limit: 10,
                before: None,
            }),
            ("recent", [limit]) => recent(limit, None),
            ("recent", [limit, cursor]) => recent(limit, Some(cursor)),
            ("history", [id]) => Id::new(*id).map(Self::Event).map_err(|_| "request.invalid"),
            ("stats", []) => Ok(Self::Stats(24)),
            ("stats", [hours]) => hours
                .parse::<u16>()
                .ok()
                .filter(|n| (1..=168).contains(n))
                .map(Self::Stats)
                .ok_or("request.invalid"),
            ("health", []) => Ok(Self::Health),
            ("recent" | "history" | "stats" | "health", _) => Err("request.invalid"),
            _ => return None,
        })
    }
}

fn recent(limit: &str, cursor: Option<&str>) -> Result<Query> {
    let limit = limit
        .parse::<u8>()
        .ok()
        .filter(|n| (1..=10).contains(n))
        .ok_or("request.invalid")?;
    let before = cursor
        .map(|value| {
            let (at, id) = value.split_once(':').ok_or("request.invalid")?;
            let at = at
                .parse::<i64>()
                .ok()
                .filter(|n| *n >= 0)
                .ok_or("request.invalid")?;
            Ok::<_, &'static str>((at, Id::new(id).map_err(|_| "request.invalid")?))
        })
        .transpose()?;
    Ok(Query::Recent { limit, before })
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum Output {
    Recent(HistoryPage),
    Event(EventSummary),
    Stats(HistoryStats),
    Health(Box<InstallationHealth>),
}

pub fn inspect(config: &Config, query: Query, now: i64) -> Result<Output> {
    let connection = inspect_connection(
        &config.state_dir,
        config.server_id.as_str(),
        config.owner()?,
    )?;
    match query {
        Query::Recent { limit, before } => {
            let before = before.as_ref().map(|(at, id)| (*at, id.as_str()));
            let mut statement = sql(connection.prepare(
                "SELECT payload,created_at FROM events WHERE (?1 IS NULL OR (created_at,id)<(?1,?2))
                 AND json_extract(payload,'$.source')<>'summary'
                 ORDER BY created_at DESC,id DESC LIMIT ?3",
            ))?;
            let mut rows = sql(statement.query(params![
                before.map(|(at, _)| at),
                before.map(|(_, id)| id),
                u16::from(limit) + 1
            ]))?;
            let mut events = Vec::new();
            let mut last_at = 0;
            let mut next_cursor = None;
            while let Some(row) = sql(rows.next())? {
                if events.len() == usize::from(limit) {
                    next_cursor = events
                        .last()
                        .map(|last: &EventSummary| format!("{last_at}:{}", last.id.as_str()));
                    break;
                }
                events.push(summary(&sql(row.get::<_, String>(0))?)?);
                last_at = sql(row.get(1))?;
            }
            Ok(Output::Recent(HistoryPage {
                version: WireVersion::V1,
                events,
                next_cursor,
            }))
        }
        Query::Event(id) => {
            let payload: String = sql(connection
                .query_row(
                    "SELECT payload FROM events WHERE id=?1",
                    [id.as_str()],
                    |r| r.get(0),
                )
                .optional())?
            .ok_or("history.not_found")?;
            summary(&payload).map(Output::Event)
        }
        Query::Stats(hours) => {
            let snapshot = sql(connection.unchecked_transaction())?;
            let output = stats(&snapshot, hours, now)?;
            sql(snapshot.commit())?;
            Ok(Output::Stats(output))
        }
        Query::Health => health(&connection, config, now).map(|h| Output::Health(Box::new(h))),
    }
}

fn summary(payload: &str) -> Result<EventSummary> {
    let event: Event = serde_json::from_str(payload).map_err(|_| "storage.corrupt")?;
    Ok(EventSummary {
        id: event.id,
        observed_at: event.observed_at,
        source: event.source,
        severity: event.classification.severity,
        reason_key: event.classification.reason_key,
    })
}

fn stats(connection: &Connection, hours: u16, now: i64) -> Result<HistoryStats> {
    let cutoff = now.saturating_sub(i64::from(hours) * 3600);
    let total = sql(connection.query_row(
        "SELECT count(*) FROM events WHERE created_at BETWEEN ?1 AND ?2 AND json_extract(payload,'$.source')<>'summary'",
        params![cutoff, now],
        |r| r.get(0),
    ))?;
    let mut statement = sql(connection.prepare(
        "SELECT json_extract(payload,'$.source'),json_extract(payload,'$.classification.severity'),count(*)
         FROM events WHERE created_at BETWEEN ?1 AND ?2 AND json_extract(payload,'$.source')<>'summary'
         GROUP BY 1,2 ORDER BY 3 DESC,1,2 LIMIT 24",
    ))?;
    let rows = sql(statement.query_map(params![cutoff, now], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get(2)?))
    }))?;
    let mut groups = Vec::new();
    for row in rows {
        let (source, severity, count) = sql(row)?;
        groups.push(EventCount {
            source: Id::new(source).map_err(|_| "storage.corrupt")?,
            severity: serde_json::from_value(serde_json::Value::String(severity))
                .map_err(|_| "storage.corrupt")?,
            count,
        });
    }
    Ok(HistoryStats {
        version: WireVersion::V1,
        hours,
        total,
        groups,
    })
}

fn health(connection: &Connection, config: &Config, now: i64) -> Result<InstallationHealth> {
    fn report(name: &str, enabled: bool, healthy: bool, reason_key: Option<Id>) -> SourceReport {
        SourceReport {
            source: Id::new(name).expect("static source"),
            enabled,
            healthy,
            reason_key,
            gaps: 0,
            last_gap_at: None,
        }
    }
    fn log(name: &str, health: SourceHealth) -> SourceReport {
        let mut report = report(
            name,
            true,
            health.state == SshStatus::Healthy,
            health.reason_key,
        );
        report.gaps = health.gap_count;
        report.last_gap_at = health.last_gap_at;
        report
    }
    let mut sources = Vec::new();
    let mut add = |name: &str, value: Result<SourceReport>| {
        sources.push(value.unwrap_or_else(|error| report(name, true, false, Id::new(error).ok())));
    };
    add(
        "ssh",
        Store::inspect_ssh(config, now).map(|h| log("ssh", h)),
    );
    match Store::inspect_firewall(config, now) {
        Ok(h) => {
            add("fail2ban", Ok(log("fail2ban", h.fail2ban)));
            add("firewall", Ok(log("firewall", h.firewall)));
        }
        Err(error) => {
            add("fail2ban", Err(error));
            add("firewall", Err(error));
        }
    }
    add(
        "host",
        Store::inspect_host(config, now).map(|h| {
            report(
                "host",
                h.enabled,
                crate::host_health::healthy(&h),
                h.reason_key,
            )
        }),
    );
    add(
        "services",
        Store::inspect_services(config, now)
            .map(|h| report("services", h.enabled, crate::services::healthy(&h), None)),
    );
    add(
        "ports",
        Store::inspect_ports(config, now).map(|h| {
            report(
                "ports",
                h.enabled,
                crate::listeners::healthy(&h),
                h.problems.into_iter().next(),
            )
        }),
    );
    add(
        "xui",
        Store::inspect_xui(config, now).map(|h| {
            let healthy = crate::xui::healthy(&h);
            let mut view = report(
                "xui",
                h.enabled,
                healthy,
                h.sources.into_iter().find_map(|s| s.reason_key),
            );
            view.gaps = h.gaps;
            view
        }),
    );
    add(
        "geoip",
        Store::inspect_geoip(config, now).map(|h| {
            report(
                "geoip",
                h.enabled,
                h.reason_key.as_str() == "geoip_ready",
                Some(h.reason_key),
            )
        }),
    );
    let delivery = crate::delivery::health(connection, now)?;
    let policy = config.retention;
    let pages: u32 = sql(connection.pragma_query_value(None, "page_count", |r| r.get(0)))?;
    let free: u32 = sql(connection.pragma_query_value(None, "freelist_count", |r| r.get(0)))?;
    let occupied_bytes = u64::from(pages.saturating_sub(free)) * 4096;
    let free_disk_bytes = free_bytes(&config.state_dir)?;
    let results = result_usage(&config.state_dir);
    let result_error = results.as_ref().err().copied();
    let (result_bytes, result_count) = results.unwrap_or_default();
    if let Some(error) = result_error {
        sources.push(report("results", true, false, Id::new(error).ok()));
    }
    let (action_audit_records, process_audit_records) = crate::retention::audit_counts(connection)?;
    let backpressure = delivery.pending.saturating_add(delivery.in_flight)
        >= policy.pending_deliveries
        || u64::from(pages) * 4096 > policy.database_bytes()
        || occupied_bytes >= database_soft_limit(policy)
        || free_disk_bytes < disk_reserve(policy)
        || result_bytes >= policy.result_bytes()
        || result_count >= crate::retention::MAX_RESULTS as u32;
    let degraded = backpressure
        || delivery.dead_letter > 0
        || delivery
            .oldest_pending_at
            .is_some_and(|at| now.saturating_sub(at.timestamp()) >= 60)
        || sources
            .iter()
            .any(|s| s.enabled && (!s.healthy || s.gaps > 0));
    Ok(InstallationHealth {
        version: WireVersion::V1,
        degraded,
        sources,
        delivery,
        storage: StorageReport {
            database_bytes: u64::from(pages) * 4096,
            database_limit_bytes: policy.database_bytes(),
            occupied_bytes,
            free_disk_bytes,
            pending_limit: policy.pending_deliveries,
            result_bytes,
            result_limit_bytes: policy.result_bytes(),
            result_count,
            action_audit_records,
            process_audit_records,
            backpressure,
        },
    })
}

impl Output {
    pub fn degraded(&self) -> bool {
        matches!(self, Self::Health(h) if h.degraded)
    }

    pub(crate) fn render(&self, locale: &str) -> String {
        let label = |key| message(locale, key);
        fn severity(value: Severity) -> &'static str {
            match value {
                Severity::Info => "info",
                Severity::Warning => "warning",
                Severity::High => "high",
                Severity::Critical => "critical",
            }
        }
        let event_line = |row: &EventSummary| {
            [
                crate::isolate(row.id.as_str()),
                crate::isolate(&row.observed_at.to_rfc3339()),
                crate::isolate(row.source.as_str()),
                severity(row.severity).to_owned(),
            ]
            .join(" ")
        };
        let mut text = String::new();
        match self {
            Self::Recent(page) => {
                text.push_str(label("history_title"));
                for event in &page.events {
                    let _ = write!(text, "\n{}", event_line(event));
                }
                if let Some(cursor) = &page.next_cursor {
                    let _ = write!(text, "\n/recent {} {cursor}", page.events.len());
                }
            }
            Self::Event(event) => {
                let reason = label(event.reason_key.as_str());
                let reason = if reason == "message.unavailable" {
                    event.reason_key.as_str()
                } else {
                    reason
                };
                let _ = write!(
                    text,
                    "{}\n{}\n{}",
                    label("history_title"),
                    event_line(event),
                    crate::isolate(reason)
                );
            }
            Self::Stats(stats) => {
                let _ = write!(
                    text,
                    "{} / {}: {}",
                    label("history_stats_title"),
                    stats.hours,
                    stats.total
                );
                for group in &stats.groups {
                    let _ = write!(
                        text,
                        "\n{}",
                        format_args!(
                            "{} {}: {}",
                            crate::isolate(group.source.as_str()),
                            severity(group.severity),
                            group.count
                        )
                    );
                }
            }
            Self::Health(health) => {
                text.push_str(label("history_health_title"));
                for source in &health.sources {
                    let status = if !source.enabled {
                        "history_disabled"
                    } else if source.healthy {
                        "history_healthy"
                    } else {
                        "history_degraded"
                    };
                    let _ = write!(
                        text,
                        "\n{}: {}",
                        crate::isolate(source.source.as_str()),
                        label(status)
                    );
                    if let Some(reason) = &source.reason_key {
                        let _ = write!(text, " [{}]", crate::isolate(reason.as_str()));
                    }
                    if source.gaps > 0 {
                        let _ = write!(text, " · {}: {}", label("history_gaps"), source.gaps);
                    }
                    if let Some(at) = source.last_gap_at {
                        let _ = write!(text, " {}", crate::isolate(&at.to_rfc3339()));
                    }
                }
                let storage = &health.storage;
                let _ = write!(
                    text,
                    "\n{}: {} / {} B\n{}: {} / {} B",
                    label("history_database"),
                    storage.occupied_bytes,
                    storage.database_limit_bytes,
                    label("history_results"),
                    storage.result_bytes,
                    storage.result_limit_bytes
                );
                if storage.backpressure {
                    let _ = write!(text, "\n{}", label("history_backpressure"));
                }
                let mut delivery = health.delivery.clone();
                delivery.problems.truncate(5);
                let _ = write!(text, "\n\n{}", crate::render_delivery(&delivery, locale));
            }
        }
        text
    }
}
