use garde_daemon::{StatusSource, host::LinuxHost};
use serde_json::json;
use std::fs;

#[tokio::test]
async fn host_snapshot_exposes_core_count_normalized_load_and_explicit_missing_filesystem() {
    let directory = tempfile::tempdir().unwrap();
    let proc = directory.path().join("proc");
    fs::create_dir_all(proc.join("sys/kernel")).unwrap();
    for (name, value) in [
        ("stat", "cpu 100 0 100 800 0 0 0 0\ncpu0 0\ncpu1 0\n"),
        ("loadavg", "2.00 1.00 0.50 1/2 3\n"),
        ("meminfo", "MemTotal: 1024 kB\nMemAvailable: 512 kB\n"),
        ("uptime", "12.0 2.0\n"),
    ] {
        fs::write(proc.join(name), value).unwrap();
    }
    let os = directory.path().join("os-release");
    fs::write(&os, "ID=unknown\nPRETTY_NAME=\"Test Linux\"\n").unwrap();
    let missing = directory.path().join("absent");
    let host = LinuxHost::from_paths(proc, os, missing.clone());
    let snapshot = serde_json::to_value(host.snapshot().await).unwrap();
    assert_eq!(snapshot["cpu_count"], json!({"type":"available","value":2}));
    assert_eq!(
        snapshot["load_per_core"],
        json!({"type":"available","value":[1.0,0.5,0.25]})
    );
    assert_eq!(snapshot["disks"][0]["path"], missing.to_str().unwrap());
    assert_eq!(snapshot["disks"][0]["space"]["type"], "unavailable");
    assert_eq!(snapshot["disks"][0]["inodes"]["type"], "unavailable");
    assert_eq!(snapshot["reboot_required"]["type"], "unavailable");
    assert_eq!(snapshot["memory"]["value"]["available_bytes"], 524288);
}

fn sample(cpu: f64) -> garde_contracts::HostSnapshot {
    serde_json::from_value(json!({
        "observed_at":"2026-11-03T00:00:00Z",
        "hostname":{"type":"available","value":"test-host"},
        "os":{"type":"available","value":"Test Linux"},
        "kernel":{"type":"available","value":"test"},
        "uptime_seconds":{"type":"available","value":100},
        "cpu_usage_percent":{"type":"available","value":cpu},
        "cpu_count":{"type":"available","value":2},
        "load_average":{"type":"available","value":[1.0,0.5,0.25]},
        "load_per_core":{"type":"available","value":[0.5,0.25,0.125]},
        "memory":{"type":"available","value":{"total_bytes":1000,"available_bytes":500}},
        "root_disk":{"type":"available","value":{"total_bytes":1000,"available_bytes":500}},
        "disks":[{"path":"/","space":{"type":"available","value":{"total_bytes":1000,"used_bytes":500,"available_bytes":500}},"inodes":{"type":"available","value":{"total":1000,"available":500}}}],
        "reboot_required":{"type":"available","value":false}
    })).unwrap()
}

