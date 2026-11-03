use garde_config::Config;
use garde_contracts::{Event, Id};
use garde_daemon::{Bot, runtime::Database};
use std::{fs, os::unix::fs::PermissionsExt};

fn configuration(dir: &std::path::Path) -> Config {
    serde_json::from_value(serde_json::json!({
        "server_id":"server-demo", "owner_user_id":"123456789",
        "bot_token_file":dir.join("token"), "state_dir":dir, "default_locale":"en"
    }))
    .unwrap()
}
fn event(id: &str, ip: &str) -> Event {
    let mut event: Event =
        serde_json::from_str(include_str!("../../../fixtures/contracts/event.json")).unwrap();
    event.id = Id::new(id).unwrap();
    event.evidence = vec![garde_contracts::Evidence {
        name: Id::new("ip").unwrap(),
        value: ip.into(),
    }];
    event
}
#[tokio::test]
async fn disabled_enrichment_preserves_delivery_and_reports_missing_context() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config = configuration(dir.path());
    let bot = Bot::new(config.clone()).unwrap();
    let database = Database::open(config).await.unwrap();
    bot.publish_event(&database, event("geo-disabled", "8.8.8.8"))
        .await
        .unwrap();
    let context = database
        .call(|s| s.geoip_event("geo-disabled"))
        .await
        .unwrap();
    assert_eq!(context.state, garde_contracts::GeoIpState::Missing);
    assert_eq!(context.reason_key.as_str(), "geoip_disabled");
    assert!(
        database
            .call(|s| s.claim_geoip(garde_daemon::runtime::now()))
            .await
            .unwrap()
            .is_none()
    );
    let delivery = database
        .call(|s| s.claim(garde_daemon::runtime::now()))
        .await
        .unwrap()
        .unwrap();
    assert!(delivery.reply.text.contains("geo-disabled"));
    assert!(delivery.reply.text.contains("GeoIP"));
}

struct Provider;
impl garde_daemon::geoip::Provider for Provider {
    async fn lookup(
        &self,
        _: &str,
    ) -> Result<garde_contracts::GeoIpDetails, garde_daemon::geoip::LookupFailure> {
        Ok(garde_contracts::GeoIpDetails {
            country_code: Some("US".into()),
            asn: Some(15169),
            organization: Some("Example network".into()),
        })
    }
}
#[tokio::test]
async fn cold_lookup_enriches_the_first_delivery_and_retries_keep_the_same_text() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut config = configuration(dir.path());
    config.geoip.enabled = true;
    config.geoip.token_file = Some(dir.path().join("geo-token"));
    let bot = Bot::new(config.clone()).unwrap();
    let database = Database::open(config).await.unwrap();
    bot.publish_event(&database, event("geo-cold", "8.8.8.8"))
        .await
        .unwrap();
    assert!(
        database
            .call(|s| s.claim(garde_daemon::runtime::now()))
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        garde_daemon::geoip::run_once(&database, &Provider, garde_daemon::runtime::now())
            .await
            .unwrap()
    );
    let context = database.call(|s| s.geoip_event("geo-cold")).await.unwrap();
    assert_eq!(context.state, garde_contracts::GeoIpState::Retrieved);
    assert_eq!(context.details.unwrap().country_code.as_deref(), Some("US"));
    let delivery = database
        .call(|s| s.claim(garde_daemon::runtime::now()))
        .await
        .unwrap()
        .unwrap();
    let original = delivery.reply.text.clone();
    assert!(original.contains("Example network"));
    assert!(original.contains("AS15169"));
    assert!(original.contains("/geoip geo-cold"));
    assert!(!original.contains("lookup pending"));
    let count = database
        .call(|s| s.health(garde_daemon::runtime::now()))
        .await
        .unwrap();
    assert_eq!(count.in_flight, 1);
    assert_eq!(count.pending, 0);
    let at = garde_daemon::runtime::now();
    database
        .call(move |s| {
            s.finish(
                delivery.id,
                garde_daemon::delivery::Outcome::Retry {
                    code: "telegram.network",
                    retry_after: 0,
                    uncertain: false,
                },
                at,
            )
        })
        .await
        .unwrap();
    let retry = database
        .call(move |s| s.claim(at + 5))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retry.reply.text, original);
    database
        .call(move |s| s.finish(retry.id, garde_daemon::delivery::Outcome::Delivered, at + 5))
        .await
        .unwrap();
    bot.publish_event(&database, event("geo-warm", "8.8.8.8"))
        .await
        .unwrap();
    let context = database.call(|s| s.geoip_event("geo-warm")).await.unwrap();
    assert_eq!(context.state, garde_contracts::GeoIpState::Cached);
    let warm = database
        .call(|s| s.claim(garde_daemon::runtime::now()))
        .await
        .unwrap()
        .unwrap();
    assert!(warm.reply.text.contains("GeoIP: cached"));
    assert!(warm.reply.text.contains("Example network"));

    assert!(
        !garde_daemon::geoip::run_once(&database, &Provider, garde_daemon::runtime::now() + 2)
            .await
            .unwrap()
    );
}

