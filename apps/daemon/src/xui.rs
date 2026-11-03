use crate::{
    observation::{publication, stale},
    runtime::Database,
};
use garde_config::{Config, Result};
use garde_contracts::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    version: u8,
    sequence: u64,
    checked_at: Option<i64>,
    logs_at: Option<i64>,
    source_id: Option<String>,
    schema: Option<Id>,
    panel: Option<Vec<XuiPanelRow>>,
    clients_initialized: bool,
    clients: Vec<XuiClientStatus>,
    active: BTreeSet<String>,
    sources: BTreeMap<String, XuiSourceHealth>,
    checkpoint: XuiCheckpoint,
    gaps: u32,
}
impl Default for State {
    fn default() -> Self {
        Self {
            version: 1,
            sequence: 0,
            checked_at: None,
            logs_at: None,
            source_id: None,
            schema: None,
            panel: None,
            clients_initialized: false,
            clients: vec![],
            active: BTreeSet::new(),
            sources: BTreeMap::new(),
            checkpoint: Default::default(),
            gaps: 0,
        }
    }
}
fn hex(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn clients_valid(rows: &[XuiClient]) -> bool {
    let mut keys = BTreeSet::new();
    rows.len() <= 1024
        && rows.iter().all(|v| {
            hex(&v.key)
                && keys.insert(&v.key)
                && v.inbound_id >= 0
                && !v.label.is_empty()
                && v.label.len() <= 80
                && !v.label.chars().any(char::is_control)
                && v.last_online.is_none_or(|at| at > 0)
        })
}
fn panel_valid(rows: &[XuiPanelRow]) -> bool {
    let mut keys = BTreeSet::new();
    rows.len() <= 5120
        && rows.iter().all(|r| {
            keys.insert(&r.key)
                && !r.key.is_empty()
                && r.key.len() <= 128
                && r.key
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b":_".contains(&b))
                && r.fields.len() <= 128
                && r.fields.iter().all(|(n, v)| {
                    !n.is_empty()
                        && n.len() <= 64
                        && n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                        && hex(v)
                })
        })
}
fn source_valid(h: &XuiSourceHealth) -> bool {
    [
        "xui.database",
        "xui.panel",
        "xui.clients",
        "xui.connectivity",
        "xui.journal",
        "xui.file-0",
        "xui.file-1",
        "xui.file-2",
        "xui.file-3",
    ]
    .contains(&h.source.as_str())
        && h.reason_key
            .as_ref()
            .is_none_or(|s| s.as_str().starts_with("xui.") && s.as_str().len() <= 96)
}
fn decode(raw: &str) -> Result<State> {
    if raw.len() > 1048576 {
        return Err("xui.state");
    }
    let s: State = serde_json::from_str(raw).map_err(|_| "xui.state")?;
    if s.version != 1
        || s.checked_at.is_some_and(|v| v < 0)
        || s.logs_at.is_some_and(|v| v < 0)
        || s.source_id.as_ref().is_some_and(|v| !hex(v))
        || s.panel.as_ref().is_some_and(|p| !panel_valid(p))
        || !clients_valid(
            &s.clients
                .iter()
                .map(|r| r.client.clone())
                .collect::<Vec<_>>(),
        )
        || s.active.len() > 1024
        || s.active.iter().any(|k| !hex(k))
        || s.sources.len() > 9
        || s.sources
            .iter()
            .any(|(k, h)| k != h.source.as_str() || !source_valid(h))
    {
        return Err("xui.state");
    }
    validate_xui_checkpoint(&s.checkpoint)?;
    Ok(s)
}
pub(crate) fn validate_state(raw: &str) -> Result<()> {
    decode(raw).map(|_| ())
}
type Publications = Vec<(Event, Vec<crate::Reply>)>;
pub struct Monitor {
    config: Config,
    database: Database,
}
impl Monitor {
    pub fn new(config: Config, database: Database) -> Self {
        Self { config, database }
    }
    async fn load(&self) -> Result<(Option<String>, State)> {
        let old = self
            .database
            .call(|s| s.observation_state("xui.state"))
            .await?;
        let state = old.as_deref().map(decode).transpose()?.unwrap_or_default();
        Ok((old, state))
    }
    async fn commit(
        &self,
        old: Option<String>,
        state: State,
        pubs: Publications,
        at: i64,
    ) -> Result<()> {
        let raw = serde_json::to_string(&state).map_err(|_| "xui.state")?;
        self.database
            .call(move |s| s.publish_observation("xui.state", old.as_deref(), &raw, &pubs, at))
            .await
    }
    fn event(
        &self,
        s: &mut State,
        pubs: &mut Publications,
        reason: &str,
        severity: Severity,
        values: Vec<(&str, String)>,
        at: i64,
    ) -> Result<()> {
        pubs.push(publication(
            &self.config,
            &mut s.sequence,
            "xui",
            reason,
            severity,
            values,
            at,
        )?);
        Ok(())
    }
    fn source(
        &self,
        s: &mut State,
        pubs: &mut Publications,
        name: &str,
        reason: Option<&str>,
        gap: bool,
        at: i64,
    ) -> Result<()> {
        let old = s.sources.get(name);
        if old.and_then(|v| v.reason_key.as_ref()).map(Id::as_str) != reason {
            if let Some(reason) = reason {
                self.event(
                    s,
                    pubs,
                    "xui_coverage_lost",
                    Severity::Warning,
                    vec![("xui_source", name.into()), ("xui_reason", reason.into())],
                    at,
                )?
            } else if old.is_some() {
                self.event(
                    s,
                    pubs,
                    "xui_coverage_restored",
                    Severity::Info,
                    vec![("xui_source", name.into())],
                    at,
                )?
            }
        }
        if gap && !s.sources.get(name).is_some_and(|v| v.gap) {
            s.gaps = s.gaps.saturating_add(1);
            self.event(
                s,
                pubs,
                "xui_gap",
                Severity::Warning,
                vec![("xui_source", name.into())],
                at,
            )?
        }
        s.sources.insert(
            name.into(),
            XuiSourceHealth {
                source: Id::new(name)?,
                reason_key: reason.map(Id::new).transpose()?,
                gap,
            },
        );
        Ok(())
    }
    pub async fn observe(&mut self, raw: Result<XuiSnapshot>, at: i64) -> Result<()> {
        if !self.config.xui.enabled {
            return Ok(());
        }
        if at < 0 {
            return Err("xui.clock");
        }
        let (old, mut s) = self.load().await?;
        let mut pubs = vec![];
        let raw = raw.and_then(|v| {
            if !hex(&v.source_id)
                || v.observed_at.timestamp() < 0
                || v.observed_at.timestamp().abs_diff(at) > 60
                || v.schema
                    .as_ref()
                    .is_none_or(|v| !["xui.normalized", "xui.embedded"].contains(&v.as_str()))
                || matches!(&v.panel,Measurement::Available{value}if !panel_valid(value))
                || matches!(&v.clients,Measurement::Available{value}if !clients_valid(value))
            {
                Err("xui.output")
            } else {
                Ok(v)
            }
        });
        match raw {
            Err(code) => self.source(&mut s, &mut pubs, "xui.database", Some(code), false, at)?,
            Ok(snapshot) => {
                let changed = s
                    .source_id
                    .as_ref()
                    .is_some_and(|v| *v != snapshot.source_id);
                self.source(&mut s, &mut pubs, "xui.database", None, changed, at)?;
                if changed {
                    s.panel = None;
                    s.clients_initialized = false;
                    s.active.clear();
                    s.clients.clear()
                }
                s.source_id = Some(snapshot.source_id);
                s.schema = snapshot.schema;
                s.checked_at = Some(at);
                match snapshot.panel {
                    Measurement::Unavailable { reason_key } => self.source(
                        &mut s,
                        &mut pubs,
                        "xui.panel",
                        Some(reason_key.as_str()),
                        false,
                        at,
                    )?,
                    Measurement::Available { value } => {
                        self.source(&mut s, &mut pubs, "xui.panel", None, false, at)?;
                        if let Some(previous) = &s.panel {
                            let before: BTreeMap<_, _> =
                                previous.iter().map(|r| (&r.key, r)).collect();
                            let after: BTreeMap<_, _> = value.iter().map(|r| (&r.key, r)).collect();
                            let mut changes = vec![];
                            for (key, row) in &after {
                                match before.get(key) {
                                    None => changes.push(format!("+ {key}")),
                                    Some(old) if old.fields != row.fields => {
                                        let fields: BTreeSet<_> = old
                                            .fields
                                            .keys()
                                            .chain(row.fields.keys())
                                            .filter(|k| old.fields.get(*k) != row.fields.get(*k))
                                            .collect();
                                        let mut text = format!("{key}:");
                                        let count = fields.len();
                                        let mut shown = 0;
                                        for field in fields {
                                            if text.len() + field.len() + 12 > 220 {
                                                break;
                                            }
                                            text.push(' ');
                                            text.push_str(field);
                                            shown += 1
                                        }
                                        if shown < count {
                                            text.push_str(&format!(" (+{})", count - shown))
                                        }
                                        changes.push(text)
                                    }
                                    _ => {}
                                }
                            }
                            for key in before.keys().filter(|k| !after.contains_key(*k)) {
                                changes.push(format!("- {key}"))
                            }
                            for chunk in changes.chunks(8) {
                                self.event(
                                    &mut s,
                                    &mut pubs,
                                    "xui_panel_changed",
                                    Severity::Warning,
                                    chunk.iter().map(|v| ("xui_change", v.clone())).collect(),
                                    at,
                                )?
                            }
                        }
                        s.panel = Some(value);
                    }
                }
                match snapshot.clients {
                    Measurement::Unavailable { reason_key } => self.source(
                        &mut s,
                        &mut pubs,
                        "xui.clients",
                        Some(reason_key.as_str()),
                        false,
                        at,
                    )?,
                    Measurement::Available { value } => {
                        self.source(&mut s, &mut pubs, "xui.clients", None, false, at)?;
                        self.source(
                            &mut s,
                            &mut pubs,
                            "xui.connectivity",
                            (!snapshot.connectivity_available)
                                .then_some("xui.last_online_unavailable"),
                            false,
                            at,
                        )?;
                        let mut connected = vec![];
                        let mut disconnected = vec![];
                        let mut statuses = vec![];
                        let keys: BTreeSet<_> = value.iter().map(|v| v.key.clone()).collect();
                        s.active.retain(|k| keys.contains(k));
                        for client in value {
                            let state = if !client.enabled {
                                XuiConnectionState::Disabled
                            } else if !snapshot.connectivity_available
                                || client.last_online.is_none()
                                || client.last_online.is_some_and(|v| v > at)
                            {
                                XuiConnectionState::Unknown
                            } else if at.saturating_sub(client.last_online.unwrap_or(0))
                                <= self.config.xui.client_disconnect_after_seconds as i64
                            {
                                XuiConnectionState::Online
                            } else {
                                XuiConnectionState::Offline
                            };
                            let was = s.active.contains(&client.key);
                            let mut values = vec![
                                ("xui_client", client.label.clone()),
                                ("xui_inbound", client.inbound_id.to_string()),
                                (
                                    "xui_last_online",
                                    client
                                        .last_online
                                        .map(|v| v.to_string())
                                        .unwrap_or_else(|| "?".into()),
                                ),
                            ];
                            match state {
                                XuiConnectionState::Online => {
                                    s.active.insert(client.key.clone());
                                    if s.clients_initialized && !was {
                                        connected.push(values)
                                    }
                                }
                                XuiConnectionState::Offline | XuiConnectionState::Disabled => {
                                    s.active.remove(&client.key);
                                    if s.clients_initialized && was {
                                        values.push((
                                            "xui_reason",
                                            if state == XuiConnectionState::Disabled {
                                                "xui_disabled"
                                            } else {
                                                "xui_offline"
                                            }
                                            .into(),
                                        ));
                                        disconnected.push(values)
                                    }
                                }
                                XuiConnectionState::Unknown => {}
                            }
                            statuses.push(XuiClientStatus { client, state });
                        }
                        for (reason, values) in [
                            ("xui_client_connected", connected),
                            ("xui_client_disconnected", disconnected),
                        ] {
                            for values in values {
                                self.event(&mut s, &mut pubs, reason, Severity::Info, values, at)?;
                            }
                        }
                        if snapshot.connectivity_available {
                            s.clients_initialized = true
                        }
                        s.clients = statuses;
                    }
                }
            }
        }
        self.commit(old, s, pubs, at).await
    }
    pub async fn observe_logs(&mut self, raw: Result<XuiLogBatch>, at: i64) -> Result<()> {
        if !self.config.xui.enabled {
            return Ok(());
        }
        if at < 0 {
            return Err("xui.clock");
        }
        let (old, mut s) = self.load().await?;
        let mut pubs = vec![];
        s.logs_at = Some(at);
        let raw = raw.and_then(|b| {
            validate_xui_checkpoint(&b.checkpoint)?;
            let names: BTreeSet<_> = b.sources.iter().map(|s| s.source.as_str()).collect();
            let ids: BTreeSet<_> = b.records.iter().map(|r| r.id.as_str()).collect();
            if !names.contains("xui.journal")
                || names.len() != b.sources.len()
                || ids.len() != b.records.len()
                || b.sources.len() > 5
                || b.sources.iter().any(|v| !source_valid(v))
                || b.records.len() > 512
                || b.records.iter().any(|r| {
                    !r.id.as_str().strip_prefix("xui-log-").is_some_and(hex)
                        || ![
                            "xui_panel_login",
                            "xui_login_failed",
                            "xui_client_activity",
                            "xui_suspicious_ip",
                        ]
                        .contains(&r.reason_key.as_str())
                        || r.observed_at.timestamp() < 0
                        || r.evidence.len() > 8
                        || r.evidence.iter().any(|e| {
                            ![
                                "xui_account",
                                "xui_ip",
                                "xui_client",
                                "xui_inbound",
                                "xui_source",
                                "xui_target",
                            ]
                            .contains(&e.name.as_str())
                                || e.value.len() > 255
                                || e.value.chars().any(char::is_control)
                        })
                })
            {
                Err("xui.output")
            } else {
                Ok(b)
            }
        });
        match raw {
            Err(code) => {
                let mut names: Vec<String> = s
                    .sources
                    .keys()
                    .filter(|k| k.starts_with("xui.file-") || k.as_str() == "xui.journal")
                    .cloned()
                    .collect();
                if names.is_empty() {
                    names.push("xui.journal".into())
                }
                for name in names {
                    self.source(&mut s, &mut pubs, &name, Some(code), false, at)?
                }
            }
            Ok(batch) => {
                let names: BTreeSet<_> = batch
                    .sources
                    .iter()
                    .map(|v| v.source.as_str().to_owned())
                    .collect();
                s.sources.retain(|name, _| {
                    !(name.starts_with("xui.file-") || name == "xui.journal")
                        || names.contains(name)
                });
                for h in batch.sources {
                    self.source(
                        &mut s,
                        &mut pubs,
                        h.source.as_str(),
                        h.reason_key.as_ref().map(Id::as_str),
                        h.gap,
                        at,
                    )?
                }
                for r in batch.records {
                    let severity = if ["xui_login_failed", "xui_suspicious_ip"]
                        .contains(&r.reason_key.as_str())
                    {
                        Severity::Warning
                    } else {
                        Severity::Info
                    };
                    let event = Event {
                        version: WireVersion::V1,
                        id: r.id,
                        server_id: self.config.server_id.clone(),
                        observed_at: r.observed_at,
                        source: Id::new("xui")?,
                        classification: Classification {
                            category: Id::new("xui.observation")?,
                            severity,
                            rule_id: r.reason_key.clone(),
                            reason_key: r.reason_key,
                        },
                        evidence: r.evidence,
                    };
                    let replies = crate::Bot::new(self.config.clone())?.event_replies(&event)?;
                    pubs.push((event, replies));
                }
                s.checkpoint = batch.checkpoint;
            }
        }
        self.commit(old, s, pubs, at).await
    }
    async fn request(&self, request: XuiReadRequest) -> Result<XuiReadResponse> {
        let mut bytes = serde_json::to_vec(&request).map_err(|_| "xui.request")?;
        bytes.push(b'\n');
        let raw = crate::observation::read_helper(&self.config.xui.helper_socket, &bytes, 1048576)
            .await
            .map_err(|_| "xui.helper")?;
        serde_json::from_slice(&raw).map_err(|_| "xui.output")
    }
    pub async fn run(&mut self) -> Result<()> {
        if !self.config.xui.enabled {
            std::future::pending::<()>().await
        }
        let mut next = 0;
        let mut last = None;
        loop {
            let at = crate::runtime::now();
            let result = async {
                if at >= next || next.saturating_sub(at) > self.config.xui.interval_seconds as i64 {
                    let raw = match self
                        .request(XuiReadRequest::Snapshot {
                            version: WireVersion::V1,
                        })
                        .await
                    {
                        Ok(XuiReadResponse::Snapshot { snapshot, .. }) => Ok(snapshot),
                        Ok(XuiReadResponse::Unavailable { reason_key, .. }) => {
                            Err(code(reason_key.as_str()))
                        }
                        _ => Err("xui.helper"),
                    };
                    self.observe(raw, at).await?;
                    next = at.saturating_add(self.config.xui.interval_seconds as i64);
                }
                let (_, s) = self.load().await?;
                let raw = match self
                    .request(XuiReadRequest::Logs {
                        version: WireVersion::V1,
                        checkpoint: s.checkpoint,
                    })
                    .await
                {
                    Ok(XuiReadResponse::Logs { batch, .. }) => Ok(batch),
                    Ok(XuiReadResponse::Unavailable { reason_key, .. }) => {
                        Err(code(reason_key.as_str()))
                    }
                    _ => Err("xui.helper"),
                };
                self.observe_logs(raw, crate::runtime::now()).await
            }
            .await;
            match result {
                Ok(()) => last = None,
                Err(e) => {
                    if last != Some(e) {
                        eprintln!("{e}");
                        last = Some(e)
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }
}
fn code(code: &str) -> &'static str {
    match code {
        "xui.access" => "xui.access",
        "xui.schema" => "xui.schema",
        "xui.locked" => "xui.locked",
        "xui.limit" => "xui.limit",
        "xui.policy" => "xui.policy",
        "xui.timeout" => "xui.timeout",
        "xui.journal" => "xui.journal",
        "xui.output" => "xui.output",
        _ => "xui.helper",
    }
}
pub fn health(config: &Config, raw: Option<&str>, at: i64) -> Result<XuiHealth> {
    let s = raw.map(decode).transpose()?.unwrap_or_default();
    let enabled = config.xui.enabled;
    let stamp = |v: Option<i64>| v.and_then(|v| chrono::DateTime::from_timestamp(v, 0));
    Ok(XuiHealth {
        version: WireVersion::V1,
        server_id: config.server_id.clone(),
        enabled,
        checked_at: stamp(s.checked_at),
        logs_checked_at: stamp(s.logs_at),
        stale: enabled
            && (stale(s.checked_at, at, config.xui.interval_seconds) || stale(s.logs_at, at, 10)),
        schema: s.schema,
        sources: if enabled {
            s.sources.into_values().collect()
        } else {
            vec![]
        },
        gaps: s.gaps,
        clients: if enabled { s.clients } else { vec![] },
    })
}
pub fn healthy(h: &XuiHealth) -> bool {
    !h.enabled
        || (!h.stale && h.sources.iter().all(|s| s.reason_key.is_none()) && h.sources.len() >= 5)
}
pub fn render(h: &XuiHealth, page: usize, locale: &str) -> String {
    use garde_entrypoint::message;
    let mut title = crate::observation::header(
        "xui_title",
        h.server_id.as_str(),
        h.checked_at,
        h.enabled,
        h.stale,
        locale,
    );
    for source in &h.sources {
        title.push_str(&format!(
            "\n{}: {}",
            message(locale, source.source.as_str()),
            source
                .reason_key
                .as_ref()
                .map(|r| message(locale, r.as_str()))
                .unwrap_or_else(|| message(locale, "xui_healthy"))
        ))
    }
    title.push_str(&format!("\n{}: {}", message(locale, "xui_gaps"), h.gaps));
    let pages = h.clients.len().div_ceil(8).max(1);
    if page == 0 || page > pages {
        return message(locale, "monitor_usage").into();
    }
    // Isolate each value so long labels cannot truncate the remaining columns.
    for v in h.clients.iter().skip((page - 1) * 8).take(8) {
        let state = match v.state {
            XuiConnectionState::Online => "xui_online",
            XuiConnectionState::Offline => "xui_offline",
            XuiConnectionState::Disabled => "xui_disabled",
            XuiConnectionState::Unknown => "xui_unknown",
        };
        title.push_str(&format!(
            "\n{} | {}:{} | {} | {}:{}",
            crate::isolate(&v.client.label),
            message(locale, "xui_inbound"),
            crate::isolate(&v.client.inbound_id.to_string()),
            message(locale, state),
            message(locale, "xui_last_online"),
            crate::isolate(
                &v.client
                    .last_online
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "?".into())
            )
        ));
    }
    title.push_str(&format!(
        "\n{} {page}/{pages}",
        message(locale, "monitor_page")
    ));
    title
}
