use crate::{ingest::Kind, runtime::now};
use garde_config::{Config, FirewallBackend};
use garde_contracts::{Classification, Event, Evidence, Id, Severity, WireVersion};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, sync::LazyLock};

pub(crate) fn file_message(kind: Kind, line: &str) -> Option<&str> {
    static KERNEL: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
        r"^(?:(?:[A-Z][a-z]{2}\s+[0-9]{1,2}\s+[0-9]{2}:[0-9]{2}:[0-9]{2}|[0-9]{4}-[0-9]{2}-[0-9]{2}T\S+)\s+\S+\s+)?kernel(?:\[[0-9]+\])?:\s*(?:\[\s*[0-9.]+\]\s*)?(?P<message>.*)$"
    ).expect("static_pattern")
    });
    static ACTIONS: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
        r"^(?:[0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2}:[0-9]{2},[0-9]{3}\s+)?fail2ban\.actions\s+\[[0-9]+\]:\s+(?:NOTICE|INFO|WARNING)\s+(?P<message>.*)$"
    ).expect("static_pattern")
    });
    static SYSLOG: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
        r"^(?:(?:[A-Z][a-z]{2}\s+[0-9]{1,2}\s+[0-9]{2}:[0-9]{2}:[0-9]{2}|[0-9]{4}-[0-9]{2}-[0-9]{2}T\S+)\s+\S+\s+)?fail2ban(?:-server|\.actions)?(?:\[[0-9]+\])?:\s+(?P<message>.*)$"
    ).expect("static_pattern")
    });
    let line = line.trim_end();
    match kind {
        Kind::Fail2ban => ACTIONS
            .captures(line)
            .or_else(|| SYSLOG.captures(line))?
            .name("message")
            .map(|m| m.as_str()),
        Kind::Firewall => KERNEL.captures(line)?.name("message").map(|m| m.as_str()),
        _ => None,
    }
}
pub(crate) fn candidate(kind: Kind, line: &str, config: &Config) -> bool {
    static BAN: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"^\[[^\]\r\n]{1,256}\]\s+(?:Restore Ban|Ban|Unban)(?:\s|$)")
            .expect("static_pattern")
    });
    match kind {
        Kind::Fail2ban => BAN.is_match(file_message(kind, line).unwrap_or(line).trim()),
        Kind::Firewall => packet_prefix(line, config).is_some(),
        Kind::Ssh => true,
    }
}