struct Never;

#[tokio::test]
async fn unresolved_lookup_releases_delivery_within_ten_seconds_and_late_results_do_not_rewrite_it()
{
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut config = configuration(dir.path());
    config.geoip.enabled = true;
    let database = Database::open(config).await.unwrap();
    let at = garde_daemon::runtime::now();
    publish_at(&database, "deadline-context".into(), "8.8.8.8".into(), at).await;
    assert!(
        database
            .call(move |s| s.claim(at + 8))
            .await
            .unwrap()
            .is_none()
    );
    let delivery = database
        .call(move |s| s.claim(at + 10))
        .await
        .unwrap()
        .unwrap();
    let original = delivery.reply.text.clone();
    assert!(original.contains("lookup pending"));
    assert!(original.contains("/geoip deadline-context"));
    garde_daemon::geoip::run_once(&database, &Provider, at + 11)
        .await
        .unwrap();
    database
        .call(move |s| {
            s.finish(
                delivery.id,
                garde_daemon::delivery::Outcome::Retry {
                    code: "telegram.network",
                    retry_after: 0,
                    uncertain: false,
                },
                at + 11,
            )
        })
        .await
        .unwrap();
    let retry = database
        .call(move |s| s.claim(at + 16))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retry.reply.text, original);
    assert_eq!(
        database
            .call(|s| s.geoip_event("deadline-context"))
            .await
            .unwrap()
            .state,
        garde_contracts::GeoIpState::Retrieved
    );
}

impl garde_daemon::geoip::Provider for Never {
    async fn lookup(
        &self,
        _: &str,
    ) -> Result<garde_contracts::GeoIpDetails, garde_daemon::geoip::LookupFailure> {
        std::future::pending().await
    }
}
#[tokio::test]
async fn hung_lookup_does_not_block_collection_or_private_address_filtering() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut config = configuration(dir.path());
    config.geoip.enabled = true;
    config.geoip.token_file = Some(dir.path().join("geo-token"));
    let bot = Bot::new(config.clone()).unwrap();
    let database = Database::open(config).await.unwrap();
    for (n, ip) in [
        "127.0.0.1",
        "10.1.2.3",
        "100.64.0.1",
        "169.254.1.1",
        "192.0.0.9",
        "192.31.196.1",
        "192.52.193.1",
        "192.175.48.1",
        "2620:4f:8000::1",
        "192.0.2.1",
        "198.18.0.1",
        "198.51.100.1",
        "203.0.113.1",
        "224.1.1.1",
        "240.0.0.1",
        "::1",
        "fc00::1",
        "fe80::1",
        "ff02::1",
        "2001:db8::1",
        "::ffff:10.1.2.3",
        "invalid",
        "[fe80::1%en0]",
    ]
    .iter()
    .enumerate()
    {
        let id = format!("local-{n}");
        bot.publish_event(&database, event(&id, ip)).await.unwrap();
        let context = database.call(move |s| s.geoip_event(&id)).await.unwrap();
        assert_eq!(context.state, garde_contracts::GeoIpState::Missing);
    }
    assert!(
        database
            .call(|s| s.claim_geoip(garde_daemon::runtime::now()))
            .await
            .unwrap()
            .is_none()
    );
    let mut critical = event("critical-now", "8.8.4.4");
    critical.classification.severity = garde_contracts::Severity::Critical;
    bot.publish_event(&database, critical).await.unwrap();
    let worker_database = database.clone();
    let worker = tokio::spawn(async move {
        garde_daemon::geoip::run_once(&worker_database, &Never, garde_daemon::runtime::now()).await
    });
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let health = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        database.call(|s| s.health(garde_daemon::runtime::now())),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(health.pending, 24);
    worker.abort();
    let _ = worker.await;
}

