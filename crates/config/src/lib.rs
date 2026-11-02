use garde_contracts::Id;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

pub type Result<T> = std::result::Result<T, &'static str>;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub server_id: Id,
    pub bot_token_file: PathBuf,
    pub state_dir: PathBuf,
    pub default_locale: String,
    pub owner_user_id: Option<String>,
}

pub fn valid_user_id(value: &str) -> bool {
    !value.starts_with('0')
        && value.bytes().all(|c| c.is_ascii_digit())
        && value.parse::<u64>().is_ok_and(|n| n > 0 && n < (1 << 52))
}

pub fn valid_locale(locale: &str) -> bool {
    garde_entrypoint::catalog()["locales"].get(locale).is_some()
}

fn safe_parent(path: &Path) -> Result<()> {
    let parent = path.parent().ok_or("config.path")?;
    let m = fs::symlink_metadata(parent).map_err(|_| "config.path")?;
    if !m.is_dir()
        || m.mode() & 0o022 != 0
        || (m.uid() != 0 && m.uid() != rustix::process::geteuid().as_raw())
    {
        return Err("config.permissions");
    }
    Ok(())
}

pub fn read_private(path: &Path, limit: u64) -> Result<Vec<u8>> {
    safe_parent(path)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32)
        .open(path)
        .map_err(|_| "config.read")?;
    let m = file.metadata().map_err(|_| "config.read")?;
    if !m.is_file()
        || m.mode() & 0o027 != 0
        || (m.uid() != 0 && m.uid() != rustix::process::geteuid().as_raw())
    {
        return Err("config.permissions");
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "config.read")?;
    if bytes.len() as u64 > limit {
        return Err("config.size");
    }
    Ok(bytes)
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.is_absolute() {
            return Err("config.path");
        }
        let value: Self =
            serde_json::from_slice(&read_private(path, 65536)?).map_err(|_| "config.invalid")?;
        if !value.bot_token_file.is_absolute()
            || !value.state_dir.is_absolute()
            || value.bot_token_file == path
            || !valid_locale(&value.default_locale)
            || value
                .owner_user_id
                .as_ref()
                .is_some_and(|id| !valid_user_id(id))
        {
            return Err("config.invalid");
        }
        Ok(value)
    }

    pub fn owner(&self) -> Result<&str> {
        self.owner_user_id.as_deref().ok_or("owner.unbound")
    }

    pub fn token(&self) -> Result<Token> {
        let bytes = read_private(&self.bot_token_file, 256)?;
        let value = std::str::from_utf8(&bytes)
            .map_err(|_| "config.token")?
            .trim_end_matches(['\r', '\n']);
        let (number, secret) = value.split_once(':').ok_or("config.token")?;
        if !(5..=20).contains(&number.len())
            || !number.bytes().all(|c| c.is_ascii_digit())
            || !(20..=128).contains(&secret.len())
            || !secret
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
        {
            return Err("config.token");
        }
        Ok(Token(value.to_owned()))
    }

    pub fn validate_runtime(&self) -> Result<()> {
        self.owner()?;
        self.token()?;
        let m = fs::symlink_metadata(&self.state_dir).map_err(|_| "config.state")?;
        if !m.is_dir() || m.mode() & 0o077 != 0 || m.uid() != rustix::process::geteuid().as_raw() {
            return Err("config.state");
        }
        Ok(())
    }
}

// Token deliberately has no Debug or Serialize implementation.
pub struct Token(String);
impl Token {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub fn atomic_write(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    safe_parent(path)?;
    let temp = path.with_file_name(format!(
        ".garde-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&temp)
        .map_err(|_| "config.write")?;
    let result = (|| {
        if let Ok(metadata) = fs::symlink_metadata(path) {
            rustix::fs::fchown(
                &f,
                None,
                Some(rustix::process::Gid::from_raw(metadata.gid())),
            )
            .map_err(|_| "config.write")?;
        }
        f.set_permissions(fs::Permissions::from_mode(mode))
            .map_err(|_| "config.write")?;
        f.write_all(bytes).map_err(|_| "config.write")?;
        f.sync_all().map_err(|_| "config.write")?;
        fs::rename(&temp, path).map_err(|_| "config.write")?;
        File::open(path.parent().ok_or("config.path")?)
            .and_then(|f| f.sync_all())
            .map_err(|_| "config.write")
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

pub fn bind_owner(path: &Path, user_id: &str) -> Result<()> {
    if !valid_user_id(user_id) {
        return Err("owner.invalid");
    }
    safe_parent(path)?;
    let lock_path = path.with_extension("lock");
    let lock = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&lock_path)
        .map_err(|_| "config.locked")?;
    let result = (|| {
        let mut config = Config::load(path)?;
        if let Some(owner) = &config.owner_user_id {
            return if owner == user_id {
                Ok(())
            } else {
                Err("owner.conflict")
            };
        }
        let metadata = fs::metadata(path).map_err(|_| "config.read")?;
        if metadata.uid() != rustix::process::geteuid().as_raw() {
            return Err("config.permissions");
        }
        config.owner_user_id = Some(user_id.to_owned());
        let mut bytes = serde_json::to_vec_pretty(&config).map_err(|_| "config.invalid")?;
        bytes.push(b'\n');
        atomic_write(path, &bytes, metadata.mode() & 0o777)
    })();
    drop(lock);
    fs::remove_file(&lock_path).map_err(|_| "config.write")?;
    result
}
