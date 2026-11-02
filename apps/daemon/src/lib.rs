use garde_config::{Config, Result, atomic_write, read_private, valid_locale};
use garde_contracts::{HostSnapshot, Measurement, OperationResult, PRODUCT_VERSION};
use garde_entrypoint::{catalog, message};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fs, future::Future, path::PathBuf};

pub mod host;
pub mod telegram;

pub trait StatusSource {
    fn snapshot(&self) -> impl Future<Output = HostSnapshot>;
}

#[derive(Debug, Serialize)]
pub struct Reply {
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

    pub async fn handle(
        &mut self,
        update: &Value,
        source: &impl StatusSource,
        username: &str,
    ) -> Result<Option<Reply>> {
        let m = &update["message"];
        let owner = self.config.owner()?;
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
            return Ok(None);
        }
        let Some(text) = m["text"].as_str().filter(|s| s.len() <= 4096) else {
            return Ok(None);
        };
        let words: Vec<_> = text.split_whitespace().collect();
        let Some(command) = words.first() else {
            return Ok(None);
        };
        let command = match command.split_once('@') {
            Some((cmd, target)) if target.eq_ignore_ascii_case(username) => cmd,
            Some(_) => return Ok(None),
            None => command,
        };
        let text = match (command, &words[1..]) {
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
            chat_id: owner.to_owned(),
            text,
        }))
    }
}

fn isolate(value: &str) -> String {
    let text: String = value
        .chars()
        .filter(|c| {
            !c.is_control() && !matches!(*c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .take(128)
        .collect();
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
    } = result;
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
    for (key, value) in [("memory", &s.memory), ("root_disk", &s.root_disk)] {
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
    lines.join("\n")
}

pub async fn serve(config: Config) -> Result<()> {
    use std::time::Duration;
    let transport = telegram::Telegram::new(config.token()?).map_err(|e| e.code)?;
    let locale = config.default_locale.clone();
    let mut bot = Bot::new(config)?;
    let source = host::LinuxHost::default();
    let mut delay = 1u64;
    let username = loop {
        match transport.identity().await {
            Ok(username) => break username,
            Err(e) if e.fatal => return Err(e.code),
            Err(e) => {
                eprintln!("{}: {}", e.code, message(&locale, "operation_failed"));
                tokio::time::sleep(Duration::from_secs(delay).max(e.retry_after)).await;
                delay = (delay * 2).min(30);
            }
        }
    };
    eprintln!("service.ready: {}", message(&locale, "service_ready"));
    let mut offset = 0;
    delay = 1;
    loop {
        let updates = match transport.updates(offset).await {
            Ok(updates) => {
                delay = 1;
                updates
            }
            Err(e) if e.fatal => return Err(e.code),
            Err(e) => {
                eprintln!("{}: {}", e.code, message(&locale, "operation_failed"));
                tokio::time::sleep(Duration::from_secs(delay).max(e.retry_after)).await;
                delay = (delay * 2).min(30);
                continue;
            }
        };
        for update in updates {
            let id = update["update_id"].as_i64().ok_or("telegram.response")?;
            if id < offset {
                continue;
            }
            match bot.handle(&update, &source, &username).await {
                Ok(Some(reply)) => match transport.send(&reply).await {
                    Ok(()) => eprintln!("reply.sent"),
                    Err(e) if e.fatal => return Err(e.code),
                    Err(e) => eprintln!("{}: {}", e.code, message(&locale, "operation_failed")),
                },
                Ok(None) => {}
                Err(code) => {
                    eprintln!("{code}: {}", message(&locale, "operation_failed"));
                    let reply = Reply {
                        chat_id: bot.config.owner()?.to_owned(),
                        text: message(&bot.locale, "operation_failed").to_owned(),
                    };
                    if let Err(e) = transport.send(&reply).await {
                        if e.fatal {
                            return Err(e.code);
                        }
                        eprintln!("{}: {}", e.code, message(&locale, "operation_failed"));
                    }
                }
            }
            // shortcut: offsets and replies are not durable until the outbox implementation in #4.
            offset = id + 1;
        }
    }
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