#[tokio::test]
async fn restart_retains_budgets_and_interrupted_lookup_does_not_repeat() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut config = configuration(dir.path());
    config.geoip.enabled = true;
    config.geoip.token_file = Some(dir.path().join("geo-token"));
    let bot = Bot::new(config.clone()).unwrap();
    let database = Database::open(config.clone()).await.unwrap();
    bot.publish_event(&database, event("interrupted", "8.8.8.8"))
        .await
        .unwrap();
    assert_eq!(
        database
            .call(|s| s.claim_geoip(garde_daemon::runtime::now()))
            .await
            .unwrap()
            .as_deref(),
        Some("8.8.8.8")
    );
    drop(database);
    let database = loop {
        match Database::open(config.clone()).await {
            Ok(d) => break d,
            Err("storage.locked") => tokio::time::sleep(std::time::Duration::from_millis(10)).await,
            Err(e) => panic!("{e}"),
        }
    };
    let context = database
        .call(|s| s.geoip_event("interrupted"))
        .await
        .unwrap();
    assert_eq!(context.reason_key.as_str(), "geoip_interrupted");
    let delivery = database
        .call(|s| s.claim(garde_daemon::runtime::now()))
        .await
        .unwrap()
        .unwrap();
    assert!(delivery.reply.text.contains("lookup interrupted"));
    assert!(delivery.reply.text.contains("/geoip interrupted"));
    let health = database
        .call(|s| s.geoip_health(garde_daemon::runtime::now()))
        .await
        .unwrap();
    assert_eq!(health.requests_today, 1);
    assert!(
        database
            .call(|s| s.claim_geoip(garde_daemon::runtime::now() + 2))
            .await
            .unwrap()
            .is_none()
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
async fn saved_context_and_health_enforce_owner_access_and_follow_saved_language() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config = configuration(dir.path());
    let mut bot = Bot::new(config.clone()).unwrap();
    let database = Database::open(config).await.unwrap();
    let update = |id, user, kind, text: &str| serde_json::json!({"update_id":id,"message":{"from":{"id":user,"is_bot":false},"chat":{"id":user,"type":kind},"text":text}});
    for (id, user, kind) in [(0, 123456789, "group"), (1, 987654321, "private")] {
        for (offset, text) in ["/geoip", "/geoip d-1"].into_iter().enumerate() {
            bot.process_update(
                &database,
                &update(id * 2 + offset, user, kind, text),
                &Host,
                "test_bot",
            )
            .await
            .unwrap();
        }
    }
    assert_eq!(
        database
            .call(|s| s.health(garde_daemon::runtime::now()))
            .await
            .unwrap()
            .pending,
        0
    );
    bot.publish_event(&database, event("saved-context", "8.8.8.8"))
        .await
        .unwrap();
    let mut id = 4;
    for locale in garde_entrypoint::catalog()["locales"]
        .as_object()
        .unwrap()
        .keys()
    {
        for text in [
            format!("/language {locale}"),
            "/geoip".into(),
            "/geoip saved-context".into(),
        ] {
            bot.process_update(
                &database,
                &update(id, 123456789, "private", &text),
                &Host,
                "test_bot",
            )
            .await
            .unwrap();
            id += 1;
        }
        let mut texts = Vec::new();
        while let Some(delivery) = database
            .call(|s| s.claim(garde_daemon::runtime::now()))
            .await
            .unwrap()
        {
            texts.push(delivery.reply.text);
            database
                .call(move |s| {
                    s.finish(
                        delivery.id,
                        garde_daemon::delivery::Outcome::Delivered,
                        garde_daemon::runtime::now(),
                    )
                })
                .await
                .unwrap();
        }
        assert!(
            texts
                .iter()
                .any(|s| s.contains(garde_entrypoint::message(locale, "geoip_title")))
        );
        assert!(texts.iter().any(|s| s.contains("saved-context")
            && s.contains(garde_entrypoint::message(locale, "geoip_disabled"))));
        assert!(texts.iter().all(|s| !s.contains("message.unavailable")));
    }
}

