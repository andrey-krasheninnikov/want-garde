pub mod listeners;
use garde_contracts::{Action, WireVersion};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: WireVersion,
    action: Action,
}
pub fn decode_request(input: &str) -> Result<(), &'static str> {
    if input.len() > 1024 {
        return Err("helper.request");
    }
    let request: Request = serde_json::from_str(input).map_err(|_| "helper.request")?;
    match (request.version, request.action) {
        (WireVersion::V1, Action::Fail2banStatus {}) => Ok(()),
        _ => Err("helper.request"),
    }
}

use garde_contracts::{BansSnapshot, Id, JailBans};
use std::future::Future;

pub enum Query {
    Status,
    Jail(String),
    LogTarget,
    LogLevel,
}
pub trait StatusSource: Send + Sync {
    fn query(&self, query: Query) -> impl Future<Output = Result<String, &'static str>> + Send;
}
fn valid_jail(jail: &str) -> bool {
    !jail.is_empty()
        && jail.len() <= 128
        && jail.as_bytes()[0].is_ascii_alphanumeric()
        && jail
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}
fn field<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    text.lines()
        .find_map(|line| line.split_once(name).map(|(_, value)| value.trim()))
}
pub async fn status(source: &impl StatusSource) -> Result<BansSnapshot, &'static str> {
    let output = source.query(Query::Status).await?;
    if output.len() > 65536 {
        return Err("helper.output");
    }
    let jails = field(&output, "Jail list:").ok_or("helper.output")?;
    let all: Vec<_> = jails
        .split(',')
        .map(str::trim)
        .filter(|j| !j.is_empty())
        .collect();
    if all.iter().any(|j| !valid_jail(j)) {
        return Err("helper.output");
    }
    let mut result = BansSnapshot {
        version: WireVersion::V1,
        observed_at: chrono::Utc::now(),
        jails: vec![],
        partial: all.len() > 32,
        log_target: None,
        log_level: None,
    };
    let mut remaining = 1024usize;
    for jail in all.into_iter().take(32) {
        let read = async {
            let text = source.query(Query::Jail(jail.into())).await?;
            if text.len() > 65536 {
                return Err("helper.output");
            }
            let count = field(&text, "Currently banned:")
                .and_then(|n| n.parse::<u32>().ok())
                .ok_or("helper.output")?;
            let mut addresses = Vec::new();
            let list = field(&text, "Banned IP list:").ok_or("helper.output")?;
            let mut total = 0usize;
            for address in list.split_whitespace() {
                let ip = address
                    .parse::<std::net::IpAddr>()
                    .map_err(|_| "helper.output")?;
                total += 1;
                if addresses.len() < remaining {
                    addresses.push(ip.to_string());
                }
            }
            if total != count as usize {
                return Err("helper.output");
            }
            Ok::<_, &'static str>((count, addresses, total > remaining))
        }
        .await;
        let row = match read {
            Ok((count, addresses, truncated)) => {
                remaining -= addresses.len();
                result.partial |= truncated;
                JailBans {
                    jail: Id::new(jail).map_err(|_| "helper.output")?,
                    current_banned: Some(count),
                    addresses,
                    reason_key: None,
                }
            }
            Err(code) => {
                result.partial = true;
                JailBans {
                    jail: Id::new(jail).map_err(|_| "helper.output")?,
                    current_banned: None,
                    addresses: vec![],
                    reason_key: Some(Id::new(code).map_err(|_| "helper.output")?),
                }
            }
        };
        result.jails.push(row);
    }
    if let Ok(target) = source.query(Query::LogTarget).await {
        result.log_target = target
            .lines()
            .find_map(|line| line.trim().strip_prefix("`- "))
            .filter(|s| !s.is_empty() && s.len() <= 4096 && !s.chars().any(char::is_control))
            .map(str::to_owned);
    }
    if let Ok(level) = source.query(Query::LogLevel).await {
        result.log_level = level
            .split('\'')
            .nth(1)
            .filter(|s| {
                [
                    "CRITICAL",
                    "ERROR",
                    "WARNING",
                    "NOTICE",
                    "INFO",
                    "DEBUG",
                    "TRACEDEBUG",
                    "HEAVYDEBUG",
                ]
                .contains(s)
            })
            .map(str::to_owned);
    }
    Ok(result)
}

pub struct Fail2ban;
impl StatusSource for Fail2ban {
    async fn query(&self, query: Query) -> Result<String, &'static str> {
        use std::{process::Stdio, time::Duration};
        use tokio::io::AsyncReadExt;
        let mut command = tokio::process::Command::new("/usr/bin/fail2ban-client");
        match query {
            Query::Status => {
                command.arg("status");
            }
            Query::Jail(jail) if valid_jail(&jail) => {
                command.args(["status", &jail]);
            }
            Query::LogTarget => {
                command.args(["get", "logtarget"]);
            }
            Query::LogLevel => {
                command.args(["get", "loglevel"]);
            }
            _ => return Err("helper.request"),
        }
        let mut child = command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("LC_ALL", "C.UTF-8")
            .env("PYTHONNOUSERSITE", "1")
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .current_dir("/")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| "fail2ban.unavailable")?;
        let output = child.stdout.take().ok_or("helper.output")?;
        let read = tokio::time::timeout(Duration::from_secs(1), async {
            let mut bytes = Vec::new();
            output
                .take(65537)
                .read_to_end(&mut bytes)
                .await
                .map_err(|_| "helper.output")?;
            if bytes.len() > 65536 {
                return Err("helper.output");
            }
            String::from_utf8(bytes).map_err(|_| "helper.output")
        })
        .await;
        let result = match read {
            Ok(value) => value,
            Err(_) => Err("helper.timeout"),
        };
        if result.is_err() {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return result;
        }
        let exited = tokio::time::timeout(Duration::from_millis(250), child.wait()).await;
        if !exited.is_ok_and(|status| status.is_ok_and(|s| s.success())) {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err("fail2ban.unavailable");
        }
        result
    }
}

pub async fn serve_connection(
    mut stream: tokio::net::UnixStream,
    allowed_uid: u32,
    source: &impl StatusSource,
) -> Result<(), &'static str> {
    use garde_contracts::HelperResponse;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
    if allowed_uid == 0 || stream.peer_cred().map_err(|_| "helper.peer")?.uid() != allowed_uid {
        return Err("helper.peer");
    }
    let result = tokio::time::timeout(std::time::Duration::from_secs(4), async {
        let mut bytes = Vec::new();
        let mut reader = tokio::io::BufReader::new(&mut stream).take(1025);
        reader
            .read_until(b'\n', &mut bytes)
            .await
            .map_err(|_| "helper.request")?;
        if bytes.pop() != Some(b'\n') {
            return Err("helper.request");
        }
        decode_request(std::str::from_utf8(&bytes).map_err(|_| "helper.request")?)?;
        status(source).await
    })
    .await
    .map_err(|_| "helper.timeout")
    .and_then(|r| r);
    let response = match result {
        Ok(snapshot) => HelperResponse::Snapshot { snapshot },
        Err(reason) => HelperResponse::Unavailable {
            reason_key: Id::new(reason).map_err(|_| "helper.output")?,
        },
    };
    let bytes = serde_json::to_vec(&response).map_err(|_| "helper.output")?;
    if bytes.len() > 65536 {
        return Err("helper.output");
    }
    tokio::time::timeout(
        std::time::Duration::from_millis(500),
        stream.write_all(&bytes),
    )
    .await
    .map_err(|_| "helper.timeout")?
    .map_err(|_| "helper.output")?;
    Ok(())
}
pub mod xui;
