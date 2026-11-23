use garde_config::Config;
use garde_contracts::{Event, Id, Severity};
use garde_daemon::{Reply, delivery::Outcome, runtime::Database};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};

fn private_dir() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    dir
}

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
            "default_preferences":{"quiet_hours":["22:00-06:00"],"time_zone":"Etc/UTC","critical_bypasses_quiet":true},
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

#[tokio::test]
async fn overlapping_windows_after_config_change_each_produce_a_summary() {
    let dir = private_dir();
    let settings = |seconds| {
        config(
            dir.path(),
            json!({
                "default_rule":{"immediate":false,"aggregate":true},
                "default_preferences":{"live":{"window_seconds":seconds,"minimum_events":1}}
            }),
        )
    };
    let start = at("2026-01-05T12:00:00Z");
    let db = open(settings(120)).await;
    db.call(move |s| {
        s.publish(
            &event("before-change", Severity::Warning),
            &[reply("123456789")],
            start + 10,
        )
    })
    .await
    .unwrap();
    drop(db);
    let db = open(settings(60)).await;
    db.call(move |s| {
        s.publish(
            &event("after-change", Severity::Warning),
            &[reply("123456789")],
            start + 70,
        )?;
        s.tick_notifications(start + 120)?;
        s.tick_notifications(start + 121)
    })
    .await
    .unwrap();
    assert_eq!(claim(&db, start + 121).await.len(), 2);
}

#[tokio::test]
async fn live_buckets_older_than_retention_are_not_delivered_after_restart() {
    let dir = private_dir();
    let enabled = config(
        dir.path(),
        json!({
            "default_rule":{"immediate":false,"aggregate":true},
            "default_preferences":{"live":{"minimum_events":1}}
        }),
    );
    let start = at("2026-01-05T12:00:00Z");
    let db = open(enabled.clone()).await;
    db.call(move |s| {
        s.publish(
            &event("before-removal", Severity::Warning),
            &[reply("123456789")],
            start,
        )
    })
    .await
    .unwrap();
    drop(db);
    let now = start + (i64::from(enabled.retention.event_days) + 1) * 86400;
    let db = open(enabled).await;
    db.call(move |s| s.tick_notifications(now)).await.unwrap();
    assert!(claim(&db, now).await.is_empty());
}

#[tokio::test]
async fn queued_events_respect_quiet_hours_at_claim_while_critical_bypass_is_explicit() {
    let dir = private_dir();
    let config = config(
        dir.path(),
        json!({
            "default_preferences":{"quiet_hours":["22:00-06:00"],"critical_bypasses_quiet":true},
            "recipients":{"123456788":{"quiet_hours":["22:00-06:00"]}}
        }),
    );
    let db = open(config.clone()).await;
    let now = at("2026-01-05T21:59:59Z");
    db.call(move |s| {
        let recipients = [reply("123456789"), reply("123456788")];
        s.publish(
            &event("pre-quiet-ordinary", Severity::Warning),
            &recipients,
            now,
        )?;
        s.publish(
            &event("pre-quiet-critical", Severity::Critical),
            &recipients,
            now,
        )?;
        Ok(())
    })
    .await
    .unwrap();
    drop(db);
    let db = open(config).await;
    let claimed = claim(&db, now + 1).await;
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].chat_id, "123456789");
    assert!(claimed[0].text.contains("pre-quiet-critical"));
    assert!(claim(&db, at("2026-01-06T05:59:59Z")).await.is_empty());
    assert_eq!(claim(&db, at("2026-01-06T06:00:00Z")).await.len(), 3);
}