struct Unavailable;

#[tokio::test]
async fn exact_event_without_ip_never_inherits_a_colliding_delivery_context() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config = configuration(dir.path());
    let mut bot = Bot::new(config.clone()).unwrap();
    let database = Database::open(config).await.unwrap();
    bot.publish_event(&database, event("source-with-context", "8.8.8.8"))
        .await
        .unwrap();
    let first = database
        .call(|s| s.claim(garde_daemon::runtime::now()))
        .await
        .unwrap()
        .unwrap();
    assert!(first.reply.text.ends_with("ID: d-1"));
    database
        .call(move |s| {
            s.finish(
                first.id,
                garde_daemon::delivery::Outcome::Delivered,
                garde_daemon::runtime::now(),
            )
        })
        .await
        .unwrap();
    let mut without_ip = event("d-1", "8.8.8.8");
    without_ip.evidence.clear();
    bot.publish_event(&database, without_ip).await.unwrap();
    let second = database
        .call(|s| s.claim(garde_daemon::runtime::now()))
        .await
        .unwrap()
        .unwrap();
    database
        .call(move |s| {
            s.finish(
                second.id,
                garde_daemon::delivery::Outcome::Delivered,
                garde_daemon::runtime::now(),
            )
        })
        .await
        .unwrap();
    let update = serde_json::json!({"update_id":0,"message":{"from":{"id":123456789,"is_bot":false},"chat":{"id":123456789,"type":"private"},"text":"/geoip d-1"}});
    bot.process_update(&database, &update, &Host, "test_bot")
        .await
        .unwrap();
    let response = database
        .call(|s| s.claim(garde_daemon::runtime::now()))
        .await
        .unwrap()
        .unwrap();
    assert!(
        response
            .reply
            .text
            .contains("No saved GeoIP context for this event.")
    );
    assert!(!response.reply.text.contains("source-with-context"));
}

#[tokio::test]
async fn notification_delivery_id_reads_the_same_context_as_its_event_id() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut config = configuration(dir.path());
    config.geoip.enabled = true;
    let mut bot = Bot::new(config.clone()).unwrap();
    let database = Database::open(config).await.unwrap();
    bot.publish_event(&database, event("copyable-context", "8.8.8.8"))
        .await
        .unwrap();
    garde_daemon::geoip::run_once(&database, &Provider, garde_daemon::runtime::now())
        .await
        .unwrap();
    let notification = database
        .call(|s| s.claim(garde_daemon::runtime::now()))
        .await
        .unwrap()
        .unwrap();
    let reference = notification
        .reply
        .text
        .lines()
        .last()
        .unwrap()
        .strip_prefix("ID: ")
        .unwrap();
    let update = serde_json::json!({"update_id":0,"message":{"from":{"id":123456789,"is_bot":false},"chat":{"id":123456789,"type":"private"},"text":format!("/geoip {reference}")}});
    database
        .call(move |s| {
            s.finish(
                notification.id,
                garde_daemon::delivery::Outcome::Delivered,
                garde_daemon::runtime::now(),
            )
        })
        .await
        .unwrap();
    bot.process_update(&database, &update, &Host, "test_bot")
        .await
        .unwrap();
    let response = database
        .call(|s| s.claim(garde_daemon::runtime::now()))
        .await
        .unwrap()
        .unwrap();
    assert!(response.reply.text.contains("copyable-context"));
    assert!(response.reply.text.contains("Example network"));
}

