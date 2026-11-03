use crate::{Evidence, Id, Measurement, WireVersion};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct XuiPanelRow {
    pub key: String,
    pub fields: BTreeMap<String, String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct XuiClient {
    pub key: String,
    pub label: String,
    #[ts(type = "number")]
    pub inbound_id: i64,
    pub enabled: bool,
    #[ts(type = "number | null")]
    pub last_online: Option<i64>,
    #[ts(type = "number")]
    pub traffic_bytes: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct XuiSnapshot {
    pub source_id: String,
    pub version: WireVersion,
    pub observed_at: DateTime<Utc>,
    pub schema: Option<Id>,
    pub panel: Measurement<Vec<XuiPanelRow>>,
    pub clients: Measurement<Vec<XuiClient>>,
    pub connectivity_available: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct XuiFilePosition {
    pub slot: u8,
    #[ts(type = "number")]
    pub device: u64,
    #[ts(type = "number")]
    pub inode: u64,
    #[ts(type = "number")]
    pub generation: u64,
    #[ts(type = "number")]
    pub offset: u64,
    pub anchor: String,
    pub discard: bool,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct XuiCheckpoint {
    pub source_id: Option<String>,
    pub journal_started: bool,
    pub journal_cursor: Option<String>,
    #[ts(type = "number")]
    pub journal_time: u64,
    pub files: Vec<XuiFilePosition>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct XuiSourceHealth {
    pub source: Id,
    pub reason_key: Option<Id>,
    pub gap: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct XuiLogRecord {
    pub id: Id,
    pub observed_at: DateTime<Utc>,
    pub reason_key: Id,
    pub evidence: Vec<Evidence>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct XuiLogBatch {
    pub checkpoint: XuiCheckpoint,
    pub sources: Vec<XuiSourceHealth>,
    pub records: Vec<XuiLogRecord>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum XuiReadRequest {
    Snapshot {
        version: WireVersion,
    },
    Logs {
        version: WireVersion,
        checkpoint: XuiCheckpoint,
    },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum XuiReadResponse {
    Unavailable {
        version: WireVersion,
        reason_key: Id,
    },
    Snapshot {
        version: WireVersion,
        snapshot: XuiSnapshot,
    },
    Logs {
        version: WireVersion,
        batch: XuiLogBatch,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum XuiConnectionState {
    Online,
    Offline,
    Disabled,
    Unknown,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct XuiClientStatus {
    pub client: XuiClient,
    pub state: XuiConnectionState,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct XuiHealth {
    pub version: WireVersion,
    pub server_id: Id,
    pub enabled: bool,
    pub checked_at: Option<DateTime<Utc>>,
    pub logs_checked_at: Option<DateTime<Utc>>,
    pub stale: bool,
    pub schema: Option<Id>,
    pub sources: Vec<XuiSourceHealth>,
    pub gaps: u32,
    pub clients: Vec<XuiClientStatus>,
}
pub fn validate_xui_checkpoint(checkpoint: &XuiCheckpoint) -> Result<(), &'static str> {
    let mut slots = std::collections::BTreeSet::new();
    if checkpoint
        .source_id
        .as_ref()
        .is_some_and(|s| s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
        || checkpoint.files.len() > 4
        || checkpoint.journal_cursor.as_ref().is_some_and(|s| {
            s.is_empty() || s.len() > 4096 || !s.is_ascii() || s.chars().any(char::is_control)
        })
        || checkpoint.files.iter().any(|p| {
            p.slot > 3
                || !slots.insert(p.slot)
                || p.anchor.len() != 64
                || !p.anchor.bytes().all(|b| b.is_ascii_hexdigit())
        })
    {
        return Err("xui.checkpoint");
    }
    Ok(())
}
