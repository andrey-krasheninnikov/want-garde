use crate::{
    delivery::{Store, sql},
    runtime::Database,
};
use garde_config::{Config, Result};
use garde_contracts::{Event, EventGeoIp, GeoIpDetails, GeoIpHealth, GeoIpState, Id, WireVersion};
use garde_entrypoint::message;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{future::Future, net::IpAddr, time::Duration};

const CACHE_TTL: i64 = 86400;
const NEGATIVE_TTL: i64 = 900;
const QUEUE_TTL: i64 = 600;

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Budget {
    minute: i64,
    minute_count: u32,
    day: i64,
    day_count: u32,
    last_attempt: i64,
    cooldown: i64,
    skipped: u32,
    problem: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cached {
    details: Option<GeoIpDetails>,
    reason: String,
    fetched_at: i64,
}
fn decode<T: serde::de::DeserializeOwned>(value: &str) -> Result<T> {
    if value.len() > 4096 {
        return Err("geoip.state");
    }
    serde_json::from_str(value).map_err(|_| "geoip.state")
}
fn encode(value: &impl Serialize) -> Result<String> {
    let value = serde_json::to_string(value).map_err(|_| "geoip.state")?;
    if value.len() > 4096 {
        return Err("geoip.state");
    }
    Ok(value)
}
fn budget(connection: &Connection) -> Result<Budget> {
    let value: String = sql(connection.query_row(
        "SELECT value FROM meta WHERE key='geoip.budget'",
        [],
        |r| r.get(0),
    ))?;
    decode(&value)
}
fn save_budget(connection: &Connection, value: &Budget) -> Result<()> {
    sql(connection.execute(
        "UPDATE meta SET value=?1 WHERE key='geoip.budget'",
        [encode(value)?],
    ))?;
    Ok(())
}
pub(crate) fn initialize(connection: &Connection) -> Result<()> {
    sql(connection.execute_batch("CREATE TABLE IF NOT EXISTS geoip_context(event_id TEXT PRIMARY KEY REFERENCES events(id) ON DELETE CASCADE, ip TEXT, payload TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS geoip_context_ip ON geoip_context(ip);
        CREATE TABLE IF NOT EXISTS geoip_cache(ip TEXT PRIMARY KEY, payload TEXT NOT NULL, expires_at INTEGER NOT NULL, last_used INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS geoip_pending(ip TEXT PRIMARY KEY, created_at INTEGER NOT NULL, attempted INTEGER NOT NULL DEFAULT 0);"))?;
    sql(connection.execute(
        "INSERT OR IGNORE INTO meta VALUES('geoip.budget',?1)",
        [encode(&Budget::default())?],
    ))?;
    for query in [
        "SELECT event_id,ip,payload FROM geoip_context LIMIT 0",
        "SELECT ip,payload,expires_at,last_used FROM geoip_cache LIMIT 0",
        "SELECT ip,created_at,attempted FROM geoip_pending LIMIT 0",
    ] {
        sql(connection.prepare(query))?;
    }
    budget(connection)?;
    Ok(())
}

fn source_ip(event: &Event) -> Option<Option<IpAddr>> {
    event
        .evidence
        .iter()
        .find_map(|e| match e.name.as_str() {
            "ip" | "xui_ip" => Some(e.value.parse::<IpAddr>().ok()),
            "xui_source" => Some(e.value.parse::<std::net::SocketAddr>().ok().map(|a| a.ip())),
            _ => None,
        })
        .map(|ip| {
            ip.map(|ip| match ip {
                IpAddr::V6(v) => v.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
                _ => ip,
            })
        })
}
// Special-purpose ranges stay local, including routable special anycast addresses.
fn public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let n = u32::from(v);
            !v.is_private()
                && !v.is_loopback()
                && !v.is_link_local()
                && !v.is_documentation()
                && !v.is_multicast()
                && n >> 24 != 0
                && n >> 22 != 0x191
                && n >> 8 != 0xc00000
                && ![0xc05863, 0xc01fc4, 0xc034c1, 0xc0af30].contains(&(n >> 8))
                && n >> 17 != 0x6309
                && n >> 28 < 14
        }
        IpAddr::V6(v) => {
            let s = v.segments();
            s[0] & 0xe000 == 0x2000
                && !(s[0] == 0x2001 && (s[1] < 0x0200 || s[1] == 0x0db8))
                && s[0] != 0x2002
                && !(s[0] == 0x2620 && s[1] == 0x004f && s[2] == 0x8000)
                && !(s[0] == 0x3fff && s[1] & 0xf000 == 0)
        }
    }
}
fn stamp(at: i64) -> Result<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::from_timestamp(at, 0).ok_or("geoip.state")
}
pub(crate) fn record(
    connection: &Connection,
    event: &Event,
    config: Option<&Config>,
    now: i64,
) -> Result<Option<EventGeoIp>> {
    let Some(config) = config else {
        return Ok(None);
    };
    let Some(address) = source_ip(event) else {
        return Ok(None);
    };
    let mut context = EventGeoIp {
        version: WireVersion::V1,
        server_id: event.server_id.clone(),
        event_id: event.id.clone(),
        ip: address.map(|v| v.to_string()),
        state: GeoIpState::Missing,
        reason_key: Id::new("geoip_disabled")?,
        provider: "ipinfo_lite".into(),
        fetched_at: None,
        details: None,
    };
    if config.geoip.enabled {
        context.reason_key = Id::new(if address.is_none() {
            "geoip_invalid"
        } else {
            "geoip_private"
        })?;
        if let Some(ip) = address.filter(|ip| public(*ip)) {
            let ip = ip.to_string();
            let cached: Option<String> = sql(connection.query_row("SELECT payload FROM geoip_cache WHERE ip=?1 AND expires_at>?2 AND last_used<=?2",params![ip,now],|r|r.get(0)).optional())?;
            if let Some(cached) = cached {
                let cached: Cached = decode(&cached)?;
                context.state = if cached.details.is_some() {
                    GeoIpState::Cached
                } else {
                    GeoIpState::Missing
                };
                context.reason_key = Id::new(cached.reason)?;
                context.fetched_at = Some(stamp(cached.fetched_at)?);
                context.details = cached.details;
                sql(connection.execute(
                    "UPDATE geoip_cache SET last_used=?2 WHERE ip=?1",
                    params![ip, now],
                ))?;
            } else {
                let exists: bool = sql(connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM geoip_pending WHERE ip=?1)",
                    [&ip],
                    |r| r.get(0),
                ))?;
                let count: i64 =
                    sql(connection
                        .query_row("SELECT count(*) FROM geoip_pending", [], |r| r.get(0)))?;
                if exists || count < 128 {
                    sql(connection.execute(
                        "INSERT OR IGNORE INTO geoip_pending(ip,created_at) VALUES(?1,?2)",
                        params![ip, now],
                    ))?;
                    context.reason_key = Id::new("geoip_pending")?;
                } else {
                    context.reason_key = Id::new("geoip_limit")?;
                    let mut budget = budget(connection)?;
                    budget.skipped = budget.skipped.saturating_add(1);
                    save_budget(connection, &budget)?;
                }
            }
        }
    }
    sql(connection.execute(
        "INSERT INTO geoip_context VALUES(?1,?2,?3)",
        params![event.id.as_str(), context.ip, encode(&context)?],
    ))?;
    Ok(Some(context))
}

