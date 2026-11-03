use garde_config::Result;
use garde_contracts::{Action, AddressFamily, Id, ListenerReadResponse, WireVersion};
use serde::Deserialize;
use std::{future::Future, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: WireVersion,
    action: Action,
}
pub trait ListenerSource: Send + Sync {
    fn read(&self, family: AddressFamily) -> impl Future<Output = Result<String>> + Send;
}
pub struct SystemListeners;
impl ListenerSource for SystemListeners {
    async fn read(&self, family: AddressFamily) -> Result<String> {
        let mut child = tokio::process::Command::new("/usr/bin/ss")
            .args([
                "-H",
                "-O",
                "-l",
                "-n",
                "-t",
                "-u",
                "-p",
                if family == AddressFamily::Ipv4 {
                    "-4"
                } else {
                    "-6"
                },
            ])
            .env_clear()
            .env("LC_ALL", "C")
            .current_dir("/")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| "ports.unavailable")?;
        let output = child.stdout.take().ok_or("helper.output")?;
        let result = tokio::time::timeout(Duration::from_millis(1500), async {
            let mut bytes = vec![];
            output
                .take(65537)
                .read_to_end(&mut bytes)
                .await
                .map_err(|_| "helper.output")?;
            if bytes.len() > 65536 {
                return Err("helper.output");
            }
            if !child
                .wait()
                .await
                .map_err(|_| "ports.unavailable")?
                .success()
            {
                return Err("ports.unavailable");
            }
            String::from_utf8(bytes).map_err(|_| "helper.output")
        })
        .await
        .unwrap_or(Err("helper.timeout"));
        if result.is_err() {
            let _ = child.start_kill();
        }
        result
    }
}
pub async fn serve_connection(
    mut stream: UnixStream,
    allowed_uid: u32,
    source: &impl ListenerSource,
) -> Result<()> {
    if allowed_uid == 0 || stream.peer_cred().map_err(|_| "helper.peer")?.uid() != allowed_uid {
        return Err("helper.peer");
    }
    let result = tokio::time::timeout(Duration::from_secs(4), async {
        let mut bytes = vec![];
        tokio::io::BufReader::new(&mut stream)
            .take(1025)
            .read_until(b'\n', &mut bytes)
            .await
            .map_err(|_| "helper.request")?;
        if bytes.pop() != Some(b'\n') || bytes.len() > 1024 {
            return Err("helper.request");
        }
        let request: Request = serde_json::from_slice(&bytes).map_err(|_| "helper.request")?;
        if request.version != WireVersion::V1 || !matches!(request.action, Action::PortsStatus {}) {
            return Err("helper.request");
        }
        let ipv4 = source.read(AddressFamily::Ipv4).await?;
        let ipv6 = source.read(AddressFamily::Ipv6).await?;
        if ipv4.len() > 65536 || ipv6.len() > 65536 {
            return Err("helper.output");
        }
        Ok((ipv4, ipv6))
    })
    .await
    .unwrap_or(Err("helper.timeout"));
    let response = match result {
        Ok((ipv4, ipv6)) => ListenerReadResponse::Snapshot { ipv4, ipv6 },
        Err(code) => ListenerReadResponse::Unavailable {
            reason_key: Id::new(code)?,
        },
    };
    let bytes = serde_json::to_vec(&response).map_err(|_| "helper.output")?;
    if bytes.len() > 524288 {
        return Err("helper.output");
    }
    tokio::time::timeout(Duration::from_millis(500), stream.write_all(&bytes))
        .await
        .map_err(|_| "helper.timeout")?
        .map_err(|_| "helper.output")
}