#[tokio::test]
async fn confirmed_cpu_warning_survives_collector_restart_and_recovers_without_repeats() {
    use garde_daemon::{delivery::Outcome, host_health::Monitor, runtime::Database};
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config: garde_config::Config = serde_json::from_value(json!({
        "server_id":"test-host","owner_user_id":"123456789","default_locale":"en",
        "bot_token_file":directory.path().join("token"),"state_dir":directory.path()
    }))
    .unwrap();
    let database = Database::open(config.clone()).await.unwrap();
    let mut monitor = Monitor::new(config.clone(), database.clone());
    monitor.observe(sample(50.0)).await.unwrap();
    monitor.observe(sample(95.0)).await.unwrap();
    assert_eq!(database.call(|s| s.health(1000)).await.unwrap().pending, 0);
    monitor.observe(sample(95.0)).await.unwrap();
    let warning = database
        .call(|s| s.claim(garde_daemon::runtime::now()))
        .await
        .unwrap()
        .unwrap();
    assert!(warning.reply.text.contains("CPU"));
    assert!(warning.reply.text.contains("95"));
    database
        .call(move |s| s.finish(warning.id, Outcome::Delivered, garde_daemon::runtime::now()))
        .await
        .unwrap();
    let mut monitor = Monitor::new(config, database.clone());
    monitor.observe(sample(99.0)).await.unwrap();
    monitor.observe(sample(86.0)).await.unwrap();
    assert!(
        database
            .call(|s| s.claim(garde_daemon::runtime::now()))
            .await
            .unwrap()
            .is_none()
    );
    monitor.observe(sample(85.0)).await.unwrap();
    let recovery = database
        .call(|s| s.claim(garde_daemon::runtime::now()))
        .await
        .unwrap()
        .unwrap();
    assert!(recovery.reply.text.contains("CPU"));
    assert_ne!(recovery.id, warning.id);
    database
        .call(move |s| {
            s.finish(
                recovery.id,
                Outcome::Delivered,
                garde_daemon::runtime::now(),
            )
        })
        .await
        .unwrap();
    monitor.observe(sample(50.0)).await.unwrap();
    assert!(
        database
            .call(|s| s.claim(garde_daemon::runtime::now()))
            .await
            .unwrap()
            .is_none()
    );
}

struct Snapshot;
impl garde_daemon::StatusSource for Snapshot {
    async fn snapshot(&self) -> garde_contracts::HostSnapshot {
        sample(25.0)
    }
}
#[tokio::test]
async fn status_shows_filesystems_inodes_and_reboot_state_in_the_saved_language() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config=serde_json::from_value(json!({"server_id":"test-host","owner_user_id":"123456789","default_locale":"en","bot_token_file":directory.path().join("token"),"state_dir":directory.path()})).unwrap();
    let mut bot = garde_daemon::Bot::new(config).unwrap();
    let update = json!({"message":{"from":{"id":123456789,"is_bot":false},"chat":{"id":123456789,"type":"private"},"text":"/status"}});
    let reply = bot
        .handle(&update, &Snapshot, "test_bot")
        .await
        .unwrap()
        .unwrap();
    assert!(reply.text.contains("Inodes"));
    assert!(reply.text.contains("Reboot"));
    assert!(reply.text.contains("500 / 1000"));
    assert!(!reply.text.contains("message.unavailable"));
}

#[tokio::test]
async fn disk_and_inode_transitions_are_immediate_and_unknown_does_not_clear_them() {
    use garde_contracts::{HostLevel, Measurement};
    use garde_daemon::{host_health::Monitor, runtime::Database};
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config: garde_config::Config=serde_json::from_value(json!({"server_id":"test-host","owner_user_id":"123456789","default_locale":"en","bot_token_file":dir.path().join("token"),"state_dir":dir.path()})).unwrap();
    let database = Database::open(config.clone()).await.unwrap();
    let mut monitor = Monitor::new(config.clone(), database.clone());
    let mut high = sample(20.0);
    if let Measurement::Available { value } = &mut high.disks[0].space {
        value.used_bytes = 950;
        value.available_bytes = 50;
    }
    if let Measurement::Available { value } = &mut high.disks[0].inodes {
        value.available = 50;
    }
    monitor.observe(high.clone()).await.unwrap();
    let c = config.clone();
    let health = database
        .call(move |s| s.host_health(&c, garde_daemon::runtime::now()))
        .await
        .unwrap();
    assert_eq!(health.alerts.len(), 2);
    assert!(health.alerts.iter().all(|a| a.level == HostLevel::Critical));
    let mut unknown = high.clone();
    unknown.disks[0].space = Measurement::Unavailable {
        reason_key: garde_contracts::Id::new("source.read").unwrap(),
    };
    monitor.observe(unknown.clone()).await.unwrap();
    let count = database
        .call(|s| s.health(garde_daemon::runtime::now()))
        .await
        .unwrap()
        .pending;
    monitor.observe(unknown).await.unwrap();
    assert_eq!(
        count,
        database
            .call(|s| s.health(garde_daemon::runtime::now()))
            .await
            .unwrap()
            .pending
    );
    let c = config.clone();
    let health = database
        .call(move |s| s.host_health(&c, garde_daemon::runtime::now()))
        .await
        .unwrap();
    let disk = health
        .alerts
        .iter()
        .find(|a| a.metric.as_str() == "disk")
        .unwrap();
    assert_eq!(disk.level, HostLevel::Critical);
    assert!(disk.reason_key.is_some());
    monitor.observe(high).await.unwrap();
    monitor.observe(sample(20.0)).await.unwrap();
    let health = database
        .call(move |s| s.host_health(&config, garde_daemon::runtime::now()))
        .await
        .unwrap();
    assert!(health.alerts.is_empty());
}

