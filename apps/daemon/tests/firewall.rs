use garde_config::Config;
use garde_daemon::{
    ingest::{Collector, Kind},
    runtime::{Database, now},
};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::PermissionsExt,
};

#[tokio::test]
async fn file_bans_reach_durable_delivery_and_resume_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let log = dir.path().join("fail2ban.log");
    fs::write(
        &log,
        "2026-11-03 00:00:00,000 fail2ban.actions [10]: NOTICE [old] Ban 192.0.2.1\n",
    )
    .unwrap();
    let config: Config = serde_json::from_value(serde_json::json!({
        "server_id":"server-demo", "owner_user_id":"123456789", "default_locale":"en",
        "bot_token_file":dir.path().join("token"), "state_dir":dir.path(),
        "fail2ban":{"mode":"file","path":log}
    }))
    .unwrap();
    let database = Database::open(config.clone()).await.unwrap();
    let mut collector = Collector::for_source(Kind::Fail2ban, config.clone(), database.clone());
    collector.scan().await.unwrap();
    assert_eq!(database.call(|s| s.health(now())).await.unwrap().pending, 0);
    writeln!(OpenOptions::new().append(true).open(&log).unwrap(), "2026-11-03 00:01:00,000 fail2ban.actions [10]: NOTICE [sshd] Ban 2001:db8::42 PRIVATE-SUFFIX").unwrap();
    collector.scan().await.unwrap();
    let delivery = database.call(|s| s.claim(now())).await.unwrap().unwrap();
    assert!(delivery.reply.text.contains("2001:db8::42"));
    assert!(delivery.reply.text.contains("sshd"));
    assert!(!delivery.reply.text.contains("PRIVATE-SUFFIX"));
    database
        .call(move |s| {
            s.finish(
                delivery.id,
                garde_daemon::delivery::Outcome::Delivered,
                now(),
            )
        })
        .await
        .unwrap();
    drop(collector);
    let mut collector = Collector::for_source(Kind::Fail2ban, config, database.clone());
    collector.scan().await.unwrap();
    assert_eq!(
        database.call(|s| s.health(now())).await.unwrap().delivered,
        1
    );
    assert_eq!(database.call(|s| s.health(now())).await.unwrap().pending, 0);
}

