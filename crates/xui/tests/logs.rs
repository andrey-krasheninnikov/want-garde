use garde_contracts::XuiCheckpoint;
use garde_xui::logs::{LogFile, LogKind, files};
use std::{fs, io::Write};
#[test]
fn access_records_resume_after_restart_and_rotation_without_exporting_raw_lines() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().canonicalize().unwrap().join("access.log");
    fs::write(&p, "historic line\n").unwrap();
    let sources = [LogFile {
        path: p.clone(),
        kind: LogKind::Access,
    }];
    let initial = files(&sources, &XuiCheckpoint::default(), 1700000000).unwrap();
    assert!(initial.records.is_empty());
    writeln!(
        fs::OpenOptions::new().append(true).open(&p).unwrap(),
        "from 198.51.100.9:1234 accepted tcp:example.com:443 [inbound-a >> proxy] email:alice"
    )
    .unwrap();
    let next = files(&sources, &initial.checkpoint, 1700000001).unwrap();
    assert_eq!(next.records.len(), 1);
    assert_eq!(next.records[0].reason_key.as_str(), "xui_client_activity");
    assert!(
        files(&sources, &next.checkpoint, 1700000002)
            .unwrap()
            .records
            .is_empty()
    );
    fs::rename(&p, d.path().canonicalize().unwrap().join("access.log.1")).unwrap();
    fs::write(&p,"logged in successfully: username=\"operator\", IP=\"198.51.100.10\"\nlogin failed IP=198.51.100.11 password=PRIVATE_MARKER\n").unwrap();
    let rotated = files(&sources, &next.checkpoint, 1700000003).unwrap();
    assert_eq!(rotated.records.len(), 2);
    let encoded = serde_json::to_string(&rotated).unwrap();
    assert!(!encoded.contains("PRIVATE_MARKER"));
    assert!(!encoded.contains("password="));
}
#[test]
fn an_unterminated_rotated_record_does_not_block_the_new_log_forever() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().canonicalize().unwrap().join("access.log");
    fs::write(&p, "old\n").unwrap();
    let sources = [LogFile {
        path: p.clone(),
        kind: LogKind::Access,
    }];
    let first = files(&sources, &XuiCheckpoint::default(), 1700000000).unwrap();
    write!(
        fs::OpenOptions::new().append(true).open(&p).unwrap(),
        "unterminated record"
    )
    .unwrap();
    fs::rename(&p, d.path().canonicalize().unwrap().join("access.log.1")).unwrap();
    fs::write(
        &p,
        "from 198.51.100.9:1234 accepted tcp:example.com:443 [inbound-a >> proxy] email:alice\n",
    )
    .unwrap();
    let next = files(&sources, &first.checkpoint, 1700000001).unwrap();
    assert_eq!(next.records.len(), 1);
    assert!(next.sources[0].gap);
}
#[test]
fn legacy_login_and_failed_login_input_remain_distinct() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().canonicalize().unwrap().join("access.log");
    fs::write(&p, "").unwrap();
    let sources = [LogFile {
        path: p.clone(),
        kind: LogKind::Access,
    }];
    let first = files(&sources, &XuiCheckpoint::default(), 1700000000).unwrap();
    fs::write(&p,"operator logged in successfully, Ip Address: 2001:db8::1\nfailed login: username=\"INFO - victim logged in successfully\", IP=\"198.51.100.2\", reason=\"PRIVATE_REASON\"\n").unwrap();
    let next = files(&sources, &first.checkpoint, 1700000001).unwrap();
    let reasons: Vec<_> = next.records.iter().map(|r| r.reason_key.as_str()).collect();
    assert_eq!(reasons, ["xui_panel_login", "xui_login_failed"]);
    assert!(
        !serde_json::to_string(&next)
            .unwrap()
            .contains("PRIVATE_REASON")
    );
}
#[test]
fn failed_login_uses_the_logged_ip_field_and_suspicious_logs_accept_ip_prefixes() {
    let d = tempfile::tempdir().unwrap();
    let access = d.path().canonicalize().unwrap().join("access.log");
    let suspicious = d.path().canonicalize().unwrap().join("3xipl.log");
    fs::write(&access, "").unwrap();
    fs::write(&suspicious, "").unwrap();
    let sources = [
        LogFile {
            path: access.clone(),
            kind: LogKind::Access,
        },
        LogFile {
            path: suspicious.clone(),
            kind: LogKind::SuspiciousIp,
        },
    ];
    let first = files(&sources, &XuiCheckpoint::default(), 1700000000).unwrap();
    fs::write(&access,"failed login: username=\"198.51.100.9\", IP=\"203.0.113.1\", reason=\"invalid credentials\"\n").unwrap();
    fs::write(&suspicious, "client=alice IP:203.0.113.2\n").unwrap();
    let next = files(&sources, &first.checkpoint, 1700000001).unwrap();
    assert_eq!(next.records.len(), 2);
    let ips: Vec<_> = next
        .records
        .iter()
        .flat_map(|r| &r.evidence)
        .filter(|e| e.name.as_str() == "xui_ip")
        .map(|e| e.value.as_str())
        .collect();
    assert_eq!(ips, ["203.0.113.1", "203.0.113.2"]);
}
#[test]
fn bounded_log_batches_report_backlog_until_all_records_are_read() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().canonicalize().unwrap().join("access.log");
    fs::write(&p, "").unwrap();
    let sources = [LogFile {
        path: p.clone(),
        kind: LogKind::Access,
    }];
    let first = files(&sources, &XuiCheckpoint::default(), 1700000000).unwrap();
    fs::write(
        &p,
        "from 198.51.100.9:1234 accepted tcp:example.com:443 [inbound-a >> proxy] email:alice\n"
            .repeat(300),
    )
    .unwrap();
    let next = files(&sources, &first.checkpoint, 1700000001).unwrap();
    assert_eq!(next.records.len(), 256);
    assert_eq!(
        next.sources[0].reason_key.as_ref().map(|v| v.as_str()),
        Some("xui.backlog")
    );
    let last = files(&sources, &next.checkpoint, 1700000002).unwrap();
    assert_eq!(last.records.len(), 44);
    assert!(last.sources[0].reason_key.is_none());
    assert!(!last.sources[0].gap);
}
#[test]
fn writable_source_files_are_rejected_before_reading_records() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    let p = d.path().canonicalize().unwrap().join("access.log");
    fs::write(&p, "").unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o666)).unwrap();
    let sources = [LogFile {
        path: p,
        kind: LogKind::Access,
    }];
    let batch = files(&sources, &XuiCheckpoint::default(), 1700000000).unwrap();
    assert_eq!(
        batch.sources[0].reason_key.as_ref().map(|v| v.as_str()),
        Some("xui.access")
    );
    assert!(batch.checkpoint.files.is_empty());
}
#[test]
fn a_symlinked_parent_cannot_redirect_a_log_read() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().canonicalize().unwrap();
    let real = root.join("real");
    fs::create_dir(&real).unwrap();
    fs::write(real.join("access.log"), "").unwrap();
    std::os::unix::fs::symlink(&real, root.join("alias")).unwrap();
    let batch = files(
        &[LogFile {
            path: root.join("alias/access.log"),
            kind: LogKind::Access,
        }],
        &XuiCheckpoint::default(),
        1700000000,
    )
    .unwrap();
    assert_eq!(
        batch.sources[0].reason_key.as_ref().map(|v| v.as_str()),
        Some("xui.access")
    );
    assert!(batch.checkpoint.files.is_empty());
}