impl garde_daemon::geoip::Provider for Unavailable {
    async fn lookup(
        &self,
        _: &str,
    ) -> Result<garde_contracts::GeoIpDetails, garde_daemon::geoip::LookupFailure> {
        Err(garde_daemon::geoip::LookupFailure {
            reason: "geoip_network",
            retry_after: 3600,
        })
    }
}
#[tokio::test]
async fn failures_cool_down_without_retries_and_queue_overflow_remains_observable() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut config = configuration(dir.path());
    config.geoip.enabled = true;
    config.geoip.token_file = Some(dir.path().join("geo-token"));
    let bot = Bot::new(config.clone()).unwrap();
    let database = Database::open(config).await.unwrap();
    bot.publish_event(&database, event("failed", "8.8.8.8"))
        .await
        .unwrap();
    let now = garde_daemon::runtime::now();
    assert!(
        garde_daemon::geoip::run_once(&database, &Unavailable, now)
            .await
            .unwrap()
    );
    bot.publish_event(&database, event("failed-again", "8.8.8.8"))
        .await
        .unwrap();
    assert_eq!(
        database
            .call(|s| s.geoip_event("failed-again"))
            .await
            .unwrap()
            .reason_key
            .as_str(),
        "geoip_network"
    );
    for n in 1..=129 {
        bot.publish_event(
            &database,
            event(&format!("queued-{n}"), &format!("8.8.4.{n}")),
        )
        .await
        .unwrap();
    }
    let health = database
        .call(move |s| s.geoip_health(now + 2))
        .await
        .unwrap();
    assert_eq!(health.queued, 128);
    assert_eq!(health.skipped, 1);
    assert_eq!(health.requests_today, 1);
    assert_eq!(health.reason_key.as_str(), "geoip_network");
    assert!(
        !garde_daemon::geoip::run_once(&database, &Provider, now + 2)
            .await
            .unwrap()
    );
    assert_eq!(
        database
            .call(|s| s.geoip_event("queued-129"))
            .await
            .unwrap()
            .reason_key
            .as_str(),
        "geoip_limit"
    );
}
#[tokio::test]
async fn minute_budget_and_backwards_clock_cannot_start_extra_requests() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut config = configuration(dir.path());
    config.geoip.enabled = true;
    config.geoip.token_file = Some(dir.path().join("geo-token"));
    let bot = Bot::new(config.clone()).unwrap();
    let database = Database::open(config).await.unwrap();
    for n in 1..=31 {
        bot.publish_event(
            &database,
            event(&format!("rate-{n}"), &format!("8.8.4.{n}")),
        )
        .await
        .unwrap();
    }
    let start = (garde_daemon::runtime::now() / 60 + 1) * 60;
    for n in 0..30 {
        assert!(
            garde_daemon::geoip::run_once(&database, &Provider, start + n)
                .await
                .unwrap()
        );
    }
    assert!(
        !garde_daemon::geoip::run_once(&database, &Provider, start + 31)
            .await
            .unwrap()
    );
    assert!(
        !garde_daemon::geoip::run_once(&database, &Provider, start - 1)
            .await
            .unwrap()
    );
    assert_eq!(
        database
            .call(move |s| s.geoip_health(start + 31))
            .await
            .unwrap()
            .reason_key
            .as_str(),
        "geoip_rate_limited"
    );
    assert!(
        garde_daemon::geoip::run_once(&database, &Provider, start + 60)
            .await
            .unwrap()
    );
}

