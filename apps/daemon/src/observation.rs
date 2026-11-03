use crate::{Bot, Reply};
use garde_config::{Config, Result};
use garde_contracts::{Classification, Event, Evidence, Id, Severity, WireVersion};
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::AsyncReadExt,
    process::{Child, Command},
};

pub(crate) fn publication(
    config: &Config,
    sequence: &mut u64,
    source: &str,
    reason: &str,
    severity: Severity,
    values: Vec<(&str, String)>,
    at: i64,
) -> Result<(Event, Vec<Reply>)> {
    *sequence = sequence.checked_add(1).ok_or("monitor.state")?;
    let event = Event {
        version: WireVersion::V1,
        id: Id::new(format!("{source}-{}", sequence))?,
        server_id: config.server_id.clone(),
        observed_at: chrono::DateTime::from_timestamp(at, 0).ok_or("monitor.clock")?,
        source: Id::new(source)?,
        classification: Classification {
            category: Id::new(format!("{source}.observation"))?,
            severity,
            rule_id: Id::new(reason)?,
            reason_key: Id::new(reason)?,
        },
        evidence: values
            .into_iter()
            .map(|(name, value)| {
                Ok(Evidence {
                    name: Id::new(name)?,
                    value,
                })
            })
            .collect::<Result<Vec<_>>>()?,
    };
    let replies = Bot::new(config.clone())?.event_replies(&event)?;
    Ok((event, replies))
}
pub(crate) fn stale(at: Option<i64>, now: i64, interval: u64) -> bool {
    at.is_none_or(|at| now < at || now.saturating_sub(at) > interval.saturating_mul(2) as i64)
}

pub struct Probe {
    program: PathBuf,
    child: Option<Child>,
}
impl Probe {
    pub fn new(program: PathBuf) -> Self {
        Self {
            program,
            child: None,
        }
    }
    pub async fn read(&mut self, args: &[&str]) -> Result<String> {
        if let Some(child) = self.child.as_mut() {
            if child.try_wait().map_err(|_| "monitor.probe")?.is_none() {
                return Err("monitor.probe_busy");
            }
            self.child = None;
        }
        self.child = Some(
            Command::new(&self.program)
                .args(args)
                .env_clear()
                .env("LC_ALL", "C")
                .current_dir("/")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .map_err(|_| "monitor.probe")?,
        );
        let child = self.child.as_mut().ok_or("monitor.probe")?;
        let output = child.stdout.take().ok_or("monitor.probe")?;
        let result = tokio::time::timeout(Duration::from_secs(2), async {
            let mut bytes = Vec::new();
            output
                .take(65537)
                .read_to_end(&mut bytes)
                .await
                .map_err(|_| "monitor.probe")?;
            if bytes.len() > 65536 {
                return Err("monitor.output");
            }
            if !child.wait().await.map_err(|_| "monitor.probe")?.success() {
                return Err("monitor.probe");
            }
            String::from_utf8(bytes).map_err(|_| "monitor.output")
        })
        .await
        .unwrap_or(Err("monitor.timeout"));
        if result.is_err() {
            let _ = child.start_kill();
        }
        if child.try_wait().ok().flatten().is_some() {
            self.child = None;
        }
        result
    }
}

pub(crate) fn page(args: &[&str]) -> Result<usize> {
    match args {
        [] => Ok(1),
        [p] => p
            .parse::<usize>()
            .ok()
            .filter(|p| (1..=1000).contains(p))
            .ok_or("request.invalid"),
        _ => Err("request.invalid"),
    }
}
pub(crate) fn header(
    title: &str,
    server: &str,
    at: Option<chrono::DateTime<chrono::Utc>>,
    enabled: bool,
    is_stale: bool,
    locale: &str,
) -> String {
    use garde_entrypoint::message;
    let mut text = format!(
        "{}: {}\n{}: {}",
        message(locale, title),
        crate::isolate(server),
        message(locale, "observed_at"),
        crate::isolate(
            &at.map(|v| v.to_rfc3339())
                .unwrap_or_else(|| message(locale, "monitor_waiting").into())
        )
    );
    if !enabled {
        text.push_str(&format!("\n{}", message(locale, "monitor_disabled")));
    } else if is_stale {
        text.push_str(&format!("\n{}", message(locale, "monitor_stale")));
    }
    text
}
pub(crate) fn page_rows(mut header: String, rows: &[String], page: usize, locale: &str) -> String {
    use garde_entrypoint::message;
    let pages = rows.len().div_ceil(8).max(1);
    if page > pages {
        return message(locale, "monitor_usage").into();
    }
    for row in rows.iter().skip((page - 1) * 8).take(8) {
        header.push_str(&format!("\n{}", crate::isolate(row)));
    }
    header.push_str(&format!(
        "\n{} {page}/{pages}",
        message(locale, "monitor_page")
    ));
    header
}

pub(crate) async fn read_helper(
    path: &std::path::Path,
    request: &[u8],
    limit: u64,
) -> Result<Vec<u8>> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    use tokio::io::AsyncWriteExt;
    tokio::time::timeout(Duration::from_secs(5), async {
        let metadata = std::fs::symlink_metadata(path).map_err(|_| "helper.unavailable")?;
        if !metadata.file_type().is_socket() || metadata.uid() != 0 || metadata.mode() & 7 != 0 {
            return Err("helper.peer");
        }
        let mut stream = tokio::net::UnixStream::connect(path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                "helper.permissions"
            } else {
                "helper.unavailable"
            }
        })?;
        if stream.peer_cred().map_err(|_| "helper.peer")?.uid() != 0 {
            return Err("helper.peer");
        }
        stream
            .write_all(request)
            .await
            .map_err(|_| "helper.unavailable")?;
        let mut bytes = vec![];
        stream
            .take(limit + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| "helper.output")?;
        if bytes.len() as u64 > limit {
            return Err("helper.output");
        }
        Ok(bytes)
    })
    .await
    .map_err(|_| "helper.timeout")?
}