pub struct LookupFailure {
    pub reason: &'static str,
    pub retry_after: u64,
}
pub trait Provider {
    fn lookup(
        &self,
        ip: &str,
    ) -> impl Future<Output = std::result::Result<GeoIpDetails, LookupFailure>>;
}
fn failure(reason: &'static str) -> LookupFailure {
    LookupFailure {
        reason,
        retry_after: 0,
    }
}
fn valid_details(details: &GeoIpDetails) -> bool {
    details
        .country_code
        .as_ref()
        .is_none_or(|c| c.len() == 2 && c.bytes().all(|b| b.is_ascii_uppercase()))
        && details.asn.is_none_or(|a| a > 0)
        && details.organization.as_ref().is_none_or(|s| {
            !s.is_empty()
                && s.chars().count() <= 128
                && s.chars().all(|c| {
                    !c.is_control() && !matches!(c,'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}')
                })
        })
        && (details.country_code.is_some() || details.asn.is_some())
}

impl Store {
    pub fn geoip_event(&self, event_id: &str) -> Result<EventGeoIp> {
        Id::new(event_id)?;
        let value: Option<String> = sql(self
            .connection
            .query_row(
                "SELECT payload FROM geoip_context WHERE event_id=?1",
                [event_id],
                |r| r.get(0),
            )
            .optional())?;
        decode(&value.ok_or("geoip.not_found")?)
    }
    pub fn geoip_health(&self, now: i64) -> Result<GeoIpHealth> {
        health(
            &self.connection,
            self.geoip_config.as_ref().is_some_and(|c| c.geoip.enabled),
            now,
        )
    }
    pub fn inspect_geoip(config: &Config, now: i64) -> Result<GeoIpHealth> {
        let connection = crate::delivery::inspect_connection(
            &config.state_dir,
            config.server_id.as_str(),
            config.owner()?,
        )?;
        health(&connection, config.geoip.enabled, now)
    }
    pub fn inspect_geoip_event(config: &Config, event_id: &str) -> Result<EventGeoIp> {
        Id::new(event_id)?;
        let connection = crate::delivery::inspect_connection(
            &config.state_dir,
            config.server_id.as_str(),
            config.owner()?,
        )?;
        let value: Option<String> = sql(connection
            .query_row(
                "SELECT payload FROM geoip_context WHERE event_id=?1",
                [event_id],
                |r| r.get(0),
            )
            .optional())?;
        decode(&value.ok_or("geoip.not_found")?)
    }
    pub fn claim_geoip(&mut self, now: i64) -> Result<Option<String>> {
        if !self.geoip_config.as_ref().is_some_and(|c| c.geoip.enabled) {
            return Ok(None);
        }
        let transaction = sql(self.connection.transaction())?;
        let expired: Vec<String> = {
            let mut statement = sql(transaction
                .prepare("SELECT ip FROM geoip_pending WHERE created_at<?1 OR created_at>?2"))?;
            let rows = sql(
                statement.query_map(params![now.saturating_sub(QUEUE_TTL), now], |r| r.get(0))
            )?;
            rows.map(sql).collect::<Result<_>>()?
        };
        for ip in expired {
            resolve(&transaction, &ip, None, "geoip_expired", now)?;
        }
        let mut b = budget(&transaction)?;
        if now <= b.last_attempt || now < b.cooldown {
            sql(transaction.commit())?;
            return Ok(None);
        }
        if now / 60 > b.minute {
            b.minute = now / 60;
            b.minute_count = 0;
        }
        if now / 86400 > b.day {
            b.day = now / 86400;
            b.day_count = 0;
        }
        if b.minute_count >= 30 || b.day_count >= 1000 {
            sql(transaction.commit())?;
            return Ok(None);
        }
        let ip: Option<String> = sql(transaction
            .query_row(
                "SELECT ip FROM geoip_pending WHERE attempted=0 ORDER BY created_at,ip LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional())?;
        if let Some(ip) = &ip {
            if !ip.parse::<IpAddr>().is_ok_and(public) {
                return Err("geoip.state");
            }
            b.last_attempt = now;
            b.minute_count += 1;
            b.day_count += 1;
            save_budget(&transaction, &b)?;
            sql(transaction.execute("UPDATE geoip_pending SET attempted=1 WHERE ip=?1", [ip]))?;
        }
        sql(transaction.commit())?;
        Ok(ip)
    }
    pub fn finish_geoip(
        &mut self,
        ip: &str,
        result: std::result::Result<GeoIpDetails, LookupFailure>,
        now: i64,
    ) -> Result<()> {
        let transaction = sql(self.connection.transaction())?;
        let (details, reason, retry) = match result {
            Ok(details) if valid_details(&details) => (Some(details), "geoip_retrieved", 0),
            Ok(_) => (None, "geoip_response", 900),
            Err(error) => (None, error.reason, error.retry_after.clamp(900, 86400)),
        };
        let mut b = budget(&transaction)?;
        b.problem = details.is_none().then(|| reason.to_string());
        b.cooldown = if details.is_none() {
            now.saturating_add(retry as i64)
        } else {
            0
        };
        save_budget(&transaction, &b)?;
        resolve(&transaction, ip, details, reason, now)?;
        sql(transaction.commit())
    }
    pub(crate) fn recover_geoip(&mut self, now: i64) -> Result<()> {
        let enabled = self.geoip_config.as_ref().is_some_and(|c| c.geoip.enabled);
        let pending: Vec<String> = {
            let mut statement = sql(self
                .connection
                .prepare("SELECT ip FROM geoip_pending WHERE attempted=1 OR ?1=0"))?;
            let rows = sql(statement.query_map([enabled], |r| r.get(0)))?;
            rows.map(sql).collect::<Result<_>>()?
        };
        for ip in pending {
            self.finish_geoip(
                &ip,
                Err(failure(if enabled {
                    "geoip_interrupted"
                } else {
                    "geoip_disabled"
                })),
                now,
            )?;
        }
        Ok(())
    }
}
fn resolve(
    connection: &Connection,
    ip: &str,
    details: Option<GeoIpDetails>,
    reason: &str,
    now: i64,
) -> Result<()> {
    let contexts: Vec<(String, String)> = {
        let mut statement =
            sql(connection.prepare("SELECT event_id,payload FROM geoip_context WHERE ip=?1"))?;
        let rows = sql(statement.query_map([ip], |r| Ok((r.get(0)?, r.get(1)?))))?;
        rows.map(sql).collect::<Result<_>>()?
    };
    for (event_id, payload) in contexts {
        let mut context: EventGeoIp = decode(&payload)?;
        if context.reason_key.as_str() != "geoip_pending" {
            continue;
        }
        context.state = if details.is_some() {
            GeoIpState::Retrieved
        } else {
            GeoIpState::Missing
        };
        context.reason_key = Id::new(reason)?;
        context.details = details.clone();
        context.fetched_at = Some(stamp(now)?);
        sql(connection.execute(
            "UPDATE geoip_context SET payload=?2 WHERE event_id=?1",
            params![event_id, encode(&context)?],
        ))?;
    }
    sql(connection.execute("DELETE FROM geoip_cache WHERE expires_at<=?1", [now]))?;
    let cached = Cached {
        details: details.clone(),
        reason: reason.into(),
        fetched_at: now,
    };
    sql(connection.execute("INSERT INTO geoip_cache VALUES(?1,?2,?3,?4) ON CONFLICT(ip) DO UPDATE SET payload=excluded.payload,expires_at=excluded.expires_at,last_used=excluded.last_used",params![ip,encode(&cached)?,now.saturating_add(if details.is_some(){CACHE_TTL}else{NEGATIVE_TTL}),now]))?;
    sql(connection.execute("DELETE FROM geoip_cache WHERE ip IN (SELECT ip FROM geoip_cache ORDER BY last_used DESC,ip LIMIT -1 OFFSET 1024)",[]))?;
    sql(connection.execute("DELETE FROM geoip_pending WHERE ip=?1", [ip]))?;
    Ok(())
}
pub async fn run_once(database: &Database, provider: &impl Provider, now: i64) -> Result<bool> {
    let Some(ip) = database.call(move |s| s.claim_geoip(now)).await? else {
        return Ok(false);
    };
    let result = tokio::time::timeout(Duration::from_secs(2), provider.lookup(&ip))
        .await
        .unwrap_or_else(|_| Err(failure("geoip_timeout")));
    let completed_at = now.max(crate::runtime::now());
    database
        .call(move |s| s.finish_geoip(&ip, result, completed_at))
        .await?;
    Ok(true)
}
pub(crate) async fn run(config: Config, database: Database) -> Result<()> {
    if !config.geoip.enabled {
        return std::future::pending().await;
    }
    let provider = IpInfo::new(config.geoip.token_file.clone())?;
    let mut last_error = None;
    loop {
        match run_once(&database, &provider, crate::runtime::now()).await {
            Err(code) => {
                if last_error != Some(code) {
                    eprintln!("{code}");
                    last_error = Some(code);
                }
            }
            Ok(_) => last_error = None,
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}
fn retry_after(value: &str, now: i64) -> Option<u64> {
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(seconds);
    }
    let at = chrono::DateTime::parse_from_rfc2822(value)
        .ok()
        .map(|at| at.timestamp())
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(value, "%A, %d-%b-%y %H:%M:%S GMT")
                .ok()
                .map(|at| at.and_utc().timestamp())
        })
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(value, "%a %b %e %H:%M:%S %Y")
                .ok()
                .map(|at| at.and_utc().timestamp())
        })?;
    Some(at.saturating_sub(now).max(0) as u64)
}

