use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServicesConfig {
    pub units: Vec<String>,
    pub interval_seconds: u64,
}
impl Default for ServicesConfig {
    fn default() -> Self {
        Self {
            units: vec![],
            interval_seconds: 60,
        }
    }
}
pub fn valid_unit(unit: &str) -> bool {
    unit.ends_with(".service")
        && unit.len() <= 255
        && unit.len() > 8
        && unit.as_bytes()[0].is_ascii_alphanumeric()
        && unit
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.@:-".contains(&b))
        && !unit.ends_with("@.service")
}
impl ServicesConfig {
    pub fn valid(&self) -> bool {
        self.units.len() <= 32
            && (10..=3600).contains(&self.interval_seconds)
            && self.units.iter().all(|u| valid_unit(u))
            && self
                .units
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                == self.units.len()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PortsConfig {
    pub enabled: bool,
    pub interval_seconds: u64,
    pub ignore_patterns: Vec<String>,
    pub ignore_ephemeral_udp: bool,
    pub ephemeral_range: Option<[u16; 2]>,
    pub important_udp_ports: Vec<u16>,
    pub expected_process_names: Vec<String>,
}
impl Default for PortsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_seconds: 120,
            ignore_patterns: vec![],
            ignore_ephemeral_udp: true,
            ephemeral_range: None,
            important_udp_ports: vec![53, 67, 68, 123, 323, 443, 500, 4500, 51820],
            expected_process_names: vec![],
        }
    }
}
impl PortsConfig {
    pub fn valid(&self) -> bool {
        (10..=3600).contains(&self.interval_seconds)
            && self.ignore_patterns.len() <= 32
            && self.ignore_patterns.iter().all(|s| {
                !s.is_empty()
                    && s.len() <= 128
                    && s.is_ascii()
                    && !s.bytes().any(|b| b.is_ascii_control())
            })
            && self.ephemeral_range.is_none_or(|[l, h]| l > 0 && l <= h)
            && self.important_udp_ports.len() <= 64
            && self.important_udp_ports.iter().all(|p| *p > 0)
            && self.expected_process_names.len() <= 64
            && self
                .expected_process_names
                .iter()
                .all(|s| !s.is_empty() && s.len() <= 64 && !s.chars().any(char::is_control))
    }
}
