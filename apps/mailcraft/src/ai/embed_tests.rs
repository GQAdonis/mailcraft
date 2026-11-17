// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Embeddings on the desktop (#ai): only with an embeddings model the owner named, only the
//! accounts a provider elsewhere was allowed, the question as the owner's words alone, against a
//! fake provider on this machine. No real provider, no real key.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mailcraft_ai::{ProviderConfig, ProviderKind, Timeouts};
use mailcraft_engine::Session;
use mailcraft_model::Mailbox;
use mailcraft_model::ai::ErrorKind;
use mailcraft_ui_egui::{EmbedJob, EmbedTarget, MailApp, Services};
use serde_json::{Value, json};

use super::embed::{query, route_allows, run_batch, target};
use super::settings::{AiSettings, Shared};

/// Wednesday 2026-10-14, 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// A model list of models on this computer alone (the route check passes it).
const LOCAL_LIST: &str = r#"{"object":"list","data":[{"id":"test-model","owned_by":"library"},{"id":"test-embed","owned_by":"library"}]}"#;
/// A "local" server's list naming a hosted service's routes: a gateway posing as local (RELAY-001).
const HOSTED_LIST: &str =
    r#"{"object":"list","data":[{"id":"openai/gpt-5","owned_by":"openai"},{"id":"alibaba-token/qwen4-max","owned_by":"openai"}]}"#;
/// Several publishers: the owner would have to be asked (RELAY-002), which embeddings never do.
const ASK_LIST: &str = r#"{"object":"list","data":[{"id":"llama3.2:latest","owned_by":"library"},{"id":"hf.co/bartowski/Llama-3.2-3B-Instruct-GGUF:Q4_K_M","owned_by":"library"},{"id":"lmstudio-community/qwen3-8b"},{"id":"openai/gpt-oss-20b"}]}"#;

/// A fake embeddings endpoint on 127.0.0.1: one vector of `[n, 0.5]` for the n-th text of each
/// request; the bodies it was sent are kept. Its model list is [`LOCAL_LIST`].
fn endpoint() -> (u16, Arc<Mutex<Vec<Value>>>) {
    let (port, seen, _) = endpoint_listing(LOCAL_LIST);
    (port, seen)
}

/// [`endpoint`] whose `GET /v1/models` answers `models`; the third value counts those lists (they
/// carry no mail and aren't kept with the bodies).
fn endpoint_listing(models: &'static str) -> (u16, Arc<Mutex<Vec<Value>>>, Arc<AtomicUsize>) {
    let lists = Arc::new(AtomicUsize::new(0));
    let listed = lists.clone();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let kept = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let mut bytes = Vec::new();
            let mut buf = [0u8; 16 * 1024];
            let mut whole = None;
            while whole.is_none_or(|end| bytes.len() < end) {
                if whole.is_none()
                    && let Some(at) = bytes.windows(4).position(|w| w == b"\r\n\r\n")
                {
                    let head = String::from_utf8_lossy(&bytes[..at]).to_ascii_lowercase();
                    let length =
                        head.lines().find_map(|l| l.strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap())).unwrap_or(0);
                    whole = Some(at + 4 + length);
                    continue;
                }
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => bytes.extend_from_slice(&buf[..n]),
                }
            }
            if bytes.starts_with(b"GET /v1/models ") {
                listed.fetch_add(1, Ordering::SeqCst);
                let _ = stream.write_all(
                    format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{models}", models.len()).as_bytes(),
                );
                continue;
            }
            let at = bytes.windows(4).position(|w| w == b"\r\n\r\n").map_or(bytes.len(), |at| at + 4);
            let body: Value = serde_json::from_slice(&bytes[at..]).unwrap_or(Value::Null);
            let count = body["input"].as_array().map_or(0, Vec::len);
            kept.lock().unwrap().push(body);
            let data: Vec<Value> = (0..count).map(|n| json!({"index": n, "embedding": [n as f64, 0.5]})).collect();
            let answer = json!({"data": data, "model": "test-embed"}).to_string();
            let _ = stream.write_all(
                format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{answer}", answer.len()).as_bytes(),
            );
        }
    });
    (port, seen, lists)
}

