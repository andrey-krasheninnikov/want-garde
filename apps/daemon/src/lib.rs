use garde_config::{
    Config, NotificationOptions, Privacy, Result, atomic_write, read_private, valid_locale,
};
use garde_contracts::{
    HostSnapshot, Measurement, OperationResult, PRODUCT_VERSION,
    access::{AccessCommand, AccessOutput},
};
use garde_entrypoint::{catalog, message};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fs, future::Future, path::PathBuf};

mod access;
mod access_socket;
pub mod delivery;
pub mod firewall;
pub mod geoip;
pub mod history;
pub mod host;
pub mod host_health;
pub mod ingest;
pub mod listeners;
mod notification_policy;
mod observation;
pub mod retention;
pub mod runtime;
pub mod services;
pub mod ssh;
pub mod telegram;

pub trait StatusSource {
    fn snapshot(&self) -> impl Future<Output = HostSnapshot>;
}

#[derive(Debug, Serialize)]
pub struct Reply {
    #[serde(skip)]
    pub locale: Option<String>,
    pub chat_id: String,
    pub text: String,
    #[serde(skip)]
    pub envelope: DeliveryEnvelope,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeliveryEnvelope {
    pub topic_id: Option<i32>,
    pub silent: bool,
    pub protect_content: bool,
    pub redacted: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preferences {
    locale: String,
}

pub struct Bot {
    config: Config,
    locale: String,
    preferences: PathBuf,
}

impl Bot {
    pub fn new(config: Config) -> Result<Self> {
        config.owner()?;
        config.validate_notifications()?;
        let preferences = config.state_dir.join("owner-preferences.json");
        let locale = match fs::symlink_metadata(&preferences) {
            Ok(_) => {
                let p: Preferences = serde_json::from_slice(&read_private(&preferences, 1024)?)
                    .map_err(|_| "preferences.invalid")?;
                if !valid_locale(&p.locale) {
                    return Err("preferences.invalid");
                }
                p.locale
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => config.default_locale.clone(),
            Err(_) => return Err("preferences.read"),
        };
        Ok(Self {
            config,
            locale,
            preferences,
        })
    }

    fn command<'a>(message: &'a Value, username: &str) -> Option<(&'a str, Vec<&'a str>)> {
        let text = message["text"].as_str().filter(|s| s.len() <= 4096)?;
        let mut words = text.split_whitespace();
        let command = words.next()?;
        let command = match command.split_once('@') {
            Some((cmd, target)) if target.eq_ignore_ascii_case(username) => cmd,
            Some(_) => return None,
            None => command,
        };
        Some((command, words.collect()))
    }

    pub async fn handle(
        &mut self,
        update: &Value,
        source: &impl StatusSource,
        username: &str,
    ) -> Result<Option<Reply>> {
        let input = &update["message"];
        let owner = self.config.owner()?;
        let (_, origin) = access::Origin::telegram(&input["from"], input);
        let Some((command, args)) = Self::command(input, username) else {
            return Ok(None);
        };
        if !origin.is_some_and(|origin| origin.owner(owner)) {
            return Ok(None);
        }
        let owner = owner.to_owned();
        let text = self.owner_text(command, &args, source).await?;
        Ok(Some(command_reply(owner, text)))
    }

    async fn status_text(&self, source: &impl StatusSource, locale: &str) -> String {
        let result = OperationResult::ServerStatus {
            server_id: self.config.server_id.clone(),
            product_version: PRODUCT_VERSION.into(),
            snapshot: Box::new(source.snapshot().await),
        };
        render_status(&result, locale)
    }

    async fn owner_text(
        &mut self,
        command: &str,
        args: &[&str],
        source: &impl StatusSource,
    ) -> Result<String> {
        Ok(match (command, args) {
            ("/status", []) => self.status_text(source, &self.locale).await,
            ("/language", [locale]) if valid_locale(locale) => {
                let p = Preferences {
                    locale: (*locale).to_owned(),
                };
                let bytes = serde_json::to_vec(&p).map_err(|_| "preferences.invalid")?;
                atomic_write(&self.preferences, &bytes, 0o600)?;
                self.locale = p.locale;
                message(&self.locale, "language_saved").to_owned()
            }
            ("/language", _) => {
                let mut text = format!("{}\n", message(&self.locale, "language_choose"));
                for (locale, copy) in catalog()["locales"]
                    .as_object()
                    .expect("embedded_locale_catalog")
                {
                    text.push_str(&format!(
                        "/language {locale} : {}\n",
                        copy["nativeName"]
                            .as_str()
                            .expect("embedded_locale_catalog")
                    ));
                }
                text
            }
            _ => message(&self.locale, "help_commands").to_owned(),
        })
    }

    pub async fn process_update(
        &mut self,
        database: &runtime::Database,
        update: &Value,
        source: &impl StatusSource,
        username: &str,
    ) -> Result<i64> {
        self.process_update_with_bans(database, update, source, &firewall::HelperClient, username)
            .await
    }
    pub async fn process_update_with_bans(
        &mut self,
        database: &runtime::Database,
        update: &Value,
        source: &impl StatusSource,
        bans: &impl firewall::BansSource,
        username: &str,
    ) -> Result<i64> {
        self.process_update_and_ack(database, update, source, bans, username)
            .await
            .map(|(offset, _)| offset)
    }

    pub(crate) async fn process_update_and_ack(
        &mut self,
        database: &runtime::Database,
        update: &Value,
        source: &impl StatusSource,
        bans: &impl firewall::BansSource,
        username: &str,
    ) -> Result<(i64, Option<runtime::CallbackAck>)> {
        let id = update["update_id"]
            .as_i64()
            .filter(|id| (0..i64::MAX).contains(id))
            .ok_or("telegram.response")?;
        let offset = database.call(|store| store.offset()).await?;
        if id < offset {
            return Ok((offset, None));
        }
        if let Some(callback) = update.get("callback_query") {
            return self.callback(database, id, callback, source).await;
        }
        self.process_message(database, id, &update["message"], source, bans, username)
            .await
            .map(|offset| (offset, None))
    }

    async fn process_message(
        &mut self,
        database: &runtime::Database,
        id: i64,
        input: &Value,
        source: &impl StatusSource,
        bans: &impl firewall::BansSource,
        username: &str,
    ) -> Result<i64> {
        let (actor, origin) = access::Origin::telegram(&input["from"], input);
        let command = Self::command(input, username);
        let slash = input["text"]
            .as_str()
            .and_then(|s| s.split_whitespace().next())
            .filter(|s| s.starts_with('/'));
        let (origin, (command, args)) = match (origin, command) {
            (Some(origin), Some(command)) => (origin, command),
            _ => {
                let capability = slash
                    .and_then(|s| s.split('@').next())
                    .and_then(access::capability);
                let auditable = slash.is_some();
                return database
                    .call(move |store| {
                        store.consume_with(id, runtime::now(), false, false, |transaction| {
                            if auditable {
                                access::audit(
                                    transaction,
                                    actor,
                                    capability,
                                    "invalid_origin",
                                    runtime::now(),
                                )?;
                            }
                            Ok(None)
                        })
                    })
                    .await;
            }
        };
        let owner = origin.owner(self.config.owner()?);
        let locale = self.actor_locale(&input["from"], owner).to_owned();
        if command == "/access" && owner {
            let request = AccessCommand::parse(&args);
            let has_arguments = !args.is_empty();
            return database
                .call(move |store| {
                    let now = runtime::now();
                    store.consume_with(id, now, true, false, |transaction| {
                        let text = match request {
                            Some(request) => {
                                match access::administer(transaction, actor, request, now) {
                                    Ok(output) => render_access(output, &locale),
                                    Err("access.limit") => {
                                        access::audit(
                                            transaction,
                                            actor,
                                            None,
                                            "policy_limit",
                                            now,
                                        )?;
                                        message(&locale, "access_limit").into()
                                    }
                                    Err(code) => return Err(code),
                                }
                            }
                            None => {
                                if has_arguments {
                                    access::audit(
                                        transaction,
                                        actor,
                                        None,
                                        "invalid_request",
                                        now,
                                    )?;
                                }
                                access_usage(&locale)
                            }
                        };
                        Ok(Some((command_reply(origin.chat.to_string(), text), None)))
                    })
                })
                .await;
        }

        let proof = if !owner || (command == "/status" && args.is_empty()) {
            let capability = access::capability(command);
            let auditable = slash.is_some();
            let proof = database
                .call(move |store| {
                    let transaction = delivery::sql(store.connection.transaction())?;
                    let proof = if let Some(capability) = capability {
                        access::authorize(&transaction, origin, capability, runtime::now())?
                    } else {
                        if auditable {
                            access::audit(
                                &transaction,
                                actor,
                                None,
                                "owner_required",
                                runtime::now(),
                            )?;
                        }
                        None
                    };
                    delivery::sql(transaction.commit())?;
                    Ok(proof)
                })
                .await?;
            if proof.is_none() {
                return database
                    .call(move |store| store.consume_update(id, None, runtime::now()))
                    .await;
            }
            proof
        } else {
            None
        };

        let health_reply = command == "/health" && args.is_empty();
        let text = self
            .command_text(database, (command, &args), source, bans, &locale, owner)
            .await?;
        let Some(proof) = proof else {
            let reply = command_reply(origin.chat.to_string(), text);
            return database
                .call(move |store| {
                    store.consume_update_with_reserve(id, Some(reply), runtime::now(), health_reply)
                })
                .await;
        };
        let button = command == "/status" && args.is_empty();
        self.queue_protected(database, id, proof, text, locale, button)
            .await
            .map(|(offset, _)| offset)
    }

    async fn command_text(
        &mut self,
        database: &runtime::Database,
        (command, args): (&str, &[&str]),
        source: &impl StatusSource,
        bans: &impl firewall::BansSource,
        locale: &str,
        owner: bool,
    ) -> Result<String> {
        if let Some(query) = command
            .strip_prefix('/')
            .and_then(|name| history::Query::parse(name, args))
        {
            return Ok(
                match query.and_then(|query| history::inspect(&self.config, query, runtime::now()))
                {
                    Ok(output) => output.render(locale),
                    Err("request.invalid") => message(locale, "history_usage").into(),
                    Err("history.not_found") => message(locale, "history_not_found").into(),
                    Err(code) => {
                        eprintln!("{code}");
                        message(locale, "operation_failed").into()
                    }
                },
            );
        }
        let text = match (command, args) {
            ("/status", []) => self.status_text(source, locale).await,
            ("/notifications", []) => {
                notification_policy::render_status(&self.config, locale, runtime::now())
                    .unwrap_or_else(|code| {
                        eprintln!("{code}");
                        message(locale, "operation_failed").into()
                    })
            }
            ("/geoip", []) => {
                let health = database
                    .call(|store| store.geoip_health(runtime::now()))
                    .await?;
                crate::geoip::render_health(&health, locale)
            }
            ("/geoip", [id]) if garde_contracts::Id::new(*id).is_ok() => {
                let id = id.to_string();
                match database.call(move |store| store.geoip_reference(&id)).await {
                    Ok(context) => format!(
                        "Event: {}\n{}",
                        isolate(context.event_id.as_str()),
                        crate::geoip::render_context(&context, locale)
                    ),
                    Err("geoip.not_found") => message(locale, "geoip_not_found").into(),
                    Err(code) => return Err(code),
                }
            }
            ("/geoip", _) => message(locale, "geoip_usage").into(),
            ("/services" | "/ports" | "/xui", args) => match crate::observation::page(args) {
                Err(_) => message(locale, "monitor_usage").into(),
                Ok(page) => {
                    let config = self.config.clone();
                    match command {
                        "/xui" => {
                            let health = database
                                .call(move |store| store.xui_health(&config, runtime::now()))
                                .await?;
                            crate::xui::render(&health, page, locale)
                        }
                        "/services" => {
                            let health = database
                                .call(move |store| store.services_health(&config, runtime::now()))
                                .await?;
                            crate::services::render(&health, page, locale)
                        }
                        _ => {
                            let health = database
                                .call(move |store| store.ports_health(&config, runtime::now()))
                                .await?;
                            crate::listeners::render(&health, page, locale)
                        }
                    }
                }
            },
            ("/delivery", []) => {
                let health = database.call(|store| store.health(runtime::now())).await?;
                render_delivery(&health, locale)
            }
            ("/ssh", []) => {
                let config = self.config.clone();
                let health = database
                    .call(move |store| store.ssh_health(&config, runtime::now()))
                    .await?;
                render_ssh(&health, locale)
            }
            ("/bans", args) => {
                let text = match firewall::bans_args(args) {
                    Ok((jail, page)) => match bans.snapshot().await {
                        Ok(snapshot) => firewall::render_bans(&snapshot, jail, page, locale),
                        Err(code) => {
                            format!("{}\n{}", message(locale, "bans_unavailable"), isolate(code))
                        }
                    },
                    Err(_) => message(locale, "bans_usage").into(),
                };
                let config = self.config.clone();
                let health = database
                    .call(move |store| store.firewall_health(&config, runtime::now()))
                    .await?;
                format!("{text}\n\n{}", firewall::render_health(&health, locale))
            }
            _ if owner => match self.owner_text(command, args, source).await {
                Ok(text) => text,
                Err(code) => {
                    eprintln!("{code}: {}", message(locale, "operation_failed"));
                    message(locale, "operation_failed").into()
                }
            },
            _ => message(locale, "operation_failed").into(),
        };
        Ok(text)
    }

    fn actor_locale(&self, from: &Value, owner: bool) -> &str {
        if owner {
            return &self.locale;
        }
        if let Some(code) = from["language_code"].as_str().filter(|s| s.len() <= 35) {
            let primary = code.split('-').next();
            if let Some(locale) = catalog()["locales"]
                .as_object()
                .expect("embedded_locale_catalog")
                .keys()
                .find(|locale| {
                    locale.eq_ignore_ascii_case(code)
                        || locale
                            .split('-')
                            .next()
                            .zip(primary)
                            .is_some_and(|(a, b)| a.eq_ignore_ascii_case(b))
                })
            {
                return locale;
            }
        }
        &self.config.default_locale
    }

    async fn queue_protected(
        &self,
        database: &runtime::Database,
        id: i64,
        proof: access::Proof,
        text: String,
        locale: String,
        button: bool,
    ) -> Result<(i64, bool)> {
        let destination = proof.destination();
        let mut reply = command_reply(destination.chat.to_string(), text);
        reply.locale = Some(locale);
        reply.envelope.topic_id = destination.topic;
        database
            .call(move |store| {
                store.consume_protected_update(id, reply, proof, button, runtime::now())
            })
            .await
    }

    async fn callback(
        &self,
        database: &runtime::Database,
        id: i64,
        callback: &Value,
        source: &impl StatusSource,
    ) -> Result<(i64, Option<runtime::CallbackAck>)> {
        let (actor, destination) =
            access::Origin::telegram(&callback["from"], &callback["message"]);
        let callback_id = callback["id"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 128 && s.bytes().all(|b| b.is_ascii_graphic()));
        let destination = destination.filter(|_| {
            callback_id.is_some()
                && callback["message"]["date"]
                    .as_i64()
                    .is_some_and(|date| date > 0)
        });
        let token = callback["data"]
            .as_str()
            .filter(|s| s.len() == 32)
            .unwrap_or("")
            .to_owned();
        let proof = database
            .call(move |store| {
                let transaction = delivery::sql(store.connection.transaction())?;
                let proof = access::consume_button(
                    &transaction,
                    actor,
                    destination,
                    &token,
                    id,
                    runtime::now(),
                )?;
                delivery::sql(transaction.commit())?;
                Ok(proof)
            })
            .await?;
        let is_owner = destination
            .is_some_and(|origin| self.config.owner().is_ok_and(|owner| origin.owner(owner)));
        let locale = self.actor_locale(&callback["from"], is_owner).to_owned();
        let (offset, accepted) = if let Some(proof) = proof {
            let text = self.status_text(source, &locale).await;
            self.queue_protected(database, id, proof, text, locale.clone(), true)
                .await?
        } else {
            (
                database
                    .call(move |store| store.consume_update(id, None, runtime::now()))
                    .await?,
                false,
            )
        };
        let acknowledgement = callback_id.map(|id| runtime::CallbackAck {
            id: id.to_owned(),
            text: (!accepted).then(|| message(&locale, "access_denied")),
        });
        Ok((offset, acknowledgement))
    }

    pub async fn publish_event(
        &self,
        database: &runtime::Database,
        event: garde_contracts::Event,
    ) -> Result<()> {
        let replies = self.event_replies(&event)?;
        database
            .call(move |store| store.publish(&event, &replies, runtime::now()))
            .await
    }

    pub(crate) fn event_replies(&self, event: &garde_contracts::Event) -> Result<Vec<Reply>> {
        let routing = &self.config.notifications;
        let mut replies = vec![render_event(
            event,
            self.config.owner()?,
            None,
            &routing.owner,
            &self.locale,
            Privacy::Full,
        )];
        let default_locale = &self.config.default_locale;
        replies.extend(routing.private.iter().map(|recipient| {
            render_event(
                event,
                &recipient.chat_id,
                None,
                &recipient.options,
                default_locale,
                Privacy::Redacted,
            )
        }));
        replies.extend(routing.forums.iter().map(|forum| {
            let topic = forum
                .topics
                .iter()
                .find(|route| route.event_type == event.classification.rule_id)
                .map_or(forum.default_topic_id, |route| route.topic_id)
                .thread_id();
            render_event(
                event,
                &forum.chat_id,
                topic,
                &forum.options,
                default_locale,
                Privacy::Redacted,
            )
        }));
        Ok(replies)
    }
}

fn command_reply(chat_id: String, text: String) -> Reply {
    Reply {
        chat_id,
        text,
        locale: None,
        envelope: Default::default(),
    }
}

fn access_usage(locale: &str) -> String {
    format!(
        "{}\n/access list\n/access grant USER_ID CHAT_ID CAPABILITY\n/access revoke USER_ID CHAT_ID CAPABILITY\n/access group CHAT_ID private|group",
        message(locale, "access_usage")
    )
}

fn render_access(output: AccessOutput, locale: &str) -> String {
    match output {
        AccessOutput::Updated {} => message(locale, "access_updated").into(),
        AccessOutput::List {
            grants,
            group_chats,
        } => {
            let mut text = message(locale, "access_grants").to_owned();
            for grant in grants {
                text.push_str(&format!(
                    "\n{} {} {}",
                    isolate(&grant.user.to_string()),
                    isolate(&grant.chat.to_string()),
                    isolate(grant.capability.as_str())
                ));
            }
            text.push_str("\n\n");
            text.push_str(message(locale, "access_groups"));
            for chat in group_chats {
                text.push_str(&format!("\n{}", isolate(&chat.to_string())));
            }
            text
        }
    }
}

fn render_event(
    event: &garde_contracts::Event,
    chat_id: &str,
    topic_id: Option<i32>,
    options: &NotificationOptions,
    default_locale: &str,
    default_privacy: Privacy,
) -> Reply {
    let locale = options.locale.as_deref().unwrap_or(default_locale);
    let redacted = options.privacy.unwrap_or(default_privacy) == Privacy::Redacted;
    let translate = |key| message(locale, key);
    let reason_key = event.classification.reason_key.as_str();
    let reason = translate(reason_key);
    let reason = if reason == "message.unavailable" {
        reason_key
    } else {
        reason
    };
    let mut text = format!(
        "{}: {}\n{}\n{}",
        translate("event_title"),
        isolate(event.server_id.as_str()),
        isolate(reason),
        isolate(&event.observed_at.to_rfc3339())
    );
    for evidence in event.evidence.iter().take(8) {
        let name = evidence.name.as_str();
        let masked;
        let value = if redacted {
            let Some(value) = redact_evidence(name, &evidence.value, locale) else {
                continue;
            };
            masked = value;
            &masked
        } else if name == "xui_reason" && translate(&evidence.value) != "message.unavailable" {
            translate(&evidence.value)
        } else if name == "host_metric" {
            translate(match evidence.value.as_str() {
                "disk" => "host_disk",
                "inode" => "host_inode",
                "reboot" => "host_reboot",
                value => value,
            })
        } else {
            &evidence.value
        };
        let label = translate(name);
        let label = if label == "message.unavailable" {
            name
        } else {
            label
        };
        text.push_str(&format!(
            "\n{}: {}",
            isolate(label),
            isolate_evidence(value)
        ));
    }
    Reply {
        locale: Some(locale.to_owned()),
        chat_id: chat_id.to_owned(),
        text,
        envelope: DeliveryEnvelope {
            topic_id,
            silent: options.silent,
            protect_content: options.protect_content,
            redacted,
        },
    }
}

fn redact_evidence(name: &str, value: &str, locale: &str) -> Option<String> {
    use std::net::{IpAddr, SocketAddr};
    let marker = message(locale, "event_redacted");
    let ip = match name {
        "user" => return Some(format!("[{marker}]")),
        "ip" | "remote_ip" | "xui_ip" => value.parse::<IpAddr>().ok()?,
        "xui_source" => value.parse::<SocketAddr>().ok()?.ip(),
        _ => return None,
    };
    Some(match ip {
        IpAddr::V4(v) => {
            let [a, b, c, _] = v.octets();
            format!("{a}.{b}.{c}.x")
        }
        IpAddr::V6(_) => format!("IPv6 [{marker}]"),
    })
}

fn render_ssh(health: &garde_contracts::SshHealth, locale: &str) -> String {
    use garde_contracts::{SshSourceKind, SshStatus};
    let state = match health.state {
        SshStatus::Starting => "ssh_starting",
        SshStatus::Healthy => "ssh_healthy",
        SshStatus::Degraded => "ssh_degraded",
        SshStatus::Unavailable => "ssh_unavailable",
    };
    let source = match health.source {
        SshSourceKind::Unselected => "?",
        SshSourceKind::Journald => "journald",
        SshSourceKind::File => "file",
    };
    let mut lines = vec![
        format!(
            "{}: {}",
            message(locale, "ssh_title"),
            isolate(health.server_id.as_str())
        ),
        format!("{}: {}", message(locale, "ssh_source"), isolate(source)),
        message(locale, state).into(),
        message(locale, "ssh_first_run").into(),
    ];
    for (key, at) in [
        ("ssh_coverage", health.coverage_started_at),
        ("ssh_checked", health.checked_at),
        ("ssh_last_record", health.last_record_at),
    ] {
        lines.push(format!(
            "{}: {}",
            message(locale, key),
            isolate(&at.map(|t| t.to_rfc3339()).unwrap_or_else(|| "?".into()))
        ));
    }
    lines.push(format!(
        "{}: {}",
        message(locale, "ssh_skipped"),
        isolate(&health.skipped_records.to_string())
    ));
    lines.push(format!(
        "{}: {}",
        message(locale, "ssh_gaps"),
        isolate(&health.gap_count.to_string())
    ));
    for code in [health.reason_key.as_ref(), health.last_gap.as_ref()]
        .into_iter()
        .flatten()
    {
        lines.push(isolate(code.as_str()));
    }
    lines.join("\n")
}

fn render_delivery(health: &garde_contracts::DeliveryHealth, locale: &str) -> String {
    let mut lines = vec![
        format!(
            "{}: {}",
            message(locale, "delivery_title"),
            isolate(health.server_id.as_str())
        ),
        format!(
            "{}: {}",
            message(locale, "observed_at"),
            isolate(&health.observed_at.to_rfc3339())
        ),
    ];
    for (key, value) in [
        ("delivery_pending", health.pending),
        ("delivery_in_flight", health.in_flight),
        ("delivery_delivered", health.delivered),
        ("delivery_dead_letter", health.dead_letter),
        ("delivery_rate_limited", health.rate_limited),
        ("delivery_pruned", health.pruned_records),
    ] {
        lines.push(format!(
            "{}: {}",
            message(locale, key),
            isolate(&value.to_string())
        ));
    }
    if let Some(at) = health.oldest_pending_at {
        lines.push(format!(
            "{}: {}",
            message(locale, "delivery_oldest"),
            isolate(&at.to_rfc3339())
        ));
    }
    lines.push(message(locale, "delivery_uncertainty").into());
    for problem in &health.problems {
        lines.push(isolate(&format!(
            "{} {} ({}){}",
            problem.delivery_id.as_str(),
            problem.reason_key.as_str(),
            problem.attempts,
            if problem.uncertain { " ?" } else { "" }
        )));
    }
    lines.join("\n")
}

fn presentation_chars(value: &str) -> impl Iterator<Item = char> + '_ {
    value.chars().filter(|c| {
        !c.is_control() && !matches!(*c,'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}')
    })
}
fn isolate(value: &str) -> String {
    let text: String = presentation_chars(value).take(128).collect();
    format!("\u{2068}{text}\u{2069}")
}
fn isolate_evidence(value: &str) -> String {
    // Preserve Unicode values while bounding Telegram's UTF-16 message size.
    let mut text = String::new();
    let mut units = 0;
    for c in presentation_chars(value) {
        if units + c.len_utf16() > 256 {
            break;
        }
        units += c.len_utf16();
        text.push(c);
    }
    format!("\u{2068}{text}\u{2069}")
}

fn reading<T>(value: &Measurement<T>, locale: &str, display: impl FnOnce(&T) -> String) -> String {
    match value {
        Measurement::Available { value } => isolate(&display(value)),
        Measurement::Unavailable { reason_key } => format!(
            "{} ({})",
            message(locale, "metric_unavailable"),
            reason_key.as_str()
        ),
    }
}

fn render_status(result: &OperationResult, locale: &str) -> String {
    let OperationResult::ServerStatus {
        server_id,
        product_version,
        snapshot: s,
    } = result
    else {
        return message(locale, "operation_failed").into();
    };
    let mut lines = vec![
        format!(
            "{}: {}",
            message(locale, "status_title"),
            isolate(server_id.as_str())
        ),
        format!(
            "{}: {}",
            message(locale, "version"),
            isolate(product_version)
        ),
        format!(
            "{}: {}",
            message(locale, "observed_at"),
            isolate(&s.observed_at.to_rfc3339())
        ),
    ];
    for (key, value) in [("host", &s.hostname), ("os", &s.os), ("kernel", &s.kernel)] {
        lines.push(format!(
            "{}: {}",
            message(locale, key),
            reading(value, locale, Clone::clone)
        ));
    }
    lines.push(format!(
        "{}: {}",
        message(locale, "uptime"),
        reading(&s.uptime_seconds, locale, |v| format!("{v} s"))
    ));
    lines.push(format!(
        "{}: {}",
        message(locale, "cpu"),
        reading(&s.cpu_usage_percent, locale, |v| format!("{v:.1}%"))
    ));
    lines.push(format!(
        "{} (1/5/15 min): {}",
        message(locale, "load"),
        reading(&s.load_average, locale, |v| format!(
            "{:.2} / {:.2} / {:.2}",
            v[0], v[1], v[2]
        ))
    ));
    for (key, value) in [("memory", &s.memory)]
        .into_iter()
        .chain(s.disks.is_empty().then_some(("root_disk", &s.root_disk)))
    {
        lines.push(format!(
            "{}: {}",
            message(locale, key),
            reading(value, locale, |v| format!(
                "{} / {} MiB ({})",
                v.available_bytes / 1048576,
                v.total_bytes / 1048576,
                message(locale, "available_total")
            ))
        ));
    }
    lines.push(format!(
        "{}: {}",
        message(locale, "host_cpu_count"),
        reading(&s.cpu_count, locale, |v| v.to_string())
    ));
    lines.push(format!(
        "{} (1/5/15 min): {}",
        message(locale, "host_load_per_core"),
        reading(&s.load_per_core, locale, |v| format!(
            "{:.2} / {:.2} / {:.2}",
            v[0], v[1], v[2]
        ))
    ));
    for disk in s.disks.iter().take(8) {
        lines.push(format!(
            "{} {}: {}",
            message(locale, "host_disk"),
            isolate(&disk.path),
            reading(&disk.space, locale, |v| format!(
                "{} / {} MiB; {:.1}%",
                v.available_bytes / 1048576,
                v.total_bytes / 1048576,
                100.0 * v.used_bytes as f64 / v.total_bytes.max(1) as f64
            ))
        ));
        lines.push(format!(
            "{}: {}",
            message(locale, "host_inode"),
            reading(&disk.inodes, locale, |v| format!(
                "{} / {}; {:.1}%",
                v.available,
                v.total,
                100.0 * v.total.saturating_sub(v.available) as f64 / v.total.max(1) as f64
            ))
        ));
    }
    lines.push(format!(
        "{}: {}",
        message(locale, "host_reboot"),
        reading(&s.reboot_required, locale, |v| message(
            locale,
            if *v {
                "host_required"
            } else {
                "host_not_required"
            }
        )
        .into())
    ));
    lines.join("\n")
}

pub async fn serve(config: Config) -> Result<()> {
    runtime::serve(config).await
}

pub fn run_process(work: impl Future<Output = std::process::ExitCode>) -> std::process::ExitCode {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            eprintln!("service.runtime");
            return std::process::ExitCode::from(2);
        }
    };
    let result = runtime.block_on(work);
    // Native DNS workers cannot be cancelled; they must not hold process shutdown.
    runtime.shutdown_timeout(std::time::Duration::from_secs(1));
    result
}

pub mod xui;
