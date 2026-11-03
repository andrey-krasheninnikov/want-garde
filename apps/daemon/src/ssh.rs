use crate::{
    Bot,
    runtime::{Database, now},
};
use garde_config::{Config, Result, SshConfig};
use garde_contracts::{Classification, Event, Evidence, Id, Severity, WireVersion};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt},
    path::Path,
    sync::LazyLock,
};

const LIMIT: usize = 65536;
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
}
impl State {
    pub(crate) fn new(config: &Config) -> Self {
        Self {
            version: 1,
            config: config.ssh.clone(),
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
    fn gap(&mut self, code: &str) {
        self.gaps = self.gaps.saturating_add(1);
        self.last_gap = Some(code.into());
        self.gap_at = Some(now());
    }
}
pub struct Collector<J = Journal> {
    config: Config,
    database: Database,
    handles: BTreeMap<(u64, u64), File>,
    journal: J,
}
impl Collector<Journal> {
    pub fn new(config: Config, database: Database) -> Self {
        Self::with_journal(config, database, Journal::default())
    }
}
impl<J: JournalSource> Collector<J> {
    pub fn with_journal(config: Config, database: Database, journal: J) -> Self {
        Self {
            config,
            database,
            handles: BTreeMap::new(),
            journal,
        }
    }
    async fn load(&self) -> Result<State> {
        self.database
            .call(|s| s.ssh_state())
            .await?
            .map(|s| State::decode(&s))
            .transpose()
            .map(|s| s.unwrap_or_else(|| State::new(&self.config)))
    }
    pub async fn scan(&mut self) -> Result<()> {
        let mut state = self.load().await?;
        self.handles
            .retain(|identity, _| state.files.iter().any(|f| (f.device, f.inode) == *identity));
        if state.config != self.config.ssh {
            state.config = self.config.ssh.clone();
            state.source = None;
            state.files.clear();
            state.cursor = None;
            state.started_at = None;
            state.gap("ssh.source_changed");
            self.handles.clear();
        }
        if state.source.is_none() {
            match &self.config.ssh {
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
        Ok(self.load().await?.health(&self.config.server_id, now()))
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
            let mut event = if recovering && time <= state.journal_floor {
                None
            } else {
                message.as_deref().and_then(|message| {
                    parse(
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
            } else if !(recovering && time <= state.journal_floor) {
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
        let SshConfig::File { path } = state.source.as_ref().unwrap_or(&self.config.ssh) else {
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
                        .and_then(file_message)
                        .and_then(|message| {
                            parse(
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
                    .and_then(file_message)
                    .is_some();
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
fn open_log(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32)
        .open(path)
        .map_err(|_| "ssh.access")?;
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

fn file_message(line: &str) -> Option<&str> {
    static FRAME: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
        r"^(?:(?:[A-Z][a-z]{2}\s+[0-9]{1,2}\s+[0-9]{2}:[0-9]{2}:[0-9]{2}|[0-9]{4}-[0-9]{2}-[0-9]{2}T\S+)\s+\S+\s+)?sshd(?:-(?:session|auth))?(?:\[[0-9]+\])?:\s+(?P<message>.*)$"
    ).expect("static_pattern")
    });
    FRAME
        .captures(line.trim_end())?
        .name("message")
        .map(|m| m.as_str())
}

fn parse(line: &str, config: &Config, position: &[u8]) -> Option<Event> {
    let line = line
        .strip_prefix("error: ")
        .or_else(|| line.strip_prefix("fatal: "))
        .unwrap_or(line);
    static RULES: LazyLock<Vec<(&str, Severity, &str, regex::Regex)>> = LazyLock::new(|| {
        [
            ("ssh_login",Severity::Info,"ssh.authentication",r"^Accepted (?P<method>password|publickey|keyboard-interactive(?:/pam)?|hostbased) for (?P<user>\S+) from (?P<ip>\S+) port (?P<port>[0-9]+)(?:\s|$)"),
            ("ssh_failed",Severity::Warning,"ssh.authentication",r"^Failed (?P<method>password|publickey|keyboard-interactive(?:/pam)?|hostbased) for (?:invalid user )?(?P<user>\S+) from (?P<ip>\S+) port (?P<port>[0-9]+)(?:\s|$)"),
            ("ssh_invalid_user",Severity::Warning,"ssh.authentication",r"^Invalid user (?P<user>\S+) from (?P<ip>\S+)(?:\s+port (?P<port>[0-9]+))?(?:\s|$)"),
            ("ssh_logout",Severity::Info,"ssh.session",r"^pam_unix\(sshd:session\): session closed for user (?P<user>\S+)(?:\s|$)"),
            ("ssh_preauth_closed",Severity::Warning,"ssh.authentication",r"^(?:Connection closed by|Disconnected from) (?:(?:authenticating|invalid) user |user )(?P<user>\S+) (?P<ip>\S+) port (?P<port>[0-9]+)(?:\s|$)"),
            ("ssh_preauth_closed",Severity::Warning,"ssh.authentication",r"^(?:Connection closed by|Disconnected from) (?P<ip>\S+) port (?P<port>[0-9]+) \[preauth\]"),
        ].into_iter().map(|(key,severity,category,pattern)|(key,severity,category,regex::Regex::new(pattern).expect("static_pattern"))).collect()
    });
    let mut selected = None;
    let mut evidence = Vec::new();
    for (key, severity, category, pattern) in RULES.iter() {
        let Some(captures) = pattern.captures(line) else {
            continue;
        };
        if *key == "ssh_preauth_closed"
            && !line.contains("authenticating user ")
            && !line.contains("invalid user ")
            && !line.contains("[preauth]")
        {
            continue;
        }
        for name in ["user", "ip", "port", "method"] {
            if let Some(value) = captures.name(name) {
                let value = match name {
                    "ip" => value.as_str().parse::<std::net::IpAddr>().ok()?.to_string(),
                    "port" => {
                        let port = value.as_str().parse::<u16>().ok()?;
                        if port == 0 {
                            return None;
                        }
                        port.to_string()
                    }
                    _ => value
                        .as_str()
                        .chars()
                        .filter(|c| !c.is_control())
                        .take(80)
                        .collect(),
                };
                evidence.push(Evidence {
                    name: Id::new(name).ok()?,
                    value,
                });
            }
        }
        selected = Some((*key, *severity, *category));
        break;
    }
    if selected.is_none() {
        let marker = [
            "Unable to negotiate",
            "Did not receive identification string",
            "kex_exchange_identification",
        ]
        .into_iter()
        .find(|marker| line.starts_with(marker))?;
        if let Some(ip) = line.split_whitespace().find_map(|word| {
            word.parse::<std::net::IpAddr>()
                .ok()
                .or_else(|| word.strip_suffix(':').and_then(|w| w.parse().ok()))
        }) {
            evidence.push(Evidence {
                name: Id::new("ip").ok()?,
                value: ip.to_string(),
            });
        }
        evidence.push(Evidence {
            name: Id::new("kind").ok()?,
            value: marker.into(),
        });
        selected = Some(("ssh_scanner", Severity::Warning, "ssh.scanner"));
    }
    let (key, severity, category) = selected?;
    let mut hash = Sha256::new();
    hash.update(b"ssh-event-v1\0");
    hash.update(position);
    hash.update(b"\0");
    hash.update(line.as_bytes());
    Some(Event {
        version: WireVersion::V1,
        id: Id::new(format!("ssh:{:x}", hash.finalize())).ok()?,
        server_id: config.server_id.clone(),
        observed_at: chrono::DateTime::from_timestamp(now(), 0)?,
        source: Id::new("ssh").ok()?,
        classification: Classification {
            category: Id::new(category).ok()?,
            severity,
            rule_id: Id::new(key.replace('_', ".")).ok()?,
            reason_key: Id::new(key).ok()?,
        },
        evidence,
    })
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
    namespace: Option<String>,
}
impl Journal {
    pub fn namespace(name: &str) -> Result<Self> {
        if name.is_empty()
            || name.len() > 64
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        {
            return Err("ssh.namespace");
        }
        Ok(Self {
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