#[tokio::test]
async fn reboot_requires_a_supported_source_and_a_missing_producer_is_unknown() {
    use garde_contracts::Measurement;
    use garde_daemon::host::HostSources;
    let dir = tempfile::tempdir().unwrap();
    let os = dir.path().join("os-release");
    fs::write(&os, "ID=ubuntu\n").unwrap();
    let marker = dir.path().join("reboot-required");
    let producer = dir.path().join("producer");
    let host = LinuxHost::with_sources(
        HostSources {
            proc_root: dir.path().join("proc"),
            os_release: os,
            reboot_marker: marker.clone(),
            reboot_producer: producer.clone(),
            ..Default::default()
        },
        vec![],
    );
    assert!(matches!(
        host.snapshot().await.reboot_required,
        Measurement::Unavailable { .. }
    ));
    fs::write(&producer, "test").unwrap();
    assert_eq!(
        host.snapshot().await.reboot_required,
        Measurement::Available { value: false }
    );
    fs::write(&marker, "").unwrap();
    assert_eq!(
        host.snapshot().await.reboot_required,
        Measurement::Available { value: true }
    );
}

#[tokio::test]
async fn a_slow_filesystem_probe_cannot_hold_the_other_measurements_or_process_shutdown() {
    use garde_contracts::Measurement;
    use garde_daemon::host::HostSources;
    use std::{
        os::unix::fs::PermissionsExt,
        time::{Duration, Instant},
    };
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("probe");
    fs::write(&program, "#!/bin/sh\nexec /bin/sleep 20\n").unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(
        dir.path().join("meminfo"),
        "MemTotal: 1024 kB\nMemAvailable: 512 kB\n",
    )
    .unwrap();
    let host = LinuxHost::with_sources(
        HostSources {
            proc_root: dir.path().into(),
            filesystem_program: program,
            ..Default::default()
        },
        vec!["/".into()],
    );
    let start = Instant::now();
    let result = host.snapshot().await;
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(matches!(result.memory, Measurement::Available { .. }));
    let Measurement::Unavailable { reason_key } = &result.disks[0].space else {
        panic!("unexpected_filesystem_measurement")
    };
    assert_eq!(reason_key.as_str(), "host.probe_timeout");
    drop(host);
}

#[test]
fn old_snapshots_decode_new_measurements_as_unknown_and_host_configuration_is_validated() {
    let mut old = serde_json::to_value(sample(0.0)).unwrap();
    for key in ["cpu_count", "load_per_core", "disks", "reboot_required"] {
        old.as_object_mut().unwrap().remove(key);
    }
    let decoded: garde_contracts::HostSnapshot = serde_json::from_value(old).unwrap();
    assert!(matches!(
        decoded.cpu_count,
        garde_contracts::Measurement::Unavailable { .. }
    ));
    let mut c = garde_config::HostHealthConfig::default();
    assert!(c.valid());
    c.interval_seconds = 0;
    assert!(!c.valid());
    c.interval_seconds = 60;
    c.disk_paths = vec!["relative".into()];
    assert!(!c.valid());
    c.disk_paths = vec!["/".into(), "/".into()];
    assert!(!c.valid());
    c.disk_paths = vec!["/".into()];
    c.cpu_warn_percent = f64::NAN;
    assert!(!c.valid());
}

