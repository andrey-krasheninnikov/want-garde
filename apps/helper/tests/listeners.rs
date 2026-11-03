use garde_contracts::{AddressFamily, ListenerReadResponse};
use garde_helper::listeners::{ListenerSource, serve_connection};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
};
struct Source;
impl ListenerSource for Source {
    async fn read(&self, family: AddressFamily) -> Result<String, &'static str> {
        Ok(match family {
            AddressFamily::Ipv4 => {
                "tcp LISTEN 0 128 127.0.0.1:8080 0.0.0.0:* users:((\"example\",pid=42,fd=3))\n"
                    .into()
            }
            AddressFamily::Ipv6 => String::new(),
        })
    }
}
#[tokio::test]
async fn fixed_listener_protocol_authenticates_uid_and_rejects_other_actions() {
    let uid = rustix::process::geteuid().as_raw();
    for (body, allowed) in [
        (
            "{\"version\":\"1\",\"action\":{\"type\":\"ports.status\"}}\n",
            true,
        ),
        (
            "{\"version\":\"1\",\"action\":{\"type\":\"fail2ban.status\"}}\n",
            false,
        ),
        (
            "{\"version\":\"1\",\"action\":{\"type\":\"ports.status\",\"command\":\"/bin/sh\"}}\n",
            false,
        ),
    ] {
        let (mut client, server) = UnixStream::pair().unwrap();
        let task = tokio::spawn(async move { serve_connection(server, uid, &Source).await });
        client.write_all(body.as_bytes()).await.unwrap();
        let mut output = Vec::new();
        client.read_to_end(&mut output).await.unwrap();
        task.await.unwrap().unwrap();
        match serde_json::from_slice::<ListenerReadResponse>(&output).unwrap() {
            ListenerReadResponse::Snapshot { ipv4, ipv6 } => {
                assert!(allowed);
                assert!(ipv4.contains("example"));
                assert!(ipv6.is_empty());
            }
            ListenerReadResponse::Unavailable { .. } => assert!(!allowed),
        }
    }
    let (client, server) = UnixStream::pair().unwrap();
    assert_eq!(
        serve_connection(server, uid.wrapping_add(1), &Source).await,
        Err("helper.peer")
    );
    drop(client);
}

#[tokio::test]
async fn listener_socket_bounds_oversized_and_incomplete_requests_without_echo() {
    let uid = rustix::process::geteuid().as_raw();
    for body in ["PRIVATE-INPUT".repeat(100) + "\n", "{".into()] {
        let (mut client, server) = UnixStream::pair().unwrap();
        let task = tokio::spawn(async move { serve_connection(server, uid, &Source).await });
        client.write_all(body.as_bytes()).await.unwrap();
        let mut output = vec![];
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.read_to_end(&mut output),
        )
        .await
        .unwrap()
        .unwrap();
        task.await.unwrap().unwrap();
        assert!(matches!(
            serde_json::from_slice::<ListenerReadResponse>(&output).unwrap(),
            ListenerReadResponse::Unavailable { .. }
        ));
        assert!(!String::from_utf8(output).unwrap().contains("PRIVATE-INPUT"));
    }
}

struct Oversized;
impl ListenerSource for Oversized {
    async fn read(&self, _: AddressFamily) -> Result<String, &'static str> {
        Ok("PRIVATE-OUTPUT".repeat(6000))
    }
}
#[tokio::test]
async fn listener_helper_does_not_return_oversized_source_output() {
    let uid = rustix::process::geteuid().as_raw();
    let (mut client, server) = UnixStream::pair().unwrap();
    let task = tokio::spawn(async move { serve_connection(server, uid, &Oversized).await });
    client
        .write_all(b"{\"version\":\"1\",\"action\":{\"type\":\"ports.status\"}}\n")
        .await
        .unwrap();
    let mut output = vec![];
    client.read_to_end(&mut output).await.unwrap();
    task.await.unwrap().unwrap();
    assert!(matches!(
        serde_json::from_slice::<ListenerReadResponse>(&output).unwrap(),
        ListenerReadResponse::Unavailable { .. }
    ));
    assert!(
        !String::from_utf8(output)
            .unwrap()
            .contains("PRIVATE-OUTPUT")
    );
}
