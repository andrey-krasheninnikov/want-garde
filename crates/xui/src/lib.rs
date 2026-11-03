mod database;
pub use database::snapshot;
pub type Result<T> = std::result::Result<T, &'static str>;

pub fn fingerprint(value: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(value))
}
pub fn label(value: &str) -> Result<String> {
    if value.is_empty() || value.len() > 80 || value.chars().any(char::is_control) {
        return Err("xui.output");
    }
    Ok(value.into())
}
pub mod logs;

mod parser;
