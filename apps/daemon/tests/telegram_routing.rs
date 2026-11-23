use garde_config::Config;
use garde_contracts::{Event, Id};
use garde_daemon::{
    Bot, DeliveryEnvelope, StatusSource,
    delivery::{Delivery, Outcome, Store},
    runtime::{Database, now},
};
use serde_json::json;
use std::time::Duration;
use std::{fs, os::unix::fs::PermissionsExt, path::Path};

fn config(path: &Path) -> Config {
    load_config(path, &json!({
            "server_id":"server-demo", "owner_user_id":"123456789",
            "bot_token_file":path.join("token"), "state_dir":path, "default_locale":"en",
            "notifications": {
                "private": [{"chat_id":"234567890","options":{"locale":"ru","protect_content":true}}],
                "forums": [{
                    "chat_id":"-1001234567890","options":{"locale":"ar","silent":true},
                    "default_topic_id":20,"topics":[{"event_type":"ssh.failed","topic_id":42}]
                }]
            }
    })).unwrap()
}

fn load_config(path: &Path, value: &serde_json::Value) -> garde_config::Result<Config> {
    let config = path.join("config.json");
    fs::write(&config, value.to_string()).unwrap();
    fs::set_permissions(&config, fs::Permissions::from_mode(0o600)).unwrap();
    Config::load(&config)
}

#[test]
fn invalid_chat_ids_topic_routes_locales_and_quota_fail_configuration() {
    let directory = tempfile::tempdir().unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    config(directory.path());
    let valid: serde_json::Value =
        serde_json::from_slice(&fs::read(directory.path().join("config.json")).unwrap()).unwrap();
    let mut general = valid.clone();
    general["notifications"]["forums"][0]["default_topic_id"] = serde_json::Value::Null;
    let loaded = load_config(directory.path(), &general).unwrap();
    assert_eq!(
        loaded.notifications.forums[0].default_topic_id.thread_id(),
        None
    );

    for (pointer, value) in [
        ("/notifications/private/0/chat_id", json!("123456789")),
        ("/notifications/private/0/chat_id", json!("-123456789")),
        ("/notifications/private/0/chat_id", json!("0234567890")),
        ("/notifications/forums/0/chat_id", json!("1001234567890")),
        (
            "/notifications/forums/0/chat_id",
            json!("-4503599627370496"),
        ),
        ("/notifications/forums/0/default_topic_id", json!(1)),
        (
            "/notifications/forums/0/default_topic_id",
            json!(2147483648_i64),
        ),
        ("/notifications/forums/0/options/locale", json!("en-us")),
        (
            "/notifications/forums/0/topics",
            json!([
                {"event_type":"ssh.failed","topic_id":42},{"event_type":"ssh.failed","topic_id":43}
            ]),
        ),
        (
            "/notifications/private",
            json!([
                {"chat_id":"223456789"},{"chat_id":"323456789"},{"chat_id":"423456789"}
            ]),
        ),
    ] {
        let mut invalid = valid.clone();
        *invalid.pointer_mut(pointer).unwrap() = value;
        assert!(
            matches!(
                load_config(directory.path(), &invalid),
                Err("config.invalid")
            ),
            "{pointer}"
        );
    }
    let mut missing = valid.clone();
    missing["notifications"]["forums"][0]
        .as_object_mut()
        .unwrap()
        .remove("default_topic_id");
    assert!(matches!(
        load_config(directory.path(), &missing),
        Err("config.invalid")
    ));

    let mut too_small = valid;
    too_small["retention"] = json!({"pending_deliveries":6143});
    assert!(matches!(
        load_config(directory.path(), &too_small),
        Err("config.invalid")
    ));
    too_small["retention"] = json!({"pending_deliveries":6144});
    assert!(load_config(directory.path(), &too_small).is_ok());
}

fn event(id: &str, rule: &str) -> Event {
    let mut event: Event =
        serde_json::from_str(include_str!("../../../fixtures/contracts/event.json")).unwrap();
    event.id = Id::new(id).unwrap();
    event.classification.rule_id = Id::new(rule).unwrap();
    event
}

