// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The calls against fake providers in this process, on this machine: a streamed answer arrives
//! in parts and stops when told (SCN-STREAM-001), a provider that is down or slow is an error in
//! its time (SCN-FAIL-001), nothing goes where it may not (SCN-PRIVACY-001, SCN-LOOPBACK-001),
//! and a remote provider over TLS, trusted by its fingerprint, is sent the consented account's
//! mail alone with its key in the header alone (SCN-OPTIN-001). No real provider, no real key.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rcgen::{CertificateParams, ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose};
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use serde_json::json;

use crate::client::{chat, chat_json, chat_stream, embed, models};
use crate::config::{Carries, ProviderConfig, ProviderKind, Timeouts};
use crate::error::AiError;
use crate::key::ApiKey;
use crate::wire::{ChatRequest, Message, ResponseFormat};

/// Not anybody's key: what a test sends and looks for.
const KEY: &str = "sk-test-not-a-real-key-0123456789";

/// A fake provider: each request it takes is kept, and answered by the test's script.
struct Provider {
    port: u16,
    seen: Arc<Mutex<Vec<String>>>,
}

type Script = dyn Fn(usize, &str, &mut dyn Write) + Send + Sync;

impl Provider {
    /// Plain HTTP on 127.0.0.1.
    fn plain(script: impl Fn(usize, &str, &mut dyn Write) + Send + Sync + 'static) -> Provider {
        Provider::serve(None, Arc::new(script))
    }

    /// HTTPS on 127.0.0.1 with `config`'s certificate.
    fn tls(config: Arc<ServerConfig>, script: impl Fn(usize, &str, &mut dyn Write) + Send + Sync + 'static) -> Provider {
        Provider::serve(Some(config), Arc::new(script))
    }

    fn serve(tls: Option<Arc<ServerConfig>>, script: Arc<Script>) -> Provider {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let kept = seen.clone();
        std::thread::spawn(move || {
            for (n, stream) in listener.incoming().enumerate() {
                let Ok(mut stream) = stream else { continue };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                match &tls {
                    None => {
                        let Some(request) = read_request(&mut stream) else { continue };
                        kept.lock().unwrap().push(request.clone());
                        script(n, &request, &mut stream);
                    }
                    Some(config) => {
                        let Ok(connection) = rustls::ServerConnection::new(config.clone()) else { continue };
                        let mut tls = rustls::StreamOwned::new(connection, stream);
                        let Some(request) = read_request(&mut tls) else { continue };
                        kept.lock().unwrap().push(request.clone());
                        script(n, &request, &mut tls);
                        tls.conn.send_close_notify();
                        let _ = tls.flush();
                    }
                }
            }
        });
        Provider { port, seen }
    }

    fn asked(&self) -> usize {
        self.seen.lock().unwrap().len()
    }

    fn request(&self, n: usize) -> String {
        self.seen.lock().unwrap()[n].clone()
    }
}

/// The request's head and body (by its length), or none when the client went away first.
fn read_request(stream: &mut impl Read) -> Option<String> {
    let mut bytes = Vec::new();
    let mut buf = [0u8; 4096];
    let head_end = loop {
        if let Some(at) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => return None,
            Ok(n) => bytes.extend_from_slice(&buf[..n]),
        }
    };
    let head = String::from_utf8_lossy(&bytes[..head_end]).into_owned();
    let length = head
        .lines()
        .find_map(|line| line.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap()))
        .unwrap_or(0);
    while bytes.len() < head_end + length {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => return None,
            Ok(n) => bytes.extend_from_slice(&buf[..n]),
        }
    }
    Some(String::from_utf8_lossy(&bytes[..head_end + length]).into_owned())
}

fn sse_head(out: &mut dyn Write) {
    let _ = out.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n");
    let _ = out.flush();
}

fn event(out: &mut dyn Write, data: &str) {
    let _ = out.write_all(format!("data: {data}\n\n").as_bytes());
    let _ = out.flush();
}

fn delta(text: &str) -> String {
    json!({"choices": [{"index": 0, "delta": {"content": text}, "finish_reason": null}], "model": "test-model"}).to_string()
}

fn stop() -> String {
    json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]}).to_string()
}

fn json_reply(out: &mut dyn Write, status: &str, body: &str) {
    let _ = out.write_all(
        format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
            .as_bytes(),
    );
    let _ = out.flush();
}

fn whole(text: &str) -> String {
    json!({"choices": [{"index": 0, "message": {"role": "assistant", "content": text}, "finish_reason": "stop"}], "model": "test-model", "usage": {"prompt_tokens": 9, "completion_tokens": 3, "total_tokens": 12}}).to_string()
}

