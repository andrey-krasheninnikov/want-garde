use garde_config::{Config, SshConfig};
use garde_contracts::Id;
use garde_daemon::{runtime::Database, ssh::Collector};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::PermissionsExt,
};

#[tokio::test]
async fn first_run_starts_at_tail_and_new_login_survives_restart_without_duplicate_delivery() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let log = dir.path().join("auth.log");
    fs::write(
        &log,
        "old sshd[10]: Accepted password for old from 192.0.2.1 port 22 ssh2\n",
    )
    .unwrap();
    let config = Config {
        server_id: Id::new("server-demo").unwrap(),
        owner_user_id: Some("123456789".into()),
        bot_token_file: dir.path().join("token"),
        state_dir: dir.path().into(),
        fail2ban: Default::default(),
        firewall: Default::default(),
        default_locale: "en".into(),
        ssh: SshConfig::File { path: log.clone() },
    };
    let database = Database::open(config.clone()).await.unwrap();
    let mut collector = Collector::new(config.clone(), database.clone());
    collector.scan().await.unwrap();
    assert_eq!(database.call(|s| s.health(1000)).await.unwrap().pending, 0);
    writeln!(OpenOptions::new().append(true).open(&log).unwrap(), "2026-11-03T00:00:00Z host sshd[11]: Accepted publickey for alice from 2001:db8::1 port 4242 ssh2").unwrap();
    collector.scan().await.unwrap();
    assert_eq!(database.call(|s| s.health(1000)).await.unwrap().pending, 1);
    let message = database
        .call(|s| s.claim(garde_daemon::runtime::now()))
        .await
        .unwrap()
        .unwrap();
    assert!(message.reply.text.contains("alice"));
    assert!(message.reply.text.contains("2001:db8::1"));
    assert!(!message.reply.text.contains("old sshd"));
    drop(collector);
    let mut collector = Collector::new(config, database.clone());
    collector.scan().await.unwrap();
    let health = database.call(|s| s.health(1000)).await.unwrap();
    assert_eq!(health.pending + health.in_flight, 1);
}

#[tokio::test]
async fn six_ssh_events_ignore_other_programs_and_do_not_copy_untrusted_log_tails() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let log = dir.path().join("auth.log");
    fs::write(&log, "").unwrap();
    let config = Config {
        server_id: Id::new("server-demo").unwrap(),
        owner_user_id: Some("123456789".into()),
        bot_token_file: dir.path().join("token"),
        state_dir: dir.path().into(),
        fail2ban: Default::default(),
        firewall: Default::default(),
        default_locale: "en".into(),
        ssh: SshConfig::File { path: log.clone() },
    };
    let database = Database::open(config.clone()).await.unwrap();
    let mut collector = Collector::new(config, database.clone());
    collector.scan().await.unwrap();
    fs::write(&log, concat!(
        "Nov  3 01:00:00 host sshd[21]: Accepted publickey for alice from 192.0.2.10 port 4242 ssh2: PRIVATE-SUFFIX\n",
        "Nov  3 01:00:01 host sshd[22]: Failed password for invalid user mallory from 2001:db8::2 port 5555 ssh2\n",
        "Nov  3 01:00:02 host sshd-session[23]: Invalid user nobody from 192.0.2.12 port 12345\n",
        "Nov  3 01:00:03 host sshd[24]: pam_unix(sshd:session): session closed for user alice\n",
        "Nov  3 01:00:04 host sshd-auth[25]: Connection closed by authenticating user bob 2001:db8::3 port 4444 [preauth]\n",
        "Nov  3 01:00:05 host sshd[26]: Unable to negotiate with 192.0.2.14 port 9876: no matching host key type found\n",
        "Nov  3 01:00:06 host application[27]: Accepted password for forged from 192.0.2.15 port 4242 ssh2\n",
        "Nov  3 01:00:07 host sshd[28]: Accepted password for bad from 999.0.0.1 port 4242 ssh2\n",
        "Nov  3 01:00:08 host sshd[29]: an unsupported future record\n",
    )).unwrap();
    collector.scan().await.unwrap();
    assert_eq!(database.call(|s| s.health(1000)).await.unwrap().pending, 6);
    assert!(
        !fs::read(dir.path().join("garde.sqlite3"))
            .unwrap()
            .windows(b"PRIVATE-SUFFIX".len())
            .any(|w| w == b"PRIVATE-SUFFIX")
    );
    for _ in 0..6 {
        let delivery = database
            .call(|s| s.claim(garde_daemon::runtime::now()))
            .await
            .unwrap()
            .unwrap();
        assert!(!delivery.reply.text.contains("PRIVATE-SUFFIX"));
        assert!(!delivery.reply.text.contains("forged"));
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
}

