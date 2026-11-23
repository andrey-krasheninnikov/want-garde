use crate::Reply;
use crate::runtime::{Identity, Transport};
use garde_config::{MAX_TELEGRAM_ROUTES, TelegramEgress, TelegramRoute, Token};
use reqwest::{Client, ClientBuilder, RequestBuilder, Response, multipart};
use serde_json::{Value, json};
use std::time::Duration;

mod route;
use route::Route;

const TELEGRAM_ORIGIN: &str = "https://api.telegram.org";
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
pub const MAX_TELEGRAM_FILE_BYTES: usize = 8 * 1024 * 1024;
static DIRECT: [TelegramRoute; 1] = [TelegramRoute::Direct {}];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    Retry,
    Recipient,
    Fatal,
}

#[derive(Debug)]
pub struct TransportError {
    pub code: &'static str,
    pub kind: FailureKind,
    pub uncertain: bool,
    pub retry_after: Duration,
}
impl TransportError {
    fn transient(code: &'static str) -> Self {
        Self {
            code,
            kind: FailureKind::Retry,
            uncertain: true,
            retry_after: Duration::ZERO,
        }
    }
    fn fatal(code: &'static str) -> Self {
        Self {
            code,
            kind: FailureKind::Fatal,
            uncertain: false,
            retry_after: Duration::ZERO,
        }
    }
    fn recipient(code: &'static str) -> Self {
        Self {
            code,
            kind: FailureKind::Recipient,
            retry_after: Duration::ZERO,
            uncertain: false,
        }
    }
}

pub struct Telegram {
    routes: Vec<Route>,
    api_prefix: String,
    file_prefix: String,
}

pub struct TelegramFile {
    path: String,
    size: Option<u64>,
}

impl Telegram {
    pub fn new(token: Token) -> Result<Self, TransportError> {
        Self::configured(token, None)
    }

    pub fn configured(
        token: Token,
        egress: Option<&TelegramEgress>,
    ) -> Result<Self, TransportError> {
        Self::with_routes(token.as_str(), egress, |builder| builder)
    }

    fn with_routes(
        token: &str,
        egress: Option<&TelegramEgress>,
        configure: impl Fn(ClientBuilder) -> ClientBuilder,
    ) -> Result<Self, TransportError> {
        let routes = egress.map_or(DIRECT.as_slice(), |egress| egress.routes.as_slice());
        if !(1..=MAX_TELEGRAM_ROUTES).contains(&routes.len()) {
            return Err(TransportError::fatal("telegram.route_config"));
        }
        Ok(Self {
            routes: routes
                .iter()
                .map(|route| Route::new(route, &configure))
                .collect::<Result<_, _>>()?,
            api_prefix: format!("{TELEGRAM_ORIGIN}/bot{token}/"),
            file_prefix: format!("{TELEGRAM_ORIGIN}/file/bot{token}/"),
        })
    }

    async fn request(
        &self,
        request: impl Fn(&Client) -> RequestBuilder,
    ) -> Result<Response, TransportError> {
        let mut routes = self.routes.iter().enumerate().peekable();
        while let Some((index, route)) = routes.next() {
            match request(&route.client).send().await {
                Ok(response) => return Ok(response),
                // Hyper's connect error precedes transmission of this request.
                Err(error) if error.is_connect() => {
                    let code = route.connect_failure(&error);
                    if routes.peek().is_none() {
                        let mut error = TransportError::transient(code);
                        error.uncertain = false;
                        return Err(error);
                    }
                    eprintln!(
                        "telegram.route_fallback: {} {} {code}",
                        index + 1,
                        route.label()
                    );
                }
                Err(_) => return Err(TransportError::transient("telegram.network")),
            }
        }
        Err(TransportError::fatal("telegram.route_config"))
    }

    async fn call(&self, method: &str, data: Value) -> Result<Value, TransportError> {
        let url = format!("{}{method}", self.api_prefix);
        let response = self.request(|client| client.post(&url).json(&data)).await?;
        api_result(method, response).await
    }

    pub async fn send_document(
        &self,
        chat_id: &str,
        file_name: &str,
        bytes: &[u8],
    ) -> Result<(), TransportError> {
        if bytes.is_empty() || bytes.len() > MAX_TELEGRAM_FILE_BYTES {
            return Err(TransportError::recipient("telegram.file_size"));
        }
        if !valid_file_name(file_name) {
            return Err(TransportError::recipient("telegram.file_name"));
        }
        // Callers must authorize the private destination before uploading.
        if !garde_config::valid_user_id(chat_id) {
            return Err(TransportError::recipient("delivery.invalid"));
        }
        let url = format!("{}sendDocument", self.api_prefix);
        let response = self
            .request(|client| {
                let part = multipart::Part::bytes(bytes.to_vec()).file_name(file_name.to_owned());
                let form = multipart::Form::new()
                    .text("chat_id", chat_id.to_owned())
                    .part("document", part);
                client.post(&url).multipart(form)
            })
            .await?;
        let result = api_result("sendDocument", response).await?;
        if result["message_id"].as_i64().is_none_or(|id| id <= 0)
            || result["chat"]["id"].as_i64() != chat_id.parse().ok()
        {
            return Err(TransportError::transient("telegram.response"));
        }
        Ok(())
    }

