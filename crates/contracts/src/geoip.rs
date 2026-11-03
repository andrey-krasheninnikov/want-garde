use crate::{Id, WireVersion};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum GeoIpState {
    Missing,
    Cached,
    Retrieved,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct GeoIpDetails {
    pub country_code: Option<String>,
    pub asn: Option<u32>,
    pub organization: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct EventGeoIp {
    pub version: WireVersion,
    pub server_id: Id,
    pub event_id: Id,
    pub ip: Option<String>,
    pub state: GeoIpState,
    pub reason_key: Id,
    pub provider: String,
    pub fetched_at: Option<DateTime<Utc>>,
    pub details: Option<GeoIpDetails>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct GeoIpHealth {
    pub version: WireVersion,
    pub server_id: Id,
    pub enabled: bool,
    pub queued: u32,
    pub cache_entries: u32,
    pub requests_today: u32,
    pub skipped: u32,
    pub reason_key: Id,
    pub cooldown_until: Option<DateTime<Utc>>,
}
