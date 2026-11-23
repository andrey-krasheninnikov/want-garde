use crate::{Reply, delivery::sql};
use chrono::{DateTime, Utc};
use garde_config::{NotificationPolicy, Result};
use garde_contracts::{Event, Severity};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

pub(crate) fn initialize(connection: &Connection) -> Result<()> {
    sql(connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS notification_seen(
            chat_id TEXT NOT NULL,
            signature BLOB NOT NULL,
            seen_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL,
            PRIMARY KEY(chat_id,signature));
         CREATE INDEX IF NOT EXISTS notification_seen_expiry ON notification_seen(expires_at);
         CREATE TABLE IF NOT EXISTS notification_buckets(
            chat_id TEXT NOT NULL,
            start_at INTEGER NOT NULL,
            end_at INTEGER NOT NULL,
            total INTEGER NOT NULL,
            minimum INTEGER NOT NULL,
            closed INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY(chat_id,start_at,end_at));
         CREATE INDEX IF NOT EXISTS notification_buckets_due ON notification_buckets(end_at);
         CREATE TABLE IF NOT EXISTS notification_priority(
            delivery_id INTEGER PRIMARY KEY REFERENCES deliveries(id) ON DELETE CASCADE,
            critical INTEGER NOT NULL CHECK(critical IN(0,1)));
         CREATE TABLE IF NOT EXISTS notification_daily(
            chat_id TEXT PRIMARY KEY, last_date TEXT NOT NULL);",
    ))
}

fn rank(severity: Severity) -> u8 {
    match severity {
        Severity::Info => 0,
        Severity::Warning => 1,
        Severity::High => 2,
        Severity::Critical => 3,
    }
}

fn signature(event: &Event, severity: Severity) -> Result<[u8; 32]> {
    // Times and assigned IDs differ between observations of the same activity.
    let mut evidence: Vec<_> = event
        .evidence
        .iter()
        .filter(|item| event.source.as_str() != "ssh" || item.name.as_str() != "port")
        .map(|item| (item.name.as_str(), item.value.as_str()))
        .collect();
    evidence.sort_unstable();
    let key = serde_json::to_vec(&(
        event.source.as_str(),
        event.classification.rule_id.as_str(),
        severity,
        evidence,
    ))
    .map_err(|_| "notification.invalid")?;
    Ok(Sha256::digest(key).into())
}

pub(crate) fn prune(connection: &Connection, cutoff: i64, now: i64) -> Result<()> {
    sql(connection.execute(
        "DELETE FROM notification_seen WHERE rowid IN
          (SELECT rowid FROM notification_seen WHERE expires_at<=?1 ORDER BY expires_at LIMIT 128)",
        [now],
    ))?;
    sql(connection.execute(
        "DELETE FROM notification_buckets WHERE rowid IN
          (SELECT rowid FROM notification_buckets WHERE end_at<?1 ORDER BY end_at LIMIT 32)",
        [cutoff],
    ))?;
    Ok(())
}

pub(crate) fn quiet_chats(
    config: Option<&garde_config::Config>,
    now: i64,
) -> Result<(String, String)> {
    let mut blocked = Vec::new();
    let mut critical_only = Vec::new();
    if let Some(config) = config
        && let Some(policy) = &config.notification_policy
    {
        let now = DateTime::<Utc>::from_timestamp(now, 0).ok_or("service.clock")?;
        for chat in config
            .owner_user_id
            .iter()
            .chain(
                config
                    .notifications
                    .private
                    .iter()
                    .map(|recipient| &recipient.chat_id),
            )
            .chain(
                config
                    .notifications
                    .forums
                    .iter()
                    .map(|forum| &forum.chat_id),
            )
            .chain(policy.recipients.keys())
        {
            let preference = policy.preferences(chat);
            if preference.quiet(now) {
                if preference.critical_bypasses_quiet {
                    critical_only.push(chat);
                } else {
                    blocked.push(chat);
                }
            }
        }
    }
    Ok((
        serde_json::to_string(&blocked).map_err(|_| "notification.invalid")?,
        serde_json::to_string(&critical_only).map_err(|_| "notification.invalid")?,
    ))
}

