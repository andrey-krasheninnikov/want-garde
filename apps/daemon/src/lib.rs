use garde_config::{Config, Result, atomic_write, read_private, valid_locale};
use garde_contracts::{HostSnapshot, Measurement, OperationResult, PRODUCT_VERSION};
use garde_entrypoint::{catalog, message};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fs, future::Future, path::PathBuf};

pub mod delivery;
pub mod firewall;
pub mod geoip;
pub mod host;
pub mod host_health;
pub mod ingest;
pub mod listeners;
mod observation;
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

    fn command<'a>(&self, update: &'a Value, username: &str) -> Option<(&'a str, Vec<&'a str>)> {
        let m = &update["message"];
        let owner = self.config.owner().ok()?;
        let authorized = m["from"]["id"]
            .as_u64()
            .is_some_and(|n| n.to_string() == owner)
            && m["from"]["is_bot"].as_bool() == Some(false)
            && m["chat"]["type"].as_str() == Some("private")
            && m["chat"]["id"]
                .as_u64()
                .is_some_and(|n| n.to_string() == owner)
            && m.get("sender_chat").is_none();
        if !authorized {
            return None;
        }
        let text = m["text"].as_str().filter(|s| s.len() <= 4096)?;
        let words: Vec<_> = text.split_whitespace().collect();
        let command = *words.first()?;
        let command = match command.split_once('@') {
            Some((cmd, target)) if target.eq_ignore_ascii_case(username) => cmd,
            Some(_) => return None,
            None => command,
        };
        Some((command, words[1..].to_vec()))
    }

    pub async fn handle(
        &mut self,
        update: &Value,
        source: &impl StatusSource,
        username: &str,
    ) -> Result<Option<Reply>> {
        let Some((command, args)) = self.command(update, username) else {
            return Ok(None);
        };
        let owner = self.config.owner()?;
        let text = match (command, args.as_slice()) {
            ("/status", []) => {
                let result = OperationResult::ServerStatus {
                    server_id: self.config.server_id.clone(),
                    product_version: PRODUCT_VERSION.into(),
                    snapshot: Box::new(source.snapshot().await),
                };
                render_status(&result, &self.locale)
            }
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
        };
        Ok(Some(Reply {
            locale: None,
            chat_id: owner.to_owned(),
            text,
        }))
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
        let id = update["update_id"]
            .as_i64()
            .filter(|id| (0..i64::MAX).contains(id))
            .ok_or("telegram.response")?;
        let offset = database.call(|store| store.offset()).await?;
        if id < offset {
            return Ok(offset);
        }
        let reply = if let Some(("/geoip", args)) = self.command(update, username) {
            let text = match args.as_slice() {
                [] => {
                    let health = database.call(|s| s.geoip_health(runtime::now())).await?;
                    crate::geoip::render_health(&health, &self.locale)
                }
                [id] if garde_contracts::Id::new(*id).is_ok() => {
                    let id = id.to_string();
                    match database.call(move |s| s.geoip_event(&id)).await {
                        Ok(context) => format!(
                            "{}\n{}",
                            isolate(context.event_id.as_str()),
                            crate::geoip::render_context(&context, &self.locale)
                        ),
                        Err("geoip.not_found") => message(&self.locale, "geoip_not_found").into(),
                        Err(code) => return Err(code),
                    }
                }
                _ => message(&self.locale, "geoip_usage").into(),
            };
            Some(Reply {
                locale: None,
                chat_id: self.config.owner()?.into(),
                text,
            })
        } else if let Some((command @ ("/services" | "/ports" | "/xui"), args)) =
            self.command(update, username)
        {
            let text = match crate::observation::page(&args) {
                Err(_) => message(&self.locale, "monitor_usage").into(),
                Ok(page) => {
                    if command == "/xui" {
                        let config = self.config.clone();
                        let health = database
                            .call(move |s| s.xui_health(&config, runtime::now()))
                            .await?;
                        crate::xui::render(&health, page, &self.locale)
                    } else if command == "/services" {
                        let config = self.config.clone();
                        let health = database
                            .call(move |s| s.services_health(&config, runtime::now()))
                            .await?;
                        crate::services::render(&health, page, &self.locale)
                    } else {
                        let config = self.config.clone();
                        let health = database
                            .call(move |s| s.ports_health(&config, runtime::now()))
                            .await?;
                        crate::listeners::render(&health, page, &self.locale)
                    }
                }
            };
            Some(Reply {
                locale: None,
                chat_id: self.config.owner()?.into(),
                text,
            })
        } else if self
            .command(update, username)
            .is_some_and(|(command, args)| command == "/delivery" && args.is_empty())
        {
            let health = database.call(|store| store.health(runtime::now())).await?;
            Some(Reply {
                locale: None,
                chat_id: self.config.owner()?.into(),
                text: render_delivery(&health, &self.locale),
            })
        } else if self
            .command(update, username)
            .is_some_and(|(command, args)| command == "/ssh" && args.is_empty())
        {
            let config = self.config.clone();
            let health = database
                .call(move |store| store.ssh_health(&config, runtime::now()))
                .await?;
            Some(Reply {
                locale: None,
                chat_id: self.config.owner()?.into(),
                text: render_ssh(&health, &self.locale),
            })
        } else if let Some(("/bans", args)) = self.command(update, username) {
            let text = match firewall::bans_args(&args) {
                Ok((jail, page)) => match bans.snapshot().await {
                    Ok(snapshot) => firewall::render_bans(&snapshot, jail, page, &self.locale),
                    Err(code) => format!(
                        "{}\n{}",
                        message(&self.locale, "bans_unavailable"),
                        isolate(code)
                    ),
                },
                Err(_) => message(&self.locale, "bans_usage").into(),
            };
            let config = self.config.clone();
            let health = database
                .call(move |store| store.firewall_health(&config, runtime::now()))
                .await?;
            let text = format!(
                "{}\n\n{}",
                text,
                firewall::render_health(&health, &self.locale)
            );
            Some(Reply {
                locale: None,
                chat_id: self.config.owner()?.into(),
                text,
            })
        } else {
            match self.handle(update, source, username).await {
                Ok(reply) => reply,
                Err(code) => {
                    eprintln!("{code}: {}", message(&self.locale, "operation_failed"));
                    Some(Reply {
                        locale: None,
                        chat_id: self.config.owner()?.into(),
                        text: message(&self.locale, "operation_failed").into(),
                    })
                }
            }
        };
        database
            .call(move |store| store.consume_update(id, reply, runtime::now()))
            .await
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
        let reason = message(&self.locale, event.classification.reason_key.as_str());
        let reason = if reason == "message.unavailable" {
            event.classification.reason_key.as_str()
        } else {
            reason
        };
        let mut text = format!(
            "{}: {}\n{}\n{}",
            message(&self.locale, "event_title"),
            isolate(self.config.server_id.as_str()),
            isolate(reason),
            event.observed_at.to_rfc3339()
        );
        for evidence in event.evidence.iter().take(8) {
            text.push_str(&format!(
                "\n{}: {}",
                isolate(
                    if message(&self.locale, evidence.name.as_str()) == "message.unavailable" {
                        evidence.name.as_str()
                    } else {
                        message(&self.locale, evidence.name.as_str())
                    }
                ),
                isolate_evidence(
                    if evidence.name.as_str() == "xui_reason"
                        && message(&self.locale, &evidence.value) != "message.unavailable"
                    {
                        message(&self.locale, &evidence.value)
                    } else if evidence.name.as_str() == "host_metric" {
                        message(
                            &self.locale,
                            match evidence.value.as_str() {
                                "disk" => "host_disk",
                                "inode" => "host_inode",
                                "reboot" => "host_reboot",
                                value => value,
                            },
                        )
                    } else {
                        &evidence.value
                    }
                )
            ));
        }
        let replies = vec![Reply {
            locale: Some(self.locale.clone()),
            chat_id: self.config.owner()?.into(),
            text,
        }];
        Ok(replies)
    }
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