#[tokio::test]
async fn firewall_backends_deliver_only_valid_blocks_and_never_copy_packet_tails() {
    for (backend, record) in [
        (
            "ufw",
            "[UFW BLOCK] IN=eth0 OUT= SRC=192.0.2.42 DST=198.51.100.2 PROTO=TCP SPT=4242 DPT=443",
        ),
        (
            "firewalld",
            "filter_IN_public_REJECT: IN=eth0 OUT= SRC=2001:db8::42 DST=2001:db8::1 PROTO=UDP SPT=1234 DPT=53",
        ),
        (
            "nftables",
            "WG-DROP: IN=eth0 OUT= SRC=192.0.2.42 DST=198.51.100.2 PROTO=ICMP TYPE=8 CODE=0",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let log = dir.path().join("firewall.log");
        fs::write(&log, "").unwrap();
        let config: Config = serde_json::from_value(serde_json::json!({
            "server_id":"server-demo", "owner_user_id":"123456789", "default_locale":"en",
            "bot_token_file":dir.path().join("token"), "state_dir":dir.path(),
            "firewall":{"source":{"mode":"file","path":log},"backend":backend,"log_prefix":"WG-DROP: "}
        })).unwrap();
        let database = Database::open(config.clone()).await.unwrap();
        let mut collector = Collector::for_source(Kind::Firewall, config, database.clone());
        collector.scan().await.unwrap();
        let mut writer = OpenOptions::new().append(true).open(&log).unwrap();
        writeln!(
            writer,
            "Nov  3 01:00:00 host kernel: {record} PRIVATE-SUFFIX"
        )
        .unwrap();
        writeln!(writer, "Nov  3 01:00:01 host application: {record}").unwrap();
        writeln!(
            writer,
            "Nov  3 01:00:02 host kernel: [UFW ALLOW] SRC=192.0.2.1 PROTO=TCP DPT=443"
        )
        .unwrap();
        writeln!(
            writer,
            "Nov  3 01:00:03 host kernel: {}",
            record
                .replace("192.0.2.42", "999.0.0.1")
                .replace("2001:db8::42", "2001:wrong::1")
        )
        .unwrap();
        collector.scan().await.unwrap();
        assert_eq!(
            database.call(|s| s.health(now())).await.unwrap().pending,
            1,
            "{backend}"
        );
        let delivery = database.call(|s| s.claim(now())).await.unwrap().unwrap();
        assert!(delivery.reply.text.contains(if backend == "firewalld" {
            "2001:db8::42"
        } else {
            "192.0.2.42"
        }));
        assert!(!delivery.reply.text.contains("PRIVATE-SUFFIX"));
    }
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
struct Bans(std::sync::atomic::AtomicUsize);
impl garde_daemon::firewall::BansSource for Bans {
    async fn snapshot(&self) -> Result<garde_contracts::BansSnapshot, &'static str> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(garde_contracts::BansSnapshot {
            version: garde_contracts::WireVersion::V1,
            observed_at: chrono::DateTime::from_timestamp(1793664000, 0).unwrap(),
            jails: vec![garde_contracts::JailBans {
                jail: garde_contracts::Id::new("sshd").unwrap(),
                current_banned: Some(24),
                addresses: (1..=24).map(|n| format!("192.0.2.{n}")).collect(),
                reason_key: None,
            }],
            partial: false,
            log_target: Some("/var/log/fail2ban.log".into()),
            log_level: Some("INFO".into()),
        })
    }
}
#[tokio::test]
async fn bans_are_private_live_and_paginated_before_durable_reply() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config:Config=serde_json::from_value(serde_json::json!({"server_id":"server-demo","owner_user_id":"123456789","default_locale":"en","bot_token_file":dir.path().join("token"),"state_dir":dir.path()})).unwrap();
    let database = Database::open(config.clone()).await.unwrap();
    let mut bot = garde_daemon::Bot::new(config).unwrap();
    let bans = Bans(std::sync::atomic::AtomicUsize::new(0));
    for (id, user, chat) in [(0, 987654321, "private"), (1, 123456789, "group")] {
        let update = serde_json::json!({"update_id":id,"message":{"from":{"id":user,"is_bot":false},"chat":{"id":user,"type":chat},"text":"/bans"}});
        bot.process_update_with_bans(&database, &update, &Host, &bans, "test_bot")
            .await
            .unwrap();
    }
    assert_eq!(bans.0.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(database.call(|s| s.health(now())).await.unwrap().pending, 0);
    let update = serde_json::json!({"update_id":2,"message":{"from":{"id":123456789,"is_bot":false},"chat":{"id":123456789,"type":"private"},"text":"/bans sshd 2"}});
    bot.process_update_with_bans(&database, &update, &Host, &bans, "test_bot")
        .await
        .unwrap();
    let reply = database.call(|s| s.claim(now())).await.unwrap().unwrap();
    assert!(reply.reply.text.contains("192.0.2.21"));
    assert!(!reply.reply.text.contains("192.0.2.10"));
    assert!(reply.reply.text.contains("sshd"));
    assert_eq!(bans.0.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn ordinary_fail2ban_activity_is_not_reported_as_malformed_and_bad_bans_do_not_block_following_events()
 {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let log = dir.path().join("fail2ban.log");
    fs::write(&log, "").unwrap();
    let config:Config=serde_json::from_value(serde_json::json!({"server_id":"server-demo","owner_user_id":"123456789","default_locale":"en","bot_token_file":dir.path().join("token"),"state_dir":dir.path(),"fail2ban":{"mode":"file","path":log}})).unwrap();
    let database = Database::open(config.clone()).await.unwrap();
    let mut collector = Collector::for_source(Kind::Fail2ban, config, database.clone());
    collector.scan().await.unwrap();
    fs::write(
        &log,
        concat!(
            "2026-11-03 01:00:00,000 fail2ban.filter [10]: INFO [sshd] Found 192.0.2.42\n",
            "2026-11-03 01:00:00,001 fail2ban.actions [10]: NOTICE [sshd] Ban 999.0.0.1\n",
            "2026-11-03 01:00:00,002 fail2ban.actions [10]: NOTICE [sshd] Ban 192.0.2.42\n",
            "2026-11-03 01:00:00,003 fail2ban.actions [10]: NOTICE [sshd] Unban 192.0.2.42\n",
            "Nov  3 01:00:01 host fail2ban[10]: [sshd] Restore Ban 2001:db8::42\n",
            "Nov  3 01:00:02 host application[10]: [sshd] Ban 192.0.2.66\n",
        ),
    )
    .unwrap();
    collector.scan().await.unwrap();
    assert_eq!(database.call(|s| s.health(now())).await.unwrap().pending, 3);
    assert_eq!(collector.health().await.unwrap().skipped_records, 1);
}

#[tokio::test]
async fn bans_and_new_event_messages_cover_every_saved_locale_with_isolated_dynamic_values() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config:Config=serde_json::from_value(serde_json::json!({"server_id":"server-demo","owner_user_id":"123456789","default_locale":"en","bot_token_file":dir.path().join("token"),"state_dir":dir.path()})).unwrap();
    let database = Database::open(config.clone()).await.unwrap();
    let mut bot = garde_daemon::Bot::new(config).unwrap();
    let bans = Bans(std::sync::atomic::AtomicUsize::new(0));
    let update = |id, text: String| serde_json::json!({"update_id":id,"message":{"from":{"id":123456789,"is_bot":false},"chat":{"id":123456789,"type":"private"},"text":text}});
    let mut id = 0;
    for locale in garde_entrypoint::catalog()["locales"]
        .as_object()
        .unwrap()
        .keys()
    {
        for key in [
            "bans_title",
            "fail2ban_ban",
            "fail2ban_unban",
            "firewall_block",
            "helper_read_only",
        ] {
            assert_ne!(
                garde_entrypoint::message(locale, key),
                "message.unavailable",
                "{locale}/{key}"
            );
        }
        bot.process_update_with_bans(
            &database,
            &update(id, format!("/language {locale}")),
            &Host,
            &bans,
            "test_bot",
        )
        .await
        .unwrap();
        id += 1;
        let reply = database.call(|s| s.claim(now())).await.unwrap().unwrap();
        database
            .call(move |s| s.finish(reply.id, garde_daemon::delivery::Outcome::Delivered, now()))
            .await
            .unwrap();
        bot.process_update_with_bans(
            &database,
            &update(id, "/bans sshd 1".into()),
            &Host,
            &bans,
            "test_bot",
        )
        .await
        .unwrap();
        id += 1;
        let reply = database.call(|s| s.claim(now())).await.unwrap().unwrap();
        assert!(
            reply
                .reply
                .text
                .contains(garde_entrypoint::message(locale, "bans_title"))
        );
        assert!(reply.reply.text.contains("\u{2068}192.0.2.1\u{2069}"));
        database
            .call(move |s| s.finish(reply.id, garde_daemon::delivery::Outcome::Delivered, now()))
            .await
            .unwrap();
    }
}

struct Target {
    changed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    old: std::path::PathBuf,
    new: std::path::PathBuf,
}
impl garde_daemon::firewall::BansSource for Target {
    async fn snapshot(&self) -> Result<garde_contracts::BansSnapshot, &'static str> {
        let mut result = garde_daemon::firewall::BansSource::snapshot(&Bans(
            std::sync::atomic::AtomicUsize::new(0),
        ))
        .await?;
        result.log_target = Some(
            if self.changed.load(std::sync::atomic::Ordering::SeqCst) {
                &self.new
            } else {
                &self.old
            }
            .display()
            .to_string(),
        );
        Ok(result)
    }
}
#[tokio::test]
async fn an_observed_logtarget_change_keeps_the_old_source_and_a_persistent_coverage_gap() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let old = dir.path().join("old.log");
    let new = dir.path().join("new.log");
    fs::write(&old, "").unwrap();
    fs::write(&new, "").unwrap();
    let config:Config=serde_json::from_value(serde_json::json!({"server_id":"server-demo","owner_user_id":"123456789","default_locale":"en","bot_token_file":dir.path().join("token"),"state_dir":dir.path()})).unwrap();
    let database = Database::open(config.clone()).await.unwrap();
    let changed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut collector = Collector::with_sources(
        Kind::Fail2ban,
        config.clone(),
        database.clone(),
        garde_daemon::ingest::Journal::default(),
        Target {
            changed: changed.clone(),
            old: old.clone(),
            new: new.clone(),
        },
    );
    collector.scan().await.unwrap();
    changed.store(true, std::sync::atomic::Ordering::SeqCst);
    tokio::time::sleep(std::time::Duration::from_secs(16)).await;
    collector.scan().await.unwrap();
    let health = collector.health().await.unwrap();
    assert_eq!(health.file_path.as_deref(), old.to_str());
    assert_eq!(
        health.reason_key.unwrap().as_str(),
        "fail2ban.target_changed"
    );
    assert_eq!(health.gap_count, 1);
    drop(collector);
    let mut restarted = Collector::with_sources(
        Kind::Fail2ban,
        config,
        database,
        garde_daemon::ingest::Journal::default(),
        Target { changed, old, new },
    );
    restarted.scan().await.unwrap();
    assert_eq!(restarted.health().await.unwrap().gap_count, 1);
}

