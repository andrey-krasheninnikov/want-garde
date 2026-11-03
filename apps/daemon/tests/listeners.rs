use garde_config::Config;
use garde_daemon::{
    delivery::Outcome,
    listeners::{Monitor, RawSnapshot},
    runtime::Database,
};
use std::{fs, os::unix::fs::PermissionsExt};
fn config(path: &std::path::Path) -> Config {
    serde_json::from_value(serde_json::json!({"server_id":"server-demo","owner_user_id":"123456789","bot_token_file":path.join("token"),"state_dir":path,"default_locale":"en","ports":{"ignore_ephemeral_udp":false}})).unwrap()
}
fn raw(ports: &[u16]) -> RawSnapshot {
    RawSnapshot {
        ipv4: Ok(ports
            .iter()
            .map(|p| {
                format!(
                    "tcp LISTEN 0 128 127.0.0.1:{p} 0.0.0.0:* users:((\"example\",pid=42,fd=3))\n"
                )
            })
            .collect()),
        ipv6: Ok(String::new()),
        process_reason: None,
        ephemeral_range: Some([32768, 60999]),
    }
}
async fn drain(db: &Database, at: i64) -> Vec<String> {
    let mut result = Vec::new();
    while let Some(d) = db.call(move |s| s.claim(at)).await.unwrap() {
        result.push(d.reply.text.clone());
        db.call(move |s| s.finish(d.id, Outcome::Delivered, at))
            .await
            .unwrap();
    }
    result
}
#[tokio::test]
async fn fixed_baseline_survives_restart_and_explicit_acceptance_does_not_hide_new_changes() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let c = config(dir.path());
    let db = Database::open(c.clone()).await.unwrap();
    let mut m = Monitor::new(c.clone(), db.clone());
    m.observe(raw(&[8080]), 1000).await.unwrap();
    assert!(
        drain(&db, 1000)
            .await
            .iter()
            .any(|s| s.contains("Listener baseline initialized"))
    );
    m.observe(raw(&[8080, 8081]), 1001).await.unwrap();
    assert!(
        drain(&db, 1001)
            .await
            .iter()
            .any(|s| s.contains("New listening port"))
    );
    let mut m = Monitor::new(c.clone(), db.clone());
    m.observe(raw(&[8080, 8081]), 1002).await.unwrap();
    assert!(drain(&db, 1002).await.is_empty());
    let health = db
        .call({
            let c = c.clone();
            move |s| s.ports_health(&c, 1002)
        })
        .await
        .unwrap();
    assert_eq!(health.added.len(), 1);
    assert_eq!(health.baseline.unwrap().endpoints.len(), 1);
    let preview = m.preview(raw(&[8080, 8081]), 1003).await.unwrap();
    m.accept(raw(&[8080, 8081]), &preview.hash, 1003)
        .await
        .unwrap();
    drain(&db, 1003).await;
    assert!(
        m.accept(raw(&[8080, 8081]), &preview.hash, 1004)
            .await
            .is_err()
    );
    m.observe(raw(&[8080, 8081, 8082]), 1005).await.unwrap();
    assert!(
        drain(&db, 1005)
            .await
            .iter()
            .any(|s| s.contains("New listening port"))
    );
}