pub(crate) fn select(
    transaction: &Connection,
    policy: &NotificationPolicy,
    event: &Event,
    reply: &Reply,
    now: i64,
) -> Result<Option<Severity>> {
    let preferences = policy.preferences(&reply.chat_id);
    let rule = policy.rule(event.classification.rule_id.as_str());
    let severity = rule.severity.unwrap_or(event.classification.severity);
    if !rule.enabled || rank(severity) < rank(rule.min_severity).max(rank(preferences.min_severity))
    {
        return Ok(None);
    }
    if rule.aggregate
        && let Some(live) = &preferences.live
    {
        let window = i64::from(live.window_seconds);
        let start = now.div_euclid(window) * window;
        let increment = |start: i64| {
            sql(transaction.execute(
                "INSERT INTO notification_buckets(chat_id,start_at,end_at,total,minimum) VALUES (?1,?2,?3,1,?4)
                 ON CONFLICT(chat_id,start_at,end_at) DO UPDATE SET total=min(4294967295,total+1) WHERE closed=0",
                params![reply.chat_id,start,start.saturating_add(window),live.minimum_events],
            ))
        };
        if increment(start)? == 0 {
            // New events after a clock rollback belong to the next unclosed window.
            let end: i64 = sql(transaction.query_row(
                "SELECT max(end_at) FROM notification_buckets WHERE chat_id=?1 AND closed=1",
                [&reply.chat_id],
                |row| row.get(0),
            ))?;
            increment(end.div_euclid(window) * window)?;
        }
    }
    let wall_time = DateTime::<Utc>::from_timestamp(now, 0).ok_or("service.clock")?;
    if !rule.immediate
        || (preferences.quiet(wall_time)
            && !(severity == Severity::Critical && preferences.critical_bypasses_quiet))
    {
        return Ok(None);
    }
    if rule.dedupe_seconds == 0 {
        return Ok(Some(severity));
    }
    let digest = signature(event, severity)?;
    let seen: Option<i64> = sql(transaction
        .query_row(
            "SELECT seen_at FROM notification_seen WHERE chat_id=?1 AND signature=?2",
            params![reply.chat_id, digest.as_slice()],
            |r| r.get(0),
        )
        .optional())?;
    let seconds = i64::from(rule.dedupe_seconds);
    if seen.is_some_and(|seen| now.saturating_sub(seen) < seconds) {
        return Ok(None);
    }
    sql(transaction.execute(
        "INSERT INTO notification_seen VALUES (?1,?2,?3,?4)
         ON CONFLICT(chat_id,signature) DO UPDATE SET seen_at=excluded.seen_at,expires_at=excluded.expires_at",
        params![reply.chat_id,digest.as_slice(),now,now.saturating_add(seconds)],
    ))?;
    Ok(Some(severity))
}

use crate::delivery::Store;
use chrono::NaiveDate;
use garde_config::NotificationPreferences;
use garde_contracts::{Classification, Id, WireVersion};
use garde_entrypoint::message;

fn summary_event(at: i64, kind: &str, server: Id) -> Result<Event> {
    Ok(Event {
        version: WireVersion::V1,
        id: Id::new(format!("summary-{kind}-{at}"))?,
        server_id: server,
        observed_at: DateTime::<Utc>::from_timestamp(at, 0).ok_or("service.clock")?,
        source: Id::new("summary")?,
        classification: Classification {
            category: Id::new("notifications.summary")?,
            severity: Severity::Info,
            rule_id: Id::new(format!("summary.{kind}"))?,
            reason_key: Id::new(format!("notification_{kind}"))?,
        },
        evidence: vec![],
    })
}

fn persist_summary(
    transaction: &rusqlite::Transaction<'_>,
    config: &garde_config::Config,
    reply: &Reply,
    kind: &str,
    period: (i64, i64),
    now: i64,
) -> Result<()> {
    let (start, end) = period;
    let mut event = summary_event(end, kind, config.server_id.clone())?;
    let digest = Sha256::digest(format!("{}:{kind}:{start}:{end}", reply.chat_id).as_bytes());
    event.id = Id::new(format!("summary-{digest:x}"))?;
    let payload = serde_json::to_string(&event).map_err(|_| "notification.invalid")?;
    sql(transaction.execute(
        "INSERT INTO events VALUES(?1,?2,?3)",
        params![event.id.as_str(), payload, now],
    ))?;
    crate::delivery::enqueue(
        transaction,
        Some(event.id.as_str()),
        None,
        reply,
        now,
        86400,
    )
}

fn summary_text(reply: &mut Reply, kind: &str, total: u32, at: i64) -> Result<()> {
    let locale = reply.locale.as_deref().unwrap_or("en");
    let at = DateTime::<Utc>::from_timestamp(at, 0).ok_or("service.clock")?;
    reply.text = format!(
        "{}\n{}: {}\n{}",
        message(
            locale,
            if kind == "daily" {
                "notification_daily"
            } else {
                "notification_live"
            }
        ),
        message(locale, "notification_total"),
        total,
        crate::isolate(&at.to_rfc3339()),
    );
    Ok(())
}

