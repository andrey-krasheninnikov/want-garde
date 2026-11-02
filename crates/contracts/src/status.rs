use crate::Id;
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
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
}
