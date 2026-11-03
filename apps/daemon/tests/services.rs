use garde_config::Config;
use garde_daemon::{delivery::Outcome, runtime::Database, services::Monitor};
use std::{fs, os::unix::fs::PermissionsExt};

fn config(directory: &std::path::Path) -> Config {
    serde_json::from_value(serde_json::json!({
        "server_id":"server-demo","owner_user_id":"123456789",
        "bot_token_file":directory.join("token"),"state_dir":directory,
        "default_locale":"en","services":{"units":["example.service"]}
    }))
    .unwrap()
}
fn state(active: &str) -> String {
    format!("Id=example.service\nLoadState=loaded\nActiveState={active}\nSubState=dead\n")
}
#[tokio::test]
async fn service_failure_and_recovery_survive_restart_without_duplicate_delivery() {
    let directory = tempfile::tempdir().unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config = config(directory.path());
    let database = Database::open(config.clone()).await.unwrap();
    let mut monitor = Monitor::new(config.clone(), database.clone());
    monitor.observe(Ok(state("inactive")), 1000).await.unwrap();
    let first = database.call(|s| s.claim(1000)).await.unwrap().unwrap();
    assert!(first.reply.text.contains("Service is not active"));
    assert!(first.reply.text.contains("example.service"));
    database
        .call(move |s| s.finish(first.id, Outcome::Delivered, 1000))
        .await
        .unwrap();
    let mut restarted = Monitor::new(config.clone(), database.clone());
    restarted
        .observe(Ok(state("inactive")), 1001)
        .await
        .unwrap();
    assert!(database.call(|s| s.claim(1001)).await.unwrap().is_none());
    restarted
        .observe(Err("services.permissions"), 1002)
        .await
        .unwrap();
    assert_eq!(
        database
            .call({
                let config = config.clone();
                move |s| s.services_health(&config, 1002)
            })
            .await
            .unwrap()
            .down,
        vec!["example.service"]
    );
    restarted.observe(Ok(state("active")), 1003).await.unwrap();
    let mut messages = Vec::new();
    while let Some(reply) = database.call(|s| s.claim(1003)).await.unwrap() {
        messages.push(reply.reply.text.clone());
        database
            .call(move |s| s.finish(reply.id, Outcome::Delivered, 1003))
            .await
            .unwrap();
    }
    assert!(messages.iter().any(|m| m.contains("Service recovered")));
}

struct Host;
impl garde_daemon::StatusSource for Host {
    async fn snapshot(&self) -> garde_contracts::HostSnapshot {
        serde_json::from_str(include_str!(
            "../../../fixtures/contracts/host-snapshot.json"
        ))
        .unwrap()
    }
}
#[tokio::test]
async fn service_commands_deny_groups_and_strangers_and_show_saved_owner_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let c = config(dir.path());
    let db = Database::open(c.clone()).await.unwrap();
    let mut m = Monitor::new(c.clone(), db.clone());
    m.observe(Ok(state("active")), 1000).await.unwrap();
    let mut b = garde_daemon::Bot::new(c).unwrap();
    for (id, user, chat) in [
        (0, 987654321, "private"),
        (1, 123456789, "group"),
        (2, 123456789, "private"),
    ] {
        let update = serde_json::json!({"update_id":id,"message":{"from":{"id":user,"is_bot":false},"chat":{"id":user,"type":chat},"text":"/services"}});
        b.process_update(&db, &update, &Host, "example_bot")
            .await
            .unwrap();
    }
    let d = db
        .call(|s| s.claim(garde_daemon::runtime::now()))
        .await
        .unwrap()
        .unwrap();
    assert!(d.reply.text.contains("Services"));
    assert!(d.reply.text.contains("example.service"));
    assert_eq!(d.reply.chat_id, "123456789");
    db.call(move |s| s.finish(d.id, Outcome::Delivered, garde_daemon::runtime::now()))
        .await
        .unwrap();
    assert!(
        db.call(|s| s.claim(garde_daemon::runtime::now()))
            .await
            .unwrap()
            .is_none()
    );
}