fn count_for_daily(
    connection: &Connection,
    policy: &NotificationPolicy,
    chat: &str,
    start: i64,
    end: i64,
) -> Result<u32> {
    let min = rank(policy.preferences(chat).min_severity);
    let mut statement = sql(connection.prepare(
        "SELECT json_extract(payload,'$.classification.rule_id'),json_extract(payload,'$.classification.severity'),count(*)
         FROM events WHERE created_at>=?1 AND created_at<?2 AND json_extract(payload,'$.source')<>'summary' GROUP BY 1,2",
    ))?;
    let rows = sql(statement.query_map(params![start, end], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, u32>(2)?,
        ))
    }))?;
    let mut total = 0u32;
    for row in rows {
        let (rule_key, severity, count) = sql(row)?;
        let rule = policy.rule(&rule_key);
        let severity: Severity = serde_json::from_value(serde_json::Value::String(severity))
            .map_err(|_| "storage.corrupt")?;
        if rule.enabled
            && rank(rule.severity.unwrap_or(severity)) >= min.max(rank(rule.min_severity))
        {
            total = total.saturating_add(count);
        }
    }
    Ok(total)
}

fn next_date(last: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(last, "%F")
        .ok()
        .and_then(|d| d.succ_opt())
        .ok_or("storage.corrupt")
}

fn daily_slot(
    pref: &NotificationPreferences,
    mut date: NaiveDate,
    cutoff: i64,
) -> Result<(NaiveDate, i64)> {
    for _ in 0..4 {
        if let Some(at) = pref.scheduled_at(date)
            && at > cutoff
        {
            return Ok((date, at));
        }
        date = date.succ_opt().ok_or("service.clock")?;
    }
    Err("service.clock")
}

fn daily_date(
    pref: &NotificationPreferences,
    last: Option<&str>,
    today: NaiveDate,
    cutoff: i64,
) -> Result<NaiveDate> {
    let oldest = DateTime::<Utc>::from_timestamp(cutoff, 0)
        .ok_or("service.clock")?
        .with_timezone(&pref.time_zone)
        .date_naive();
    Ok(last
        .map(next_date)
        .transpose()?
        .unwrap_or(today)
        .max(oldest))
}

impl Store {
    pub fn tick_notifications(&mut self, now: i64) -> Result<()> {
        let Some(config) = self
            .geoip_config
            .as_ref()
            .filter(|c| c.notification_policy.is_some())
            .cloned()
        else {
            return Ok(());
        };
        let policy = config
            .notification_policy
            .as_ref()
            .expect("configured policy");
        let now_utc = DateTime::<Utc>::from_timestamp(now, 0).ok_or("service.clock")?;
        let bot = crate::Bot::new(config.clone())?;
        let daily = summary_event(now, "daily", config.server_id.clone())?;
        let live = summary_event(now, "live", config.server_id.clone())?;
        let mut failure = None;
        for reply in bot.event_replies(&daily)? {
            let pref = policy.preferences(&reply.chat_id);
            if let Err(code) = self.tick_daily(&config, policy, pref, reply, now_utc) {
                failure.get_or_insert(code);
            }
        }
        for reply in bot.event_replies(&live)? {
            let pref = policy.preferences(&reply.chat_id);
            if let Err(code) = self.tick_live(&config, pref, reply, now_utc) {
                failure.get_or_insert(code);
            }
        }
        failure.map_or(Ok(()), Err)
    }

    fn tick_daily(
        &mut self,
        config: &garde_config::Config,
        policy: &NotificationPolicy,
        pref: &NotificationPreferences,
        mut reply: Reply,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let Some(daily) = &pref.daily else {
            return Ok(());
        };
        let today = now.with_timezone(&pref.time_zone).date_naive();
        let last: Option<String> = sql(self
            .connection
            .query_row(
                "SELECT last_date FROM notification_daily WHERE chat_id=?1",
                [&reply.chat_id],
                |r| r.get(0),
            )
            .optional())?;
        let timestamp = now.timestamp();
        let cutoff = timestamp.saturating_sub(i64::from(self.retention.event_days) * 86400);
        let date = daily_date(pref, last.as_deref(), today, cutoff)?;
        if last.is_none() {
            let yesterday = today.pred_opt().ok_or("service.clock")?;
            sql(self.connection.execute(
                "INSERT INTO notification_daily VALUES (?1,?2)",
                params![reply.chat_id, yesterday.to_string()],
            ))?;
        }
        if date > today {
            return Ok(());
        }
        let (date, at) = daily_slot(pref, date, cutoff)?;
        if at > timestamp {
            return Ok(());
        }
        self.admit(1, timestamp)?;
        let tx = sql(self.connection.transaction())?;
        let total = count_for_daily(
            &tx,
            policy,
            &reply.chat_id,
            at.saturating_sub(i64::from(daily.hours) * 3600).max(cutoff),
            at,
        )?;
        summary_text(&mut reply, "daily", total, at)?;
        persist_summary(&tx, config, &reply, "daily", (at, at), timestamp)?;
        sql(tx.execute(
            "UPDATE notification_daily SET last_date=?2 WHERE chat_id=?1",
            params![reply.chat_id, date.to_string()],
        ))?;
        sql(tx.commit())
    }

