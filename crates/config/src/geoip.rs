use serde::{Deserialize, Serialize};
use std::path::{Component, PathBuf};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GeoIpConfig {
    pub enabled: bool,
    pub token_file: Option<PathBuf>,
}
impl GeoIpConfig {
    pub fn valid(&self) -> bool {
        (!self.enabled || self.token_file.is_some())
            && self.token_file.as_ref().is_none_or(|p| {
                p.is_absolute()
                    && p.as_os_str().len() <= 4096
                    && !p.components().any(|c| matches!(c, Component::ParentDir))
            })
    }
}
