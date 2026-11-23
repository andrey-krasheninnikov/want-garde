use chrono::{DateTime, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use garde_contracts::{Id, Severity};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NotificationPolicy {
    pub default_rule: EventRule,
    pub rules: BTreeMap<String, EventRule>,
    pub default_preferences: Preferences,
    pub recipients: BTreeMap<String, Preferences>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EventRule {
    pub enabled: bool,
    pub severity: Option<Severity>,
    pub min_severity: Severity,
    pub immediate: bool,
    pub aggregate: bool,
    pub dedupe_seconds: u32,
}
impl Default for EventRule {
    fn default() -> Self {
        Self {
            enabled: true,
            severity: None,
            min_severity: Severity::Info,
            immediate: true,
            aggregate: false,
            dedupe_seconds: 60,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Preferences {
    pub min_severity: Severity,
    pub time_zone: Tz,
    pub quiet_hours: Vec<String>,
    pub critical_bypasses_quiet: bool,
    pub daily: Option<DailySummary>,
    pub live: Option<LiveSummary>,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            min_severity: Severity::Info,
            time_zone: chrono_tz::UTC,
            quiet_hours: Vec::new(),
            critical_bypasses_quiet: false,
            daily: None,
            live: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DailySummary {
    pub at: String,
    pub hours: u16,
}
impl Default for DailySummary {
    fn default() -> Self {
        Self {
            at: "14:00".into(),
            hours: 24,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LiveSummary {
    pub window_seconds: u32,
    pub minimum_events: u32,
}
impl Default for LiveSummary {
    fn default() -> Self {
        Self {
            window_seconds: 300,
            minimum_events: 3,
        }
    }
}

fn time(value: &str) -> Option<NaiveTime> {
    let bytes = value.as_bytes();
    if bytes.len() == 5
        && bytes[2] == b':'
        && bytes[..2].iter().chain(&bytes[3..]).all(u8::is_ascii_digit)
    {
        NaiveTime::parse_from_str(value, "%H:%M").ok()
    } else {
        None
    }
}
fn window(value: &str) -> Option<(NaiveTime, NaiveTime)> {
    let (start, end) = value.split_once('-')?;
    let (start, end) = (time(start)?, time(end)?);
    (start != end).then_some((start, end))
}

impl NotificationPolicy {
    pub fn valid(&self) -> bool {
        self.rules.len() <= 128
            && self.default_rule.dedupe_seconds <= 86400
            && self
                .rules
                .iter()
                .all(|(key, rule)| Id::new(key).is_ok() && rule.dedupe_seconds <= 86400)
            && self.recipients.len() <= 4
            && self.recipients.iter().all(|(id, p)| {
                id.parse::<i64>()
                    .ok()
                    .is_some_and(|n| n != 0 && n.unsigned_abs() < (1 << 52) && n.to_string() == *id)
                    && p.valid()
            })
            && self.default_preferences.valid()
    }
    pub fn preferences(&self, chat: &str) -> &Preferences {
        self.recipients
            .get(chat)
            .unwrap_or(&self.default_preferences)
    }
    pub fn rule(&self, rule_id: &str) -> &EventRule {
        self.rules.get(rule_id).unwrap_or(&self.default_rule)
    }
}
impl Preferences {
    fn valid(&self) -> bool {
        self.quiet_hours.len() <= 16
            && self.quiet_hours.iter().all(|s| window(s).is_some())
            && self
                .daily
                .as_ref()
                .is_none_or(|d| time(&d.at).is_some() && (1..=168).contains(&d.hours))
            && self.live.as_ref().is_none_or(|l| {
                (60..=3600).contains(&l.window_seconds) && (1..=1000).contains(&l.minimum_events)
            })
    }
    pub fn quiet(&self, now: DateTime<Utc>) -> bool {
        let local = now.with_timezone(&self.time_zone).time();
        self.quiet_hours
            .iter()
            .filter_map(|s| window(s))
            .any(|(start, end)| {
                if start < end {
                    start <= local && local < end
                } else {
                    start <= local || local < end
                }
            })
    }
    pub fn scheduled_at(&self, date: chrono::NaiveDate) -> Option<i64> {
        let time = time(&self.daily.as_ref()?.at)?;
        let start = date.and_time(time);
        // A clock jump can remove this wall-clock minute. Keep the schedule on the same date.
        (0..1440)
            .map_while(|offset| {
                let at = start.checked_add_signed(chrono::Duration::minutes(offset))?;
                (at.date() == date).then_some(at)
            })
            .find_map(|at| {
                self.time_zone
                    .from_local_datetime(&at)
                    .earliest()
                    .map(|t| t.timestamp())
            })
    }
}
