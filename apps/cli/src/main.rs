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
        return ExitCode::SUCCESS;
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
        Err(code) => {
            eprintln!(
                "{code}: {}",
                garde_entrypoint::message(locale, "operation_failed")
            );
            ExitCode::from(2)
        }
    }
}