pub(crate) fn parse(kind: Kind, line: &str, config: &Config, position: &[u8]) -> Option<Event> {
    let (key, severity, evidence) = match kind {
        Kind::Fail2ban => ban(line)?,
        Kind::Firewall => packet(line, config)?,
        Kind::Ssh => return None,
    };
    let source = if kind == Kind::Fail2ban {
        "fail2ban"
    } else {
        "firewall"
    };
    let mut hash = Sha256::new();
    hash.update(source.as_bytes());
    hash.update(b"-event-v1\0");
    hash.update(position);
    hash.update(b"\0");
    hash.update(line.as_bytes());
    Some(Event {
        version: WireVersion::V1,
        id: Id::new(format!("{source}:{:x}", hash.finalize())).ok()?,
        server_id: config.server_id.clone(),
        observed_at: chrono::DateTime::from_timestamp(now(), 0)?,
        source: Id::new(source).ok()?,
        classification: Classification {
            category: Id::new(if kind == Kind::Fail2ban {
                "network.ban"
            } else {
                "network.firewall"
            })
            .ok()?,
            severity,
            rule_id: Id::new(key.replace('_', ".")).ok()?,
            reason_key: Id::new(key).ok()?,
        },
        evidence,
    })
}
fn evidence(name: &str, value: impl Into<String>) -> Evidence {
    Evidence {
        name: Id::new(name).expect("static_identifier"),
        value: value.into(),
    }
}
fn ban(line: &str) -> Option<(&'static str, Severity, Vec<Evidence>)> {
    static BAN: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
        r"^\[(?P<jail>[A-Za-z0-9][A-Za-z0-9_.-]{0,127})\]\s+(?P<action>Restore Ban|Ban|Unban)\s+(?P<ip>\S+)(?:\s|$)"
    ).expect("static_pattern")
    });
    let body = file_message(Kind::Fail2ban, line).unwrap_or(line).trim();
    let captures = BAN.captures(body)?;
    let ip = captures
        .name("ip")?
        .as_str()
        .parse::<std::net::IpAddr>()
        .ok()?;
    let (key, severity) = if &captures["action"] == "Unban" {
        ("fail2ban_unban", Severity::Info)
    } else {
        ("fail2ban_ban", Severity::Critical)
    };
    Some((
        key,
        severity,
        vec![
            evidence("jail", &captures["jail"]),
            evidence("ip", ip.to_string()),
        ],
    ))
}
fn packet_prefix<'a>(line: &'a str, config: &Config) -> Option<(&'static str, &'a str)> {
    static FIREWALLD: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
        r"^(?:(?:filter_)?(?:IN|OUT|FWDI|FWDO|FWD)_[A-Za-z0-9_.-]{1,80}_(?:DROP|REJECT)|FINAL_(?:DROP|REJECT)):\s*"
    ).expect("static_pattern")
    });
    let backend = config.firewall.backend;
    let selected = if matches!(backend, FirewallBackend::Auto | FirewallBackend::Ufw)
        && line.starts_with("[UFW BLOCK] ")
    {
        ("ufw", line.strip_prefix("[UFW BLOCK] ")?)
    } else if matches!(backend, FirewallBackend::Auto | FirewallBackend::Firewalld)
        && FIREWALLD.is_match(line)
    {
        let matched = FIREWALLD.find(line)?;
        ("firewalld", &line[matched.end()..])
    } else if matches!(backend, FirewallBackend::Auto | FirewallBackend::Nftables) {
        (
            "nftables",
            line.strip_prefix(config.firewall.log_prefix.as_deref()?)?,
        )
    } else {
        return None;
    };
    Some(selected)
}
fn packet(line: &str, config: &Config) -> Option<(&'static str, Severity, Vec<Evidence>)> {
    let (backend, body) = packet_prefix(line, config)?;
    let mut fields = BTreeMap::new();
    for word in body.split_whitespace() {
        if let Some((name, value)) = word.split_once('=')
            && ["SRC", "DST", "PROTO", "DPT"].contains(&name)
            && fields.insert(name, value).is_some()
        {
            return None;
        }
    }
    let ip = fields.get("SRC")?.parse::<std::net::IpAddr>().ok()?;
    let proto = *fields.get("PROTO")?;
    if proto.is_empty()
        || proto.len() > 16
        || !proto
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return None;
    }
    let mut values = vec![
        evidence("ip", ip.to_string()),
        evidence("backend", backend),
        evidence("proto", proto.to_ascii_uppercase()),
    ];
    if let Some(dst) = fields.get("DST") {
        values.push(evidence(
            "destination",
            dst.parse::<std::net::IpAddr>().ok()?.to_string(),
        ));
    }
    if let Some(port) = fields.get("DPT") {
        values.push(evidence("port", port.parse::<u16>().ok()?.to_string()));
    }
    Some(("firewall_block", Severity::Warning, values))
}

