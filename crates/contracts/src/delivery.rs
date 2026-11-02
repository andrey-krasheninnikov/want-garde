use crate::{Id, WireVersion};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    Pending,
    InFlight,
    Delivered,
    DeadLetter,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct DeliveryProblem {
    pub delivery_id: Id,
    pub event_id: Option<Id>,
    pub state: DeliveryState,
    pub attempts: u32,
    pub reason_key: Id,
    pub uncertain: bool,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct DeliveryHealth {
    pub version: WireVersion,
    pub server_id: Id,
    pub observed_at: DateTime<Utc>,
    pub pending: u32,
    pub in_flight: u32,
    pub delivered: u32,
    pub dead_letter: u32,
    pub rate_limited: u32,
    pub oldest_pending_at: Option<DateTime<Utc>>,
    pub pruned_records: u32,
    pub problems: Vec<DeliveryProblem>,
}