fn quick() -> Timeouts {
    Timeouts { connect_secs: 2, first_byte_secs: 5, idle_secs: 5, total_secs: 20 }
}

fn local(port: u16) -> ProviderConfig {
    ProviderConfig {
        id: "local-test".into(),
        base_url: format!("http://127.0.0.1:{port}"),
        model: "test-model".into(),
        embedding_model: "test-embed".into(),
        timeouts: quick(),
        ..ProviderConfig::default()
    }
}

fn question() -> ChatRequest {
    ChatRequest {
        messages: vec![Message::system("Answer from the excerpts only."), Message::user("When is the review with Ana?")],
        max_tokens: Some(128),
        ..ChatRequest::default()
    }
}

const MAIL: Carries<'static> = Carries::MailOf(&["owner@example.org"]);

#[test]
fn a_streamed_answer_arrives_in_parts_before_it_is_done() {
    let provider = Provider::plain(|_, _, out| {
        sse_head(out);
        let _ = out.write_all(b": keep-alive\n\n");
        for piece in ["The review ", "is on ", "Thursday."] {
            event(out, &delta(piece));
            std::thread::sleep(Duration::from_millis(120));
        }
        event(out, &stop());
        event(out, "[DONE]");
    });
    let cancel = AtomicBool::new(false);
    let mut parts: Vec<(String, Instant)> = Vec::new();
    let answer = chat_stream(&local(provider.port), MAIL, &question(), &cancel, &mut |text: &str| {
        parts.push((text.to_string(), Instant::now()));
        true
    })
    .unwrap();
    assert_eq!(answer.text, "The review is on Thursday.");
    assert_eq!(answer.finish_reason.as_deref(), Some("stop"));
    assert_eq!(answer.model.as_deref(), Some("test-model"));
    assert_eq!(parts.iter().map(|(text, _)| text.as_str()).collect::<Vec<_>>(), ["The review ", "is on ", "Thursday."]);
    assert!(parts[2].1.duration_since(parts[0].1) >= Duration::from_millis(200), "the parts came as they were sent");
    let sent = provider.request(0);
    assert!(sent.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"), "{sent}");
    assert!(sent.contains("Accept: text/event-stream\r\n") && sent.contains("Content-Type: application/json\r\n"), "{sent}");
    assert!(!sent.contains("Authorization"), "no key, no header: {sent}");
    let body: serde_json::Value = serde_json::from_str(sent.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!((body["model"].as_str(), body["stream"].as_bool(), body["max_tokens"].as_u64()), (Some("test-model"), Some(true), Some(128)));
}

#[test]
fn a_whole_answer_a_structured_one_and_embeddings() {
    let provider = Provider::plain(|n, request, out| match n {
        0 => json_reply(out, "200 OK", &whole("Thursday at ten.")),
        1 => json_reply(out, "200 OK", &whole("```json\n{\"query\":\"from:ana is:unread\"}\n```")),
        _ => {
            assert!(request.starts_with("POST /v1/embeddings "), "{request}");
            json_reply(out, "200 OK", r#"{"data":[{"index":0,"embedding":[0.5,0.25]},{"index":1,"embedding":[0.75,1.5]}],"model":"test-embed"}"#);
        }
    });
    let config = local(provider.port);
    let cancel = AtomicBool::new(false);
    let answer = chat(&config, MAIL, &question(), &cancel).unwrap();
    assert_eq!(answer.text, "Thursday at ten.");
    assert_eq!(answer.usage.map(|u| u.total_tokens), Some(12));

    let schema = json!({"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]});
    let structured =
        ChatRequest { response_format: Some(ResponseFormat::JsonSchema { name: "mail_query".into(), schema, strict: true }), ..question() };
    assert_eq!(chat_json(&config, Carries::OwnerTextOnly, &structured, &cancel).unwrap(), json!({"query": "from:ana is:unread"}));
    let sent: serde_json::Value = serde_json::from_str(provider.request(1).split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(sent["response_format"]["type"], "json_schema");
    assert!(matches!(chat_json(&config, MAIL, &question(), &cancel), Err(AiError::Config(_))), "a format must be asked for");

    let vectors = embed(&config, MAIL, &["first".to_string(), "second".to_string()], &cancel).unwrap();
    assert_eq!(vectors.vectors, vec![vec![0.5, 0.25], vec![0.75, 1.5]]);
    let sent: serde_json::Value = serde_json::from_str(provider.request(2).split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(sent, json!({"model": "test-embed", "input": ["first", "second"]}));
    // No embedding model, no request.
    let none = ProviderConfig { embedding_model: String::new(), ..config };
    assert_eq!(embed(&none, MAIL, &["x".to_string()], &cancel), Err(AiError::NotConfigured));
    assert_eq!(provider.asked(), 3);
}

#[test]
fn a_server_that_answers_whole_when_asked_to_stream_is_read_whole() {
    let provider = Provider::plain(|n, _, out| match n {
        0 => json_reply(out, "200 OK", &whole("All at once.")),
        // No Content-Type at all: an event stream is told by its first byte.
        _ => {
            let _ = out.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n");
            event(out, &delta("Sniffed."));
            event(out, "[DONE]");
        }
    });
    let cancel = AtomicBool::new(false);
    let mut parts = Vec::new();
    let answer = chat_stream(&local(provider.port), MAIL, &question(), &cancel, &mut |text: &str| {
        parts.push(text.to_string());
        true
    })
    .unwrap();
    assert_eq!((answer.text.as_str(), parts.as_slice()), ("All at once.", &["All at once.".to_string()][..]));
    let answer = chat_stream(&local(provider.port), MAIL, &question(), &cancel, &mut |_: &str| true).unwrap();
    assert_eq!(answer.text, "Sniffed.");
}

#[test]
fn nothing_is_sent_without_a_provider_or_to_an_address_that_may_not_be_spoken_to() {
    let cancel = AtomicBool::new(false);
    // No provider set up.
    assert!(matches!(chat(&ProviderConfig::default(), MAIL, &question(), &cancel), Err(AiError::Config(_))));
    // A "local" provider elsewhere, plain or not: refused before anything is dialled.
    for url in ["http://192.0.2.1:11434", "http://[::ffff:192.0.2.1]:11434", "https://192.0.2.1:11434"] {
        let config = ProviderConfig { base_url: url.into(), ..local(1) };
        let started = Instant::now();
        let error = chat(&config, MAIL, &question(), &cancel).unwrap_err();
        assert!(error.is_refused_before_sending(), "{url}: {error:?}");
        assert!(started.elapsed() < Duration::from_millis(500), "{url}");
    }
}

#[test]
fn a_remote_provider_is_sent_nothing_for_an_account_the_owner_has_not_allowed() {
    let provider = Provider::plain(|_, _, out| json_reply(out, "200 OK", &whole("should never be asked")));
    let config = ProviderConfig {
        id: "mailcraft-boundary-run1".into(),
        kind: ProviderKind::Remote,
        base_url: format!("https://127.0.0.1:{}/v1", provider.port),
        model: "remote-model".into(),
        embedding_model: "remote-embed".into(),
        consented_accounts: vec!["owner@example.org".into()],
        api_key: Some(ApiKey::new(KEY).unwrap()),
        timeouts: quick(),
        ..ProviderConfig::default()
    };
    let cancel = AtomicBool::new(false);
    for accounts in [&["other@example.org"][..], &["owner@example.org", "other@example.org"], &[]] {
        assert_eq!(chat(&config, Carries::MailOf(accounts), &question(), &cancel), Err(AiError::NoConsent), "{accounts:?}");
        assert_eq!(embed(&config, Carries::MailOf(accounts), &["x".to_string()], &cancel), Err(AiError::NoConsent));
        assert_eq!(chat_stream(&config, Carries::MailOf(accounts), &question(), &cancel, &mut |_: &str| true), Err(AiError::NoConsent));
    }
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(provider.asked(), 0);
}

/// The server settings for a self-signed certificate for this machine, and its DER.
fn self_signed() -> (Arc<ServerConfig>, Vec<u8>) {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let year = 1970 + i32::try_from(now / 31_556_952).unwrap();
    let mut params = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    params.distinguished_name.push(rcgen::DnType::CommonName, "localhost");
    params.is_ca = IsCa::ExplicitNoCa;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.not_before = rcgen::date_time_ymd(year - 1, 1, 1);
    params.not_after = rcgen::date_time_ymd(year + 1, 1, 1);
    let key = KeyPair::generate().unwrap();
    let cert = params.self_signed(&key).unwrap();
    let der = cert.der().to_vec();
    let config = ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![CertificateDer::from(der.clone())], PrivatePkcs8KeyDer::from(key.serialize_der()).into())
        .unwrap();
    (Arc::new(config), der)
}

#[test]
fn a_remote_provider_trusted_by_fingerprint_gets_the_consented_mail_and_the_key_in_the_header_alone() {
    let (server, der) = self_signed();
    let provider = Provider::tls(server, |_, _, out| {
        sse_head(out);
        event(out, &delta("Remote "));
        event(out, &delta("answer."));
        event(out, &stop());
        event(out, "[DONE]");
    });
    let fingerprint = mailcraft_net::describe_certificate(&der).unwrap().fingerprint;
    let config = ProviderConfig {
        id: "mailcraft-boundary-run1".into(),
        kind: ProviderKind::Remote,
        base_url: format!("https://localhost:{}/v1", provider.port),
        model: "remote-model".into(),
        pinned_sha256: fingerprint,
        consented_accounts: vec!["owner@example.org".into()],
        api_key: Some(ApiKey::new(KEY).unwrap()),
        timeouts: quick(),
        ..ProviderConfig::default()
    };
    let cancel = AtomicBool::new(false);
    let answer = chat_stream(&config, MAIL, &question(), &cancel, &mut |_: &str| true).unwrap();
    assert_eq!(answer.text, "Remote answer.");
    let sent = provider.request(0);
    assert!(sent.contains(&format!("Authorization: Bearer {KEY}\r\n")), "{sent}");
    let request_line = sent.lines().next().unwrap();
    assert_eq!(request_line, "POST /v1/chat/completions HTTP/1.1");
    assert_eq!(sent.matches(KEY).count(), 1, "the key is in the header and nowhere else");

    // The same server without the fingerprint: refused at the handshake, nothing sent.
    let unpinned = ProviderConfig { pinned_sha256: String::new(), ..config.clone() };
    let error = chat(&unpinned, MAIL, &question(), &cancel).unwrap_err();
    assert!(matches!(error, AiError::Certificate(_)), "{error:?}");
    assert!(!format!("{error} {error:?}").contains(KEY));
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(provider.asked(), 1);
}

#[test]
fn a_closed_port_is_an_error_within_a_second() {
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let cancel = AtomicBool::new(false);
    for base in [format!("http://127.0.0.1:{port}"), format!("http://localhost:{port}/v1")] {
        let config = ProviderConfig { base_url: base.clone(), ..local(port) };
        let started = Instant::now();
        let error = chat_stream(&config, MAIL, &question(), &cancel, &mut |_: &str| true).unwrap_err();
        assert!(matches!(error, AiError::Unreachable(_)), "{base}: {error:?}");
        assert!(started.elapsed() < Duration::from_secs(1), "{base}");
    }
}

#[test]
fn a_provider_that_does_not_begin_to_answer_runs_out_of_its_time() {
    let provider = Provider::plain(|_, _, _| std::thread::sleep(Duration::from_secs(4)));
    let config = ProviderConfig { timeouts: Timeouts { first_byte_secs: 1, ..quick() }, ..local(provider.port) };
    let cancel = AtomicBool::new(false);
    let started = Instant::now();
    let error = chat_stream(&config, MAIL, &question(), &cancel, &mut |_: &str| true).unwrap_err();
    let took = started.elapsed();
    assert!(error.is_timeout(), "{error:?}");
    assert!(took >= Duration::from_millis(900) && took < Duration::from_millis(2_500), "{took:?}");
}

#[test]
fn a_stop_closes_the_stream_within_a_second() {
    let provider = Provider::plain(|_, _, out| {
        sse_head(out);
        event(out, &delta("Partial "));
        std::thread::sleep(Duration::from_secs(8));
    });
    let config = ProviderConfig { timeouts: Timeouts { idle_secs: 60, first_byte_secs: 60, total_secs: 120, ..quick() }, ..local(provider.port) };
    let cancel = Arc::new(AtomicBool::new(false));
    let set_at: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
    let (flag, when) = (cancel.clone(), set_at.clone());
    let mut parts = Vec::new();
    let error = chat_stream(&config, MAIL, &question(), &cancel, &mut |text: &str| {
        parts.push(text.to_string());
        let (flag, when) = (flag.clone(), when.clone());
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            *when.lock().unwrap() = Some(Instant::now());
            flag.store(true, Ordering::SeqCst);
        });
        true
    })
    .unwrap_err();
    assert_eq!(error, AiError::Cancelled);
    assert_eq!(parts, ["Partial "]);
    let set = set_at.lock().unwrap().unwrap();
    assert!(set.elapsed() < Duration::from_secs(1), "stopped {:?} after the flag", set.elapsed());

    // The owner's own "enough" stops it as well.
    let provider = Provider::plain(|_, _, out| {
        sse_head(out);
        event(out, &delta("One "));
        event(out, &delta("two "));
        std::thread::sleep(Duration::from_secs(8));
    });
    let quiet = AtomicBool::new(false);
    let started = Instant::now();
    let error = chat_stream(&local(provider.port), MAIL, &question(), &quiet, &mut |_: &str| false).unwrap_err();
    assert_eq!(error, AiError::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(2));
    // A flag already set sends nothing.
    let set = AtomicBool::new(true);
    assert_eq!(chat(&local(provider.port), MAIL, &question(), &set), Err(AiError::Cancelled));
    assert_eq!(provider.asked(), 1);
}

#[test]
fn a_malformed_or_unfinished_stream_is_a_clear_error() {
    let provider = Provider::plain(|n, _, out| {
        sse_head(out);
        match n {
            0 => event(out, "{not json"),
            1 => event(out, &json!({"error": {"message": "the model crashed"}}).to_string()),
            2 => event(out, &delta("cut off before the end")),
            _ => {
                let _ = out.write_all(format!("data: {}\n", "x".repeat(300 * 1024)).as_bytes());
            }
        }
    });
    let config = local(provider.port);
    let cancel = AtomicBool::new(false);
    let ask = || chat_stream(&config, MAIL, &question(), &cancel, &mut |_: &str| true);
    assert!(matches!(ask(), Err(AiError::Malformed(_))));
    assert_eq!(ask(), Err(AiError::Provider("the model crashed".into())));
    assert!(matches!(ask(), Err(AiError::Malformed(_))));
    assert!(matches!(ask(), Err(AiError::Malformed(_))));
}

#[test]
fn an_error_status_says_what_the_provider_said_without_the_key() {
    let provider = Provider::plain(|n, _, out| match n {
        0 => json_reply(out, "401 Unauthorized", &json!({"error": {"message": format!("Incorrect API key provided: {KEY}.")}}).to_string()),
        1 => {
            let _ = out.write_all(b"HTTP/1.1 307 Temporary Redirect\r\nLocation: http://192.0.2.1/v1/chat/completions\r\nContent-Length: 0\r\n\r\n");
        }
        _ => json_reply(out, "503 Service Unavailable", "<html>busy</html>"),
    });
    let config = ProviderConfig { api_key: Some(ApiKey::new(KEY).unwrap()), ..local(provider.port) };
    let cancel = AtomicBool::new(false);
    let error = chat(&config, MAIL, &question(), &cancel).unwrap_err();
    assert_eq!(error, AiError::Status { status: 401, message: "Incorrect API key provided: ***.".into() });
    assert!(!format!("{error} {error:?}").contains(KEY));
    let error = chat(&config, MAIL, &question(), &cancel).unwrap_err();
    assert!(matches!(&error, AiError::Status { status: 307, message } if message.contains("not followed")), "{error:?}");
    assert_eq!(chat(&config, MAIL, &question(), &cancel), Err(AiError::Status { status: 503, message: String::new() }));
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(provider.asked(), 3, "the redirect was not followed");
}

#[test]
fn the_model_list_is_asked_for_with_a_get_and_no_mail() {
    let provider = Provider::plain(|_, _, out| {
        json_reply(out, "200 OK", r#"{"object":"list","data":[{"id":"llama3.2:latest","owned_by":"library"},{"id":"alibaba-token/qwen4-max"}]}"#)
    });
    let never = AtomicBool::new(false);
    let list = models(&local(provider.port), &never).unwrap();
    assert_eq!(list.models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["llama3.2:latest", "alibaba-token/qwen4-max"]);
    let asked = provider.request(0);
    assert!(asked.starts_with("GET /v1/models HTTP/1.1\r\n"), "{asked}");
    assert!(!asked.to_ascii_lowercase().contains("authorization"), "no key is held, none is sent");
    assert!(asked.ends_with("\r\n\r\n"), "no body: {asked}");
}

#[test]
fn a_model_list_that_isnt_one_is_an_error_not_a_panic() {
    let provider = Provider::plain(|n, _, out| match n {
        0 => json_reply(out, "200 OK", "[1, 2, 3]"),
        1 => json_reply(out, "401 Unauthorized", r#"{"error":{"message":"key needed"}}"#),
        _ => json_reply(out, "200 OK", &"x".repeat(2 * 1024 * 1024)),
    });
    let never = AtomicBool::new(false);
    assert!(matches!(models(&local(provider.port), &never), Err(AiError::Malformed(_))));
    assert!(matches!(models(&local(provider.port), &never), Err(AiError::Status { status: 401, .. })));
    assert!(models(&local(provider.port), &never).is_err());
}