#[tokio::test]
async fn rotation_keeps_unread_and_late_old_file_records_across_restart() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let log = dir.path().join("auth.log");
    fs::write(&log, "").unwrap();
    let config = Config {
        server_id: Id::new("server-demo").unwrap(),
        owner_user_id: Some("123456789".into()),
        bot_token_file: dir.path().join("token"),
        state_dir: dir.path().into(),
        fail2ban: Default::default(),
        firewall: Default::default(),
        default_locale: "en".into(),
        ssh: SshConfig::File { path: log.clone() },
    };
    let database = Database::open(config.clone()).await.unwrap();
    let mut collector = Collector::new(config.clone(), database.clone());
    collector.scan().await.unwrap();
    let mut writer = OpenOptions::new().append(true).open(&log).unwrap();
    writeln!(
        writer,
        "sshd[1]: Accepted password for before from 192.0.2.1 port 4444 ssh2"
    )
    .unwrap();
    fs::rename(&log, dir.path().join("auth.log.1")).unwrap();
    fs::write(
        &log,
        "sshd[2]: Accepted password for after from 192.0.2.2 port 5555 ssh2\n",
    )
    .unwrap();
    drop(collector);
    let mut collector = Collector::new(config, database.clone());
    collector.scan().await.unwrap();
    assert_eq!(database.call(|s| s.health(1000)).await.unwrap().pending, 2);
    writeln!(
        writer,
        "sshd[3]: Accepted password for late from 192.0.2.3 port 6666 ssh2"
    )
    .unwrap();
    collector.scan().await.unwrap();
    assert_eq!(database.call(|s| s.health(1000)).await.unwrap().pending, 3);
}

#[tokio::test]
async fn restart_recovers_intermediate_rotations_and_reports_uncertain_coverage() {
    for (numeric, middle) in [(true, true), (true, false), (false, true)] {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let log = dir.path().join("auth.log");
        let record =
            |user| format!("sshd[1]: Accepted password for {user} from 192.0.2.1 port 4444 ssh2\n");
        fs::write(&log, "").unwrap();
        fs::write(dir.path().join("auth.log.1"), record("history")).unwrap();
        let config = Config {
            server_id: Id::new("server-demo").unwrap(),
            owner_user_id: Some("123456789".into()),
            bot_token_file: dir.path().join("token"),
            state_dir: dir.path().into(),
            fail2ban: Default::default(),
            firewall: Default::default(),
            default_locale: "en".into(),
            ssh: SshConfig::File { path: log.clone() },
        };
        let database = Database::open(config.clone()).await.unwrap();
        let mut collector = Collector::new(config.clone(), database.clone());
        collector.scan().await.unwrap();
        drop(collector);
        fs::write(&log, record("first")).unwrap();
        fs::rename(dir.path().join("auth.log.1"), dir.path().join("auth.log.3")).unwrap();
        fs::rename(
            &log,
            dir.path().join(if numeric {
                "auth.log.2"
            } else {
                "auth.log-previous"
            }),
        )
        .unwrap();
        if middle {
            fs::write(dir.path().join("auth.log.1"), record("middle")).unwrap();
        }
        fs::write(&log, record("current")).unwrap();
        let mut collector = Collector::new(config.clone(), database.clone());
        collector.scan().await.unwrap();
        assert_eq!(
            database.call(|s| s.health(1000)).await.unwrap().pending,
            if !numeric {
                4
            } else if middle {
                3
            } else {
                2
            }
        );
        assert_eq!(
            collector.health().await.unwrap().gap_count == 0,
            numeric && middle
        );
        drop(collector);
        let mut collector = Collector::new(config, database.clone());
        collector.scan().await.unwrap();
        assert_eq!(
            database.call(|s| s.health(1000)).await.unwrap().pending,
            if !numeric {
                4
            } else if middle {
                3
            } else {
                2
            }
        );
    }
}