pub struct IpInfo {
    client: reqwest::Client,
    token_file: Option<std::path::PathBuf>,
    endpoint: String,
}
impl IpInfo {
    pub fn new(token_file: Option<std::path::PathBuf>) -> Result<Self> {
        let client = reqwest::Client::builder()
            .https_only(true)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(1))
            .timeout(Duration::from_secs(2))
            .build()
            .map_err(|_| "geoip.client")?;
        Ok(Self {
            client,
            token_file,
            endpoint: "https://api.ipinfo.io/lite/".into(),
        })
    }
}
impl Provider for IpInfo {
    async fn lookup(&self, ip: &str) -> std::result::Result<GeoIpDetails, LookupFailure> {
        if !ip.parse::<IpAddr>().is_ok_and(public) {
            return Err(failure("geoip_private"));
        }
        let path = self
            .token_file
            .as_ref()
            .ok_or_else(|| failure("geoip_token"))?;
        let bytes = garde_config::read_private(path, 256).map_err(|_| failure("geoip_token"))?;
        let token = std::str::from_utf8(&bytes)
            .map_err(|_| failure("geoip_token"))?
            .trim_end_matches(['\r', '\n']);
        if token.is_empty()
            || !token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._~+/=".contains(&b))
        {
            return Err(failure("geoip_token"));
        }
        let mut response = self
            .client
            .get(format!("{}{ip}", self.endpoint))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| {
                failure(if e.is_timeout() {
                    "geoip_timeout"
                } else {
                    "geoip_network"
                })
            })?;
        let status = response.status().as_u16();
        if status != 200 {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| retry_after(v, crate::runtime::now()))
                .unwrap_or(900);
            return Err(LookupFailure {
                reason: match status {
                    429 => "geoip_rate_limited",
                    401 | 403 => "geoip_token",
                    _ => "geoip_unavailable",
                },
                retry_after,
            });
        }
        if response.content_length().is_some_and(|n| n > 16384) {
            return Err(failure("geoip_response"));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| failure("geoip_network"))?
        {
            if bytes.len() + chunk.len() > 16384 {
                return Err(failure("geoip_response"));
            }
            bytes.extend_from_slice(&chunk);
        }
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|_| failure("geoip_response"))?;
        if value["ip"].as_str().and_then(|v| v.parse::<IpAddr>().ok()) != ip.parse::<IpAddr>().ok()
        {
            return Err(failure("geoip_response"));
        }
        let optional_string = |key: &str| -> std::result::Result<Option<String>, LookupFailure> {
            match value.get(key) {
                None | Some(serde_json::Value::Null) => Ok(None),
                Some(serde_json::Value::String(s)) => Ok(Some(s.clone())),
                _ => Err(failure("geoip_response")),
            }
        };
        let asn = optional_string("asn")?
            .map(|s| {
                s.strip_prefix("AS")
                    .and_then(|v| v.parse::<u32>().ok())
                    .filter(|v| *v > 0)
                    .ok_or_else(|| failure("geoip_response"))
            })
            .transpose()?;
        let details = GeoIpDetails {
            country_code: optional_string("country_code")?,
            asn,
            organization: optional_string("as_name")?,
        };
        if !valid_details(&details) {
            return Err(failure("geoip_response"));
        }
        Ok(details)
    }
}
pub fn render_context(context: &EventGeoIp, locale: &str) -> String {
    let state = match context.state {
        GeoIpState::Missing => context.reason_key.as_str(),
        GeoIpState::Cached => "geoip_cached",
        GeoIpState::Retrieved => "geoip_retrieved",
    };
    let mut text = format!("GeoIP: {}", message(locale, state));
    if let Some(details) = &context.details {
        for value in [
            details.country_code.clone(),
            details.asn.map(|v| format!("AS{v}")),
            details.organization.clone(),
        ]
        .into_iter()
        .flatten()
        {
            text.push_str(&format!(" | {}", crate::isolate_evidence(&value)));
        }
    }
    text
}