#[tokio::test]
async fn incomplete_endpoint_observation_never_reports_mass_removal_or_replaces_baseline() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let c = config(dir.path());
    let db = Database::open(c.clone()).await.unwrap();
    let mut m = Monitor::new(c.clone(), db.clone());
    m.observe(raw(&[8080]), 1000).await.unwrap();
    drain(&db, 1000).await;
    let incomplete = RawSnapshot {
        ipv4: Err("monitor.timeout"),
        ipv6: Ok(String::new()),
        process_reason: Some("helper.permissions"),
        ephemeral_range: None,
    };
    assert!(m.preview(incomplete.clone(), 1001).await.is_err());
    m.observe(incomplete, 1001).await.unwrap();
    let h = db
        .call({
            let c = c.clone();
            move |s| s.ports_health(&c, 1001)
        })
        .await
        .unwrap();
    assert!(h.removed.is_empty());
    assert!(!h.snapshot.unwrap().complete);
    assert_eq!(h.baseline.unwrap().endpoints.len(), 1);
    assert!(
        !drain(&db, 1001)
            .await
            .iter()
            .any(|s| s.contains("Listening port disappeared"))
    );
    m.observe(raw(&[8080]), 1002).await.unwrap();
    assert!(
        !drain(&db, 1002)
            .await
            .iter()
            .any(|s| s.contains("New listening port"))
    );
}
#[tokio::test]
async fn udp_filters_do_not_suppress_suspicious_names_and_bracket_patterns_match() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut c = config(dir.path());
    c.ports.ignore_ephemeral_udp = true;
    c.ports.ignore_patterns = vec!["tcp 127.0.0.1:808[01]".into()];
    let db = Database::open(c.clone()).await.unwrap();
    let mut m = Monitor::new(c.clone(), db.clone());
    let sample=RawSnapshot{ipv4:Ok("udp UNCONN 0 0 0.0.0.0:40000 0.0.0.0:* users:((\"python\",pid=42,fd=3))\ntcp LISTEN 0 128 127.0.0.1:8080 0.0.0.0:* users:((\"example\",pid=43,fd=3))\nudp UNCONN 0 0 0.0.0.0:51820 0.0.0.0:* users:((\"example\",pid=43,fd=4))\n".into()),ipv6:Ok("tcp LISTEN 0 128 [::]:8080 [::]:* users:((\"example\",pid=43,fd=5))\n".into()),process_reason:None,ephemeral_range:Some([32768,60999])};
    m.observe(sample.clone(), 1000).await.unwrap();
    let h = db
        .call({
            let c = c.clone();
            move |s| s.ports_health(&c, 1000)
        })
        .await
        .unwrap();
    assert_eq!(h.snapshot.unwrap().ignored_count, 2);
    assert_eq!(h.baseline.unwrap().endpoints.len(), 2);
    assert_eq!(h.suspicious.len(), 1);
    let messages = drain(&db, 1000).await;
    assert!(
        messages
            .iter()
            .any(|s| s.contains("Suspicious listener process name"))
    );
    let preview = m.preview(sample.clone(), 1001).await.unwrap();
    m.accept(sample.clone(), &preview.hash, 1001).await.unwrap();
    drain(&db, 1001).await;
    m.observe(sample, 1002).await.unwrap();
    assert!(drain(&db, 1002).await.is_empty());
}
#[tokio::test]
async fn changed_filters_require_acceptance_and_partial_process_metadata_keeps_the_warning() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let c = config(dir.path());
    let db = Database::open(c.clone()).await.unwrap();
    let mut m = Monitor::new(c.clone(), db.clone());
    let mut sample = raw(&[8080]);
    sample.ipv4 = sample.ipv4.map(|s| s.replace("example", "python"));
    m.observe(sample.clone(), 1000).await.unwrap();
    drain(&db, 1000).await;
    let before = m.preview(sample.clone(), 1001).await.unwrap();
    let mut changed = c.clone();
    changed.ports.ignore_patterns = vec!["tcp *".into()];
    let mut next = Monitor::new(changed.clone(), db.clone());
    next.observe(sample.clone(), 1001).await.unwrap();
    let h = db
        .call({
            let c = changed.clone();
            move |s| s.ports_health(&c, 1001)
        })
        .await
        .unwrap();
    assert!(
        h.problems
            .iter()
            .any(|p| p.as_str() == "ports.baseline_review")
    );
    assert!(h.removed.is_empty());
    assert!(
        next.accept(sample.clone(), &before.hash, 1002)
            .await
            .is_err()
    );
    let mut partial = raw(&[8080]);
    partial.ipv4 = Ok("tcp LISTEN 0 128 127.0.0.1:8080 0.0.0.0:*\n".into());
    partial.process_reason = Some("helper.permissions");
    next.observe(partial, 1003).await.unwrap();
    let h = db
        .call(move |s| s.ports_health(&changed, 1003))
        .await
        .unwrap();
    assert_eq!(h.suspicious.len(), 1);
}

