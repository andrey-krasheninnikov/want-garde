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