#[tokio::test]
async fn explicit_private_recipients_and_forum_topics_share_the_persistent_outbox() {
    let directory = tempfile::tempdir().unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config = config(directory.path());
    let bot = Bot::new(config.clone()).unwrap();
    let database = Database::open(config).await.unwrap();
    for (id, rule) in [
        ("event-login", "ssh.login"),
        ("event-failure", "ssh.failed"),
    ] {
        bot.publish_event(&database, event(id, rule)).await.unwrap();
    }
    for (id, topic_id) in [("event-login", 20), ("event-failure", 42)] {
        for (chat, title, expected) in [
            ("123456789", "Event", DeliveryEnvelope::default()),
            (
                "234567890",
                "Событие",
                DeliveryEnvelope {
                    protect_content: true,
                    redacted: true,
                    ..Default::default()
                },
            ),
            (
                "-1001234567890",
                "حدث",
                DeliveryEnvelope {
                    topic_id: Some(topic_id),
                    silent: true,
                    redacted: true,
                    ..Default::default()
                },
            ),
        ] {
            let delivery = claim(&database, now()).await.unwrap();
            assert_eq!(delivery.reply.chat_id, chat);
            assert_eq!(delivery.reply.envelope, expected);
            assert!(delivery.reply.text.starts_with(title));
            assert!(delivery.reply.text.contains(id));
            finish(&database, delivery.id, Outcome::Delivered).await;
        }
    }
    assert!(claim(&database, now()).await.is_none());
}

async fn claim(database: &Database, at: i64) -> Option<Delivery> {
    database.call(move |store| store.claim(at)).await.unwrap()
}

async fn finish(database: &Database, id: i64, outcome: Outcome) {
    database
        .call(move |store| store.finish(id, outcome, now()))
        .await
        .unwrap();
}

struct GeoIp;
impl garde_daemon::geoip::Provider for GeoIp {
    async fn lookup(
        &self,
        _: &str,
    ) -> Result<garde_contracts::GeoIpDetails, garde_daemon::geoip::LookupFailure> {
        Ok(garde_contracts::GeoIpDetails {
            country_code: Some("US".into()),
            asn: Some(15169),
            organization: Some("Test network".into()),
        })
    }
}