async fn publish_at(database: &Database, id: String, ip: String, at: i64) {
    database
        .call(move |s| {
            s.publish(
                &event(&id, &ip),
                &[garde_daemon::Reply {
                    locale: None,
                    chat_id: "123456789".into(),
                    text: "Observation".into(),
                }],
                at,
            )
        })
        .await
        .unwrap();
}
#[tokio::test]
async fn daily_budget_cache_capacity_and_expiry_remain_bounded_without_losing_saved_context() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut config = configuration(dir.path());
    config.geoip.enabled = true;
    config.geoip.token_file = Some(dir.path().join("geo-token"));
    let database = Database::open(config).await.unwrap();
    let boundary = (garde_daemon::runtime::now() / 86400 + 1) * 86400;
    let start = boundary - 2100;
    for n in 0..1001 {
        let at = start + (n / 30) * 60 + n % 30;
        publish_at(
            &database,
            format!("daily-{n}"),
            format!("8.0.{}.{}", n / 256, n % 256),
            at,
        )
        .await;
        assert_eq!(
            garde_daemon::geoip::run_once(&database, &Provider, at)
                .await
                .unwrap(),
            n < 1000
        );
    }
    assert_eq!(
        database
            .call(move |s| s.geoip_health(boundary - 1))
            .await
            .unwrap()
            .requests_today,
        1000
    );
    for n in 0..26 {
        publish_at(
            &database,
            format!("next-day-{n}"),
            format!("9.0.0.{n}"),
            boundary + n + 1,
        )
        .await;
        assert!(
            garde_daemon::geoip::run_once(&database, &Provider, boundary + n + 1)
                .await
                .unwrap()
        );
    }
    let health = database
        .call(move |s| s.geoip_health(boundary + 27))
        .await
        .unwrap();
    assert_eq!(health.cache_entries, 1024);
    assert_eq!(health.requests_today, 26);
    assert_eq!(
        database
            .call(|s| s.geoip_event("daily-0"))
            .await
            .unwrap()
            .state,
        garde_contracts::GeoIpState::Retrieved
    );
    publish_at(&database, "evicted".into(), "8.0.0.0".into(), boundary + 28).await;
    assert_eq!(
        database
            .call(|s| s.geoip_event("evicted"))
            .await
            .unwrap()
            .reason_key
            .as_str(),
        "geoip_pending"
    );
    assert!(
        garde_daemon::geoip::run_once(&database, &Provider, boundary + 28)
            .await
            .unwrap()
    );
    let expired = boundary + 86461;
    publish_at(&database, "expired-cache".into(), "8.0.0.0".into(), expired).await;
    assert_eq!(
        database
            .call(|s| s.geoip_event("expired-cache"))
            .await
            .unwrap()
            .reason_key
            .as_str(),
        "geoip_pending"
    );
}

#[tokio::test]
async fn local_diagnostics_are_read_only_and_context_follows_event_retention() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config = configuration(dir.path());
    let path = dir.path().join("config.json");
    garde_config::atomic_write(&path, &serde_json::to_vec(&config).unwrap(), 0o600).unwrap();
    let database = Database::open(config).await.unwrap();
    let now = garde_daemon::runtime::now();
    publish_at(&database, "retained".into(), "8.8.8.8".into(), now).await;
    let before = fs::read(dir.path().join("garde.sqlite3")).unwrap();
    for flag in ["--geoip-status", "--geoip-event"] {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_want-garde-daemon"));
        command.arg(flag);
        if flag == "--geoip-event" {
            command.arg("retained");
        }
        let result = command.arg("--config").arg(&path).output().unwrap();
        assert!(result.status.success(), "{:?}", result.stderr);
        let json: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(json["version"], "1");
        assert_eq!(json["server_id"], "server-demo");
    }
    assert_eq!(fs::read(dir.path().join("garde.sqlite3")).unwrap(), before);
    let delivery = database.call(move |s| s.claim(now)).await.unwrap().unwrap();
    database
        .call(move |s| s.finish(delivery.id, garde_daemon::delivery::Outcome::Delivered, now))
        .await
        .unwrap();
    database
        .call(move |s| s.claim(now + 31 * 86400))
        .await
        .unwrap();
    assert_eq!(
        database.call(|s| s.geoip_event("retained")).await.err(),
        Some("geoip.not_found")
    );
}

