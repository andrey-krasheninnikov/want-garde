use serde::{Deserialize, Serialize};

pub const MAX_OBSERVATION_BATCH: usize = 2048;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Retention {
    pub event_days: u16,
    pub action_audit_days: u16,
    pub process_audit_days: u16,
    pub result_hours: u16,
    pub database_mib: u16,
    pub pending_deliveries: u32,
    pub result_mib: u16,
}

impl Default for Retention {
    fn default() -> Self {
        Self {
            event_days: 30,
            action_audit_days: 30,
            process_audit_days: 7,
            result_hours: 24,
            database_mib: 64,
            pending_deliveries: 10_000,
            result_mib: 32,
        }
    }
}

impl Retention {
    pub fn valid(self) -> bool {
        [
            self.event_days,
            self.action_audit_days,
            self.process_audit_days,
        ]
        .into_iter()
        .all(|days| (1..=3650).contains(&days))
            && (1..=8760).contains(&self.result_hours)
            && (8..=256).contains(&self.database_mib)
            && (MAX_OBSERVATION_BATCH as u32..=10_000).contains(&self.pending_deliveries)
            && (1..=128).contains(&self.result_mib)
    }

    pub fn database_bytes(self) -> u64 {
        u64::from(self.database_mib) << 20
    }

    pub fn result_bytes(self) -> u64 {
        u64::from(self.result_mib) << 20
    }
}