#[tokio::test]
async fn delivery_keeps_its_locale_privacy_and_topic_across_cold_geoip_and_restart() {
    let directory = tempfile::tempdir().unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut config = config(directory.path());
    config.geoip.enabled = true;
    config.geoip.token_file = Some(directory.path().join("geoip-token"));
    let forum = &mut config.notifications.forums[0];
    forum.options.locale = Some("he".into());
    forum.options.protect_content = true;
    let mut full_forum = forum.clone();
    full_forum.chat_id = "-1009876543210".into();
    full_forum.options.locale = Some("ru".into());
    full_forum.options.privacy = Some(garde_config::Privacy::Full);
    config.notifications.forums.push(full_forum);
    let bot = Bot::new(config.clone()).unwrap();
    let database = Database::open(config.clone()).await.unwrap();
    let mut event = event("cold-event", "ssh.failed");
    event.classification.reason_key = Id::new("ssh_failed").unwrap();
    event.evidence = [
        ("ip", "8.8.8.8"),
        ("user", "private-user"),
        ("xui_source", "[2001:4860:4860::8888]:1234"),
        ("process_path", "/private/process"),
    ]
    .into_iter()
    .map(|(name, value)| garde_contracts::Evidence {
        name: Id::new(name).unwrap(),
        value: value.into(),
    })
    .collect();
    bot.publish_event(&database, event).await.unwrap();
    let extra = claim(&database, now()).await.unwrap();
    assert_eq!(extra.reply.chat_id, "234567890");
    assert!(extra.reply.text.contains("8.8.8.x"));
    finish(&database, extra.id, Outcome::Delivered).await;
    drop(database);

    config.notifications = Default::default();
    config.default_locale = "de".into();
    let database = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match Database::open(config.clone()).await {
                Ok(database) => break database,
                Err("storage.locked") => tokio::time::sleep(Duration::from_millis(10)).await,
                Err(code) => panic!("{code}"),
            }
        }
    })
    .await
    .unwrap();
    let forum = claim(&database, now()).await.unwrap();
    assert_eq!(forum.reply.chat_id, "-1001234567890");
    assert_eq!(
        forum.reply.envelope,
        DeliveryEnvelope {
            topic_id: Some(42),
            redacted: true,
            silent: true,
            protect_content: true,
        }
    );
    assert!(forum.reply.text.starts_with("אירוע"));
    assert!(forum.reply.text.contains("\u{2068}8.8.8.x\u{2069}"));
    assert!(forum.reply.text.contains("[מוסתר]"));
    assert!(forum.reply.text.contains("IPv6 [מוסתר]"));
    for secret in [
        "8.8.8.8",
        "private-user",
        "2001:4860",
        "1234",
        "/private/process",
        "GeoIP",
        "/geoip",
    ] {
        assert!(!forum.reply.text.contains(secret), "{secret}");
    }
    finish(
        &database,
        forum.id,
        Outcome::Permanent {
            code: "telegram.forum_topic",
        },
    )
    .await;
    assert!(claim(&database, now()).await.is_none());
    assert!(
        garde_daemon::geoip::run_once(&database, &GeoIp, now())
            .await
            .unwrap()
    );
    let owner = claim(&database, now()).await.unwrap();
    assert_eq!(owner.reply.chat_id, "123456789");
    assert!(owner.reply.text.starts_with("Event"));
    for text in [
        "8.8.8.8",
        "private-user",
        "Test network",
        "/geoip cold-event",
        "Event: cold-event",
        "ID: d-",
    ] {
        assert!(owner.reply.text.contains(text), "{text}");
    }
    finish(&database, owner.id, Outcome::Delivered).await;

    let forum = claim(&database, now()).await.unwrap();
    assert_eq!(forum.reply.chat_id, "-1009876543210");
    assert_eq!(
        forum.reply.envelope,
        DeliveryEnvelope {
            topic_id: Some(42),
            redacted: false,
            silent: true,
            protect_content: true,
        }
    );
    assert!(forum.reply.text.starts_with("Событие"));
    assert!(forum.reply.text.contains("Test network"));
    assert!(!forum.reply.text.contains("/geoip"));
    let original = forum.reply.text;
    finish(
        &database,
        forum.id,
        Outcome::Retry {
            code: "telegram.network",
            retry_after: 0,
            uncertain: true,
        },
    )
    .await;
    let retry = claim(&database, now() + 10).await.unwrap();
    assert_eq!(retry.id, forum.id);
    assert_eq!(retry.reply.envelope, forum.reply.envelope);
    assert_eq!(retry.reply.text, original);
    let health = database.call(|store| store.health(now())).await.unwrap();
    assert_eq!(health.delivered, 2);
    assert!(
        health
            .problems
            .iter()
            .any(|p| p.reason_key.as_str() == "telegram.forum_topic")
    );
}

#[tokio::test]
async fn owner_can_read_saved_context_by_extra_private_and_forum_delivery_ids() {
    let directory = tempfile::tempdir().unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut config = config(directory.path());
    config.geoip.enabled = true;
    config.notifications.private[0].options.privacy = Some(garde_config::Privacy::Full);
    config.notifications.forums[0].options.privacy = Some(garde_config::Privacy::Full);
    let mut bot = Bot::new(config.clone()).unwrap();
    let database = Database::open(config).await.unwrap();
    let mut event = event("warm-cache", "ssh.failed");
    event.evidence = vec![garde_contracts::Evidence {
        name: Id::new("ip").unwrap(),
        value: "8.8.8.8".into(),
    }];
    bot.publish_event(&database, event.clone()).await.unwrap();
    assert!(
        garde_daemon::geoip::run_once(&database, &GeoIp, now())
            .await
            .unwrap()
    );
    while let Some(delivery) = claim(&database, now()).await {
        finish(&database, delivery.id, Outcome::Delivered).await;
    }
    event.id = Id::new("cached-geoip").unwrap();
    bot.publish_event(&database, event).await.unwrap();
    let mut references = Vec::new();
    for chat in ["123456789", "234567890", "-1001234567890"] {
        let delivery = claim(&database, now()).await.unwrap();
        assert_eq!(delivery.reply.chat_id, chat);
        assert!(delivery.reply.text.contains("Test network"));
        if chat == "123456789" {
            assert!(delivery.reply.text.contains("/geoip cached-geoip"));
        } else {
            assert!(!delivery.reply.text.contains("/geoip"));
            references.push(
                delivery
                    .reply
                    .text
                    .lines()
                    .last()
                    .unwrap()
                    .strip_prefix("ID: ")
                    .unwrap()
                    .to_owned(),
            );
        }
        finish(&database, delivery.id, Outcome::Delivered).await;
    }

    for (update_id, reference) in references.into_iter().enumerate() {
        let update = json!({
            "update_id": update_id,
            "message": {
                "from": {"id":123456789,"is_bot":false},
                "chat": {"id":123456789,"type":"private"},
                "text": format!("/geoip {reference}")
            }
        });
        bot.process_update(&database, &update, &Host, "test_bot")
            .await
            .unwrap();
        let response = claim(&database, now()).await.unwrap();
        assert_eq!(response.reply.chat_id, "123456789");
        assert!(response.reply.text.contains("cached-geoip"));
        assert!(response.reply.text.contains("Test network"));
        finish(&database, response.id, Outcome::Delivered).await;
    }
}

