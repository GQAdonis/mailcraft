// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The chain of providers on the desktop (#ai, ai-provider-chain 5.2–5.5, ai-cited-answers-v2
//! 6.4), against fake servers in this process on 127.0.0.1: a primary that fails before its first
//! byte with a kind that may fall back is answered by the confirmed on-device model (an "apfel"
//! fake), the request made again for its window and its sources named; nothing falls back after
//! the first byte, when the owner chose None, or to a look-alike or a changed model; a look lists
//! apfel alone and sends it nothing until the owner confirms one address; a streamed answer with a
//! schema streams its text and ends with the whole JSON. No real provider, no real key, and the
//! default port 11434 is never asked (every look names its fake ports).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mailcraft_ai::on_device::{APPLE_MODEL, identity};
use mailcraft_ai::{ProviderConfig, ProviderKind, Timeouts, parse_health, parse_models};
use mailcraft_host::{SecretError, SecretStore};
use mailcraft_model::ai::{
    AiKind, AnswerExtractor, AnswerInput, AnsweredBy, ChatMessage, ErrorKind, ModelEvent, ModelRequest, Passage, Role, answer_schema, marker,
};
use mailcraft_ui_egui::ModelJob;
use serde_json::{Value, json};

use super::device::{self, PROVIDER_ID};
use super::keys::Broker;
use super::run::run;
use super::settings::{Backup, OnDeviceBinding, Shared, device_address};

const APFEL_MODELS: &str = r#"{"object":"list","data":[{"id":"apple-foundationmodel","object":"model","owned_by":"apple"}]}"#;
const OLLAMA_MODELS: &str = r#"{"object":"list","data":[{"id":"llama3.2:latest","owned_by":"library"}]}"#;
const PRIMARY_MODELS: &str = r#"{"object":"list","data":[{"id":"test-model","owned_by":"library"}]}"#;
const RELAY_001: &str = r#"{"object":"list","data":[{"id":"alibaba-token/qwen4-max","owned_by":"openai"}]}"#;

fn apfel_health(window: u32) -> String {
    json!({"status": "ok", "model": APPLE_MODEL, "model_available": true, "context_window": window}).to_string()
}

/// A fake server: `GET /v1/models` answers `models`, `GET /health` answers `health` (or 404), any
/// other request is a chat request answered by `chat`; each counted, chats kept.
struct Fake {
    port: u16,
    chats: Arc<AtomicUsize>,
    lists: Arc<AtomicUsize>,
    healths: Arc<AtomicUsize>,
    seen: Arc<Mutex<Vec<String>>>,
}

fn fake(models: impl Into<String>, health: Option<String>, chat: impl Fn(&str, &mut dyn Write) + Send + Sync + 'static) -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let models = models.into();
    let (chats, lists, healths, seen) =
        (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)), Arc::new(Mutex::new(Vec::new())));
    let (c, l, h, k) = (chats.clone(), lists.clone(), healths.clone(), seen.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let request = read_request(&mut stream);
            if request.starts_with("GET /v1/models ") {
                l.fetch_add(1, Ordering::SeqCst);
                reply(&mut stream, "200 OK", &models);
            } else if request.starts_with("GET /health ") {
                h.fetch_add(1, Ordering::SeqCst);
                match &health {
                    Some(body) => reply(&mut stream, "200 OK", body),
                    None => reply(&mut stream, "404 Not Found", "{}"),
                }
            } else {
                c.fetch_add(1, Ordering::SeqCst);
                k.lock().unwrap().push(request.clone());
                chat(&request, &mut stream);
            }
        }
    });
    Fake { port, chats, lists, healths, seen }
}

fn read_request(stream: &mut impl Read) -> String {
    let mut buf = [0u8; 64 * 1024];
    let mut seen = Vec::new();
    let mut whole = None;
    while whole.is_none_or(|end| seen.len() < end) {
        if whole.is_none()
            && let Some(at) = seen.windows(4).position(|w| w == b"\r\n\r\n")
        {
            let head = String::from_utf8_lossy(&seen[..at]).to_ascii_lowercase();
            let length = head.lines().find_map(|l| l.strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap())).unwrap_or(0);
            whole = Some(at + 4 + length);
            continue;
        }
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => seen.extend_from_slice(&buf[..n]),
        }
    }
    String::from_utf8_lossy(&seen).into_owned()
}

