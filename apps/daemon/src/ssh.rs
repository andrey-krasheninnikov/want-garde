use crate::{
    Bot,
    runtime::{Database, now},
};
use garde_config::{Config, Result, SshConfig};
use garde_contracts::{Classification, Event, Evidence, Id, Severity, WireVersion};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::OpenOptions,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    sync::LazyLock,
};

const LIMIT: usize = 65536;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    version: u8,
    config: SshConfig,
    source: SshConfig,
    started_at: i64,
    checked_at: i64,
    last_record_at: Option<i64>,
    device: u64,
    inode: u64,
    generation: u64,
    offset: u64,
    skipped: u64,
}
pub struct Collector {
    config: Config,
    database: Database,
}
impl Collector {
    pub fn new(config: Config, database: Database) -> Self {
        Self { config, database }
    }
    pub async fn scan(&mut self) -> Result<()> {
        let saved = self.database.call(|s| s.ssh_state()).await?;
        let mut state: Option<State> = saved
            .map(|s| serde_json::from_str(&s).map_err(|_| "ssh.state"))
            .transpose()?;
        if state.as_ref().is_some_and(|s| s.version != 1) {
            return Err("ssh.version");
        }
        let SshConfig::File { path } = &self.config.ssh else {
            return Err("ssh.unavailable");
        };
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(
                (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32,
            )
            .open(path)
            .map_err(|_| "ssh.access")?;
        let m = file.metadata().map_err(|_| "ssh.access")?;
        if !m.is_file() {
            return Err("ssh.access");
        }
        if state.is_none() {
            let s = State {
                version: 1,
                config: self.config.ssh.clone(),
                source: self.config.ssh.clone(),
                started_at: now(),
                checked_at: now(),
                last_record_at: None,
                device: m.dev(),
                inode: m.ino(),
                generation: 0,
                offset: m.len(),
                skipped: 0,
            };
            return self.commit(s, None).await;
        }
        let mut state = state.take().ok_or("ssh.state")?;
        file.seek(SeekFrom::Start(state.offset))
            .map_err(|_| "ssh.read")?;
        let mut reader = BufReader::new(file);
        for _ in 0..64 {
            let mut bytes = Vec::new();
            reader
                .by_ref()
                .take((LIMIT + 1) as u64)
                .read_until(b'\n', &mut bytes)
                .map_err(|_| "ssh.read")?;
            if bytes.is_empty() || bytes.last() != Some(&b'\n') {
                break;
            }
            let position = state.offset;
            state.offset += bytes.len() as u64;
            state.checked_at = now();
            state.last_record_at = Some(now());
            let event = std::str::from_utf8(&bytes).ok().and_then(|line| {
                parse(
                    line,
                    &self.config,
                    format!(
                        "{}:{}:{}:{position}",
                        state.device, state.inode, state.generation
                    )
                    .as_bytes(),
                )
            });
            if event.is_none() {
                state.skipped = state.skipped.saturating_add(1);
            }
            self.commit(state.clone(), event).await?;
        }
        Ok(())
    }
    async fn commit(&self, state: State, event: Option<Event>) -> Result<()> {
        let checkpoint = serde_json::to_string(&state).map_err(|_| "ssh.state")?;
        let replies = match &event {
            Some(event) => Bot::new(self.config.clone())?.event_replies(event)?,
            None => vec![],
        };
        self.database
            .call(move |s| s.publish_checkpoint(event.as_ref(), &replies, now(), Some(&checkpoint)))
            .await
    }
}
fn parse(line: &str, config: &Config, position: &[u8]) -> Option<Event> {
    static LOGIN: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"Accepted (?P<method>password|publickey|keyboard-interactive/pam) for (?P<user>\S+) from (?P<ip>\S+) port (?P<port>\d+)").expect("static_pattern")
    });
    let captures = LOGIN.captures(line)?;
    let ip = captures["ip"].parse::<std::net::IpAddr>().ok()?;
    let port = captures["port"].parse::<u16>().ok()?;
    let id = format!(
        "ssh:{:x}",
        Sha256::digest([position, line.as_bytes()].concat())
    );
    Some(Event {
        version: WireVersion::V1,
        id: Id::new(id).ok()?,
        server_id: config.server_id.clone(),
        observed_at: chrono::DateTime::from_timestamp(now(), 0)?,
        source: Id::new("ssh").ok()?,
        classification: Classification {
            category: Id::new("ssh.authentication").ok()?,
            severity: Severity::Info,
            rule_id: Id::new("ssh.login").ok()?,
            reason_key: Id::new("ssh_login").ok()?,
        },
        evidence: vec![
            Evidence {
                name: Id::new("user").ok()?,
                value: captures["user"].chars().take(80).collect(),
            },
            Evidence {
                name: Id::new("ip").ok()?,
                value: ip.to_string(),
            },
            Evidence {
                name: Id::new("port").ok()?,
                value: port.to_string(),
            },
            Evidence {
                name: Id::new("method").ok()?,
                value: captures["method"].into(),
            },
        ],
    })
}
