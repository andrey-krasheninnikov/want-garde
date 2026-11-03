use crate::{ContractError, HostSnapshot, Id, WireVersion};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    High,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct Classification {
    pub category: Id,
    pub severity: Severity,
    pub rule_id: Id,
    pub reason_key: Id,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub name: Id,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub version: WireVersion,
    pub id: Id,
    pub server_id: Id,
    pub observed_at: DateTime<Utc>,
    pub source: Id,
    pub classification: Classification,
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum IncidentState {
    // Empty struct variants enforce unknown-field rejection; Serde unit variants do not.
    Open {},
    Acknowledged {
        by: Id,
        at: DateTime<Utc>,
    },
    Closed {
        by: Id,
        at: DateTime<Utc>,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct Incident {
    pub version: WireVersion,
    pub id: Id,
    pub server_id: Id,
    pub event_ids: Vec<Id>,
    pub severity: Severity,
    pub state: IncidentState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub enum Capability {
    #[serde(rename = "status.read")]
    StatusRead,
    #[serde(rename = "bans.read")]
    BansRead,
    #[serde(rename = "services.read")]
    ServicesRead,
    #[serde(rename = "ports.read")]
    PortsRead,
    #[serde(rename = "events.read")]
    EventsRead,
    #[serde(rename = "incidents.read")]
    IncidentsRead,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub server_id: Id,
    pub resource_ids: Vec<Id>,
    pub allowed_chat_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    pub version: WireVersion,
    pub principal_id: Id,
    pub scope: Scope,
    pub capabilities: Vec<Capability>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum OperationResult {
    ServicesStatus {
        server_id: Id,
        health: Box<crate::ServicesHealth>,
    },
    PortsStatus {
        server_id: Id,
        health: Box<crate::PortsHealth>,
    },
    Fail2banStatus {
        server_id: Id,
        snapshot: Box<crate::BansSnapshot>,
    },
    ServerStatus {
        server_id: Id,
        product_version: String,
        snapshot: Box<HostSnapshot>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum JobState {
    AwaitingApproval {},
    Queued {},
    Running {},
    Succeeded { result: OperationResult },
    Failed { error: ContractError },
    Cancelled {},
    TimedOut {},
    NeedsReconciliation { reason_key: Id },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct Job {
    pub version: WireVersion,
    pub id: Id,
    pub request_id: Id,
    pub server_id: Id,
    pub state: JobState,
}
