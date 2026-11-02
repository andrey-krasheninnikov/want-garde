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

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
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
            "{}: want-garde-daemon [--lang LOCALE] [--check-config] --config PATH",
            message(locale, "usage")
        );
        return ExitCode::SUCCESS;
    }
    let (check, path) = match args {
        [flag, path] if flag == "--config" => (false, path),
        [check, flag, path] if check == "--check-config" && flag == "--config" => (true, path),
        _ => {
            eprintln!("request.invalid: {}", message(locale, "operation_failed"));
            return ExitCode::from(2);
        }
    };
    let config = match Config::load(Path::new(path)).and_then(|c| {
        c.validate_runtime()?;
        Ok(c)
    }) {
        Ok(config) => config,
        Err(code) => {
            eprintln!("{code}: {}", message(locale, "operation_failed"));
            return ExitCode::from(2);
        }
    };
    if !explicit_locale {
        locale = &config.default_locale;
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
