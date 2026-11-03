use crate::Reply;
use chrono::{DateTime, Utc};
use garde_config::Result;
use garde_contracts::{DeliveryHealth, DeliveryProblem, DeliveryState, Event, Id, WireVersion};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::{
    fs::{self, File, OpenOptions},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
    time::Duration,
};

pub struct Store {
    pub(crate) connection: Connection,
    pub(crate) geoip_config: Option<garde_config::Config>,
    _lock: File,
}
pub struct Delivery {
    pub id: i64,
    pub reply: Reply,
    pub attempts: u32,
}
#[derive(Clone, Copy)]
pub enum Outcome {
    Delivered,
    Retry {
        code: &'static str,
        retry_after: u64,
        uncertain: bool,
    },
    Permanent {
        code: &'static str,
    },
}

pub(crate) fn sql<T>(value: rusqlite::Result<T>) -> Result<T> {
    value.map_err(|error| match error.sqlite_error_code() {
        Some(rusqlite::ErrorCode::DiskFull) => "storage.full",
        Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
            "storage.busy"
        }
        Some(rusqlite::ErrorCode::ReadOnly) => "storage.read_only",
        Some(rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase) => {
            "storage.corrupt"
        }
        _ => "storage.failed",
    })
}

fn private_file(path: &Path) -> Result<()> {
    let m = fs::symlink_metadata(path).map_err(|_| "storage.permissions")?;
    if !m.is_file()
        || m.nlink() != 1
        || m.mode() & 0o077 != 0
        || m.uid() != rustix::process::geteuid().as_raw()
    {
        return Err("storage.permissions");
    }
    Ok(())
}

fn directory(path: &Path) -> Result<()> {
    let m = fs::symlink_metadata(path).map_err(|_| "storage.permissions")?;
    if !m.is_dir() || m.mode() & 0o077 != 0 || m.uid() != rustix::process::geteuid().as_raw() {
        return Err("storage.permissions");
    }
    Ok(())
}

