use crate::{Result, fingerprint};
use garde_contracts::{Id, XuiCheckpoint, XuiFilePosition, XuiLogBatch, XuiSourceHealth};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    os::unix::fs::{FileExt, MetadataExt},
    path::{Path, PathBuf},
};
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogKind {
    Access,
    SuspiciousIp,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogFile {
    pub path: PathBuf,
    pub kind: LogKind,
}
pub fn validate(checkpoint: &XuiCheckpoint) -> Result<()> {
    garde_contracts::validate_xui_checkpoint(checkpoint)
}
fn open(path: &Path) -> Result<File> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("xui.access");
    }
    let mut directory = File::open("/").map_err(|_| "xui.access")?;
    let components: Vec<_> = path.components().skip(1).collect();
    let own_uid = rustix::process::geteuid().as_raw();
    for (index, component) in components.iter().enumerate() {
        let last = index + 1 == components.len();
        let mut flags = rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::CLOEXEC
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::NONBLOCK;
        if !last {
            flags |= rustix::fs::OFlags::DIRECTORY;
        }
        let fd = rustix::fs::openat(
            &directory,
            component.as_os_str(),
            flags,
            rustix::fs::Mode::empty(),
        )
        .map_err(|_| "xui.access")?;
        let file = File::from(fd);
        let metadata = file.metadata().map_err(|_| "xui.access")?;
        if (metadata.uid() != 0 && metadata.uid() != own_uid)
            || (!last && !metadata.is_dir())
            || (last && (!metadata.is_file() || metadata.mode() & 0o022 != 0))
        {
            return Err("xui.access");
        }
        directory = file;
    }
    if !directory.metadata().map_err(|_| "xui.access")?.is_file() {
        return Err("xui.access");
    }
    Ok(directory)
}
fn anchor(f: &File, offset: u64) -> Result<String> {
    let start = offset.saturating_sub(64);
    let mut bytes = vec![0; (offset - start) as usize];
    f.read_exact_at(&mut bytes, start)
        .map_err(|_| "xui.access")?;
    Ok(fingerprint(&bytes))
}
fn initial(f: &File, slot: u8, tail: bool, generation: u64) -> Result<XuiFilePosition> {
    let m = f.metadata().map_err(|_| "xui.access")?;
    let offset = if tail { m.len() } else { 0 };
    let mut byte = [b'\n'];
    if offset > 0 {
        f.read_exact_at(&mut byte, offset - 1)
            .map_err(|_| "xui.access")?
    }
    Ok(XuiFilePosition {
        slot,
        device: m.dev(),
        inode: m.ino(),
        generation,
        offset,
        anchor: anchor(f, offset)?,
        discard: offset > 0 && byte[0] != b'\n',
    })
}
fn rotated(path: &Path, p: &XuiFilePosition) -> Result<Option<File>> {
    let parent = path.parent().ok_or("xui.access")?;
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("xui.access")?;
    let entries = std::fs::read_dir(parent).map_err(|_| "xui.access")?;
    for item in entries.take(128) {
        let item = item.map_err(|_| "xui.access")?;
        let n = item.file_name();
        let n = n.to_string_lossy();
        if n.starts_with(&format!("{name}."))
            && let Ok(f) = open(&item.path())
        {
            let m = f.metadata().map_err(|_| "xui.access")?;
            if (m.dev(), m.ino()) == (p.device, p.inode) {
                return Ok(Some(f));
            }
        }
    }
    Ok(None)
}
fn drain(
    f: &mut File,
    p: &mut XuiFilePosition,
    kind: &LogKind,
    at: i64,
    batch: &mut XuiLogBatch,
    gap: &mut bool,
    finalized: bool,
) -> Result<bool> {
    let m = f.metadata().map_err(|_| "xui.access")?;
    if m.len() < p.offset || anchor(f, p.offset).ok().as_deref() != Some(&p.anchor) {
        *gap = true;
        p.generation = p.generation.checked_add(1).ok_or("xui.checkpoint")?;
        p.offset = 0;
        p.discard = false;
    }
    f.seek(SeekFrom::Start(p.offset))
        .map_err(|_| "xui.access")?;
    let mut reader = BufReader::new(f.take(65536));
    let mut consumed = 0;
    while consumed < 65536 && batch.records.len() < 256 {
        let mut line = vec![];
        let read = reader
            .read_until(b'\n', &mut line)
            .map_err(|_| "xui.access")?;
        if read == 0 {
            break;
        }
        consumed += read;
        let start = p.offset;
        if !line.ends_with(b"\n") && line.len() <= 4096 && !p.discard {
            if finalized {
                p.offset = p.offset.checked_add(read as u64).ok_or("xui.checkpoint")?;
                *gap = true;
            }
            break;
        }
        p.offset = p.offset.checked_add(read as u64).ok_or("xui.checkpoint")?;
        if p.discard {
            if line.ends_with(b"\n") {
                p.discard = false
            };
            continue;
        }
        if line.len() > 4096 {
            *gap = true;
            p.discard = !line.ends_with(b"\n");
            continue;
        }
        let Ok(raw) = std::str::from_utf8(&line) else {
            *gap = true;
            continue;
        };
        let position = format!(
            "{}:{}:{}:{}:{start}:{}",
            p.slot,
            p.device,
            p.inode,
            p.generation,
            fingerprint(&line)
        );
        if let Some(record) = crate::parser::parse(
            raw,
            matches!(kind, LogKind::SuspiciousIp),
            position.as_bytes(),
            at,
        ) {
            batch.records.push(record)
        }
    }
    let f = reader.into_inner().into_inner();
    p.anchor = anchor(f, p.offset)?;
    Ok(p.offset >= f.metadata().map_err(|_| "xui.access")?.len())
}
pub fn files(paths: &[LogFile], checkpoint: &XuiCheckpoint, at: i64) -> Result<XuiLogBatch> {
    validate(checkpoint)?;
    if paths.len() > 4 || at < 0 {
        return Err("xui.policy");
    }
    let mut batch = XuiLogBatch {
        checkpoint: checkpoint.clone(),
        sources: vec![],
        records: vec![],
    };
    batch
        .checkpoint
        .files
        .retain(|p| (p.slot as usize) < paths.len());
    for (slot, source) in paths.iter().enumerate() {
        let slot = slot as u8;
        let old = checkpoint.files.iter().find(|p| p.slot == slot);
        let mut gap = false;
        let mut backlog = false;
        let before = batch.records.len();
        let result = (|| {
            let mut current = open(&source.path)?;
            let m = current.metadata().map_err(|_| "xui.access")?;
            let mut position = match old {
                Some(p) => p.clone(),
                None => initial(&current, slot, true, 0)?,
            };
            if old.is_some() && (m.dev(), m.ino()) != (position.device, position.inode) {
                if let Some(mut previous) = rotated(&source.path, &position)? {
                    if !drain(
                        &mut previous,
                        &mut position,
                        &source.kind,
                        at,
                        &mut batch,
                        &mut gap,
                        true,
                    )? {
                        backlog = true;
                        return Ok(position);
                    }
                } else {
                    gap = true
                }
                position = initial(
                    &current,
                    slot,
                    false,
                    position.generation.checked_add(1).ok_or("xui.checkpoint")?,
                )?;
            }
            let ended = drain(
                &mut current,
                &mut position,
                &source.kind,
                at,
                &mut batch,
                &mut gap,
                false,
            )?;
            backlog = !ended
                && (batch.records.len() >= 256
                    || current
                        .metadata()
                        .map_err(|_| "xui.access")?
                        .len()
                        .saturating_sub(position.offset)
                        > 4096);
            Ok::<_, &'static str>(position)
        })();
        let reason = match result {
            Ok(position) => {
                batch.checkpoint.files.retain(|p| p.slot != slot);
                batch.checkpoint.files.push(position);
                if backlog {
                    Some(Id::new("xui.backlog")?)
                } else {
                    None
                }
            }
            Err(code) => {
                batch.records.truncate(before);
                Some(Id::new(code)?)
            }
        };
        batch.sources.push(XuiSourceHealth {
            source: Id::new(format!("xui.file-{slot}"))?,
            reason_key: reason,
            gap,
        });
    }
    Ok(batch)
}
pub fn journal(
    raw: &str,
    checkpoint: &XuiCheckpoint,
    unit: &str,
    tail: bool,
) -> Result<XuiLogBatch> {
    validate(checkpoint)?;
    if raw.len() > 262144 {
        return Err("xui.limit");
    }
    let mut batch = XuiLogBatch {
        checkpoint: checkpoint.clone(),
        sources: vec![],
        records: vec![],
    };
    for line in raw.lines().filter(|s| !s.trim().is_empty()) {
        if line.len() > 65536 {
            return Err("xui.limit");
        }
        let v: serde_json::Value = serde_json::from_str(line).map_err(|_| "xui.journal")?;
        let cursor = v["__CURSOR"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 4096)
            .ok_or("xui.journal")?;
        let stamp = v["__REALTIME_TIMESTAMP"]
            .as_str()
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or("xui.journal")?;
        if v["_SYSTEMD_UNIT"].as_str() != Some(unit) {
            return Err("xui.journal");
        }
        if !tail
            && let Some(text) = v["MESSAGE"].as_str()
            && let Some(record) = crate::parser::parse(
                text,
                false,
                cursor.as_bytes(),
                (stamp / 1000000).try_into().map_err(|_| "xui.clock")?,
            )
        {
            batch.records.push(record)
        }
        batch.checkpoint.journal_cursor = Some(cursor.into());
        batch.checkpoint.journal_time = stamp;
        if batch.records.len() > 256 {
            return Err("xui.limit");
        }
    }
    batch.checkpoint.journal_started = true;
    batch.sources.push(XuiSourceHealth {
        source: Id::new("xui.journal")?,
        reason_key: None,
        gap: false,
    });
    Ok(batch)
}