fn provider(port: u16, kind: ProviderKind, embedding_model: &str) -> ProviderConfig {
    let base_url = match kind {
        ProviderKind::Local | ProviderKind::Gateway => format!("http://127.0.0.1:{port}"),
        ProviderKind::Remote => format!("https://127.0.0.1:{port}/v1"),
    };
    ProviderConfig {
        id: "test-provider".into(),
        kind,
        base_url,
        model: "test-model".into(),
        embedding_model: embedding_model.into(),
        timeouts: Timeouts { connect_secs: 2, first_byte_secs: 5, idle_secs: 5, total_secs: 20 },
        ..ProviderConfig::default()
    }
}

fn shared(config: ProviderConfig, consented: &[&str]) -> Shared {
    let shared = Shared::open(None);
    let id = config.id.clone();
    shared.change(|s| s.set_provider(config)).unwrap();
    for account in consented {
        shared.change(|s| s.set_consent(&id, account, true)).unwrap();
    }
    shared.change(|s| s.set_active(&id)).unwrap();
    shared
}

fn job(texts: &[&str], accounts: &[&str]) -> EmbedJob {
    EmbedJob {
        provider: "test-provider".into(),
        model: "test-embed".into(),
        texts: texts.iter().map(|t| t.to_string()).collect(),
        accounts: accounts.iter().map(|a| a.to_string()).collect(),
        cancel: Arc::new(AtomicBool::new(false)),
    }
}

#[test]
fn passages_are_embedded_only_with_an_embeddings_model_the_owner_named() {
    assert_eq!(target(&AiSettings::default()), None);
    assert_eq!(target(&shared(provider(1, ProviderKind::Local, ""), &[]).snapshot()), None);
    assert_eq!(
        target(&shared(provider(1, ProviderKind::Local, "test-embed"), &[]).snapshot()),
        Some(EmbedTarget { provider: "test-provider".into(), model: "test-embed".into(), accounts: None })
    );
    // Elsewhere: only the accounts allowed, and none at all when none is.
    assert_eq!(target(&shared(provider(1, ProviderKind::Remote, "test-embed"), &[]).snapshot()), None);
    let allowed = target(&shared(provider(1, ProviderKind::Remote, "test-embed"), &["Owner@example.org"]).snapshot()).unwrap();
    assert_eq!(allowed.accounts, Some(vec!["owner@example.org".to_string()]));
    // A gateway on this computer sends mail to the internet: the same as elsewhere.
    assert_eq!(target(&shared(provider(1, ProviderKind::Gateway, "test-embed"), &[]).snapshot()), None);
    let through = target(&shared(provider(1, ProviderKind::Gateway, "test-embed"), &["owner@example.org"]).snapshot()).unwrap();
    assert_eq!(through.accounts, Some(vec!["owner@example.org".to_string()]));
}

#[test]
fn a_batch_comes_back_one_vector_a_text_in_order() {
    let (port, seen) = endpoint();
    let settings = shared(provider(port, ProviderKind::Local, "test-embed"), &[]);
    let vectors = run_batch(&job(&["first passage", "second passage"], &["owner@example.org"]), &settings, None).unwrap();
    assert_eq!(vectors, vec![vec![0.0, 0.5], vec![1.0, 0.5]]);
    assert_eq!(seen.lock().unwrap()[0], json!({"model": "test-embed", "input": ["first passage", "second passage"]}));
}

#[test]
fn a_batch_for_another_provider_or_an_account_not_allowed_sends_nothing() {
    let (port, seen) = endpoint();
    let settings = shared(provider(port, ProviderKind::Local, "other-embed"), &[]);
    assert!(run_batch(&job(&["x"], &[]), &settings, None).is_err(), "the model changed since the batch was asked for");
    let remote = shared(provider(port, ProviderKind::Remote, "test-embed"), &["owner@example.org"]);
    let refused = run_batch(&job(&["x"], &["other@example.org"]), &remote, None).unwrap_err();
    assert!(refused.contains("hasn't been allowed"), "{refused}");
    std::thread::sleep(Duration::from_millis(100));
    assert!(seen.lock().unwrap().is_empty());
}