#[tokio::test]
async fn truncate_regrow_and_incomplete_or_oversized_records_leave_following_events_readable() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let log = dir.path().join("auth.log");
    fs::write(&log, "").unwrap();
    let config = Config {
        server_id: Id::new("server-demo").unwrap(),
        owner_user_id: Some("123456789".into()),
        bot_token_file: dir.path().join("token"),
        state_dir: dir.path().into(),
        fail2ban: Default::default(),
        firewall: Default::default(),
        default_locale: "en".into(),
        ssh: SshConfig::File { path: log.clone() },
    };
    let database = Database::open(config.clone()).await.unwrap();
    let mut collector = Collector::new(config, database.clone());
    collector.scan().await.unwrap();
    fs::write(
        &log,
        "sshd[1]: Accepted password for before from 192.0.2.1 port 4444 ssh2\n",
    )
    .unwrap();
    collector.scan().await.unwrap();
    fs::write(
        &log,
        "sshd[2]: Accepted password for after_regrow from 192.0.2.2 port 5555 ssh2\n",
    )
    .unwrap();
    collector.scan().await.unwrap();
    assert_eq!(database.call(|s| s.health(1000)).await.unwrap().pending, 2);
    write!(
        OpenOptions::new().append(true).open(&log).unwrap(),
        "sshd[3]: Accepted password for partial from 192.0.2.3 port 6666 ssh2"
    )
    .unwrap();
    collector.scan().await.unwrap();
    assert_eq!(database.call(|s| s.health(1000)).await.unwrap().pending, 2);
    let mut writer = OpenOptions::new().append(true).open(&log).unwrap();
    writeln!(writer).unwrap();
    writer.write_all(&vec![b'x'; 140000]).unwrap();
    writeln!(writer).unwrap();
    writeln!(
        writer,
        "sshd[4]: Accepted password for following from 192.0.2.4 port 7777 ssh2"
    )
    .unwrap();
    collector.scan().await.unwrap();
    assert_eq!(database.call(|s| s.health(1000)).await.unwrap().pending, 4);
}

#[derive(Clone)]
struct JournalFixture(std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>);
impl garde_daemon::ssh::JournalSource for JournalFixture {
    async fn query(
        &self,
        query: garde_daemon::ssh::JournalQuery,
    ) -> Result<Vec<serde_json::Value>, &'static str> {
        use garde_daemon::ssh::JournalQuery;
        let records = self.0.lock().unwrap();
        match query {
            JournalQuery::Tail => Ok(records.last().cloned().into_iter().collect()),
            JournalQuery::At { cursor } => Ok(records
                .iter()
                .find(|r| r["__CURSOR"] == cursor)
                .cloned()
                .into_iter()
                .collect()),
            JournalQuery::After { cursor } => Ok(records
                .iter()
                .skip_while(|r| r["__CURSOR"] != cursor)
                .skip(1)
                .cloned()
                .collect()),
            JournalQuery::Since { microseconds } => Ok(records
                .iter()
                .filter(|r| {
                    r["__REALTIME_TIMESTAMP"]
                        .as_str()
                        .unwrap()
                        .parse::<u64>()
                        .unwrap()
                        >= microseconds
                })
                .cloned()
                .collect()),
        }
    }
}