struct EmptyJournal;
impl garde_daemon::ingest::JournalSource for EmptyJournal {
    async fn query(
        &self,
        _: garde_daemon::ingest::JournalQuery,
    ) -> Result<Vec<serde_json::Value>, &'static str> {
        Ok(vec![])
    }
}
struct MissingHelper;
impl garde_daemon::firewall::BansSource for MissingHelper {
    async fn snapshot(&self) -> Result<garde_contracts::BansSnapshot, &'static str> {
        Err("helper.unavailable")
    }
}
#[tokio::test]
async fn auto_does_not_pin_an_unverified_journal_while_the_helper_is_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config:Config=serde_json::from_value(serde_json::json!({"server_id":"server-demo","owner_user_id":"123456789","default_locale":"en","bot_token_file":dir.path().join("token"),"state_dir":dir.path()})).unwrap();
    let database = Database::open(config.clone()).await.unwrap();
    let mut collector = Collector::with_sources(
        Kind::Fail2ban,
        config,
        database,
        EmptyJournal,
        MissingHelper,
    );
    assert!(collector.scan().await.is_err());
    assert!(collector.scan().await.is_err());
    let health = collector.health().await.unwrap();
    assert_eq!(health.source, garde_contracts::SshSourceKind::Unselected);
    assert_eq!(health.reason_key.unwrap().as_str(), "helper.unavailable");
}