#[tokio::test]
async fn failed_storage_does_not_advance_baseline_or_lose_a_listener_transition() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let c = config(dir.path());
    let db = Database::open(c.clone()).await.unwrap();
    let mut m = Monitor::new(c.clone(), db.clone());
    m.observe(raw(&[8080]), 1000).await.unwrap();
    drain(&db, 1000).await;
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o500)).unwrap();
    let result = m.observe(raw(&[8080, 8081]), 1001).await;
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.is_err());
    let h = db
        .call({
            let c = c.clone();
            move |s| s.ports_health(&c, 1001)
        })
        .await
        .unwrap();
    assert!(h.added.is_empty());
    assert!(drain(&db, 1001).await.is_empty());
    m.observe(raw(&[8080, 8081]), 1002).await.unwrap();
    assert_eq!(
        drain(&db, 1002)
            .await
            .iter()
            .filter(|s| s.contains("New listening port"))
            .count(),
        1
    );
}
#[tokio::test]
async fn available_address_families_and_process_names_are_validated_without_guessing() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let c = config(dir.path());
    let db = Database::open(c.clone()).await.unwrap();
    let mut m = Monitor::new(c.clone(), db.clone());
    let sample=RawSnapshot{ipv4:Ok("udp UNCONN 0 0 *:53 *:* users:((\"example\",pid=42,fd=3))\n".into()),ipv6:Ok("tcp LISTEN 0 128 [2001:db8::1]:8080 [::]:* users:((\"example\",pid=43,fd=4),(\"python\",pid=44,fd=3))\n".into()),process_reason:None,ephemeral_range:Some([32768,60999])};
    m.observe(sample, 1000).await.unwrap();
    let h = db
        .call({
            let c = c.clone();
            move |s| s.ports_health(&c, 1000)
        })
        .await
        .unwrap();
    let snapshot = h.snapshot.unwrap();
    assert!(snapshot.complete && snapshot.processes_complete);
    assert_eq!(snapshot.listeners[0].endpoint.address, "0.0.0.0");
    assert_eq!(snapshot.listeners[1].processes.len(), 2);
    let mut invalid = raw(&[8080]);
    invalid.ipv4 = Ok("tcp LISTEN 0 128 [192.0.2.1:8080 0.0.0.0:*\n".into());
    m.observe(invalid, 1001).await.unwrap();
    let h = db.call(move |s| s.ports_health(&c, 1001)).await.unwrap();
    assert!(!h.snapshot.unwrap().complete);
    assert_eq!(h.baseline.unwrap().endpoints.len(), 2);
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
async fn listener_commands_preserve_all_locales_rtl_and_private_owner_authorization() {
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let c = config(dir.path());
    let db = Database::open(c.clone()).await.unwrap();
    let mut m = Monitor::new(c.clone(), db.clone());
    m.observe(raw(&[8080]), garde_daemon::runtime::now())
        .await
        .unwrap();
    drain(&db, garde_daemon::runtime::now()).await;
    let mut b = garde_daemon::Bot::new(c.clone()).unwrap();
    for (id, user, kind) in [(0, 987654321, "private"), (1, 123456789, "group")] {
        let u = json!({"update_id":id,"message":{"from":{"id":user,"is_bot":false},"chat":{"id":user,"type":kind},"text":"/ports"}});
        b.process_update(&db, &u, &Host, "example_bot")
            .await
            .unwrap();
    }
    let callback =
        json!({"update_id":2,"callback_query":{"from":{"id":123456789},"data":"/ports"}});
    b.process_update(&db, &callback, &Host, "example_bot")
        .await
        .unwrap();
    assert!(drain(&db, garde_daemon::runtime::now()).await.is_empty());
    let mut id = 3;
    for (locale, copy) in garde_entrypoint::catalog()["locales"].as_object().unwrap() {
        for (text, key) in [
            (format!("/language {locale}"), "language_saved"),
            ("/services".into(), "services_title"),
            ("/ports".into(), "ports_title"),
        ] {
            let u = json!({"update_id":id,"message":{"from":{"id":123456789,"is_bot":false},"chat":{"id":123456789,"type":"private"},"text":text}});
            b.process_update(&db, &u, &Host, "example_bot")
                .await
                .unwrap();
            let messages = drain(&db, garde_daemon::runtime::now()).await;
            assert_eq!(messages.len(), 1);
            assert!(messages[0].contains(copy[key].as_str().unwrap()));
            assert!(!messages[0].contains("message.unavailable"));
            assert!(messages[0].encode_utf16().count() < 3900);
            if ["ar", "he"].contains(&locale.as_str()) && key != "language_saved" {
                assert!(messages[0].contains('\u{2068}'));
            }
            id += 1;
        }
    }
    let stored: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.path().join("owner-preferences.json")).unwrap())
            .unwrap();
    assert!(garde_config::valid_locale(
        stored["locale"].as_str().unwrap()
    ));
}
#[tokio::test]
async fn raw_listener_change_reaches_transport_through_persistent_outbox() {
    use garde_daemon::{
        Reply,
        runtime::{Identity, Transport, send_loop},
        telegram::TransportError,
    };
    use std::sync::{Arc, Mutex};
    #[derive(Default)]
    struct Telegram(Mutex<Vec<String>>);
    impl Transport for Telegram {
        async fn identity(&self) -> Result<Identity, TransportError> {
            unreachable!()
        }
        async fn updates(&self, _: i64) -> Result<Vec<serde_json::Value>, TransportError> {
            std::future::pending().await
        }
        async fn send(&self, reply: &Reply) -> Result<(), TransportError> {
            self.0.lock().unwrap().push(reply.text.clone());
            Ok(())
        }
    }
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let c = config(dir.path());
    let db = Database::open(c.clone()).await.unwrap();
    let mut m = Monitor::new(c, db.clone());
    m.observe(raw(&[8080]), garde_daemon::runtime::now())
        .await
        .unwrap();
    drain(&db, garde_daemon::runtime::now()).await;
    m.observe(raw(&[8080, 8081]), garde_daemon::runtime::now())
        .await
        .unwrap();
    let telegram = Arc::new(Telegram::default());
    let sender = tokio::spawn(send_loop(db.clone(), telegram.clone()));
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while telegram.0.lock().unwrap().is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    sender.abort();
    let _ = sender.await;
    let messages = telegram.0.lock().unwrap();
    assert!(messages[0].contains("New listening port"));
    assert!(messages[0].contains("ports-2"));
    assert!(messages[0].contains("ID: d-2"));
}

#[tokio::test]
async fn oversized_listener_output_and_excess_endpoints_are_visible_and_cannot_seed_baseline() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let c = config(dir.path());
    let db = Database::open(c.clone()).await.unwrap();
    let mut m = Monitor::new(c.clone(), db.clone());
    let mut sample = raw(&[]);
    sample.ipv4 = Ok("x".repeat(65537));
    m.observe(sample, 1000).await.unwrap();
    let h = db
        .call({
            let c = c.clone();
            move |s| s.ports_health(&c, 1000)
        })
        .await
        .unwrap();
    assert!(h.baseline.is_none());
    assert!(h.problems.iter().any(|p| p.as_str() == "monitor.output"));
    m.observe(raw(&(1..=513).collect::<Vec<_>>()), 1001)
        .await
        .unwrap();
    let h = db.call(move |s| s.ports_health(&c, 1001)).await.unwrap();
    assert!(h.baseline.is_none());
    assert!(h.problems.iter().any(|p| p.as_str() == "ports.limit"));
}