#[test]
fn the_question_is_embedded_as_the_owners_words_and_a_failure_is_none() {
    let (port, seen) = endpoint();
    let settings = shared(provider(port, ProviderKind::Local, "test-embed"), &[]);
    assert_eq!(query("when is the review?", &settings, None), Some((vec![0.0, 0.5], "test-embed".to_string())));
    assert_eq!(seen.lock().unwrap()[0]["input"], json!(["when is the review?"]));
    // No embeddings model: nothing asked.
    assert_eq!(query("x", &shared(provider(port, ProviderKind::Local, ""), &[]), None), None);
    assert_eq!(seen.lock().unwrap().len(), 1);
    // A provider that is down: none, at once.
    let closed = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let started = Instant::now();
    assert_eq!(query("x", &shared(provider(closed, ProviderKind::Local, "test-embed"), &[]), None), None);
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn base_002_a_gateway_posing_as_local_gets_no_embeddings_request() {
    for list in [HOSTED_LIST, ASK_LIST] {
        let (port, seen, lists) = endpoint_listing(list);
        let settings = shared(provider(port, ProviderKind::Local, "test-embed"), &[]);
        let refused = run_batch(&job(&["a passage of mail"], &["owner@example.org"]), &settings, None).unwrap_err();
        assert!(refused.starts_with("Nothing was sent"), "{refused}");
        assert_eq!(query("when is the review?", &settings, None), None);
        // The check itself: a typed kind, and no question put to the owner.
        let config = settings.snapshot().active().cloned().unwrap();
        let failure = route_allows(&config, &settings, &AtomicBool::new(false)).unwrap_err();
        assert_eq!(failure.kind, ErrorKind::Settings);
        assert!(settings.gateway_question().is_none(), "embeddings never ask");
        std::thread::sleep(Duration::from_millis(100));
        assert!(lists.load(Ordering::SeqCst) >= 1, "the list was read");
        assert!(seen.lock().unwrap().is_empty(), "zero /embeddings requests");
    }
}

#[test]
fn base_002_negative_control_a_plain_local_server_is_sent_the_embeddings() {
    let (port, seen, lists) = endpoint_listing(LOCAL_LIST);
    let settings = shared(provider(port, ProviderKind::Local, "test-embed"), &[]);
    assert_eq!(run_batch(&job(&["a passage of mail"], &["owner@example.org"]), &settings, None).unwrap(), vec![vec![0.0, 0.5]]);
    assert!(query("when is the review?", &settings, None).is_some());
    assert_eq!(seen.lock().unwrap().len(), 2, "both /embeddings requests arrived");
    assert_eq!(lists.load(Ordering::SeqCst), 1, "checked once a session");
}

/// A window whose host hands every embedding batch to the test, for `target`.
fn window(target: Option<EmbedTarget>) -> (MailApp, std::sync::mpsc::Receiver<(EmbedJob, Sender<Result<Vec<Vec<f32>>, String>>)>, Arc<AtomicUsize>) {
    let (jobs_tx, jobs_rx) = channel();
    let jobs_tx = Mutex::new(jobs_tx);
    let asked = Arc::new(AtomicUsize::new(0));
    let counted = asked.clone();
    let services = Services {
        embed_target: Some(Box::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
            target.clone()
        })),
        embed_batch: Some(Box::new(move |job: EmbedJob| {
            let (tx, rx) = channel();
            jobs_tx.lock().unwrap().send((job, tx)).unwrap();
            rx
        })),
        ..Default::default()
    };
    (MailApp::new(Session::at(Mailbox::default(), NOW, 0), services), jobs_rx, asked)
}

#[test]
fn without_an_embeddings_model_the_window_asks_nothing() {
    let ctx = egui::Context::default();
    let (mut app, jobs, asked) = window(None);
    for _ in 0..3 {
        app.model_tick(&ctx);
    }
    assert!(asked.load(Ordering::SeqCst) >= 3, "the target is looked at every frame");
    assert!(jobs.try_recv().is_err());
}

#[test]
fn an_empty_mailbox_has_no_batch_to_hand_out() {
    let ctx = egui::Context::default();
    let target = EmbedTarget { provider: "test-provider".into(), model: "test-embed".into(), accounts: None };
    let (mut app, jobs, _) = window(Some(target));
    for _ in 0..3 {
        app.model_tick(&ctx);
    }
    assert!(jobs.try_recv().is_err(), "no passage, no batch");
}
