use garde_config::Result;
use garde_contracts::{
    Id, WireVersion, XuiCheckpoint, XuiReadRequest, XuiReadResponse, XuiSourceHealth,
};
use garde_xui::logs::LogFile;
use serde::Deserialize;
use std::{
    future::Future,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
};
pub trait XuiSource: Send + Sync {
    fn read(&self, request: XuiReadRequest)
    -> impl Future<Output = Result<XuiReadResponse>> + Send;
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub database: PathBuf,
    pub journal_unit: String,
    pub files: Vec<LogFile>,
}
fn source_path(path: &Path, log: bool) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    if !path.is_absolute()
        || path.as_os_str().len() > 4096
        || path
            .components()
            .any(|p| matches!(p, std::path::Component::ParentDir))
    {
        return Err("xui.policy");
    }
    let mut current = PathBuf::from("/");
    for component in path.components().skip(1) {
        current.push(component.as_os_str());
        match std::fs::symlink_metadata(&current) {
            Ok(m) => {
                if m.uid() != 0
                    || m.mode() & if log && m.is_dir() { 0o002 } else { 0o022 } != 0
                    || m.file_type().is_symlink()
                    || (!m.is_dir() && !m.is_file())
                {
                    return Err("xui.policy");
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(_) => return Err("xui.policy"),
        }
    }
    Ok(())
}
impl Policy {
    pub fn load(path: &Path) -> Result<Self> {
        let p: Self = serde_json::from_slice(&garde_config::read_private(path, 65536)?)
            .map_err(|_| "xui.policy")?;
        p.validate()?;
        Ok(p)
    }
    fn validate(&self) -> Result<()> {
        if !garde_config::valid_unit(&self.journal_unit) || self.files.len() > 4 {
            return Err("xui.policy");
        }
        source_path(&self.database, false)?;
        for file in &self.files {
            source_path(&file.path, true)?
        }
        Ok(())
    }
}
pub struct SystemXui(pub Policy);
#[derive(Clone)]
enum Query {
    Tail,
    At(String),
    After(String),
    Since(u64),
}
async fn journal(unit: &str, query: Query) -> Result<String> {
    use std::process::Stdio;
    let metadata = matches!(query, Query::Tail | Query::At(_));
    let mut command = tokio::process::Command::new("/usr/bin/journalctl");
    command
        .args(["--system", "--no-pager", "--quiet", "--output=json"])
        .arg(format!("_SYSTEMD_UNIT={unit}"));
    command.arg(if metadata {
        "--output-fields=__CURSOR,__REALTIME_TIMESTAMP,_SYSTEMD_UNIT"
    } else {
        "--output-fields=__CURSOR,__REALTIME_TIMESTAMP,_SYSTEMD_UNIT,MESSAGE"
    });
    match query {
        Query::Tail => {
            command.args(["-n", "1"]);
        }
        Query::At(c) => {
            command.arg(format!("--cursor={c}")).args(["-n", "1"]);
        }
        Query::After(c) => {
            command.arg(format!("--after-cursor={c}")).arg("--no-tail");
        }
        Query::Since(at) => {
            command
                .arg(format!("--since=@{}.{:06}", at / 1000000, at % 1000000))
                .arg("--no-tail");
        }
    }
    let mut child = command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LC_ALL", "C.UTF-8")
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| "xui.journal")?;
    let output = child.stdout.take().ok_or("xui.journal")?;
    let result = tokio::time::timeout(Duration::from_secs(2), async {
        let mut reader = tokio::io::BufReader::new(output);
        let mut result = String::new();
        let mut limited = false;
        for _ in 0..if metadata { 1 } else { 65 } {
            let mut line = vec![];
            (&mut reader)
                .take(65537)
                .read_until(b'\n', &mut line)
                .await
                .map_err(|_| "xui.journal")?;
            if line.is_empty() {
                break;
            }
            if line.len() > 65536 || result.len() + line.len() > 262144 {
                return Err("xui.limit");
            };
            result.push_str(std::str::from_utf8(&line).map_err(|_| "xui.journal")?);
            limited = true;
        }
        Ok::<_, &'static str>((result, limited))
    })
    .await;
    match result {
        Ok(Ok((raw, true))) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            Ok(raw)
        }
        Ok(Ok((raw, false))) => {
            match tokio::time::timeout(Duration::from_millis(500), child.wait()).await {
                Ok(Ok(s)) if s.success() => Ok(raw),
                _ => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    Err("xui.journal")
                }
            }
        }
        Ok(Err(e)) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            Err(e)
        }
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            Err("xui.timeout")
        }
    }
}
impl XuiSource for SystemXui {
    async fn read(&self, request: XuiReadRequest) -> Result<XuiReadResponse> {
        self.0.validate()?;
        let at = chrono::Utc::now().timestamp();
        match request {
            XuiReadRequest::Snapshot { .. } => {
                let path = self.0.database.clone();
                let snapshot = tokio::task::spawn_blocking(move || garde_xui::snapshot(&path, at))
                    .await
                    .map_err(|_| "xui.reader")??;
                Ok(XuiReadResponse::Snapshot {
                    version: WireVersion::V1,
                    snapshot,
                })
            }
            XuiReadRequest::Logs { checkpoint, .. } => {
                garde_contracts::validate_xui_checkpoint(&checkpoint)?;
                let identity=garde_xui::fingerprint(&serde_json::to_vec(&serde_json::json!({"database":self.0.database,"journal_unit":self.0.journal_unit,"files":self.0.files})).map_err(|_|"xui.policy")?);
                let changed = checkpoint
                    .source_id
                    .as_ref()
                    .is_some_and(|s| s != &identity);
                let checkpoint = if changed {
                    XuiCheckpoint::default()
                } else {
                    checkpoint
                };
                let files = self.0.files.clone();
                let cp = checkpoint.clone();
                let mut batch =
                    tokio::task::spawn_blocking(move || garde_xui::logs::files(&files, &cp, at))
                        .await
                        .map_err(|_| "xui.reader")??;
                let scanned = async {
                    let mut gap = false;
                    let tail = !checkpoint.journal_started;
                    let query = if tail {
                        Query::Tail
                    } else if let Some(cursor) = &checkpoint.journal_cursor {
                        let found = journal(&self.0.journal_unit, Query::At(cursor.clone()))
                            .await
                            .ok()
                            .and_then(|raw| {
                                raw.lines().next().and_then(|line| {
                                    serde_json::from_str::<serde_json::Value>(line).ok()
                                })
                            })
                            .is_some_and(|v| v["__CURSOR"].as_str() == Some(cursor));
                        if found {
                            Query::After(cursor.clone())
                        } else {
                            journal(&self.0.journal_unit, Query::Tail).await?;
                            gap = true;
                            Query::Since(checkpoint.journal_time)
                        }
                    } else {
                        Query::Since(checkpoint.journal_time)
                    };
                    let raw = journal(&self.0.journal_unit, query).await?;
                    let backlog = raw.lines().count() > 64;
                    let raw = if backlog {
                        raw.lines().take(64).collect::<Vec<_>>().join("\n")
                    } else {
                        raw
                    };
                    let mut journal_batch = garde_xui::logs::journal(
                        &raw,
                        &batch.checkpoint,
                        &self.0.journal_unit,
                        tail,
                    )?;
                    if journal_batch.checkpoint.journal_cursor.is_none() {
                        journal_batch.checkpoint.journal_time = (at as u64).saturating_mul(1000000);
                        journal_batch.sources[0].reason_key =
                            Some(Id::new("xui.journal_unverified")?);
                    }
                    if gap && raw.is_empty() {
                        let raw = journal(&self.0.journal_unit, Query::Tail).await?;
                        journal_batch = garde_xui::logs::journal(
                            &raw,
                            &journal_batch.checkpoint,
                            &self.0.journal_unit,
                            true,
                        )?;
                        if journal_batch.checkpoint.journal_cursor == checkpoint.journal_cursor {
                            journal_batch.checkpoint.journal_cursor = None;
                            journal_batch.checkpoint.journal_time =
                                (at as u64).saturating_mul(1000000);
                        }
                    }
                    if backlog {
                        journal_batch.sources[0].reason_key = Some(Id::new("xui.backlog")?);
                    }
                    journal_batch.sources[0].gap = gap;
                    Ok::<_, &'static str>(journal_batch)
                }
                .await;
                match scanned {
                    Ok(j) => {
                        batch.checkpoint = j.checkpoint;
                        batch.sources.extend(j.sources);
                        batch.records.extend(j.records)
                    }
                    Err(e) => batch.sources.push(XuiSourceHealth {
                        source: Id::new("xui.journal")?,
                        reason_key: Some(Id::new(e)?),
                        gap: false,
                    }),
                }
                if changed {
                    for h in &mut batch.sources {
                        h.gap = true
                    }
                }
                batch.checkpoint.source_id = Some(identity);
                Ok(XuiReadResponse::Logs {
                    version: WireVersion::V1,
                    batch,
                })
            }
        }
    }
}
pub async fn serve_connection(
    mut stream: UnixStream,
    allowed_uid: u32,
    source: &impl XuiSource,
) -> Result<()> {
    if allowed_uid == 0 || stream.peer_cred().map_err(|_| "helper.peer")?.uid() != allowed_uid {
        return Err("helper.peer");
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut bytes = vec![];
        tokio::io::BufReader::new(&mut stream)
            .take(16385)
            .read_until(b'\n', &mut bytes)
            .await
            .map_err(|_| "helper.request")?;
        if bytes.pop() != Some(b'\n') || bytes.len() > 16384 {
            return Err("helper.request");
        }
        let request: XuiReadRequest =
            serde_json::from_slice(&bytes).map_err(|_| "helper.request")?;
        if let XuiReadRequest::Logs { checkpoint, .. } = &request {
            garde_contracts::validate_xui_checkpoint(checkpoint)?
        }
        let response = match source.read(request).await {
            Ok(response) => response,
            Err(code) => XuiReadResponse::Unavailable {
                version: WireVersion::V1,
                reason_key: Id::new(code)?,
            },
        };
        let mut output = serde_json::to_vec(&response).map_err(|_| "helper.output")?;
        if output.len() > 1048576 {
            return Err("helper.output");
        };
        output.push(b'\n');
        stream
            .write_all(&output)
            .await
            .map_err(|_| "helper.output")?;
        stream.shutdown().await.map_err(|_| "helper.output")
    })
    .await
    .map_err(|_| "helper.timeout")?
}
