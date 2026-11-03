use std::{os::fd::AsFd, path::Path, process::ExitCode};
fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if let [serve, flag, path] = args.as_slice()
        && (serve == "--serve-read" || serve == "--serve-listeners-read")
        && flag == "--daemon-uid-file"
    {
        return match run(Path::new(path), serve == "--serve-listeners-read") {
            Ok(()) => ExitCode::SUCCESS,
            Err(code) => {
                eprintln!("{code}");
                ExitCode::from(2)
            }
        };
    }
    if args.last().is_some_and(|a| a == "--help")
        && (args.len() == 1 || (args.len() == 3 && args[0] == "--lang"))
    {
        let locale = if args.len() == 3 {
            args[1].to_str().unwrap_or("en")
        } else {
            "en"
        };
        println!(
            "{}: want-garde-helper [--lang LOCALE] --help | --version\nwant-garde-helper [--serve-read | --serve-listeners-read] --daemon-uid-file PATH",
            garde_entrypoint::message(locale, "usage")
        );
        println!("{}", garde_entrypoint::message(locale, "helper_read_only"));
        return ExitCode::SUCCESS;
    }
    garde_entrypoint::run("want-garde-helper")
}
fn run(path: &Path, listeners: bool) -> Result<(), &'static str> {
    if rustix::process::geteuid().as_raw() != 0 {
        return Err("helper.peer");
    }
    if !path.is_absolute() {
        return Err("helper.policy");
    }
    let bytes = garde_config::read_private(path, 32).map_err(|_| "helper.policy")?;
    let uid = std::str::from_utf8(&bytes)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .filter(|n| *n > 0)
        .ok_or("helper.policy")?;
    let fd = std::io::stdin()
        .as_fd()
        .try_clone_to_owned()
        .map_err(|_| "helper.socket")?;
    let socket = std::os::unix::net::UnixStream::from(fd);
    socket.set_nonblocking(true).map_err(|_| "helper.socket")?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| "helper.runtime")?
        .block_on(async {
            let stream = tokio::net::UnixStream::from_std(socket).map_err(|_| "helper.socket")?;
            if listeners {
                garde_helper::listeners::serve_connection(
                    stream,
                    uid,
                    &garde_helper::listeners::SystemListeners,
                )
                .await
            } else {
                garde_helper::serve_connection(stream, uid, &garde_helper::Fail2ban).await
            }
        })
}
