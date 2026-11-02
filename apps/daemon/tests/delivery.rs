use garde_contracts::Event;
use garde_daemon::{
    Reply,
    delivery::{Outcome, Store},
};
use std::{fs, os::unix::fs::PermissionsExt};

fn private_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    dir
}

fn event() -> Event {
    serde_json::from_str(include_str!("../../../fixtures/contracts/event.json")).unwrap()
}

#[test]
fn expired_events_are_not_sent_after_the_delivery_deadline() {
    let dir = private_dir();
    let event = event();
    let mut store = Store::open(dir.path(), event.server_id.as_str(), "123456789").unwrap();
    store
        .publish(
            &event,
            &[Reply {
                chat_id: "123456789".into(),
                text: "expired alert".into(),
            }],
            1000,
        )
        .unwrap();
    assert!(store.claim(87400).unwrap().is_none());
    assert_eq!(store.health(87400).unwrap().pending, 0);
}

#[test]
fn a_second_writer_cannot_recover_an_active_sending_attempt() {
    let dir = private_dir();
    let event = event();
    let mut store = Store::open(dir.path(), event.server_id.as_str(), "123456789").unwrap();
    store
        .publish(
            &event,
            &[Reply {
                chat_id: "123456789".into(),
                text: "active alert".into(),
            }],
            1000,
        )
        .unwrap();
    store.claim(1000).unwrap().unwrap();
    assert!(Store::open(dir.path(), event.server_id.as_str(), "123456789").is_err());
}

#[test]
fn command_replies_and_polling_progress_commit_together_and_expire_after_ten_minutes() {
    let dir = private_dir();
    let mut store = Store::open(dir.path(), "server-demo", "123456789").unwrap();
    let reply = || {
        Some(Reply {
            chat_id: "123456789".into(),
            text: "saved status snapshot".into(),
        })
    };
    assert_eq!(store.consume_update(42, reply(), 1000).unwrap(), 43);
    assert_eq!(store.consume_update(42, reply(), 1001).unwrap(), 43);
    drop(store);
    let mut store = Store::open(dir.path(), "server-demo", "123456789").unwrap();
    assert_eq!(store.offset().unwrap(), 43);
    assert!(store.claim(1600).unwrap().is_none());
    assert_eq!(store.health(1600).unwrap().pending, 0);
}

#[test]
fn retries_back_off_and_keep_the_original_delivery_identifier() {
    let dir = private_dir();
    let mut store = Store::open(dir.path(), "server-demo", "123456789").unwrap();
    store
        .consume_update(
            1,
            Some(Reply {
                chat_id: "123456789".into(),
                text: "retry result".into(),
            }),
            1000,
        )
        .unwrap();
    let first = store.claim(1000).unwrap().unwrap();
    store
        .finish(
            first.id,
            Outcome::Retry {
                code: "telegram.network",
                retry_after: 0,
                uncertain: true,
            },
            1000,
        )
        .unwrap();
    let second = store.claim(1005).unwrap().unwrap();
    store
        .finish(
            second.id,
            Outcome::Retry {
                code: "telegram.network",
                retry_after: 0,
                uncertain: true,
            },
            1005,
        )
        .unwrap();
    assert!(store.claim(1014).unwrap().is_none());
    let third = store.claim(1015).unwrap().unwrap();
    assert_eq!(first.reply.text, third.reply.text);
}

