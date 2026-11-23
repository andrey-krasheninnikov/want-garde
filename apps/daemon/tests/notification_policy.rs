use garde_config::Config;
use garde_contracts::{Event, Id, Severity};
use garde_daemon::{Reply, delivery::Outcome, runtime::Database};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};

fn at(time: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(time)
        .unwrap()
        .timestamp()
}

fn config(dir: &Path, policy: Value) -> Config {
    serde_json::from_value(json!({
        "server_id":"server-demo", "bot_token_file":dir.join("absent"), "state_dir":dir,
        "owner_user_id":"123456789", "default_locale":"en",
        "notifications":{"private":[{"chat_id":"123456788"}]},
        "notification_policy":policy
    }))
    .unwrap()
}

fn event(id: &str, severity: Severity) -> Event {
    let mut event: Event =
        serde_json::from_str(include_str!("../../../fixtures/contracts/event.json")).unwrap();
    event.id = Id::new(id).unwrap();
    event.classification.severity = severity;
    event
}

fn reply(chat: &str) -> Reply {
    Reply {
        locale: Some("en".into()),
        chat_id: chat.into(),
        text: "event notification".into(),
        envelope: Default::default(),
    }
}

async fn open(config: Config) -> Database {
    tokio::time::timeout(Duration::from_secs(2), async move {
        loop {
            match Database::open(config.clone()).await {
                Ok(db) => return db,
                Err("storage.locked") => tokio::task::yield_now().await,
                Err(error) => panic!("{error}"),
            }
        }
    })
    .await
    .unwrap()
}

async fn claim(database: &Database, now: i64) -> Vec<Reply> {
    database
        .call(move |store| {
            let mut replies = Vec::new();
            while let Some(delivery) = store.claim(now)? {
                store.finish(delivery.id, Outcome::Delivered, now)?;
                replies.push(delivery.reply);
            }
            Ok(replies)
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn quiet_hours_critical_and_dedup_are_independent_per_recipient_and_survive_restart() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let config = config(
        dir.path(),
        json!({
            "default_preferences":{"quiet_hours":["22:00-06:00"],"time_zone":"Etc/UTC"},
            "recipients":{"123456788":{"critical_bypasses_quiet":false,"min_severity":"warning"}}
        }),
    );
    let now = at("2026-01-05T23:00:00Z");
    let db = open(config.clone()).await;
    db.call(move |s| {
        s.publish(
            &event("normal-1", Severity::Warning),
            &[reply("123456789"), reply("123456788")],
            now,
        )
    })
    .await
    .unwrap();
    let replies = claim(&db, now).await;
    assert_eq!(
        replies
            .iter()
            .map(|r| r.chat_id.as_str())
            .collect::<Vec<_>>(),
        ["123456788"]
    );
    drop(db);
    let db = open(config.clone()).await;
    db.call(move |s| {
        s.publish(
            &event("normal-2", Severity::Warning),
            &[reply("123456789"), reply("123456788")],
            now + 30,
        )
    })
    .await
    .unwrap();
    assert!(claim(&db, now + 30).await.is_empty());
    db.call(move |s| {
        s.publish(
            &event("critical-1", Severity::Critical),
            &[reply("123456789"), reply("123456788")],
            now + 31,
        )
    })
    .await
    .unwrap();
    let replies = claim(&db, now + 31).await;
    assert_eq!(replies.len(), 2);
    let policy = config.notification_policy.unwrap();
    assert!(
        policy
            .default_preferences
            .quiet(chrono::DateTime::from_timestamp(at("2026-01-06T05:59:59Z"), 0).unwrap())
    );
    assert!(
        !policy
            .default_preferences
            .quiet(chrono::DateTime::from_timestamp(at("2026-01-06T06:00:00Z"), 0).unwrap())
    );
}

#[tokio::test]
async fn daily_and_live_summaries_enqueued_once_across_restart_on_a_dst_gap() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let config = config(
        dir.path(),
        json!({
             "default_rule":{"immediate":false,"aggregate":true},
             "default_preferences":{"time_zone":"America/New_York","daily":{"at":"02:30"},"live":{"minimum_events":2}},
             "recipients":{"123456788":{}}
        }),
    );
    let observed = at("2026-03-08T06:51:00Z");
    let due = at("2026-03-08T07:00:00Z");
    let db = open(config.clone()).await;
    db.call(move |s| {
        s.publish(
            &event("counted-a", Severity::Warning),
            &[reply("123456789")],
            observed,
        )?;
        s.publish(
            &event("counted-b", Severity::Warning),
            &[reply("123456789")],
            observed + 60,
        )?;
        s.tick_notifications(observed + 61)?;
        Ok(())
    })
    .await
    .unwrap();
    assert!(claim(&db, observed + 61).await.is_empty());
    db.call(move |s| s.tick_notifications(due)).await.unwrap();
    drop(db);
    let db = open(config).await;
    db.call(move |s| s.tick_notifications(due + 1))
        .await
        .unwrap();
    let messages = claim(&db, due + 1).await;
    assert_eq!(messages.len(), 2);
    assert!(
        messages
            .iter()
            .all(|message| message.text.contains("Events: 2"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.text.starts_with("Daily summary"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.text.starts_with("Live summary"))
    );
    db.call(move |s| s.tick_notifications(due + 60))
        .await
        .unwrap();
    assert!(claim(&db, due + 60).await.is_empty());
}

#[tokio::test]
async fn disabled_alerts_and_delivery_severity_do_not_rewrite_event_history() {
    use garde_daemon::history::{self, Output, Query};
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let config = config(
        dir.path(),
        json!({
            "default_rule":{"enabled":false},
            "default_preferences":{"min_severity":"high"},
            "rules":{"special.rule":{"severity":"critical"}}
        }),
    );
    let db = open(config.clone()).await;
    let now = at("2026-01-05T12:00:00Z");
    db.call(move |s| {
        s.publish(
            &event("suppressed-id", Severity::Warning),
            &[reply("123456789")],
            now,
        )?;
        let mut prioritized = event("priority-id", Severity::Warning);
        prioritized.classification.rule_id = Id::new("special.rule")?;
        s.publish(&prioritized, &[reply("123456789")], now)?;
        Ok(())
    })
    .await
    .unwrap();
    let delivered = claim(&db, now).await;
    assert_eq!(delivered.len(), 1);
    assert!(delivered[0].text.contains("priority-id"));
    for id in ["priority-id", "suppressed-id"] {
        let Output::Event(event) =
            history::inspect(&config, Query::Event(Id::new(id).unwrap()), now).unwrap()
        else {
            panic!("expected event");
        };
        assert_eq!(event.severity, Severity::Warning);
    }
}