pub trait BansSource: Send + Sync {
    fn snapshot(
        &self,
    ) -> impl std::future::Future<Output = garde_config::Result<garde_contracts::BansSnapshot>> + Send;
}
pub struct HelperClient;
impl BansSource for HelperClient {
    async fn snapshot(&self) -> garde_config::Result<garde_contracts::BansSnapshot> {
        use garde_contracts::{BansSnapshot, HelperResponse};
        let operation = async {
            let bytes = crate::observation::read_helper(
                std::path::Path::new("/run/want-garde-helper.sock"),
                b"{\"version\":\"1\",\"action\":{\"type\":\"fail2ban.status\"}}\n",
                65536,
            )
            .await?;
            match serde_json::from_slice::<HelperResponse>(&bytes).map_err(|_| "helper.output")? {
                HelperResponse::Snapshot { snapshot } => {
                    validate_snapshot(&snapshot)?;
                    Ok(snapshot)
                }
                HelperResponse::Unavailable { reason_key } => Err(match reason_key.as_str() {
                    "helper.request" => "helper.request",
                    "helper.timeout" => "helper.timeout",
                    "helper.output" => "helper.output",
                    "fail2ban.unavailable" => "fail2ban.unavailable",
                    _ => "helper.output",
                }),
            }
        };
        let result: Result<BansSnapshot, _> =
            tokio::time::timeout(std::time::Duration::from_secs(5), operation)
                .await
                .map_err(|_| "helper.timeout")?;
        result
    }
}
fn validate_snapshot(snapshot: &garde_contracts::BansSnapshot) -> garde_config::Result<()> {
    if snapshot.jails.len() > 32
        || snapshot
            .jails
            .iter()
            .map(|j| j.addresses.len())
            .sum::<usize>()
            > 1024
        || snapshot
            .log_target
            .as_ref()
            .is_some_and(|p| p.len() > 4096 || p.chars().any(char::is_control))
        || snapshot
            .log_level
            .as_ref()
            .is_some_and(|p| p.len() > 32 || !p.bytes().all(|b| b.is_ascii_uppercase()))
        || snapshot.jails.iter().any(|j| {
            j.addresses
                .iter()
                .any(|ip| ip.parse::<std::net::IpAddr>().is_err())
                || j.current_banned
                    .is_some_and(|n| n < (j.addresses.len() as u32))
        })
    {
        return Err("helper.output");
    }
    Ok(())
}
pub(crate) fn bans_args<'a>(args: &[&'a str]) -> Result<(Option<&'a str>, usize), ()> {
    let page = |s: &str| {
        s.parse::<usize>()
            .ok()
            .filter(|n| (1..=1_000_000).contains(n))
            .ok_or(())
    };
    let jail = |s: &str| {
        !s.is_empty()
            && s.len() <= 128
            && s.as_bytes()[0].is_ascii_alphanumeric()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
    };
    match args {
        [] => Ok((None, 1)),
        ["--page", n] => Ok((None, page(n)?)),
        [name] if jail(name) => Ok((Some(name), 1)),
        [name, n] if jail(name) => Ok((Some(name), page(n)?)),
        _ => Err(()),
    }
}
pub(crate) fn render_bans(
    snapshot: &garde_contracts::BansSnapshot,
    jail: Option<&str>,
    page: usize,
    locale: &str,
) -> String {
    use garde_entrypoint::message;
    let mut lines = vec![
        message(locale, "bans_title").into(),
        format!(
            "{}: {}",
            message(locale, "observed_at"),
            crate::isolate(&snapshot.observed_at.to_rfc3339())
        ),
    ];
    if snapshot.partial {
        lines.push(message(locale, "bans_partial").into());
    }
    if let Some(name) = jail {
        let Some(row) = snapshot.jails.iter().find(|j| j.jail.as_str() == name) else {
            return message(locale, "bans_unknown_jail").into();
        };
        lines.push(crate::isolate(name));
        if let Some(reason) = &row.reason_key {
            lines.push(crate::isolate(reason.as_str()));
        }
        let count = row
            .current_banned
            .map(|n| n.to_string())
            .unwrap_or_else(|| "?".into());
        lines.push(format!(
            "{}: {}",
            message(locale, "bans_count"),
            crate::isolate(&count)
        ));
        let start = (page - 1) * 20;
        for ip in row.addresses.iter().skip(start).take(20) {
            lines.push(crate::isolate(ip));
        }
        if start >= row.addresses.len() {
            lines.push(message(locale, "bans_no_entries").into());
        }
        lines.push(format!(
            "{}: {} / {}",
            message(locale, "bans_page"),
            page,
            row.addresses.len().div_ceil(20).max(1)
        ));
    } else {
        for row in snapshot.jails.iter().skip((page - 1) * 10).take(10) {
            lines.push(format!(
                "{}: {}",
                crate::isolate(row.jail.as_str()),
                crate::isolate(
                    &row.current_banned
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "?".into())
                )
            ));
        }
        if snapshot.jails.is_empty() {
            lines.push(message(locale, "bans_no_entries").into());
        }
        lines.push(format!(
            "{}: {} / {}",
            message(locale, "bans_page"),
            page,
            snapshot.jails.len().div_ceil(10).max(1)
        ));
    }
    lines.push(message(locale, "bans_usage").into());
    lines.join("\n")
}

