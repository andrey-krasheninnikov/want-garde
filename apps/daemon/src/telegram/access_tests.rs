use super::tests::{server, server_with_hook};
use crate::{
    Bot, StatusSource,
    delivery::{Delivery, Outcome, sql},
    firewall::HelperClient,
    runtime::{Database, Transport, now},
};
use garde_config::Config;
use garde_contracts::{HostSnapshot, access::AccessCommand};
use serde_json::{Value, json};
use std::{cell::Cell, fs, os::unix::fs::PermissionsExt};

const OWNER: i64 = 123456789;
const USER: i64 = 234567890;
const GROUP: i64 = -1001234567890;

#[derive(Default)]
struct Host(Cell<usize>);

impl StatusSource for Host {
    async fn snapshot(&self) -> HostSnapshot {
        self.0.set(self.0.get() + 1);
        snapshot()
    }
}

fn snapshot() -> HostSnapshot {
    serde_json::from_str(include_str!(
        "../../../../fixtures/contracts/host-snapshot.json"
    ))
    .unwrap()
}

async fn fixture() -> (tempfile::TempDir, Bot, Database) {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config: Config = serde_json::from_value(json!({
        "server_id":"access-fixture",
        "state_dir":dir.path(),
        "bot_token_file":dir.path().join("token"),
        "owner_user_id":OWNER.to_string(),
        "default_locale":"en",
    }))
    .unwrap();
    let database = Database::open(config.clone()).await.unwrap();
    (dir, Bot::new(config).unwrap(), database)
}

async fn administer(database: &Database, words: &str) {
    let command = AccessCommand::parse(&words.split_whitespace().collect::<Vec<_>>()).unwrap();
    database
        .call(move |store| store.local_access(command, now()))
        .await
        .unwrap();
}

fn entry(id: i64, user: i64, chat: i64, kind: &str, text: &str) -> Value {
    json!({
        "update_id":id,
        "message":{"text":text,"from":{"id":user,"is_bot":false,"language_code":"he"},"chat":{"id":chat,"type":kind}}
    })
}

fn press(id: i64, token: &str, user: i64, chat: i64, kind: &str) -> Value {
    json!({
        "update_id":id,
        "callback_query":{
            "id":id.to_string(),"data":token,"from":{"id":user,"is_bot":false,"language_code":"ar"},
            "message":{"chat":{"id":chat,"type":kind},"date":1}
        }
    })
}

async fn claim(database: &Database) -> Delivery {
    database
        .call(|store| store.claim(now()))
        .await
        .unwrap()
        .unwrap()
}

async fn finish(database: &Database, id: i64, outcome: Outcome, at: i64) {
    database
        .call(move |store| store.finish(id, outcome, at))
        .await
        .unwrap();
}