#[tokio::test]
async fn failed_storage_does_not_accept_a_transition_or_lose_its_confirmation() {
    use garde_daemon::{host_health::Monitor, runtime::Database};
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config:garde_config::Config=serde_json::from_value(json!({"server_id":"test-host","owner_user_id":"123456789","default_locale":"en","bot_token_file":dir.path().join("token"),"state_dir":dir.path()})).unwrap();
    let database = Database::open(config.clone()).await.unwrap();
    let mut monitor = Monitor::new(config.clone(), database.clone());
    monitor.observe(sample(50.0)).await.unwrap();
    monitor.observe(sample(95.0)).await.unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o500)).unwrap();
    let failed = monitor.observe(sample(95.0)).await;
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    assert!(failed.is_err());
    assert_eq!(
        database
            .call(|s| s.health(garde_daemon::runtime::now()))
            .await
            .unwrap()
            .pending,
        0
    );
    let c = config.clone();
    assert!(
        database
            .call(move |s| s.host_health(&c, garde_daemon::runtime::now()))
            .await
            .unwrap()
            .alerts
            .is_empty()
    );
    monitor.observe(sample(95.0)).await.unwrap();
    assert_eq!(
        database
            .call(|s| s.health(garde_daemon::runtime::now()))
            .await
            .unwrap()
            .pending,
        1
    );
}

#[tokio::test]
async fn host_diagnostics_are_read_only_json_without_a_token() {
    use garde_daemon::{host_health::Monitor, runtime::Database};
    use std::{os::unix::fs::PermissionsExt, process::Command};
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config:garde_config::Config=serde_json::from_value(json!({"server_id":"test-host","owner_user_id":"123456789","default_locale":"en","bot_token_file":dir.path().join("absent-token"),"state_dir":dir.path()})).unwrap();
    let path = dir.path().join("config.json");
    fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let database = Database::open(config.clone()).await.unwrap();
    let mut monitor = Monitor::new(config, database);
    let mut snapshot = sample(20.0);
    snapshot.observed_at = std::time::SystemTime::now().into();
    monitor.observe(snapshot).await.unwrap();
    let before = fs::read(dir.path().join("garde.sqlite3")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_want-garde-daemon"))
        .arg("--host-status")
        .arg("--config")
        .arg(path)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let health: garde_contracts::HostHealth = serde_json::from_slice(&output.stdout).unwrap();
    assert!(health.alerts.is_empty());
    assert!(!health.stale);
    assert_eq!(before, fs::read(dir.path().join("garde.sqlite3")).unwrap());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("123456789"));
}

#[tokio::test]
async fn status_and_threshold_messages_cover_all_locales_and_rtl() {
    use garde_daemon::{host_health::Monitor, runtime::Database};
    use std::os::unix::fs::PermissionsExt;
    for (locale, copy) in garde_entrypoint::catalog()["locales"].as_object().unwrap() {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let config:garde_config::Config=serde_json::from_value(json!({"server_id":"test-host","owner_user_id":"123456789","default_locale":locale,"bot_token_file":dir.path().join("token"),"state_dir":dir.path()})).unwrap();
        let mut bot = garde_daemon::Bot::new(config.clone()).unwrap();
        let update = json!({"message":{"from":{"id":123456789,"is_bot":false},"chat":{"id":123456789,"type":"private"},"text":"/status"}});
        let reply = bot
            .handle(&update, &Snapshot, "test_bot")
            .await
            .unwrap()
            .unwrap();
        assert!(reply.text.contains(copy["host_inode"].as_str().unwrap()));
        assert!(
            reply
                .text
                .contains(copy["host_not_required"].as_str().unwrap())
        );
        assert!(!reply.text.contains("message.unavailable"));
        assert!(reply.text.encode_utf16().count() <= 4096);
        let database = Database::open(config.clone()).await.unwrap();
        let mut monitor = Monitor::new(config, database.clone());
        monitor.observe(sample(95.0)).await.unwrap();
        monitor.observe(sample(95.0)).await.unwrap();
        let reply = database
            .call(|s| s.claim(garde_daemon::runtime::now()))
            .await
            .unwrap()
            .unwrap();
        assert!(
            reply
                .reply
                .text
                .contains(copy["host_warning"].as_str().unwrap())
        );
        assert!(!reply.reply.text.contains("message.unavailable"));
        if ["ar", "he"].contains(&locale.as_str()) {
            assert!(reply.reply.text.contains('\u{2068}'));
        }
    }
}