    pub async fn get_file(&self, file_id: &str) -> Result<TelegramFile, TransportError> {
        if file_id.is_empty()
            || file_id.len() > 512
            || !file_id.bytes().all(|b| b.is_ascii_graphic())
        {
            return Err(TransportError::recipient("telegram.file_id"));
        }
        let result = self.call("getFile", json!({"file_id":file_id})).await?;
        let path = result["file_path"]
            .as_str()
            .filter(|path| valid_file_path(path))
            .ok_or_else(|| TransportError::recipient("telegram.file_path"))?;
        let size = result
            .get("file_size")
            .map(|size| {
                size.as_u64()
                    .filter(|size| *size <= MAX_TELEGRAM_FILE_BYTES as u64)
                    .ok_or_else(|| TransportError::recipient("telegram.file_size"))
            })
            .transpose()?;
        Ok(TelegramFile {
            path: path.to_owned(),
            size,
        })
    }

    pub async fn download_file(&self, file: &TelegramFile) -> Result<Vec<u8>, TransportError> {
        let url = format!("{}{}", self.file_prefix, file.path);
        let response = self.request(|client| client.get(&url)).await?;
        match response.status().as_u16() {
            200 => bounded_body(
                response,
                MAX_TELEGRAM_FILE_BYTES,
                TransportError::recipient("telegram.file_size"),
            )
            .await
            .and_then(|body| {
                if file.size.is_some_and(|size| size != body.len() as u64) {
                    Err(TransportError::transient("telegram.response"))
                } else {
                    Ok(body)
                }
            }),
            401 => Err(TransportError::fatal("telegram.unauthorized")),
            429 => Err(rate_limited(response).await),
            400 | 403 | 404 => Err(TransportError::recipient("telegram.file_unavailable")),
            _ => Err(TransportError::transient("telegram.api")),
        }
    }
}

fn valid_file_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && !matches!(name, "." | "..")
        && !name.contains(['/', '\\'])
        && !name.chars().any(char::is_control)
}

fn valid_file_path(path: &str) -> bool {
    path.len() <= 512
        && path.split('/').all(|part| {
            !part.is_empty()
                && !matches!(part, "." | "..")
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        })
}

