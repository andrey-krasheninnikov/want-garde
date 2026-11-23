use crate::delivery::{Store, directory, private_file, sql};
use garde_config::{Result, Retention, atomic_write};
use garde_contracts::Id;
use rusqlite::{Connection, params};
use std::{
    fs,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
};

pub(crate) const MAX_RESULTS: usize = 1024;

pub(crate) fn initialize(connection: &Connection) -> Result<()> {
    sql(connection.execute_batch(
        "CREATE INDEX IF NOT EXISTS events_recent ON events(created_at,id);
         CREATE TABLE IF NOT EXISTS retained_audit(
            id TEXT PRIMARY KEY, process INTEGER NOT NULL, at INTEGER NOT NULL, code TEXT NOT NULL);
         CREATE INDEX IF NOT EXISTS retained_audit_age ON retained_audit(process,at,id);",
    ))
}

pub(crate) fn database_soft_limit(retention: Retention) -> u64 {
    retention.database_bytes() - (4 << 20)
}

pub(crate) fn disk_reserve(retention: Retention) -> u64 {
    // Keep space for the rollback journal and a filesystem reserve.
    retention.database_bytes() + (16 << 20)
}

pub(crate) fn free_bytes(path: &Path) -> Result<u64> {
    let space = rustix::fs::statvfs(path).map_err(|_| "storage.failed")?;
    Ok(space.f_bavail.saturating_mul(space.f_frsize))
}

#[derive(Clone, Copy)]
pub enum AuditClass {
    Action,
    Process,
}

pub(crate) fn insert_audit(
    connection: &Connection,
    class: AuditClass,
    id: &Id,
    code: &Id,
    now: i64,
) -> Result<()> {
    if now < 0 {
        return Err("request.invalid");
    }
    sql(connection.execute(
        "INSERT INTO retained_audit VALUES (?1,?2,?3,?4)",
        params![
            id.as_str(),
            matches!(class, AuditClass::Process),
            now,
            code.as_str()
        ],
    ))?;
    Ok(())
}

pub(crate) fn prune_audit(
    connection: &Connection,
    policy: Retention,
    now: i64,
    pressure: bool,
) -> Result<usize> {
    let mut removed = 0;
    for (process, days) in [
        (false, policy.action_audit_days),
        (true, policy.process_audit_days),
    ] {
        removed += sql(connection.execute(
            "DELETE FROM retained_audit WHERE id IN (
             SELECT id FROM retained_audit WHERE process=?1 AND (at<?2 OR ?3)
             ORDER BY at,id LIMIT 16)",
            params![
                process,
                now.saturating_sub(i64::from(days) * 86400),
                pressure
            ],
        ))?;
    }
    Ok(removed)
}

pub(crate) fn audit_counts(connection: &Connection) -> Result<(u32, u32)> {
    let exists: bool = sql(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='retained_audit' AND type='table')",
        [],
        |r| r.get(0),
    ))?;
    if !exists {
        return Ok((0, 0));
    }
    sql(connection.query_row(
        "SELECT count(*) FILTER (WHERE process=0), count(*) FILTER (WHERE process=1) FROM retained_audit",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ))
}

fn files(state_dir: &Path) -> Result<Vec<(PathBuf, u64)>> {
    let path = state_dir.join("results");
    match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(_) => return Err("storage.results"),
        Ok(_) => directory(&path)?,
    }
    let mut entries = Vec::new();
    for entry in fs::read_dir(path)
        .map_err(|_| "storage.results")?
        .take(MAX_RESULTS + 1)
    {
        let path = entry.map_err(|_| "storage.results")?.path();
        private_file(&path)?;
        let size = fs::symlink_metadata(&path)
            .map_err(|_| "storage.results")?
            .len();
        entries.push((path, size));
    }
    if entries.len() > MAX_RESULTS {
        return Err("storage.quota");
    }
    Ok(entries)
}

pub(crate) fn recover_result_writes(state_dir: &Path) -> Result<()> {
    for (path, _) in files(state_dir)? {
        let Some((pid, sequence)) = path
            .file_name()
            .and_then(|s| s.to_str())
            .and_then(|s| s.strip_prefix(".garde-"))
            .and_then(|s| s.split_once('-'))
        else {
            continue;
        };
        if pid
            .parse::<u32>()
            .is_ok_and(|n| n > 0 && pid == n.to_string())
            && sequence
                .parse::<u64>()
                .is_ok_and(|n| sequence == n.to_string())
        {
            fs::remove_file(path).map_err(|_| "storage.results")?;
        }
    }
    Ok(())
}

pub(crate) fn result_usage(state_dir: &Path) -> Result<(u64, u32)> {
    let entries = files(state_dir)?;
    let bytes = entries
        .iter()
        .fold(0u64, |total, (_, size)| total.saturating_add(*size));
    Ok((bytes, entries.len() as u32))
}

pub(crate) fn prune_results(state_dir: &Path, policy: Retention, now: i64) -> Result<()> {
    let cutoff = now.saturating_sub(i64::from(policy.result_hours) * 3600);
    let expired = |path: &Path| {
        let Some((date, id)) = path
            .file_name()
            .and_then(|s| s.to_str())
            .and_then(|s| s.split_once('_'))
        else {
            return false;
        };
        date.parse::<i64>()
            .is_ok_and(|at| at >= 0 && at < cutoff && date == at.to_string())
            && Id::new(id).is_ok()
    };
    for (path, _) in files(state_dir)?
        .into_iter()
        .filter(|(path, _)| expired(path))
        .take(16)
    {
        fs::remove_file(path).map_err(|_| "storage.results")?;
    }
    Ok(())
}

impl Store {
    pub fn record_audit(&mut self, class: AuditClass, id: &Id, code: &Id, now: i64) -> Result<()> {
        self.admit(0, now)?;
        insert_audit(&self.connection, class, id, code, now)
    }

    /// Store a completed result. In-progress operations do not use this directory.
    pub fn store_result(&mut self, id: &Id, contents: &[u8], now: i64) -> Result<PathBuf> {
        if contents.is_empty() || now < 0 {
            return Err("request.invalid");
        }
        let state_dir = Path::new(self.connection.path().ok_or("storage.failed")?)
            .parent()
            .ok_or("storage.failed")?;
        prune_results(state_dir, self.retention, now)?;
        let (bytes, count) = result_usage(state_dir)?;
        let total = bytes.saturating_add(contents.len() as u64);
        if count as usize >= MAX_RESULTS || total > self.retention.result_bytes() {
            return Err("storage.quota");
        }
        if free_bytes(state_dir)?
            < disk_reserve(self.retention).saturating_add(contents.len() as u64)
        {
            return Err("storage.full");
        }
        let path = state_dir.join("results");
        match fs::DirBuilder::new().mode(0o700).create(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err("storage.results"),
        }
        directory(&path)?;
        let path = path.join(format!("{now}_{}", id.as_str()));
        match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            _ => return Err("storage.results"),
        }
        atomic_write(&path, contents, 0o600)?;
        Ok(path)
    }
}