#[tokio::test]
async fn damaged_source_metadata_is_visible_without_stopping_the_writer_or_other_collectors() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let bans = dir.path().join("fail2ban.log");
    let packets = dir.path().join("ufw.log");
    fs::write(&bans, "").unwrap();
    fs::write(&packets, "").unwrap();
    let config:Config=serde_json::from_value(serde_json::json!({"server_id":"server-demo","owner_user_id":"123456789","default_locale":"en","bot_token_file":dir.path().join("token"),"state_dir":dir.path(),"fail2ban":{"mode":"file","path":bans},"firewall":{"source":{"mode":"file","path":packets},"backend":"ufw"}})).unwrap();
    let database = Database::open(config.clone()).await.unwrap();
    let mut fail2ban = Collector::for_source(Kind::Fail2ban, config.clone(), database.clone());
    let mut firewall = Collector::for_source(Kind::Firewall, config.clone(), database.clone());
    fail2ban.scan().await.unwrap();
    firewall.scan().await.unwrap();
    let external = rusqlite::Connection::open(dir.path().join("garde.sqlite3")).unwrap();
    let saved: String = external
        .query_row(
            "SELECT value FROM meta WHERE key='fail2ban.state'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&saved).unwrap();
    value["error"] = serde_json::json!(format!("ssh.{}", "a".repeat(124)));
    external
        .execute(
            "UPDATE meta SET value=?1 WHERE key='fail2ban.state'",
            [value.to_string()],
        )
        .unwrap();
    drop(external);
    let health = database
        .call(move |s| s.firewall_health(&config, now()))
        .await
        .unwrap();
    assert_eq!(
        health.fail2ban.state,
        garde_contracts::SshStatus::Unavailable
    );
    assert_eq!(
        health.fail2ban.reason_key.unwrap().as_str(),
        "fail2ban.state"
    );
    assert_eq!(health.firewall.state, garde_contracts::SshStatus::Healthy);
    writeln!(
        OpenOptions::new().append(true).open(&packets).unwrap(),
        "kernel: [UFW BLOCK] SRC=192.0.2.42 PROTO=TCP DPT=443"
    )
    .unwrap();
    firewall.scan().await.unwrap();
    assert_eq!(database.call(|s| s.health(now())).await.unwrap().pending, 1);
}

