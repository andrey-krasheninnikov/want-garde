use crate::{
    Bot, StatusSource,
    runtime::{Database, now},
};
use garde_config::{Config, Result};
use garde_contracts::{
    Classification, Event, Evidence, HostAlert, HostHealth, HostLevel, HostSnapshot, Id,
    Measurement, Severity, WireVersion,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, time::Duration};

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MetricState {
    level: HostLevel,
    pending: u8,
    problem: Option<Id>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    version: u8,
    sequence: u64,
    checked_at: i64,
    snapshot: HostSnapshot,
    metrics: BTreeMap<String, MetricState>,
}
impl State {
    fn decode(raw: &str) -> Result<Self> {
        if raw.len() > 65536 {
            return Err("host.state");
        }
        let state: Self = serde_json::from_str(raw).map_err(|_| "host.state")?;
        if state.version != 1
            || state.metrics.len() > 20
            || state.checked_at < 0
            || state.snapshot.disks.len() > 8
            || state.metrics.values().any(|m| m.pending > 2)
        {
            return Err("host.state");
        }
        Ok(state)
    }
}
struct Metric {
    name: &'static str,
    resource: Option<String>,
    value: std::result::Result<f64, Id>,
    warning: f64,
    critical: Option<f64>,
    hysteresis: f64,
    confirmations: u8,
    unit: &'static str,
}
impl Metric {
    fn key(&self) -> String {
        format!("{}:{}", self.name, self.resource.as_deref().unwrap_or(""))
    }
    fn target(&self, value: f64, old: HostLevel) -> HostLevel {
        if self.critical.is_some_and(|threshold| {
            value >= threshold
                || (old == HostLevel::Critical && value > threshold - self.hysteresis)
        }) {
            HostLevel::Critical
        } else if value >= self.warning
            || (old != HostLevel::Normal && value > self.warning - self.hysteresis)
        {
            HostLevel::Warning
        } else {
            HostLevel::Normal
        }
    }
}
fn value<T>(
    measurement: &Measurement<T>,
    convert: impl FnOnce(&T) -> Option<f64>,
) -> std::result::Result<f64, Id> {
    match measurement {
        Measurement::Available { value } => convert(value)
            .filter(|v| v.is_finite() && *v >= 0.0)
            .ok_or_else(|| Id::new("host.measurement_invalid").expect("static_identifier")),
        Measurement::Unavailable { reason_key } => Err(reason_key.clone()),
    }
}
fn metrics(snapshot: &HostSnapshot, config: &Config) -> Vec<Metric> {
    let c = &config.host_health;
    let mut metrics = vec![
        Metric {
            name: "cpu",
            resource: None,
            value: value(&snapshot.cpu_usage_percent, |v| (*v <= 100.0).then_some(*v)),
            warning: c.cpu_warn_percent,
            critical: None,
            hysteresis: 5.0,
            confirmations: 2,
            unit: "%",
        },
        Metric {
            name: "memory",
            resource: None,
            value: value(&snapshot.memory, |v| {
                (v.total_bytes > 0
                    && v.available_bytes <= v.total_bytes
                    && v.total_bytes < (1 << 53))
                    .then(|| {
                        100.0 * (v.total_bytes - v.available_bytes) as f64 / v.total_bytes as f64
                    })
            }),
            warning: c.memory_warn_percent,
            critical: None,
            hysteresis: 5.0,
            confirmations: 2,
            unit: "%",
        },
        Metric {
            name: "load",
            resource: None,
            value: value(&snapshot.load_per_core, |v| Some(v[0])),
            warning: c.load_warn_per_core,
            critical: None,
            hysteresis: 0.2,
            confirmations: 2,
            unit: "/core",
        },
    ];
    for path in &c.disk_paths {
        let path = path.to_string_lossy().into_owned();
        let disk = snapshot.disks.iter().find(|d| d.path == path);
        let missing = || Err(Id::new("host.filesystem_unavailable").expect("static_identifier"));
        metrics.push(Metric {
            name: "disk",
            resource: Some(path.clone()),
            value: disk
                .map(|d| {
                    value(&d.space, |v| {
                        (v.total_bytes > 0
                            && v.used_bytes <= v.total_bytes
                            && v.available_bytes <= v.total_bytes - v.used_bytes
                            && v.total_bytes < (1 << 53))
                            .then(|| 100.0 * v.used_bytes as f64 / v.total_bytes as f64)
                    })
                })
                .unwrap_or_else(missing),
            warning: c.disk_warn_percent,
            critical: Some(c.disk_critical_percent),
            hysteresis: 5.0,
            confirmations: 1,
            unit: "%",
        });
        metrics.push(Metric {
            name: "inode",
            resource: Some(path),
            value: disk
                .map(|d| {
                    value(&d.inodes, |v| {
                        (v.total > 0 && v.available <= v.total && v.total < (1 << 53))
                            .then(|| 100.0 * (v.total - v.available) as f64 / v.total as f64)
                    })
                })
                .unwrap_or_else(missing),
            warning: c.inode_warn_percent,
            critical: Some(c.inode_critical_percent),
            hysteresis: 5.0,
            confirmations: 1,
            unit: "%",
        });
    }
    if c.check_reboot_required {
        metrics.push(Metric {
            name: "reboot",
            resource: None,
            value: value(&snapshot.reboot_required, |v| Some(u8::from(*v) as f64)),
            warning: 1.0,
            critical: None,
            hysteresis: 1.0,
            confirmations: 1,
            unit: "",
        });
    }
    metrics
}

