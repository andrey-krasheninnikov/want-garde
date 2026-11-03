#![cfg(target_os = "linux")]

use garde_config::{Config, SshConfig};
use garde_contracts::Id;
use garde_daemon::{
    runtime::{Database, now},
    ssh::{Collector, Journal, JournalQuery, JournalSource},
};
use std::{fs, os::unix::fs::PermissionsExt, process::Command, time::Duration};

fn namespace() -> String {
    std::env::var("WANT_GARDE_TEST_JOURNAL_NAMESPACE").expect("isolated_namespace_required")
}
fn emit(message: &str) {
    assert!(
        Command::new("/usr/bin/logger")
            .args(["-p", "auth.info", "-t", "sshd", "--", message])
            .status()
            .unwrap()
            .success()
    );
}

#[tokio::test]
#[ignore = "requires an isolated journald namespace and service journal access"]
async fn native_journal_tail_restart_and_missing_cursor() {
    let name = namespace();
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    emit("Accepted password for old-fixture from 192.0.2.1 port 1111 ssh2");
    tokio::time::sleep(Duration::from_millis(200)).await;
    let config = Config {
        server_id: Id::new("linux-fixture").unwrap(),
        owner_user_id: Some("123456789".into()),
        bot_token_file: dir.path().join("absent-token"),
        state_dir: dir.path().into(),
        fail2ban: Default::default(),
        firewall: Default::default(),
        default_locale: "en".into(),
        ssh: SshConfig::Journald {},
    };
    let database = Database::open(config.clone()).await.unwrap();
    let mut collector = Collector::with_journal(
        config.clone(),
        database.clone(),
        Journal::namespace(&name).unwrap(),
    );
    collector.scan().await.unwrap();
    assert_eq!(database.call(|s| s.health(now())).await.unwrap().pending, 0);
    emit("Accepted publickey for new-fixture from 2001:db8::1 port 2222 ssh2");
    tokio::time::timeout(Duration::from_secs(10), async {
        while database.call(|s| s.health(now())).await.unwrap().pending != 1 {
            collector.scan().await.unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
    drop(collector);
    let mut collector =
        Collector::with_journal(config, database.clone(), Journal::namespace(&name).unwrap());
    collector.scan().await.unwrap();
    assert_eq!(database.call(|s| s.health(now())).await.unwrap().pending, 1);
    let missing=Journal::namespace(&name).unwrap().query(JournalQuery::At{cursor:"s=11111111111111111111111111111111;i=1;b=11111111111111111111111111111111;m=1;t=1;x=1".into()}).await.unwrap();
    assert!(
        missing.is_empty()
            || missing[0]["__CURSOR"]
                != "s=11111111111111111111111111111111;i=1;b=11111111111111111111111111111111;m=1;t=1;x=1"
    );
}

#[tokio::test]
#[ignore = "requires an isolated journal without service journal access"]
async fn native_journal_access_denial() {
    let result = Journal::namespace(&namespace())
        .unwrap()
        .query(JournalQuery::Tail)
        .await;
    assert!(result.is_err());
}

#[tokio::test]
#[ignore = "requires an isolated journal, service journal access and a non-root emitter"]
async fn native_journal_rejects_a_user_supplied_fail2ban_identifier() {
    assert_ne!(rustix::process::geteuid().as_raw(), 0);
    emit("Accepted password for old-fixture from 192.0.2.1 port 1111 ssh2");
    tokio::time::sleep(Duration::from_millis(200)).await;
    let journal =
        Journal::namespace_for_source(garde_daemon::ingest::Kind::Fail2ban, &namespace()).unwrap();
    let tail = journal.query(JournalQuery::Tail).await.unwrap();
    let cursor = tail.last().unwrap()["__CURSOR"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        Command::new("/usr/bin/logger")
            .args(["-t", "fail2ban", "--", "[sshd] Ban 192.0.2.42"])
            .status()
            .unwrap()
            .success()
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        journal
            .query(JournalQuery::After { cursor })
            .await
            .unwrap()
            .is_empty()
    );
}
