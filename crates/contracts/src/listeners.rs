use crate::{Id, WireVersion};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(rename_all = "snake_case")]
pub enum AddressFamily {
    Ipv4,
    Ipv6,
}
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(rename_all = "snake_case")]
pub enum ListenerProtocol {
    Tcp,
    Udp,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct Endpoint {
    pub family: AddressFamily,
    pub protocol: ListenerProtocol,
    pub address: String,
    pub port: u16,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct ListenerProcess {
    pub pid: u32,
    pub name: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct Listener {
    pub endpoint: Endpoint,
    pub processes: Vec<ListenerProcess>,
    pub process_reason: Option<Id>,
    pub ignored: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct SuspiciousListener {
    pub endpoint: Endpoint,
    pub process_name: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct PortsSnapshot {
    pub observed_at: DateTime<Utc>,
    pub listeners: Vec<Listener>,
    pub complete: bool,
    pub processes_complete: bool,
    pub ignored_count: u32,
    pub ephemeral_range: [u16; 2],
    pub range_fallback: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct PortBaseline {
    pub revision: u32,
    pub hash: String,
    pub filter_hash: String,
    pub accepted_at: DateTime<Utc>,
    pub endpoints: Vec<Endpoint>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct PortsHealth {
    pub version: WireVersion,
    pub server_id: Id,
    pub enabled: bool,
    pub stale: bool,
    pub snapshot: Option<PortsSnapshot>,
    pub baseline: Option<PortBaseline>,
    pub added: Vec<Endpoint>,
    pub removed: Vec<Endpoint>,
    pub suspicious: Vec<SuspiciousListener>,
    pub problems: Vec<Id>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct BaselinePreview {
    pub version: WireVersion,
    pub server_id: Id,
    pub observed_at: DateTime<Utc>,
    pub hash: String,
    pub previous_hash: Option<String>,
    pub endpoints: Vec<Endpoint>,
    pub added: Vec<Endpoint>,
    pub removed: Vec<Endpoint>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ListenerReadResponse {
    Snapshot { ipv4: String, ipv6: String },
    Unavailable { reason_key: Id },
}