    fn tick_live(
        &mut self,
        config: &garde_config::Config,
        pref: &NotificationPreferences,
        mut reply: Reply,
        now: DateTime<Utc>,
    ) -> Result<()> {
        if pref.live.is_none() {
            return Ok(());
        }
        let timestamp = now.timestamp();
        let cutoff = timestamp.saturating_sub(i64::from(self.retention.event_days) * 86400);
        let bucket: Option<(i64, i64, u32, u32)> = sql(self
            .connection
            .query_row(
                "SELECT start_at,end_at,total,minimum FROM notification_buckets
             WHERE chat_id=?1 AND end_at BETWEEN ?2 AND ?3 AND closed=0 ORDER BY end_at,start_at LIMIT 1",
                params![reply.chat_id, cutoff, timestamp],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional())?;
        let Some((start, end, total, minimum)) = bucket else {
            return Ok(());
        };
        if total >= minimum {
            self.admit(1, timestamp)?;
        }
        let tx = sql(self.connection.transaction())?;
        if total >= minimum {
            summary_text(&mut reply, "live", total, end)?;
            persist_summary(&tx, config, &reply, "live", (start, end), timestamp)?;
        }
        sql(tx.execute("UPDATE notification_buckets SET closed=1 WHERE chat_id=?1 AND start_at=?2 AND end_at=?3",
            params![reply.chat_id,start,end]))?;
        sql(tx.commit())
    }
}

pub(crate) async fn run(
    config: garde_config::Config,
    database: crate::runtime::Database,
) -> Result<()> {
    if config.notification_policy.is_none() {
        return Ok(());
    }
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut previous_error = None;
    loop {
        interval.tick().await;
        let error = database
            .call(|s| s.tick_notifications(crate::runtime::now()))
            .await
            .err();
        if error != previous_error
            && let Some(code) = error
        {
            eprintln!("{code}");
        }
        previous_error = error;
    }
}

pub(crate) fn render_status(
    config: &garde_config::Config,
    locale: &str,
    now: i64,
) -> Result<String> {
    use std::fmt::Write;
    let mut text = message(locale, "notification_title").to_owned();
    let Some(policy) = &config.notification_policy else {
        return Ok(format!("{}: {}", text, message(locale, "history_disabled")));
    };
    let at = DateTime::<Utc>::from_timestamp(now, 0).ok_or("service.clock")?;
    let connection = crate::delivery::inspect_connection(
        &config.state_dir,
        config.server_id.as_str(),
        config.owner()?,
    )?;
    let bot = crate::Bot::new(config.clone())?;
    for reply in bot.event_replies(&summary_event(now, "daily", config.server_id.clone())?)? {
        let pref = policy.preferences(&reply.chat_id);
        let _ = write!(
            text,
            "\n\n{} · {}",
            crate::isolate(&reply.chat_id),
            crate::isolate(pref.time_zone.name())
        );
        if !pref.quiet_hours.is_empty() {
            let _ = write!(
                text,
                "\n{}: {}",
                message(locale, "notification_quiet"),
                pref.quiet_hours
                    .iter()
                    .map(|window| crate::isolate(window))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        if pref.daily.is_some() {
            let local_today = at.with_timezone(&pref.time_zone).date_naive();
            let last: Option<String> = sql(connection
                .query_row(
                    "SELECT last_date FROM notification_daily WHERE chat_id=?1",
                    [&reply.chat_id],
                    |r| r.get(0),
                )
                .optional())?;
            let cutoff = now.saturating_sub(i64::from(config.retention.event_days) * 86400);
            let next = daily_date(pref, last.as_deref(), local_today, cutoff)?;
            let (_, stamp) = daily_slot(pref, next, cutoff)?;
            let due = DateTime::<Utc>::from_timestamp(stamp, 0).ok_or("service.clock")?;
            let _ = write!(
                text,
                "\n{}: {}",
                message(locale, "notification_daily"),
                crate::isolate(&due.with_timezone(&pref.time_zone).to_rfc3339())
            );
        }
        if let Some(live) = &pref.live {
            let _ = write!(
                text,
                "\n{}: {} s / {}",
                message(locale, "notification_live"),
                live.window_seconds,
                live.minimum_events
            );
        }
    }
    Ok(text)
}