#[tokio::test]
async fn context_keeps_the_same_locale_as_its_notification_when_preferences_change() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut config = configuration(dir.path());
    config.geoip.enabled = true;
    let bot = Bot::new(config.clone()).unwrap();
    let database = Database::open(config).await.unwrap();
    garde_config::atomic_write(
        &dir.path().join("owner-preferences.json"),
        br#"{"locale":"ru"}"#,
        0o600,
    )
    .unwrap();
    bot.publish_event(&database, event("saved-locale", "8.8.8.8"))
        .await
        .unwrap();
    garde_daemon::geoip::run_once(&database, &Provider, garde_daemon::runtime::now())
        .await
        .unwrap();
    let delivery = database
        .call(|s| s.claim(garde_daemon::runtime::now()))
        .await
        .unwrap()
        .unwrap();
    assert!(
        delivery
            .reply
            .text
            .contains("GeoIP: retrieved from provider")
    );
    assert!(!delivery.reply.text.contains("получено от провайдера"));
}

#[derive(Default)]
struct CapturedTelegram(std::sync::Mutex<Vec<String>>);
impl garde_daemon::runtime::Transport for CapturedTelegram {
    async fn identity(
        &self,
    ) -> Result<garde_daemon::runtime::Identity, garde_daemon::telegram::TransportError> {
        std::future::pending().await
    }
    async fn updates(
        &self,
        _: i64,
    ) -> Result<Vec<serde_json::Value>, garde_daemon::telegram::TransportError> {
        std::future::pending().await
    }
    async fn send(
        &self,
        reply: &garde_daemon::Reply,
    ) -> Result<(), garde_daemon::telegram::TransportError> {
        self.0.lock().unwrap().push(reply.text.clone());
        Ok(())
    }
}
#[tokio::test]
async fn overdue_critical_event_reaches_telegram_transport_while_geoip_provider_is_still_waiting() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut config = configuration(dir.path());
    config.geoip.enabled = true;
    config.geoip.token_file = Some(dir.path().join("geo-token"));
    let database = Database::open(config).await.unwrap();
    let mut event = event("critical-transport", "8.8.8.8");
    event.classification.severity = garde_contracts::Severity::Critical;
    database
        .call(move |s| {
            s.publish(
                &event,
                &[garde_daemon::Reply {
                    locale: Some("en".into()),
                    chat_id: "123456789".into(),
                    text: "Critical event".into(),
                }],
                garde_daemon::runtime::now() - 10,
            )
        })
        .await
        .unwrap();
    let worker_database = database.clone();
    let worker = tokio::spawn(async move {
        garde_daemon::geoip::run_once(&worker_database, &Never, garde_daemon::runtime::now()).await
    });
    let transport = std::sync::Arc::new(CapturedTelegram::default());
    let sender = tokio::spawn(garde_daemon::runtime::send_loop(
        database.clone(),
        transport.clone(),
    ));
    let observed = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if let Some(text) = transport.0.lock().unwrap().first().cloned() {
                break text;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await;
    sender.abort();
    worker.abort();
    let _ = sender.await;
    let _ = worker.await;
    let text = observed.unwrap();
    assert!(text.contains("critical-transport"));
    assert!(text.contains("lookup pending"));
}
