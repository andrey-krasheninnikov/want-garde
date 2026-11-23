use crate::{
    access::Actor,
    delivery,
    runtime::{Database, now},
};
use garde_config::{Config, Result};
use garde_contracts::access::{LocalRequest, MAX_REQUEST, MAX_RESPONSE, SOCKET_NAME};
use std::{
    fs,
    io::ErrorKind,
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    path::Path,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    task::JoinSet,
};

const MAX_CLIENTS: usize = 4;
const TIMEOUT: Duration = Duration::from_secs(3);

pub(crate) async fn run(config: &Config, database: Database) -> Result<()> {
    delivery::directory(&config.state_dir)?;
    let path = config.state_dir.join(SOCKET_NAME);
    let listener = bind(&path)?;
    let mut clients = JoinSet::new();
    let owner = config.owner()?;
    loop {
        tokio::select! {
            client = listener.accept(), if clients.len() < MAX_CLIENTS => {
                let (stream, _) = client.map_err(|_| "access.socket")?;
                let database = database.clone();
                let server = config.server_id.clone();
                let owner = owner.to_owned();
                clients.spawn(async move {
                    let _ = tokio::time::timeout(TIMEOUT, handle(stream, database, server, owner)).await;
                });
            }
            _ = clients.join_next(), if !clients.is_empty() => {}
        }
    }
}

fn bind(path: &Path) -> Result<UnixListener> {
    match fs::symlink_metadata(path) {
        Ok(m)
            if m.file_type().is_socket()
                && m.uid() == rustix::process::geteuid().as_raw()
                && m.mode() & 0o077 == 0 =>
        {
            // The writer lock excludes another daemon for this state directory.
            fs::remove_file(path).map_err(|_| "access.socket")?;
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        _ => return Err("access.socket"),
    }
    let listener = UnixListener::bind(path).map_err(|_| "access.socket")?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|_| "access.socket")?;
    Ok(listener)
}

async fn handle(
    mut stream: UnixStream,
    database: Database,
    server: garde_contracts::Id,
    owner: String,
) -> Result<()> {
    let uid = stream.peer_cred().ok().map(|peer| peer.uid());
    let result = if uid == Some(0) {
        execute(&mut stream, &database, &server, &owner).await
    } else {
        database
            .call(move |store| {
                crate::access::audit(
                    &store.connection,
                    Actor::local(uid),
                    None,
                    "local_peer_denied",
                    now(),
                )
            })
            .await?;
        // Close cleanly after reading the bounded frame without interpreting it.
        tokio::io::copy(
            &mut (&mut stream).take((MAX_REQUEST + 1) as u64),
            &mut tokio::io::sink(),
        )
        .await
        .map_err(|_| "access.socket")?;
        Err("access.denied")
    };
    let response = serde_json::to_vec(&result).map_err(|_| "access.response")?;
    if response.len() > MAX_RESPONSE {
        return Err("access.response");
    }
    stream
        .write_all(&response)
        .await
        .map_err(|_| "access.socket")?;
    stream.shutdown().await.map_err(|_| "access.socket")
}

async fn execute(
    stream: &mut UnixStream,
    database: &Database,
    server: &garde_contracts::Id,
    owner: &str,
) -> Result<garde_contracts::access::AccessOutput> {
    let mut bytes = Vec::new();
    stream
        .take((MAX_REQUEST + 1) as u64)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| "access.socket")?;
    if bytes.len() > MAX_REQUEST {
        return Err("request.invalid");
    }
    let request: LocalRequest = serde_json::from_slice(&bytes).map_err(|_| "request.invalid")?;
    if &request.server_id != server || request.owner_user_id != owner {
        return Err("storage.identity");
    }
    database
        .call(move |store| store.local_access(request.command, now()))
        .await
}