fn health(connection: &Connection, enabled: bool, now: i64) -> Result<GeoIpHealth> {
    let b = budget(connection)?;
    let limited = (now / 60 == b.minute && b.minute_count >= 30)
        || (now / 86400 == b.day && b.day_count >= 1000);
    Ok(GeoIpHealth {
        version: WireVersion::V1,
        server_id: Id::new(sql(connection.query_row(
            "SELECT value FROM meta WHERE key='server'",
            [],
            |r| r.get::<_, String>(0),
        ))?)?,
        enabled,
        queued: sql(connection.query_row("SELECT count(*) FROM geoip_pending", [], |r| r.get(0)))?,
        cache_entries: sql(connection.query_row(
            "SELECT count(*) FROM geoip_cache WHERE expires_at>?1",
            [now],
            |r| r.get(0),
        ))?,
        requests_today: if now / 86400 == b.day { b.day_count } else { 0 },
        skipped: b.skipped,
        reason_key: Id::new(if !enabled {
            "geoip_disabled"
        } else if b.cooldown > now {
            b.problem.as_deref().unwrap_or("geoip_rate_limited")
        } else if limited {
            "geoip_rate_limited"
        } else {
            "geoip_ready"
        })?,
        cooldown_until: if b.cooldown > now {
            Some(stamp(b.cooldown)?)
        } else {
            None
        },
    })
}
pub fn render_health(health: &GeoIpHealth, locale: &str) -> String {
    let mut lines = vec![
        message(locale, "geoip_title").to_string(),
        message(locale, health.reason_key.as_str()).to_string(),
    ];
    for (key, count) in [
        ("geoip_queue", health.queued),
        ("geoip_cache", health.cache_entries),
        ("geoip_requests", health.requests_today),
        ("geoip_skipped", health.skipped),
    ] {
        lines.push(format!(
            "{}: {}",
            message(locale, key),
            crate::isolate(&count.to_string())
        ));
    }
    if let Some(until) = health.cooldown_until {
        lines.push(format!(
            "{}: {}",
            message(locale, "geoip_cooldown"),
            crate::isolate(&until.to_rfc3339())
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod http_tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        os::unix::fs::PermissionsExt,
    };
    async fn request(
        status: &str,
        headers: &str,
        body: &str,
    ) -> std::result::Result<GeoIpDetails, LookupFailure> {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let response = format!(
            "HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n{body}",
            body.len()
        );
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut bytes = [0; 4096];
            let n = socket.read(&mut bytes).unwrap();
            let request = String::from_utf8_lossy(&bytes[..n]);
            assert!(request.starts_with("GET /lite/8.8.8.8 HTTP/1.1"));
            assert!(
                request
                    .to_lowercase()
                    .contains("authorization: bearer fixture-token")
            );
            assert!(!request.lines().next().unwrap().contains("token"));
            let _ = socket.write_all(response.as_bytes());
        });
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.path().join("token");
        garde_config::atomic_write(&path, b"fixture-token", 0o600).unwrap();
        let provider = IpInfo {
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .timeout(Duration::from_secs(2))
                .build()
                .unwrap(),
            token_file: Some(path),
            endpoint: format!("http://{address}/lite/"),
        };
        let result = provider.lookup("8.8.8.8").await;
        server.join().unwrap();
        result
    }
    #[tokio::test]
    async fn provider_adapter_uses_header_auth_and_keeps_only_normalized_context() {
        let details=request("200 OK","Content-Type: application/json\r\n",r#"{"ip":"8.8.8.8","country_code":"US","asn":"AS15169","as_name":"Example network","ignored":"untrusted extra data"}"#).await.ok().unwrap();
        assert_eq!(details.country_code.as_deref(), Some("US"));
        assert_eq!(details.asn, Some(15169));
        assert_eq!(details.organization.as_deref(), Some("Example network"));
    }
    #[tokio::test]
    async fn provider_failures_are_bounded_and_do_not_echo_provider_bodies() {
        for (status, headers, body, reason) in [
            ("401 Unauthorized", "", "private diagnostic", "geoip_token"),
            (
                "503 Unavailable",
                "",
                "private diagnostic",
                "geoip_unavailable",
            ),
            (
                "302 Found",
                "Location: https://example.invalid/\r\n",
                "",
                "geoip_unavailable",
            ),
            ("200 OK", "", "not json", "geoip_response"),
            (
                "200 OK",
                "",
                r#"{"ip":"1.1.1.1","country_code":"US"}"#,
                "geoip_response",
            ),
            (
                "200 OK",
                "",
                r#"{"ip":"8.8.8.8","country_code":"US","as_name":"bad\nfield"}"#,
                "geoip_response",
            ),
        ] {
            assert_eq!(
                request(status, headers, body).await.err().unwrap().reason,
                reason
            );
        }
        let error = request("429 Too Many Requests", "Retry-After: 3600\r\n", "")
            .await
            .err()
            .unwrap();
        assert_eq!(error.retry_after, 3600);
        assert_eq!(
            request("200 OK", "", &"x".repeat(17000))
                .await
                .err()
                .unwrap()
                .reason,
            "geoip_response"
        );
    }
    #[tokio::test]
    async fn service_unavailable_retry_after_survives_into_the_persisted_cooldown() {
        let clock = crate::runtime::now();
        let future = chrono::DateTime::from_timestamp(clock + 3600, 0).unwrap();
        let headers = vec![
            "3600".to_string(),
            future.format("%a, %d %b %Y %H:%M:%S GMT").to_string(),
            future.format("%A, %d-%b-%y %H:%M:%S GMT").to_string(),
            future.format("%a %b %e %H:%M:%S %Y").to_string(),
        ];
        for value in headers {
            let error = request("503 Unavailable", &format!("Retry-After: {value}\r\n"), "")
                .await
                .err()
                .unwrap();
            assert!(
                (3590..=3600).contains(&error.retry_after),
                "Retry-After was discarded"
            );
            let dir = tempfile::tempdir().unwrap();
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
            let config: Config=serde_json::from_value(serde_json::json!({"server_id":"server-demo","owner_user_id":"123456789","bot_token_file":dir.path().join("token"),"state_dir":dir.path(),"default_locale":"en","geoip":{"enabled":true,"token_file":dir.path().join("geo-token")}})).unwrap();
            let database = Database::open(config).await.unwrap();
            let mut event: Event =
                serde_json::from_str(include_str!("../../../fixtures/contracts/event.json"))
                    .unwrap();
            event.evidence = vec![garde_contracts::Evidence {
                name: Id::new("ip").unwrap(),
                value: "8.8.8.8".into(),
            }];
            database
                .call(move |s| s.publish(&event, &[], clock))
                .await
                .unwrap();
            assert!(
                database
                    .call(move |s| s.claim_geoip(clock))
                    .await
                    .unwrap()
                    .is_some()
            );
            let delay = error.retry_after;
            database
                .call(move |s| s.finish_geoip("8.8.8.8", Err(error), clock))
                .await
                .unwrap();
            let health = database
                .call(move |s| s.geoip_health(clock + 901))
                .await
                .unwrap();
            assert_eq!(
                health.cooldown_until.unwrap().timestamp(),
                clock + delay as i64
            );
            assert_eq!(health.reason_key.as_str(), "geoip_unavailable");
        }
    }
}
