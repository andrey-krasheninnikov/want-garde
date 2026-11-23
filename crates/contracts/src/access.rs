use crate::Capability;
use serde::{Deserialize, Serialize};

pub const SOCKET_NAME: &str = "access.sock";
pub const MAX_REQUEST: usize = 1024;
pub const MAX_RESPONSE: usize = 8192;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplyMode {
    #[default]
    Private,
    Group,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessGrant {
    pub user: i64,
    pub chat: i64,
    pub capability: Capability,
}

impl AccessGrant {
    pub fn valid(self) -> bool {
        valid_actor(self.user, self.chat) && self.capability != Capability::IncidentsRead
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AccessCommand {
    List {},
    Grant { grant: AccessGrant },
    Revoke { grant: AccessGrant },
    Group { chat: i64, mode: ReplyMode },
}

impl AccessCommand {
    pub fn parse(words: &[&str]) -> Option<Self> {
        let command = match words {
            ["list"] => Self::List {},
            [action @ ("grant" | "revoke"), user, chat, capability] => {
                let grant = AccessGrant {
                    user: parse_id(user)?,
                    chat: parse_id(chat)?,
                    capability: capability.parse().ok()?,
                };
                if *action == "grant" {
                    Self::Grant { grant }
                } else {
                    Self::Revoke { grant }
                }
            }
            ["group", chat, mode @ ("private" | "group")] => Self::Group {
                chat: parse_id(chat)?,
                mode: if *mode == "group" {
                    ReplyMode::Group
                } else {
                    ReplyMode::Private
                },
            },
            _ => return None,
        };
        command.valid().then_some(command)
    }

    pub fn valid(self) -> bool {
        match self {
            Self::List {} => true,
            Self::Grant { grant } | Self::Revoke { grant } => grant.valid(),
            Self::Group { chat, .. } => valid_chat(chat) && chat < 0,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalRequest {
    pub server_id: crate::Id,
    pub owner_user_id: String,
    pub command: AccessCommand,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AccessOutput {
    List {
        grants: Vec<AccessGrant>,
        group_chats: Vec<i64>,
    },
    Updated {},
}

pub fn valid_chat(id: i64) -> bool {
    id != 0 && id.unsigned_abs() < (1 << 52)
}

pub fn valid_actor(user: i64, chat: i64) -> bool {
    user > 0 && valid_chat(user) && valid_chat(chat) && (chat < 0 || chat == user)
}

fn parse_id(value: &str) -> Option<i64> {
    let id: i64 = value.parse().ok()?;
    (id.to_string() == value).then_some(id)
}
