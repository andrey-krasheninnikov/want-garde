use crate::{
    Bot,
    runtime::{Database, now},
};
use garde_config::{Config, Result, SshConfig};
use garde_contracts::{Event, Id, WireVersion};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt},
    path::Path,
};

const LIMIT: usize = 65536;
fn is_zero(value: &i64) -> bool {
    *value == 0
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Ssh,
    Fail2ban,
    Firewall,
}
impl Kind {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Ssh => "ssh.state",
            Self::Fail2ban => "fail2ban.state",
            Self::Firewall => "firewall.state",
        }
    }
    fn input(self, config: &Config) -> &SshConfig {
        match self {
            Self::Ssh => &config.ssh,
            Self::Fail2ban => &config.fail2ban,
            Self::Firewall => &config.firewall.source,
        }
    }
    fn message(self, line: &str) -> Option<&str> {
        match self {
            Self::Ssh => crate::ssh::file_message(line),
            _ => crate::firewall::file_message(self, line),
        }
    }
    fn candidate(self, line: &str, config: &Config) -> bool {
        self == Self::Ssh || crate::firewall::candidate(self, line, config)
    }
    fn event(self, line: &str, config: &Config, position: &[u8]) -> Option<Event> {
        match self {
            Self::Ssh => crate::ssh::parse(line, config, position),
            _ => crate::firewall::parse(self, line, config, position),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileCheckpoint {
    device: u64,
    inode: u64,
    generation: u64,
    offset: u64,
    anchor: String,
    discard: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct State {
    version: u8,
    config: SshConfig,
    source: Option<SshConfig>,
    started_at: Option<i64>,
    checked_at: i64,
    last_record_at: Option<i64>,
    files: Vec<FileCheckpoint>,
    sequence: u64,
    cursor: Option<String>,
    journal_time: u64,
    journal_floor: u64,
    skipped: u32,
    gaps: u32,
    last_gap: Option<String>,
    gap_at: Option<i64>,
    error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    readiness: Option<String>,
    #[serde(default, skip_serializing_if = "is_zero")]
    probe_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parser_config: Option<garde_config::FirewallConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_event_at: Option<i64>,
}
impl State {
    pub(crate) fn new(config: &Config) -> Self {
        Self::new_for(config, Kind::Ssh)
    }
    pub(crate) fn new_for(config: &Config, kind: Kind) -> Self {
        Self {
            version: 1,
            config: kind.input(config).clone(),
            source: None,
            started_at: None,
            checked_at: 0,
            last_record_at: None,
            files: vec![],
            sequence: 0,
            cursor: None,
            journal_time: 0,
            journal_floor: 0,
            skipped: 0,
            gaps: 0,
            last_gap: None,
            gap_at: None,
            error: None,
            readiness: None,
            probe_at: 0,
            parser_config: (kind == Kind::Firewall).then(|| config.firewall.clone()),
            last_event_at: None,
        }
    }
    pub(crate) fn decode(input: &str) -> Result<Self> {
        if input.len() > 32768 {
            return Err("ssh.state");
        }
        let s: Self = serde_json::from_str(input).map_err(|_| "ssh.state")?;
        if s.version != 1 {
            return Err("ssh.version");
        }
        if s.files.len() > 4
            || s.files
                .iter()
                .any(|p| p.anchor.len() != 64 || !p.anchor.bytes().all(|c| c.is_ascii_hexdigit()))
            || s.cursor.as_ref().is_some_and(|c| c.len() > 4096)
            || s.checked_at < 0
            || s.probe_at < 0
            || s.error
                .iter()
                .chain(s.readiness.iter())
                .chain(s.last_gap.iter())
                .any(|code| code.len() > 96 || Id::new(code.as_str()).is_err())
        {
            return Err("ssh.state");
        }
        Ok(s)
    }
    pub(crate) fn health(&self, server: &Id, at: i64) -> garde_contracts::SshHealth {
        use garde_contracts::{SshHealth, SshSourceKind, SshStatus};
        let stamp = |at| chrono::DateTime::from_timestamp(at, 0);
        let stale = self.checked_at > 0 && at.saturating_sub(self.checked_at) >= 60;
        let reason = self
            .error
            .as_deref()
            .or(self.readiness.as_deref())
            .or(if stale { Some("ssh.stale") } else { None });
        SshHealth {
            version: WireVersion::V1,
            server_id: server.clone(),
            observed_at: stamp(at).expect("valid_clock"),
            source: match self.source {
                Some(SshConfig::Journald {}) => SshSourceKind::Journald,
                Some(SshConfig::File { .. }) => SshSourceKind::File,
                _ => SshSourceKind::Unselected,
            },
            state: if reason.is_some() {
                SshStatus::Unavailable
            } else if self.started_at.is_none() {
                SshStatus::Starting
            } else if self.gaps > 0 {
                SshStatus::Degraded
            } else {
                SshStatus::Healthy
            },
            reason_key: reason.and_then(|s| Id::new(s).ok()),
            file_path: match &self.source {
                Some(SshConfig::File { path }) => Some(path.display().to_string()),
                _ => None,
            },
            coverage_started_at: self.started_at.and_then(stamp),
            checked_at: (self.checked_at > 0)
                .then(|| stamp(self.checked_at))
                .flatten(),
            last_record_at: self.last_record_at.and_then(stamp),
            skipped_records: self.skipped,
            gap_count: self.gaps,
            last_gap: self.last_gap.as_ref().and_then(|s| Id::new(s.clone()).ok()),
            last_gap_at: self.gap_at.and_then(stamp),
        }
    }
    pub(crate) fn health_for(
        &self,
        server: &Id,
        at: i64,
        kind: Kind,
    ) -> garde_contracts::SourceHealth {
        let mut health = self.health(server, at);
        if kind != Kind::Ssh {
            let name = if kind == Kind::Fail2ban {
                "fail2ban"
            } else {
                "firewall"
            };
            for code in [&mut health.reason_key, &mut health.last_gap]
                .into_iter()
                .flatten()
            {
                if let Some(suffix) = code.as_str().strip_prefix("ssh.") {
                    *code = Id::new(format!("{name}.{suffix}")).expect("source_reason");
                }
            }
            if (health.skipped_records > 0 && health.state == garde_contracts::SshStatus::Healthy)
                || (self.error.is_none()
                    && self.readiness.as_deref() == Some("firewall.logging_unverified"))
            {
                health.state = garde_contracts::SshStatus::Degraded;
            }
        }
        health
    }
    fn gap(&mut self, code: &str) {
        self.gaps = self.gaps.saturating_add(1);
        self.last_gap = Some(code.into());
        self.gap_at = Some(now());
    }
}
pub struct Collector<J = Journal, B = crate::firewall::HelperClient> {
    kind: Kind,
    config: Config,
    database: Database,
    handles: BTreeMap<(u64, u64), File>,
    journal: J,
    bans: B,
}
impl Collector<Journal> {
    pub fn new(config: Config, database: Database) -> Self {
        Self::for_source(Kind::Ssh, config, database)
    }
    pub fn for_source(kind: Kind, config: Config, database: Database) -> Self {
        Self::with_sources(
            kind,
            config,
            database,
            Journal {
                kind,
                namespace: None,
            },
            crate::firewall::HelperClient,
        )
    }
}
impl<J: JournalSource> Collector<J> {
    pub fn with_journal(config: Config, database: Database, journal: J) -> Self {
        Self::with_sources(
            Kind::Ssh,
            config,
            database,
            journal,
            crate::firewall::HelperClient,
        )
    }
}
impl<J: JournalSource, B: crate::firewall::BansSource> Collector<J, B> {
    pub fn with_sources(
        kind: Kind,
        config: Config,
        database: Database,
        journal: J,
        bans: B,
    ) -> Self {
        Self {
            kind,
            config,
            database,
            handles: BTreeMap::new(),
            journal,
            bans,
        }
    }
    async fn load(&self) -> Result<State> {
        let kind = self.kind;
        self.database
            .call(move |s| s.source_state(kind))
            .await?
            .map(|s| State::decode(&s))
            .transpose()
            .map(|s| s.unwrap_or_else(|| State::new_for(&self.config, self.kind)))
    }
    pub async fn scan(&mut self) -> Result<()> {
        let mut state = self.load().await?;
        self.handles
            .retain(|identity, _| state.files.iter().any(|f| (f.device, f.inode) == *identity));
        if state.config != *self.kind.input(&self.config)
            || (self.kind == Kind::Firewall
                && state.parser_config.as_ref() != Some(&self.config.firewall))
        {
            state.config = self.kind.input(&self.config).clone();
            state.source = None;
            state.files.clear();
            state.cursor = None;
            state.started_at = None;
            state.parser_config =
                (self.kind == Kind::Firewall).then(|| self.config.firewall.clone());
            state.probe_at = 0;
            state.readiness = None;
            state.last_event_at = None;
            state.gap("ssh.source_changed");
            self.handles.clear();
        }
        if self.kind != Kind::Ssh
            && matches!(self.kind.input(&self.config), SshConfig::Auto {})
            && (state.probe_at == 0 || now().saturating_sub(state.probe_at) >= 15)
        {
            state.probe_at = now();
            match crate::firewall::probe(self.kind, &self.config, &self.bans).await {
                Ok(selected) => {
                    state.readiness = if state.source.as_ref().is_some_and(|old| old != &selected) {
                        let reason = if self.kind == Kind::Fail2ban {
                            "fail2ban.target_changed"
                        } else {
                            "firewall.target_changed"
                        };
                        if state.readiness.as_deref() != Some(reason) {
                            state.gap(reason);
                        }
                        Some(reason.into())
                    } else {
                        None
                    };
                    if state.source.is_none() {
                        state.source = Some(selected);
                    }
                }
                Err(code) => {
                    if state.started_at.is_some()
                        && state.readiness.as_deref() != Some(code)
                        && matches!(
                            code,
                            "fail2ban.logging_insufficient"
                                | "firewall.logging_disabled"
                                | "firewall.disabled"
                        )
                    {
                        state.gap(code);
                    }
                    state.readiness = Some(code.into());
                    self.commit(state.clone(), None).await?;
                    if state.source.is_none() {
                        return Err(code);
                    }
                }
            }
            self.commit(state.clone(), None).await?;
        }
        if self.kind == Kind::Firewall
            && self.config.firewall.backend == garde_config::FirewallBackend::Nftables
            && state.last_event_at.is_none()
            && state.readiness.is_none()
        {
            state.readiness = Some("firewall.logging_unverified".into());
        }
        if state.source.is_none()
            && self.kind != Kind::Ssh
            && matches!(self.kind.input(&self.config), SshConfig::Auto {})
        {
            return Err(if self.kind == Kind::Fail2ban {
                "fail2ban.unavailable"
            } else {
                "firewall.unavailable"
            });
        }
        if state.source.is_none() {
            match self.kind.input(&self.config) {
                SshConfig::Auto {} => {
                    if self.journal.query(JournalQuery::Tail).await.is_ok() {
                        state.source = Some(SshConfig::Journald {});
                    } else {
                        state.source = ["/var/log/auth.log", "/var/log/secure"]
                            .into_iter()
                            .find(|p| open_log(Path::new(p)).is_ok())
                            .map(|p| SshConfig::File { path: p.into() });
                    }
                }
                mode => state.source = Some(mode.clone()),
            }
        }
        let result = match state.source {
            Some(SshConfig::Journald {}) => self.scan_journal(&mut state).await,
            Some(SshConfig::File { .. }) => self.scan_file(&mut state).await,
            _ => Err("ssh.unavailable"),
        };
        if let Err(code) = result {
            let mut saved = self.load().await?;
            if saved.error.as_deref() != Some(code) {
                saved.error = Some(code.into());
                self.commit(saved, None).await?;
            }
        }
        result
    }
    pub async fn run(&mut self) -> Result<()> {
        let mut last_error = None;
        loop {
            match self.scan().await {
                Ok(()) => last_error = None,
                Err(code) => {
                    if last_error != Some(code) {
                        eprintln!("{code}");
                        last_error = Some(code);
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }
    pub async fn health(&self) -> Result<garde_contracts::SshHealth> {
        Ok(self
            .load()
            .await?
            .health_for(&self.config.server_id, now(), self.kind))
    }
    async fn scan_journal(&mut self, state: &mut State) -> Result<()> {
        if state.started_at.is_none() {
            let tail = self.journal.query(JournalQuery::Tail).await?;
            if let Some(record) = tail.last() {
                let (cursor, time) = journal_position(record)?;
                state.cursor = Some(cursor);
                state.journal_time = time;
            } else {
                state.journal_time = (now() as u64).saturating_mul(1000000);
            }
            state.journal_floor = state.journal_time;
            state.started_at = Some(now());
            state.checked_at = now();
            state.error = None;
            return self.commit(state.clone(), None).await;
        }
        if let Some(cursor) = &state.cursor {
            let found = self
                .journal
                .query(JournalQuery::At {
                    cursor: cursor.clone(),
                })
                .await?;
            if found
                .first()
                .is_none_or(|r| r["__CURSOR"].as_str() != Some(cursor))
            {
                state.gap("ssh.cursor_missing");
                state.cursor = None;
                self.commit(state.clone(), None).await?;
            }
        }
        let recovering = state.cursor.is_none();
        let query = match &state.cursor {
            Some(cursor) => JournalQuery::After {
                cursor: cursor.clone(),
            },
            None => JournalQuery::Since {
                microseconds: state
                    .journal_time
                    .saturating_sub(1000000)
                    .max(state.journal_floor),
            },
        };
        let records = self.journal.query(query).await?;
        if records.len() > 64 {
            return Err("ssh.journal_output");
        }
        let had_error = state.error.is_some();
        for record in records {
            let (cursor, time) = journal_position(&record)?;
            let message = journal_message(&record);
            let candidate = message
                .as_deref()
                .is_none_or(|m| self.kind.candidate(m, &self.config));
            let mut event = if recovering && time <= state.journal_floor {
                None
            } else {
                message.as_deref().and_then(|message| {
                    self.kind.event(
                        message,
                        &self.config,
                        format!("journal:{cursor}").as_bytes(),
                    )
                })
            };
            if let Some(event) = &mut event {
                event.observed_at = chrono::DateTime::from_timestamp(
                    (time / 1000000) as i64,
                    ((time % 1000000) * 1000) as u32,
                )
                .ok_or("ssh.journal_output")?;
            } else if !(recovering && time <= state.journal_floor) && candidate {
                state.skipped = state.skipped.saturating_add(1);
            }
            if !recovering && time < state.journal_time {
                state.gap("ssh.clock_changed");
            }
            state.cursor = Some(cursor);
            state.journal_time = time;
            state.checked_at = now();
            state.last_record_at = Some(now());
            state.error = None;
            self.commit(state.clone(), event).await?;
        }
        state.error = None;
        if now().saturating_sub(state.checked_at) >= 15 || had_error {
            state.checked_at = now();
            self.commit(state.clone(), None).await?;
        }
        Ok(())
    }
    async fn scan_file(&mut self, state: &mut State) -> Result<()> {
        let SshConfig::File { path } = state
            .source
            .as_ref()
            .unwrap_or(self.kind.input(&self.config))
        else {
            return Err("ssh.unavailable");
        };
        let path = path.clone();
        let current = open_log(&path)?;
        let m = current.metadata().map_err(|_| "ssh.read")?;
        let identity = (m.dev(), m.ino());
        if !state.files.iter().any(|f| (f.device, f.inode) == identity) {
            let initial = state.started_at.is_none();
            if let Some(previous) = state.files.last() {
                let (rotations, complete) =
                    intermediate_rotations(&path, previous, state.checked_at)?;
                if !complete {
                    state.gap("ssh.rotation_order");
                }
                for file in rotations {
                    self.register_file(state, file, false)?;
                }
            }
            self.register_file(state, current, initial)?;
            state.source = Some(SshConfig::File { path: path.clone() });
            state.checked_at = now();
            state.error = None;
            if initial {
                state.started_at = Some(now());
                return self.commit(state.clone(), None).await;
            }
            self.commit(state.clone(), None).await?;
        } else {
            self.handles.insert(identity, current);
        }
        let recovering = state.error.is_some();
        let mut budget = 64;
        let mut missing = Vec::new();
        for index in 0..state.files.len() {
            let mut position = state.files[index].clone();
            let identity = (position.device, position.inode);
            if let std::collections::btree_map::Entry::Vacant(entry) = self.handles.entry(identity)
            {
                if let Some(file) = find_rotation(&path, identity)? {
                    entry.insert(file);
                } else {
                    missing.push(index);
                    continue;
                }
            }
            let file = self.handles.get(&identity).ok_or("ssh.read")?;
            let length = file.metadata().map_err(|_| "ssh.read")?.len();
            if length < position.offset || anchor(file, position.offset)? != position.anchor {
                state.sequence = state.sequence.checked_add(1).ok_or("ssh.state")?;
                position.generation = state.sequence;
                position.offset = 0;
                position.anchor = anchor(file, 0)?;
                position.discard = false;
                state.files[index] = position.clone();
                state.gap("ssh.truncated");
                self.commit(state.clone(), None).await?;
            }
            let mut file = file.try_clone().map_err(|_| "ssh.read")?;
            file.seek(SeekFrom::Start(position.offset))
                .map_err(|_| "ssh.read")?;
            let mut reader = BufReader::new(file);
            while budget > 0 {
                let mut bytes = Vec::new();
                reader
                    .by_ref()
                    .take((LIMIT + 1) as u64)
                    .read_until(b'\n', &mut bytes)
                    .map_err(|_| "ssh.read")?;
                if bytes.is_empty() {
                    break;
                }
                let complete = bytes.last() == Some(&b'\n');
                if !complete && bytes.len() <= LIMIT && !position.discard {
                    break;
                }
                budget -= 1;
                let start = position.offset;
                position.offset = position
                    .offset
                    .checked_add(bytes.len() as u64)
                    .ok_or("ssh.state")?;
                let too_long = bytes.len() > LIMIT;
                let event = if position.discard || too_long {
                    None
                } else {
                    std::str::from_utf8(&bytes)
                        .ok()
                        .and_then(|line| self.kind.message(line))
                        .and_then(|message| {
                            self.kind.event(
                                message,
                                &self.config,
                                format!(
                                    "{}:{}:{}:{}:{start}",
                                    path.display(),
                                    position.device,
                                    position.inode,
                                    position.generation
                                )
                                .as_bytes(),
                            )
                        })
                };
                let candidate = std::str::from_utf8(&bytes)
                    .ok()
                    .and_then(|line| self.kind.message(line))
                    .is_some_and(|m| self.kind.candidate(m, &self.config));
                if !position.discard
                    && (too_long
                        || (candidate && event.is_none())
                        || std::str::from_utf8(&bytes).is_err())
                {
                    state.skipped = state.skipped.saturating_add(1);
                }
                position.discard = !complete && (position.discard || too_long);
                position.anchor = anchor(reader.get_ref(), position.offset)?;
                state.files[index] = position.clone();
                state.checked_at = now();
                state.last_record_at = Some(now());
                state.error = None;
                self.commit(state.clone(), event).await?;
            }
        }
        let lost_file = !missing.is_empty();
        for index in missing.into_iter().rev() {
            state.files.remove(index);
            state.gap("ssh.file_missing");
        }
        state.error = None;
        if now().saturating_sub(state.checked_at) >= 15 || lost_file || recovering {
            state.checked_at = now();
            self.commit(state.clone(), None).await?;
        }
        Ok(())
    }
    fn register_file(&mut self, state: &mut State, file: File, initial: bool) -> Result<()> {
        let m = file.metadata().map_err(|_| "ssh.read")?;
        let identity = (m.dev(), m.ino());
        if state.files.iter().any(|f| (f.device, f.inode) == identity) {
            self.handles.insert(identity, file);
            return Ok(());
        }
        state.sequence = state.sequence.checked_add(1).ok_or("ssh.state")?;
        if state.files.len() == 4 {
            let old = state.files.remove(0);
            self.handles.remove(&(old.device, old.inode));
            state.gap("ssh.rotation_limit");
        }
        let offset = if initial { m.len() } else { 0 };
        state.files.push(FileCheckpoint {
            device: m.dev(),
            inode: m.ino(),
            generation: state.sequence,
            offset,
            anchor: anchor(&file, offset)?,
            discard: initial && offset > 0 && last_byte(&file, offset)? != b'\n',
        });
        self.handles.insert(identity, file);
        Ok(())
    }
    async fn commit(&self, mut state: State, event: Option<Event>) -> Result<()> {
        if self.kind != Kind::Ssh && event.is_some() {
            state.last_event_at = Some(now());
            if state.readiness.as_deref() == Some("firewall.logging_unverified") {
                state.readiness = None;
            }
        }
        let checkpoint = serde_json::to_string(&state).map_err(|_| "ssh.state")?;
        let replies = match &event {
            Some(event) => Bot::new(self.config.clone())?.event_replies(event)?,
            None => vec![],
        };
        let kind = self.kind;
        self.database
            .call(move |s| {
                s.publish_source_checkpoint(
                    event.as_ref(),
                    &replies,
                    now(),
                    Some(&checkpoint),
                    kind,
                )
            })
            .await
    }
}
pub(crate) fn open_log(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32)
        .open(path)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::PermissionDenied {
                "ssh.permissions"
            } else {
                "ssh.access"
            }
        })?;
    let m = file.metadata().map_err(|_| "ssh.access")?;
    if !m.is_file() || m.mode() & 0o002 != 0 {
        return Err("ssh.access");
    }
    Ok(file)
}
fn anchor(file: &File, offset: u64) -> Result<String> {
    let start = offset.saturating_sub(64);
    let mut bytes = vec![0; (offset - start) as usize];
    let mut done = 0;
    while done < bytes.len() {
        let n = file
            .read_at(&mut bytes[done..], start + done as u64)
            .map_err(|_| "ssh.read")?;
        if n == 0 {
            bytes.truncate(done);
            break;
        }
        done += n;
    }
    Ok(format!("{:x}", Sha256::digest(&bytes)))
}
fn last_byte(file: &File, offset: u64) -> Result<u8> {
    let mut byte = [0];
    file.read_at(&mut byte, offset - 1)
        .map_err(|_| "ssh.read")?;
    Ok(byte[0])
}
fn intermediate_rotations(
    path: &Path,
    previous: &FileCheckpoint,
    since: i64,
) -> Result<(Vec<File>, bool)> {
    let parent = path.parent().ok_or("ssh.access")?;
    let name = path.file_name().ok_or("ssh.access")?.to_string_lossy();
    let mut entries = fs::read_dir(parent).map_err(|_| "ssh.access")?;
    let mut files = Vec::new();
    let mut previous_rank = None;
    for entry in entries.by_ref().take(128) {
        let entry = entry.map_err(|_| "ssh.read")?;
        let candidate = entry.file_name();
        let candidate = candidate.to_string_lossy();
        let Some(suffix) = candidate.strip_prefix(name.as_ref()) else {
            continue;
        };
        if !suffix.starts_with(['.', '-'])
            || [".gz", ".xz", ".bz2", ".zst"]
                .iter()
                .any(|s| suffix.ends_with(s))
        {
            continue;
        }
        if let Ok(file) = open_log(&entry.path()) {
            let m = file.metadata().map_err(|_| "ssh.read")?;
            let rank = suffix
                .strip_prefix('.')
                .and_then(|s| s.parse::<u64>().ok())
                .filter(|n| *n > 0);
            if (m.dev(), m.ino()) == (previous.device, previous.inode) {
                previous_rank = rank;
            } else {
                files.push((rank, m.mtime(), file));
            }
        }
    }
    let mut complete = entries.next().is_none();
    if let Some(rank) = previous_rank {
        files.retain(|(n, _, _)| n.is_some_and(|n| n < rank));
        files.sort_by_key(|(n, _, _)| std::cmp::Reverse(*n));
        complete &= files.len() as u64 == rank - 1
            && files
                .iter()
                .enumerate()
                .all(|(i, (n, _, _))| *n == Some(rank - 1 - i as u64));
    } else {
        // shortcut: unknown rotation order uses modification time, prefer numeric rotation for complete recovery.
        complete = false;
        files.retain(|(_, modified, _)| *modified >= since);
        files.sort_by_key(|(_, modified, _)| *modified);
    }
    Ok((
        files.into_iter().map(|(_, _, file)| file).collect(),
        complete,
    ))
}

fn find_rotation(path: &Path, identity: (u64, u64)) -> Result<Option<File>> {
    let parent = path.parent().ok_or("ssh.access")?;
    let name = path.file_name().ok_or("ssh.access")?.to_string_lossy();
    for entry in fs::read_dir(parent).map_err(|_| "ssh.access")?.take(129) {
        let entry = entry.map_err(|_| "ssh.read")?;
        let candidate = entry.file_name();
        let candidate = candidate.to_string_lossy();
        if !(candidate.starts_with(&format!("{name}."))
            || candidate.starts_with(&format!("{name}-")))
            || [".gz", ".xz", ".bz2", ".zst"]
                .iter()
                .any(|s| candidate.ends_with(s))
        {
            continue;
        }
        if let Ok(file) = open_log(&entry.path()) {
            let m = file.metadata().map_err(|_| "ssh.read")?;
            if (m.dev(), m.ino()) == identity {
                return Ok(Some(file));
            }
        }
    }
    Ok(None)
}

#[derive(Clone)]
pub enum JournalQuery {
    Tail,
    At { cursor: String },
    After { cursor: String },
    Since { microseconds: u64 },
}
pub trait JournalSource: Send + Sync {
    fn query(
        &self,
        query: JournalQuery,
    ) -> impl std::future::Future<Output = Result<Vec<serde_json::Value>>> + Send;
}
#[derive(Default)]
pub struct Journal {
    kind: Kind,
    namespace: Option<String>,
}
impl Journal {
    pub fn namespace(name: &str) -> Result<Self> {
        Self::namespace_for_source(Kind::Ssh, name)
    }
    pub fn namespace_for_source(kind: Kind, name: &str) -> Result<Self> {
        if name.is_empty()
            || name.len() > 64
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        {
            return Err("ssh.namespace");
        }
        Ok(Self {
            kind,
            namespace: Some(name.into()),
        })
    }
    async fn read(
        &self,
        query: JournalQuery,
        message: bool,
        limit: usize,
    ) -> Result<(Vec<serde_json::Value>, bool)> {
        use std::process::Stdio;
        use tokio::io::{AsyncBufReadExt, AsyncReadExt};
        let mut command = tokio::process::Command::new("/usr/bin/journalctl");
        command.args(["--system", "--no-pager", "--quiet", "--output=json"]);
        command.arg(if message {
            "--output-fields=__CURSOR,__REALTIME_TIMESTAMP,MESSAGE"
        } else {
            "--output-fields=__CURSOR,__REALTIME_TIMESTAMP"
        });
        if let Some(namespace) = &self.namespace {
            command.arg(format!("--namespace={namespace}"));
        }
        match &query {
            JournalQuery::Tail => {
                command.args(["-n", "1"]);
            }
            JournalQuery::At { cursor } => {
                command.arg(format!("--cursor={cursor}")).args(["-n", "1"]);
            }
            JournalQuery::After { cursor } => {
                command
                    .arg(format!("--after-cursor={cursor}"))
                    .arg("--no-tail");
            }
            JournalQuery::Since { microseconds } => {
                command
                    .arg(format!(
                        "--since=@{}.{:06}",
                        microseconds / 1000000,
                        microseconds % 1000000
                    ))
                    .arg("--no-tail");
            }
        }
        if matches!(
            query,
            JournalQuery::After { .. } | JournalQuery::Since { .. }
        ) {
            match self.kind {
                Kind::Ssh => {
                    command.args([
                        "_COMM=sshd",
                        "_COMM=sshd-session",
                        "_COMM=sshd-auth",
                        "+",
                        "SYSLOG_IDENTIFIER=sshd",
                        "SYSLOG_IDENTIFIER=sshd-session",
                        "SYSLOG_IDENTIFIER=sshd-auth",
                    ]);
                }
                Kind::Fail2ban => {
                    command.args([
                        "_UID=0",
                        "SYSLOG_IDENTIFIER=fail2ban",
                        "SYSLOG_IDENTIFIER=fail2ban-server",
                        "SYSLOG_IDENTIFIER=fail2ban.actions",
                        "+",
                        "_UID=0",
                        "_SYSTEMD_UNIT=fail2ban.service",
                    ]);
                }
                Kind::Firewall => {
                    command.arg("_TRANSPORT=kernel");
                }
            }
        }
        let mut child = command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("LC_ALL", "C.UTF-8")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| "ssh.journal_access")?;
        let output = child.stdout.take().ok_or("ssh.journal_access")?;
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let mut reader = tokio::io::BufReader::new(output);
            let mut records = Vec::new();
            let mut overflow = false;
            for _ in 0..limit {
                let mut bytes = Vec::new();
                (&mut reader)
                    .take((LIMIT + 1) as u64)
                    .read_until(b'\n', &mut bytes)
                    .await
                    .map_err(|_| "ssh.journal_read")?;
                if bytes.is_empty() {
                    break;
                }
                if bytes.len() > LIMIT {
                    overflow = true;
                    break;
                }
                let value: serde_json::Value =
                    serde_json::from_slice(&bytes).map_err(|_| "ssh.journal_output")?;
                journal_position(&value)?;
                records.push(value);
            }
            Ok::<_, &'static str>((records, overflow))
        })
        .await;
        let bounded = result
            .as_ref()
            .ok()
            .and_then(|r| r.as_ref().ok())
            .is_some_and(|(records, overflow)| records.len() == limit || *overflow);
        if bounded || result.is_err() || result.as_ref().is_ok_and(|r| r.is_err()) {
            let _ = child.kill().await;
            let _ = child.wait().await;
        } else {
            let status =
                tokio::time::timeout(std::time::Duration::from_secs(1), child.wait()).await;
            if !status.is_ok_and(|s| s.is_ok_and(|s| s.success())) {
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err("ssh.journal_access");
            }
        }
        result.map_err(|_| "ssh.journal_timeout")?
    }
}
impl JournalSource for Journal {
    async fn query(&self, query: JournalQuery) -> Result<Vec<serde_json::Value>> {
        let metadata = matches!(query, JournalQuery::Tail | JournalQuery::At { .. });
        let response = self
            .read(query.clone(), !metadata, if metadata { 1 } else { 64 })
            .await;
        let (mut records, overflow) = match response {
            Ok(result) => result,
            Err(_) if matches!(query, JournalQuery::At { .. }) => {
                self.read(JournalQuery::Tail, false, 1).await?;
                return Ok(vec![]);
            }
            Err(code) => return Err(code),
        };
        if overflow {
            let next = match records.last() {
                Some(record) => JournalQuery::After {
                    cursor: journal_position(record)?.0,
                },
                None => query,
            };
            let (mut skipped, overflow) = self.read(next, false, 1).await?;
            if overflow || skipped.is_empty() {
                return Err("ssh.journal_output");
            }
            records.append(&mut skipped);
        }
        Ok(records)
    }
}
fn journal_position(record: &serde_json::Value) -> Result<(String, u64)> {
    let cursor = record["__CURSOR"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 4096 && !s.chars().any(char::is_control))
        .ok_or("ssh.journal_output")?;
    let at = record["__REALTIME_TIMESTAMP"]
        .as_str()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|n| *n / 1000000 <= i64::MAX as u64)
        .ok_or("ssh.journal_output")?;
    Ok((cursor.into(), at))
}
fn journal_message(record: &serde_json::Value) -> Option<String> {
    match &record["MESSAGE"] {
        serde_json::Value::String(s) if s.len() <= LIMIT => Some(s.clone()),
        serde_json::Value::Array(bytes) if bytes.len() <= LIMIT => String::from_utf8(
            bytes
                .iter()
                .map(|v| v.as_u64().and_then(|n| u8::try_from(n).ok()))
                .collect::<Option<Vec<_>>>()?,
        )
        .ok(),
        _ => None,
    }
}
