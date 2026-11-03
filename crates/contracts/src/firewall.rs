use crate::{Id, WireVersion};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct JailBans {
    pub jail: Id,
    pub current_banned: Option<u32>,
    pub addresses: Vec<String>,
    pub reason_key: Option<Id>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct BansSnapshot {
    pub version: WireVersion,
    pub observed_at: DateTime<Utc>,
    pub jails: Vec<JailBans>,
    pub partial: bool,
    pub log_target: Option<String>,
    pub log_level: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum HelperResponse {
    Snapshot { snapshot: BansSnapshot },
    Unavailable { reason_key: Id },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum FirewallBackend {
    #[default]
    Auto,
    Ufw,
    Firewalld,
    Nftables,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct FirewallHealth {
    pub version: WireVersion,
    pub server_id: Id,
    pub observed_at: DateTime<Utc>,
    pub backend: FirewallBackend,
    pub fail2ban: crate::SourceHealth,
    pub firewall: crate::SourceHealth,
}