fn reply(out: &mut dyn Write, status: &str, body: &str) {
    let _ = out.write_all(
        format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
            .as_bytes(),
    );
    let _ = out.flush();
}

fn whole(text: &str) -> String {
    json!({"choices": [{"message": {"content": text}, "finish_reason": "stop"}]}).to_string()
}

fn answering(text: &'static str) -> impl Fn(&str, &mut dyn Write) + Send + Sync + 'static {
    move |_, out| reply(out, "200 OK", &whole(text))
}

/// Streams `pieces` as an event stream; ends it properly when `finish`.
fn stream(out: &mut dyn Write, pieces: &[&str], finish: bool) {
    let _ = out.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n");
    for piece in pieces {
        let event = json!({"choices": [{"delta": {"content": piece}, "finish_reason": null}]});
        let _ = out.write_all(format!("data: {event}\n\n").as_bytes());
        let _ = out.flush();
    }
    if finish {
        let _ = out.write_all(b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n");
    }
    let _ = out.flush();
}

/// An apfel fake answering every chat with `text`, its window `window`.
fn apfel(window: u32, text: &'static str) -> Fake {
    fake(APFEL_MODELS, Some(apfel_health(window)), answering(text))
}

fn quick() -> Timeouts {
    Timeouts { connect_secs: 2, first_byte_secs: 5, idle_secs: 5, total_secs: 20 }
}

/// Settings with `primary` (when one) as the owner's primary, and the on-device model at `apfel`
/// (when one) confirmed as the backup with the identity apfel's answers give.
fn chain(primary: Option<ProviderConfig>, apfel_port: Option<u16>, window: u32) -> Shared {
    let shared = Shared::open(None);
    if let Some(primary) = primary {
        let id = primary.id.clone();
        shared.change(|s| s.set_provider(primary)).unwrap();
        shared.change(|s| s.set_active(&id)).unwrap();
    }
    if let Some(port) = apfel_port {
        let list = parse_models(APFEL_MODELS.as_bytes()).unwrap();
        let health = parse_health(apfel_health(window).as_bytes()).unwrap();
        let binding = OnDeviceBinding { address: device_address(port), identity: identity(&list, &health).unwrap(), context_window: window };
        shared
            .change(|s| {
                s.set_backup(Backup::AppleOnDevice)?;
                s.on_device = Some(binding);
                Ok(())
            })
            .unwrap();
    }
    shared
}

fn primary_at(port: u16, kind: ProviderKind) -> ProviderConfig {
    let base_url = match kind {
        ProviderKind::Remote => format!("https://127.0.0.1:{port}/v1"),
        _ => format!("http://127.0.0.1:{port}"),
    };
    ProviderConfig { id: "test-provider".into(), kind, base_url, model: "test-model".into(), timeouts: quick(), ..ProviderConfig::default() }
}

fn closed_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn narrow_job() -> ModelJob {
    let messages = vec![ChatMessage { role: Role::User, content: "unread from Ana".into() }];
    let mut request = ModelRequest::new(AiKind::Narrow, messages, vec![], false, Some(64), Some(0.0), None).unwrap();
    request.id = 1;
    job(request, &[])
}

fn job(request: ModelRequest, accounts: &[&str]) -> ModelJob {
    ModelJob { request, accounts: accounts.iter().map(|a| a.to_string()).collect(), cancel: Arc::new(AtomicBool::new(false)) }
}

/// An answer request as `ai.ask` makes it: `sources` passages of mail, a prompt made for
/// `window`, streamed, asked for as JSON when `json`.
fn answer_job(sources: &[u64], window: usize, json: bool) -> ModelJob {
    let input = AnswerInput {
        question: "What deposit did Harbor Hall ask for?".into(),
        passages: sources
            .iter()
            .map(|id| Passage {
                message: *id,
                subject: format!("Subject {id}"),
                from: "Harbor Hall Events <events@harborhall.example>".into(),
                date: "2026-10-14".into(),
                text: "We ask for a deposit of 500 euros. ".repeat(30),
            })
            .collect(),
        marker: marker(9),
    };
    let prompt = input.prompt(window).unwrap();
    let schema = json.then(|| answer_schema(prompt.sources.len()));
    let mut request =
        ModelRequest::new(AiKind::Answer, prompt.messages, prompt.sources, true, Some(512), Some(0.2), schema).unwrap().with_answer(input).unwrap();
    request.id = 7;
    job(request, &["owner@example.org"])
}

fn events_of(job: &ModelJob, settings: &Shared) -> Vec<ModelEvent> {
    events_with(job, settings, &Broker::new(Duration::from_secs(5)), None)
}

fn events_with(job: &ModelJob, settings: &Shared, broker: &Broker, store: Option<&Arc<dyn SecretStore>>) -> Vec<ModelEvent> {
    let mut events = Vec::new();
    run(job, settings, broker, store, &mut |event: ModelEvent| {
        events.push(event);
        true
    });
    events
}

fn by_apfel(text: Option<&str>, sources: Option<Vec<u64>>) -> ModelEvent {
    ModelEvent::Done {
        text: text.map(str::to_string),
        answered_by: Some(AnsweredBy { provider: PROVIDER_ID.into(), model: APPLE_MODEL.into() }),
        sources,
    }
}

fn kind(events: &[ModelEvent]) -> ErrorKind {
    match events {
        [.., ModelEvent::Error { kind, .. }] => *kind,
        other => panic!("no error: {other:?}"),
    }
}

/// A credential store that never answers while the test holds its gate.
struct Silent(Mutex<std::sync::mpsc::Receiver<()>>);

impl SecretStore for Silent {
    fn get(&self, _key: &str) -> Option<String> {
        let _ = self.0.lock().unwrap().recv();
        None
    }
    fn set(&self, _key: &str, _secret: &str) -> Result<(), SecretError> {
        Ok(())
    }
    fn delete(&self, _key: &str) {}
}

fn silent_store() -> (Arc<dyn SecretStore>, Sender<()>) {
    let (tx, rx) = channel();
    (Arc::new(Silent(Mutex::new(rx))), tx)
}

// ---- 5.3, 5.5: fallback before the first byte, on each kind that falls back ----

#[test]
fn a_primary_failing_before_its_first_byte_is_answered_by_the_confirmed_on_device_model() {
    let refusing = |status: &'static str| fake(PRIMARY_MODELS, None, move |_, out| reply(out, status, r#"{"error":{"message":"no"}}"#));
    let slow = fake(PRIMARY_MODELS, None, |_, out| {
        std::thread::sleep(Duration::from_millis(2_500));
        reply(out, "200 OK", &whole("too late"));
    });
    let hosted = fake(RELAY_001, None, answering("never asked"));
    let (r500, r401, r400) = (refusing("500 Internal Server Error"), refusing("401 Unauthorized"), refusing("400 Bad Request"));
    let cases: Vec<(&str, Option<ProviderConfig>, ErrorKind)> = vec![
        ("unreachable", Some(primary_at(closed_port(), ProviderKind::Local)), ErrorKind::Unreachable),
        ("refused 500", Some(primary_at(r500.port, ProviderKind::Local)), ErrorKind::Refused { status: 500 }),
        ("refused 401", Some(primary_at(r401.port, ProviderKind::Local)), ErrorKind::Refused { status: 401 }),
        ("refused 400", Some(primary_at(r400.port, ProviderKind::Local)), ErrorKind::Refused { status: 400 }),
        (
            "timed out before the first byte",
            Some(ProviderConfig { timeouts: Timeouts { first_byte_secs: 1, ..quick() }, ..primary_at(slow.port, ProviderKind::Local) }),
            ErrorKind::TimedOut { before_first_byte: true },
        ),
        ("settings (a local server listing hosted routes)", Some(primary_at(hosted.port, ProviderKind::Local)), ErrorKind::Settings),
        ("never configured", None, ErrorKind::NotConfigured),
    ];
    for (case, primary, expected) in cases {
        assert!(expected.falls_back(), "{case}");
        let backup = apfel(4_096, "From the device.");
        let settings = chain(primary, Some(backup.port), 4_096);
        if settings.snapshot().active.is_empty() {
            assert!(settings.snapshot().never_configured, "{case}");
        }
        let events = events_of(&narrow_job(), &settings);
        assert_eq!(events, [by_apfel(Some("From the device."), None)], "{case}");
        assert_eq!(backup.chats.load(Ordering::SeqCst), 1, "{case}");
    }
    assert_eq!(hosted.chats.load(Ordering::SeqCst), 0, "the hosted route was sent nothing");
}

#[test]
fn an_account_not_allowed_to_the_primary_is_answered_on_this_computer() {
    // A provider elsewhere without the account's consent: it is sent nothing (not even a
    // connection), and the mail goes to the confirmed model on this computer alone.
    let elsewhere = fake(PRIMARY_MODELS, None, answering("never asked"));
    let backup = apfel(4_096, "From the device.");
    let settings = chain(Some(primary_at(elsewhere.port, ProviderKind::Remote)), Some(backup.port), 4_096);
    let asked = answer_job(&[3], 100_000, false);
    let expected = asked.request.rebuilt_for(4_096).unwrap();
    let events = events_of(&asked, &settings);
    assert_eq!(events.last(), Some(&by_apfel(None, Some(expected.sources))));
    assert_eq!((elsewhere.chats.load(Ordering::SeqCst), elsewhere.lists.load(Ordering::SeqCst)), (0, 0));
    assert!(ErrorKind::NoConsent.falls_back());
}

#[test]
fn a_credential_store_that_never_answers_falls_back_to_the_on_device_model() {
    let primary = fake(PRIMARY_MODELS, None, answering("never asked"));
    let backup = apfel(4_096, "From the device.");
    let settings = chain(Some(primary_at(primary.port, ProviderKind::Gateway)), Some(backup.port), 4_096);
    settings
        .change(|s| {
            s.note_key("test-provider");
            s.set_consent("test-provider", "owner@example.org", true)
        })
        .unwrap();
    let (store, gate) = silent_store();
    let started = Instant::now();
    let asked = answer_job(&[3], 100_000, false);
    let events = events_with(&asked, &settings, &Broker::new(Duration::from_millis(300)), Some(&store));
    assert!(started.elapsed() < Duration::from_secs(6));
    let expected = asked.request.rebuilt_for(4_096).unwrap();
    assert_eq!(events, [ModelEvent::Delta { text: "From the device.".into() }, by_apfel(None, Some(expected.sources))]);
    assert_eq!(primary.chats.load(Ordering::SeqCst), 0);
    drop(gate);
}

#[test]
fn nothing_falls_back_after_the_first_byte() {
    // The answer stops half way, and a silence after some of it: the kind is shown, the device asked nothing.
    let cut = fake(PRIMARY_MODELS, None, |_, out| stream(out, &["On "], false));
    let stalls = fake(PRIMARY_MODELS, None, |_, out| {
        stream(out, &["On "], false);
        std::thread::sleep(Duration::from_millis(2_500));
    });
    for (primary, expected) in [
        // A stream cut short after some of it was shown broke off: it is never kept as an answer.
        (primary_at(cut.port, ProviderKind::Local), ErrorKind::BrokenOff),
        (
            ProviderConfig { timeouts: Timeouts { idle_secs: 1, ..quick() }, ..primary_at(stalls.port, ProviderKind::Local) },
            ErrorKind::TimedOut { before_first_byte: false },
        ),
    ] {
        let backup = apfel(4_096, "never asked");
        let settings = chain(Some(primary), Some(backup.port), 4_096);
        let mut asked = narrow_job();
        asked.request.stream = true;
        let events = events_of(&asked, &settings);
        assert_eq!(events.first(), Some(&ModelEvent::Delta { text: "On ".into() }));
        assert_eq!(kind(&events), expected);
        assert_eq!((backup.chats.load(Ordering::SeqCst), backup.lists.load(Ordering::SeqCst)), (0, 0), "{expected:?}");
    }
}

#[test]
fn what_doesnt_fall_back_stays_the_primarys_failure() {
    // An answer that can't be read is no failure of the provider to reach.
    let long = fake(PRIMARY_MODELS, None, |_, out| reply(out, "200 OK", "this is not an answer"));
    let backup = apfel(4_096, "never asked");
    let settings = chain(Some(primary_at(long.port, ProviderKind::Local)), Some(backup.port), 4_096);
    let events = events_of(&narrow_job(), &settings);
    assert!(!kind(&events).falls_back());
    assert_eq!(backup.chats.load(Ordering::SeqCst), 0);
}

// ---- 5.1: the owner's None, and no backup ----

#[test]
fn the_owners_none_is_respected_and_nothing_is_sent_anywhere() {
    let backup = apfel(4_096, "never asked");
    let settings = chain(Some(primary_at(closed_port(), ProviderKind::Local)), Some(backup.port), 4_096);
    settings.change(|s| s.set_active("")).unwrap();
    let snapshot = settings.snapshot();
    assert!(snapshot.explicitly_none() && !snapshot.never_configured);
    assert_eq!((snapshot.backup, snapshot.on_device.is_none()), (Backup::None, true), "a primary of None turns the backup off");
    assert!(settings.change(|s| s.set_backup(Backup::AppleOnDevice)).is_err());
    let events = events_of(&narrow_job(), &settings);
    assert_eq!(kind(&events), ErrorKind::NotConfigured);
    // No look either (owner decision 2).
    assert_eq!(device::discover_on(&settings, &[backup.port], &AtomicBool::new(false))["state"], "off");
    assert_eq!((backup.chats.load(Ordering::SeqCst), backup.lists.load(Ordering::SeqCst), backup.healths.load(Ordering::SeqCst)), (0, 0, 0));
}

#[test]
fn a_backup_of_none_is_never_asked() {
    let backup = apfel(4_096, "never asked");
    let settings = chain(Some(primary_at(closed_port(), ProviderKind::Local)), Some(backup.port), 4_096);
    settings.change(|s| s.set_backup(Backup::None)).unwrap();
    assert_eq!(kind(&events_of(&narrow_job(), &settings)), ErrorKind::Unreachable);
    assert_eq!(device::discover_on(&settings, &[backup.port], &AtomicBool::new(false))["state"], "off");
    assert_eq!((backup.chats.load(Ordering::SeqCst), backup.lists.load(Ordering::SeqCst)), (0, 0));
}

// ---- 5.2: looking, confirming, re-checking ----

#[test]
fn a_look_lists_apfel_alone_and_sends_nothing_until_one_address_is_confirmed() {
    let real = apfel(4_096, "From the device.");
    let other = apfel(4_096, "never asked");
    let ollama = fake(OLLAMA_MODELS, Some(apfel_health(4_096)), answering("never asked"));
    let lookalike_health = fake(APFEL_MODELS, Some(json!({"status": "ok", "model": "llama3.2"}).to_string()), answering("never asked"));
    let settings = chain(Some(primary_at(closed_port(), ProviderKind::Local)), None, 4_096);
    settings.change(|s| s.set_backup(Backup::AppleOnDevice)).unwrap();
    let ports = [real.port, other.port, ollama.port, lookalike_health.port, closed_port()];
    let shown = device::discover_on(&settings, &ports, &AtomicBool::new(false));
    assert_eq!(shown["state"], "found");
    let listed: Vec<&str> = shown["candidates"].as_array().unwrap().iter().map(|c| c["address"].as_str().unwrap()).collect();
    assert_eq!(listed, [device_address(real.port), device_address(other.port)]);
    assert_eq!(shown["candidates"][0]["contextWindow"], 4_096);
    assert_eq!(shown["confirmed"], Value::Null);
    // Found is not confirmed: a failing primary is not answered by a candidate.
    assert_eq!(kind(&events_of(&narrow_job(), &settings)), ErrorKind::Unreachable);
    // Look-alikes can't be confirmed; an address the look didn't find can't either.
    let stop = AtomicBool::new(false);
    assert!(device::confirm(&settings, &device_address(ollama.port), &stop).is_err());
    assert!(device::confirm(&settings, &device_address(closed_port()), &stop).is_err());
    assert!(device::confirm(&settings, "http://10.0.0.1:11434", &stop).is_err());
    device::confirm(&settings, &device_address(real.port), &stop).unwrap();
    assert_eq!(device::state(&settings, true)["state"], "using");
    assert_eq!(device::state(&settings, true)["confirmed"], device_address(real.port));
    assert_eq!(device::state(&settings, false)["state"], "unavailable");
    let events = events_of(&narrow_job(), &settings);
    assert_eq!(events, [by_apfel(Some("From the device."), None)]);
    // Only the confirmed address was sent a request; the others only the look's GETs.
    assert_eq!(real.chats.load(Ordering::SeqCst), 1);
    for unasked in [&other, &ollama, &lookalike_health] {
        assert_eq!(unasked.chats.load(Ordering::SeqCst), 0);
    }
    assert_eq!(ollama.healths.load(Ordering::SeqCst), 0, "a look-alike's list is enough to pass it by");
}

#[test]
fn a_confirmed_model_whose_identity_changed_is_not_used() {
    // The confirmed port now answers as a look-alike (another server took it).
    let lookalike = fake(OLLAMA_MODELS, Some(apfel_health(4_096)), answering("never asked"));
    let settings = chain(Some(primary_at(closed_port(), ProviderKind::Local)), Some(lookalike.port), 4_096);
    let events = events_of(&narrow_job(), &settings);
    assert_eq!(kind(&events), ErrorKind::Unreachable, "the primary's failure stands");
    assert_eq!(lookalike.chats.load(Ordering::SeqCst), 0);
    assert_eq!(device::state(&settings, true)["state"], "changed");
    // Not asked again until the owner confirms it again.
    let lists = lookalike.lists.load(Ordering::SeqCst);
    let _ = events_of(&narrow_job(), &settings);
    assert_eq!(lookalike.lists.load(Ordering::SeqCst), lists);
}

#[test]
fn identity_is_checked_once_a_run_and_again_after_an_error() {
    let backup = apfel(4_096, "From the device.");
    let settings = chain(Some(primary_at(closed_port(), ProviderKind::Local)), Some(backup.port), 4_096);
    for _ in 0..3 {
        assert_eq!(events_of(&narrow_job(), &settings), [by_apfel(Some("From the device."), None)]);
    }
    assert_eq!((backup.lists.load(Ordering::SeqCst), backup.healths.load(Ordering::SeqCst)), (1, 1), "checked on the first use of the run");
    device::after_error(&settings);
    let _ = events_of(&narrow_job(), &settings);
    assert_eq!(backup.lists.load(Ordering::SeqCst), 2, "and again after an error");
}

#[test]
fn a_confirmed_model_that_isnt_there_is_not_marked_changed() {
    let settings = chain(Some(primary_at(closed_port(), ProviderKind::Local)), Some(closed_port()), 4_096);
    assert_eq!(kind(&events_of(&narrow_job(), &settings)), ErrorKind::Unreachable);
    assert_eq!(device::state(&settings, true)["state"], "using");
}

// ---- 5.4: the window of the provider that answers; sources named after a rebuild ----

#[test]
fn an_answer_is_made_again_for_the_backups_window_and_names_its_new_sources() {
    let backup = apfel(1_024, "500 euros [1].");
    let settings = chain(Some(primary_at(closed_port(), ProviderKind::Local)), Some(backup.port), 1_024);
    let asked = answer_job(&[3, 4, 5, 6, 8], 100_000, false);
    let expected = asked.request.rebuilt_for(1_024).unwrap();
    assert!(expected.sources.len() < asked.request.sources.len(), "a small window keeps fewer passages");
    assert_eq!(expected.id, asked.request.id);
    let events = events_of(&asked, &settings);
    // Streamed: the pieces, then the end naming the sources of the prompt the device was sent.
    assert_eq!(events.last(), Some(&by_apfel(None, Some(expected.sources.clone()))));
    let sent = backup.seen.lock().unwrap()[0].clone();
    let body: Value = serde_json::from_str(&sent[sent.find("\r\n\r\n").unwrap() + 4..]).unwrap();
    assert_eq!(body["max_tokens"], expected.max_tokens);
    assert_eq!(body["messages"].as_array().unwrap().len(), expected.messages.len());
    assert_eq!(body["messages"][1]["content"].as_str().unwrap(), expected.messages[1].content);
}

#[test]
fn the_window_said_by_the_device_is_the_one_used() {
    // Confirmed at 4,096; the device now says 2,048 (same identity): the rebuild uses 2,048.
    let backup = apfel(2_048, "500 euros [1].");
    let settings = chain(Some(primary_at(closed_port(), ProviderKind::Local)), Some(backup.port), 4_096);
    let asked = answer_job(&[3, 4, 5, 6, 8], 100_000, false);
    let events = events_of(&asked, &settings);
    assert_eq!(events.last(), Some(&by_apfel(None, Some(asked.request.rebuilt_for(2_048).unwrap().sources))));
}

// ---- 6.4: a streamed answer with a schema ----

#[test]
fn a_streamed_answer_with_a_schema_streams_its_text_and_ends_with_the_whole_json() {
    let pieces = [r#"{"answer":"The deposit"#, r#" is 500 euros [1].","#, r#""citations":[1]}"#];
    let primary = fake(PRIMARY_MODELS, None, move |request, out| {
        assert!(request.contains(r#""type":"json_schema""#) && request.contains(r#""stream":true"#));
        stream(out, &pieces, true);
    });
    let settings = chain(Some(primary_at(primary.port, ProviderKind::Local)), None, 4_096);
    let asked = answer_job(&[3], 100_000, true);
    let events = events_of(&asked, &settings);
    let mut extractor = AnswerExtractor::new();
    let mut expected: Vec<ModelEvent> =
        pieces.iter().map(|p| extractor.push(p)).filter(|t| !t.is_empty()).map(|text| ModelEvent::Delta { text }).collect();
    let document: Value = serde_json::from_str(&pieces.concat()).unwrap();
    expected.push(ModelEvent::Done {
        text: Some(document.to_string()),
        answered_by: Some(AnsweredBy { provider: "test-provider".into(), model: "test-model".into() }),
        sources: None,
    });
    assert_eq!(events, expected);
    let shown: String = events.iter().filter_map(|e| if let ModelEvent::Delta { text } = e { Some(text.as_str()) } else { None }).collect();
    assert_eq!(shown, "The deposit is 500 euros [1].");
}

#[test]
fn a_streamed_answer_that_isnt_json_after_its_first_byte_is_handed_over_as_it_came_without_fallback() {
    let primary = fake(PRIMARY_MODELS, None, |_, out| stream(out, &[r#"{"answer":"The deposit is 5"#], true));
    let backup = apfel(4_096, "never asked");
    let settings = chain(Some(primary_at(primary.port, ProviderKind::Local)), Some(backup.port), 4_096);
    let events = events_of(&answer_job(&[3], 100_000, true), &settings);
    match events.last() {
        // The engine finds it unreadable, keeps what was shown and says so (kind malformed).
        Some(ModelEvent::Done { text: Some(text), .. }) => assert_eq!(text, r#"{"answer":"The deposit is 5"#),
        other => panic!("{other:?}"),
    }
    assert_eq!(backup.chats.load(Ordering::SeqCst), 0);
}

#[test]
fn a_provider_refusing_structured_streams_steps_down_to_streamed_plain_text() {
    let primary = fake(PRIMARY_MODELS, None, |request, out| {
        if request.contains("response_format") {
            reply(out, "400 Bad Request", r#"{"error":{"message":"Json mode response is not supported when enable_thinking is true"}}"#);
        } else {
            stream(out, &["The deposit ", "is 500 euros [1]."], true);
        }
    });
    let settings = chain(Some(primary_at(primary.port, ProviderKind::Local)), None, 4_096);
    let events = events_of(&answer_job(&[3], 100_000, true), &settings);
    assert_eq!(
        events,
        [
            ModelEvent::Delta { text: "The deposit ".into() },
            ModelEvent::Delta { text: "is 500 euros [1].".into() },
            ModelEvent::Done {
                text: None,
                answered_by: Some(AnsweredBy { provider: "test-provider".into(), model: "test-model".into() }),
                sources: None
            },
        ]
    );
    let seen = primary.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 3, "json_schema, json_object, then plain");
    assert!(!seen[2].contains("response_format") && !seen[2].contains("json object and nothing else"), "plain text, as before");
    // The level is kept: the next streamed answer goes plain at once.
    let _ = events_of(&answer_job(&[3], 100_000, true), &settings);
    assert_eq!(primary.chats.load(Ordering::SeqCst), 4);
}

#[test]
fn a_backup_answer_with_a_schema_starts_its_own_ladder_and_names_its_sources() {
    let pieces = [r#"{"answer":"500 euros [1].","citations":[1]}"#];
    let backup = fake(APFEL_MODELS, Some(apfel_health(1_024)), move |request, out| {
        assert!(request.contains(r#""type":"json_schema""#));
        stream(out, &pieces, true);
    });
    let settings = chain(Some(primary_at(closed_port(), ProviderKind::Local)), Some(backup.port), 1_024);
    let asked = answer_job(&[3, 4, 5, 6, 8], 100_000, true);
    let rebuilt = asked.request.rebuilt_for(1_024).unwrap();
    let events = events_of(&asked, &settings);
    let document: Value = serde_json::from_str(pieces[0]).unwrap();
    assert_eq!(events.last(), Some(&by_apfel(Some(&document.to_string()), Some(rebuilt.sources))));
    // The schema sent is the one for the new number of messages.
    let sent = backup.seen.lock().unwrap()[0].clone();
    let body: Value = serde_json::from_str(&sent[sent.find("\r\n\r\n").unwrap() + 4..]).unwrap();
    let schema: Value = serde_json::from_str(rebuilt.schema.as_deref().unwrap()).unwrap();
    assert_eq!(body["response_format"]["json_schema"]["schema"], schema);
}