impl Store {
    pub fn open(directory: &Path, server: &str, owner: &str) -> Result<Self> {
        Id::new(server)?;
        if !garde_config::valid_user_id(owner) {
            return Err("storage.identity");
        }
        self::directory(directory)?;
        let directory = directory
            .canonicalize()
            .map_err(|_| "storage.permissions")?;
        let lock_path = directory.join("garde.lock");
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(
                (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32,
            )
            .open(&lock_path)
            .map_err(|_| "storage.locked")?;
        private_file(&lock_path)?;
        rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive)
            .map_err(|_| "storage.locked")?;
        let path = directory.join("garde.sqlite3");
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(file) => file.sync_all().map_err(|_| "storage.failed")?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err("storage.failed"),
        }
        private_file(&path)?;
        for suffix in ["-journal", "-wal", "-shm"] {
            let path = directory.join(format!("garde.sqlite3{suffix}"));
            match fs::symlink_metadata(&path) {
                Ok(_) => private_file(&path)?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err("storage.permissions"),
            }
        }
        let mut connection = sql(Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        ))?;
        sql(connection.busy_timeout(Duration::from_millis(250)))?;
        let version: i64 = sql(connection.query_row("PRAGMA user_version", [], |r| r.get(0)))?;
        let page_size: i64 = sql(connection.query_row("PRAGMA page_size", [], |r| r.get(0)))?;
        if page_size != 4096 {
            return Err("storage.version");
        }
        if ![0, 1].contains(&version) {
            return Err("storage.version");
        }
        if version == 0 {
            let tables: i64 = sql(connection.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table'",
                [],
                |r| r.get(0),
            ))?;
            if tables != 0 {
                return Err("storage.version");
            }
        }
        let pages: i64 = sql(connection.query_row("PRAGMA page_count", [], |r| r.get(0)))?;
        if pages > 16384 {
            return Err("storage.quota");
        }
        if version == 1 {
            for query in [
                "SELECT id,payload,created_at FROM events LIMIT 0",
                "SELECT id,event_id,request_id,chat_id,text,state,attempts,next_at,created_at,updated_at,expires_at,last_error,uncertain FROM deliveries LIMIT 0",
                "SELECT chat_id,until_at FROM cooldowns LIMIT 0",
            ] {
                sql(connection.prepare(query))?;
            }
        }
        sql(connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=DELETE; PRAGMA synchronous=EXTRA; PRAGMA cache_size=-2048; PRAGMA max_page_count=16384;"))?;
        if version == 0 {
            let transaction = sql(connection.transaction())?;
            sql(transaction.execute_batch("
            CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS events(id TEXT PRIMARY KEY, payload TEXT NOT NULL, created_at INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS cooldowns(chat_id TEXT PRIMARY KEY, until_at INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS deliveries(
                id INTEGER PRIMARY KEY AUTOINCREMENT, event_id TEXT REFERENCES events(id),
                request_id TEXT UNIQUE,
                chat_id TEXT NOT NULL, text TEXT NOT NULL, state TEXT NOT NULL DEFAULT 'pending',
                attempts INTEGER NOT NULL DEFAULT 0, next_at INTEGER NOT NULL,
                created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, expires_at INTEGER NOT NULL,
                last_error TEXT, uncertain INTEGER NOT NULL DEFAULT 0,
                UNIQUE(event_id,chat_id));
            CREATE INDEX IF NOT EXISTS delivery_due ON deliveries(state,next_at,id);"))?;
            for (key, value) in [
                ("server", server),
                ("owner", owner),
                ("offset", "0"),
                ("pruned", "0"),
                ("epoch", "0"),
                ("last_update_at", "0"),
            ] {
                sql(transaction.execute("INSERT INTO meta VALUES(?1,?2)", params![key, value]))?;
            }
            sql(transaction.pragma_update(None, "user_version", 1))?;
            sql(transaction.commit())?;
        }
        for (key, value) in [("server", server), ("owner", owner)] {
            let saved: String =
                sql(connection
                    .query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0)))?;
            if saved != value {
                return Err("storage.identity");
            }
        }
        for (key, limit) in [
            ("offset", i64::MAX as u64),
            ("pruned", u32::MAX as u64),
            ("epoch", i64::MAX as u64),
            ("last_update_at", i64::MAX as u64),
        ] {
            let value: String =
                sql(connection
                    .query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0)))?;
            if value.parse::<u64>().map_err(|_| "storage.corrupt")? > limit {
                return Err("storage.corrupt");
            }
        }
        let saved: Option<String> = sql(connection
            .query_row("SELECT value FROM meta WHERE key='ssh.state'", [], |r| {
                r.get(0)
            })
            .optional())?;
        if let Some(saved) = saved {
            crate::ssh::State::decode(&saved)?;
        }
        sql(connection.execute("UPDATE deliveries SET state='pending',uncertain=1,last_error='delivery.uncertain' WHERE state='in_flight'",[]))?;
        crate::geoip::initialize(&connection)?;
        Ok(Self {
            geoip_config: None,
            connection,
            _lock: lock,
        })
    }

    pub fn offset(&self) -> Result<i64> {
        let value: String =
            sql(self
                .connection
                .query_row("SELECT value FROM meta WHERE key='offset'", [], |r| {
                    r.get(0)
                }))?;
        value.parse().map_err(|_| "storage.corrupt")
    }

    pub fn poll_offset(&mut self, now: i64) -> Result<i64> {
        let offset = self.offset()?;
        let last: String = sql(self.connection.query_row(
            "SELECT value FROM meta WHERE key='last_update_at'",
            [],
            |r| r.get(0),
        ))?;
        let last = last.parse::<i64>().map_err(|_| "storage.corrupt")?;
        // Telegram retains updates for 24h and can reset IDs after a week of inactivity.
        if offset > 0 && now.saturating_sub(last) >= 2 * 86400 {
            let epoch: String = sql(self.connection.query_row(
                "SELECT value FROM meta WHERE key='epoch'",
                [],
                |r| r.get(0),
            ))?;
            let epoch = epoch
                .parse::<i64>()
                .map_err(|_| "storage.corrupt")?
                .checked_add(1)
                .ok_or("storage.quota")?;
            let transaction = sql(self.connection.transaction())?;
            sql(transaction.execute("UPDATE meta SET value='0' WHERE key='offset'", []))?;
            sql(transaction.execute(
                "UPDATE meta SET value=?1 WHERE key='epoch'",
                [epoch.to_string()],
            ))?;
            sql(transaction.commit())?;
            return Ok(0);
        }
        Ok(offset)
    }

    pub fn bind_bot(&mut self, id: &str) -> Result<()> {
        if !garde_config::valid_user_id(id) {
            return Err("storage.identity");
        }
        sql(self
            .connection
            .execute("INSERT OR IGNORE INTO meta VALUES('bot',?1)", [id]))?;
        let saved: String =
            sql(self
                .connection
                .query_row("SELECT value FROM meta WHERE key='bot'", [], |r| r.get(0)))?;
        if saved != id {
            return Err("storage.identity");
        }
        Ok(())
    }

    pub fn start_id(&mut self) -> Result<Id> {
        let sequence:String=sql(self.connection.query_row("INSERT INTO meta VALUES('startup','1') ON CONFLICT(key) DO UPDATE SET value=CAST(value AS INTEGER)+1 RETURNING value",[],|r|r.get(0)))?;
        Id::new(format!("startup-{sequence}"))
    }

    pub fn consume_update(&mut self, id: i64, reply: Option<Reply>, now: i64) -> Result<i64> {
        if !(0..i64::MAX).contains(&id) {
            return Err("request.invalid");
        }
        let offset = self.offset()?;
        if id < offset {
            return Ok(offset);
        }
        if reply.is_some() {
            self.admit(1, now)?;
        }
        let transaction = sql(self.connection.transaction())?;
        if let Some(reply) = reply {
            let epoch: String =
                sql(transaction
                    .query_row("SELECT value FROM meta WHERE key='epoch'", [], |r| r.get(0)))?;
            enqueue(
                &transaction,
                None,
                Some(&format!("telegram:{epoch}:{id}")),
                &reply,
                now,
                600,
            )?;
        }
        let next = id + 1;
        sql(transaction.execute(
            "UPDATE meta SET value=?1 WHERE key='offset'",
            [next.to_string()],
        ))?;
        sql(transaction.execute(
            "UPDATE meta SET value=?1 WHERE key='last_update_at'",
            [now.to_string()],
        ))?;
        sql(transaction.commit())?;
        Ok(next)
    }

    pub fn publish(&mut self, event: &Event, replies: &[Reply], now: i64) -> Result<()> {
        self.publish_checkpoint(Some(event), replies, now, None)
    }

    pub(crate) fn source_state(&self, kind: crate::ingest::Kind) -> Result<Option<String>> {
        sql(self
            .connection
            .query_row("SELECT value FROM meta WHERE key=?1", [kind.key()], |r| {
                r.get(0)
            })
            .optional())
    }
    pub(crate) fn publish_checkpoint(
        &mut self,
        event: Option<&Event>,
        replies: &[Reply],
        now: i64,
        checkpoint: Option<&str>,
    ) -> Result<()> {
        self.publish_source_checkpoint(event, replies, now, checkpoint, crate::ingest::Kind::Ssh)
    }

    pub(crate) fn publish_source_checkpoint(
        &mut self,
        event: Option<&Event>,
        replies: &[Reply],
        now: i64,
        checkpoint: Option<&str>,
        kind: crate::ingest::Kind,
    ) -> Result<()> {
        if let Some(checkpoint) = checkpoint {
            crate::ingest::State::decode(checkpoint)?;
        }
        let Some(event) = event else {
            if checkpoint.is_some() {
                self.admit(0, now)?;
            }
            let transaction = sql(self.connection.transaction())?;
            if let Some(checkpoint) = checkpoint {
                sql(transaction.execute("INSERT INTO meta VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![kind.key(),checkpoint]))?;
            }
            return sql(transaction.commit());
        };
        let payload = serde_json::to_string(event).map_err(|_| "event.invalid")?;
        let server: String =
            sql(self
                .connection
                .query_row("SELECT value FROM meta WHERE key='server'", [], |r| {
                    r.get(0)
                }))?;
        if event.server_id.as_str() != server
            || payload.len() > 65536
            || replies.len() > 32
            || now < 0
        {
            return Err("event.invalid");
        }
        let existing: Option<String> = sql(self
            .connection
            .query_row(
                "SELECT payload FROM events WHERE id=?1",
                [event.id.as_str()],
                |r| r.get(0),
            )
            .optional())?;
        if let Some(existing) = existing {
            let mut replay = event.clone();
            if checkpoint.is_some() {
                let saved: Event =
                    serde_json::from_str(&existing).map_err(|_| "storage.corrupt")?;
                replay.observed_at = saved.observed_at;
            }
            if serde_json::to_string(&replay).map_err(|_| "event.invalid")? != existing {
                return Err("event.conflict");
            }
            return self.publish_source_checkpoint(None, &[], now, checkpoint, kind);
        }
        self.admit(replies.len() as i64, now)?;
        let transaction = sql(self.connection.transaction())?;
        sql(transaction.execute(
            "INSERT INTO events VALUES(?1,?2,?3)",
            params![event.id.as_str(), payload, now],
        ))?;
        enqueue_event(
            &transaction,
            event,
            replies,
            self.geoip_config.as_ref(),
            now,
        )?;
        if let Some(checkpoint) = checkpoint {
            sql(transaction.execute("INSERT INTO meta VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![kind.key(),checkpoint]))?;
        }
        sql(transaction.commit())
    }

    pub(crate) fn host_state(&self) -> Result<Option<String>> {
        sql(self
            .connection
            .query_row("SELECT value FROM meta WHERE key='host.state'", [], |r| {
                r.get(0)
            })
            .optional())
    }
    pub(crate) fn publish_host(
        &mut self,
        previous: Option<&str>,
        state: &str,
        publications: &[(Event, Vec<Reply>)],
        now: i64,
    ) -> Result<()> {
        if publications.len() > 40 || publications.iter().map(|(_, r)| r.len()).sum::<usize>() > 40
        {
            return Err("event.invalid");
        }
        self.publish_observation("host.state", previous, state, publications, now)
    }
    pub(crate) fn observation_state(&self, key: &str) -> Result<Option<String>> {
        sql(self
            .connection
            .query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0))
            .optional())
    }
    pub fn inspect_observation(config: &garde_config::Config, key: &str) -> Result<Option<String>> {
        let connection = inspect_connection(
            &config.state_dir,
            config.server_id.as_str(),
            config.owner()?,
        )?;
        sql(connection
            .query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0))
            .optional())
    }
    pub fn ports_health(
        &self,
        config: &garde_config::Config,
        now: i64,
    ) -> Result<garde_contracts::PortsHealth> {
        crate::listeners::health(
            config,
            self.observation_state("ports.state")?.as_deref(),
            now,
        )
    }
    pub fn inspect_ports(
        config: &garde_config::Config,
        now: i64,
    ) -> Result<garde_contracts::PortsHealth> {
        crate::listeners::health(
            config,
            Self::inspect_observation(config, "ports.state")?.as_deref(),
            now,
        )
    }
    pub fn services_health(
        &self,
        config: &garde_config::Config,
        now: i64,
    ) -> Result<garde_contracts::ServicesHealth> {
        crate::services::health(
            config,
            self.observation_state("services.state")?.as_deref(),
            now,
        )
    }
    pub fn inspect_services(
        config: &garde_config::Config,
        now: i64,
    ) -> Result<garde_contracts::ServicesHealth> {
        crate::services::health(
            config,
            Self::inspect_observation(config, "services.state")?.as_deref(),
            now,
        )
    }
    pub fn xui_health(
        &self,
        config: &garde_config::Config,
        now: i64,
    ) -> Result<garde_contracts::XuiHealth> {
        crate::xui::health(config, self.observation_state("xui.state")?.as_deref(), now)
    }
    pub fn inspect_xui(
        config: &garde_config::Config,
        now: i64,
    ) -> Result<garde_contracts::XuiHealth> {
        crate::xui::health(
            config,
            Self::inspect_observation(config, "xui.state")?.as_deref(),
            now,
        )
    }
    pub(crate) fn publish_observation(
        &mut self,
        key: &str,
        previous: Option<&str>,
        state: &str,
        publications: &[(Event, Vec<Reply>)],
        now: i64,
    ) -> Result<()> {
        match key {
            "host.state" => crate::host_health::validate_state(state)?,
            "services.state" => crate::services::validate_state(state)?,
            "ports.state" => crate::listeners::validate_state(state)?,
            "xui.state" => crate::xui::validate_state(state)?,
            _ => return Err("monitor.state"),
        }
        if state.len() > 1048576 {
            return Err("monitor.state");
        }
        if self.observation_state(key)?.as_deref() != previous {
            return Err("monitor.state_changed");
        }
        let server: String =
            sql(self
                .connection
                .query_row("SELECT value FROM meta WHERE key='server'", [], |r| {
                    r.get(0)
                }))?;
        let mut events = Vec::new();
        let mut count = 0;
        for (event, replies) in publications {
            let payload = serde_json::to_string(event).map_err(|_| "event.invalid")?;
            if event.server_id.as_str() != server || payload.len() > 65536 || replies.len() > 32 {
                return Err("event.invalid");
            }
            if key == "xui.state" && event.id.as_str().starts_with("xui-log-") {
                let existing: Option<String> = sql(self
                    .connection
                    .query_row(
                        "SELECT payload FROM events WHERE id=?1",
                        [event.id.as_str()],
                        |r| r.get(0),
                    )
                    .optional())?;
                if let Some(existing) = existing {
                    let saved: Event =
                        serde_json::from_str(&existing).map_err(|_| "storage.corrupt")?;
                    let mut replay = event.clone();
                    replay.observed_at = saved.observed_at;
                    if serde_json::to_string(&replay).map_err(|_| "event.invalid")? != existing {
                        return Err("event.conflict");
                    }
                    events.push(None);
                    continue;
                }
            }
            count += replies.len();
            events.push(Some(payload));
        }
        // 512 endpoints can each contribute four old and new process records.
        let limit = if key == "xui.state" { 2048 } else { 1024 };
        if publications.len() > limit || count > limit || now < 0 {
            return Err("event.invalid");
        }
        self.admit(count as i64, now)?;
        let transaction = sql(self.connection.transaction())?;
        for ((event, replies), payload) in publications.iter().zip(events) {
            let Some(payload) = payload else { continue };
            sql(transaction.execute(
                "INSERT INTO events VALUES(?1,?2,?3)",
                params![event.id.as_str(), payload, now],
            ))?;
            enqueue_event(
                &transaction,
                event,
                replies,
                self.geoip_config.as_ref(),
                now,
            )?;
        }
        sql(transaction.execute(
            "INSERT INTO meta VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, state],
        ))?;
        sql(transaction.commit())
    }
    pub fn host_health(
        &self,
        config: &garde_config::Config,
        now: i64,
    ) -> Result<garde_contracts::HostHealth> {
        crate::host_health::health(config, self.host_state()?.as_deref(), now)
    }
    pub fn inspect_host(
        config: &garde_config::Config,
        now: i64,
    ) -> Result<garde_contracts::HostHealth> {
        let connection = inspect_connection(
            &config.state_dir,
            config.server_id.as_str(),
            config.owner()?,
        )?;
        let raw: Option<String> = sql(connection
            .query_row("SELECT value FROM meta WHERE key='host.state'", [], |r| {
                r.get(0)
            })
            .optional())?;
        crate::host_health::health(config, raw.as_deref(), now)
    }

    pub fn claim(&mut self, now: i64) -> Result<Option<Delivery>> {
        self.maintenance(now)?;
        let transaction = sql(self.connection.transaction())?;
        sql(transaction.execute("UPDATE deliveries SET state='dead_letter',last_error=CASE WHEN expires_at<=?1 THEN 'delivery.expired' ELSE 'delivery.exhausted' END,updated_at=?1 WHERE id IN (SELECT id FROM deliveries WHERE state='pending' AND (expires_at<=?1 OR attempts>=128) LIMIT 100)",[now]))?;
        let value = sql(transaction.query_row("SELECT id,chat_id,text,attempts FROM deliveries d WHERE state='pending' AND next_at<=?1 AND expires_at>?1 AND attempts<128 AND NOT EXISTS(SELECT 1 FROM deliveries a WHERE a.chat_id=d.chat_id AND a.state='in_flight') AND NOT EXISTS(SELECT 1 FROM cooldowns c WHERE c.chat_id=d.chat_id AND c.until_at>?1) ORDER BY next_at,id LIMIT 1",[now],|r| Ok(Delivery { id:r.get(0)?, reply:Reply { locale: None, chat_id:r.get(1)?, text:r.get(2)? },attempts:r.get::<_,u32>(3)?+1 })).optional())?;
        if let Some(value) = &value {
            sql(transaction.execute("UPDATE deliveries SET state='in_flight',attempts=attempts+1,updated_at=?1,next_at=?3 WHERE id=?2",params![now,value.id,now.saturating_add(backoff(value.attempts) as i64)]))?;
        }
        sql(transaction.commit())?;
        Ok(value)
    }

    pub fn finish(&mut self, id: i64, outcome: Outcome, now: i64) -> Result<()> {
        let (attempts, expires): (u32, i64) = sql(self.connection.query_row(
            "SELECT attempts,expires_at FROM deliveries WHERE id=?1 AND state='in_flight'",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ))?;
        let (state, code, uncertain, retry) = match outcome {
            Outcome::Delivered => ("delivered", None, false, 0),
            Outcome::Permanent { code } => ("dead_letter", Some(code), false, 0),
            Outcome::Retry { uncertain, .. } if now >= expires => {
                ("dead_letter", Some("delivery.expired"), uncertain, 0)
            }
            Outcome::Retry { uncertain, .. } if attempts >= 128 => {
                ("dead_letter", Some("delivery.exhausted"), uncertain, 0)
            }
            Outcome::Retry {
                code,
                retry_after,
                uncertain,
            } => (
                "pending",
                Some(code),
                uncertain,
                retry_after.max(backoff(attempts)),
            ),
        };
        let transaction = sql(self.connection.transaction())?;
        if let Outcome::Retry {
            code: "telegram.rate_limited",
            retry_after,
            ..
        } = outcome
        {
            sql(transaction.execute("INSERT INTO cooldowns SELECT chat_id,?1 FROM deliveries WHERE id=?2 ON CONFLICT(chat_id) DO UPDATE SET until_at=max(until_at,excluded.until_at)",params![now.saturating_add(i64::try_from(retry_after).unwrap_or(i64::MAX)),id]))?;
        }
        sql(transaction.execute("UPDATE deliveries SET state=?1,last_error=?2,uncertain=max(uncertain,?3),next_at=?4,updated_at=?5 WHERE id=?6 AND state='in_flight'",params![state,code,uncertain,now.saturating_add(i64::try_from(retry).unwrap_or(i64::MAX)),now,id]))?;
        sql(transaction.commit())
    }

    pub fn health(&self, now: i64) -> Result<DeliveryHealth> {
        health(&self.connection, now)
    }

    fn used_bytes(&self) -> Result<i64> {
        let pages: i64 = sql(self
            .connection
            .query_row("PRAGMA page_count", [], |r| r.get(0)))?;
        let free: i64 = sql(self
            .connection
            .query_row("PRAGMA freelist_count", [], |r| r.get(0)))?;
        Ok((pages - free) * 4096)
    }

    fn admit(&mut self, count: i64, now: i64) -> Result<()> {
        self.maintenance(now)?;
        let pending: i64 = sql(self.connection.query_row(
            "SELECT count(*) FROM deliveries WHERE state IN ('pending','in_flight')",
            [],
            |r| r.get(0),
        ))?;
        if pending + count > 10000 || self.used_bytes()? >= 60 * 1024 * 1024 {
            return Err("storage.quota");
        }
        let space = rustix::fs::statvfs(self.connection.path().ok_or("storage.failed")?)
            .map_err(|_| "storage.failed")?;
        if u128::from(space.f_bavail) * u128::from(space.f_frsize) < 16 * 1024 * 1024 {
            return Err("storage.full");
        }
        Ok(())
    }

    fn maintenance(&mut self, now: i64) -> Result<()> {
        let pressure = self.used_bytes()? >= 60 * 1024 * 1024;
        let transaction = sql(self.connection.transaction())?;
        sql(transaction.execute("DELETE FROM cooldowns WHERE chat_id IN(SELECT chat_id FROM cooldowns WHERE until_at<=?1 LIMIT 16)",[now]))?;
        let removed=sql(transaction.execute("DELETE FROM deliveries WHERE id IN (SELECT id FROM deliveries WHERE state IN ('delivered','dead_letter') AND (updated_at<?1 OR ?2 OR (SELECT count(*) FROM deliveries WHERE state IN ('delivered','dead_letter'))>10000) ORDER BY updated_at,id LIMIT 16)",params![now.saturating_sub(30*86400),pressure]))?;
        let events=sql(transaction.execute("DELETE FROM events WHERE id IN (SELECT id FROM events e WHERE NOT EXISTS(SELECT 1 FROM deliveries d WHERE d.event_id=e.id) AND (created_at<?1 OR ?2 OR (SELECT count(*) FROM events)>20000) ORDER BY created_at,id LIMIT 16)",params![now.saturating_sub(30*86400),pressure]))?;
        if removed + events > 0 {
            sql(transaction.execute("UPDATE meta SET value=min(4294967295,CAST(value AS INTEGER)+?1) WHERE key='pruned'",[(removed+events) as i64]))?;
        }
        sql(transaction.commit())
    }

    pub fn inspect(
        directory: &Path,
        server: &str,
        owner: &str,
        now: i64,
    ) -> Result<DeliveryHealth> {
        health(&inspect_connection(directory, server, owner)?, now)
    }

    pub fn ssh_health(
        &self,
        config: &garde_config::Config,
        now: i64,
    ) -> Result<garde_contracts::SshHealth> {
        ssh_health(&self.connection, config, now)
    }
    pub fn firewall_health(
        &self,
        config: &garde_config::Config,
        now: i64,
    ) -> Result<garde_contracts::FirewallHealth> {
        firewall_health(&self.connection, config, now)
    }
    pub fn inspect_firewall(
        config: &garde_config::Config,
        now: i64,
    ) -> Result<garde_contracts::FirewallHealth> {
        firewall_health(
            &inspect_connection(
                &config.state_dir,
                config.server_id.as_str(),
                config.owner()?,
            )?,
            config,
            now,
        )
    }
    pub fn inspect_ssh(
        config: &garde_config::Config,
        now: i64,
    ) -> Result<garde_contracts::SshHealth> {
        ssh_health(
            &inspect_connection(
                &config.state_dir,
                config.server_id.as_str(),
                config.owner()?,
            )?,
            config,
            now,
        )
    }
}

