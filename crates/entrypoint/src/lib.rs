use garde_contracts::PRODUCT_VERSION;
use std::{env, process::ExitCode};

pub fn run(program: &str) -> ExitCode {
    let args: Vec<_> = env::args_os().skip(1).collect();
    let args: Vec<_> = args.iter().map(|arg| arg.to_string_lossy()).collect();
    let (flag, requested_locale) = match args.as_slice() {
        [flag] => (flag.as_ref(), "en"),
        [language, locale, flag] if language == "--lang" => (flag.as_ref(), locale.as_ref()),
        _ => ("", "en"),
    };
    let catalog: serde_json::Value =
        serde_json::from_str(include_str!("../../../packages/i18n/catalog.json"))
            .expect("embedded_locale_catalog");
    let copy = catalog["locales"]
        .get(requested_locale)
        .unwrap_or(&catalog["locales"]["en"]);
    match flag {
        "--version" => {
            println!("{program} {PRODUCT_VERSION}");
            ExitCode::SUCCESS
        }
        "--help" => {
            println!(
                "{}: {program} [--lang LOCALE] --help | --version",
                copy["usage"].as_str().unwrap()
            );
            println!("{}", copy["scaffold"].as_str().unwrap());
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("not_implemented: {}", copy["unavailable"].as_str().unwrap());
            ExitCode::from(2)
        }
    }
}