#[test]
fn ambiguous_attempts_and_permanent_failures_remain_diagnosable_without_message_content() {
    let dir = private_dir();
    let event = event();
    let mut store = Store::open(dir.path(), event.server_id.as_str(), "123456789").unwrap();
    store
        .publish(
            &event,
            &[Reply {
                chat_id: "123456789".into(),
                text: "private_result_do_not_disclose".into(),
            }],
            1000,
        )
        .unwrap();
    let attempt = store.claim(1000).unwrap().unwrap();
    drop(store);
    let mut store = Store::open(dir.path(), event.server_id.as_str(), "123456789").unwrap();
    let health = store.health(1000).unwrap();
    assert!(health.problems[0].uncertain);
    let retried = store.claim(1005).unwrap().unwrap();
    assert_eq!(retried.id, attempt.id);
    store
        .finish(
            retried.id,
            Outcome::Permanent {
                code: "telegram.forbidden",
            },
            1005,
        )
        .unwrap();
    let health = store.health(1005).unwrap();
    assert_eq!(health.dead_letter, 1);
    assert_eq!(health.problems[0].reason_key.as_str(), "telegram.forbidden");
    let json = serde_json::to_string(&health).unwrap();
    assert!(!json.contains("private_result_do_not_disclose"));
    assert!(!json.contains("123456789"));
}

#[test]
fn unbounded_or_wrong_installation_events_are_rejected_before_delivery() {
    let dir = private_dir();
    let mut store = Store::open(dir.path(), "another-server", "123456789").unwrap();
    assert!(store.publish(&event(), &[], 1000).is_err());
    let mut event = event();
    event.server_id = garde_contracts::Id::new("another-server").unwrap();
    event.evidence[0].value = "x".repeat(65536);
    assert!(store.publish(&event, &[], 1000).is_err());
}

#[test]
fn local_inspection_does_not_recover_or_modify_an_active_attempt() {
    let dir = private_dir();
    let mut store = Store::open(dir.path(), "server-demo", "123456789").unwrap();
    store
        .consume_update(
            1,
            Some(Reply {
                chat_id: "123456789".into(),
                text: "active reply".into(),
            }),
            1000,
        )
        .unwrap();
    let attempt = store.claim(1000).unwrap().unwrap();
    let before = fs::read(dir.path().join("garde.sqlite3")).unwrap();
    let health = Store::inspect(dir.path(), "server-demo", "123456789", 1001).unwrap();
    assert_eq!((health.pending, health.in_flight), (0, 1));
    assert_eq!(before, fs::read(dir.path().join("garde.sqlite3")).unwrap());
    store.finish(attempt.id, Outcome::Delivered, 1001).unwrap();
    assert_eq!(store.health(1001).unwrap().delivered, 1);
}

#[test]
fn unsafe_database_files_and_future_schemas_are_rejected_without_replacement() {
    let dir = private_dir();
    drop(Store::open(dir.path(), "server-demo", "123456789").unwrap());
    let path = dir.path().join("garde.sqlite3");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(Store::open(dir.path(), "server-demo", "123456789").is_err());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.pragma_update(None, "user_version", 2).unwrap();
    drop(connection);
    let before = fs::read(&path).unwrap();
    assert!(matches!(
        Store::open(dir.path(), "server-demo", "123456789"),
        Err("storage.version")
    ));
    assert_eq!(before, fs::read(&path).unwrap());
    fs::rename(&path, dir.path().join("future.sqlite3")).unwrap();
    std::os::unix::fs::symlink(dir.path().join("future.sqlite3"), &path).unwrap();
    assert!(Store::open(dir.path(), "server-demo", "123456789").is_err());
    assert_eq!(before, fs::read(dir.path().join("future.sqlite3")).unwrap());
}

#[test]
fn invalid_replies_do_not_advance_the_polling_offset() {
    let dir = private_dir();
    let mut store = Store::open(dir.path(), "server-demo", "123456789").unwrap();
    assert!(
        store
            .consume_update(
                42,
                Some(Reply {
                    chat_id: "123456789".into(),
                    text: "x".repeat(4000)
                }),
                1000
            )
            .is_err()
    );
    assert_eq!(store.offset().unwrap(), 0);
    assert_eq!(store.health(1000).unwrap().pending, 0);
    assert_eq!(store.consume_update(42, None, 1000).unwrap(), 43);
}

