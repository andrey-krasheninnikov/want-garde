use crate::{Id, WireVersion};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum SshSourceKind {
    Unselected,
    Journald,
    File,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum SshStatus {
    Starting,
    Healthy,
    Degraded,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct SourceHealth {
    pub version: WireVersion,
    pub server_id: Id,
    pub observed_at: DateTime<Utc>,
    pub source: SshSourceKind,
    pub state: SshStatus,
    pub reason_key: Option<Id>,
    pub file_path: Option<String>,
    pub coverage_started_at: Option<DateTime<Utc>>,
    pub checked_at: Option<DateTime<Utc>>,
    pub last_record_at: Option<DateTime<Utc>>,
    pub skipped_records: u32,
    pub gap_count: u32,
    pub last_gap: Option<Id>,
    pub last_gap_at: Option<DateTime<Utc>>,
}

pub type SshHealth = SourceHealth;