async fn bounded_body(
    mut response: Response,
    limit: usize,
    oversized: TransportError,
) -> Result<Vec<u8>, TransportError> {
    if response.content_length().is_some_and(|n| n > limit as u64) {
        return Err(oversized);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| TransportError::transient("telegram.network"))?
    {
        if chunk.len() > limit - bytes.len() {
            return Err(oversized);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn rate_limit_error(retry_after: Option<u64>) -> TransportError {
    TransportError {
        code: "telegram.rate_limited",
        kind: FailureKind::Retry,
        uncertain: false,
        retry_after: Duration::from_secs(retry_after.unwrap_or(30).max(1)),
    }
}

async fn rate_limited(response: Response) -> TransportError {
    let header = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    let body = bounded_body(
        response,
        MAX_RESPONSE_BYTES,
        TransportError::transient("telegram.response"),
    )
    .await
    .ok()
    .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
    .and_then(|body| body["parameters"]["retry_after"].as_u64());
    rate_limit_error(header.max(body))
}

async fn api_result(method: &str, response: Response) -> Result<Value, TransportError> {
    let recipient_method = matches!(
        method,
        "sendMessage" | "getChat" | "sendDocument" | "getFile"
    );
    let status = response.status().as_u16();
    if matches!(status, 401 | 404) {
        return Err(TransportError::fatal("telegram.unauthorized"));
    }
    if status == 409 {
        return Err(TransportError::fatal("telegram.conflict"));
    }
    if status == 429 {
        return Err(rate_limited(response).await);
    }
    if recipient_method && matches!(status, 400 | 403) {
        return Err(TransportError::recipient(if status == 403 {
            "telegram.forbidden"
        } else {
            "telegram.bad_request"
        }));
    }
    let bytes = bounded_body(
        response,
        MAX_RESPONSE_BYTES,
        TransportError::transient("telegram.response"),
    )
    .await?;
    let body: Value = serde_json::from_slice(&bytes)
        .map_err(|_| TransportError::transient("telegram.response"))?;
    if body["error_code"] == 429 {
        return Err(rate_limit_error(body["parameters"]["retry_after"].as_u64()));
    }
    if body["ok"] != true || !(200..300).contains(&status) {
        return Err(if matches!(body["error_code"].as_u64(), Some(401 | 404)) {
            TransportError::fatal("telegram.unauthorized")
        } else if body["error_code"] == 409 {
            TransportError::fatal("telegram.conflict")
        } else if recipient_method && matches!(body["error_code"].as_u64(), Some(400 | 403)) {
            TransportError::recipient(if body["error_code"] == 403 {
                "telegram.forbidden"
            } else {
                "telegram.bad_request"
            })
        } else {
            TransportError::transient("telegram.api")
        });
    }
    body.get("result")
        .cloned()
        .ok_or_else(|| TransportError::transient("telegram.response"))
}

impl Transport for Telegram {
    async fn identity(&self) -> Result<Identity, TransportError> {
        let me = self.call("getMe", json!({})).await?;
        let username = me["username"]
            .as_str()
            .filter(|s| {
                (5..=32).contains(&s.len())
                    && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
            })
            .ok_or_else(|| TransportError::fatal("telegram.identity"))?;
        if me["is_bot"] != true || !me["id"].as_u64().is_some_and(|n| n > 0 && n < (1 << 52)) {
            return Err(TransportError::fatal("telegram.identity"));
        }
        let webhook = self.call("getWebhookInfo", json!({})).await?;
        if webhook["url"].as_str() != Some("") {
            return Err(TransportError::fatal("telegram.webhook"));
        }
        Ok(Identity {
            id: me["id"]
                .as_u64()
                .ok_or_else(|| TransportError::fatal("telegram.identity"))?
                .to_string(),
            username: username.to_owned(),
        })
    }

    async fn updates(&self, offset: i64) -> Result<Vec<Value>, TransportError> {
        let value = self
            .call(
                "getUpdates",
                json!({"offset":offset,"timeout":25,"limit":32,"allowed_updates":["message"]}),
            )
            .await?;
        let list = value
            .as_array()
            .filter(|a| a.len() <= 32)
            .ok_or_else(|| TransportError::transient("telegram.response"))?;
        if !list.iter().all(|v| {
            v["update_id"]
                .as_i64()
                .is_some_and(|n| (0..i64::MAX).contains(&n))
        }) {
            return Err(TransportError::transient("telegram.response"));
        }
        Ok(list.clone())
    }

    async fn send(&self, reply: &Reply) -> Result<(), TransportError> {
        if reply.text.encode_utf16().count() > 4096 {
            return Err(TransportError::recipient("telegram.message_size"));
        }
        let topic_id = reply.envelope.topic_id;
        let expected_chat = reply
            .chat_id
            .parse::<i64>()
            .ok()
            .filter(|_| garde_config::valid_notification_destination(&reply.chat_id, topic_id))
            .ok_or_else(|| TransportError::recipient("delivery.invalid"))?;
        let mut payload = json!({
            "chat_id": reply.chat_id,
            "text": reply.text,
            "entities": [{"type":"pre","offset":0,"length":reply.text.trim_end().encode_utf16().count()}],
            "disable_notification": reply.envelope.silent,
            "protect_content": reply.envelope.protect_content,
        });
        let forum = expected_chat < 0;
        if forum {
            // Telegram can ignore a thread ID when the chat is not a forum.
            let chat = self
                .call("getChat", json!({"chat_id": reply.chat_id}))
                .await
                .map_err(|mut error| {
                    error.uncertain = false;
                    forum_error(error)
                })?;
            if chat["id"].as_i64() != Some(expected_chat) {
                let mut error = TransportError::transient("telegram.forum_response");
                error.uncertain = false;
                return Err(error);
            }
            if chat["type"] != "supergroup" || chat["is_forum"] != true {
                return Err(TransportError::recipient("telegram.forum_required"));
            }
        }
        if let Some(topic_id) = topic_id {
            payload["message_thread_id"] = json!(topic_id);
        }
        let result = self
            .call("sendMessage", payload)
            .await
            .map_err(|error| if forum { forum_error(error) } else { error })?;
        if result["message_id"].as_i64().is_none_or(|id| id <= 0)
            || result["chat"]["id"].as_i64() != Some(expected_chat)
        {
            return Err(TransportError::transient("telegram.response"));
        }
        if forum
            && (result["chat"]["is_forum"] != true
                || result["message_thread_id"].as_i64() != topic_id.map(i64::from))
        {
            return Err(TransportError::recipient("telegram.forum_response"));
        }
        Ok(())
    }
}

fn forum_error(mut error: TransportError) -> TransportError {
    error.code = match error.code {
        "telegram.bad_request" => "telegram.forum_topic",
        "telegram.forbidden" => "telegram.forum_forbidden",
        code => code,
    };
    error
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::TcpListener,
        thread,
    };

    fn server(
        responses: Vec<(u16, Value)>,
    ) -> (Telegram, thread::JoinHandle<Vec<(String, Value)>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let worker = thread::spawn(move || {
            let mut requests = Vec::new();
            for (status, body) in responses {
                let until = std::time::Instant::now() + Duration::from_secs(5);
                let stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(e)
                            if e.kind() == std::io::ErrorKind::WouldBlock
                                && std::time::Instant::now() < until =>
                        {
                            thread::sleep(Duration::from_millis(5))
                        }
                        Err(_) => panic!("test_transport_timeout"),
                    }
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut reader = BufReader::new(stream);
                let mut path = String::new();
                reader.read_line(&mut path).unwrap();
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':')
                        && name.eq_ignore_ascii_case("content-length")
                    {
                        length = value.trim().parse::<usize>().unwrap();
                    }
                }
                assert!(length < 16384);
                let mut payload = vec![0; length];
                reader.read_exact(&mut payload).unwrap();
                requests.push((path, serde_json::from_slice(&payload).unwrap()));
                if status == 0 {
                    continue;
                }
                let body = match body {
                    Value::String(raw) => raw.into_bytes(),
                    json => serde_json::to_vec(&json).unwrap(),
                };
                write!(reader.get_mut(),"HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).unwrap();
                reader.get_mut().write_all(&body).unwrap();
            }
            requests
        });
        (
            Telegram {
                routes: vec![Route {
                    client: route::builder(false).build().unwrap(),
                    kind: route::Kind::Direct,
                }],
                api_prefix: format!("http://{address}/"),
                file_prefix: format!("http://{address}/file/"),
            },
            worker,
        )
    }

    #[test]
    fn older_configs_and_explicit_direct_routes_load() {
        use std::{fs, os::unix::fs::PermissionsExt};

        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let config_path = dir.path().join("config.json");
        let mut config = json!({
            "server_id":"server-test",
            "bot_token_file":dir.path().join("telegram-token"),
            "state_dir":dir.path(),
            "default_locale":"en",
            "owner_user_id":"42"
        });
        let write = |value: &Value| {
            garde_config::atomic_write(&config_path, &serde_json::to_vec(value).unwrap(), 0o600)
                .unwrap();
        };
        write(&config);
        assert!(garde_config::Config::load(&config_path).is_ok());
        config["telegram_egress"] = json!({"routes":[{"type":"direct"}]});
        write(&config);
        assert!(garde_config::Config::load(&config_path).is_ok());

        for invalid in [
            Value::Null,
            json!({"routes":[]}),
            json!({"routes":[{"type":"direct","proxy_credentials_file":"/tmp/proxy"}]}),
            json!({"routes":[{"type":"vless","proxy_credentials_file":"relative"}]}),
            json!({"routes":[{"type":"vless","proxy_credentials_file":config["bot_token_file"]}]}),
            json!({"routes":[{"type":"wireguard","interface":"../wg0","dns_server":"192.0.2.1:53"}]}),
            json!({"routes":[{"type":"tailscale","interface":"tailscale0","dns_server":"resolver.test:53"}]}),
        ] {
            config["telegram_egress"] = invalid;
            write(&config);
            assert!(garde_config::Config::load(&config_path).is_err());
        }
    }

    #[tokio::test]
    async fn a_malformed_rate_limit_body_still_respects_http_status() {
        let (mut api, worker) = server(vec![(429, json!("<html>Unparseable body</html>"))]);
        api.routes.push(Route {
            client: route::builder(false).build().unwrap(),
            kind: route::Kind::Direct,
        });
        let error = api.updates(0).await.unwrap_err();
        assert_eq!(error.code, "telegram.rate_limited");
        assert_eq!(error.retry_after, Duration::from_secs(30));
        assert!(!error.uncertain);
        worker.join().unwrap();
    }

    fn private_proxy_file(value: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        use std::{fs, os::unix::fs::PermissionsExt};

        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.path().join("proxy-url");
        garde_config::atomic_write(&path, value.as_bytes(), 0o600).unwrap();
        (dir, path)
    }

    #[test]
    fn proxy_credentials_require_the_private_file_and_remote_dns_on_loopback() {
        use std::{fs, os::unix::fs::PermissionsExt};

        let (dir, path) =
            private_proxy_file("socks5h://fixture-user:fixture-secret@127.0.0.1:1080\n");
        let configure = |path: &std::path::Path| {
            let egress = TelegramEgress {
                routes: vec![TelegramRoute::Vless {
                    proxy_credentials_file: path.to_owned(),
                }],
            };
            Telegram::with_routes("fixture-token", Some(&egress), |builder| builder)
        };
        assert!(configure(&path).is_ok());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let error = configure(&path).err().unwrap();
        assert_eq!(error.code, "telegram.proxy_credentials");
        assert!(!format!("{error:?}").contains("fixture-secret"));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let link = dir.path().join("proxy-link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert_eq!(
            configure(&link).err().unwrap().code,
            "telegram.proxy_credentials"
        );

        for url in [
            "socks5://127.0.0.1:1080",
            "socks5h://resolver.example:1080",
            "socks5h://192.0.2.1:1080",
            "socks5h://127.0.0.1:1080/path",
            "socks5h://127.0.0.1:1080/?extra=1",
        ] {
            garde_config::atomic_write(&path, url.as_bytes(), 0o600).unwrap();
            assert_eq!(
                configure(&path).err().unwrap().code,
                "telegram.proxy_credentials"
            );
        }
    }

    #[test]
    fn proxy_credentials_limit_decoded_authentication_bytes() {
        let (_dir, path) = private_proxy_file("");
        let egress = TelegramEgress {
            routes: vec![TelegramRoute::Vless {
                proxy_credentials_file: path.clone(),
            }],
        };
        let configure = |authority: &str| {
            let url = format!("socks5h://{authority}127.0.0.1:1080");
            garde_config::atomic_write(&path, url.as_bytes(), 0o600).unwrap();
            Telegram::with_routes("fixture-token", Some(&egress), |builder| builder)
        };

        assert!(configure("").is_ok());
        assert!(configure(&format!("{}:%C3%A9@", "%61".repeat(231))).is_ok());
        for invalid in [
            "fixture-user@".to_owned(),
            "fixture-user:@".to_owned(),
            ":fixture-pass@".to_owned(),
            "@".to_owned(),
            format!("{}:%C3%A9@", "%61".repeat(232)),
        ] {
            assert_eq!(
                configure(&invalid).err().unwrap().code,
                "telegram.proxy_credentials"
            );
        }
    }

    fn private_reply() -> Reply {
        Reply {
            envelope: Default::default(),
            locale: None,
            chat_id: "42".into(),
            text: "saved notification".into(),
        }
    }

    fn unavailable_proxy(path: std::path::PathBuf) -> Route {
        Route::new(
            &TelegramRoute::Vless {
                proxy_credentials_file: path,
            },
            |builder| builder.https_only(false),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn a_proxy_connection_failure_uses_only_the_next_configured_route() {
        let unused = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = unused.local_addr().unwrap();
        drop(unused);
        let (_dir, path) = private_proxy_file(&format!("socks5h://{address}"));
        let (mut api, worker) = server(vec![(
            200,
            json!({"ok":true,"result":{"message_id":1,"chat":{"id":42}}}),
        )]);
        let direct = api.routes.remove(0);
        api.routes.push(unavailable_proxy(path));
        let error = api.send(&private_reply()).await.unwrap_err();
        assert_eq!(error.code, "telegram.proxy_connect");
        assert!(!error.uncertain);

        api.routes.push(direct);
        api.send(&private_reply()).await.unwrap();
        let observed = worker.join().unwrap();
        assert_eq!(observed.len(), 1);
        assert_eq!(observed[0].1["text"], "saved notification");
    }

    #[tokio::test]
    async fn a_disconnected_send_is_retried_by_the_durable_outbox() {
        use crate::delivery::{Outcome, Store};

        let (mut api, worker) = server(vec![
            (0, Value::Null),
            (
                200,
                json!({"ok":true,"result":{"message_id":1,"chat":{"id":42}}}),
            ),
        ]);
        api.routes.push(Route {
            client: route::builder(false).build().unwrap(),
            kind: route::Kind::Direct,
        });
        let (dir, _) = private_proxy_file("");
        let mut store = Store::open(dir.path(), "server-test", "42").unwrap();
        store
            .consume_update(1, Some(private_reply()), 1000)
            .unwrap();
        let first = store.claim(1000).unwrap().unwrap();
        let error = api.send(&first.reply).await.unwrap_err();
        assert_eq!(error.code, "telegram.network");
        assert!(error.uncertain);
        store
            .finish(
                first.id,
                Outcome::Retry {
                    code: error.code,
                    retry_after: error.retry_after.as_secs(),
                    uncertain: error.uncertain,
                },
                1000,
            )
            .unwrap();
        drop(store);

        let mut store = Store::open(dir.path(), "server-test", "42").unwrap();
        let second = store.claim(1005).unwrap().unwrap();
        assert_eq!(first.id, second.id);
        api.send(&second.reply).await.unwrap();
        store.finish(second.id, Outcome::Delivered, 1005).unwrap();
        let observed = worker.join().unwrap();
        assert_eq!(observed[0], observed[1]);
        assert_eq!(store.health(1005).unwrap().delivered, 1);
    }

    #[tokio::test]
    async fn file_metadata_rejects_unsafe_paths_and_excess_size() {
        let paths = [
            "../fixture",
            "/fixture",
            "documents/%2e%2e/fixture",
            "documents//fixture",
            "documents/fi?le",
            "https://api.telegram.org/fixture",
        ];
        let responses = paths.iter().map(|path| (
            200, json!({"ok":true,"result":{"file_path":path}}),
        )).chain([(
            200, json!({"ok":true,"result":{"file_path":"documents/fixture.txt","file_size":MAX_TELEGRAM_FILE_BYTES + 1}}),
        )]).collect();
        let (api, worker) = server(responses);
        for _ in paths {
            assert_eq!(
                api.get_file("fixture-file-id").await.err().unwrap().code,
                "telegram.file_path"
            );
        }
        assert_eq!(
            api.get_file("fixture-file-id").await.err().unwrap().code,
            "telegram.file_size"
        );
        assert_eq!(
            api.send_document("42", "fixture.txt", &[])
                .await
                .unwrap_err()
                .code,
            "telegram.file_size"
        );
        assert_eq!(
            api.send_document("42", "fixture.txt", &vec![0; MAX_TELEGRAM_FILE_BYTES + 1])
                .await
                .unwrap_err()
                .code,
            "telegram.file_size"
        );
        assert_eq!(
            api.send_document("42", "../fixture.txt", b"fixture")
                .await
                .unwrap_err()
                .code,
            "telegram.file_name"
        );
        let requests = worker.join().unwrap();
        assert_eq!(requests.len(), paths.len() + 1);
        assert!(
            requests
                .iter()
                .all(|request| request.0.starts_with("POST /getFile "))
        );
    }

    #[tokio::test]
    async fn file_download_bounds_a_chunked_response_and_rejects_redirects() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            for header in [
                format!(
                    "HTTP/1.1 302 Found\r\nLocation: http://{address}/unexpected\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                ),
                "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n".into(),
            ] {
                let (stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = BufReader::new(stream);
                let mut line = String::new();
                request.read_line(&mut line).unwrap();
                assert!(line.starts_with("GET /file/documents/fixture.txt "));
                loop {
                    line.clear();
                    assert_ne!(request.read_line(&mut line).unwrap(), 0);
                    if line == "\r\n" {
                        break;
                    }
                }
                let stream = request.get_mut();
                stream.write_all(header.as_bytes()).unwrap();
                if header.contains("chunked") {
                    let chunk = vec![b'x'; 1024 * 1024];
                    for _ in 0..=8 {
                        stream.write_all(b"100000\r\n").unwrap();
                        if stream
                            .write_all(&chunk)
                            .and_then(|()| stream.write_all(b"\r\n"))
                            .is_err()
                        {
                            break;
                        }
                    }
                    let _ = stream.write_all(b"0\r\n\r\n");
                }
            }
        });
        let api = Telegram {
            routes: vec![Route {
                client: route::builder(false).build().unwrap(),
                kind: route::Kind::Direct,
            }],
            api_prefix: format!("http://{address}/"),
            file_prefix: format!("http://{address}/file/"),
        };
        let file = TelegramFile {
            path: "documents/fixture.txt".into(),
            size: None,
        };
        assert_eq!(
            api.download_file(&file).await.unwrap_err().code,
            "telegram.api"
        );
        assert_eq!(
            api.download_file(&file).await.unwrap_err().code,
            "telegram.file_size"
        );
        worker.join().unwrap();
    }

    #[tokio::test]
    async fn direct_ignores_ambient_proxies() {
        const CHILD: &str = "WANT_GARDE_TEST_AMBIENT_PROXY";
        if std::env::var_os(CHILD).is_none() {
            let proxy = "http://127.0.0.1:9";
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "telegram::tests::direct_ignores_ambient_proxies",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .env("NO_PROXY", "")
                .env("no_proxy", "")
                .env("ALL_PROXY", proxy)
                .env("all_proxy", proxy)
                .env("HTTP_PROXY", proxy)
                .env("http_proxy", proxy)
                .env("HTTPS_PROXY", proxy)
                .env("https_proxy", proxy)
                .output()
                .unwrap();
            assert!(
                child.status.success(),
                "{}",
                String::from_utf8_lossy(&child.stdout)
            );
            return;
        }
        let (api, worker) = server(vec![(200, json!({"ok":true,"result":[]}))]);
        assert!(api.updates(0).await.unwrap().is_empty());
        assert_eq!(worker.join().unwrap().len(), 1);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[ignore = "requires an isolated Linux network and Telegram TLS fixture"]
    async fn linux_egress_fixture() {
        let route = std::env::var("WANT_GARDE_TEST_ROUTE").expect("fixture route");
        let route: TelegramRoute = serde_json::from_str(&route).expect("valid fixture route");
        let path = std::env::var_os("WANT_GARDE_TEST_CA").expect("fixture CA");
        let pem = std::fs::read(path).expect("read fixture CA");
        let ca = reqwest::Certificate::from_pem(&pem).expect("valid fixture CA");
        let api = Telegram::with_routes(
            "fixture-token",
            Some(&TelegramEgress {
                routes: vec![route],
            }),
            |builder| builder.tls_certs_merge([ca.clone()]),
        )
        .unwrap();
        let identity = api.identity().await.unwrap();
        assert_eq!(identity.id, "42");
        assert_eq!(identity.username, "fixture_bot");
        assert!(api.updates(0).await.unwrap().is_empty());
        api.send(&Reply {
            envelope: crate::DeliveryEnvelope {
                topic_id: Some(7),
                ..Default::default()
            },
            chat_id: "-100123".into(),
            ..private_reply()
        })
        .await
        .unwrap();
        api.send_document("42", "fixture.txt", b"fixture")
            .await
            .unwrap();
        let file = api.get_file("fixture-file-id").await.unwrap();
        assert_eq!(api.download_file(&file).await.unwrap(), b"fixture");
    }

    #[tokio::test]
    async fn sends_copyable_monospace_unicode_without_parsing_markup_and_polls_only_messages() {
        let (api, worker) = server(vec![
            (
                200,
                json!({"ok":true,"result":{"id":12345,"is_bot":true,"username":"test_bot"}}),
            ),
            (200, json!({"ok":true,"result":{"url":""}})),
            (
                200,
                json!({"ok":true,"result":[{"update_id":42,"message":{"text":"/status"}}]}),
            ),
            (
                200,
                json!({"ok":true,"result":{"message_id":1,"chat":{"id":123456789}}}),
            ),
        ]);
        assert_eq!(api.identity().await.unwrap().username, "test_bot");
        assert_eq!(api.updates(42).await.unwrap()[0]["update_id"], 42);
        api.send(&Reply {
            envelope: Default::default(),
            locale: None,
            chat_id: "123456789".into(),
            text: "שלום <host> مرحبا 🛡".into(),
        })
        .await
        .unwrap();
        let observed = worker.join().unwrap();
        assert_eq!(observed[2].1["allowed_updates"], json!(["message"]));
        assert_eq!(observed[2].1["offset"], 42);
        assert_eq!(observed[3].1["text"], "שלום <host> مرحبا 🛡");
        assert!(observed[3].1.get("parse_mode").is_none());
        assert_eq!(
            observed[3].1["entities"],
            json!([{"type":"pre","offset":0,"length":20}])
        );
        assert_ne!(observed[3].1["protect_content"], true);
    }

    fn forum_reply(topic_id: Option<i32>) -> Reply {
        Reply {
            locale: None,
            chat_id: "-1001234567890".into(),
            text: "حدث".into(),
            envelope: crate::DeliveryEnvelope {
                topic_id,
                silent: true,
                protect_content: true,
                redacted: true,
            },
        }
    }

    fn forum_chat(enabled: bool) -> Value {
        json!({"ok":true,"result":{"id":-1001234567890_i64,"type":"supergroup","is_forum":enabled}})
    }

    #[tokio::test]
    async fn sends_to_the_signed_forum_chat_and_only_the_configured_topic() {
        let (api, worker) = server(vec![
            (200, forum_chat(true)),
            (
                200,
                json!({"ok":true,"result":{"message_id":1,"chat":{"id":-1001234567890_i64,"is_forum":true},"message_thread_id":42}}),
            ),
            (200, forum_chat(true)),
            (
                200,
                json!({"ok":true,"result":{"message_id":2,"chat":{"id":-1001234567890_i64,"is_forum":true}}}),
            ),
        ]);
        api.send(&forum_reply(Some(42))).await.unwrap();
        api.send(&forum_reply(None)).await.unwrap();

        let observed = worker.join().unwrap();
        assert!(observed[0].0.starts_with("POST /getChat "));
        assert_eq!(observed[0].1, json!({"chat_id":"-1001234567890"}));
        assert!(observed[1].0.starts_with("POST /sendMessage "));
        assert_eq!(
            observed[1].1,
            json!({
                "chat_id":"-1001234567890","text":"حدث",
                "disable_notification":true,"protect_content":true,
                "entities":[{"type":"pre","offset":0,"length":3}],"message_thread_id":42
            }),
        );
        assert!(observed[2].0.starts_with("POST /getChat "));
        assert!(observed[3].0.starts_with("POST /sendMessage "));
        assert!(observed[3].1.get("message_thread_id").is_none());
    }

    #[tokio::test]
    async fn forum_rejection_does_not_send_to_general_or_prevent_a_private_send() {
        let (api, worker) = server(vec![
            (200, forum_chat(false)),
            (200, forum_chat(true)),
            (
                400,
                json!({"ok":false,"description":"private_remote_description"}),
            ),
            (
                200,
                json!({"ok":true,"result":{"message_id":3,"chat":{"id":123456789}}}),
            ),
        ]);
        let error = api.send(&forum_reply(None)).await.unwrap_err();
        assert_eq!(error.kind, FailureKind::Recipient);
        assert_eq!(error.code, "telegram.forum_required");
        let error = api.send(&forum_reply(Some(42))).await.unwrap_err();
        assert_eq!(error.kind, FailureKind::Recipient);
        assert_eq!(error.code, "telegram.forum_topic");
        assert!(!format!("{error:?}").contains("private_remote_description"));
        api.send(&Reply {
            locale: None,
            chat_id: "123456789".into(),
            text: "Private".into(),
            envelope: Default::default(),
        })
        .await
        .unwrap();

        let observed = worker.join().unwrap();
        assert!(observed[0].0.starts_with("POST /getChat "));
        assert!(observed[1].0.starts_with("POST /getChat "));
        assert_eq!(observed[2].1["message_thread_id"], 42);
        assert_eq!(observed[3].1["chat_id"], "123456789");
        assert!(observed[3].1.get("message_thread_id").is_none());
        assert_eq!(observed[3].1["disable_notification"], false);
        assert_eq!(observed[3].1["protect_content"], false);
    }

    #[tokio::test]
    async fn successful_send_to_the_wrong_thread_is_terminal_but_incomplete_ack_can_retry() {
        let chat = json!({"id":-1001234567890_i64,"is_forum":true});
        for (result, kind, code) in [
            (
                json!({"message_id":1,"chat":chat,"message_thread_id":43}),
                FailureKind::Recipient,
                "telegram.forum_response",
            ),
            (
                json!({"message_id":1,"chat":chat}),
                FailureKind::Recipient,
                "telegram.forum_response",
            ),
            (
                json!({"chat":chat,"message_thread_id":42}),
                FailureKind::Retry,
                "telegram.response",
            ),
        ] {
            let (api, worker) = server(vec![
                (200, forum_chat(true)),
                (200, json!({"ok":true,"result":result})),
            ]);
            let error = api.send(&forum_reply(Some(42))).await.unwrap_err();
            assert_eq!(error.kind, kind);
            assert_eq!(error.code, code);
            assert_eq!(error.uncertain, kind == FailureKind::Retry);
            assert_eq!(worker.join().unwrap()[1].1["message_thread_id"], 42);
        }
    }

    #[tokio::test]
    async fn general_acknowledgement_from_a_chat_that_disabled_topics_is_terminal() {
        let (api, worker) = server(vec![
            (200, forum_chat(true)),
            (
                200,
                json!({"ok":true,"result":{"message_id":4,"chat":{"id":-1001234567890_i64,"type":"supergroup"}}}),
            ),
        ]);
        let error = api.send(&forum_reply(None)).await.unwrap_err();
        assert_eq!(error.kind, FailureKind::Recipient);
        assert_eq!(error.code, "telegram.forum_response");
        assert!(!error.uncertain);
        assert!(
            worker.join().unwrap()[1]
                .1
                .get("message_thread_id")
                .is_none()
        );
    }

    #[tokio::test]
    async fn response_errors_never_expose_remote_descriptions_and_rate_limits_are_bounded() {
        let (api, worker) = server(vec![(
            429,
            json!({"ok":false,"description":"synthetic_secret_do_not_log","parameters":{"retry_after":7}}),
        )]);
        let error = api.updates(0).await.unwrap_err();
        assert_eq!(error.code, "telegram.rate_limited");
        assert_eq!(error.retry_after, Duration::from_secs(7));
        assert!(!format!("{error:?}").contains("synthetic_secret"));
        worker.join().unwrap();
        let (api, worker) = server(vec![(
            401,
            json!({"ok":false,"description":"synthetic_secret_do_not_log"}),
        )]);
        let error = api.updates(0).await.unwrap_err();
        assert_eq!(error.kind, FailureKind::Fatal);
        assert_eq!(error.code, "telegram.unauthorized");
        worker.join().unwrap();
    }

    #[tokio::test]
    async fn recipient_failures_are_permanent_and_ambiguous_success_keeps_retry_semantics() {
        for status in [400, 403] {
            let (api, worker) = server(vec![(
                status,
                json!({"ok":false,"description":"private_remote_description"}),
            )]);
            let error = api
                .send(&Reply {
                    envelope: Default::default(),
                    locale: None,
                    chat_id: "123456789".into(),
                    text: "result".into(),
                })
                .await
                .unwrap_err();
            assert_eq!(error.kind, FailureKind::Recipient);
            assert!(!error.uncertain);
            assert!(!format!("{error:?}").contains("private_remote_description"));
            worker.join().unwrap();
        }
        let (api, worker) = server(vec![(
            200,
            json!({"ok":true,"result":{"message_id":1,"chat":{"id":987654321}}}),
        )]);
        let error = api
            .send(&Reply {
                envelope: Default::default(),
                locale: None,
                chat_id: "123456789".into(),
                text: "result".into(),
            })
            .await
            .unwrap_err();
        assert_eq!(error.kind, FailureKind::Retry);
        assert!(error.uncertain);
        worker.join().unwrap();
        let (api, worker) = server(vec![(
            429,
            json!({"ok":false,"parameters":{"retry_after":7200}}),
        )]);
        let error = api
            .send(&Reply {
                envelope: Default::default(),
                locale: None,
                chat_id: "123456789".into(),
                text: "result".into(),
            })
            .await
            .unwrap_err();
        assert_eq!(error.retry_after, Duration::from_secs(7200));
        assert!(!error.uncertain);
        worker.join().unwrap();
    }
}