#[tokio::test]
async fn memory_and_load_require_two_samples_and_restart_resets_unconfirmed_peaks() {
    use garde_contracts::Measurement;
    use garde_daemon::{host_health::Monitor, runtime::Database};
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config:garde_config::Config=serde_json::from_value(json!({"server_id":"test-host","owner_user_id":"123456789","default_locale":"en","bot_token_file":dir.path().join("token"),"state_dir":dir.path()})).unwrap();
    let db = Database::open(config.clone()).await.unwrap();
    let mut monitor = Monitor::new(config.clone(), db.clone());
    let mut high = sample(20.0);
    if let Measurement::Available { value } = &mut high.memory {
        value.available_bytes = 100;
    }
    high.load_per_core = Measurement::Available {
        value: [2.0, 1.0, 0.5],
    };
    monitor.observe(high.clone()).await.unwrap();
    let mut monitor = Monitor::new(config.clone(), db.clone());
    monitor.observe(high.clone()).await.unwrap();
    assert_eq!(
        db.call(|s| s.health(garde_daemon::runtime::now()))
            .await
            .unwrap()
            .pending,
        0
    );
    monitor.observe(high).await.unwrap();
    let health = db
        .call(move |s| s.host_health(&config, garde_daemon::runtime::now()))
        .await
        .unwrap();
    assert_eq!(
        health
            .alerts
            .iter()
            .map(|a| a.metric.as_str())
            .collect::<Vec<_>>(),
        vec!["memory", "load"]
    );
}

#[derive(Default)]
struct Transport(std::sync::Mutex<Vec<String>>);
impl garde_daemon::runtime::Transport for Transport {
    async fn identity(
        &self,
    ) -> Result<garde_daemon::runtime::Identity, garde_daemon::telegram::TransportError> {
        Ok(garde_daemon::runtime::Identity {
            id: "12345".into(),
            username: "test_bot".into(),
        })
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
async fn an_observation_reaches_the_transport_through_real_sqlite_after_offline_collection() {
    use garde_daemon::{
        host_health::Monitor,
        runtime::{Database, send_loop},
    };
    use std::{os::unix::fs::PermissionsExt, sync::Arc, time::Duration};
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config:garde_config::Config=serde_json::from_value(json!({"server_id":"test-host","owner_user_id":"123456789","default_locale":"en","bot_token_file":dir.path().join("absent-token"),"state_dir":dir.path()})).unwrap();
    let db = Database::open(config.clone()).await.unwrap();
    let mut monitor = Monitor::new(config, db.clone());
    monitor.observe(sample(95.0)).await.unwrap();
    monitor.observe(sample(95.0)).await.unwrap();
    assert_eq!(
        db.call(|s| s.health(garde_daemon::runtime::now()))
            .await
            .unwrap()
            .pending,
        1
    );
    let transport = Arc::new(Transport::default());
    let sender = tokio::spawn(send_loop(db.clone(), transport.clone()));
    tokio::time::timeout(Duration::from_secs(3), async {
        while db
            .call(|s| s.health(garde_daemon::runtime::now()))
            .await
            .unwrap()
            .delivered
            != 1
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    sender.abort();
    let _ = sender.await;
    let messages = transport.0.lock().unwrap();
    assert_eq!(messages.len(), 1);
    assert!(messages[0].contains("CPU"));
    assert!(messages[0].contains("Resource threshold exceeded"));
    assert!(messages[0].contains("Event: host-1"));
}