pub(crate) fn inspect_connection(
    directory: &Path,
    server: &str,
    owner: &str,
) -> Result<Connection> {
    self::directory(directory)?;
    let path = directory
        .canonicalize()
        .map_err(|_| "storage.permissions")?
        .join("garde.sqlite3");
    private_file(&path)?;
    let connection = sql(Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    ))?;
    sql(connection.busy_timeout(Duration::from_millis(250)))?;
    let version: i64 = sql(connection.query_row("PRAGMA user_version", [], |r| r.get(0)))?;
    if version != 1 {
        return Err("storage.version");
    }
    for (key, value) in [("server", server), ("owner", owner)] {
        let saved: String =
            sql(connection.query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0)))?;
        if saved != value {
            return Err("storage.identity");
        }
    }
    Ok(connection)
}
fn ssh_health(
    connection: &Connection,
    config: &garde_config::Config,
    now: i64,
) -> Result<garde_contracts::SshHealth> {
    let saved: Option<String> = sql(connection
        .query_row("SELECT value FROM meta WHERE key='ssh.state'", [], |r| {
            r.get(0)
        })
        .optional())?;
    let state = saved
        .map(|s| crate::ssh::State::decode(&s))
        .transpose()?
        .unwrap_or_else(|| crate::ssh::State::new(config));
    Ok(state.health(&config.server_id, now))
}