#[tokio::test]
async fn journal_cursor_recovery_reports_gap_and_deduplicates_replay() {
    use garde_daemon::ssh::JournalSource;
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let config = Config {
        server_id: Id::new("server-demo").unwrap(),
        owner_user_id: Some("123456789".into()),
        bot_token_file: dir.path().join("token"),
        state_dir: dir.path().into(),
        fail2ban: Default::default(),
        firewall: Default::default(),
        default_locale: "en".into(),
        ssh: SshConfig::Journald {},
    };
    let database = Database::open(config.clone()).await.unwrap();
    let record = |cursor: &str, at: u64, message: &str| serde_json::json!({"__CURSOR":cursor,"__REALTIME_TIMESTAMP":at.to_string(),"MESSAGE":message});
    let journal = JournalFixture(std::sync::Arc::new(std::sync::Mutex::new(vec![record(
        "cursor-0",
        100000000,
        "Accepted password for old from 192.0.2.1 port 1111 ssh2",
    )])));
    let mut collector = Collector::with_journal(config.clone(), database.clone(), journal.clone());
    collector.scan().await.unwrap();
    journal.0.lock().unwrap().push(record(
        "cursor-1",
        101000000,
        "Accepted publickey for new from 2001:db8::1 port 2222 ssh2",
    ));
    collector.scan().await.unwrap();
    assert_eq!(database.call(|s| s.health(1000)).await.unwrap().pending, 1);
    drop(collector);
    journal
        .0
        .lock()
        .unwrap()
        .retain(|r| r["__CURSOR"] != "cursor-1");
    journal
        .0
        .lock()
        .unwrap()
        .push(record("cursor-2", 102000000, "an unsupported SSH record"));
    journal.0.lock().unwrap().push(record(
        "cursor-3",
        103000000,
        "Invalid user stranger from 192.0.2.3 port 3333",
    ));
    let mut collector = Collector::with_journal(config.clone(), database.clone(), journal.clone());
    collector.scan().await.unwrap();
    assert_eq!(database.call(|s| s.health(1000)).await.unwrap().pending, 2);
    let health = collector.health().await.unwrap();
    assert!(health.gap_count >= 1);
    assert_eq!(health.skipped_records, 1);
    let records = journal
        .query(garde_daemon::ssh::JournalQuery::Since {
            microseconds: 100000000,
        })
        .await
        .unwrap();
    assert_eq!(records.len(), 3);
    collector.scan().await.unwrap();
    assert_eq!(database.call(|s| s.health(1000)).await.unwrap().pending, 2);
}

#[tokio::test]
async fn local_diagnostics_are_read_only_without_token_and_access_failure_remains_visible() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let log = dir.path().join("auth.log");
    fs::write(&log, "").unwrap();
    let config = Config {
        server_id: Id::new("server-demo").unwrap(),
        owner_user_id: Some("123456789".into()),
        bot_token_file: dir.path().join("absent-token"),
        state_dir: dir.path().into(),
        fail2ban: Default::default(),
        firewall: Default::default(),
        default_locale: "en".into(),
        ssh: SshConfig::File { path: log.clone() },
    };
    let path = dir.path().join("config.json");
    fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let database = Database::open(config.clone()).await.unwrap();
    let mut collector = Collector::new(config, database.clone());
    collector.scan().await.unwrap();
    let before = fs::read(dir.path().join("garde.sqlite3")).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_want-garde-daemon"))
        .args(["--ssh-status", "--config"])
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let health: garde_contracts::SshHealth = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(health.state, garde_contracts::SshStatus::Healthy);
    assert_eq!(before, fs::read(dir.path().join("garde.sqlite3")).unwrap());
    fs::set_permissions(&log, fs::Permissions::from_mode(0o0)).unwrap();
    assert!(collector.scan().await.is_err());
    assert_eq!(
        collector.health().await.unwrap().state,
        garde_contracts::SshStatus::Unavailable
    );
    fs::set_permissions(&log, fs::Permissions::from_mode(0o600)).unwrap();
    collector.scan().await.unwrap();
    assert_eq!(
        collector.health().await.unwrap().state,
        garde_contracts::SshStatus::Healthy
    );
}

