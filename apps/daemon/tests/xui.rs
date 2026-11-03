use garde_config::Config;
use garde_daemon::{
    Reply,
    runtime::{Database, Identity, Transport, send_loop},
    telegram::TransportError,
    xui::Monitor,
};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    sync::{Arc, Mutex},
    time::Duration,
};
#[derive(Default)]
struct Telegram(Mutex<Vec<String>>);
impl Transport for Telegram {
    async fn identity(&self) -> Result<Identity, TransportError> {
        unreachable!()
    }
    async fn updates(&self, _: i64) -> Result<Vec<serde_json::Value>, TransportError> {
        std::future::pending().await
    }
    async fn send(&self, reply: &Reply) -> Result<(), TransportError> {
        self.0.lock().unwrap().push(reply.text.clone());
        Ok(())
    }
}
#[tokio::test]
async fn synthetic_panel_activity_reaches_delivery_and_survives_monitor_restart() {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config:Config=serde_json::from_value(serde_json::json!({"server_id":"server-demo","owner_user_id":"123456789","bot_token_file":d.path().join("absent"),"state_dir":d.path().join("state"),"default_locale":"en","xui":{"enabled":true,"client_disconnect_after_seconds":30}})).unwrap();
    fs::create_dir(&config.state_dir).unwrap();
    fs::set_permissions(&config.state_dir, fs::Permissions::from_mode(0o700)).unwrap();
    let path = d.path().join("panel.db");
    let c = rusqlite::Connection::open(&path).unwrap();
    c.execute_batch("CREATE TABLE inbounds(id INTEGER PRIMARY KEY,enable INTEGER,port INTEGER,protocol TEXT,settings TEXT);CREATE TABLE settings(id INTEGER PRIMARY KEY,key TEXT,value TEXT);CREATE TABLE users(id INTEGER PRIMARY KEY,username TEXT,password TEXT);CREATE TABLE client_traffics(inbound_id INTEGER,email TEXT,enable INTEGER,up INTEGER,down INTEGER,last_online INTEGER);INSERT INTO users VALUES(1,'operator','PRIVATE_PASSWORD_MARKER');INSERT INTO client_traffics VALUES(7,'alice',1,10,20,0);").unwrap();
    let db = Database::open(config.clone()).await.unwrap();
    let mut monitor = Monitor::new(config.clone(), db.clone());
    let now = garde_daemon::runtime::now();
    monitor
        .observe(garde_xui::snapshot(&path, now), now)
        .await
        .unwrap();
    assert_eq!(db.call(move |s| s.health(now)).await.unwrap().pending, 0);
    c.execute("UPDATE client_traffics SET last_online=?1", [now])
        .unwrap();
    monitor
        .observe(garde_xui::snapshot(&path, now), now)
        .await
        .unwrap();
    let transport = Arc::new(Telegram::default());
    let sender = tokio::spawn(send_loop(db.clone(), transport.clone()));
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if !transport.0.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await
        }
    })
    .await
    .unwrap();
    sender.abort();
    let _ = sender.await;
    {
        let messages = transport.0.lock().unwrap();
        assert!(messages.iter().any(|s| s.contains("alice")));
        assert!(messages.iter().all(|s| !s.contains("PRIVATE_")));
    }
    let mut restarted = Monitor::new(config, db.clone());
    restarted.observe(Err("xui.locked"), now + 1).await.unwrap();
    let h = db.call(move |s| s.health(now + 1)).await.unwrap();
    assert_eq!(h.pending, 1);
    restarted
        .observe(garde_xui::snapshot(&path, now + 2), now + 2)
        .await
        .unwrap();
    let mut replies = vec![];
    while let Some(reply) = db.call(move |s| s.claim(now + 2)).await.unwrap() {
        replies.push(reply.reply.text.clone());
        db.call(move |s| {
            s.finish(
                reply.id,
                garde_daemon::delivery::Outcome::Delivered,
                now + 2,
            )
        })
        .await
        .unwrap();
    }
    assert!(
        !replies
            .iter()
            .any(|s| s.contains("xui_client_disconnected"))
    );
    assert!(
        !fs::read(d.path().join("state/garde.sqlite3"))
            .unwrap()
            .windows(8)
            .any(|w| w == b"PRIVATE_")
    );
}
#[tokio::test]
async fn replaying_a_saved_log_position_does_not_enqueue_another_notification() {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config:Config=serde_json::from_value(serde_json::json!({"server_id":"server-demo","owner_user_id":"123456789","bot_token_file":d.path().join("absent"),"state_dir":d.path(),"default_locale":"en","xui":{"enabled":true}})).unwrap();
    let db = Database::open(config.clone()).await.unwrap();
    let mut monitor = Monitor::new(config, db.clone());
    let now = garde_daemon::runtime::now();
    let record = garde_contracts::XuiLogRecord {
        id: garde_contracts::Id::new(format!("xui-log-{}", "a".repeat(64))).unwrap(),
        observed_at: chrono::DateTime::from_timestamp(now, 0).unwrap(),
        reason_key: garde_contracts::Id::new("xui_panel_login").unwrap(),
        evidence: vec![garde_contracts::Evidence {
            name: garde_contracts::Id::new("xui_account").unwrap(),
            value: "operator".into(),
        }],
    };
    let batch = garde_contracts::XuiLogBatch {
        checkpoint: Default::default(),
        sources: vec![garde_contracts::XuiSourceHealth {
            source: garde_contracts::Id::new("xui.journal").unwrap(),
            reason_key: None,
            gap: false,
        }],
        records: vec![record],
    };
    monitor.observe_logs(Ok(batch.clone()), now).await.unwrap();
    let reply = db.call(move |s| s.claim(now)).await.unwrap().unwrap();
    db.call(move |s| s.finish(reply.id, garde_daemon::delivery::Outcome::Delivered, now))
        .await
        .unwrap();
    monitor.observe_logs(Ok(batch), now + 1).await.unwrap();
    assert_eq!(
        db.call(move |s| s.health(now + 1)).await.unwrap().pending,
        0
    );
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
async fn xui_status_replies_only_to_the_owner_in_private_chat() {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let c:Config=serde_json::from_value(serde_json::json!({"server_id":"server-demo","owner_user_id":"123456789","bot_token_file":d.path().join("absent"),"state_dir":d.path(),"default_locale":"en","xui":{"enabled":true}})).unwrap();
    let db = Database::open(c.clone()).await.unwrap();
    let mut b = garde_daemon::Bot::new(c).unwrap();
    for (id, user, chat) in [
        (0, 987654321, "private"),
        (1, 123456789, "group"),
        (2, 123456789, "private"),
    ] {
        let u = serde_json::json!({"update_id":id,"message":{"from":{"id":user,"is_bot":false},"chat":{"id":user,"type":chat},"text":"/xui"}});
        b.process_update(&db, &u, &Host, "example").await.unwrap();
    }
    let reply = db
        .call(|s| s.claim(garde_daemon::runtime::now()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply.reply.chat_id, "123456789");
    assert!(reply.reply.text.contains("x-ui"));
    db.call(move |s| {
        s.finish(
            reply.id,
            garde_daemon::delivery::Outcome::Delivered,
            garde_daemon::runtime::now(),
        )
    })
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
fn xui_diagnostic_needs_no_token_and_leaves_saved_data_unchanged() {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let c:Config=serde_json::from_value(serde_json::json!({"server_id":"server-demo","owner_user_id":"123456789","bot_token_file":d.path().join("absent"),"state_dir":d.path(),"default_locale":"en","xui":{"enabled":true}})).unwrap();
    drop(garde_daemon::delivery::Store::open(d.path(), "server-demo", "123456789").unwrap());
    let p = d.path().join("config.json");
    fs::write(&p, serde_json::to_vec(&c).unwrap()).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
    let before = fs::read(d.path().join("garde.sqlite3")).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_want-garde-daemon"))
        .args(["--xui-status", "--config"])
        .arg(p)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let h: garde_contracts::XuiHealth = serde_json::from_slice(&out.stdout).unwrap();
    assert!(h.stale);
    assert_eq!(before, fs::read(d.path().join("garde.sqlite3")).unwrap());
}
#[tokio::test]
async fn outage_retains_online_state_and_only_a_valid_expired_snapshot_disconnects() {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let c:Config=serde_json::from_value(serde_json::json!({"server_id":"server-demo","owner_user_id":"123456789","bot_token_file":d.path().join("absent"),"state_dir":d.path(),"default_locale":"en","xui":{"enabled":true,"client_disconnect_after_seconds":30}})).unwrap();
    let db = Database::open(c.clone()).await.unwrap();
    let mut monitor = Monitor::new(c.clone(), db.clone());
    let snapshot = |at, last| garde_contracts::XuiSnapshot {
        source_id: "a".repeat(64),
        version: garde_contracts::WireVersion::V1,
        observed_at: chrono::DateTime::from_timestamp(at, 0).unwrap(),
        schema: Some(garde_contracts::Id::new("xui.normalized").unwrap()),
        panel: garde_contracts::Measurement::Available { value: vec![] },
        clients: garde_contracts::Measurement::Available {
            value: vec![garde_contracts::XuiClient {
                key: "b".repeat(64),
                label: "alice".into(),
                inbound_id: 7,
                enabled: true,
                last_online: Some(last),
                traffic_bytes: 30,
            }],
        },
        connectivity_available: true,
    };
    monitor
        .observe(Ok(snapshot(1000, 1000)), 1000)
        .await
        .unwrap();
    monitor.observe(Err("xui.locked"), 1100).await.unwrap();
    let h = db
        .call({
            let c = c.clone();
            move |s| s.xui_health(&c, 1100)
        })
        .await
        .unwrap();
    assert_eq!(
        h.clients[0].state,
        garde_contracts::XuiConnectionState::Online
    );
    monitor
        .observe(Ok(snapshot(1101, 1000)), 1101)
        .await
        .unwrap();
    let mut messages = vec![];
    while let Some(reply) = db.call(|s| s.claim(1101)).await.unwrap() {
        messages.push(reply.reply.text.clone());
        db.call(move |s| s.finish(reply.id, garde_daemon::delivery::Outcome::Delivered, 1101))
            .await
            .unwrap();
    }
    assert_eq!(
        messages
            .iter()
            .filter(|m| m.contains("x-ui client disconnected"))
            .count(),
        1
    );
    let mut restarted = Monitor::new(c, db.clone());
    restarted
        .observe(Ok(snapshot(1102, 1000)), 1102)
        .await
        .unwrap();
    assert!(db.call(|s| s.claim(1102)).await.unwrap().is_none());
}
#[tokio::test]
async fn xui_messages_and_status_follow_every_saved_locale_and_isolate_rtl_values() {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let c:Config=serde_json::from_value(serde_json::json!({"server_id":"server-demo","owner_user_id":"123456789","bot_token_file":d.path().join("absent"),"state_dir":d.path(),"default_locale":"en","xui":{"enabled":true}})).unwrap();
    let db = Database::open(c.clone()).await.unwrap();
    let mut monitor = Monitor::new(c.clone(), db.clone());
    let mut bot = garde_daemon::Bot::new(c).unwrap();
    let now = garde_daemon::runtime::now();
    let long_label = "a".repeat(80);
    monitor
        .observe(
            Ok(garde_contracts::XuiSnapshot {
                source_id: "a".repeat(64),
                version: garde_contracts::WireVersion::V1,
                observed_at: chrono::DateTime::from_timestamp(now, 0).unwrap(),
                schema: Some(garde_contracts::Id::new("xui.normalized").unwrap()),
                panel: garde_contracts::Measurement::Available { value: vec![] },
                clients: garde_contracts::Measurement::Available {
                    value: vec![garde_contracts::XuiClient {
                        key: "b".repeat(64),
                        label: long_label.clone(),
                        inbound_id: 7,
                        enabled: true,
                        last_online: Some(now),
                        traffic_bytes: 30,
                    }],
                },
                connectivity_available: true,
            }),
            now,
        )
        .await
        .unwrap();
    for (index, (locale, copy)) in garde_entrypoint::catalog()["locales"]
        .as_object()
        .unwrap()
        .iter()
        .enumerate()
    {
        let update = |id, text: String| serde_json::json!({"update_id":id,"message":{"from":{"id":123456789,"is_bot":false},"chat":{"id":123456789,"type":"private"},"text":text}});
        bot.process_update(
            &db,
            &update(index * 2, format!("/language {locale}")),
            &Host,
            "example",
        )
        .await
        .unwrap();
        let ack = db
            .call(move |s| s.claim(garde_daemon::runtime::now()))
            .await
            .unwrap()
            .unwrap();
        db.call(move |s| {
            s.finish(
                ack.id,
                garde_daemon::delivery::Outcome::Delivered,
                garde_daemon::runtime::now(),
            )
        })
        .await
        .unwrap();
        let record = garde_contracts::XuiLogRecord {
            id: garde_contracts::Id::new(format!("xui-log-{index:064x}")).unwrap(),
            observed_at: chrono::DateTime::from_timestamp(now, 0).unwrap(),
            reason_key: garde_contracts::Id::new("xui_panel_login").unwrap(),
            evidence: vec![garde_contracts::Evidence {
                name: garde_contracts::Id::new("xui_account").unwrap(),
                value: "operator".into(),
            }],
        };
        monitor
            .observe_logs(
                Ok(garde_contracts::XuiLogBatch {
                    checkpoint: Default::default(),
                    sources: vec![garde_contracts::XuiSourceHealth {
                        source: garde_contracts::Id::new("xui.journal").unwrap(),
                        reason_key: None,
                        gap: false,
                    }],
                    records: vec![record],
                }),
                now,
            )
            .await
            .unwrap();
        let event = db
            .call(move |s| s.claim(garde_daemon::runtime::now()))
            .await
            .unwrap()
            .unwrap();
        assert!(
            event
                .reply
                .text
                .contains(copy["xui_panel_login"].as_str().unwrap())
        );
        assert!(!event.reply.text.contains("message.unavailable"));
        if ["ar", "he"].contains(&locale.as_str()) {
            assert!(event.reply.text.contains('\u{2068}'))
        }
        db.call(move |s| {
            s.finish(
                event.id,
                garde_daemon::delivery::Outcome::Delivered,
                garde_daemon::runtime::now(),
            )
        })
        .await
        .unwrap();
        bot.process_update(&db, &update(index * 2 + 1, "/xui".into()), &Host, "example")
            .await
            .unwrap();
        let status = db
            .call(move |s| s.claim(garde_daemon::runtime::now()))
            .await
            .unwrap()
            .unwrap();
        assert!(
            status
                .reply
                .text
                .contains(copy["xui_title"].as_str().unwrap())
        );
        assert!(status.reply.text.encode_utf16().count() <= 4096);
        assert!(status.reply.text.contains(&long_label));
        assert!(
            status.reply.text.contains(&now.to_string()),
            "{locale}: missing last-online value"
        );
        db.call(move |s| {
            s.finish(
                status.id,
                garde_daemon::delivery::Outcome::Delivered,
                garde_daemon::runtime::now(),
            )
        })
        .await
        .unwrap();
    }
}
#[tokio::test]
async fn client_activity_notification_keeps_the_complete_bounded_target_endpoint() {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config:Config=serde_json::from_value(serde_json::json!({"server_id":"server-demo","owner_user_id":"123456789","bot_token_file":d.path().join("absent"),"state_dir":d.path(),"default_locale":"en","xui":{"enabled":true}})).unwrap();
    let db = Database::open(config.clone()).await.unwrap();
    let mut monitor = Monitor::new(config, db.clone());
    let target = format!(
        "{}.{}.{}.{}.example:443",
        "a".repeat(50),
        "b".repeat(50),
        "c".repeat(50),
        "d".repeat(50)
    );
    let record = garde_contracts::XuiLogRecord {
        id: garde_contracts::Id::new(format!("xui-log-{}", "a".repeat(64))).unwrap(),
        observed_at: chrono::DateTime::from_timestamp(1000, 0).unwrap(),
        reason_key: garde_contracts::Id::new("xui_client_activity").unwrap(),
        evidence: vec![garde_contracts::Evidence {
            name: garde_contracts::Id::new("xui_target").unwrap(),
            value: target.clone(),
        }],
    };
    monitor
        .observe_logs(
            Ok(garde_contracts::XuiLogBatch {
                checkpoint: Default::default(),
                sources: vec![garde_contracts::XuiSourceHealth {
                    source: garde_contracts::Id::new("xui.journal").unwrap(),
                    reason_key: None,
                    gap: false,
                }],
                records: vec![record],
            }),
            1000,
        )
        .await
        .unwrap();
    let reply = db.call(|s| s.claim(1000)).await.unwrap().unwrap();
    assert!(reply.reply.text.contains(&target));
    assert!(reply.reply.text.encode_utf16().count() <= 4096);
}
#[tokio::test]
async fn a_supported_client_set_and_panel_changes_commit_together_without_losing_progress() {
    let d = tempfile::tempdir().unwrap();
    fs::set_permissions(d.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config:Config=serde_json::from_value(serde_json::json!({"server_id":"server-demo","owner_user_id":"123456789","bot_token_file":d.path().join("absent"),"state_dir":d.path(),"default_locale":"en","xui":{"enabled":true}})).unwrap();
    let db = Database::open(config.clone()).await.unwrap();
    let mut monitor = Monitor::new(config.clone(), db.clone());
    let clients = (0..1024)
        .map(|i| garde_contracts::XuiClient {
            key: format!("{i:064x}"),
            label: format!("client-{i}"),
            inbound_id: 7,
            enabled: true,
            last_online: Some(1),
            traffic_bytes: 30,
        })
        .collect();
    let panel = (0..160)
        .map(|i| garde_contracts::XuiPanelRow {
            key: format!("clients:{i}"),
            fields: std::collections::BTreeMap::from([("enable".into(), "0".repeat(64))]),
        })
        .collect();
    let mut snapshot = garde_contracts::XuiSnapshot {
        source_id: "a".repeat(64),
        version: garde_contracts::WireVersion::V1,
        observed_at: chrono::DateTime::from_timestamp(1000, 0).unwrap(),
        schema: Some(garde_contracts::Id::new("xui.normalized").unwrap()),
        panel: garde_contracts::Measurement::Available { value: panel },
        clients: garde_contracts::Measurement::Available { value: clients },
        connectivity_available: true,
    };
    monitor.observe(Ok(snapshot.clone()), 1000).await.unwrap();
    if let garde_contracts::Measurement::Available { value } = &mut snapshot.clients {
        for c in value {
            c.last_online = Some(1000)
        }
    }
    if let garde_contracts::Measurement::Available { value } = &mut snapshot.panel {
        for r in value {
            r.fields.insert("enable".into(), "1".repeat(64));
        }
    }
    monitor.observe(Ok(snapshot.clone()), 1000).await.unwrap();
    let pending = db.call(|s| s.health(1000)).await.unwrap().pending;
    assert!(pending > 1024);
    let health = db.call(move |s| s.xui_health(&config, 1000)).await.unwrap();
    assert_eq!(health.clients.len(), 1024);
    assert!(
        health
            .clients
            .iter()
            .all(|v| v.state == garde_contracts::XuiConnectionState::Online)
    );
    monitor.observe(Ok(snapshot), 1001).await.unwrap();
    assert_eq!(db.call(|s| s.health(1001)).await.unwrap().pending, pending);
}
