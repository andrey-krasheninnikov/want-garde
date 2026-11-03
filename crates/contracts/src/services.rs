use crate::{Id, Measurement, WireVersion};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct ServiceStatus {
    pub load_state: String,
    pub active_state: String,
    pub sub_state: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct ServiceObservation {
    pub unit: String,
    pub state: Measurement<ServiceStatus>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct ServicesHealth {
    pub version: WireVersion,
    pub server_id: Id,
    pub checked_at: Option<DateTime<Utc>>,
    pub enabled: bool,
    pub stale: bool,
    pub services: Vec<ServiceObservation>,
    pub down: Vec<String>,
}