fn firewall_health(
    connection: &Connection,
    config: &garde_config::Config,
    now: i64,
) -> Result<garde_contracts::FirewallHealth> {
    use crate::ingest::{Kind, State};
    let source = |kind: Kind| -> Result<garde_contracts::SourceHealth> {
        let saved: Option<String> = sql(connection
            .query_row("SELECT value FROM meta WHERE key=?1", [kind.key()], |r| {
                r.get(0)
            })
            .optional())?;
        let state = saved.map(|s| State::decode(&s)).transpose();
        Ok(match state {
            Ok(state) => state
                .unwrap_or_else(|| State::new_for(config, kind))
                .health_for(&config.server_id, now, kind),
            Err(_) => {
                let mut health =
                    State::new_for(config, kind).health_for(&config.server_id, now, kind);
                health.state = garde_contracts::SshStatus::Unavailable;
                health.reason_key = Some(Id::new(if kind == Kind::Fail2ban {
                    "fail2ban.state"
                } else {
                    "firewall.state"
                })?);
                health
            }
        })
    };
    Ok(garde_contracts::FirewallHealth {
        version: WireVersion::V1,
        server_id: config.server_id.clone(),
        observed_at: timestamp(now)?,
        backend: config.firewall.backend,
        fail2ban: source(Kind::Fail2ban)?,
        firewall: source(Kind::Firewall)?,
    })
}