struct Host;
impl StatusSource for Host {
    async fn snapshot(&self) -> garde_contracts::HostSnapshot {
        panic!("foreign commands must not request host data");
    }
}

#[tokio::test]
async fn notifications_do_not_authorize_commands_or_register_an_unknown_chat() {
    let directory = tempfile::tempdir().unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config = config(directory.path());
    let mut bot = Bot::new(config.clone()).unwrap();
    let database = Database::open(config).await.unwrap();
    for (update_id, user, chat, kind, text) in [
        (1, 234567890, 234567890, "private", "/status"),
        (2, 345678901, 345678901, "private", "/start"),
        (
            3,
            123456789,
            -1001234567890_i64,
            "supergroup",
            "/geoip event-001",
        ),
    ] {
        let update = json!({
            "update_id": update_id,
            "message": {"from":{"id":user,"is_bot":false},"chat":{"id":chat,"type":kind},"text":text}
        });
        assert_eq!(
            bot.process_update(&database, &update, &Host, "test_bot")
                .await
                .unwrap(),
            update_id + 1
        );
    }
    assert!(claim(&database, now()).await.is_none());
    bot.publish_event(&database, event("no-registration", "ssh.failed"))
        .await
        .unwrap();
    let health = database.call(|store| store.health(now())).await.unwrap();
    assert_eq!(health.pending, 3);
}

#[test]
fn legacy_owner_delivery_retains_its_identity_body_and_settings_on_schema_upgrade() {
    let directory = tempfile::tempdir().unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let path = directory.path().join("garde.sqlite3");
    let connection = rusqlite::Connection::open(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    connection.execute_batch("
        CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
        INSERT INTO meta VALUES ('server','server-demo'),('owner','123456789'),
            ('offset','7'),('pruned','0'),('epoch','0'),('last_update_at','1000');
        CREATE TABLE events(id TEXT PRIMARY KEY, payload TEXT NOT NULL, created_at INTEGER NOT NULL);
        CREATE TABLE cooldowns(chat_id TEXT PRIMARY KEY, until_at INTEGER NOT NULL);
        CREATE TABLE deliveries(
            id INTEGER PRIMARY KEY AUTOINCREMENT, event_id TEXT REFERENCES events(id),
            request_id TEXT UNIQUE, chat_id TEXT NOT NULL, text TEXT NOT NULL,
            state TEXT NOT NULL DEFAULT 'pending', attempts INTEGER NOT NULL DEFAULT 0,
            next_at INTEGER NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL, last_error TEXT, uncertain INTEGER NOT NULL DEFAULT 0,
            UNIQUE(event_id,chat_id));
        INSERT INTO deliveries(id,request_id,chat_id,text,next_at,created_at,updated_at,expires_at)
        VALUES(5,'telegram:0:6','123456789','saved message' || char(10) || 'ID: d-5',1000,1000,1000,2000);
        PRAGMA user_version=1;
    ").unwrap();
    drop(connection);

    let mut store = Store::open(directory.path(), "server-demo", "123456789").unwrap();
    assert_eq!(store.offset().unwrap(), 7);
    let delivery = store.claim(1000).unwrap().unwrap();
    assert_eq!(delivery.id, 5);
    assert_eq!(delivery.reply.chat_id, "123456789");
    assert_eq!(delivery.reply.text, "saved message\nID: d-5");
    assert_eq!(delivery.reply.envelope, DeliveryEnvelope::default());
    drop(store);
    let connection = rusqlite::Connection::open(&path).unwrap();
    let version: i32 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 2);
}
