use crate::{Result, fingerprint, label};
use garde_contracts::{Id, Measurement, WireVersion, XuiClient, XuiPanelRow, XuiSnapshot};
use rusqlite::{Connection, OpenFlags, limits::Limit, types::ValueRef};
use std::{
    collections::BTreeSet,
    path::Path,
    time::{Duration, Instant},
};
const TABLES: [&str; 6] = [
    "inbounds",
    "settings",
    "users",
    "clients",
    "client_inbounds",
    "client_traffics",
];
fn sql<T>(value: rusqlite::Result<T>) -> Result<T> {
    value.map_err(|error| match error.sqlite_error_code() {
        Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
            "xui.locked"
        }
        Some(rusqlite::ErrorCode::TooBig | rusqlite::ErrorCode::OperationInterrupted) => {
            "xui.limit"
        }
        _ => "xui.schema",
    })
}
fn measurement<T>(result: Result<T>) -> Measurement<T> {
    match result {
        Ok(value) => Measurement::Available { value },
        Err(reason) => Measurement::Unavailable {
            reason_key: Id::new(reason).expect("static_identifier"),
        },
    }
}
fn columns(c: &Connection, table: &str) -> Result<BTreeSet<String>> {
    let mut statement = sql(c.prepare(&format!("PRAGMA table_info(\"{table}\")")))?;
    sql(statement.query_map([], |r| r.get::<_, String>(1)))?
        .map(sql)
        .collect()
}
fn has(c: &Connection, table: &str, required: &[&str]) -> Result<bool> {
    let names = columns(c, table)?;
    Ok(required.iter().all(|n| names.contains(*n)))
}
fn bounded<T: serde::Serialize>(value: T, limit: usize) -> Result<T> {
    if serde_json::to_vec(&value).map_err(|_| "xui.output")?.len() > limit {
        Err("xui.limit")
    } else {
        Ok(value)
    }
}
pub fn snapshot(path: &Path, at: i64) -> Result<XuiSnapshot> {
    let observed_at = chrono::DateTime::from_timestamp(at, 0).ok_or("xui.clock")?;
    let m = std::fs::symlink_metadata(path).map_err(|_| "xui.access")?;
    if !m.is_file() || m.len() > 268435456 {
        return Err("xui.access");
    }
    let canonical = std::fs::canonicalize(path).map_err(|_| "xui.access")?;
    let source_id = fingerprint(canonical.as_os_str().as_encoded_bytes());
    let mut c = Connection::open_with_flags(
        canonical,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|_| "xui.access")?;
    sql(c.set_limit(Limit::SQLITE_LIMIT_LENGTH, 1048576))?;
    sql(c.set_limit(Limit::SQLITE_LIMIT_COLUMN, 128))?;
    let deadline = Instant::now() + Duration::from_millis(1500);
    sql(c.progress_handler(1000, Some(move || Instant::now() >= deadline)))?;
    sql(c.busy_timeout(Duration::from_millis(200)))?;
    sql(c.execute_batch("PRAGMA query_only=ON; PRAGMA trusted_schema=OFF; BEGIN"))?;
    let mut tables = BTreeSet::new();
    for t in TABLES {
        let kind = sql(c
            .query_row("SELECT type FROM sqlite_schema WHERE name=?1", [t], |r| {
                r.get::<_, String>(0)
            })
            .optional())?;
        if let Some(kind) = kind {
            if kind != "table" {
                return Err("xui.schema");
            };
            tables.insert(t);
        }
    }
    if !tables.contains("inbounds")
        || !has(
            &c,
            "inbounds",
            &["id", "enable", "port", "protocol", "settings"],
        )?
    {
        return Err("xui.schema");
    }
    let normalized = tables.contains("clients") && tables.contains("client_inbounds");
    let schema = Some(Id::new(if normalized {
        "xui.normalized"
    } else {
        "xui.embedded"
    })?);
    let panel = measurement(panel(&c, &tables).and_then(|p| bounded(p, 524288)));
    let (clients, connectivity_available) = match clients(&c, &tables, at)
        .and_then(|(v, online)| bounded(v, 262144).map(|v| (v, online)))
    {
        Ok((v, online)) => (measurement(Ok(v)), online),
        Err(e) => (measurement(Err(e)), false),
    };
    Ok(XuiSnapshot {
        source_id,
        version: WireVersion::V1,
        observed_at,
        schema,
        panel,
        clients,
        connectivity_available,
    })
}
use rusqlite::OptionalExtension;
fn panel(c: &Connection, tables: &BTreeSet<&str>) -> Result<Vec<XuiPanelRow>> {
    if !tables.contains("settings")
        || !tables.contains("users")
        || !has(c, "settings", &["key", "value"])?
        || !has(c, "users", &["id", "username", "password"])?
    {
        return Err("xui.schema");
    }
    if tables.contains("clients") != tables.contains("client_inbounds") {
        return Err("xui.schema");
    }
    let mut result = vec![];
    for table in TABLES
        .into_iter()
        .filter(|t| *t != "client_traffics" && tables.contains(t))
    {
        let mut statement = sql(c.prepare(&format!(
            "SELECT rowid,* FROM \"{table}\" ORDER BY rowid LIMIT 1025"
        )))?;
        let names: Vec<_> = statement.column_names().map(|s| s.to_string()).collect();
        if names
            .iter()
            .any(|n| n.len() > 64 || !n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
        {
            return Err("xui.schema");
        }
        let mut rows = sql(statement.query([]))?;
        let mut count = 0;
        while let Some(row) = sql(rows.next())? {
            count += 1;
            if count > 1024 {
                return Err("xui.limit");
            }
            let mut key = format!("{table}:{}", sql(row.get::<_, i64>(0))?);
            if table == "client_inbounds" {
                key = format!(
                    "{table}:{}:{}",
                    sql(row.get::<_, i64>("client_id"))?,
                    sql(row.get::<_, i64>("inbound_id"))?
                );
            }
            let mut fields = std::collections::BTreeMap::new();
            for (i, name) in names.iter().enumerate().skip(1) {
                if [
                    "up",
                    "down",
                    "last_online",
                    "last_sub_fetch",
                    "created_at",
                    "updated_at",
                    "last_traffic_reset_time",
                    "login_epoch",
                    "reset_count",
                    "sync_orphaned_at",
                ]
                .contains(&name.as_str())
                {
                    continue;
                }
                let value = sql(row.get_ref(i))?;
                let bytes = match value {
                    ValueRef::Null => b"null".to_vec(),
                    ValueRef::Integer(v) => format!("i:{v}").into_bytes(),
                    ValueRef::Real(v) => format!("f:{v}").into_bytes(),
                    ValueRef::Text(v) => {
                        let canonical = serde_json::from_slice::<serde_json::Value>(v)
                            .ok()
                            .and_then(|v| serde_json::to_vec(&v).ok());
                        let mut b = b"t:".to_vec();
                        b.extend_from_slice(canonical.as_deref().unwrap_or(v));
                        b
                    }
                    ValueRef::Blob(v) => {
                        let mut b = b"b:".to_vec();
                        b.extend_from_slice(v);
                        b
                    }
                };
                fields.insert(name.clone(), fingerprint(&bytes));
            }
            result.push(XuiPanelRow { key, fields });
        }
    }
    Ok(result)
}
fn integer(row: &rusqlite::Row<'_>, name: &str) -> Result<i64> {
    match sql(row.get_ref(name))? {
        ValueRef::Integer(v) => Ok(v),
        ValueRef::Text(v) => std::str::from_utf8(v)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .ok_or("xui.output"),
        _ => Err("xui.output"),
    }
}
fn clients(c: &Connection, tables: &BTreeSet<&str>, at: i64) -> Result<(Vec<XuiClient>, bool)> {
    let mut result = vec![];
    if tables.contains("client_traffics") {
        if !has(
            c,
            "client_traffics",
            &["inbound_id", "email", "enable", "up", "down"],
        )? {
            return Err("xui.schema");
        }
        let online = has(c, "client_traffics", &["last_online"])?;
        let mut statement =
            sql(c.prepare("SELECT * FROM client_traffics ORDER BY inbound_id,email LIMIT 1025"))?;
        let mut rows = sql(statement.query([]))?;
        let mut keys = BTreeSet::new();
        while let Some(row) = sql(rows.next())? {
            if result.len() >= 1024 {
                return Err("xui.limit");
            }
            let inbound_id = integer(row, "inbound_id")?;
            if inbound_id < 0 {
                return Err("xui.output");
            }
            let name = label(&sql(row.get::<_, String>("email"))?)?;
            let key = fingerprint(format!("{inbound_id}:{}:{name}", name.len()).as_bytes());
            if !keys.insert(key.clone()) {
                return Err("xui.output");
            }
            let up = u64::try_from(integer(row, "up")?).map_err(|_| "xui.output")?;
            let down = u64::try_from(integer(row, "down")?).map_err(|_| "xui.output")?;
            let raw = if online {
                integer(row, "last_online").ok()
            } else {
                None
            };
            let last_online = raw
                .map(|v| if v > 10_000_000_000 { v / 1000 } else { v })
                .filter(|v| *v > 0 && *v <= at.saturating_add(60));
            let enabled = match integer(row, "enable")? {
                0 => false,
                1 => true,
                _ => return Err("xui.output"),
            };
            result.push(XuiClient {
                key,
                label: name,
                inbound_id,
                enabled,
                last_online,
                traffic_bytes: up.checked_add(down).ok_or("xui.output")?,
            });
        }
        return Ok((result, online));
    }
    let mut statement =
        sql(c.prepare("SELECT id,enable,settings FROM inbounds ORDER BY id LIMIT 1025"))?;
    let mut rows = sql(statement.query([]))?;
    let mut keys = BTreeSet::new();
    let mut inbounds = 0;
    while let Some(row) = sql(rows.next())? {
        inbounds += 1;
        if inbounds > 1024 {
            return Err("xui.limit");
        }
        let inbound_id = integer(row, "id")?;
        let raw = sql(row.get::<_, String>("settings"))?;
        let data: serde_json::Value = serde_json::from_str(&raw).map_err(|_| "xui.output")?;
        if let Some(items) = data.get("clients") {
            for item in items.as_array().ok_or("xui.output")? {
                if result.len() >= 1024 {
                    return Err("xui.limit");
                }
                let Some(email) = item.get("email").and_then(|v| v.as_str()) else {
                    continue;
                };
                let name = label(email)?;
                let key = fingerprint(format!("{inbound_id}:{}:{name}", name.len()).as_bytes());
                if !keys.insert(key.clone()) {
                    return Err("xui.output");
                }
                result.push(XuiClient {
                    key,
                    label: name,
                    inbound_id,
                    enabled: integer(row, "enable")? == 1
                        && item.get("enable").and_then(|v| v.as_bool()).unwrap_or(true),
                    last_online: None,
                    traffic_bytes: 0,
                });
            }
        }
    }
    Ok((result, false))
}
