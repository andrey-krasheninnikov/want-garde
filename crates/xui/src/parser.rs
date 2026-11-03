use crate::{fingerprint, label};
use garde_contracts::{Evidence, Id, XuiLogRecord};
use regex::Regex;
use std::{net::IpAddr, sync::OnceLock};
fn ip(raw: &str) -> Option<String> {
    raw.trim_matches(['[', ']', '"', ',', ';'])
        .parse::<IpAddr>()
        .ok()
        .map(|v| v.to_string())
}
fn first_ip(raw: &str) -> Option<String> {
    static QUOTED: OnceLock<Regex> = OnceLock::new();
    static NAMED: OnceLock<Regex> = OnceLock::new();
    static V4: OnceLock<Regex> = OnceLock::new();
    let quoted =
        QUOTED.get_or_init(|| Regex::new(r#"(?:^|[,\s])IP="([^"]*)""#).expect("static_pattern"));
    if let Some(value) = quoted.captures(raw) {
        return ip(&value[1]);
    }
    let named = NAMED.get_or_init(|| {
        Regex::new(r"(?i)\b(?:ip|ip address|ipaddress)\s*[:=]\s*([0-9a-f:.\[\]]+)")
            .expect("static_pattern")
    });
    if let Some(value) = named.captures_iter(raw).find_map(|v| ip(&v[1])) {
        return Some(value);
    }
    if let Some(value) = raw
        .split(|c: char| c.is_whitespace() || ['=', ',', '"', '(', ')'].contains(&c))
        .find_map(ip)
    {
        return Some(value);
    }
    V4.get_or_init(|| Regex::new(r"\b[0-9]{1,3}(?:\.[0-9]{1,3}){3}\b").expect("static_pattern"))
        .find_iter(raw)
        .find_map(|v| ip(v.as_str()))
}
fn endpoint(raw: &str, source: bool) -> Option<String> {
    if raw.len() > 255 || !raw.is_ascii() || raw.contains(['/', '?', '#', '@']) {
        return None;
    }
    let (host, port) = raw.rsplit_once(':')?;
    let port = port.parse::<u16>().ok()?;
    if let Some(address) = ip(host) {
        return Some(if address.contains(':') {
            format!("[{address}]:{port}")
        } else {
            format!("{address}:{port}")
        });
    }
    if !source
        && !host.is_empty()
        && host.len() <= 253
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b))
    {
        Some(format!("{host}:{port}"))
    } else {
        None
    }
}
pub fn parse(raw: &str, suspicious: bool, position: &[u8], at: i64) -> Option<XuiLogRecord> {
    let raw = raw.trim_end_matches(['\r', '\n']);
    if raw.len() > 4096 || raw.contains(['\0', '\r']) {
        return None;
    }
    let raw = raw.trim();
    static MODERN: OnceLock<Regex> = OnceLock::new();
    static OLD: OnceLock<Regex> = OnceLock::new();
    static ACTIVITY: OnceLock<Regex> = OnceLock::new();
    static FAILED: OnceLock<Regex> = OnceLock::new();
    let modern = MODERN.get_or_init(|| {
        Regex::new(r#"logged in successfully: username=("(?:[^"\\]|\\.)*"), IP=("[^"]+")"#)
            .expect("static_pattern")
    });
    let old=OLD.get_or_init(||Regex::new(r"(?i)^(?P<account>[^\r\n]+?) logged in successfully(?:, Ip Address: (?P<ip>[a-fA-F0-9.:]+))?").expect("static_pattern"));
    let activity=ACTIVITY.get_or_init(||Regex::new(r"(?i)from (?P<src>\S+) accepted (?P<network>tcp|udp):(?P<target>\S+) \[(?P<inbound>[^\s\]]+)\s*>>\s*[^\]\s]+\](?:\s*email:\s*(?P<email>\S+))?").expect("static_pattern"));
    let failed=FAILED.get_or_init(||Regex::new(r"(?i)login failed|failed login|failed to log in|invalid.*login|invalid username or password|wrong username|wrong password").expect("static_pattern"));
    let mut values: Vec<(&str, String)> = vec![];
    let reason = if suspicious {
        values.push(("xui_ip", first_ip(raw)?));
        "xui_suspicious_ip"
    } else if let Some(c) = modern.captures(raw) {
        let account: String = serde_json::from_str(&c[1]).ok()?;
        let address: String = serde_json::from_str(&c[2]).ok()?;
        values.push(("xui_account", label(&account).ok()?));
        values.push(("xui_ip", ip(&address)?));
        "xui_panel_login"
    } else if failed.is_match(raw) {
        if let Some(address) = first_ip(raw) {
            values.push(("xui_ip", address))
        };
        "xui_login_failed"
    } else if let Some(c) = old.captures(raw.rsplit_once("INFO - ").map(|(_, s)| s).unwrap_or(raw))
    {
        values.push((
            "xui_account",
            label(c.name("account")?.as_str().trim()).ok()?,
        ));
        if let Some(address) = c
            .name("ip")
            .and_then(|v| ip(v.as_str()))
            .or_else(|| first_ip(raw))
        {
            values.push(("xui_ip", address))
        };
        "xui_panel_login"
    } else if let Some(c) = activity.captures(raw) {
        if let Some(email) = c.name("email") {
            values.push(("xui_client", label(email.as_str()).ok()?))
        }
        values.push(("xui_inbound", label(&c["inbound"]).ok()?));
        values.push(("xui_source", endpoint(&c["src"], true)?));
        values.push(("xui_target", endpoint(&c["target"], false)?));
        "xui_client_activity"
    } else {
        return None;
    };
    Some(XuiLogRecord {
        id: Id::new(format!("xui-log-{}", fingerprint(position))).ok()?,
        observed_at: chrono::DateTime::from_timestamp(at, 0)?,
        reason_key: Id::new(reason).ok()?,
        evidence: values
            .into_iter()
            .map(|(name, value)| Evidence {
                name: Id::new(name).expect("static_identifier"),
                value,
            })
            .collect(),
    })
}
