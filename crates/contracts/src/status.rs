use crate::Id;
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Measurement<T> {
    Available { value: T },
    Unavailable { reason_key: Id },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct ByteUsage {
    #[ts(type = "number")]
    pub total_bytes: u64,
    #[ts(type = "number")]
    pub available_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct HostSnapshot {
    pub observed_at: DateTime<Utc>,
    pub hostname: Measurement<String>,
    pub os: Measurement<String>,
    pub kernel: Measurement<String>,
    pub uptime_seconds: Measurement<u32>,
    pub cpu_usage_percent: Measurement<f64>,
    pub load_average: Measurement<[f64; 3]>,
    pub memory: Measurement<ByteUsage>,
    pub root_disk: Measurement<ByteUsage>,
    #[serde(default)]
    pub cpu_count: Measurement<u32>,
    #[serde(default)]
    pub load_per_core: Measurement<[f64; 3]>,
    #[serde(default)]
    pub disks: Vec<DiskMeasurement>,
    #[serde(default)]
    pub reboot_required: Measurement<bool>,
}

impl<T> Default for Measurement<T> {
    fn default() -> Self {
        Self::Unavailable {
            reason_key: Id::new("source.unavailable").expect("static_identifier"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct DiskUsage {
    #[ts(type = "number")]
    pub total_bytes: u64,
    #[ts(type = "number")]
    pub used_bytes: u64,
    #[ts(type = "number")]
    pub available_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct InodeUsage {
    #[ts(type = "number")]
    pub total: u64,
    #[ts(type = "number")]
    pub available: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct DiskMeasurement {
    pub path: String,
    pub space: Measurement<DiskUsage>,
    pub inodes: Measurement<InodeUsage>,
}

#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    JsonSchema,
    TS,
)]
#[serde(rename_all = "snake_case")]
pub enum HostLevel {
    #[default]
    Normal,
    Warning,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct HostAlert {
    pub metric: Id,
    pub resource: Option<String>,
    pub level: HostLevel,
    pub reason_key: Option<Id>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct HostHealth {
    pub version: crate::WireVersion,
    pub server_id: Id,
    pub observed_at: DateTime<Utc>,
    pub checked_at: Option<DateTime<Utc>>,
    pub enabled: bool,
    pub stale: bool,
    pub snapshot: Option<HostSnapshot>,
    pub alerts: Vec<HostAlert>,
    pub reason_key: Option<Id>,
}