async fn status(bot: &mut Bot, database: &Database, host: &impl StatusSource, id: i64) {
    bot.process_update(
        database,
        &entry(id, USER, GROUP, "supergroup", "/status"),
        host,
        "fixture_bot",
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn only_the_exact_telegram_principal_can_read_from_the_granted_chat() {
    let (_dir, mut bot, database) = fixture().await;
    let host = Host::default();
    administer(&database, "grant 234567890 -1001234567890 status.read").await;
    let mut invalid = [
        entry(1, USER + 1, GROUP, "supergroup", "/status"),
        entry(2, USER, GROUP - 1, "supergroup", "/status"),
        entry(3, USER, GROUP, "channel", "/status"),
        entry(4, USER, GROUP, "supergroup", "/status@some_other_bot"),
        entry(5, USER, USER, "private", "/status"),
        entry(6, USER, GROUP, "supergroup", "/access list"),
        entry(7, USER, GROUP, "supergroup", "/status"),
        entry(8, USER, GROUP, "supergroup", "/status"),
        entry(9, USER, GROUP, "private", "/status"),
    ];
    invalid[0]["message"]["from"]["status"] = json!("administrator");
    invalid[6]["message"]["sender_chat"] = json!({"id":GROUP});
    invalid[7]["message"]["from"]["is_bot"] = json!(true);
    for update in invalid {
        bot.process_update(&database, &update, &host, "fixture_bot")
            .await
            .unwrap();
    }
    assert_eq!(host.0.get(), 0);
    assert!(
        database
            .call(|store| store.claim(now()))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        database
            .call(|store| sql(store.connection.query_row(
                "SELECT count(*) FROM access_audit WHERE source='telegram'",
                [],
                |row| row.get::<_, i64>(0),
            )))
            .await
            .unwrap(),
        9
    );

    administer(&database, "grant 234567890 234567890 status.read").await;
    let private = entry(10, USER, USER, "private", "/status@fixture_bot");
    bot.process_update(&database, &private, &host, "fixture_bot")
        .await
        .unwrap();
    let delivery = claim(&database).await;
    assert_eq!(delivery.reply.chat_id, USER.to_string());
    assert!(
        delivery
            .reply
            .text
            .contains(garde_entrypoint::message("he", "status_title"))
    );
    assert_eq!(host.0.get(), 1);
}

#[tokio::test]
async fn blocked_private_reply_never_falls_back_and_explicit_group_reply_keeps_topic() {
    let (_dir, mut bot, database) = fixture().await;
    let host = Host::default();
    administer(&database, "grant 234567890 -1001234567890 status.read").await;
    status(&mut bot, &database, &host, 1).await;
    let delivery = claim(&database).await;
    let (telegram, worker) = server(vec![(403, json!({"ok":false,"error_code":403}))]);
    let error = telegram
        .send_access(&delivery, &database)
        .await
        .unwrap_err();
    assert_eq!(error.code, "telegram.forbidden");
    let observed = worker.join().unwrap();
    assert_eq!(observed.len(), 1);
    assert!(observed[0].0.contains("sendMessage"));
    assert_eq!(observed[0].1["chat_id"], USER.to_string());
    finish(
        &database,
        delivery.id,
        Outcome::Permanent { code: error.code },
        now(),
    )
    .await;
    assert!(
        database
            .call(|store| store.claim(now()))
            .await
            .unwrap()
            .is_none()
    );

    administer(&database, "group -1001234567890 group").await;
    let mut command = entry(2, USER, GROUP, "supergroup", "/status");
    command["message"]["is_topic_message"] = json!(true);
    command["message"]["message_thread_id"] = json!(17);
    bot.process_update(&database, &command, &host, "fixture_bot")
        .await
        .unwrap();
    let delivery = claim(&database).await;
    let (telegram, worker) = server(vec![
        (
            200,
            json!({"ok":true,"result":{"id":GROUP,"type":"supergroup","is_forum":true}}),
        ),
        (
            200,
            json!({"ok":true,"result":{"message_id":5,"chat":{"id":GROUP,"type":"supergroup","is_forum":true},"message_thread_id":17}}),
        ),
    ]);
    telegram.send_access(&delivery, &database).await.unwrap();
    let observed = worker.join().unwrap();
    assert!(observed[0].0.contains("getChat"));
    assert_eq!(observed[1].1["chat_id"], GROUP.to_string());
    assert_eq!(observed[1].1["message_thread_id"], 17);
    assert_eq!(
        observed[1].1["reply_markup"]["inline_keyboard"][0][0]["text"],
        garde_entrypoint::message("he", "access_refresh")
    );
}

#[tokio::test]
async fn changing_policy_during_group_preflight_blocks_actual_send() {
    let (_dir, mut bot, database) = fixture().await;
    let host = Host::default();
    administer(&database, "grant 234567890 -1001234567890 status.read").await;
    administer(&database, "group -1001234567890 group").await;
    status(&mut bot, &database, &host, 1).await;
    let delivery = claim(&database).await;
    let handle = tokio::runtime::Handle::current();
    let admin_db = database.clone();
    let (telegram, worker) = server_with_hook(
        vec![(
            200,
            json!({"ok":true,"result":{"id":GROUP,"type":"supergroup"}}),
        )],
        move |path| {
            assert!(path.contains("getChat"));
            handle.block_on(administer(&admin_db, "group -1001234567890 private"));
        },
    );
    let error = telegram
        .send_access(&delivery, &database)
        .await
        .unwrap_err();
    assert_eq!(error.code, "access.changed");
    assert_eq!(worker.join().unwrap().len(), 1);
}

#[tokio::test]
async fn a_regranted_permission_does_not_release_a_retry_or_a_stale_button() {
    let (_dir, mut bot, database) = fixture().await;
    let host = Host::default();
    administer(&database, "grant 234567890 -1001234567890 status.read").await;
    status(&mut bot, &database, &host, 1).await;
    let delivery = claim(&database).await;
    let token = delivery.protected.unwrap().button.unwrap().token;
    finish(
        &database,
        delivery.id,
        Outcome::Retry {
            code: "telegram.network",
            retry_after: 0,
            uncertain: false,
        },
        now() - 10,
    )
    .await;
    administer(&database, "revoke 234567890 -1001234567890 status.read").await;
    administer(&database, "grant 234567890 -1001234567890 status.read").await;
    let delivery = claim(&database).await;
    assert_eq!(delivery.attempts, 2);
    let (telegram, worker) = server(vec![]);
    assert_eq!(
        telegram
            .send_access(&delivery, &database)
            .await
            .unwrap_err()
            .code,
        "access.changed"
    );
    assert!(worker.join().unwrap().is_empty());
    let (_, ack) = bot
        .process_update_and_ack(
            &database,
            &press(2, &token, USER, USER, "private"),
            &host,
            &HelperClient,
            "fixture_bot",
        )
        .await
        .unwrap();
    assert_eq!(
        ack.unwrap().text,
        Some(garde_entrypoint::message("ar", "access_denied"))
    );
    assert_eq!(host.0.get(), 1);
}

#[tokio::test]
async fn a_private_button_for_a_group_grant_is_bound_to_its_user_and_consumes_once() {
    let (_dir, mut bot, database) = fixture().await;
    let host = Host::default();
    administer(&database, "grant 234567890 -1001234567890 status.read").await;
    status(&mut bot, &database, &host, 1).await;
    let delivery = claim(&database).await;
    let token = delivery.protected.unwrap().button.unwrap().token;
    finish(&database, delivery.id, Outcome::Delivered, now()).await;
    for (id, user, chat, kind, value, expected) in [
        (2, USER + 1, USER + 1, "private", token.as_str(), false),
        (3, USER, GROUP, "supergroup", token.as_str(), false),
        (
            4,
            USER,
            USER,
            "private",
            "ffffffffffffffffffffffffffffffff",
            false,
        ),
        (5, USER, USER, "private", token.as_str(), true),
        (6, USER, USER, "private", token.as_str(), false),
    ] {
        let (_, ack) = bot
            .process_update_and_ack(
                &database,
                &press(id, value, user, chat, kind),
                &host,
                &HelperClient,
                "fixture_bot",
            )
            .await
            .unwrap();
        let ack = ack.unwrap();
        assert_eq!(ack.text.is_none(), expected);
        if id == 6 {
            let (telegram, worker) = server(vec![(200, json!({"ok":true,"result":true}))]);
            telegram.answer_callback(&ack).await.unwrap();
            let observed = worker.join().unwrap();
            assert!(observed[0].0.contains("answerCallbackQuery"));
            assert_eq!(
                observed[0].1,
                json!({"callback_query_id":"6","text":garde_entrypoint::message("ar", "access_denied")})
            );
        }
    }
    assert_eq!(host.0.get(), 2);
    let delivery = claim(&database).await;
    assert_eq!(delivery.reply.chat_id, USER.to_string());
    assert!(
        delivery
            .reply
            .text
            .contains(garde_entrypoint::message("ar", "status_title"))
    );
    let token = delivery.protected.unwrap().button.unwrap().token;
    finish(&database, delivery.id, Outcome::Delivered, now()).await;
    database
        .call(|store| {
            sql(store
                .connection
                .execute("UPDATE access_buttons SET expires_at=0", []))
        })
        .await
        .unwrap();
    let (_, ack) = bot
        .process_update_and_ack(
            &database,
            &press(7, &token, USER, USER, "private"),
            &host,
            &HelperClient,
            "fixture_bot",
        )
        .await
        .unwrap();
    assert!(ack.unwrap().text.is_some());
    assert_eq!(host.0.get(), 2);
}

#[tokio::test]
async fn revoke_during_a_read_prevents_the_result_from_entering_the_outbox() {
    struct RevokingHost(Database);
    impl StatusSource for RevokingHost {
        async fn snapshot(&self) -> HostSnapshot {
            administer(&self.0, "revoke 234567890 -1001234567890 status.read").await;
            snapshot()
        }
    }
    let (_dir, mut bot, database) = fixture().await;
    administer(&database, "grant 234567890 -1001234567890 status.read").await;
    status(&mut bot, &database, &RevokingHost(database.clone()), 1).await;
    assert!(
        database
            .call(|store| store.claim(now()))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(database.call(|store| store.offset()).await.unwrap(), 2);
}

#[tokio::test]
async fn audit_failure_stops_authorization_group_changes_and_pending_delivery() {
    let (_dir, mut bot, database) = fixture().await;
    let host = Host::default();
    administer(&database, "grant 234567890 -1001234567890 status.read").await;
    status(&mut bot, &database, &host, 1).await;
    let pending = claim(&database).await;
    database.call(|store| sql(store.connection.execute_batch(
        "CREATE TRIGGER fail_audit BEFORE INSERT ON access_audit BEGIN SELECT RAISE(ABORT,'audit failure'); END;",
    ))).await.unwrap();
    let command = entry(
        2,
        OWNER,
        OWNER,
        "private",
        "/access group -1001234567890 group",
    );
    assert_eq!(
        bot.process_update(&database, &command, &host, "fixture_bot")
            .await,
        Err("storage.failed")
    );
    let request = entry(2, USER, GROUP, "supergroup", "/status");
    assert_eq!(
        bot.process_update(&database, &request, &host, "fixture_bot")
            .await,
        Err("storage.failed")
    );
    assert_eq!(host.0.get(), 1);
    assert_eq!(database.call(|store| store.offset()).await.unwrap(), 2);

    let (telegram, worker) = server(vec![]);
    let failure = telegram.send_access(&pending, &database).await.unwrap_err();
    assert_eq!(failure.code, "storage.failed");
    assert!(!failure.uncertain);
    assert!(worker.join().unwrap().is_empty());

    database
        .call(|store| sql(store.connection.execute_batch("DROP TRIGGER fail_audit")))
        .await
        .unwrap();
    assert!(
        database
            .call(|store| store.local_access(AccessCommand::List {}, now()))
            .await
            .is_ok()
    );
    assert_eq!(
        database
            .call(|store| sql(store.connection.query_row(
                "SELECT count(*) FROM access_group_delivery",
                [],
                |row| row.get::<_, i64>(0),
            )))
            .await
            .unwrap(),
        0
    );
}