pub struct Monitor {
    config: Config,
    database: Database,
    fresh: bool,
}
impl Monitor {
    pub fn new(config: Config, database: Database) -> Self {
        Self {
            config,
            database,
            fresh: true,
        }
    }
    pub async fn observe(&mut self, snapshot: HostSnapshot) -> Result<()> {
        if !self.config.host_health.enabled {
            return Ok(());
        }
        let previous = self.database.call(|store| store.host_state()).await?;
        let mut state = previous
            .as_deref()
            .map(State::decode)
            .transpose()?
            .unwrap_or(State {
                version: 1,
                sequence: 0,
                checked_at: now(),
                snapshot: snapshot.clone(),
                metrics: BTreeMap::new(),
            });
        if self.fresh
            || snapshot
                .observed_at
                .signed_duration_since(state.snapshot.observed_at)
                .num_seconds()
                > i64::from(self.config.host_health.interval_seconds) * 2
            || snapshot.observed_at < state.snapshot.observed_at
        {
            for metric in state.metrics.values_mut() {
                metric.pending = 0;
            }
        }
        let bot = Bot::new(self.config.clone())?;
        let mut publications = Vec::new();
        let mut next = BTreeMap::new();
        for metric in metrics(&snapshot, &self.config) {
            let key = metric.key();
            let mut saved = state.metrics.remove(&key).unwrap_or_default();
            let mut changes = Vec::new();
            match &metric.value {
                Err(reason) => {
                    saved.pending = 0;
                    if saved.problem.as_ref() != Some(reason) {
                        changes.push((
                            "host_source_unavailable",
                            Severity::Warning,
                            Some(reason.clone()),
                        ));
                    }
                    saved.problem = Some(reason.clone());
                }
                Ok(value) => {
                    if saved.problem.take().is_some() {
                        changes.push(("host_source_recovered", Severity::Info, None));
                    }
                    let target = metric.target(*value, saved.level);
                    if target > saved.level {
                        saved.pending = (saved.pending + 1).min(2);
                        if saved.pending >= metric.confirmations {
                            saved.pending = 0;
                            saved.level = target;
                            changes.push((
                                if target == HostLevel::Critical {
                                    "host_critical"
                                } else {
                                    "host_warning"
                                },
                                if target == HostLevel::Critical {
                                    Severity::Critical
                                } else {
                                    Severity::Warning
                                },
                                None,
                            ));
                        }
                    } else {
                        saved.pending = 0;
                        if target < saved.level {
                            saved.level = target;
                            changes.push((
                                if target == HostLevel::Normal {
                                    "host_recovered"
                                } else {
                                    "host_warning"
                                },
                                if target == HostLevel::Normal {
                                    Severity::Info
                                } else {
                                    Severity::Warning
                                },
                                None,
                            ));
                        }
                    }
                }
            }
            for (reason, severity, problem) in changes {
                state.sequence = state.sequence.checked_add(1).ok_or("host.state")?;
                let mut evidence = vec![Evidence {
                    name: Id::new("host_metric")?,
                    value: metric.name.into(),
                }];
                if let Some(path) = &metric.resource {
                    evidence.push(Evidence {
                        name: Id::new("host_path")?,
                        value: path.clone(),
                    });
                }
                if let Ok(value) = metric.value {
                    evidence.push(Evidence {
                        name: Id::new("host_value")?,
                        value: format!("{value:.2}{}", metric.unit),
                    });
                    evidence.push(Evidence {
                        name: Id::new("host_threshold")?,
                        value: format!(
                            "{:.2}{}",
                            if saved.level == HostLevel::Critical {
                                metric.critical.unwrap_or(metric.warning)
                            } else {
                                metric.warning
                            },
                            metric.unit
                        ),
                    });
                }
                if let Some(problem) = problem {
                    evidence.push(Evidence {
                        name: Id::new("host_reason")?,
                        value: problem.as_str().into(),
                    });
                }
                let event = Event {
                    version: WireVersion::V1,
                    id: Id::new(format!("host-{}", state.sequence))?,
                    server_id: self.config.server_id.clone(),
                    observed_at: snapshot.observed_at,
                    source: Id::new("host")?,
                    classification: Classification {
                        category: Id::new("host.resources")?,
                        severity,
                        rule_id: Id::new(format!("host.{}.{reason}", metric.name))?,
                        reason_key: Id::new(reason)?,
                    },
                    evidence,
                };
                let replies = bot.event_replies(&event)?;
                publications.push((event, replies));
            }
            next.insert(key, saved);
        }
        state.metrics = next;
        state.checked_at = now();
        state.snapshot = snapshot;
        let raw = serde_json::to_string(&state).map_err(|_| "host.state")?;
        self.database
            .call(move |store| store.publish_host(previous.as_deref(), &raw, &publications, now()))
            .await?;
        self.fresh = false;
        Ok(())
    }
    pub async fn run(&mut self, source: &impl StatusSource) -> Result<()> {
        if !self.config.host_health.enabled {
            return std::future::pending().await;
        }
        let mut tick = tokio::time::interval(Duration::from_secs(
            self.config.host_health.interval_seconds as u64,
        ));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last_error = None;
        let mut pending = None;
        loop {
            if pending.is_none() {
                tick.tick().await;
                match tokio::time::timeout(Duration::from_secs(5), source.snapshot()).await {
                    Ok(snapshot) => pending = Some(snapshot),
                    Err(_) => {
                        if last_error != Some("host.snapshot_timeout") {
                            eprintln!("host.snapshot_timeout");
                            last_error = Some("host.snapshot_timeout");
                        }
                        continue;
                    }
                }
            }
            let snapshot = pending.as_ref().expect("pending_snapshot").clone();
            match self.observe(snapshot).await {
                Ok(()) => {
                    last_error = None;
                    pending = None;
                }
                Err(code) => {
                    if last_error != Some(code) {
                        eprintln!("{code}");
                        last_error = Some(code);
                    }
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }
    }
}

pub(crate) fn health(config: &Config, raw: Option<&str>, time: i64) -> Result<HostHealth> {
    let mut health = HostHealth {
        version: WireVersion::V1,
        server_id: config.server_id.clone(),
        observed_at: chrono::DateTime::from_timestamp(time, 0).ok_or("host.clock")?,
        checked_at: None,
        enabled: config.host_health.enabled,
        stale: true,
        snapshot: None,
        alerts: vec![],
        reason_key: None,
    };
    let state = match raw.map(State::decode).transpose() {
        Ok(Some(s)) => s,
        Ok(None) => {
            health.reason_key = Some(Id::new("host.starting")?);
            return Ok(health);
        }
        Err(code) => {
            health.reason_key = Some(Id::new(code)?);
            return Ok(health);
        }
    };
    health.checked_at = chrono::DateTime::from_timestamp(state.checked_at, 0);
    health.stale = time < state.snapshot.observed_at.timestamp()
        || time.saturating_sub(state.snapshot.observed_at.timestamp())
            > i64::from(config.host_health.interval_seconds) * 2
        || time < state.checked_at
        || time.saturating_sub(state.checked_at)
            > i64::from(config.host_health.interval_seconds) * 2;
    for metric in metrics(&state.snapshot, config) {
        let saved = state.metrics.get(&metric.key());
        let problem = metric.value.err();
        let level = saved.map(|s| s.level).unwrap_or_default();
        if problem.is_some() || level != HostLevel::Normal {
            health.alerts.push(HostAlert {
                metric: Id::new(metric.name)?,
                resource: metric.resource,
                level,
                reason_key: problem,
            });
        }
    }
    health.snapshot = Some(state.snapshot);
    Ok(health)
}

pub fn healthy(health: &HostHealth) -> bool {
    health.enabled && !health.stale && health.reason_key.is_none() && health.alerts.is_empty()
}

pub(crate) fn validate_state(raw: &str) -> Result<()> {
    State::decode(raw).map(|_| ())
}
