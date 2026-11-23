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
        fail2ban: Default::default(),
        firewall: Default::default(),
        host_health: Default::default(),
        services: Default::default(),
        ports: Default::default(),
        xui: Default::default(),
        geoip: Default::default(),
        retention: Default::default(),
        notification_policy: None,
        default_locale: "ru".into(),
        ssh: Default::default(),
        owner_user_id: Some("123456789".into()),
        notifications: Default::default(),
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
        fail2ban: Default::default(),
        firewall: Default::default(),
        host_health: Default::default(),
        services: Default::default(),
        ports: Default::default(),
        xui: Default::default(),
        geoip: Default::default(),
        retention: Default::default(),
        notification_policy: None,
        default_locale: "en".into(),
        ssh: Default::default(),
        owner_user_id: Some("123456789".into()),
        notifications: Default::default(),
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
        fail2ban: Default::default(),
        firewall: Default::default(),
        host_health: Default::default(),
        services: Default::default(),
        ports: Default::default(),
        xui: Default::default(),
        geoip: Default::default(),
        retention: Default::default(),
        notification_policy: None,
        default_locale: "en".into(),
        ssh: Default::default(),
        owner_user_id: Some("123456789".into()),
        notifications: Default::default(),
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

#[tokio::test]
async fn history_uses_saved_locale_and_rejects_unsafe_chat_context() {
    use garde_daemon::{
        delivery::Outcome,
        runtime::{Database, now},
    };
    let directory = tempfile::tempdir().unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config: Config = serde_json::from_value(json!({
        "server_id":"server-demo", "bot_token_file":directory.path().join("absent"),
        "state_dir":directory.path(), "owner_user_id":"123456789", "default_locale":"he"
    }))
    .unwrap();
    let database = Database::open(config.clone()).await.unwrap();
    let mut bot = Bot::new(config).unwrap();
    let mut event: garde_contracts::Event =
        serde_json::from_str(include_str!("../../../fixtures/contracts/event.json")).unwrap();
    let event_id = "z".repeat(128);
    let first_id = event_id.clone();
    let source = event.source.as_str().to_owned();
    let at = event.observed_at.to_rfc3339();
    database
        .call(move |s| {
            let persisted_at = now();
            event.id = Id::new("older")?;
            s.publish(&event, &[], persisted_at)?;
            event.id = Id::new(first_id)?;
            s.publish(&event, &[], persisted_at)
        })
        .await
        .unwrap();
    let update = |epoch, user, kind, text: &str| {
        json!({
            "update_id":epoch, "message":{"from":{"id":user,"is_bot":false},
            "chat":{"id":user,"type":kind},"text":text}
        })
    };
    for (epoch, user, kind) in [
        (1, 987654321, "private"),
        (2, 123456789, "group"),
        (3, 123456789, "private"),
    ] {
        bot.process_update(
            &database,
            &update(epoch, user, kind, "/recent 1"),
            &Host,
            "test_bot",
        )
        .await
        .unwrap();
    }
    let (reply, pending) = database
        .call(|s| {
            let delivery = s.claim(now())?.unwrap();
            s.finish(delivery.id, Outcome::Delivered, now())?;
            Ok((delivery.reply, s.health(now())?.pending))
        })
        .await
        .unwrap();
    assert_eq!(reply.chat_id, "123456789");
    assert!(reply.text.starts_with("היסטוריית אירועים"));
    assert!(reply.text.contains(&event_id));
    assert!(reply.text.contains(&source));
    assert!(reply.text.contains(&at));
    assert!(reply.text.contains('\u{2068}'));
    assert_eq!(pending, 0);
    let next = reply
        .text
        .lines()
        .find(|line| line.starts_with("/recent 1 "))
        .unwrap();
    assert!(next.ends_with(&event_id));
    bot.process_update(
        &database,
        &update(4, 123456789, "private", next),
        &Host,
        "test_bot",
    )
    .await
    .unwrap();
    let reply = database
        .call(|s| Ok(s.claim(now())?.unwrap().reply))
        .await
        .unwrap();
    assert!(reply.text.contains("older"));
    assert!(!reply.text.contains(&event_id));
}

