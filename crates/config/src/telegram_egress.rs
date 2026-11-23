use serde::{Deserialize, Deserializer, Serialize};
use std::{
    net::SocketAddr,
    path::{Component, Path, PathBuf},
};

pub const MAX_TELEGRAM_ROUTES: usize = 4;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelegramEgress {
    pub routes: Vec<TelegramRoute>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TelegramRoute {
    Direct {},
    Vless {
        proxy_credentials_file: PathBuf,
    },
    Wireguard {
        interface: String,
        dns_server: SocketAddr,
    },
    Tailscale {
        interface: String,
        dns_server: SocketAddr,
    },
}

pub(crate) fn deserialize_section<'de, D>(
    value: D,
) -> std::result::Result<Option<TelegramEgress>, D::Error>
where
    D: Deserializer<'de>,
{
    TelegramEgress::deserialize(value).map(Some)
}

impl TelegramEgress {
    pub(crate) fn valid(&self, reserved: &[&Path]) -> bool {
        (1..=MAX_TELEGRAM_ROUTES).contains(&self.routes.len())
            && self.routes.iter().enumerate().all(|(index, route)| {
                !self.routes[..index].contains(route)
                    && match route {
                        TelegramRoute::Direct {} => true,
                        TelegramRoute::Vless {
                            proxy_credentials_file: path,
                        } => {
                            path.is_absolute()
                                && path.as_os_str().len() <= 4096
                                && !path.components().any(|c| c == Component::ParentDir)
                                && !reserved.contains(&path.as_path())
                        }
                        TelegramRoute::Wireguard {
                            interface,
                            dns_server,
                        }
                        | TelegramRoute::Tailscale {
                            interface,
                            dns_server,
                        } => {
                            (1..=15).contains(&interface.len())
                                && interface.as_bytes()[0].is_ascii_alphanumeric()
                                && interface
                                    .bytes()
                                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
                                && dns_server.port() != 0
                                && !dns_server.ip().is_unspecified()
                                && !dns_server.ip().is_multicast()
                        }
                    }
            })
    }
}