#[test]
fn retry_limits_still_apply_when_the_clock_moves_backwards() {
    let dir = private_dir();
    let event = event();
    let mut store = Store::open(dir.path(), event.server_id.as_str(), "123456789").unwrap();
    store
        .publish(
            &event,
            &[Reply {
                chat_id: "123456789".into(),
                text: "bounded retries".into(),
            }],
            1000,
        )
        .unwrap();
    for _ in 0..128 {
        let attempt = store.claim(2000).unwrap().unwrap();
        store
            .finish(
                attempt.id,
                Outcome::Retry {
                    code: "telegram.network",
                    retry_after: 0,
                    uncertain: true,
                },
                1000,
            )
            .unwrap();
    }
    assert!(store.claim(2000).unwrap().is_none());
    let health = store.health(2000).unwrap();
    assert_eq!(health.dead_letter, 1);
    assert_eq!(health.problems[0].reason_key.as_str(), "delivery.exhausted");
}

#[test]
fn retention_removes_completed_records_and_preserves_unfinished_delivery() {
    let dir = private_dir();
    let event = event();
    let mut store = Store::open(dir.path(), event.server_id.as_str(), "123456789").unwrap();
    store
        .publish(
            &event,
            &[
                Reply {
                    chat_id: "123456789".into(),
                    text: "completed".into(),
                },
                Reply {
                    chat_id: "987654321".into(),
                    text: "unfinished".into(),
                },
            ],
            1000,
        )
        .unwrap();
    let first = store.claim(1000).unwrap().unwrap();
    store.finish(first.id, Outcome::Delivered, 1000).unwrap();
    store
        .consume_update(
            42,
            Some(Reply {
                chat_id: "123456789".into(),
                text: "new reply".into(),
            }),
            1000 + 31 * 86400,
        )
        .unwrap();
    let health = store.health(1000 + 31 * 86400).unwrap();
    assert_eq!((health.pending, health.delivered), (2, 0));
    assert_eq!(health.pruned_records, 1);
}

#[test]
fn changing_the_bot_identity_requires_explicit_state_separation() {
    let dir = private_dir();
    let mut store = Store::open(dir.path(), "server-demo", "123456789").unwrap();
    store.bind_bot("12345").unwrap();
    store.bind_bot("12345").unwrap();
    assert_eq!(store.bind_bot("54321"), Err("storage.identity"));
}

#[test]
fn rate_limit_cooldown_survives_restart_and_expiration_of_the_original_reply() {
    let dir = private_dir();
    let mut store = Store::open(dir.path(), "server-demo", "123456789").unwrap();
    store
        .consume_update(
            1,
            Some(Reply {
                chat_id: "123456789".into(),
                text: "rate limited".into(),
            }),
            1000,
        )
        .unwrap();
    let first = store.claim(1000).unwrap().unwrap();
    store
        .finish(
            first.id,
            Outcome::Retry {
                code: "telegram.rate_limited",
                retry_after: 7200,
                uncertain: false,
            },
            1000,
        )
        .unwrap();
    store
        .consume_update(
            2,
            Some(Reply {
                chat_id: "123456789".into(),
                text: "later reply".into(),
            }),
            1500,
        )
        .unwrap();
    drop(store);
    let mut store = Store::open(dir.path(), "server-demo", "123456789").unwrap();
    assert!(store.claim(1600).unwrap().is_none());
    assert_eq!(store.health(1600).unwrap().pending, 1);
}

#[test]
fn missing_tables_in_a_known_schema_are_not_silently_recreated() {
    let dir = private_dir();
    drop(Store::open(dir.path(), "server-demo", "123456789").unwrap());
    let path = dir.path().join("garde.sqlite3");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute("DROP TABLE deliveries", []).unwrap();
    drop(connection);
    let before = fs::read(&path).unwrap();
    assert!(Store::open(dir.path(), "server-demo", "123456789").is_err());
    assert_eq!(before, fs::read(&path).unwrap());
}

