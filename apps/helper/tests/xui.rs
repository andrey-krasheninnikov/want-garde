use garde_contracts::{Id, XuiReadRequest, XuiReadResponse};
use garde_helper::xui::{XuiSource, serve_connection};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
};
struct Fixture;
impl XuiSource for Fixture {
    async fn read(&self, _: XuiReadRequest) -> Result<XuiReadResponse, &'static str> {
        Ok(XuiReadResponse::Unavailable {
            version: garde_contracts::WireVersion::V1,
            reason_key: Id::new("xui.schema").unwrap(),
        })
    }
}
async fn exchange(request: &str, uid: u32) -> (Result<(), &'static str>, String) {
    let (mut client, server) = UnixStream::pair().unwrap();
    let text = request.to_owned();
    let peer = tokio::spawn(async move {
        let _ = client.write_all(text.as_bytes()).await;
        let _ = client.shutdown().await;
        let mut out = String::new();
        let _ = client.read_to_string(&mut out).await;
        out
    });
    let result = serve_connection(server, uid, &Fixture).await;
    (result, peer.await.unwrap())
}
#[tokio::test]
async fn kernel_peer_identity_and_closed_request_shape_limit_xui_reads() {
    let uid = rustix::process::geteuid().as_raw();
    if uid == 0 {
        return;
    }
    let (r, text) = exchange("{\"version\":\"1\",\"type\":\"snapshot\"}\n", uid).await;
    assert!(r.is_ok());
    let v: XuiReadResponse = serde_json::from_str(&text).unwrap();
    assert!(matches!(v, XuiReadResponse::Unavailable { .. }));
    for request in [
        "{\"version\":\"1\",\"type\":\"snapshot\",\"sql\":\"select password from users\"}\n",
        "{\"version\":\"1\",\"type\":\"snapshot\",\"path\":\"/etc/shadow\"}\n",
        "{\"version\":\"1\",\"type\":\"snapshot\",\"uid\":0}\n",
        "{\"type\":\"exec\",\"command\":\"id\"}\n",
    ] {
        let (r, out) = exchange(request, uid).await;
        assert!(r.is_err());
        assert!(out.is_empty())
    }
    let (r, out) = exchange("{\"version\":\"1\",\"type\":\"snapshot\"}\n", uid + 1).await;
    assert!(r.is_err());
    assert!(out.is_empty());
}
