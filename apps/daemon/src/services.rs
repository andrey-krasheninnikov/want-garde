use crate::{
    observation::{Probe, publication, stale},
    runtime::Database,
};
use garde_config::{Config, Result};
use garde_contracts::{
    Id, Measurement, ServiceObservation, ServiceStatus, ServicesHealth, Severity, WireVersion,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    version: u8,
    sequence: u64,
    checked_at: i64,
    services: Vec<ServiceObservation>,
    down: BTreeSet<String>,
    problems: BTreeMap<String, Id>,
}
fn decode(raw: &str) -> Result<State> {
    if raw.len() > 65536 {
        return Err("services.state");
    }
    let s: State = serde_json::from_str(raw).map_err(|_| "services.state")?;
    if s.version != 1
        || s.checked_at < 0
        || s.services.len() > 32
        || s.down.len() > 32
        || s.problems.len() > 32
        || s.services
            .iter()
            .any(|v| !garde_config::valid_unit(&v.unit))
    {
        return Err("services.state");
    }
    Ok(s)
}
pub(crate) fn validate_state(raw: &str) -> Result<()> {
    decode(raw).map(|_| ())
}
fn unavailable(unit: &str, reason: &str) -> ServiceObservation {
    ServiceObservation {
        unit: unit.into(),
        state: Measurement::Unavailable {
            reason_key: Id::new(reason).expect("static_identifier"),
        },
    }
}
fn parse(units: &[String], raw: Result<String>) -> Vec<ServiceObservation> {
    let error = match &raw {
        Err(e) => Some(*e),
        Ok(s) if s.len() > 65536 => Some("monitor.output"),
        _ => None,
    };
    if let Some(reason) = error {
        return units.iter().map(|u| unavailable(u, reason)).collect();
    }
    let text = raw.unwrap_or_default();
    let mut rows = BTreeMap::new();
    let mut invalid = false;
    for block in text.split("\n\n").filter(|b| !b.trim().is_empty()) {
        let mut fields = BTreeMap::new();
        for line in block.lines() {
            let Some((key, value)) = line.split_once('=') else {
                invalid = true;
                continue;
            };
            if fields.insert(key, value).is_some() {
                invalid = true;
            }
        }
        if let Some(unit) = fields.get("Id") {
            if rows.get(unit).is_some_and(|previous| previous != &fields) {
                invalid = true;
            }
            rows.insert(*unit, fields);
        } else {
            invalid = true;
        }
    }
    units
        .iter()
        .map(|unit| {
            if invalid {
                return unavailable(unit, "services.output");
            }
            let Some(fields) = rows.get(unit.as_str()).or_else(|| {
                rows.values().find(|fields| {
                    fields
                        .get("Names")
                        .is_some_and(|names| names.split_whitespace().any(|name| name == unit))
                })
            }) else {
                return unavailable(unit, "services.unit_missing");
            };
            let Some((&load, &active, &sub)) = fields
                .get("LoadState")
                .zip(fields.get("ActiveState"))
                .zip(fields.get("SubState"))
                .map(|((l, a), s)| (l, a, s))
            else {
                return unavailable(unit, "services.output");
            };
            if !["loaded", "masked"].contains(&load) {
                return unavailable(unit, "services.unit_missing");
            }
            if ![
                "active",
                "inactive",
                "failed",
                "activating",
                "deactivating",
                "reloading",
                "refreshing",
                "maintenance",
            ]
            .contains(&active)
                || sub.is_empty()
                || sub.len() > 64
                || !sub
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            {
                return unavailable(unit, "services.output");
            }
            ServiceObservation {
                unit: unit.clone(),
                state: Measurement::Available {
                    value: ServiceStatus {
                        load_state: load.into(),
                        active_state: active.into(),
                        sub_state: sub.into(),
                    },
                },
            }
        })
        .collect()
}
pub struct Monitor {
    config: Config,
    database: Database,
}
impl Monitor {
    pub fn new(config: Config, database: Database) -> Self {
        Self { config, database }
    }
    pub async fn observe(&mut self, raw: Result<String>, at: i64) -> Result<()> {
        if self.config.services.units.is_empty() {
            return Ok(());
        }
        if at < 0 {
            return Err("monitor.clock");
        }
        let previous = self
            .database
            .call(|s| s.observation_state("services.state"))
            .await?;
        let mut state = previous
            .as_deref()
            .map(decode)
            .transpose()?
            .unwrap_or(State {
                version: 1,
                sequence: 0,
                checked_at: at,
                services: vec![],
                down: BTreeSet::new(),
                problems: BTreeMap::new(),
            });
        state
            .down
            .retain(|u| self.config.services.units.contains(u));
        state
            .problems
            .retain(|u, _| self.config.services.units.contains(u));
        state.checked_at = at;
        state.services = parse(&self.config.services.units, raw);
        let mut publications = Vec::new();
        for row in &state.services {
            let mut changes = Vec::new();
            match &row.state {
                Measurement::Unavailable { reason_key } => {
                    if state.problems.get(&row.unit) != Some(reason_key) {
                        changes.push((
                            "services_coverage_lost",
                            Severity::Warning,
                            Some(reason_key.as_str().to_owned()),
                        ));
                    }
                    state.problems.insert(row.unit.clone(), reason_key.clone());
                }
                Measurement::Available { value } => {
                    if state.problems.remove(&row.unit).is_some() {
                        changes.push(("services_coverage_restored", Severity::Info, None));
                    }
                    if ["inactive", "failed"].contains(&value.active_state.as_str())
                        && state.down.insert(row.unit.clone())
                    {
                        changes.push((
                            "service_down",
                            Severity::Critical,
                            Some(value.active_state.clone()),
                        ));
                    }
                    if value.active_state == "active" && state.down.remove(&row.unit) {
                        changes.push((
                            "service_recovered",
                            Severity::Info,
                            Some(value.active_state.clone()),
                        ));
                    }
                }
            }
            for (reason, severity, detail) in changes {
                let mut values = vec![("service_unit", row.unit.clone())];
                if let Some(value) = detail {
                    values.push(("service_state", value));
                }
                publications.push(publication(
                    &self.config,
                    &mut state.sequence,
                    "services",
                    reason,
                    severity,
                    values,
                    at,
                )?);
            }
        }
        let raw = serde_json::to_string(&state).map_err(|_| "services.state")?;
        self.database
            .call(move |s| {
                s.publish_observation(
                    "services.state",
                    previous.as_deref(),
                    &raw,
                    &publications,
                    at,
                )
            })
            .await
    }
    pub async fn run(&mut self) -> Result<()> {
        let mut probe = Probe::new("/usr/bin/systemctl".into());
        loop {
            let mut args = vec![
                "show",
                "--no-pager",
                "--all",
                "--property=Id,Names,LoadState,ActiveState,SubState",
                "--",
            ];
            args.extend(self.config.services.units.iter().map(String::as_str));
            let raw = if self.config.services.units.is_empty() {
                Ok(String::new())
            } else {
                probe.read(&args).await
            };
            let at = crate::runtime::now();
            while let Err(code) = self.observe(raw.clone(), at).await {
                eprintln!("{code}");
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
            tokio::time::sleep(std::time::Duration::from_secs(
                self.config.services.interval_seconds,
            ))
            .await;
        }
    }
}
pub(crate) fn health(config: &Config, raw: Option<&str>, now: i64) -> Result<ServicesHealth> {
    let state = raw.map(decode).transpose()?;
    let checked_at = state.as_ref().map(|s| s.checked_at);
    Ok(ServicesHealth {
        version: WireVersion::V1,
        server_id: config.server_id.clone(),
        checked_at: checked_at.and_then(|at| chrono::DateTime::from_timestamp(at, 0)),
        enabled: !config.services.units.is_empty(),
        stale: stale(checked_at, now, config.services.interval_seconds),
        services: config
            .services
            .units
            .iter()
            .map(|u| {
                state
                    .as_ref()
                    .and_then(|s| s.services.iter().find(|r| r.unit == *u))
                    .cloned()
                    .unwrap_or_else(|| unavailable(u, "monitor.not_checked"))
            })
            .collect(),
        down: state
            .map(|s| {
                s.down
                    .into_iter()
                    .filter(|u| config.services.units.contains(u))
                    .collect()
            })
            .unwrap_or_default(),
    })
}
pub fn healthy(health: &ServicesHealth) -> bool {
    health.enabled
        && !health.stale
        && health.down.is_empty()
        && health
            .services
            .iter()
            .all(|r| matches!(r.state, Measurement::Available { .. }))
}

pub fn render(health: &ServicesHealth, page: usize, locale: &str) -> String {
    use garde_entrypoint::message;
    let header = crate::observation::header(
        "services_title",
        health.server_id.as_str(),
        health.checked_at,
        health.enabled,
        health.stale,
        locale,
    );
    let rows: Vec<_> = health
        .services
        .iter()
        .map(|row| match &row.state {
            Measurement::Available { value } => format!(
                "{}: {}/{}{}",
                row.unit,
                value.active_state,
                value.sub_state,
                if health.down.contains(&row.unit) {
                    format!(" ({})", message(locale, "service_down"))
                } else {
                    String::new()
                }
            ),
            Measurement::Unavailable { reason_key } => format!(
                "{}: {} ({})",
                row.unit,
                message(locale, "metric_unavailable"),
                reason_key.as_str()
            ),
        })
        .collect();
    crate::observation::page_rows(header, &rows, page, locale)
}