#[test]
fn local_service_diagnostic_reads_without_token_and_does_not_change_storage() {
    use garde_daemon::delivery::Store;
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let c = config(dir.path());
    let store = Store::open(dir.path(), c.server_id.as_str(), c.owner().unwrap()).unwrap();
    drop(store);
    let path = dir.path().join("config.json");
    fs::write(&path, serde_json::to_vec(&c).unwrap()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let before = fs::read(dir.path().join("garde.sqlite3")).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_want-garde-daemon"))
        .args(["--services-status", "--config"])
        .arg(path)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["enabled"], true);
    assert_eq!(json["stale"], true);
    assert_eq!(before, fs::read(dir.path().join("garde.sqlite3")).unwrap());
}

#[tokio::test]
async fn transitional_or_unknown_services_do_not_clear_a_failure_and_oneshot_active_is_healthy() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut c = config(dir.path());
    c.services.units.push("absent.service".into());
    let db = Database::open(c.clone()).await.unwrap();
    let mut m = Monitor::new(c.clone(), db.clone());
    m.observe(Ok(state("failed")), 1000).await.unwrap();
    for active in ["activating", "deactivating", "reloading", "unknown"] {
        m.observe(Ok(state(active)), 1001).await.unwrap();
        let h = db
            .call({
                let c = c.clone();
                move |s| s.services_health(&c, 1001)
            })
            .await
            .unwrap();
        assert_eq!(h.down, vec!["example.service"]);
        assert!(matches!(
            h.services[1].state,
            garde_contracts::Measurement::Unavailable { .. }
        ));
    }
    m.observe(
        Ok(state("active").replace("SubState=dead", "SubState=exited")),
        1002,
    )
    .await
    .unwrap();
    let h = db.call(move |s| s.services_health(&c, 1002)).await.unwrap();
    assert!(h.down.is_empty());
}

#[test]
fn monitoring_configuration_rejects_command_shapes_and_invalid_bounds() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = dir.path().join("config.json");
    let base = serde_json::to_value(config(dir.path())).unwrap();
    for (field, value) in [
        ("services", serde_json::json!({"units":["--help.service"]})),
        (
            "services",
            serde_json::json!({"units":["example.service","example.service"]}),
        ),
        (
            "services",
            serde_json::json!({"units":["example@.service"]}),
        ),
        ("services", serde_json::json!({"interval_seconds":0})),
        ("ports", serde_json::json!({"ephemeral_range":[60000,1000]})),
        (
            "ports",
            serde_json::json!({"ignore_patterns":["x".repeat(129)]}),
        ),
        (
            "ports",
            serde_json::json!({"expected_process_names":["bad\nname"]}),
        ),
    ] {
        let mut json = base.clone();
        json[field] = value;
        fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(Config::load(&path).is_err());
    }
    let mut old = base;
    old.as_object_mut().unwrap().remove("services");
    old.as_object_mut().unwrap().remove("ports");
    fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
    let c = Config::load(&path).unwrap();
    assert!(c.services.units.is_empty());
    assert!(c.ports.enabled);
}

#[tokio::test]
async fn service_aliases_report_failure_and_recovery_without_invalidating_other_units() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut c = config(dir.path());
    c.services.units.push("alias.service".into());
    let db = Database::open(c.clone()).await.unwrap();
    let mut m = Monitor::new(c.clone(), db.clone());
    for (at, active, reason) in [
        (1000, "failed", "Service is not active"),
        (1001, "active", "Service recovered"),
    ] {
        let row = state(active).replace(
            "LoadState=",
            "Names=example.service alias.service\nLoadState=",
        );
        m.observe(Ok(format!("{row}\n{row}")), at).await.unwrap();
        let mut units = Vec::new();
        while let Some(d) = db.call(move |s| s.claim(at)).await.unwrap() {
            assert!(d.reply.text.contains(reason), "{}", d.reply.text);
            units.push(if d.reply.text.contains("alias.service") {
                "alias.service"
            } else {
                "example.service"
            });
            db.call(move |s| s.finish(d.id, Outcome::Delivered, at))
                .await
                .unwrap();
        }
        units.sort();
        assert_eq!(units, vec!["alias.service", "example.service"]);
    }
}
