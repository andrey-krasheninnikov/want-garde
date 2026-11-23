use crate::delivery::{Store, sql};
use garde_config::Result;
use garde_contracts::{
    Capability,
    access::{AccessCommand, AccessGrant, AccessOutput, ReplyMode, valid_chat},
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(crate) const PROOF_TTL: i64 = 600;
const BUTTON_TTL: i64 = 300;
const MAX_BUTTONS: i64 = 256;
const MAX_GRANTS: i64 = 48;
const MAX_GROUPS: i64 = 24;
const MAX_AUDIT: i64 = 4096;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Actor {
    user: Option<i64>,
    chat: Option<i64>,
    source: &'static str,
}

impl Actor {
    fn telegram(from: &Value, chat: &Value) -> Self {
        let id = |object: &Value| object["id"].as_i64().filter(|id| valid_chat(*id));
        Self {
            user: id(from).filter(|id| *id > 0),
            chat: id(chat),
            source: "telegram",
        }
    }

    pub(crate) fn local(uid: Option<u32>) -> Self {
        Self {
            user: uid.map(i64::from),
            chat: None,
            source: "local",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Origin {
    pub user: i64,
    pub chat: i64,
    pub topic: Option<i32>,
}

impl Origin {
    pub fn actor(self) -> Actor {
        Actor {
            user: Some(self.user),
            chat: Some(self.chat),
            source: "telegram",
        }
    }

    pub fn telegram(from: &Value, message: &Value) -> (Actor, Option<Self>) {
        let actor = Actor::telegram(from, &message["chat"]);
        let origin = (|| {
            let user = actor.user?;
            let chat = actor.chat?;
            if from["is_bot"].as_bool() != Some(false) || message.get("sender_chat").is_some() {
                return None;
            }
            let topic = match (message["chat"]["type"].as_str()?, chat) {
                ("private", id) if id == user => None,
                ("group", id) if id < 0 && message["is_topic_message"] != true => None,
                ("supergroup", id) if id < 0 => {
                    if message["is_topic_message"] == true {
                        let id = message["message_thread_id"].as_i64()?;
                        let id = i32::try_from(id).ok().filter(|id| *id > 0)?;
                        (id > 1).then_some(id)
                    } else {
                        None
                    }
                }
                _ => return None,
            };
            Some(Self { user, chat, topic })
        })();
        (actor, origin)
    }

    pub fn owner(self, owner: &str) -> bool {
        self.chat == self.user && self.user.to_string() == owner
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Proof {
    pub origin: Origin,
    pub capability: Capability,
    generation: i64,
    mode: ReplyMode,
}

impl Proof {
    pub fn destination(self) -> Origin {
        if self.mode == ReplyMode::Group {
            self.origin
        } else {
            Origin {
                user: self.origin.user,
                chat: self.origin.user,
                topic: None,
            }
        }
    }
}

#[derive(Debug)]
pub(crate) struct ProtectedReply {
    pub proof: Proof,
    pub button: Option<Button>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Button {
    pub token: String,
    pub locale: String,
}

pub(crate) fn capability(command: &str) -> Option<Capability> {
    Some(match command {
        "/status" => Capability::StatusRead,
        "/geoip" => Capability::GeoIpRead,
        "/bans" => Capability::BansRead,
        "/services" => Capability::ServicesRead,
        "/ports" => Capability::PortsRead,
        "/xui" => Capability::XuiRead,
        "/recent" | "/history" | "/stats" => Capability::EventsRead,
        _ => return None,
    })
}

pub(crate) fn migrate(transaction: &Transaction<'_>) -> Result<()> {
    sql(transaction.execute_batch("
        ALTER TABLE deliveries ADD COLUMN access_proof TEXT CHECK (access_proof IS NULL OR event_id IS NULL);
        ALTER TABLE deliveries ADD COLUMN access_button TEXT CHECK (access_button IS NULL OR access_proof IS NOT NULL);
        CREATE TABLE access_grants(
            user INTEGER NOT NULL, chat INTEGER NOT NULL, capability TEXT NOT NULL,
            generation INTEGER NOT NULL, PRIMARY KEY(user,chat,capability));
        CREATE TABLE access_group_delivery(chat INTEGER PRIMARY KEY);
        CREATE TABLE access_audit(
            id INTEGER PRIMARY KEY AUTOINCREMENT, at INTEGER NOT NULL, source TEXT NOT NULL,
            actor INTEGER, chat INTEGER, subject INTEGER, target_chat INTEGER,
            capability TEXT, reason TEXT NOT NULL);
        CREATE INDEX access_audit_age ON access_audit(at);
        CREATE TABLE access_buttons(
            token TEXT PRIMARY KEY, proof TEXT NOT NULL, expires_at INTEGER NOT NULL,
            used_epoch TEXT, used_update INTEGER);
        CREATE INDEX access_buttons_expiry ON access_buttons(expires_at);
    "))?;
    sql(transaction.pragma_update(None, "user_version", 3))
}

pub(crate) fn validate(connection: &Connection) -> Result<()> {
    for query in [
        "SELECT access_proof,access_button FROM deliveries LIMIT 0",
        "SELECT user,chat,capability,generation FROM access_grants LIMIT 0",
        "SELECT chat FROM access_group_delivery LIMIT 0",
        "SELECT id,at,source,actor,chat,subject,target_chat,capability,reason FROM access_audit LIMIT 0",
        "SELECT token,proof,expires_at,used_epoch,used_update FROM access_buttons LIMIT 0",
    ] {
        sql(connection.prepare(query))?;
    }
    Ok(())
}

pub(crate) fn prune_audit(connection: &Connection, days: u16, now: i64) -> Result<usize> {
    let cutoff = now.saturating_sub(i64::from(days) * 86400);
    sql(connection.execute("DELETE FROM access_audit WHERE at<?1", [cutoff]))
}

pub(crate) fn encode(proof: Proof) -> Result<String> {
    serde_json::to_string(&proof).map_err(|_| "storage.failed")
}

pub(crate) fn decode(proof: &str) -> Result<Proof> {
    serde_json::from_str(proof).map_err(|_| "storage.corrupt")
}

fn lookup(
    connection: &Connection,
    origin: Origin,
    capability: Capability,
) -> Result<Option<Proof>> {
    let owner: String = sql(connection.query_row(
        "SELECT value FROM meta WHERE key='owner'",
        [],
        |row| row.get(0),
    ))?;
    if origin.owner(&owner) {
        return Ok(Some(Proof {
            origin,
            capability,
            generation: 0,
            mode: ReplyMode::Private,
        }));
    }
    let grant = AccessGrant {
        user: origin.user,
        chat: origin.chat,
        capability,
    };
    if !grant.valid() {
        return Ok(None);
    }
    sql(connection
        .query_row(
            "SELECT generation,EXISTS(SELECT 1 FROM access_group_delivery WHERE chat=?2)
         FROM access_grants WHERE user=?1 AND chat=?2 AND capability=?3",
            params![origin.user, origin.chat, capability.as_str()],
            |row| {
                Ok(Proof {
                    origin,
                    capability,
                    generation: row.get(0)?,
                    mode: if row.get(1)? {
                        ReplyMode::Group
                    } else {
                        ReplyMode::Private
                    },
                })
            },
        )
        .optional())
}

pub(crate) fn audit(
    connection: &Connection,
    actor: Actor,
    capability: Option<Capability>,
    reason: &'static str,
    now: i64,
) -> Result<()> {
    record(connection, actor, None, None, capability, reason, now).map(|_| ())
}

fn record(
    connection: &Connection,
    actor: Actor,
    subject: Option<i64>,
    target_chat: Option<i64>,
    capability: Option<Capability>,
    reason: &'static str,
    now: i64,
) -> Result<i64> {
    let id = sql(connection.query_row(
        "INSERT INTO access_audit(at,source,actor,chat,subject,target_chat,capability,reason)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8) RETURNING id",
        params![
            now,
            actor.source,
            actor.user,
            actor.chat,
            subject,
            target_chat,
            capability.map(Capability::as_str),
            reason
        ],
        |row| row.get::<_, i64>(0),
    ))?;
    sql(connection.execute("DELETE FROM access_audit WHERE id<=?1", [id - MAX_AUDIT]))?;
    Ok(id)
}

pub(crate) fn authorize(
    connection: &Connection,
    origin: Origin,
    capability: Capability,
    now: i64,
) -> Result<Option<Proof>> {
    let proof = lookup(connection, origin, capability)?;
    let reason = if proof.is_some() {
        "read_allowed"
    } else {
        "read_denied"
    };
    audit(connection, origin.actor(), Some(capability), reason, now)?;
    Ok(proof)
}

pub(crate) fn recheck(
    connection: &Connection,
    proof: Proof,
    reason: &'static str,
    now: i64,
) -> Result<bool> {
    let current = lookup(connection, proof.origin, proof.capability)? == Some(proof);
    let reason = if current {
        reason
    } else {
        "permission_changed"
    };
    audit(
        connection,
        proof.origin.actor(),
        Some(proof.capability),
        reason,
        now,
    )?;
    Ok(current)
}

pub(crate) fn issue_button(connection: &Connection, proof: Proof, now: i64) -> Result<String> {
    sql(connection.execute("DELETE FROM access_buttons WHERE expires_at<=?1", [now]))?;
    sql(connection.execute(
        "DELETE FROM access_buttons WHERE token IN (
          SELECT token FROM access_buttons ORDER BY expires_at DESC,token DESC LIMIT -1 OFFSET ?1)",
        [MAX_BUTTONS - 1],
    ))?;
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|_| "access.random")?;
    let token: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    sql(connection.execute(
        "INSERT INTO access_buttons(token,proof,expires_at) VALUES(?1,?2,?3)",
        params![token, encode(proof)?, now.saturating_add(BUTTON_TTL)],
    ))?;
    Ok(token)
}

pub(crate) fn active_button(connection: &Connection, token: &str, now: i64) -> Result<bool> {
    sql(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM access_buttons WHERE token=?1 AND expires_at>?2 AND used_update IS NULL)",
        params![token, now], |row| row.get(0),
    ))
}

pub(crate) fn consume_button(
    connection: &Connection,
    actor: Actor,
    destination: Option<Origin>,
    token: &str,
    update: i64,
    now: i64,
) -> Result<Option<Proof>> {
    let saved: Option<(String, String)> = if token.len() == 32
        && token.bytes().all(|b| b.is_ascii_hexdigit())
    {
        sql(connection.query_row(
            "SELECT proof,(SELECT value FROM meta WHERE key='epoch')
             FROM access_buttons WHERE token=?1 AND expires_at>?2
               AND (used_update IS NULL OR (used_epoch=(SELECT value FROM meta WHERE key='epoch') AND used_update=?3))",
            params![token, now, update], |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional())?
    } else {
        None
    };
    if let Some((saved, epoch)) = saved {
        let proof = decode(&saved)?;
        if Some(proof.destination()) == destination && proof.capability == Capability::StatusRead {
            if !recheck(connection, proof, "button_used", now)? {
                return Ok(None);
            }
            // A polling retry may resume the same update after a failed outbox write.
            sql(connection.execute(
                "UPDATE access_buttons SET used_epoch=?2,used_update=?3 WHERE token=?1",
                params![token, epoch, update],
            ))?;
            return Ok(Some(proof));
        }
    }
    audit(
        connection,
        actor,
        Some(Capability::StatusRead),
        "button_denied",
        now,
    )?;
    Ok(None)
}

fn grant_list(connection: &Connection) -> Result<AccessOutput> {
    let mut statement = sql(connection
        .prepare("SELECT user,chat,capability FROM access_grants ORDER BY user,chat,capability"))?;
    let mut rows = sql(statement.query([]))?;
    let mut grants = Vec::new();
    while let Some(row) = sql(rows.next())? {
        let capability: String = sql(row.get(2))?;
        grants.push(AccessGrant {
            user: sql(row.get(0))?,
            chat: sql(row.get(1))?,
            capability: capability.parse().map_err(|_| "storage.corrupt")?,
        });
    }
    let mut statement =
        sql(connection.prepare("SELECT chat FROM access_group_delivery ORDER BY chat"))?;
    let group_chats = sql(sql(statement.query_map([], |row| row.get(0)))?.collect())?;
    Ok(AccessOutput::List {
        grants,
        group_chats,
    })
}

pub(crate) fn administer(
    connection: &Connection,
    actor: Actor,
    command: AccessCommand,
    now: i64,
) -> Result<AccessOutput> {
    if !command.valid() {
        return Err("request.invalid");
    }
    match command {
        AccessCommand::List {} => return grant_list(connection),
        AccessCommand::Grant { grant } | AccessCommand::Revoke { grant } => {
            let key = params![grant.user, grant.chat, grant.capability.as_str()];
            let exists: bool = sql(connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM access_grants WHERE user=?1 AND chat=?2 AND capability=?3)",
                key, |row| row.get(0),
            ))?;
            let grant_access = matches!(command, AccessCommand::Grant { .. });
            if exists != grant_access {
                if grant_access {
                    let count: i64 = sql(connection.query_row(
                        "SELECT count(*) FROM access_grants",
                        [],
                        |row| row.get(0),
                    ))?;
                    if count >= MAX_GRANTS {
                        return Err("access.limit");
                    }
                }
                let generation = record(
                    connection,
                    actor,
                    Some(grant.user),
                    Some(grant.chat),
                    Some(grant.capability),
                    if grant_access { "grant" } else { "revoke" },
                    now,
                )?;
                if grant_access {
                    sql(connection.execute(
                        "INSERT INTO access_grants VALUES(?1,?2,?3,?4)",
                        params![
                            grant.user,
                            grant.chat,
                            grant.capability.as_str(),
                            generation
                        ],
                    ))?;
                } else {
                    sql(connection.execute(
                        "DELETE FROM access_grants WHERE user=?1 AND chat=?2 AND capability=?3",
                        key,
                    ))?;
                }
            }
        }
        AccessCommand::Group { chat, mode } => {
            let group_delivery = mode == ReplyMode::Group;
            let exists: bool = sql(connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM access_group_delivery WHERE chat=?1)",
                [chat],
                |row| row.get(0),
            ))?;
            if exists != group_delivery {
                if group_delivery {
                    let count: i64 = sql(connection.query_row(
                        "SELECT count(*) FROM access_group_delivery",
                        [],
                        |row| row.get(0),
                    ))?;
                    if count >= MAX_GROUPS {
                        return Err("access.limit");
                    }
                }
                let generation = record(
                    connection,
                    actor,
                    None,
                    Some(chat),
                    None,
                    if group_delivery {
                        "group_delivery"
                    } else {
                        "private_delivery"
                    },
                    now,
                )?;
                if group_delivery {
                    sql(connection.execute("INSERT INTO access_group_delivery VALUES(?1)", [chat]))?;
                } else {
                    sql(connection
                        .execute("DELETE FROM access_group_delivery WHERE chat=?1", [chat]))?;
                }
                sql(connection.execute(
                    "UPDATE access_grants SET generation=?1 WHERE chat=?2",
                    params![generation, chat],
                ))?;
            }
        }
    }
    Ok(AccessOutput::Updated {})
}

impl Store {
    pub(crate) fn local_access(
        &mut self,
        command: AccessCommand,
        now: i64,
    ) -> Result<AccessOutput> {
        let transaction = sql(self.connection.transaction())?;
        prune_audit(&transaction, self.retention.action_audit_days, now)?;
        let output = administer(&transaction, Actor::local(Some(0)), command, now)?;
        sql(transaction.commit())?;
        Ok(output)
    }
}
