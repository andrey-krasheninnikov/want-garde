use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[test]
fn validates_local_configuration_without_network_and_never_echoes_secrets() {
    let temporary = tempfile::tempdir().unwrap();
    let dir = temporary.path().to_path_buf();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let path = dir.join("config.json");
    let token = dir.join("token");
    fs::write(
        &token,
        "123456789:synthetic_secret_for_configuration_tests\n",
    )
    .unwrap();
    fs::set_permissions(&token, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&path, format!(r#"{{"server_id":"test-host","bot_token_file":"{}","state_dir":"{}","default_locale":"ru","owner_user_id":"123456789"}}"#, token.display(),dir.display())).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let check = || {
        Command::new(env!("CARGO_BIN_EXE_want-garde-daemon"))
            .args(["--check-config", "--config", path.to_str().unwrap()])
            .output()
            .unwrap()
    };
    let valid = check();
    assert!(
        valid.status.success(),
        "{}",
        String::from_utf8_lossy(&valid.stderr)
    );
    assert!(String::from_utf8_lossy(&valid.stdout).contains("Конфигурация корректна"));
    let english = Command::new(env!("CARGO_BIN_EXE_want-garde-daemon"))
        .args([
            "--lang",
            "en",
            "--check-config",
            "--config",
            path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&english.stdout).contains("Configuration is valid."));
    fs::set_permissions(&token, fs::Permissions::from_mode(0o644)).unwrap();
    let unsafe_token = check();
    assert_eq!(unsafe_token.status.code(), Some(2));
    assert!(unsafe_token.stdout.is_empty());
    assert!(String::from_utf8_lossy(&unsafe_token.stderr).contains("config.permissions"));
    assert!(!String::from_utf8_lossy(&unsafe_token.stderr).contains("synthetic_secret"));
    fs::set_permissions(&token, fs::Permissions::from_mode(0o600)).unwrap();
    let target = dir.join("real-token");
    fs::rename(&token, &target).unwrap();
    std::os::unix::fs::symlink(&target, &token).unwrap();
    assert_eq!(check().status.code(), Some(2));
    fs::remove_file(&token).unwrap();
    fs::rename(&target, &token).unwrap();
    let mut malformed: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    malformed["unexpected"] = serde_json::json!("synthetic_secret_do_not_echo");
    fs::write(&path, serde_json::to_vec(&malformed).unwrap()).unwrap();
    let bad = check();
    assert_eq!(bad.status.code(), Some(2));
    assert!(!String::from_utf8_lossy(&bad.stderr).contains("synthetic_secret"));
    fs::remove_file(&path).unwrap();
    fs::remove_file(&token).unwrap();
}