#[test]
fn inactive_polling_cursor_accepts_a_lower_new_sequence_after_restart() {
    let dir = private_dir();
    let mut store = Store::open(dir.path(), "server-demo", "123456789").unwrap();
    store
        .consume_update(
            900000000,
            Some(Reply {
                chat_id: "123456789".into(),
                text: "old result".into(),
            }),
            1000,
        )
        .unwrap();
    drop(store);
    let mut store = Store::open(dir.path(), "server-demo", "123456789").unwrap();
    assert_eq!(store.poll_offset(1000 + 2 * 86400 - 1).unwrap(), 900000001);
    assert_eq!(store.poll_offset(1000 + 2 * 86400).unwrap(), 0);
    assert_eq!(store.health(1000 + 2 * 86400).unwrap().pending, 1);
    let reply = || {
        Some(Reply {
            chat_id: "123456789".into(),
            text: "new result".into(),
        })
    };
    assert_eq!(
        store.consume_update(42, reply(), 1000 + 7 * 86400).unwrap(),
        43
    );
    assert_eq!(
        store.consume_update(42, reply(), 1000 + 7 * 86400).unwrap(),
        43
    );
    assert_eq!(store.health(1000 + 7 * 86400).unwrap().pending, 2);
}

#[test]
fn a_new_sequence_can_reuse_an_update_id_without_reusing_an_old_delivery() {
    let dir = private_dir();
    let mut store = Store::open(dir.path(), "server-demo", "123456789").unwrap();
    store
        .consume_update(
            42,
            Some(Reply {
                chat_id: "123456789".into(),
                text: "original intent".into(),
            }),
            1000,
        )
        .unwrap();
    store.poll_offset(1000 + 2 * 86400).unwrap();
    store
        .consume_update(
            42,
            Some(Reply {
                chat_id: "123456789".into(),
                text: "new intent".into(),
            }),
            1000 + 2 * 86400,
        )
        .unwrap();
    assert_eq!(store.health(1000 + 2 * 86400).unwrap().pending, 2);
    let next = store.claim(1000 + 2 * 86400).unwrap().unwrap();
    assert!(next.reply.text.starts_with("new intent"));
    assert!(next.reply.text.contains("ID: d-2"));
}

#[test]
fn an_event_reaches_independent_recipients_and_pending_delivery_survives_restart() {
    let dir = private_dir();
    let event = event();
    let mut store = Store::open(dir.path(), event.server_id.as_str(), "123456789").unwrap();
    let replies = [
        Reply {
            chat_id: "123456789".into(),
            text: "first alert".into(),
        },
        Reply {
            chat_id: "987654321".into(),
            text: "second alert".into(),
        },
    ];
    store.publish(&event, &replies, 1000).unwrap();
    store.publish(&event, &replies, 1000).unwrap();
    let first = store.claim(1000).unwrap().unwrap();
    let second = store.claim(1000).unwrap().unwrap();
    assert_ne!(first.reply.chat_id, second.reply.chat_id);
    assert!(store.claim(1000).unwrap().is_none());
    store
        .finish(
            first.id,
            Outcome::Retry {
                code: "telegram.network",
                retry_after: 0,
                uncertain: true,
            },
            1000,
        )
        .unwrap();
    store.finish(second.id, Outcome::Delivered, 1000).unwrap();
    let health = store.health(1000).unwrap();
    assert_eq!((health.pending, health.delivered), (1, 1));
    let saved = first.reply.text;
    drop(store);
    let mut store = Store::open(dir.path(), event.server_id.as_str(), "123456789").unwrap();
    assert!(store.claim(1004).unwrap().is_none());
    let retried = store.claim(1005).unwrap().unwrap();
    assert_eq!(retried.reply.text, saved);
    assert_eq!(retried.id, first.id);
    store.finish(retried.id, Outcome::Delivered, 1005).unwrap();
    assert_eq!(store.health(1005).unwrap().delivered, 2);
}
