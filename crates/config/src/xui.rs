use serde::{Deserialize, Serialize};
use std::path::PathBuf;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct XuiConfig {
    pub enabled: bool,
    pub interval_seconds: u64,
    pub client_disconnect_after_seconds: u64,
    pub helper_socket: PathBuf,
}
impl Default for XuiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_seconds: 30,
            client_disconnect_after_seconds: 180,
            helper_socket: "/run/want-garde-xui.sock".into(),
        }
    }
}
impl XuiConfig {
    pub fn valid(&self) -> bool {
        (10..=3600).contains(&self.interval_seconds)
            && (30..=86400).contains(&self.client_disconnect_after_seconds)
            && self.helper_socket.is_absolute()
            && self.helper_socket.as_os_str().len() <= 4096
            && !self
                .helper_socket
                .components()
                .any(|p| matches!(p, std::path::Component::ParentDir))
    }
}
