mod access;

use std::{path::Path, process::ExitCode};

fn main() -> ExitCode {
    let Ok(args) = std::env::args_os()
        .skip(1)
        .map(|s| s.into_string())
        .collect::<Result<Vec<_>, _>>()
    else {
        eprintln!("request.invalid");
        return ExitCode::from(2);
    };
    let (locale, args) = if args.first().is_some_and(|s| s == "--lang") && args.len() >= 2 {
        (args[1].as_str(), &args[2..])
    } else {
        ("en", args.as_slice())
    };
    if args == ["--version"] {
        return garde_entrypoint::run("want-garde");
    }
    if args == ["--help"] {
        println!(
            "{}: want-garde [--lang LOCALE] owner bind --config PATH --user-id ID",
            garde_entrypoint::message(locale, "usage")
        );
        println!("want-garde [--lang LOCALE] access [--config PATH] list|grant|revoke|group");
        return ExitCode::SUCCESS;
    }
    if let [command, rest @ ..] = args
        && command == "access"
    {
        return match access::execute(rest, locale) {
            Ok(()) => ExitCode::SUCCESS,
            Err(code) => operation_failed(code, locale),
        };
    }
    let result = match args {
        [owner, bind, config, path, user_id, id]
            if owner == "owner"
                && bind == "bind"
                && config == "--config"
                && user_id == "--user-id" =>
        {
            garde_config::bind_owner(Path::new(path), id)
        }
        _ => Err("request.invalid"),
    };
    match result {
        Ok(()) => {
            println!("{}", garde_entrypoint::message(locale, "owner_bound"));
            ExitCode::SUCCESS
        }
        Err(code) => operation_failed(code, locale),
    }
}

fn operation_failed(code: &'static str, locale: &str) -> ExitCode {
    let key = match code {
        "access.denied" => "access_denied",
        "access.limit" => "access_limit",
        _ => "operation_failed",
    };
    eprintln!("{code}: {}", garde_entrypoint::message(locale, key));
    ExitCode::from(2)
}