#[tokio::test]
async fn owner_can_queue_one_health_reply_under_outbox_pressure() {
    use garde_daemon::{
        Reply,
        runtime::{Database, now},
    };
    let directory = tempfile::tempdir().unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config: Config = serde_json::from_value(json!({
        "server_id":"server-demo", "bot_token_file":directory.path().join("absent"),
        "state_dir":directory.path(), "owner_user_id":"123456789", "default_locale":"en",
        "retention":{"pending_deliveries":2048}
    }))
    .unwrap();
    let db = Database::open(config.clone()).await.unwrap();
    let mut bot = Bot::new(config).unwrap();
    db.call(|s| {
        let mut event: garde_contracts::Event =
            serde_json::from_str(include_str!("../../../fixtures/contracts/event.json")).unwrap();
        let replies: Vec<_> = (0..32)
            .map(|i| Reply {
                envelope: Default::default(),
                locale: None,
                chat_id: (123450000 + i).to_string(),
                text: "queued event".into(),
            })
            .collect();
        for i in 0..64 {
            event.id = Id::new(format!("event-batch-{i}")).unwrap();
            s.publish(&event, &replies, now().saturating_add(20))?;
        }
        Ok(())
    })
    .await
    .unwrap();
    let update = |id| {
        json!({"update_id":id,"message":{"from":{"id":123456789,"is_bot":false},
        "chat":{"id":123456789,"type":"private"},"text":"/health"}})
    };
    assert_eq!(
        bot.process_update(&db, &update(1), &Host, "test_bot").await,
        Ok(2)
    );
    assert_eq!(
        bot.process_update(&db, &update(2), &Host, "test_bot").await,
        Err("storage.quota")
    );
    let reply = db
        .call(|s| Ok(s.claim(now())?.unwrap().reply))
        .await
        .unwrap();
    assert!(reply.text.contains("Intake limited by storage"));
}

#[tokio::test]
async fn notifications_schedule_is_localized_and_private_to_owner() {
    use garde_daemon::runtime::{Database, now};
    let temporary = tempfile::tempdir().unwrap();
    let dir = temporary.path();
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
    let windows: Vec<_> = (0..16)
        .map(|minute| format!("00:{minute:02}-01:{minute:02}"))
        .collect();
    let config: Config = serde_json::from_value(json!({
        "server_id":"server-demo","bot_token_file":dir.join("absent"),"state_dir":dir,
        "owner_user_id":"123456789","default_locale":"en",
        "notification_policy":{"default_preferences":{
            "time_zone":"Etc/UTC","daily":{"at":"14:00"},"quiet_hours":windows
        }}
    }))
    .unwrap();
    let db = Database::open(config.clone()).await.unwrap();
    let mut bot = Bot::new(config).unwrap();
    let update = |id: i64, actor: i64, chat: i64, kind: &str, text: &str| json!({"update_id":id,"message":{"from":{"id":actor,"is_bot":false},"chat":{"id":chat,"type":kind},"text":text}});
    for (id, actor, chat, kind) in [
        (1, 123456788, 123456788, "private"),
        (2, 123456789, -123456789, "supergroup"),
    ] {
        bot.process_update(
            &db,
            &update(id, actor, chat, kind, "/notifications"),
            &Host,
            "test_bot",
        )
        .await
        .unwrap();
    }
    assert!(db.call(|s| s.claim(now())).await.unwrap().is_none());
    for (index, (locale, copy)) in garde_entrypoint::catalog()["locales"]
        .as_object()
        .unwrap()
        .iter()
        .enumerate()
    {
        let first = index as i64 * 2 + 3;
        bot.process_update(
            &db,
            &update(
                first,
                123456789,
                123456789,
                "private",
                &format!("/language {locale}"),
            ),
            &Host,
            "test_bot",
        )
        .await
        .unwrap();
        bot.process_update(
            &db,
            &update(first + 1, 123456789, 123456789, "private", "/notifications"),
            &Host,
            "test_bot",
        )
        .await
        .unwrap();
        let text = db
            .call(|s| {
                let language = s.claim(now())?.expect("language reply");
                s.finish(
                    language.id,
                    garde_daemon::delivery::Outcome::Delivered,
                    now(),
                )?;
                let schedule = s.claim(now())?.expect("schedule reply");
                s.finish(
                    schedule.id,
                    garde_daemon::delivery::Outcome::Delivered,
                    now(),
                )?;
                Ok(schedule.reply.text)
            })
            .await
            .unwrap();
        assert!(text.contains(copy["notification_title"].as_str().unwrap()));
        assert!(text.contains(copy["notification_daily"].as_str().unwrap()));
        assert!(text.contains("T14:00:00"));
        assert!(text.contains("00:15-01:15"));
        if ["ar", "he"].contains(&locale.as_str()) {
            assert!(text.contains("\u{2068}00:15-01:15\u{2069}"));
        }
    }
}
