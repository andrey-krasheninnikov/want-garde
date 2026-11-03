pub(crate) use crate::ingest::State;
pub use crate::ingest::{Collector, Journal, JournalQuery, JournalSource};
use crate::runtime::now;
use garde_config::Config;
use garde_contracts::{Classification, Event, Evidence, Id, Severity, WireVersion};
use sha2::{Digest, Sha256};
use std::sync::LazyLock;

pub(crate) fn file_message(line: &str) -> Option<&str> {
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

pub(crate) fn parse(line: &str, config: &Config, position: &[u8]) -> Option<Event> {
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
