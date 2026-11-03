use garde_config::Config;
use garde_entrypoint::message;
use std::{path::Path, process::ExitCode};

async fn shutdown() -> Result<(), &'static str> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut terminate = signal(SignalKind::terminate()).map_err(|_| "service.signal")?;
    let mut interrupt = signal(SignalKind::interrupt()).map_err(|_| "service.signal")?;
    tokio::select! { _=terminate.recv()=>{}, _=interrupt.recv()=>{} }
    Ok(())
}

fn main() -> ExitCode {
    garde_daemon::run_process(run())
}

async fn run() -> ExitCode {
    let Ok(args) = std::env::args_os()
        .skip(1)
        .map(|s| s.into_string())
        .collect::<Result<Vec<_>, _>>()
    else {
        eprintln!("request.invalid");
        return ExitCode::from(2);
    };
    let explicit_locale = args.first().is_some_and(|s| s == "--lang");
    let (mut locale, args) = if args.first().is_some_and(|s| s == "--lang") && args.len() >= 2 {
        (args[1].as_str(), &args[2..])
    } else {
        ("en", args.as_slice())
    };
    if args == ["--version"] {
        return garde_entrypoint::run("want-garde-daemon");
    }
    if args == ["--help"] {
        println!(
            "{}: want-garde-daemon [--lang LOCALE] [--check-config | --delivery-status | --ssh-status | --firewall-status | --host-status | --services-status | --ports-status | --xui-status | --ports-baseline-preview | --ports-baseline-accept HASH] --config PATH",
            message(locale, "usage")
        );
        return ExitCode::SUCCESS;
    }
    match args {
        [flag, config, path]
            if [
                "--services-status",
                "--ports-status",
                "--xui-status",
                "--ports-baseline-preview",
            ]
            .contains(&flag.as_str())
                && config == "--config" =>
        {
            return monitoring_command(flag, None, Path::new(path)).await;
        }
        [flag, hash, config, path] if flag == "--ports-baseline-accept" && config == "--config" => {
            return monitoring_command(flag, Some(hash), Path::new(path)).await;
        }
        _ => {}
    }
    let host_diagnostic = args.first().is_some_and(|s| s == "--host-status");
    let (check, diagnostic, ssh_diagnostic, firewall_diagnostic, path) = match args {
        [flag, path] if flag == "--config" => (false, false, false, false, path),
        [check, flag, path] if check == "--check-config" && flag == "--config" => {
            (true, false, false, false, path)
        }
        [diagnostic, flag, path] if diagnostic == "--delivery-status" && flag == "--config" => {
            (false, true, false, false, path)
        }
        [diagnostic, flag, path] if diagnostic == "--ssh-status" && flag == "--config" => {
            (false, false, true, false, path)
        }
        [diagnostic, flag, path] if diagnostic == "--host-status" && flag == "--config" => {
            (false, false, false, false, path)
        }
        [diagnostic, flag, path] if diagnostic == "--firewall-status" && flag == "--config" => {
            (false, false, false, true, path)
        }
        _ => {
            eprintln!("request.invalid: {}", message(locale, "operation_failed"));
            return ExitCode::from(2);
        }
    };
    let config = match Config::load(Path::new(path)).and_then(|c| {
        if !host_diagnostic && !diagnostic && !ssh_diagnostic && !firewall_diagnostic {
            c.validate_runtime()?;
        }
        Ok(c)
    }) {
        Ok(config) => config,
        Err(code) => {
            if host_diagnostic || diagnostic || ssh_diagnostic || firewall_diagnostic {
                return diagnostic_error(code);
            }
            eprintln!("{code}: {}", message(locale, "operation_failed"));
            return ExitCode::from(2);
        }
    };
    if !explicit_locale {
        locale = &config.default_locale;
    }
    if host_diagnostic {
        return match garde_daemon::delivery::Store::inspect_host(
            &config,
            garde_daemon::runtime::now(),
        ) {
            Ok(health) => match serde_json::to_string(&health) {
                Ok(json) => {
                    println!("{json}");
                    if garde_daemon::host_health::healthy(&health) {
                        ExitCode::SUCCESS
                    } else {
                        ExitCode::from(1)
                    }
                }
                Err(_) => diagnostic_error("host.state"),
            },
            Err(code) => diagnostic_error(code),
        };
    }
    if firewall_diagnostic {
        return match garde_daemon::delivery::Store::inspect_firewall(
            &config,
            garde_daemon::runtime::now(),
        ) {
            Ok(health) => {
                let healthy = [&health.fail2ban, &health.firewall]
                    .iter()
                    .all(|h| h.state == garde_contracts::SshStatus::Healthy);
                match serde_json::to_string(&health) {
                    Ok(json) => {
                        println!("{json}");
                        if healthy {
                            ExitCode::SUCCESS
                        } else {
                            ExitCode::from(1)
                        }
                    }
                    Err(_) => diagnostic_error("firewall.state"),
                }
            }
            Err(code) => diagnostic_error(code),
        };
    }
    if ssh_diagnostic {
        return match garde_daemon::delivery::Store::inspect_ssh(
            &config,
            garde_daemon::runtime::now(),
        ) {
            Ok(health) => {
                let healthy = health.state == garde_contracts::SshStatus::Healthy;
                match serde_json::to_string(&health) {
                    Ok(json) => {
                        println!("{json}");
                        if healthy {
                            ExitCode::SUCCESS
                        } else {
                            ExitCode::from(1)
                        }
                    }
                    Err(_) => diagnostic_error("ssh.state"),
                }
            }
            Err(code) => diagnostic_error(code),
        };
    }
    if diagnostic {
        return match config.owner().and_then(|owner| {
            garde_daemon::delivery::Store::inspect(
                &config.state_dir,
                config.server_id.as_str(),
                owner,
                garde_daemon::runtime::now(),
            )
        }) {
            Ok(health) => {
                let degraded = health.dead_letter > 0
                    || health.oldest_pending_at.is_some_and(|at| {
                        garde_daemon::runtime::now().saturating_sub(at.timestamp()) >= 60
                    });
                match serde_json::to_string(&health) {
                    Ok(json) => {
                        println!("{json}");
                        if degraded {
                            ExitCode::from(1)
                        } else {
                            ExitCode::SUCCESS
                        }
                    }
                    Err(_) => diagnostic_error("storage.failed"),
                }
            }
            Err(code) => diagnostic_error(code),
        };
    }
    let initial = match garde_daemon::Bot::new(config.clone()) {
        Ok(bot) => bot,
        Err(code) => {
            eprintln!("{code}: {}", message(locale, "operation_failed"));
            return ExitCode::from(2);
        }
    };
    drop(initial);
    if check {
        println!("{}", message(locale, "configuration_valid"));
        return ExitCode::SUCCESS;
    }
    if !cfg!(target_os = "linux") || rustix::process::geteuid().is_root() {
        eprintln!("service.identity: {}", message(locale, "operation_failed"));
        return ExitCode::from(2);
    }
    let log_locale = locale.to_owned();
    let result =
        tokio::select! { result=garde_daemon::serve(config)=>result, result=shutdown()=>result };
    match result {
        Ok(()) => {
            eprintln!(
                "service.stopped: {}",
                message(&log_locale, "service_stopped")
            );
            ExitCode::SUCCESS
        }
        Err(code) => {
            eprintln!("{code}: {}", message(&log_locale, "operation_failed"));
            ExitCode::from(2)
        }
    }
}

