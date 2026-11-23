use garde_config::{Config, Result};
use garde_contracts::access::{
    AccessCommand, AccessOutput, LocalRequest, MAX_RESPONSE, SOCKET_NAME,
};
use std::{
    fs,
    io::{Read, Write},
    net::Shutdown,
    os::unix::{
        fs::{FileTypeExt, MetadataExt},
        net::UnixStream,
    },
    path::Path,
    time::Duration,
};

const TIMEOUT: Duration = Duration::from_secs(5);

pub fn execute(args: &[String], locale: &str) -> Result<()> {
    let (path, args) = match args {
        [flag, path, rest @ ..] if flag == "--config" => (path.as_str(), rest),
        _ => ("/etc/want-garde/config.json", args),
    };
    let words: Vec<_> = args.iter().map(String::as_str).collect();
    let command = AccessCommand::parse(&words).ok_or("request.invalid")?;
    let config = Config::load(Path::new(path))?;
    let request = LocalRequest {
        server_id: config.server_id.clone(),
        owner_user_id: config.owner()?.to_owned(),
        command,
    };
    match request_access(&config, &request)? {
        AccessOutput::Updated {} => {
            println!("{}", garde_entrypoint::message(locale, "access_updated"))
        }
        listing @ AccessOutput::List { .. } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&listing).map_err(|_| "access.response")?
            );
        }
    }
    Ok(())
}

fn request_access(config: &Config, request: &LocalRequest) -> Result<AccessOutput> {
    let path = config.state_dir.join(SOCKET_NAME);
    let dir = fs::symlink_metadata(&config.state_dir).map_err(|_| "access.socket")?;
    let socket = fs::symlink_metadata(&path).map_err(|_| "access.socket")?;
    if !dir.is_dir()
        || dir.mode() & 0o077 != 0
        || !socket.file_type().is_socket()
        || socket.uid() != dir.uid()
        || socket.mode() & 0o077 != 0
    {
        return Err("access.socket");
    }
    let mut stream = UnixStream::connect(path).map_err(|_| "access.socket")?;
    stream
        .set_read_timeout(Some(TIMEOUT))
        .map_err(|_| "access.socket")?;
    stream
        .set_write_timeout(Some(TIMEOUT))
        .map_err(|_| "access.socket")?;
    let bytes = serde_json::to_vec(request).map_err(|_| "request.invalid")?;
    stream.write_all(&bytes).map_err(|_| "access.socket")?;
    stream
        .shutdown(Shutdown::Write)
        .map_err(|_| "access.socket")?;
    let mut bytes = Vec::new();
    stream
        .take((MAX_RESPONSE + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "access.socket")?;
    if bytes.len() > MAX_RESPONSE {
        return Err("access.response");
    }
    let response: std::result::Result<AccessOutput, String> =
        serde_json::from_slice(&bytes).map_err(|_| "access.response")?;
    response.map_err(|code| match code.as_str() {
        "access.denied" => "access.denied",
        "access.limit" => "access.limit",
        "request.invalid" => "request.invalid",
        "storage.identity" => "storage.identity",
        _ => "access.failed",
    })
}
