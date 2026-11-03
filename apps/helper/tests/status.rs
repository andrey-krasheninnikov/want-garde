#[test]
fn read_protocol_rejects_commands_paths_and_unknown_authorization_claims() {
    assert!(
        garde_helper::decode_request(r#"{"version":"1","action":{"type":"fail2ban.status"}}"#)
            .is_ok()
    );
    for input in [
        r#"{"version":"1","action":{"type":"server.status"}}"#,
        r#"{"version":"1","action":{"type":"fail2ban.status","command":"unbanip"}}"#,
        r#"{"version":"1","action":{"type":"fail2ban.status"},"uid":0}"#,
        r#"{"version":"1","action":{"type":"fail2ban.status"},"path":"/private-value"}"#,
        r#"{"version":"2","action":{"type":"fail2ban.status"}}"#,
    ] {
        assert_eq!(
            garde_helper::decode_request(input).unwrap_err(),
            "helper.request"
        );
    }
    assert!(garde_helper::decode_request(&" ".repeat(1025)).is_err());
}

struct Fail2ban;
impl garde_helper::StatusSource for Fail2ban {
    async fn query(&self, query: garde_helper::Query) -> Result<String, &'static str> {
        use garde_helper::Query;
        Ok(match query {
            Query::Status => "Status\n|- Number of jail: 2\n`- Jail list: sshd, nginx".into(),
            Query::Jail(jail) if jail=="sshd"=>"Status for the jail: sshd\n|- Filter\n`- Actions\n   |- Currently banned: 2\n   `- Banned IP list: 192.0.2.42 2001:db8::42".into(),
            Query::Jail(jail) if jail=="nginx"=>"Status for the jail: nginx\n`- Actions\n   |- Currently banned: 0\n   `- Banned IP list:".into(),
            Query::LogTarget=>"Current logging target is:\n`- /var/log/fail2ban.log".into(),
            Query::LogLevel=>"Current logging level is 'INFO'".into(),
            _=>panic!("unexpected command"),
        })
    }
}
#[tokio::test]
async fn live_snapshot_contains_current_jails_and_validated_addresses() {
    let snapshot = garde_helper::status(&Fail2ban).await.unwrap();
    assert_eq!(snapshot.jails.len(), 2);
    assert_eq!(snapshot.jails[0].current_banned, Some(2));
    assert_eq!(snapshot.jails[0].addresses, ["192.0.2.42", "2001:db8::42"]);
    assert_eq!(snapshot.jails[1].current_banned, Some(0));
    assert_eq!(
        snapshot.log_target.as_deref(),
        Some("/var/log/fail2ban.log")
    );
    assert!(!snapshot.partial);
}

#[tokio::test]
async fn socket_checks_kernel_peer_identity_and_returns_only_typed_reads() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (_client, server) = tokio::net::UnixStream::pair().unwrap();
    let uid = rustix::process::geteuid().as_raw();
    assert_eq!(
        garde_helper::serve_connection(server, uid.wrapping_add(1), &Fail2ban)
            .await
            .unwrap_err(),
        "helper.peer"
    );
    let (mut client, server) = tokio::net::UnixStream::pair().unwrap();
    let task =
        tokio::spawn(async move { garde_helper::serve_connection(server, uid, &Fail2ban).await });
    client
        .write_all(b"{\"version\":\"1\",\"action\":{\"type\":\"fail2ban.status\"}}\n")
        .await
        .unwrap();
    let mut output = Vec::new();
    client.read_to_end(&mut output).await.unwrap();
    task.await.unwrap().unwrap();
    let response: serde_json::Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(response["type"], "snapshot");
    assert_eq!(
        response["snapshot"]["jails"][0]["addresses"][1],
        "2001:db8::42"
    );
}

struct MalformedJails;
impl garde_helper::StatusSource for MalformedJails {
    async fn query(&self, query: garde_helper::Query) -> Result<String, &'static str> {
        match query {
            garde_helper::Query::Status => Ok("Status\n`- Jail list: sshd, --config".into()),
            _ => panic!("unvalidated jail reached command adapter"),
        }
    }
}
#[tokio::test]
async fn malformed_jail_inventory_never_reaches_a_command() {
    assert_eq!(
        garde_helper::status(&MalformedJails).await.unwrap_err(),
        "helper.output"
    );
}
#[tokio::test]
async fn socket_rejects_oversized_input_without_echo_and_bounds_incomplete_input() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    for oversized in [true, false] {
        let (mut client, server) = tokio::net::UnixStream::pair().unwrap();
        let uid = rustix::process::geteuid().as_raw();
        let task =
            tokio::spawn(
                async move { garde_helper::serve_connection(server, uid, &Fail2ban).await },
            );
        let input = if oversized {
            "PRIVATE-INPUT".repeat(100) + "\n"
        } else {
            "{".into()
        };
        client.write_all(input.as_bytes()).await.unwrap();
        let mut output = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.read_to_end(&mut output),
        )
        .await
        .unwrap()
        .unwrap();
        task.await.unwrap().unwrap();
        let response: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(response["type"], "unavailable");
        assert!(!String::from_utf8(output).unwrap().contains("PRIVATE-INPUT"));
    }
}
