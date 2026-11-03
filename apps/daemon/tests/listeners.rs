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

#[tokio::test]
async fn invalid_process_names_preserve_endpoint_changes_and_allow_next_observations() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let c = config(dir.path());
    let db = Database::open(c.clone()).await.unwrap();
    let mut m = Monitor::new(c.clone(), db.clone());
    m.observe(raw(&[]), 1000).await.unwrap();
    drain(&db, 1000).await;
    for (at, name) in [(1001, "bad\u{7f}name"), (1002, "bad\u{80}name")] {
        let mut sample = raw(&[8080]);
        sample.ipv4 = sample.ipv4.map(|s| s.replace("example", name));
        m.observe(sample, at).await.unwrap();
        let h = db
            .call({
                let c = c.clone();
                move |s| s.ports_health(&c, at)
            })
            .await
            .unwrap();
        let snapshot = h.snapshot.unwrap();
        assert!(snapshot.complete);
        assert!(!snapshot.processes_complete);
        assert!(snapshot.listeners[0].processes.is_empty());
        assert_eq!(h.added.len(), 1);
    }
    drain(&db, 1002).await;
    m.observe(raw(&[8080, 8081]), 1003).await.unwrap();
    assert!(
        drain(&db, 1003)
            .await
            .iter()
            .any(|s| s.contains("127.0.0.1:8081"))
    );
}

#[tokio::test]
async fn expected_process_names_clear_only_matching_retained_warnings_with_partial_metadata() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let mut c = config(dir.path());
    let db = Database::open(c.clone()).await.unwrap();
    let mut m = Monitor::new(c.clone(), db.clone());
    let mut sample = raw(&[8080, 8081]);
    sample.ipv4 = sample
        .ipv4
        .map(|s| s.replace("example", "python").replacen("python", "node", 1));
    m.observe(sample, 1000).await.unwrap();
    drain(&db, 1000).await;
    c.ports.expected_process_names.push("python".into());
    let mut restarted = Monitor::new(c.clone(), db.clone());
    let mut partial = raw(&[8080, 8081]);
    partial.ipv4 = partial.ipv4.map(|s| {
        s.lines()
            .map(|line| format!("{}\n", line.split(" users:").next().unwrap()))
            .collect()
    });
    restarted.observe(partial, 1001).await.unwrap();
    let h = db.call(move |s| s.ports_health(&c, 1001)).await.unwrap();
    assert_eq!(
        h.suspicious
            .iter()
            .map(|s| s.process_name.as_str())
            .collect::<Vec<_>>(),
        vec!["node"]
    );
    let messages = drain(&db, 1001).await;
    assert!(
        messages
            .iter()
            .any(|s| s.contains("Suspicious listener no longer observed") && s.contains("python"))
    );
}

#[tokio::test]
async fn every_endpoint_and_process_change_reaches_delivery_across_bounded_pages() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let c = config(dir.path());
    let db = Database::open(c.clone()).await.unwrap();
    let mut m = Monitor::new(c, db.clone());
    m.observe(raw(&[]), 1000).await.unwrap();
    drain(&db, 1000).await;
    m.observe(raw(&(8080..8089).collect::<Vec<_>>()), 1001)
        .await
        .unwrap();
    let messages = drain(&db, 1001).await;
    for port in 8080..8089 {
        assert!(
            messages
                .iter()
                .any(|s| s.contains(&format!("127.0.0.1:{port}"))),
            "missing port {port}"
        );
    }
    for (at, first) in [(1002, 10000), (1003, 11000)] {
        let mut sample = raw(&[]);
        let rows = |address: &str, peer: &str| {
            (first..first + 256).map(|port| format!("tcp LISTEN 0 128 {address}:{port} {peer}:* users:((\"bash\",pid=1,fd=3),(\"node\",pid=2,fd=3),(\"php\",pid=3,fd=3),(\"sh\",pid=4,fd=3))\n")).collect::<String>()
        };
        sample.ipv4 = Ok(rows("127.0.0.1", "0.0.0.0"));
        sample.ipv6 = Ok(rows("[::1]", "[::]"));
        m.observe(sample, at).await.unwrap();
        let messages = drain(&db, at).await;
        assert!(messages.iter().all(|s| s.chars().count() < 4096));
        for port in first..first + 256 {
            for endpoint in [format!("127.0.0.1:{port}"), format!("[::1]:{port}")] {
                for process in ["bash", "node", "php", "sh"] {
                    let evidence = format!("{endpoint} {process}");
                    assert!(
                        messages.iter().any(|s| s.contains(&evidence)),
                        "missing {evidence}"
                    );
                }
            }
        }
        if at == 1003 {
            assert!(messages.iter().any(|s| s.contains("127.0.0.1:10000 bash")));
        }
    }
}

