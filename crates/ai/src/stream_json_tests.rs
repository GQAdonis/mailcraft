// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A structured answer streamed (#ai, ai-cited-answers-v2 6.4), and apfel's `/health` asked
//! (ai-provider-chain 5.2), against a fake server in this process on 127.0.0.1: the format goes
//! with the streamed request, the JSON comes back in its pieces and whole, a refusal of the
//! format is a 400 before any piece; `/health` is a GET with no body and no key, at the root.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;

use crate::client::{chat_stream_json, health};
use crate::config::{Carries, ProviderConfig, ProviderKind, Timeouts};
use crate::error::AiError;
use crate::key::ApiKey;
use crate::on_device::APPLE_MODEL;
use crate::wire::{ChatRequest, Message, ResponseFormat};

/// Not anybody's key.
const KEY: &str = "sk-test-not-a-real-key-0123456789";

/// A fake server that keeps each request (head and body) and answers it with `script`.
fn fake(script: impl Fn(&str, &mut dyn Write) + Send + Sync + 'static) -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let kept = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let mut bytes = Vec::new();
            let mut buf = [0u8; 4096];
            let mut end = None;
            while end.is_none_or(|end| bytes.len() < end) {
                if end.is_none()
                    && let Some(at) = bytes.windows(4).position(|w| w == b"\r\n\r\n")
                {
                    let head = String::from_utf8_lossy(&bytes[..at]).to_ascii_lowercase();
                    let length =
                        head.lines().find_map(|l| l.strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap())).unwrap_or(0);
                    end = Some(at + 4 + length);
                    continue;
                }
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => bytes.extend_from_slice(&buf[..n]),
                }
            }
            let request = String::from_utf8_lossy(&bytes).into_owned();
            kept.lock().unwrap().push(request.clone());
            script(&request, &mut stream);
        }
    });
    (port, seen)
}

fn local(port: u16) -> ProviderConfig {
    ProviderConfig {
        id: "fake".into(),
        kind: ProviderKind::Local,
        base_url: format!("http://127.0.0.1:{port}"),
        model: "test-model".into(),
        timeouts: Timeouts { connect_secs: 2, first_byte_secs: 5, idle_secs: 5, total_secs: 20 },
        ..ProviderConfig::default()
    }
}

fn structured() -> ChatRequest {
    ChatRequest {
        messages: vec![Message::system("Answer as JSON."), Message::user("When is the review?")],
        response_format: Some(ResponseFormat::JsonSchema { name: "mail_answer".into(), schema: json!({"type": "object"}), strict: true }),
        ..ChatRequest::default()
    }
}

fn reply(out: &mut dyn Write, status: &str, body: &str) {
    let _ = out.write_all(
        format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
            .as_bytes(),
    );
    let _ = out.flush();
}

#[test]
fn a_structured_answer_streams_its_json_in_pieces_and_whole() {
    let pieces = [r#"{"answer":"On "#, r#"Thursday [1]."#, r#"","citations":[1]}"#];
    let (port, seen) = fake(move |_, out| {
        let _ = out.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n");
        for piece in pieces {
            let event = json!({"choices": [{"delta": {"content": piece}, "finish_reason": null}]});
            let _ = out.write_all(format!("data: {event}\n\n").as_bytes());
        }
        let _ = out.write_all(b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n");
    });
    let mut got = Vec::new();
    let whole = chat_stream_json(&local(port), Carries::OwnerTextOnly, &structured(), &AtomicBool::new(false), &mut |text: &str| {
        got.push(text.to_string());
        true
    })
    .unwrap();
    assert_eq!(got, pieces);
    assert_eq!(whole, pieces.concat());
    let sent = seen.lock().unwrap()[0].clone();
    assert!(sent.contains(r#""stream":true"#) && sent.contains(r#""type":"json_schema""#), "the format goes with the streamed request");
}

#[test]
fn a_refused_format_is_a_400_before_any_piece_and_no_format_is_no_structured_answer() {
    let (port, seen) = fake(|_, out| reply(out, "400 Bad Request", r#"{"error":{"message":"json_schema not supported"}}"#));
    let mut pieces = 0;
    let refused = chat_stream_json(&local(port), Carries::OwnerTextOnly, &structured(), &AtomicBool::new(false), &mut |_: &str| {
        pieces += 1;
        true
    });
    assert!(matches!(refused, Err(AiError::Status { status: 400, .. })), "{refused:?}");
    assert_eq!(pieces, 0);
    let plain = ChatRequest { response_format: None, ..structured() };
    let asked = seen.lock().unwrap().len();
    assert!(matches!(
        chat_stream_json(&local(port), Carries::OwnerTextOnly, &plain, &AtomicBool::new(false), &mut |_: &str| true),
        Err(AiError::Config(_))
    ));
    assert_eq!(seen.lock().unwrap().len(), asked, "nothing sent");
}

#[test]
fn health_is_a_get_at_the_root_with_no_body_and_no_key() {
    let (port, seen) = fake(|request, out| {
        if request.starts_with("GET /health ") {
            reply(out, "200 OK", &json!({"status": "ok", "model": APPLE_MODEL, "context_window": 4096}).to_string());
        } else {
            reply(out, "404 Not Found", "{}");
        }
    });
    // Even with a key in hand, none goes to /health.
    let config = ProviderConfig { base_url: format!("http://127.0.0.1:{port}/v1"), api_key: Some(ApiKey::new(KEY).unwrap()), ..local(port) };
    let said = health(&config, &AtomicBool::new(false)).unwrap();
    assert_eq!(said.context_window, Some(4_096));
    let sent = seen.lock().unwrap()[0].clone();
    assert!(sent.starts_with("GET /health HTTP/1.1"), "{sent}");
    assert!(!sent.contains(KEY) && !sent.to_ascii_lowercase().contains("authorization"));
    assert!(!sent.to_ascii_lowercase().contains("content-length: ") || sent.to_ascii_lowercase().contains("content-length: 0"));
    // Only a server on this computer is asked.
    let remote = ProviderConfig { kind: ProviderKind::Remote, base_url: "https://api.example.org/v1".into(), ..local(port) };
    assert!(matches!(health(&remote, &AtomicBool::new(false)), Err(AiError::Config(_))));
}

#[test]
fn a_look_alike_health_is_refused() {
    let (port, _) = fake(|_, out| reply(out, "200 OK", r#"{"status":"ok","model":"llama3.2:latest"}"#));
    assert!(matches!(health(&local(port), &AtomicBool::new(false)), Err(AiError::Malformed(_))));
}