#[tokio::test]
async fn failed_ban_checkpoint_does_not_acknowledge_the_input_or_lose_a_delivery() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let log = dir.path().join("fail2ban.log");
    fs::write(&log, "").unwrap();
    let config:Config=serde_json::from_value(serde_json::json!({"server_id":"server-demo","owner_user_id":"123456789","default_locale":"en","bot_token_file":dir.path().join("token"),"state_dir":dir.path(),"fail2ban":{"mode":"file","path":log}})).unwrap();
    let database = Database::open(config.clone()).await.unwrap();
    let mut collector = Collector::for_source(Kind::Fail2ban, config, database.clone());
    collector.scan().await.unwrap();
    let external = rusqlite::Connection::open(dir.path().join("garde.sqlite3")).unwrap();
    external.execute_batch("CREATE TRIGGER fail_checkpoint BEFORE UPDATE ON meta WHEN new.key='fail2ban.state' BEGIN SELECT RAISE(FAIL,'injected checkpoint failure'); END;").unwrap();
    writeln!(
        OpenOptions::new().append(true).open(&log).unwrap(),
        "2026-11-03 01:00:00,000 fail2ban.actions [10]: NOTICE [sshd] Ban 192.0.2.42"
    )
    .unwrap();
    assert!(collector.scan().await.is_err());
    assert_eq!(database.call(|s| s.health(now())).await.unwrap().pending, 0);
    external
        .execute_batch("DROP TRIGGER fail_checkpoint;")
        .unwrap();
    drop(external);
    collector.scan().await.unwrap();
    collector.scan().await.unwrap();
    assert_eq!(database.call(|s| s.health(now())).await.unwrap().pending, 1);
}

#[tokio::test]
async fn observed_logging_loss_remains_a_gap_after_recovery_and_restart() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    struct Logging {
        path: String,
        insufficient: Arc<AtomicBool>,
    }
    impl garde_daemon::firewall::BansSource for Logging {
        async fn snapshot(&self) -> Result<garde_contracts::BansSnapshot, &'static str> {
            Ok(garde_contracts::BansSnapshot {
                version: garde_contracts::WireVersion::V1,
                observed_at: chrono::Utc::now(),
                jails: vec![],
                partial: false,
                log_target: Some(self.path.clone()),
                log_level: Some(
                    if self.insufficient.load(Ordering::SeqCst) {
                        "WARNING"
                    } else {
                        "INFO"
                    }
                    .into(),
                ),
            })
        }
    }
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let log = dir.path().join("fail2ban.log");
    fs::write(&log, "").unwrap();
    let config: Config = serde_json::from_value(serde_json::json!({
        "server_id":"server-demo", "owner_user_id":"123456789", "default_locale":"en",
        "bot_token_file":dir.path().join("token"), "state_dir":dir.path()
    }))
    .unwrap();
    let database = Database::open(config.clone()).await.unwrap();
    let insufficient = Arc::new(AtomicBool::new(false));
    let source = || Logging {
        path: log.to_str().unwrap().into(),
        insufficient: insufficient.clone(),
    };
    let mut collector = Collector::with_sources(
        Kind::Fail2ban,
        config.clone(),
        database.clone(),
        garde_daemon::ingest::Journal::default(),
        source(),
    );
    collector.scan().await.unwrap();
    assert_eq!(
        collector.health().await.unwrap().state,
        garde_contracts::SshStatus::Healthy
    );
    insufficient.store(true, Ordering::SeqCst);
    tokio::time::sleep(std::time::Duration::from_secs(16)).await;
    collector.scan().await.unwrap();
    let missing = collector.health().await.unwrap();
    assert_eq!(missing.gap_count, 1);
    assert_eq!(
        missing.last_gap.unwrap().as_str(),
        "fail2ban.logging_insufficient"
    );
    collector.scan().await.unwrap();
    assert_eq!(collector.health().await.unwrap().gap_count, 1);
    insufficient.store(false, Ordering::SeqCst);
    tokio::time::sleep(std::time::Duration::from_secs(16)).await;
    collector.scan().await.unwrap();
    drop(collector);
    let mut collector = Collector::with_sources(
        Kind::Fail2ban,
        config,
        database,
        garde_daemon::ingest::Journal::default(),
        source(),
    );
    collector.scan().await.unwrap();
    let recovered = collector.health().await.unwrap();
    assert_eq!(recovered.state, garde_contracts::SshStatus::Degraded);
    assert_eq!(recovered.gap_count, 1);
    assert_eq!(
        recovered.last_gap.unwrap().as_str(),
        "fail2ban.logging_insufficient"
    );
    assert!(recovered.reason_key.is_none());
}
