use crate::{DeliveryHealth, Id, Severity, WireVersion};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct EventSummary {
    pub id: Id,
    pub observed_at: DateTime<Utc>,
    pub source: Id,
    pub severity: Severity,
    pub reason_key: Id,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct HistoryPage {
    pub version: WireVersion,
    pub events: Vec<EventSummary>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct EventCount {
    pub source: Id,
    pub severity: Severity,
    pub count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct HistoryStats {
    pub version: WireVersion,
    pub hours: u16,
    pub total: u32,
    pub groups: Vec<EventCount>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct SourceReport {
    pub source: Id,
    pub enabled: bool,
    pub healthy: bool,
    pub reason_key: Option<Id>,
    pub gaps: u32,
    pub last_gap_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct StorageReport {
    #[ts(type = "number")]
    pub database_bytes: u64,
    #[ts(type = "number")]
    pub database_limit_bytes: u64,
    #[ts(type = "number")]
    pub occupied_bytes: u64,
    #[ts(type = "number")]
    pub free_disk_bytes: u64,
    pub pending_limit: u32,
    #[ts(type = "number")]
    pub result_bytes: u64,
    #[ts(type = "number")]
    pub result_limit_bytes: u64,
    pub result_count: u32,
    pub action_audit_records: u32,
    pub process_audit_records: u32,
    pub backpressure: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct InstallationHealth {
    pub version: WireVersion,
    pub degraded: bool,
    pub sources: Vec<SourceReport>,
    pub delivery: DeliveryHealth,
    pub storage: StorageReport,
}
