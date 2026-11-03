use crate::{
    observation::{publication, stale},
    runtime::Database,
};
use garde_config::{Config, Result};
use garde_contracts::{
    AddressFamily as Family, BaselinePreview, Endpoint, Id, Listener, ListenerProcess,
    ListenerProtocol as Protocol, PortBaseline, PortsHealth, PortsSnapshot, Severity,
    SuspiciousListener, WireVersion,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
pub struct RawSnapshot {
    pub ipv4: Result<String>,
    pub ipv6: Result<String>,
    pub process_reason: Option<&'static str>,
    pub ephemeral_range: Option<[u16; 2]>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    version: u8,
    sequence: u64,
    snapshot: PortsSnapshot,
    baseline: Option<PortBaseline>,
    last: Vec<Endpoint>,
    suspicious: Vec<SuspiciousListener>,
    problems: Vec<Id>,
}
fn digest<T: Serialize>(value: &T) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).map_err(|_| "ports.state")?)
    ))
}
fn decode(raw: &str) -> Result<State> {
    if raw.len() > 524288 {
        return Err("ports.state");
    }
    let state: State = serde_json::from_str(raw).map_err(|_| "ports.state")?;
    if state.version != 1
        || state.snapshot.listeners.len() > 512
        || state.last.len() > 512
        || state.suspicious.len() > 2048
        || state.problems.len() > 32
        || state.baseline.as_ref().is_some_and(|b| {
            b.revision == 0
                || b.endpoints.len() > 512
                || b.hash.len() != 64
                || b.filter_hash.len() != 64
                || !b
                    .hash
                    .bytes()
                    .chain(b.filter_hash.bytes())
                    .all(|v| v.is_ascii_hexdigit())
                || b.endpoints.iter().any(|e| !valid_endpoint(e))
        })
        || state.snapshot.listeners.iter().any(|l| {
            !valid_endpoint(&l.endpoint)
                || l.processes.len() > 4
                || l.processes.iter().any(|p| {
                    p.pid == 0
                        || p.name.is_empty()
                        || p.name.len() > 64
                        || p.name.chars().any(char::is_control)
                })
        })
        || state.last.iter().any(|e| !valid_endpoint(e))
        || state.snapshot.observed_at.timestamp() < 0
        || state.suspicious.iter().any(|s| {
            !valid_endpoint(&s.endpoint)
                || s.process_name.len() > 64
                || s.process_name.chars().any(char::is_control)
        })
    {
        return Err("ports.state");
    }
    Ok(state)
}
pub(crate) fn validate_state(raw: &str) -> Result<()> {
    decode(raw).map(|_| ())
}
fn valid_endpoint(e: &Endpoint) -> bool {
    e.port > 0 && e.address.len() <= 80 && parse_address(&e.address, e.family).is_some()
}
fn parse_address(address: &str, family: Family) -> Option<String> {
    let address = if let Some(rest) = address.strip_prefix('[') {
        rest.strip_suffix(']')?
    } else {
        if address.contains(['[', ']']) {
            return None;
        }
        address
    };
    let (ip, zone) = match address.split_once('%') {
        Some((ip, zone)) => (ip, Some(zone)),
        None => (address, None),
    };
    if zone.is_some_and(|z| {
        z.is_empty()
            || z.len() > 32
            || !z
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
    }) {
        return None;
    }
    let normalized = if ip == "*" {
        match family {
            Family::Ipv4 => "0.0.0.0".into(),
            Family::Ipv6 => "::".into(),
        }
    } else {
        match (family, ip.parse::<std::net::IpAddr>().ok()?) {
            (Family::Ipv4, std::net::IpAddr::V4(v)) => v.to_string(),
            (Family::Ipv6, std::net::IpAddr::V6(v)) => v.to_string(),
            _ => return None,
        }
    };
    Some(match zone {
        Some(z) => format!("{normalized}%{z}"),
        None => normalized,
    })
}
fn processes(tail: &str) -> (Vec<ListenerProcess>, Option<Id>) {
    use std::sync::OnceLock;
    static PROCESS: OnceLock<regex::Regex> = OnceLock::new();
    let pattern = PROCESS.get_or_init(|| {
        regex::Regex::new(r#"\("([^"\x00-\x1f]{1,64})",pid=([0-9]+),fd=[0-9]+\)"#)
            .expect("static_pattern")
    });
    let unavailable = || Some(Id::new("ports.processes_partial").expect("static_identifier"));
    let Some(body) = tail
        .trim()
        .strip_prefix("users:(")
        .and_then(|v| v.strip_suffix(')'))
    else {
        return (vec![], unavailable());
    };
    let mut result = BTreeSet::new();
    let mut end = 0;
    for capture in pattern.captures_iter(body) {
        let whole = capture.get(0).expect("whole_match");
        if body[end..whole.start()] != *if end == 0 { "" } else { "," } {
            return (vec![], unavailable());
        }
        if capture[1].len() > 64 || capture[1].chars().any(char::is_control) {
            return (vec![], unavailable());
        }
        let Some(pid) = capture[2].parse::<u32>().ok().filter(|v| *v > 0) else {
            return (vec![], unavailable());
        };
        result.insert(ListenerProcess {
            pid,
            name: capture[1].into(),
        });
        end = whole.end();
    }
    let partial = end != body.len() || result.is_empty() || result.len() > 4;
    (
        result.into_iter().take(4).collect(),
        if partial { unavailable() } else { None },
    )
}
fn pattern_matches(pattern: &str, value: &str) -> bool {
    let p: Vec<_> = pattern.chars().collect();
    let v: Vec<_> = value.chars().collect();
    fn token(p: &[char], i: usize, c: char) -> Option<usize> {
        if i >= p.len() {
            return None;
        }
        if p[i] == '?' {
            return Some(i + 1);
        }
        if p[i] == '[' {
            let mut start = i + 1;
            let negated = p.get(start) == Some(&'!');
            if negated {
                start += 1;
            }
            let mut end = start;
            if p.get(end) == Some(&']') {
                end += 1;
            }
            while end < p.len() && p[end] != ']' {
                end += 1;
            }
            if end < p.len() {
                let mut matched = false;
                let mut k = start;
                while k < end {
                    if k + 2 < end && p[k + 1] == '-' {
                        matched |= p[k] <= c && c <= p[k + 2];
                        k += 3;
                    } else {
                        matched |= p[k] == c;
                        k += 1;
                    }
                }
                return (matched != negated).then_some(end + 1);
            }
        }
        (p[i] == c && p[i] != '*').then_some(i + 1)
    }
    let (mut i, mut j, mut star, mut retry) = (0, 0, None, 0);
    while j < v.len() {
        if let Some(next) = token(&p, i, v[j]) {
            i = next;
            j += 1;
        } else if p.get(i) == Some(&'*') {
            star = Some(i);
            i += 1;
            retry = j;
        } else if let Some(s) = star {
            retry += 1;
            j = retry;
            i = s + 1;
        } else {
            return false;
        }
    }
    while p.get(i) == Some(&'*') {
        i += 1;
    }
    i == p.len()
}
fn ignored(config: &Config, e: &Endpoint, range: [u16; 2]) -> bool {
    let label = endpoint_label(e);
    let plain = format!(
        "{} {}:{}",
        if e.protocol == Protocol::Tcp {
            "tcp"
        } else {
            "udp"
        },
        e.address,
        e.port
    );
    config
        .ports
        .ignore_patterns
        .iter()
        .any(|p| pattern_matches(p, &label) || pattern_matches(p, &plain))
        || (e.protocol == Protocol::Udp
            && config.ports.ignore_ephemeral_udp
            && (range[0]..=range[1]).contains(&e.port)
            && !config.ports.important_udp_ports.contains(&e.port))
}
pub fn endpoint_label(e: &Endpoint) -> String {
    format!(
        "{} {}:{}",
        if e.protocol == Protocol::Tcp {
            "tcp"
        } else {
            "udp"
        },
        if e.family == Family::Ipv6 {
            format!("[{}]", e.address)
        } else {
            e.address.clone()
        },
        e.port
    )
}
fn snapshot(config: &Config, raw: RawSnapshot, at: i64) -> Result<(PortsSnapshot, Vec<Id>)> {
    if at < 0 {
        return Err("monitor.clock");
    }
    let mut problems = Vec::new();
    let mut listeners: BTreeMap<Endpoint, Listener> = BTreeMap::new();
    let mut complete = true;
    let range = config
        .ports
        .ephemeral_range
        .or(raw.ephemeral_range)
        .unwrap_or([32768, 60999]);
    let fallback = config.ports.ephemeral_range.is_none() && raw.ephemeral_range.is_none();
    if fallback {
        problems.push(Id::new("ports.range_fallback")?);
    }
    if let Some(reason) = raw.process_reason {
        problems.push(Id::new(reason)?);
    }
    for (family, read) in [(Family::Ipv4, raw.ipv4), (Family::Ipv6, raw.ipv6)] {
        let text = match read {
            Ok(t) if t.len() <= 65536 => t,
            Ok(_) => {
                complete = false;
                problems.push(Id::new("monitor.output")?);
                continue;
            }
            Err(e) => {
                complete = false;
                problems.push(Id::new(e)?);
                continue;
            }
        };
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let fields: Vec<_> = line.split_whitespace().collect();
            let parsed = (|| {
                if fields.len() < 6 {
                    return None;
                }
                let protocol = match fields[0] {
                    "tcp" => Protocol::Tcp,
                    "udp" => Protocol::Udp,
                    _ => return None,
                };
                if (protocol == Protocol::Tcp && fields[1] != "LISTEN")
                    || (protocol == Protocol::Udp && fields[1] != "UNCONN")
                {
                    return None;
                }
                fields[2].parse::<u32>().ok()?;
                fields[3].parse::<u32>().ok()?;
                let (address, port) = fields[4].rsplit_once(':')?;
                let port = port.parse::<u16>().ok().filter(|p| *p > 0)?;
                let endpoint = Endpoint {
                    family,
                    protocol,
                    address: parse_address(address, family)?,
                    port,
                };
                let tail = fields[6..].join(" ");
                let (processes, process_reason) = processes(&tail);
                Some(Listener {
                    ignored: ignored(config, &endpoint, range),
                    endpoint,
                    processes,
                    process_reason,
                })
            })();
            let Some(row) = parsed else {
                complete = false;
                problems.push(Id::new("ports.output")?);
                continue;
            };
            if let Some(saved) = listeners.get_mut(&row.endpoint) {
                saved.processes.extend(row.processes);
                saved.processes.sort();
                saved.processes.dedup();
                if saved.processes.len() > 4 {
                    saved.processes.truncate(4);
                    saved.process_reason = Some(Id::new("ports.processes_partial")?);
                }
                if row.process_reason.is_some() {
                    saved.process_reason = row.process_reason;
                }
            } else if listeners.len() < 512 {
                listeners.insert(row.endpoint.clone(), row);
            } else {
                complete = false;
                problems.push(Id::new("ports.limit")?);
            }
        }
    }
    let listeners: Vec<_> = listeners.into_values().collect();
    let processes_complete = complete
        && raw.process_reason.is_none()
        && listeners.iter().all(|l| l.process_reason.is_none());
    if !processes_complete {
        problems.push(Id::new("ports.processes_partial")?);
    }
    problems.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    problems.dedup();
    problems.truncate(32);
    Ok((
        PortsSnapshot {
            observed_at: chrono::DateTime::from_timestamp(at, 0).ok_or("monitor.clock")?,
            ignored_count: listeners.iter().filter(|l| l.ignored).count() as u32,
            listeners,
            complete,
            processes_complete,
            ephemeral_range: range,
            range_fallback: fallback,
        },
        problems,
    ))
}
fn endpoints(s: &PortsSnapshot) -> Vec<Endpoint> {
    s.listeners
        .iter()
        .filter(|l| !l.ignored)
        .map(|l| l.endpoint.clone())
        .collect()
}
fn filter_hash(config: &Config, s: &PortsSnapshot) -> Result<String> {
    digest(&(
        &config.ports.ignore_patterns,
        config.ports.ignore_ephemeral_udp,
        config.ports.ephemeral_range.unwrap_or(s.ephemeral_range),
        &config.ports.important_udp_ports,
    ))
}
fn difference(a: &[Endpoint], b: &[Endpoint]) -> Vec<Endpoint> {
    let b: BTreeSet<_> = b.iter().collect();
    a.iter().filter(|v| !b.contains(v)).cloned().collect()
}
fn preview(config: &Config, state: Option<&State>, s: &PortsSnapshot) -> Result<BaselinePreview> {
    if !config.ports.enabled || !s.complete {
        return Err("ports.preview_unavailable");
    }
    let endpoints = endpoints(s);
    let baseline = state.and_then(|s| s.baseline.as_ref());
    let previous = baseline.map(|b| b.endpoints.as_slice()).unwrap_or(&[]);
    let hash = digest(&(
        config.server_id.as_str(),
        baseline.map(|b| b.revision).unwrap_or(0),
        baseline.map(|b| b.hash.as_str()),
        filter_hash(config, s)?,
        &endpoints,
    ))?;
    Ok(BaselinePreview {
        version: WireVersion::V1,
        server_id: config.server_id.clone(),
        observed_at: s.observed_at,
        hash,
        previous_hash: baseline.map(|b| b.hash.clone()),
        added: difference(&endpoints, previous),
        removed: difference(previous, &endpoints),
        endpoints,
    })
}
fn suspicious(config: &Config, s: &PortsSnapshot) -> Vec<SuspiciousListener> {
    let names = [
        "bash", "dash", "nc", "ncat", "netcat", "node", "perl", "php", "python", "ruby", "sh",
        "socat",
    ];
    let mut result = BTreeSet::new();
    for l in &s.listeners {
        for p in &l.processes {
            if names.contains(&p.name.as_str())
                && !config.ports.expected_process_names.contains(&p.name)
            {
                result.insert(SuspiciousListener {
                    endpoint: l.endpoint.clone(),
                    process_name: p.name.clone(),
                });
            }
        }
    }
    result.into_iter().collect()
}
fn baseline(config: &Config, s: &PortsSnapshot, revision: u32) -> Result<PortBaseline> {
    let endpoints = endpoints(s);
    let filter_hash = filter_hash(config, s)?;
    let hash = digest(&(&endpoints, &filter_hash, revision))?;
    Ok(PortBaseline {
        revision,
        hash,
        filter_hash,
        accepted_at: s.observed_at,
        endpoints,
    })
}
pub struct Monitor {
    config: Config,
    database: Database,
}
impl Monitor {
    pub fn new(config: Config, database: Database) -> Self {
        Self { config, database }
    }
    pub async fn preview(&self, raw: RawSnapshot, at: i64) -> Result<BaselinePreview> {
        let old = self
            .database
            .call(|s| s.observation_state("ports.state"))
            .await?;
        let state = old.as_deref().map(decode).transpose()?;
        let (s, _) = snapshot(&self.config, raw, at)?;
        preview(&self.config, state.as_ref(), &s)
    }
    pub async fn observe(&mut self, raw: RawSnapshot, at: i64) -> Result<()> {
        self.apply(raw, at, None).await
    }
    pub async fn accept(&mut self, raw: RawSnapshot, hash: &str, at: i64) -> Result<()> {
        self.apply(raw, at, Some(hash)).await
    }
    async fn apply(&mut self, raw: RawSnapshot, at: i64, accept: Option<&str>) -> Result<()> {
        if !self.config.ports.enabled {
            return if accept.is_some() {
                Err("ports.disabled")
            } else {
                Ok(())
            };
        }
        let previous = self
            .database
            .call(|s| s.observation_state("ports.state"))
            .await?;
        let old = previous.as_deref().map(decode).transpose()?;
        let (snapshot, mut problems) = snapshot(&self.config, raw, at)?;
        if let Some(hash) = accept
            && preview(&self.config, old.as_ref(), &snapshot)?.hash != hash
        {
            return Err("ports.preview_changed");
        }
        let previous_hash = old
            .as_ref()
            .and_then(|s| s.baseline.as_ref())
            .map(|b| b.hash.clone());
        let mut state = old.unwrap_or(State {
            version: 1,
            sequence: 0,
            snapshot: snapshot.clone(),
            baseline: None,
            last: vec![],
            suspicious: vec![],
            problems: vec![],
        });
        type Change = (&'static str, Severity, Vec<(&'static str, String)>);
        let mut changes: Vec<Change> = vec![];
        let filter = filter_hash(&self.config, &snapshot)?;
        if snapshot.complete {
            if let Some(b) = &state.baseline {
                if b.filter_hash == filter {
                    let added = difference(&endpoints(&snapshot), &state.last);
                    let removed = difference(&state.last, &endpoints(&snapshot));
                    for (reason, severity, rows) in [
                        (
                            "ports_added",
                            Severity::Critical,
                            added
                                .iter()
                                .filter(|e| !b.endpoints.contains(e))
                                .cloned()
                                .collect::<Vec<_>>(),
                        ),
                        (
                            "ports_returned",
                            Severity::Info,
                            added
                                .into_iter()
                                .filter(|e| b.endpoints.contains(e))
                                .collect(),
                        ),
                        ("ports_removed", Severity::Info, removed),
                    ] {
                        for chunk in rows.chunks(8) {
                            changes.push((
                                reason,
                                severity,
                                chunk
                                    .iter()
                                    .map(|e| ("listener_endpoint", endpoint_label(e)))
                                    .collect(),
                            ));
                        }
                    }
                    state.last = endpoints(&snapshot);
                } else if accept.is_none() {
                    problems.push(Id::new("ports.baseline_review")?);
                }
            } else if accept.is_none() {
                state.baseline = Some(baseline(&self.config, &snapshot, 1)?);
                state.last = endpoints(&snapshot);
                changes.push(("ports_baseline_initialized", Severity::Info, vec![]));
            }
        }
        let current = suspicious(&self.config, &snapshot);
        let new: Vec<_> = current
            .iter()
            .filter(|s| !state.suspicious.contains(s))
            .cloned()
            .collect();
        let mut retained = current;
        if !snapshot.processes_complete {
            for old in &state.suspicious {
                if !retained.contains(old)
                    && !self
                        .config
                        .ports
                        .expected_process_names
                        .contains(&old.process_name)
                    && (!snapshot.complete
                        || snapshot
                            .listeners
                            .iter()
                            .any(|l| l.endpoint == old.endpoint))
                {
                    retained.push(old.clone());
                }
            }
        }
        for chunk in new.chunks(8) {
            changes.push((
                "ports_suspicious",
                Severity::Warning,
                chunk
                    .iter()
                    .map(|s| {
                        (
                            "listener_endpoint",
                            format!("{} {}", endpoint_label(&s.endpoint), s.process_name),
                        )
                    })
                    .collect(),
            ));
        }
        let ended: Vec<_> = state
            .suspicious
            .iter()
            .filter(|s| !retained.contains(s))
            .collect();
        for chunk in ended.chunks(8) {
            changes.push((
                "ports_suspicious_ended",
                Severity::Info,
                chunk
                    .iter()
                    .map(|s| {
                        (
                            "listener_endpoint",
                            format!("{} {}", endpoint_label(&s.endpoint), s.process_name),
                        )
                    })
                    .collect(),
            ));
        }
        state.suspicious = retained;
        state.suspicious.sort();
        state.suspicious.dedup();
        if accept.is_some() {
            let revision = state
                .baseline
                .as_ref()
                .map(|b| b.revision)
                .unwrap_or(0)
                .checked_add(1)
                .ok_or("ports.state")?;
            state.baseline = Some(baseline(&self.config, &snapshot, revision)?);
            state.last = endpoints(&snapshot);
            let mut evidence = vec![
                (
                    "baseline_hash",
                    state.baseline.as_ref().ok_or("ports.state")?.hash.clone(),
                ),
                ("local_uid", rustix::process::geteuid().as_raw().to_string()),
            ];
            if let Some(before) = previous_hash {
                evidence.push(("baseline_previous_hash", before));
            }
            if let Some(hash) = accept {
                evidence.push(("baseline_preview_hash", hash.to_owned()));
            }
            changes.push(("ports_baseline_accepted", Severity::Info, evidence));
        }
        problems.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        problems.dedup();
        if problems != state.problems {
            changes.push((
                if problems.is_empty() {
                    "ports_coverage_restored"
                } else {
                    "ports_coverage_lost"
                },
                if problems.is_empty() {
                    Severity::Info
                } else {
                    Severity::Warning
                },
                problems
                    .iter()
                    .map(|p| ("monitor_reason", p.as_str().to_owned()))
                    .collect(),
            ));
        }
        state.snapshot = snapshot;
        state.problems = problems;
        let mut publications = vec![];
        for (reason, severity, values) in changes {
            publications.push(publication(
                &self.config,
                &mut state.sequence,
                "ports",
                reason,
                severity,
                values,
                at,
            )?);
        }
        let raw = serde_json::to_string(&state).map_err(|_| "ports.state")?;
        self.database
            .call(move |s| {
                s.publish_observation("ports.state", previous.as_deref(), &raw, &publications, at)
            })
            .await
    }
}
pub(crate) fn health(config: &Config, raw: Option<&str>, now: i64) -> Result<PortsHealth> {
    let state = raw.map(decode).transpose()?;
    let mut health = PortsHealth {
        version: WireVersion::V1,
        server_id: config.server_id.clone(),
        enabled: config.ports.enabled,
        stale: true,
        snapshot: None,
        baseline: None,
        added: vec![],
        removed: vec![],
        suspicious: vec![],
        problems: vec![],
    };
    if let Some(state) = state {
        health.stale = stale(
            Some(state.snapshot.observed_at.timestamp()),
            now,
            config.ports.interval_seconds,
        );
        if let Some(b) = &state.baseline {
            if b.filter_hash == filter_hash(config, &state.snapshot)? {
                health.added = difference(&state.last, &b.endpoints);
                health.removed = difference(&b.endpoints, &state.last);
            } else {
                health.problems.push(Id::new("ports.baseline_review")?);
            }
        }
        health.problems.extend(state.problems);
        health.baseline = state.baseline;
        health.snapshot = Some(state.snapshot);
        health.suspicious = state.suspicious;
    }
    Ok(health)
}
pub fn healthy(h: &PortsHealth) -> bool {
    h.enabled
        && !h.stale
        && h.baseline.is_some()
        && h.problems.is_empty()
        && h.added.is_empty()
        && h.removed.is_empty()
        && h.suspicious.is_empty()
}