fn timestamp(value: i64) -> Result<DateTime<Utc>> {
    DateTime::from_timestamp(value, 0).ok_or("storage.corrupt")
}

fn health(connection: &Connection, now: i64) -> Result<DeliveryHealth> {
    let count = |state| {
        sql(connection.query_row(
            "SELECT count(*) FROM deliveries WHERE state=?1",
            [state],
            |r| r.get(0),
        ))
    };
    let server: String = sql(connection.query_row(
        "SELECT value FROM meta WHERE key='server'",
        [],
        |r| r.get(0),
    ))?;
    let pruned: String = sql(connection.query_row(
        "SELECT value FROM meta WHERE key='pruned'",
        [],
        |r| r.get(0),
    ))?;
    let oldest: Option<i64> = sql(connection.query_row(
        "SELECT min(created_at) FROM deliveries WHERE state IN ('pending','in_flight')",
        [],
        |r| r.get(0),
    ))?;
    let mut statement=sql(connection.prepare("SELECT id,event_id,state,attempts,coalesce(last_error,'delivery.uncertain'),uncertain,updated_at FROM deliveries WHERE last_error IS NOT NULL OR uncertain=1 ORDER BY updated_at DESC,id DESC LIMIT 20"))?;
    let rows = sql(statement.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, u32>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, bool>(5)?,
            r.get::<_, i64>(6)?,
        ))
    }))?;
    let mut problems = Vec::new();
    for row in rows {
        let (id, event, state, attempts, reason, uncertain, at) = sql(row)?;
        problems.push(DeliveryProblem {
            delivery_id: Id::new(format!("d-{id}"))?,
            event_id: event.map(Id::new).transpose()?,
            state: match state.as_str() {
                "pending" => DeliveryState::Pending,
                "in_flight" => DeliveryState::InFlight,
                "delivered" => DeliveryState::Delivered,
                "dead_letter" => DeliveryState::DeadLetter,
                _ => return Err("storage.corrupt"),
            },
            attempts,
            reason_key: Id::new(reason)?,
            uncertain,
            updated_at: timestamp(at)?,
        });
    }
    Ok(DeliveryHealth {
        version: WireVersion::V1,
        server_id: Id::new(server)?,
        observed_at: timestamp(now)?,
        pending: count("pending")?,
        in_flight: count("in_flight")?,
        delivered: count("delivered")?,
        dead_letter: count("dead_letter")?,
        rate_limited: sql(connection.query_row(
            "SELECT count(*) FROM cooldowns WHERE until_at>?1",
            [now],
            |r| r.get(0),
        ))?,
        oldest_pending_at: oldest.map(timestamp).transpose()?,
        pruned_records: pruned.parse().map_err(|_| "storage.corrupt")?,
        problems,
    })
}