fn diagnostic_error(code: &str) -> ExitCode {
    println!(
        "{}",
        serde_json::json!({"version":"1","error":{"code":"unavailable","message_key":code}})
    );
    ExitCode::from(2)
}

async fn monitoring_command(flag: &str, hash: Option<&str>, path: &Path) -> ExitCode {
    let result: Result<(serde_json::Value, bool), &'static str> = async {
        let config = Config::load(path)?;
        let now = garde_daemon::runtime::now();
        match flag {
            "--xui-status" => {
                let h = garde_daemon::delivery::Store::inspect_xui(&config, now)?;
                let normal = garde_daemon::xui::healthy(&h);
                Ok((serde_json::to_value(h).map_err(|_| "xui.state")?, normal))
            }
            "--services-status" => {
                let h = garde_daemon::delivery::Store::inspect_services(&config, now)?;
                let normal = garde_daemon::services::healthy(&h);
                Ok((
                    serde_json::to_value(h).map_err(|_| "services.state")?,
                    normal,
                ))
            }
            "--ports-status" => {
                let h = garde_daemon::delivery::Store::inspect_ports(&config, now)?;
                let normal = garde_daemon::listeners::healthy(&h);
                Ok((serde_json::to_value(h).map_err(|_| "ports.state")?, normal))
            }
            "--ports-baseline-preview" => {
                let saved =
                    garde_daemon::delivery::Store::inspect_observation(&config, "ports.state")?;
                let raw = garde_daemon::listeners::LinuxSource::default().read().await;
                let preview = garde_daemon::listeners::baseline_preview(
                    &config,
                    saved.as_deref(),
                    raw,
                    garde_daemon::runtime::now(),
                )?;
                Ok((
                    serde_json::to_value(preview).map_err(|_| "ports.state")?,
                    true,
                ))
            }
            "--ports-baseline-accept" => {
                let hash = hash
                    .filter(|h| {
                        h.len() == 64
                            && h.bytes()
                                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    })
                    .ok_or("request.invalid")?;
                if !cfg!(target_os = "linux") || rustix::process::geteuid().is_root() {
                    return Err("service.identity");
                }
                garde_daemon::delivery::Store::inspect_observation(&config, "ports.state")?;
                let database = garde_daemon::runtime::Database::open(config.clone()).await?;
                let raw = garde_daemon::listeners::LinuxSource::default().read().await;
                let mut monitor =
                    garde_daemon::listeners::Monitor::new(config.clone(), database.clone());
                monitor
                    .accept(raw, hash, garde_daemon::runtime::now())
                    .await?;
                let health = database
                    .call(move |s| s.ports_health(&config, garde_daemon::runtime::now()))
                    .await?;
                Ok((
                    serde_json::to_value(health).map_err(|_| "ports.state")?,
                    true,
                ))
            }
            _ => Err("request.invalid"),
        }
    }
    .await;
    match result {
        Ok((json, normal)) => {
            println!("{json}");
            if normal {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Err(code) => diagnostic_error(code),
    }
}
