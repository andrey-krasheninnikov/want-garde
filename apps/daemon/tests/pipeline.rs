use garde_config::Config;
use garde_contracts::{Event, Id};
use garde_daemon::{
    Bot, Reply,
    runtime::{Database, Identity, Transport, send_loop},
    telegram::TransportError,
};
use serde_json::Value;
use serde_json::json;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Default)]
struct Telegram {
    messages: Mutex<Vec<String>>,
}
impl Transport for Telegram {
    async fn identity(&self) -> Result<Identity, TransportError> {
        Ok(Identity {
            id: "12345".into(),
            username: "test_bot".into(),
        })
    }
    async fn updates(&self, _: i64) -> Result<Vec<Value>, TransportError> {
        std::future::pending().await
    }
    async fn send(&self, reply: &Reply) -> Result<(), TransportError> {
        self.messages.lock().unwrap().push(reply.text.clone());
        Ok(())
    }
}

#[tokio::test]
async fn an_event_passes_real_policy_and_sqlite_before_transport_delivery() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config = Config {
        server_id: Id::new("server-demo").unwrap(),
        owner_user_id: Some("123456789".into()),
        bot_token_file: dir.path().join("token"),
        state_dir: dir.path().into(),
        default_locale: "en".into(),
    };
    let bot = Bot::new(config.clone()).unwrap();
    let database = Database::open(config).await.unwrap();
    let event: Event =
        serde_json::from_str(include_str!("../../../fixtures/contracts/event.json")).unwrap();
    bot.publish_event(&database, event).await.unwrap();
    assert_eq!(database.call(|s| s.health(1000)).await.unwrap().pending, 1);
    let transport = Arc::new(Telegram::default());
    let sender = tokio::spawn(send_loop(database.clone(), transport.clone()));
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if database.call(|s| s.health(1000)).await.unwrap().delivered == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    sender.abort();
    let _ = sender.await;
    let messages = transport.messages.lock().unwrap();
    assert_eq!(messages.len(), 1);
    assert!(messages[0].contains("event-001"));
    assert!(messages[0].contains("ID: d-1"));
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
async fn delivery_diagnostics_enforce_owner_access_and_all_locales_survive_queued_replies() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config = Config {
        server_id: Id::new("server-demo").unwrap(),
        owner_user_id: Some("123456789".into()),
        bot_token_file: dir.path().join("token"),
        state_dir: dir.path().into(),
        default_locale: "en".into(),
    };
    let mut bot = Bot::new(config.clone()).unwrap();
    let database = Database::open(config.clone()).await.unwrap();
    for (id, user, chat) in [(0, 987654321, "private"), (1, 123456789, "group")] {
        let update = json!({"update_id":id,"message":{"from":{"id":user,"is_bot":false},"chat":{"id":user,"type":chat},"text":"/delivery"}});
        bot.process_update(&database, &update, &Host, "test_bot")
            .await
            .unwrap();
    }
    assert_eq!(database.call(|s| s.health(1000)).await.unwrap().pending, 0);
    let update = |id, text: String| json!({"update_id":id,"message":{"from":{"id":123456789,"is_bot":false},"chat":{"id":123456789,"type":"private"},"text":text}});
    let mut id = 2;
    for (locale, copy) in garde_entrypoint::catalog()["locales"].as_object().unwrap() {
        bot.process_update(
            &database,
            &update(id, format!("/language {locale}")),
            &Host,
            "test_bot",
        )
        .await
        .unwrap();
        let reply = database
            .call(|s| s.claim(garde_daemon::runtime::now()))
            .await
            .unwrap()
            .unwrap();
        assert!(
            reply
                .reply
                .text
                .starts_with(copy["language_saved"].as_str().unwrap())
        );
        database
            .call(move |s| {
                s.finish(
                    reply.id,
                    garde_daemon::delivery::Outcome::Delivered,
                    garde_daemon::runtime::now(),
                )
            })
            .await
            .unwrap();
        id += 1;
        let mut restarted = Bot::new(config.clone()).unwrap();
        restarted
            .process_update(
                &database,
                &update(id, "/delivery".into()),
                &Host,
                "test_bot",
            )
            .await
            .unwrap();
        restarted
            .process_update(
                &database,
                &update(id, "/language en".into()),
                &Host,
                "test_bot",
            )
            .await
            .unwrap();
        let reply = database
            .call(|s| s.claim(garde_daemon::runtime::now()))
            .await
            .unwrap()
            .unwrap();
        assert!(
            reply
                .reply
                .text
                .contains(copy["delivery_title"].as_str().unwrap())
        );
        assert!(!reply.reply.text.contains("message.unavailable"));
        assert!(reply.reply.text.encode_utf16().count() <= 4096);
        if ["ar", "he"].contains(&locale.as_str()) {
            assert!(reply.reply.text.contains('\u{2068}'));
        }
        database
            .call(move |s| {
                s.finish(
                    reply.id,
                    garde_daemon::delivery::Outcome::Delivered,
                    garde_daemon::runtime::now(),
                )
            })
            .await
            .unwrap();
        let preferences: Value =
            serde_json::from_slice(&fs::read(dir.path().join("owner-preferences.json")).unwrap())
                .unwrap();
        assert_eq!(preferences["locale"], locale.as_str());
        id += 1;
    }
}

struct SlowRecipient;
impl Transport for SlowRecipient {
    async fn identity(&self) -> Result<Identity, TransportError> {
        Ok(Identity {
            id: "12345".into(),
            username: "test_bot".into(),
        })
    }
    async fn updates(&self, _: i64) -> Result<Vec<Value>, TransportError> {
        std::future::pending().await
    }
    async fn send(&self, reply: &Reply) -> Result<(), TransportError> {
        if reply.chat_id == "123456789" {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        Ok(())
    }
}

#[tokio::test]
async fn a_slow_recipient_does_not_hold_delivery_to_another_recipient() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config = Config {
        server_id: Id::new("server-demo").unwrap(),
        owner_user_id: Some("123456789".into()),
        bot_token_file: dir.path().join("token"),
        state_dir: dir.path().into(),
        default_locale: "en".into(),
    };
    let database = Database::open(config).await.unwrap();
    let event: Event =
        serde_json::from_str(include_str!("../../../fixtures/contracts/event.json")).unwrap();
    database
        .call(move |store| {
            store.publish(
                &event,
                &[
                    Reply {
                        chat_id: "123456789".into(),
                        text: "slow".into(),
                    },
                    Reply {
                        chat_id: "987654321".into(),
                        text: "fast".into(),
                    },
                ],
                garde_daemon::runtime::now(),
            )
        })
        .await
        .unwrap();
    let sender = tokio::spawn(send_loop(database.clone(), Arc::new(SlowRecipient)));
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let health = database
                .call(|s| s.health(garde_daemon::runtime::now()))
                .await
                .unwrap();
            if health.delivered == 1 {
                assert_eq!(health.in_flight, 1);
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    sender.abort();
    let _ = sender.await;
}