#[tokio::test]
async fn failed_checkpoint_commit_keeps_the_record_available_for_retry() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let log = dir.path().join("auth.log");
    fs::write(&log, "").unwrap();
    let config = Config {
        server_id: Id::new("server-demo").unwrap(),
        owner_user_id: Some("123456789".into()),
        bot_token_file: dir.path().join("token"),
        state_dir: dir.path().into(),
        fail2ban: Default::default(),
        firewall: Default::default(),
        default_locale: "en".into(),
        ssh: SshConfig::File { path: log.clone() },
    };
    let database = Database::open(config.clone()).await.unwrap();
    let mut collector = Collector::new(config, database.clone());
    collector.scan().await.unwrap();
    // Inject a storage failure after the event insert, before its checkpoint commits.
    let fault = rusqlite::Connection::open(dir.path().join("garde.sqlite3")).unwrap();
    fault.execute_batch("CREATE TRIGGER checkpoint_failure BEFORE UPDATE ON meta WHEN NEW.key='ssh.state' BEGIN SELECT RAISE(ABORT,'test failure'); END;").unwrap();
    fs::write(
        &log,
        "sshd[1]: Accepted password for retry from 192.0.2.1 port 4444 ssh2\n",
    )
    .unwrap();
    assert!(collector.scan().await.is_err());
    assert_eq!(database.call(|s| s.health(1000)).await.unwrap().pending, 0);
    fault
        .execute_batch("DROP TRIGGER checkpoint_failure")
        .unwrap();
    collector.scan().await.unwrap();
    collector.scan().await.unwrap();
    assert_eq!(database.call(|s| s.health(1000)).await.unwrap().pending, 1);
}

#[tokio::test]
async fn repeated_failed_registration_does_not_keep_removed_log_files_open() {
    use std::os::unix::fs::MetadataExt;
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let log = dir.path().join("auth.log");
    fs::write(&log, "").unwrap();
    let config = Config {
        server_id: Id::new("server-demo").unwrap(),
        owner_user_id: Some("123456789".into()),
        bot_token_file: dir.path().join("token"),
        state_dir: dir.path().into(),
        fail2ban: Default::default(),
        firewall: Default::default(),
        default_locale: "en".into(),
        ssh: SshConfig::File { path: log.clone() },
    };
    let database = Database::open(config.clone()).await.unwrap();
    let mut collector = Collector::new(config, database.clone());
    collector.scan().await.unwrap();
    fs::rename(&log, dir.path().join("auth.log.1")).unwrap();
    let fault = rusqlite::Connection::open(dir.path().join("garde.sqlite3")).unwrap();
    fault.execute_batch("CREATE TRIGGER checkpoint_failure BEFORE UPDATE ON meta WHEN NEW.key='ssh.state' BEGIN SELECT RAISE(ABORT,'test failure'); END;").unwrap();
    let mut identities = std::collections::BTreeSet::new();
    for _ in 0..24 {
        fs::write(&log, "").unwrap();
        let m = fs::metadata(&log).unwrap();
        identities.insert((m.dev(), m.ino()));
        assert!(collector.scan().await.is_err());
        fs::remove_file(&log).unwrap();
    }
    #[cfg(target_os = "macos")]
    let retained = {
        let output = std::process::Command::new("/usr/sbin/lsof")
            .args(["-a", "-p", &std::process::id().to_string(), "-F", "i"])
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .filter_map(|line| line.strip_prefix('i')?.parse::<u64>().ok())
            .filter(|inode| identities.iter().any(|(_, own)| own == inode))
            .count()
    };
    #[cfg(not(target_os = "macos"))]
    let retained = fs::read_dir("/dev/fd")
        .unwrap()
        .filter_map(|entry| {
            let m = fs::metadata(entry.ok()?.path()).ok()?;
            identities.contains(&(m.dev(), m.ino())).then_some(())
        })
        .count();
    assert!(
        (1..=4).contains(&retained),
        "retained {retained} removed log files"
    );
}