fn backoff(attempt: u32) -> u64 {
    (5u64 << attempt.saturating_sub(1).min(8)).min(900)
}

fn enqueue_event(
    transaction: &rusqlite::Transaction<'_>,
    event: &Event,
    replies: &[Reply],
    config: Option<&garde_config::Config>,
    now: i64,
) -> Result<()> {
    let context = crate::geoip::record(transaction, event, config, now)?;
    for reply in replies {
        let enriched;
        let reply = if let Some(context) = &context {
            let locale = reply
                .locale
                .as_deref()
                .or_else(|| config.map(|c| c.default_locale.as_str()))
                .unwrap_or("en");
            enriched = Reply {
                locale: None,
                chat_id: reply.chat_id.clone(),
                text: format!(
                    "{}\n{}",
                    reply.text,
                    crate::geoip::render_context(context, locale)
                ),
            };
            &enriched
        } else {
            reply
        };
        enqueue(
            transaction,
            Some(event.id.as_str()),
            None,
            reply,
            now,
            86400,
        )?;
    }
    Ok(())
}

fn enqueue(
    transaction: &rusqlite::Transaction<'_>,
    event: Option<&str>,
    request: Option<&str>,
    reply: &Reply,
    now: i64,
    ttl: i64,
) -> Result<()> {
    if !garde_config::valid_user_id(&reply.chat_id)
        || reply.text.encode_utf16().count() > 3900
        || now < 0
    {
        return Err("delivery.invalid");
    }
    let expires = now.checked_add(ttl).ok_or("delivery.invalid")?;
    sql(transaction.execute("INSERT INTO deliveries(event_id,request_id,chat_id,text,next_at,created_at,updated_at,expires_at) VALUES(?1,?2,?3,?4,?5,?5,?5,?6)",params![event,request,reply.chat_id,reply.text,now,expires]))?;
    let id = transaction.last_insert_rowid();
    let text = match event {
        Some(event) => format!("{}\nEvent: {event}\nID: d-{id}", reply.text),
        None => format!("{}\nID: d-{id}", reply.text),
    };
    sql(transaction.execute(
        "UPDATE deliveries SET text=?1 WHERE id=?2",
        params![text, id],
    ))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn sqlite_full_rolls_back_the_reply_and_offset() {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let mut store = Store::open(dir.path(), "server-demo", "123456789").unwrap();
        let pages: i64 = store
            .connection
            .query_row("PRAGMA page_count", [], |r| r.get(0))
            .unwrap();
        store
            .connection
            .pragma_update(None, "max_page_count", pages)
            .unwrap();
        for id in 0..50 {
            let offset = store.offset().unwrap();
            let result = store.consume_update(
                id,
                Some(Reply {
                    locale: None,
                    chat_id: "123456789".into(),
                    text: "x".repeat(3800),
                }),
                1000,
            );
            if let Err(code) = result {
                assert_eq!(code, "storage.full");
                assert_eq!(store.offset().unwrap(), offset);
                return;
            }
        }
        panic!("expected_sqlite_full");
    }

    #[test]
    fn queue_capacity_applies_backpressure_without_acknowledging_new_commands() {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let mut store = Store::open(dir.path(), "server-demo", "123456789").unwrap();
        // Quota behavior does not require 10000 synchronous disk flushes in this fixture.
        store
            .connection
            .pragma_update(None, "synchronous", "OFF")
            .unwrap();
        for id in 0..10000 {
            store
                .consume_update(
                    id,
                    Some(Reply {
                        locale: None,
                        chat_id: "123456789".into(),
                        text: "quota".into(),
                    }),
                    1000,
                )
                .unwrap();
        }
        assert_eq!(
            store.consume_update(
                10000,
                Some(Reply {
                    locale: None,
                    chat_id: "123456789".into(),
                    text: "over quota".into()
                }),
                1000
            ),
            Err("storage.quota")
        );
        assert_eq!(store.offset().unwrap(), 10000);
        assert_eq!(store.health(1000).unwrap().pending, 10000);
    }
}
