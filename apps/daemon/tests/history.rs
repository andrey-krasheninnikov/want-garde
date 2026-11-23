use garde_contracts::{Event, Id};
use garde_daemon::delivery::Store;
use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[test]
fn history_pages_are_stable_and_local_diagnostics_need_no_token() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config = dir.path().join("config.json");
    garde_config::atomic_write(
        &config,
        &serde_json::to_vec(&serde_json::json!({
            "server_id":"server-demo", "bot_token_file":dir.path().join("absent"),
            "state_dir":dir.path(), "owner_user_id":"123456789", "default_locale":"en"
        }))
        .unwrap(),
        0o600,
    )
    .unwrap();
    let mut event: Event =
        serde_json::from_str(include_str!("../../../fixtures/contracts/event.json")).unwrap();
    let mut store = Store::open(dir.path(), "server-demo", "123456789").unwrap();
    for id in ["event-a", "event-b"] {
        event.id = Id::new(id).unwrap();
        store
            .publish(&event, &[], garde_daemon::runtime::now())
            .unwrap();
    }
    let run = |arguments: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_want-garde-daemon"))
            .args(arguments)
            .arg("--config")
            .arg(&config)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    let first = run(&["--recent", "1"]);
    assert_eq!(first["events"][0]["id"], "event-b");
    let second = run(&["--recent", "1", first["next_cursor"].as_str().unwrap()]);
    assert_eq!(second["events"][0]["id"], "event-a");
    assert!(second["next_cursor"].is_null());
    assert_eq!(
        run(&["--history", "event-a"])["source"],
        event.source.as_str()
    );
    assert_eq!(run(&["--stats", "24"])["total"], 2);
}

#[test]
fn limits_preserve_unfinished_delivery_and_expire_completed_storage() {
    use garde_daemon::{
        Reply,
        history::{self, Output, Query},
        retention::AuditClass,
    };
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config: garde_config::Config = serde_json::from_value(serde_json::json!({
        "server_id":"server-demo", "bot_token_file":dir.path().join("absent"),
        "state_dir":dir.path(), "owner_user_id":"123456789", "default_locale":"en",
        "retention": {"process_audit_days":1, "action_audit_days":3, "result_hours":1,
                      "pending_deliveries":2048, "result_mib":1}
    }))
    .unwrap();
    let mut store =
        Store::open_with_retention(dir.path(), "server-demo", "123456789", config.retention)
            .unwrap();
    let code = Id::new("accepted").unwrap();
    for (kind, id) in [
        (AuditClass::Action, "action"),
        (AuditClass::Process, "process"),
    ] {
        store
            .record_audit(kind, &Id::new(id).unwrap(), &code, 100)
            .unwrap();
    }
    let result = store
        .store_result(&Id::new("result").unwrap(), &vec![1; 1 << 20], 100)
        .unwrap();
    assert_eq!(
        store.store_result(&Id::new("overflow").unwrap(), b"no room", 101),
        Err("storage.quota")
    );
    let now = 100_000;
    let reply = || {
        Some(Reply {
            locale: None,
            chat_id: "123456789".into(),
            text: "pending result".into(),
        })
    };
    assert_eq!(store.consume_update(1, reply(), now), Ok(2));
    let mut event: Event =
        serde_json::from_str(include_str!("../../../fixtures/contracts/event.json")).unwrap();
    let replies: Vec<_> = (0..32)
        .map(|i| Reply {
            locale: None,
            chat_id: (123450000 + i).to_string(),
            text: "queued event".into(),
        })
        .collect();
    for i in 0..63 {
        event.id = Id::new(format!("event-batch-{i}")).unwrap();
        store.publish(&event, &replies, now).unwrap();
    }
    event.id = Id::new("event-last").unwrap();
    store.publish(&event, &replies[..31], now).unwrap();
    assert_eq!(store.consume_update(2, reply(), now), Err("storage.quota"));
    let Output::Health(health) = history::inspect(&config, Query::Health, now).unwrap() else {
        panic!("expected health")
    };
    assert!(health.storage.backpressure);
    assert_eq!(health.delivery.pending, 2048);
    assert_eq!(health.storage.action_audit_records, 1);
    assert_eq!(health.storage.process_audit_records, 0);
    assert_eq!(health.storage.result_bytes, 0);
    assert!(!result.exists());
    let stage = dir.path().join("results/.garde-99-2");
    garde_config::atomic_write(&stage, b"interrupted", 0o600).unwrap();
    drop(store);
    let mut store =
        Store::open_with_retention(dir.path(), "server-demo", "123456789", config.retention)
            .unwrap();
    assert!(!dir.path().join("results/.garde-99-2").exists());
    assert_eq!(store.offset(), Ok(2));
    assert!(
        store
            .claim(now)
            .unwrap()
            .unwrap()
            .reply
            .text
            .starts_with("pending result")
    );
}
