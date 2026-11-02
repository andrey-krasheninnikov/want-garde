use garde_config::Config;
use garde_contracts::{HostSnapshot, Id};
use garde_daemon::{Bot, StatusSource};
use serde_json::json;
use std::{fs, os::unix::fs::PermissionsExt};

struct Host;
impl StatusSource for Host {
    async fn snapshot(&self) -> HostSnapshot {
        serde_json::from_str(include_str!(
            "../../../fixtures/contracts/host-snapshot.json"
        ))
        .unwrap()
    }
}

#[tokio::test]
async fn only_the_locally_bound_owner_receives_a_private_identifiable_snapshot() {
    let temporary = tempfile::tempdir().unwrap();
    let dir = temporary.path().to_path_buf();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let config = Config {
        server_id: Id::new("test-server").unwrap(),
        bot_token_file: dir.join("token"),
        state_dir: dir.clone(),
        default_locale: "ru".into(),
        ssh: Default::default(),
        owner_user_id: Some("123456789".into()),
    };
    let mut bot = Bot::new(config).unwrap();
    let update = |id: u64, kind: &str| json!({"update_id":1,"message":{"from":{"id":id,"is_bot":false},"chat":{"id":id,"type":kind},"text":"/status"}});
    assert!(
        bot.handle(&update(987654321, "private"), &Host, "test_bot")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        bot.handle(&update(123456789, "group"), &Host, "test_bot")
            .await
            .unwrap()
            .is_none()
    );
    let reply = bot
        .handle(&update(123456789, "private"), &Host, "test_bot")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply.chat_id, "123456789");
    assert!(reply.text.contains("test-server"));
    assert!(reply.text.contains("тест <host>"));
    assert!(reply.text.contains("source.read"));
    assert!(!reply.text.contains("message.unavailable"));
}

#[tokio::test]
async fn language_selection_survives_restart_and_all_locales_render_unicode_status() {
    let temporary = tempfile::tempdir().unwrap();
    let dir = temporary.path().to_path_buf();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let config = Config {
        server_id: Id::new("test-server").unwrap(),
        bot_token_file: dir.join("token"),
        state_dir: dir.clone(),
        default_locale: "en".into(),
        ssh: Default::default(),
        owner_user_id: Some("123456789".into()),
    };
    let command = |text: &str| json!({"message":{"from":{"id":123456789,"is_bot":false},"chat":{"id":123456789,"type":"private"},"text":text}});
    let mut bot = Bot::new(config.clone()).unwrap();
    for (locale, copy) in garde_entrypoint::catalog()["locales"].as_object().unwrap() {
        let reply = bot
            .handle(&command(&format!("/language {locale}")), &Host, "test_bot")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reply.text, copy["language_saved"].as_str().unwrap());
        let mut restarted = Bot::new(config.clone()).unwrap();
        let status = restarted
            .handle(&command("/status@test_bot"), &Host, "test_bot")
            .await
            .unwrap()
            .unwrap();
        assert!(status.text.contains(copy["status_title"].as_str().unwrap()));
        assert!(status.text.contains("тест <host>"));
        assert!(status.text.encode_utf16().count() <= 4096);
        if ["ar", "he"].contains(&locale.as_str()) {
            assert!(status.text.contains('\u{2068}'));
            assert!(status.text.contains('\u{2069}'));
        }
        assert!(!status.text.contains("message.unavailable"));
    }
    let before = fs::read(dir.join("owner-preferences.json")).unwrap();
    bot.handle(&command("/language unknown"), &Host, "test_bot")
        .await
        .unwrap();
    assert_eq!(
        fs::read(dir.join("owner-preferences.json")).unwrap(),
        before
    );
    assert!(
        bot.handle(&command("/status@other_bot"), &Host, "test_bot")
            .await
            .unwrap()
            .is_none()
    );
    fs::remove_file(dir.join("owner-preferences.json")).unwrap();
    fs::create_dir(dir.join("owner-preferences.json")).unwrap();
    assert!(
        bot.handle(&command("/language ru"), &Host, "test_bot")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn identity_in_message_text_and_an_untrusted_chat_never_grant_access() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config {
        server_id: Id::new("test-server").unwrap(),
        bot_token_file: dir.path().join("token"),
        state_dir: dir.path().to_path_buf(),
        default_locale: "en".into(),
        ssh: Default::default(),
        owner_user_id: Some("123456789".into()),
    };
    let mut bot = Bot::new(config).unwrap();
    for update in [
        json!({"message":{"from":{"id":123456789,"is_bot":true},"chat":{"id":123456789,"type":"private"},"text":"/status"}}),
        json!({"message":{"from":{"id":123456789,"is_bot":false},"chat":{"id":987654321,"type":"private"},"text":"/status"}}),
        json!({"message":{"from":{"id":987654321,"is_bot":false},"chat":{"id":123456789,"type":"private"},"text":"/status owner=123456789"}}),
        json!({"edited_message":{"from":{"id":123456789},"text":"/status"}}),
        json!({"callback_query":{"from":{"id":123456789},"data":"/status"}}),
    ] {
        assert!(
            bot.handle(&update, &Host, "test_bot")
                .await
                .unwrap()
                .is_none()
        );
    }
}