pub fn render(health: &PortsHealth, page: usize, locale: &str) -> String {
    use garde_entrypoint::message;
    let mut header = crate::observation::header(
        "ports_title",
        health.server_id.as_str(),
        health.snapshot.as_ref().map(|s| s.observed_at),
        health.enabled,
        health.stale,
        locale,
    );
    header.push_str(&format!(
        "\n{}: {}\n{}: +{} / -{}",
        message(locale, "baseline_hash"),
        crate::isolate(
            health
                .baseline
                .as_ref()
                .map(|b| b.hash.as_str())
                .unwrap_or(message(locale, "monitor_waiting"))
        ),
        message(locale, "ports_drift"),
        health.added.len(),
        health.removed.len()
    ));
    if let Some(snapshot) = &health.snapshot {
        header.push_str(&format!(
            "\n{}: {} ({})",
            message(locale, "ports_ignored"),
            snapshot.ignored_count,
            if snapshot.range_fallback {
                message(locale, "ports_fallback")
            } else {
                message(locale, "ports_filter")
            }
        ));
        if !snapshot.processes_complete {
            header.push_str(&format!("\n{}", message(locale, "ports_process_partial")));
        }
    }
    for reason in health.problems.iter().take(4) {
        header.push_str(&format!(
            "\n{}: {}",
            message(locale, "monitor_reason"),
            crate::isolate(reason.as_str())
        ));
    }
    let mut rows: Vec<_> = health
        .snapshot
        .as_ref()
        .map(|s| {
            s.listeners
                .iter()
                .map(|l| {
                    format!(
                        "{}{}: {}",
                        endpoint_label(&l.endpoint),
                        if l.ignored {
                            format!(" ({})", message(locale, "ports_ignored"))
                        } else {
                            String::new()
                        },
                        if l.processes.is_empty() {
                            message(locale, "metric_unavailable").into()
                        } else {
                            l.processes
                                .iter()
                                .map(|p| format!("{} (PID {})", p.name, p.pid))
                                .collect::<Vec<_>>()
                                .join(", ")
                        }
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    rows.extend(health.removed.iter().map(|e| {
        format!(
            "{}: {}",
            message(locale, "ports_removed"),
            endpoint_label(e)
        )
    }));
    crate::observation::page_rows(header, &rows, page, locale)
}

pub struct LinuxSource {
    helper_socket: std::path::PathBuf,
    ss: crate::observation::Probe,
    ephemeral_path: std::path::PathBuf,
}
impl Default for LinuxSource {
    fn default() -> Self {
        Self {
            helper_socket: "/run/want-garde-listeners.sock".into(),
            ss: crate::observation::Probe::new("/usr/bin/ss".into()),
            ephemeral_path: "/proc/sys/net/ipv4/ip_local_port_range".into(),
        }
    }
}
impl LinuxSource {
    pub fn with_sources(
        helper_socket: std::path::PathBuf,
        ss: std::path::PathBuf,
        ephemeral_path: std::path::PathBuf,
    ) -> Self {
        Self {
            helper_socket,
            ss: crate::observation::Probe::new(ss),
            ephemeral_path,
        }
    }
    pub async fn read(&mut self) -> RawSnapshot {
        let read = async {
            let bytes = crate::observation::read_helper(
                &self.helper_socket,
                b"{\"version\":\"1\",\"action\":{\"type\":\"ports.status\"}}\n",
                524288,
            )
            .await?;
            match serde_json::from_slice::<garde_contracts::ListenerReadResponse>(&bytes)
                .map_err(|_| "helper.output")?
            {
                garde_contracts::ListenerReadResponse::Snapshot { ipv4, ipv6 }
                    if ipv4.len() <= 65536 && ipv6.len() <= 65536 =>
                {
                    Ok((ipv4, ipv6))
                }
                garde_contracts::ListenerReadResponse::Unavailable { reason_key } => {
                    Err(match reason_key.as_str() {
                        "helper.timeout" => "helper.timeout",
                        "helper.request" => "helper.request",
                        "ports.unavailable" => "ports.unavailable",
                        _ => "helper.output",
                    })
                }
                _ => Err("helper.output"),
            }
        }
        .await;
        let (ipv4, ipv6, process_reason) = match read {
            Ok((v4, v6)) => (Ok(v4), Ok(v6), None),
            Err(code) => {
                let v4 = self
                    .ss
                    .read(&["-H", "-O", "-l", "-n", "-t", "-u", "-p", "-4"])
                    .await;
                let v6 = self
                    .ss
                    .read(&["-H", "-O", "-l", "-n", "-t", "-u", "-p", "-6"])
                    .await;
                (v4, v6, Some(code))
            }
        };
        let range = std::fs::File::open(&self.ephemeral_path)
            .ok()
            .and_then(|file| {
                use std::io::Read;
                let mut text = String::new();
                file.take(65).read_to_string(&mut text).ok()?;
                (text.len() <= 64).then_some(text)
            })
            .and_then(|s| {
                let n: Vec<_> = s.split_whitespace().collect();
                if n.len() != 2 {
                    return None;
                }
                let l = n[0].parse::<u16>().ok()?;
                let h = n[1].parse::<u16>().ok()?;
                (l > 0 && l <= h).then_some([l, h])
            });
        RawSnapshot {
            ipv4,
            ipv6,
            process_reason,
            ephemeral_range: range,
        }
    }
}
impl Monitor {
    pub async fn run(&mut self) -> Result<()> {
        let mut source = LinuxSource::default();
        loop {
            if self.config.ports.enabled {
                let raw = source.read().await;
                let at = crate::runtime::now();
                while let Err(code) = self.observe(raw.clone(), at).await {
                    eprintln!("{code}");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(
                self.config.ports.interval_seconds,
            ))
            .await;
        }
    }
}
pub fn baseline_preview(
    config: &Config,
    saved: Option<&str>,
    raw: RawSnapshot,
    at: i64,
) -> Result<BaselinePreview> {
    let state = saved.map(decode).transpose()?;
    let (s, _) = snapshot(config, raw, at)?;
    preview(config, state.as_ref(), &s)
}