#[tokio::test]
async fn partial_warning_history_is_bounded_and_keeps_collecting_endpoint_changes() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let c = config(dir.path());
    let db = Database::open(c.clone()).await.unwrap();
    let mut m = Monitor::new(c.clone(), db.clone());
    let sample = |names: &[&str]| {
        let rows = |address: &str, peer: &str| {
            (10000..10130)
                .map(|port| {
                    format!(
                        "tcp LISTEN 0 128 {address}:{port} {peer}:* users:({})\n",
                        names
                            .iter()
                            .enumerate()
                            .map(|(i, name)| format!("(\"{name}\",pid={},fd=3)", 1000 + i))
                            .collect::<Vec<_>>()
                            .join(",")
                    )
                })
                .collect::<String>()
        };
        RawSnapshot {
            ipv4: Ok(rows("127.0.0.1", "0.0.0.0")),
            ipv6: Ok(rows("[::1]", "[::]")),
            process_reason: None,
            ephemeral_range: Some([32768, 60999]),
        }
    };
    m.observe(sample(&["bash", "node", "php", "sh", "python"]), 1000)
        .await
        .unwrap();
    drain(&db, 1000).await;
    let next = sample(&["nc", "ncat", "netcat", "socat", "ruby"]);
    m.observe(next.clone(), 1001).await.unwrap();
    let messages = drain(&db, 1001).await;
    assert!(
        !messages
            .iter()
            .any(|s| s.contains("Suspicious listener no longer observed"))
    );
    let mut restarted = Monitor::new(c.clone(), db.clone());
    restarted.observe(next.clone(), 1002).await.unwrap();
    assert!(drain(&db, 1002).await.is_empty());
    let h = db
        .call({
            let c = c.clone();
            move |s| s.ports_health(&c, 1002)
        })
        .await
        .unwrap();
    assert!(h.suspicious.len() <= 2048);
    assert!(h.problems.iter().any(|p| p.as_str() == "ports.limit"));
    let mut changed = next;
    changed
        .ipv4
        .as_mut()
        .unwrap()
        .push_str(&raw(&[8080]).ipv4.unwrap());
    restarted.observe(changed, 1003).await.unwrap();
    assert!(
        drain(&db, 1003)
            .await
            .iter()
            .any(|s| s.contains("127.0.0.1:8080"))
    );
    restarted.observe(raw(&[8080]), 1004).await.unwrap();
    let h = db.call(move |s| s.ports_health(&c, 1004)).await.unwrap();
    assert!(!h.problems.iter().any(|p| p.as_str() == "ports.limit"));
}

#[tokio::test]
async fn maximum_listener_snapshot_and_warning_history_fit_the_storage_budget() {
    let dir = tempfile::tempdir().unwrap();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let c = config(dir.path());
    let db = Database::open(c.clone()).await.unwrap();
    let mut m = Monitor::new(c.clone(), db.clone());
    for (at, names) in [
        (1000, ["bash", "node", "php", "sh", "python"]),
        (1001, ["nc", "ncat", "netcat", "socat", "ruby"]),
    ] {
        let rows = |address: &str| {
            (10000..10256)
                .map(|port| {
                    format!(
                        "tcp LISTEN 0 128 {address}:{port} *:* users:({})\n",
                        names
                            .iter()
                            .enumerate()
                            .map(|(i, name)| format!("(\"{name}\",pid={},fd=3)", i + 1))
                            .collect::<Vec<_>>()
                            .join(",")
                    )
                })
                .collect::<String>()
        };
        let v4 = rows("255.255.255.255");
        let v6 = rows(&format!(
            "[ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff%{}]",
            "z".repeat(32)
        ));
        assert!(v4.len() <= 65536 && v6.len() <= 65536);
        m.observe(
            RawSnapshot {
                ipv4: Ok(v4),
                ipv6: Ok(v6),
                process_reason: None,
                ephemeral_range: Some([32768, 60999]),
            },
            at,
        )
        .await
        .unwrap();
        let h = db
            .call({
                let c = c.clone();
                move |s| s.ports_health(&c, at)
            })
            .await
            .unwrap();
        assert_eq!(h.snapshot.unwrap().listeners.len(), 512);
        assert_eq!(h.baseline.unwrap().endpoints.len(), 512);
        assert_eq!(h.suspicious.len(), 2048);
        if at == 1001 {
            assert!(h.problems.iter().any(|p| p.as_str() == "ports.limit"));
        }
        drain(&db, at).await;
    }
}
