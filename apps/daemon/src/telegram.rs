use crate::Reply;
use crate::runtime::{Identity, Transport};
use garde_config::Token;
use reqwest::{Client, redirect::Policy, retry};
use serde_json::{Value, json};
use std::time::Duration;

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
    client: Client,
    prefix: String,
}
impl Telegram {
    pub fn new(token: Token) -> Result<Self, TransportError> {
        let client = client(true)?;
        Ok(Self {
            client,
            prefix: format!("https://api.telegram.org/bot{}/", token.as_str()),
        })
    }

    async fn call(&self, method: &str, data: Value) -> Result<Value, TransportError> {
        let mut response = self
            .client
            .post(format!("{}{method}", self.prefix))
            .json(&data)
            .send()
            .await
            .map_err(|_| TransportError::transient("telegram.network"))?;
        let status = response.status().as_u16();
        if matches!(status, 401 | 404) {
            return Err(TransportError::fatal("telegram.unauthorized"));
        }
        if status == 409 {
            return Err(TransportError::fatal("telegram.conflict"));
        }
        if method == "sendMessage" && matches!(status, 400 | 403) {
            return Err(TransportError::recipient(if status == 403 {
                "telegram.forbidden"
            } else {
                "telegram.bad_request"
            }));
        }
        if response.content_length().is_some_and(|n| n > 1048576) {
            return Err(TransportError::transient("telegram.response"));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| TransportError::transient("telegram.network"))?
        {
            if bytes.len() + chunk.len() > 1048576 {
                return Err(TransportError::transient("telegram.response"));
            }
            bytes.extend_from_slice(&chunk);
        }
        let body: Value = serde_json::from_slice(&bytes)
            .map_err(|_| TransportError::transient("telegram.response"))?;
        if status == 429 || body["error_code"] == 429 {
            return Err(TransportError {
                code: "telegram.rate_limited",
                kind: FailureKind::Retry,
                uncertain: false,
                retry_after: Duration::from_secs(
                    body["parameters"]["retry_after"]
                        .as_u64()
                        .unwrap_or(30)
                        .max(1),
                ),
            });
        }
        if body["ok"] != true || !(200..300).contains(&status) {
            return Err(if matches!(body["error_code"].as_u64(), Some(401 | 404)) {
                TransportError::fatal("telegram.unauthorized")
            } else if body["error_code"] == 409 {
                TransportError::fatal("telegram.conflict")
            } else if method == "sendMessage"
                && matches!(body["error_code"].as_u64(), Some(400 | 403))
            {
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
        let result = self
            .call(
                "sendMessage",
                json!({"chat_id":reply.chat_id,"text":reply.text,"entities":[{"type":"pre","offset":0,"length":reply.text.trim_end().encode_utf16().count()}],"protect_content":false}),
            )
            .await?;
        if result["message_id"].as_i64().is_none()
            || result["chat"]["id"]
                .as_u64()
                .map(|n| n.to_string())
                .as_deref()
                != Some(reply.chat_id.as_str())
        {
            return Err(TransportError::transient("telegram.response"));
        }
        Ok(())
    }
}

fn client(https_only: bool) -> Result<Client, TransportError> {
    Client::builder()
        .https_only(https_only)
        .redirect(Policy::none())
        .no_proxy()
        .retry(retry::never())
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(35))
        .build()
        .map_err(|_| TransportError::fatal("telegram.client"))
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
                let body = serde_json::to_vec(&body).unwrap();
                write!(reader.get_mut(),"HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).unwrap();
                reader.get_mut().write_all(&body).unwrap();
            }
            requests
        });
        (
            Telegram {
                client: client(false).unwrap(),
                prefix: format!("http://{address}/"),
            },
            worker,
        )
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