#[tokio::test]
async fn an_unavailable_rotated_file_produces_a_persistent_gap_and_new_events_continue() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let log = dir.path().join("auth.log");
    fs::write(&log, "").unwrap();
    let config = Config {
        server_id: Id::new("server-demo").unwrap(),
        owner_user_id: Some("123456789".into()),
        bot_token_file: dir.path().join("token"),
        state_dir: dir.path().into(),
        fail2ban: Default::default(),
        firewall: Default::default(),
        default_locale: "en".into(),
        ssh: SshConfig::File { path: log.clone() },
    };
    let database = Database::open(config.clone()).await.unwrap();
    let mut collector = Collector::new(config.clone(), database.clone());
    collector.scan().await.unwrap();
    fs::rename(&log, dir.path().join("auth.log.1")).unwrap();
    fs::write(
        &log,
        "sshd[2]: Accepted password for after from 192.0.2.2 port 5555 ssh2\n",
    )
    .unwrap();
    fs::remove_file(dir.path().join("auth.log.1")).unwrap();
    drop(collector);
    let mut collector = Collector::new(config, database.clone());
    collector.scan().await.unwrap();
    let health = collector.health().await.unwrap();
    assert_eq!(health.state, garde_contracts::SshStatus::Degraded);
    assert_eq!(health.last_gap.unwrap().as_str(), "ssh.file_missing");
    let gaps = health.gap_count;
    assert!(gaps > 0);
    collector.scan().await.unwrap();
    assert_eq!(collector.health().await.unwrap().gap_count, gaps);
    assert_eq!(database.call(|s| s.health(1000)).await.unwrap().pending, 1);
}

#[tokio::test]
async fn a_future_checkpoint_version_is_preserved_and_blocks_recovery_before_queue_changes() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let log = dir.path().join("auth.log");
    fs::write(&log, "").unwrap();
    let config = Config {
        server_id: Id::new("server-demo").unwrap(),
        owner_user_id: Some("123456789".into()),
        bot_token_file: dir.path().join("token"),
        state_dir: dir.path().into(),
        fail2ban: Default::default(),
        firewall: Default::default(),
        default_locale: "en".into(),
        ssh: SshConfig::File { path: log.clone() },
    };
    let database = Database::open(config.clone()).await.unwrap();
    let mut collector = Collector::new(config.clone(), database.clone());
    collector.scan().await.unwrap();
    fs::write(
        &log,
        "sshd[1]: Accepted password for pending from 192.0.2.1 port 4444 ssh2\n",
    )
    .unwrap();
    collector.scan().await.unwrap();
    database
        .call(|s| s.claim(garde_daemon::runtime::now()))
        .await
        .unwrap()
        .unwrap();
    let fault = rusqlite::Connection::open(dir.path().join("garde.sqlite3")).unwrap();
    let saved: String = fault
        .query_row("SELECT value FROM meta WHERE key='ssh.state'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&saved).unwrap();
    value["version"] = serde_json::json!(2);
    fault
        .execute(
            "UPDATE meta SET value=?1 WHERE key='ssh.state'",
            [serde_json::to_string(&value).unwrap()],
        )
        .unwrap();
    drop(fault);
    let before = fs::read(dir.path().join("garde.sqlite3")).unwrap();
    assert_eq!(collector.scan().await, Err("ssh.version"));
    assert!(
        garde_daemon::delivery::Store::inspect_ssh(&config, garde_daemon::runtime::now()).is_err()
    );
    drop(collector);
    drop(database);
    let code = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            match garde_daemon::delivery::Store::open(dir.path(), "server-demo", "123456789") {
                Err("storage.locked") => {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await
                }
                Err(code) => break code,
                Ok(_) => panic!("future_state_accepted"),
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(code, "ssh.version");
    assert_eq!(before, fs::read(dir.path().join("garde.sqlite3")).unwrap());
    assert_eq!(
        garde_daemon::delivery::Store::inspect(dir.path(), "server-demo", "123456789", 1000)
            .unwrap()
            .in_flight,
        1
    );
}
