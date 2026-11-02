use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[test]
fn local_binding_is_persistent_idempotent_and_never_replaces_an_owner() {
    let dir = std::env::temp_dir().join(format!("garde-owner-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let path = dir.join("config.json");
    fs::write(&path, format!(r#"{{"server_id":"test-host","bot_token_file":"{}","state_dir":"{}","default_locale":"ru"}}"#, dir.join("token").display(), dir.display())).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    let bind = |id: &str| {
        Command::new(env!("CARGO_BIN_EXE_want-garde"))
            .args([
                "owner",
                "bind",
                "--config",
                path.to_str().unwrap(),
                "--user-id",
                id,
            ])
            .output()
            .unwrap()
    };
    let first = bind("123456789");
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(bind("123456789").status.success());
    let before = fs::read(&path).unwrap();
    let conflict = bind("987654321");
    assert_eq!(conflict.status.code(), Some(2));
    assert_eq!(fs::read(&path).unwrap(), before);
    let saved: serde_json::Value = serde_json::from_slice(&before).unwrap();
    assert_eq!(saved["owner_user_id"], "123456789");
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o640
    );
    assert!(!String::from_utf8_lossy(&conflict.stderr).contains("987654321"));
    fs::remove_file(&path).unwrap();
    fs::remove_dir(&dir).unwrap();
}
