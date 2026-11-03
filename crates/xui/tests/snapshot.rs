use garde_contracts::Measurement;
use rusqlite::Connection;
fn database(path: &std::path::Path) -> Connection {
    let c = Connection::open(path).unwrap();
    c.execute_batch("CREATE TABLE inbounds(id INTEGER PRIMARY KEY,remark TEXT,enable INTEGER,port INTEGER,protocol TEXT,settings TEXT,stream_settings TEXT,sniffing TEXT); CREATE TABLE settings(id INTEGER PRIMARY KEY,key TEXT,value TEXT); CREATE TABLE users(id INTEGER PRIMARY KEY,username TEXT,password TEXT); CREATE TABLE clients(id INTEGER PRIMARY KEY,email TEXT,uuid TEXT,password TEXT,enable INTEGER); CREATE TABLE client_inbounds(client_id INTEGER,inbound_id INTEGER); CREATE TABLE client_traffics(inbound_id INTEGER,email TEXT,enable INTEGER,up INTEGER,down INTEGER,last_online INTEGER); INSERT INTO inbounds VALUES(7,'edge',1,443,'vless','{\"clients\":[]}','{}','{}'); INSERT INTO users VALUES(1,'operator','PRIVATE_PASSWORD_MARKER'); INSERT INTO settings VALUES(1,'webToken','PRIVATE_TOKEN_MARKER'); INSERT INTO clients VALUES(1,'alice','PRIVATE_UUID_MARKER','PRIVATE_CLIENT_PASSWORD',1); INSERT INTO client_inbounds VALUES(1,7); INSERT INTO client_traffics VALUES(7,'alice',1,10,20,1700000000000);").unwrap();
    c
}
#[test]
fn a_local_snapshot_hashes_configuration_and_normalizes_client_activity() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("panel.db");
    let c = database(&path);
    let before = garde_xui::snapshot(&path, 1700000000).unwrap();
    let encoded = serde_json::to_string(&before).unwrap();
    assert!(!encoded.contains("PRIVATE_"));
    let Measurement::Available { value: clients } = &before.clients else {
        panic!("client source unavailable")
    };
    assert_eq!(clients.len(), 1);
    assert_eq!(clients[0].last_online, Some(1700000000));
    assert_eq!(clients[0].traffic_bytes, 30);
    assert!(before.connectivity_available);
    c.execute("UPDATE users SET password='PRIVATE_REPLACEMENT'", [])
        .unwrap();
    let after = garde_xui::snapshot(&path, 1700000030).unwrap();
    assert_ne!(before.panel, after.panel);
    assert!(!serde_json::to_string(&after).unwrap().contains("PRIVATE_"));
    assert_eq!(
        c.query_row("SELECT password FROM users", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "PRIVATE_REPLACEMENT"
    );
}
#[test]
fn unknown_schema_never_creates_or_rebuilds_the_panel_database() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("missing.db");
    assert!(garde_xui::snapshot(&p, 1700000000).is_err());
    assert!(!p.exists());
    let c = Connection::open(&p).unwrap();
    c.execute("CREATE TABLE unrelated(value TEXT)", []).unwrap();
    assert!(garde_xui::snapshot(&p, 1700000000).is_err());
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type='table'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}

#[test]
fn a_locked_database_recovers_and_wal_commits_are_visible_without_database_writes() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("panel.db");
    let c = database(&p);
    c.execute_batch("BEGIN EXCLUSIVE").unwrap();
    assert!(matches!(
        garde_xui::snapshot(&p, 1700000000),
        Err("xui.locked")
    ));
    c.execute_batch("ROLLBACK; PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0")
        .unwrap();
    let first = garde_xui::snapshot(&p, 1700000000).unwrap();
    c.execute("UPDATE users SET password='PRIVATE_WAL_MARKER'", [])
        .unwrap();
    let before = std::fs::read(&p).unwrap();
    let wal_before = std::fs::read(p.with_file_name("panel.db-wal")).unwrap();
    let next = garde_xui::snapshot(&p, 1700000001).unwrap();
    assert_ne!(first.panel, next.panel);
    assert!(!serde_json::to_string(&next).unwrap().contains("PRIVATE_"));
    assert_eq!(before, std::fs::read(&p).unwrap());
    assert_eq!(
        wal_before,
        std::fs::read(p.with_file_name("panel.db-wal")).unwrap()
    );
}
#[test]
fn embedded_clients_preserve_inventory_when_last_online_is_unavailable() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("panel.db");
    let c = database(&p);
    c.execute_batch("DROP TABLE clients;DROP TABLE client_inbounds;DROP TABLE client_traffics;")
        .unwrap();
    c.execute(
        "UPDATE inbounds SET settings=?1",
        [r#"{"clients":[{"id":"PRIVATE_UUID","email":"alice","enable":true}]}"#],
    )
    .unwrap();
    let v = garde_xui::snapshot(&p, 1700000000).unwrap();
    assert_eq!(v.schema.unwrap().as_str(), "xui.embedded");
    assert!(!v.connectivity_available);
    let Measurement::Available { value } = v.clients else {
        panic!("inventory unavailable")
    };
    assert_eq!(value[0].label, "alice");
    assert_eq!(value[0].last_online, None);
}
#[test]
fn views_and_oversized_values_degrade_instead_of_becoming_a_complete_empty_snapshot() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("panel.db");
    let c = database(&p);
    c.execute("UPDATE settings SET value=?1", ["x".repeat(1048577)])
        .unwrap();
    let v = garde_xui::snapshot(&p, 1700000000).unwrap();
    assert!(matches!(v.panel, Measurement::Unavailable { .. }));
    assert!(matches!(v.clients, Measurement::Available { .. }));
    c.execute_batch("DROP TABLE users; CREATE VIEW users AS SELECT 1 AS id, 'operator' AS username,'PRIVATE_VIEW' AS password").unwrap();
    assert!(matches!(
        garde_xui::snapshot(&p, 1700000000),
        Err("xui.schema")
    ));
}
#[test]
fn missing_or_malformed_last_online_keeps_client_metadata_without_inventing_offline() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("panel.db");
    let c = database(&p);
    for value in [
        rusqlite::types::Value::Null,
        rusqlite::types::Value::Text("invalid-time".into()),
    ] {
        c.execute("UPDATE client_traffics SET last_online=?1", [value])
            .unwrap();
        let snapshot = garde_xui::snapshot(&p, 1700000000).unwrap();
        let Measurement::Available { value: clients } = snapshot.clients else {
            panic!("missing time must not discard client metadata")
        };
        assert_eq!(clients.len(), 1);
        assert_eq!(clients[0].label, "alice");
        assert_eq!(clients[0].last_online, None);
    }
}