pub(crate) async fn probe(
    kind: Kind,
    config: &Config,
    bans: &impl BansSource,
) -> garde_config::Result<garde_config::SshConfig> {
    use garde_config::{FirewallBackend, SshConfig};
    if kind == Kind::Fail2ban {
        let snapshot = bans.snapshot().await?;
        if !snapshot
            .log_level
            .as_deref()
            .is_some_and(|s| ["NOTICE", "INFO", "DEBUG", "TRACEDEBUG", "HEAVYDEBUG"].contains(&s))
        {
            return Err("fail2ban.logging_insufficient");
        }
        let target = snapshot.log_target.ok_or("fail2ban.target_unknown")?;
        return if target.starts_with('/') {
            let path = std::path::PathBuf::from(target);
            if path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                return Err("fail2ban.target_unknown");
            }
            Ok(SshConfig::File { path })
        } else if ["STDOUT", "STDERR", "SYSOUT", "SYSLOG", "SYSTEMD-JOURNAL"]
            .contains(&target.as_str())
        {
            Ok(SshConfig::Journald {})
        } else {
            Err("fail2ban.target_unknown")
        };
    }
    let backend = config.firewall.backend;
    if backend == FirewallBackend::Nftables {
        if config.firewall.log_prefix.is_none() {
            return Err("firewall.prefix_missing");
        }
        return Ok(SshConfig::Journald {});
    }
    if matches!(backend, FirewallBackend::Auto | FirewallBackend::Ufw)
        && unit_active("ufw.service").await
    {
        let conf = read_config("/etc/ufw/ufw.conf")?;
        let setting = |key: &str| {
            conf.lines()
                .filter_map(|line| line.split('#').next()?.trim().split_once('='))
                .find(|(name, _)| name.trim() == key)
                .map(|(_, value)| value.trim().trim_matches(['\'', '"']))
        };
        if setting("ENABLED") != Some("yes") {
            return Err("firewall.disabled");
        }
        if !setting("LOGLEVEL").is_some_and(|s| ["low", "medium", "high", "full"].contains(&s)) {
            return Err("firewall.logging_disabled");
        }
        return Ok(if readable_log("/var/log/ufw.log") {
            SshConfig::File {
                path: "/var/log/ufw.log".into(),
            }
        } else {
            SshConfig::Journald {}
        });
    }
    if matches!(backend, FirewallBackend::Auto | FirewallBackend::Firewalld)
        && unit_active("firewalld.service").await
    {
        let denied = read_command("/usr/bin/firewall-cmd", &["--get-log-denied"]).await?;
        if !["all", "unicast", "broadcast", "multicast"].contains(&denied.trim()) {
            return Err("firewall.logging_disabled");
        }
        return Ok(SshConfig::Journald {});
    }
    Err("firewall.service_missing")
}
fn readable_log(path: &str) -> bool {
    crate::ingest::open_log(std::path::Path::new(path)).is_ok()
}
fn read_config(path: &str) -> garde_config::Result<String> {
    use std::{
        fs::OpenOptions,
        io::Read,
        os::unix::fs::{MetadataExt, OpenOptionsExt},
    };
    let file = OpenOptions::new()
        .read(true)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32)
        .open(path)
        .map_err(|_| "firewall.config_unavailable")?;
    let info = file.metadata().map_err(|_| "firewall.config_unavailable")?;
    if !info.is_file() || info.uid() != 0 || info.mode() & 0o022 != 0 {
        return Err("firewall.config_unavailable");
    }
    let mut bytes = Vec::new();
    file.take(65537)
        .read_to_end(&mut bytes)
        .map_err(|_| "firewall.config_unavailable")?;
    if bytes.len() > 65536 {
        return Err("firewall.config_unavailable");
    }
    String::from_utf8(bytes).map_err(|_| "firewall.config_unavailable")
}
async fn unit_active(unit: &str) -> bool {
    read_command(
        "/usr/bin/systemctl",
        &[
            "show",
            unit,
            "--property=LoadState,ActiveState",
            "--no-pager",
        ],
    )
    .await
    .is_ok_and(|s| {
        s.lines().any(|l| l == "LoadState=loaded") && s.lines().any(|l| l == "ActiveState=active")
    })
}
async fn read_command(executable: &str, args: &[&str]) -> garde_config::Result<String> {
    use std::{process::Stdio, time::Duration};
    use tokio::io::AsyncReadExt;
    let mut child = tokio::process::Command::new(executable)
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LC_ALL", "C.UTF-8")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| "firewall.probe_unavailable")?;
    let output = child.stdout.take().ok_or("firewall.probe_unavailable")?;
    let result = tokio::time::timeout(Duration::from_secs(1), async {
        let mut bytes = Vec::new();
        output
            .take(4097)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| "firewall.probe_unavailable")?;
        if bytes.len() > 4096 {
            return Err("firewall.probe_unavailable");
        }
        String::from_utf8(bytes).map_err(|_| "firewall.probe_unavailable")
    })
    .await
    .map_err(|_| "firewall.probe_unavailable")
    .and_then(|r| r);
    if result.is_err() {
        let _ = child.kill().await;
        let _ = child.wait().await;
        return result;
    }
    if !tokio::time::timeout(Duration::from_millis(250), child.wait())
        .await
        .is_ok_and(|r| r.is_ok_and(|s| s.success()))
    {
        let _ = child.kill().await;
        let _ = child.wait().await;
        return Err("firewall.probe_unavailable");
    }
    result
}

pub(crate) fn render_health(health: &garde_contracts::FirewallHealth, locale: &str) -> String {
    use garde_contracts::SshStatus;
    use garde_entrypoint::message;
    let mut lines = vec![message(locale, "firewall_title").into()];
    for (name, source) in [
        ("fail2ban", &health.fail2ban),
        ("firewall", &health.firewall),
    ] {
        let key = match source.state {
            SshStatus::Starting => "ssh_starting",
            SshStatus::Healthy => "ssh_healthy",
            SshStatus::Degraded => "source_degraded",
            SshStatus::Unavailable => "ssh_unavailable",
        };
        lines.push(format!(
            "{}: {}",
            crate::isolate(name),
            message(locale, key)
        ));
        lines.push(format!(
            "{}: {}; {}: {}",
            message(locale, "ssh_gaps"),
            source.gap_count,
            message(locale, "ssh_skipped"),
            source.skipped_records
        ));
        for reason in [&source.reason_key, &source.last_gap].into_iter().flatten() {
            lines.push(crate::isolate(reason.as_str()));
        }
    }
    lines.join("\n")
}