#[tokio::test]
async fn daily_boundary_event_appears_in_next_summary_and_summaries_do_not_count_as_events() {
    use garde_daemon::history::{self, Output, Query};
    let dir = private_dir();
    let config = config(
        dir.path(),
        json!({
            "default_rule":{"immediate":false},
            "default_preferences":{"daily":{"at":"14:00"}}
        }),
    );
    let db = open(config.clone()).await;
    let due = at("2026-01-05T14:00:00Z");
    db.call(move |s| {
        s.tick_notifications(due)?;
        s.publish(
            &event("daily-boundary", Severity::Warning),
            &[reply("123456789")],
            due,
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let first_day = claim(&db, due).await;
    assert_eq!(first_day.len(), 2);
    assert!(first_day.iter().all(|r| r.text.contains("Events: 0")));
    let next = due + 86400;
    db.call(move |s| s.tick_notifications(next)).await.unwrap();
    let next_day = claim(&db, next).await;
    assert_eq!(next_day.len(), 2);
    assert!(next_day.iter().all(|r| r.text.contains("Events: 1")));
    let Output::Stats(stats) = history::inspect(&config, Query::Stats(24), next).unwrap() else {
        panic!("statistics expected");
    };
    assert_eq!(stats.total, 1);
    let Output::Recent(recent) = history::inspect(
        &config,
        Query::Recent {
            limit: 10,
            before: None,
        },
        next,
    )
    .unwrap() else {
        panic!("recent events expected");
    };
    assert_eq!(recent.events.len(), 1);
    assert_eq!(recent.events[0].id.as_str(), "daily-boundary");
}

#[tokio::test]
async fn summary_retries_after_queue_quota_and_restart_without_losing_its_place() {
    let dir = private_dir();
    let mut config = config(
        dir.path(),
        json!({
            "default_preferences":{"daily":{"at":"14:00"}},
            "recipients":{"123456788":{}}
        }),
    );
    config.retention.pending_deliveries = 2 * garde_config::MAX_OBSERVATION_BATCH as u32;
    let db = open(config.clone()).await;
    let due = at("2026-01-05T14:00:00Z");
    let limit = i64::from(config.retention.pending_deliveries);
    db.call(move |s| {
        for id in 0..limit {
            s.consume_update(id, Some(reply("123456789")), due - 1)?;
        }
        assert_eq!(s.tick_notifications(due), Err("storage.quota"));
        assert_eq!(s.health(due)?.pending, limit as u32);
        let first = s.claim(due)?.expect("queued reply");
        s.finish(first.id, Outcome::Delivered, due)
    })
    .await
    .unwrap();
    drop(db);
    let db = open(config).await;
    db.call(move |s| s.tick_notifications(due + 1))
        .await
        .unwrap();
    let queued = claim(&db, due + 1).await;
    assert_eq!(
        queued
            .iter()
            .filter(|r| r.text.starts_with("Daily summary"))
            .count(),
        1
    );
    db.call(move |s| s.tick_notifications(due + 2))
        .await
        .unwrap();
    assert!(claim(&db, due + 2).await.is_empty());
}

#[tokio::test]
async fn an_empty_daily_summary_queued_during_quiet_hours_survives_midnight() {
    let dir = private_dir();
    let config = config(
        dir.path(),
        json!({
            "default_preferences":{"daily":{"at":"23:30"},"quiet_hours":["22:00-07:00"]},
            "recipients":{"123456788":{}}
        }),
    );
    let db = open(config.clone()).await;
    let due = at("2026-01-05T23:30:00Z");
    db.call(move |s| s.tick_notifications(due)).await.unwrap();
    assert!(claim(&db, due).await.is_empty());
    drop(db);
    let db = open(config).await;
    let release = at("2026-01-06T07:00:00Z");
    db.call(move |s| s.tick_notifications(release))
        .await
        .unwrap();
    let summaries = claim(&db, release).await;
    assert_eq!(summaries.len(), 1);
    assert!(summaries[0].text.contains("Events: 0"));
    assert!(summaries[0].text.contains("2026-01-05T23:30:00"));
}

#[tokio::test]
async fn a_missed_daily_inside_elapsed_retention_survives_a_spring_clock_change() {
    let dir = private_dir();
    let mut config = config(
        dir.path(),
        json!({
            "default_preferences":{"daily":{"at":"23:30"},"time_zone":"America/New_York"},
            "recipients":{"123456788":{}}
        }),
    );
    config.retention.event_days = 1;
    let db = open(config.clone()).await;
    let before = at("2026-03-08T04:00:00Z");
    db.call(move |s| s.tick_notifications(before))
        .await
        .unwrap();
    drop(db);
    let db = open(config).await;
    let recovered = at("2026-03-09T04:15:00Z");
    db.call(move |s| s.tick_notifications(recovered))
        .await
        .unwrap();
    let summaries = claim(&db, recovered).await;
    assert_eq!(summaries.len(), 1);
    assert!(summaries[0].text.contains("2026-03-08T04:30:00"));
}
