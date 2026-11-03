use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HostHealthConfig {
    pub enabled: bool,
    pub interval_seconds: u32,
    pub disk_paths: Vec<PathBuf>,
    pub disk_warn_percent: f64,
    pub disk_critical_percent: f64,
    pub inode_warn_percent: f64,
    pub inode_critical_percent: f64,
    pub cpu_warn_percent: f64,
    pub memory_warn_percent: f64,
    pub load_warn_per_core: f64,
    pub check_reboot_required: bool,
}
impl Default for HostHealthConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_seconds: 60,
            disk_paths: vec!["/".into()],
            disk_warn_percent: 85.0,
            disk_critical_percent: 95.0,
            inode_warn_percent: 85.0,
            inode_critical_percent: 95.0,
            cpu_warn_percent: 90.0,
            memory_warn_percent: 90.0,
            load_warn_per_core: 2.0,
            check_reboot_required: true,
        }
    }
}
impl HostHealthConfig {
    pub fn valid(&self) -> bool {
        let percent = |v: f64| v.is_finite() && (5.0..=100.0).contains(&v);
        (10..=3600).contains(&self.interval_seconds)
            && !self.disk_paths.is_empty()
            && self.disk_paths.len() <= 8
            && self.disk_paths.iter().enumerate().all(|(i, p)| {
                p.is_absolute()
                    && p.as_os_str().len() <= 512
                    && p.to_str().is_some_and(|s| !s.chars().any(char::is_control))
                    && !p
                        .components()
                        .any(|c| matches!(c, std::path::Component::ParentDir))
                    && !self.disk_paths[..i].contains(p)
            })
            && [
                self.disk_warn_percent,
                self.disk_critical_percent,
                self.inode_warn_percent,
                self.inode_critical_percent,
                self.cpu_warn_percent,
                self.memory_warn_percent,
            ]
            .into_iter()
            .all(percent)
            && self.disk_warn_percent < self.disk_critical_percent
            && self.inode_warn_percent < self.inode_critical_percent
            && self.load_warn_per_core.is_finite()
            && (0.2..=100.0).contains(&self.load_warn_per_core)
    }
}
