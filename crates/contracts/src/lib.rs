mod listeners;
pub use listeners::*;
mod services;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
pub use services::*;
use ts_rs::TS;

pub const PRODUCT_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const MAX_REQUEST_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema, TS)]
#[serde(transparent)]
pub struct Id(#[schemars(regex(pattern = "^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$"))] String);

impl Id {
    pub fn new(value: impl Into<String>) -> Result<Self, &'static str> {
        let value = value.into();
        let mut bytes = value.bytes();
        if value.len() > 128
            || !bytes
                .next()
                .is_some_and(|byte| byte.is_ascii_alphanumeric())
            || !bytes.all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
        {
            return Err("invalid_identifier");
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
pub enum WireVersion {
    #[serde(rename = "1")]
    V1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum Action {
    #[serde(rename = "server.status")]
    ServerStatus {},
    #[serde(rename = "fail2ban.status")]
    Fail2banStatus {},
    #[serde(rename = "services.status")]
    ServicesStatus {},
    #[serde(rename = "ports.status")]
    PortsStatus {},
    #[serde(rename = "xui.status")]
    XuiStatus {},
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct OperationRequest {
    pub version: WireVersion,
    pub request_id: Id,
    pub server_id: Id,
    pub action: Action,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidRequest,
    AccessDenied,
    Unavailable,
    Conflict,
    AuditUnavailable,
    NotImplemented,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct ContractError {
    pub code: ErrorCode,
    pub message_key: Id,
}

pub fn decode_request(input: &str) -> Result<OperationRequest, ContractError> {
    let invalid = || ContractError {
        code: ErrorCode::InvalidRequest,
        message_key: Id("request.invalid".into()),
    };
    if input.len() > MAX_REQUEST_BYTES {
        return Err(invalid());
    }
    serde_json::from_str(input).map_err(|_| invalid())
}

mod models;
pub use models::*;

mod export;
pub use export::contract_artifacts;

mod status;
pub use status::*;
mod delivery;
pub use delivery::*;

mod ssh;
pub use ssh::*;

mod firewall;
pub use firewall::*;

mod xui;
pub use xui::*;
