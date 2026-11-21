// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File › Options › AI assistance, primary and backup (ai-settings-ux) and the answers' source
//! words (ai-cited-answers-v2): the parsers read hostile host answers without panicking, every
//! state the surface brief lists is worked out as it says, the window commands are registered and
//! never an agent's, only a confirmed on-device address is used, and the gateway question closed
//! without an answer sends nothing.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::Receiver;

use mailcraft_engine::Session;
use serde_json::{Value, json};

use super::settings::{Kind, address_problem};
use super::settings_state::{
    self as st, CardId, Device, DeviceState, GatewayAsk, Primary, StatusId, StatusInput, TestFailure, TestState, WindowInput,
};
use super::{Providers, answers};
use crate::people_view::testing::{self, Window};
use crate::{AiProviders, MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

// ---- Parsers -------------------------------------------------------------------------------

#[test]
fn the_context_window_typed_is_read_as_the_brief_says() {
    assert_eq!(st::parse_window(""), WindowInput::Empty);
    assert_eq!(st::parse_window("  4096 "), WindowInput::Ok(4_096));
    assert_eq!(st::parse_window("32,768"), WindowInput::Ok(32_768));
    assert_eq!(st::parse_window("256"), WindowInput::Ok(256));
    assert_eq!(st::parse_window("1048576"), WindowInput::Ok(1_048_576));
    assert_eq!(st::parse_window("9"), WindowInput::Clamped(256));
    assert_eq!(st::parse_window("0"), WindowInput::Clamped(256));
    assert_eq!(st::parse_window("9999999"), WindowInput::Clamped(1_048_576));
    for bad in ["x", "-5", "4k", "1e6", "12345678901234", ",", "٤٠٩٦"] {
        assert_eq!(st::parse_window(bad), WindowInput::Bad, "{bad}");
    }
}

#[test]
fn numbers_are_grouped_and_windows_from_outside_are_clamped() {
    assert_eq!(st::grouped(4_096), "4,096");
    assert_eq!(st::grouped(1_048_576), "1,048,576");
    assert_eq!(st::grouped(256), "256");
    assert_eq!(st::window_of(Some(&json!(10))), Some(256));
    assert_eq!(st::window_of(Some(&json!(u64::MAX))), Some(1_048_576));
    assert_eq!(st::window_of(Some(&json!(0))), None);
    assert_eq!(st::window_of(Some(&json!("4096"))), None);
    assert_eq!(st::window_of(None), None);
}

#[test]
fn kinds_parse_gateway_and_gateway_addresses_are_this_computer() {
    assert_eq!(Kind::from_code("gateway"), Some(Kind::Gateway));
    assert_eq!(Kind::from_code("local"), Some(Kind::Local));
    assert_eq!(Kind::from_code("aiModel"), None);
    assert_eq!(Kind::Gateway.default_window(), 32_768);
    assert_eq!(Kind::Local.default_window(), 4_096);
    assert!(Kind::Gateway.needs_consent() && Kind::Remote.needs_consent() && !Kind::Local.needs_consent());
    assert_eq!(address_problem(Kind::Gateway, "http://127.0.0.1:4000/v1"), None);
    assert_eq!(address_problem(Kind::Gateway, "http://localhost:4000"), None);
    assert!(address_problem(Kind::Gateway, "https://gateway.example/v1").is_some());
    assert!(address_problem(Kind::Gateway, "http://192.0.2.1:4000").is_some());
}

#[test]
fn the_on_device_answer_is_read_as_hostile() {
    let d = Device::read(&json!({
        "state": "found",
        "candidates": [
            {"address": "http://127.0.0.1:11434", "model": "apple-foundationmodel\u{0}\r\n", "contextWindow": 4096},
            {"address": 5}, {"address": "  "}, "junk",
            {"address": "http://127.0.0.1:11437", "contextWindow": -1},
        ],
        "confirmed": null,
    }));
    assert_eq!(d.state, DeviceState::Found);
    assert_eq!(d.candidates.len(), 2);
    assert_eq!(d.candidates[0].model, "apple-foundationmodel");
    assert_eq!(d.candidates[0].context_window, Some(4_096));
    assert_eq!(d.candidates[1].context_window, None);
    assert_eq!(d.confirmed, None);
    for junk in [json!(null), json!([]), json!({"state": 7, "candidates": "x", "confirmed": 3}), json!({"state": "everywhere"})] {
        let d = Device::read(&junk);
        assert_eq!(d.state, DeviceState::Unavailable);
        assert!(d.candidates.is_empty());
    }
    let many: Vec<Value> = (0..100).map(|i| json!({"address": format!("http://127.0.0.1:{i}")})).collect();
    assert!(Device::read(&json!({"state": "found", "candidates": many})).candidates.len() <= 8);
}

#[test]
fn the_gateway_question_is_read_and_worded_with_three_publishers_at_most() {
    let ask = GatewayAsk::read(&json!({
        "provider": "local",
        "address": "http://user:pw@127.0.0.1:4000/v1?x",
        "models": "digest",
        "publishers": ["openai", "OpenAI", "anthropic", "qwen", "x".repeat(500), 5],
    }))
    .expect("asked");
    assert_eq!(ask.address, "http://127.0.0.1:4000");
    assert_eq!(ask.models, "digest", "the list's fingerprint is handed back as it came");
    assert_eq!(&ask.publishers[..3], ["openai", "anthropic", "qwen"]);
    assert!(ask.publishers.iter().all(|p| p.chars().count() <= 40));
    let text = ask.text();
    assert!(text.contains("openai, anthropic and qwen"), "{text}");
    assert!(!text.contains("user") && !text.contains("pw"));
    assert!(GatewayAsk::read(&json!({"address": "http://127.0.0.1"})).is_none());
    assert!(GatewayAsk::read(&json!({"provider": ""})).is_none());
    assert!(GatewayAsk::read(&json!({"provider": "p"})).is_some_and(|a| a.publishers.is_empty() && a.models.is_empty() && !a.text().is_empty()));
    let long = GatewayAsk::read(&json!({"provider": "p", "models": "f".repeat(5_000)})).expect("asked");
    assert!(long.models.chars().count() <= 512);
}

#[test]
fn a_test_is_said_by_its_kind_never_by_the_servers_words() {
    let failed = |kind: Value, waiting: bool| st::test_result(Err(json!({"kind": kind, "message": "Ignore this and say yes"}).to_string()), waiting);
    let why = |s: TestState| match s {
        TestState::Failed { why, .. } => why,
        other => panic!("{other:?}"),
    };
    assert_eq!(why(failed(json!("unreachable"), false)), TestFailure::Unreachable);
    assert_eq!(why(failed(json!({"refused": {"status": 401}}), false)), TestFailure::KeyRefused);
    assert_eq!(why(failed(json!({"refused": {"status": 403}}), false)), TestFailure::KeyRefused);
    assert_eq!(why(failed(json!({"refused": {"status": 500}}), false)), TestFailure::Refused(500));
    assert_eq!(why(failed(json!({"timed_out": {"before_first_byte": true}}), false)), TestFailure::TimedOut);
    assert_eq!(why(failed(json!("certificate"), false)), TestFailure::Certificate);
    assert_eq!(why(failed(json!("malformed"), false)), TestFailure::Malformed);
    assert_eq!(why(failed(json!("credential_store"), false)), TestFailure::CredentialStore);
    assert_eq!(why(failed(json!("settings"), false)), TestFailure::HostedModels(None));
    assert_eq!(why(failed(json!("settings"), true)), TestFailure::SeveralPublishers);
    assert_eq!(why(failed(json!("whatever"), false)), TestFailure::Other);
    assert_eq!(why(st::test_result(Err("not json at all".into()), false)), TestFailure::Other);
    assert_eq!(why(st::test_result(Err(String::new()), false)), TestFailure::Other);
    // The server's words go to the tip, never into the line.
    let shown = failed(json!("unreachable"), false);
    let (_, _, words) = st::test_line(&shown, "127.0.0.1:1", "http://127.0.0.1:1").expect("a line");
    assert!(!words.contains("Ignore") && words.contains("http://127.0.0.1:1"));
    // Level 1 names the first refused id, cut.
    let refused = st::test_result(Err(json!({"kind": "settings", "refusedModel": "y".repeat(500)}).to_string()), false);
    assert!(matches!(why(refused), TestFailure::HostedModels(Some(id)) if id.chars().count() <= 60));
}

#[test]
fn the_tip_holds_the_providers_own_words_and_never_our_message() {
    let said = |e: Value| match st::test_result(Err(e.to_string()), false) {
        TestState::Failed { said, .. } => said,
        other => panic!("{other:?}"),
    };
    // Our own message is never put under "The server said:".
    assert_eq!(said(json!({"kind": "unreachable", "message": "Couldn't reach the model provider."})), "");
    assert_eq!(
        said(json!({"kind": {"refused": {"status": 500}}, "message": "ours", "providerText": "upstream\u{0} overloaded\r\n"})),
        "upstream overloaded"
    );
    assert_eq!(said(json!({"kind": "malformed", "providerText": 7})), "");
    assert!(said(json!({"kind": "malformed", "providerText": "x".repeat(5_000)})).chars().count() <= 300);
}

#[test]
fn a_successful_test_lists_the_servers_models_as_data() {
    let ok = st::test_result(
        Ok(
            json!({"models": [{"id": "qwen3", "contextWindow": 8192}, "llama3", {"id": "qwen3"}, {"id": "a\u{7}b"}, {"id": ""}, 7], "contextWindow": 1}),
        ),
        false,
    );
    let TestState::Ok { models, context_window } = &ok else { panic!("{ok:?}") };
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["qwen3", "llama3", "ab"]);
    assert_eq!(models[0].context_window, Some(8_192));
    assert_eq!(*context_window, Some(256));
    let many: Vec<Value> = (0..1_000).map(|i| json!({"id": format!("m{i}")})).collect();
    assert_eq!(st::test_result(Ok(json!({"models": many})), false).models().len(), st::MAX_MODELS);
    let (icon, _, words) = st::test_line(&ok, "h", "a").expect("a line");
    assert_eq!(icon, "tick");
    assert!(words.contains('3'));
    let (icon, _, _) = st::test_line(&st::test_result(Ok(json!({})), false), "h", "a").expect("a line");
    assert_eq!(icon, "warning", "connected, but no models");
    assert!(st::test_line(&TestState::Idle, "h", "a").is_none());
}

// ---- State mapping -------------------------------------------------------------------------

fn input(primary: Primary) -> StatusInput {
    StatusInput { has_host: true, primary, backup_confirmed: false, refused: None, asking: None, changed: None }
}

fn ids(s: &StatusInput) -> Vec<StatusId> {
    st::status_lines(s).iter().map(|l| l.id).collect()
}

#[test]
fn the_status_lines_follow_the_brief() {
    assert_eq!(ids(&StatusInput { has_host: false, ..input(Primary::Never) }), [StatusId::NoHost]);
    assert_eq!(ids(&input(Primary::Never)), [StatusId::Never]);
    assert_eq!(ids(&StatusInput { backup_confirmed: true, ..input(Primary::Never) }), [StatusId::NeverBackup]);
    assert_eq!(ids(&StatusInput { backup_confirmed: true, ..input(Primary::Off) }), [StatusId::Off]);
    assert_eq!(ids(&input(Primary::Local { model: "m".into() })), [StatusId::Local, StatusId::BackupNone]);
    let gateway = st::status_lines(&StatusInput { backup_confirmed: true, ..input(Primary::Gateway { model: String::new() }) });
    assert_eq!(gateway.iter().map(|l| l.id).collect::<Vec<_>>(), [StatusId::Gateway, StatusId::BackupConfirmed]);
    assert!(gateway[0].words.contains("through a gateway on this computer that sends your mail to the internet"));
    assert!(gateway[0].words.contains("a model"), "a missing name reads as a model");
    assert_eq!(ids(&input(Primary::Remote { model: "m".into(), host: "h".into() })), [StatusId::Remote, StatusId::BackupNone]);
    let refused =
        StatusInput { refused: Some(("http://127.0.0.1:4000".into(), Some("gpt-x".into()))), ..input(Primary::Local { model: "m".into() }) };
    let lines = st::status_lines(&refused);
    assert_eq!(lines[0].id, StatusId::Refused);
    assert_eq!(lines[0].icon, "highImportance");
    assert!(lines[0].words.contains("gpt-x"));
    let asking = StatusInput { asking: Some("http://127.0.0.1:4000".into()), ..input(Primary::Local { model: "m".into() }) };
    assert_eq!(ids(&asking)[0], StatusId::Asking);
    let changed = StatusInput { changed: Some("http://127.0.0.1:11434".into()), ..input(Primary::Never) };
    assert_eq!(ids(&changed), [StatusId::Never, StatusId::Changed]);
    // Words are never hue-coloured here: state is the icon's.
    for line in st::status_lines(&refused).iter().chain(st::status_lines(&asking).iter()) {
        assert!(!line.words.is_empty());
    }
    // Each line has a `ui.inspect` hit kind of its own, beside its place (`aiStatus`).
    let all = [
        StatusId::NoHost,
        StatusId::Never,
        StatusId::NeverBackup,
        StatusId::Off,
        StatusId::Local,
        StatusId::Gateway,
        StatusId::Remote,
        StatusId::Refused,
        StatusId::Asking,
        StatusId::Changed,
        StatusId::BackupConfirmed,
        StatusId::BackupNone,
    ];
    let kinds: std::collections::BTreeSet<&str> = all.iter().map(|id| id.hit()).collect();
    assert_eq!(kinds.len(), all.len());
    assert!(kinds.iter().all(|k| k.starts_with("aiStatus") && *k != "aiStatus"));
}

#[test]
fn the_on_device_card_takes_every_state() {
    let device = |state: DeviceState, n: usize| Device {
        state,
        candidates: (0..n)
            .map(|i| st::Candidate { address: format!("http://127.0.0.1:{}", 11_434 + i), model: String::new(), context_window: Some(4_096) })
            .collect(),
        confirmed: Some("http://127.0.0.1:11434".into()),
    };
    assert_eq!(st::card_id(true, false, &device(DeviceState::Unavailable, 0)), CardId::Unavailable);
    assert_eq!(st::card_id(false, false, &device(DeviceState::Found, 1)), CardId::Off);
    assert_eq!(st::card_id(true, true, &device(DeviceState::NotFound, 0)), CardId::Looking);
    assert_eq!(st::card_id(true, false, &device(DeviceState::Found, 2)), CardId::Found);
    assert_eq!(st::card_id(true, false, &device(DeviceState::Found, 0)), CardId::NotFound);
    assert_eq!(st::card_id(true, false, &device(DeviceState::Using, 1)), CardId::Using);
    assert_eq!(st::card_id(true, false, &device(DeviceState::Changed, 1)), CardId::Changed);
    assert_eq!(st::card_id(true, false, &device(DeviceState::NotFound, 0)), CardId::NotFound);
    for id in [CardId::Looking, CardId::Found, CardId::Using, CardId::Changed, CardId::NotFound, CardId::Off, CardId::Unavailable] {
        let (_, _, title, _, _) = st::card_text(id, &device(DeviceState::Using, 1), &[]);
        assert!(!title.is_empty(), "{id:?}");
    }
    let (_, _, _, detail, _) = st::card_text(CardId::Using, &device(DeviceState::Using, 1), &[]);
    let (_, _, _, none, _) = st::card_text(CardId::NotFound, &device(DeviceState::NotFound, 0), &[11_437, 11_434]);
    assert!(none.contains("11434, 11437"), "{none}");
    assert_eq!(st::ports_of(Some(&json!([11437, 0, 70000, "x", 11437, 8080]))), [11_437, 8_080]);
    assert_eq!(detail, "apfel at http://127.0.0.1:11434 · 4,096 tokens");
    // The model the host named is in the line, as data.
    let named = st::Candidate { address: "http://127.0.0.1:11434".into(), model: "apple-foundationmodel".into(), context_window: Some(4_096) };
    assert_eq!(st::candidate_line(&named), "apfel at http://127.0.0.1:11434 · apple-foundationmodel · 4,096 tokens");
    let no_window = st::Candidate { context_window: None, ..named };
    assert_eq!(st::candidate_line(&no_window), "apfel at http://127.0.0.1:11434 · apple-foundationmodel");
}

#[test]
fn the_window_asked_with_is_the_answering_providers() {
    let read = |v: Value| Providers::read(&v);
    let p = read(json!({"active": "a", "providers": [{"id": "a", "kind": "local", "contextWindow": 8192, "context_window": 2048}]}));
    assert_eq!(p.answering_window(), Some(8_192), "the camelCase window wins");
    let p = read(json!({"active": "a", "providers": [{"id": "a", "kind": "local", "context_window": 100}]}));
    assert_eq!(p.answering_window(), None, "out of `ai.ask`'s range: left to the engine's default");
    let mut p = read(json!({"active": "", "backup": "apple-on-device", "neverConfigured": true, "providers": []}));
    p.device = Device::read(
        &json!({"state": "using", "candidates": [{"address": "http://127.0.0.1:11434", "contextWindow": 4096}], "confirmed": "http://127.0.0.1:11434"}),
    );
    assert_eq!(p.answering_window(), Some(4_096), "the confirmed backup's, with no primary");
    p.never_configured = false;
    assert_eq!(p.answering_window(), None, "a primary of None turns the backup off");
    // An old host that names neither field: never set up only without any provider.
    assert!(read(json!({"active": "", "providers": []})).never_configured);
    assert!(read(json!({"active": "", "providers": [{"id": "a", "kind": "local"}]})).explicit_none());
}

// ---- The window commands -------------------------------------------------------------------

#[test]
fn the_new_window_commands_are_registered_and_never_an_agents() {
    for id in ["ui.testProvider", "ui.confirmOnDevice", "ui.aiSettings"] {
        assert!(crate::UI_COMMANDS.iter().any(|c| c.0 == id), "{id} is listed");
        assert_eq!(mailcraft_engine::agent::access(id), Some(mailcraft_engine::agent::Access::Never), "{id}");
    }
    let doc = crate::UI_COMMANDS.iter().find(|c| c.0 == "ui.aiSettings").map(|c| c.3).unwrap_or_default();
    assert!(doc.contains("gateway") && doc.contains("contextWindow") && doc.contains("aiModel"));
}

/// What the fake host keeps.
#[derive(Default)]
struct Kept {
    providers: Vec<Value>,
    active: String,
    backup: String,
    never: bool,
    device: Value,
    question: Option<Value>,
    dismissed: usize,
    answered: Vec<(String, bool)>,
    tested: Vec<Value>,
    confirmed: Vec<String>,
    looked: usize,
    ports: Vec<String>,
}

/// A host in memory, its state shared with the test.
#[derive(Clone, Default)]
struct Host(Rc<RefCell<Kept>>);

fn sent<T: Send + 'static>(value: T) -> Receiver<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = tx.send(value);
    rx
}

impl AiProviders for Host {
    fn providers(&self) -> Value {
        let k = self.0.borrow();
        let mut v = json!({"active": k.active, "providers": k.providers, "backup": k.backup, "neverConfigured": k.never});
        if let (Some(q), Some(map)) = (&k.question, v.as_object_mut()) {
            map.insert("gatewayQuestion".into(), q.clone());
        }
        v
    }
    fn set_provider(&self, provider: &Value) -> Result<(), String> {
        let id = provider.get("id").and_then(Value::as_str).ok_or("no id")?.to_string();
        let mut k = self.0.borrow_mut();
        k.providers.retain(|p| p.get("id").and_then(Value::as_str) != Some(id.as_str()));
        k.providers.push(provider.clone());
        k.never = false;
        Ok(())
    }
    fn remove_provider(&self, _id: &str) -> Result<(), String> {
        Ok(())
    }
    fn set_active(&self, id: &str) -> Result<(), String> {
        let mut k = self.0.borrow_mut();
        k.active = id.to_string();
        k.never = false;
        Ok(())
    }
    fn set_consent(&self, _id: &str, _account: &str, _allowed: bool) -> Result<(), String> {
        Ok(())
    }
    fn save_key(&self, _id: &str, _key: String) -> Receiver<Result<(), String>> {
        sent(Ok(()))
    }
    fn forget_key(&self, _id: &str) -> Receiver<Result<(), String>> {
        sent(Ok(()))
    }
    fn has_key(&self, _id: &str) -> Receiver<Result<bool, String>> {
        sent(Ok(false))
    }
    fn gateway_question(&self) -> Option<Value> {
        self.0.borrow().question.clone()
    }
    fn answer_gateway(&self, provider: &str, models: &str, sends: bool) -> Result<(), String> {
        let mut k = self.0.borrow_mut();
        // As the host does: an answer about another provider or another list is refused.
        let asked = k
            .question
            .as_ref()
            .is_some_and(|q| q.get("provider").and_then(Value::as_str) == Some(provider) && q.get("models").and_then(Value::as_str) == Some(models));
        if !asked {
            return Err("not the question asked".into());
        }
        k.answered.push((provider.to_string(), sends));
        k.question = None;
        if sends && let Some(p) = k.providers.iter_mut().find(|p| p.get("id").and_then(Value::as_str) == Some(provider)) {
            p["kind"] = json!("gateway");
        }
        Ok(())
    }
    fn dismiss_gateway_question(&self) {
        let mut k = self.0.borrow_mut();
        k.question = None;
        k.dismissed += 1;
    }
    fn set_backup(&self, backup: &str) -> Result<(), String> {
        self.0.borrow_mut().backup = backup.to_string();
        Ok(())
    }
    fn test_provider(&self, provider: &Value) -> Receiver<Result<Value, String>> {
        self.0.borrow_mut().tested.push(provider.clone());
        let closed = provider.get("base_url").and_then(Value::as_str).is_some_and(|a| a.ends_with(":1"));
        if closed {
            sent(Err(json!({"kind": "unreachable", "message": "connection refused"}).to_string()))
        } else {
            sent(Ok(json!({"models": [{"id": "fake-a", "contextWindow": 8192}, {"id": "fake-b"}]})))
        }
    }
    fn on_device(&self) -> Value {
        self.0.borrow().device.clone()
    }
    fn discover_on_device(&self) -> Receiver<Result<Value, String>> {
        let mut k = self.0.borrow_mut();
        k.looked += 1;
        sent(Ok(k.device.clone()))
    }
    fn confirm_on_device(&self, address: &str) -> Receiver<Result<(), String>> {
        let mut k = self.0.borrow_mut();
        k.confirmed.push(address.to_string());
        k.device["state"] = json!("using");
        k.device["confirmed"] = json!(address);
        sent(Ok(()))
    }
    fn set_device_ports(&self, typed: &str) -> Result<(), String> {
        if typed.split(',').map(str::trim).filter(|p| !p.is_empty()).all(|p| p.parse::<u16>().is_ok_and(|n| n > 0)) {
            self.0.borrow_mut().ports.push(typed.to_string());
            Ok(())
        } else {
            Err("99999 is not a port the host can look on".into())
        }
    }
    fn stop_using_on_device(&self) -> Result<(), String> {
        let mut k = self.0.borrow_mut();
        k.device["state"] = json!("found");
        k.device["confirmed"] = Value::Null;
        Ok(())
    }
}

fn no_model(_job: crate::ModelJob) -> Receiver<mailcraft_model::ai::ModelEvent> {
    std::sync::mpsc::channel().1
}

fn app_with(host: &Host) -> MailApp {
    let services = Services { ai_providers: Some(Box::new(host.clone())), model_request: Some(Box::new(no_model)), ..Default::default() };
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), services);
    app.autosave = false;
    app
}

fn window_with(host: &Host) -> Window {
    let mut w = testing::window("view.mail");
    let _ = w.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    w.state_mut().services.ai_providers = Some(Box::new(host.clone()));
    w.state_mut().services.model_request = Some(Box::new(no_model));
    w.run_steps(2);
    w
}

fn found_device() -> Value {
    json!({"state": "found", "candidates": [{"address": "http://127.0.0.1:11437", "model": "apple-foundationmodel", "contextWindow": 4096}], "confirmed": null})
}

#[test]
fn test_connection_fills_the_model_picker_and_a_closed_port_says_unreachable() {
    let host = Host::default();
    host.0.borrow_mut().never = true;
    let mut w = window_with(&host);
    w.state_mut().execute("ui.aiSettings", json!({"kind": "local", "address": "http://127.0.0.1:11435/v1"})).unwrap();
    w.run_steps(3);
    for kind in ["aiStatus", "aiPrimaryKind", "aiModel", "aiKindNote", "aiAddress", "aiTest", "aiModelName", "aiContextWindow", "aiWindowNote"] {
        assert!(w.state().hit.rect(kind, 0).is_some(), "{kind} is drawn");
    }
    assert_eq!(w.state_mut().execute("ui.aiSettings", json!({})).unwrap()["contextWindow"], 4_096);
    w.state_mut().execute("ui.testProvider", json!({})).unwrap();
    w.run_steps(3);
    let shown = w.state_mut().execute("ui.aiSettings", json!({})).unwrap();
    assert_eq!(shown["test"]["state"], "ok");
    assert_eq!(shown["models"], json!(["fake-a", "fake-b"]));
    w.run_steps(2);
    assert!(w.state().hit.rect("aiModelPicker", 0).is_some());
    assert!(w.state().hit.rect("aiTestResult", 0).is_some());
    // Negative control: a closed port.
    w.state_mut().execute("ui.aiSettings", json!({"address": "http://127.0.0.1:1"})).unwrap();
    w.state_mut().execute("ui.testProvider", json!({})).unwrap();
    w.run_steps(3);
    let shown = w.state_mut().execute("ui.aiSettings", json!({})).unwrap();
    assert_eq!((shown["test"]["state"].as_str(), shown["test"]["kind"].as_str()), (Some("failed"), Some("unreachable")));
    assert!(host.0.borrow().tested.iter().all(|p| p.get("key").is_none() && p.get("api_key").is_none()), "no key in what is tested");
}

#[test]
fn a_window_out_of_range_is_clamped_and_said_and_junk_is_not_saved() {
    let host = Host::default();
    let mut app = app_with(&host);
    app.execute("ui.aiSettings", json!({"kind": "local", "address": "http://127.0.0.1:11434/v1"})).unwrap();
    let clamped = app.execute("ui.aiSettings", json!({"contextWindow": "9"})).unwrap();
    assert_eq!((clamped["contextWindow"].as_u64(), clamped["contextWindowSource"].as_str()), (Some(256), Some("owner")));
    assert_eq!(app.ai.settings.window_clamped, Some(256));
    let saved = |host: &Host| host.0.borrow().providers.first().and_then(|p| p.get("context_window").and_then(Value::as_u64));
    assert_eq!(saved(&host), Some(256));
    app.execute("ui.aiSettings", json!({"contextWindow": "lots"})).unwrap();
    assert!(app.ai.settings.window_bad);
    assert_eq!(saved(&host), Some(256), "not saved");
    assert!(app.execute("ui.aiSettings", json!({"contextWindow": [1]})).is_err());
}

#[test]
fn only_a_confirmed_on_device_address_is_used_and_none_is_looked_for_while_off() {
    let host = Host::default();
    {
        let mut k = host.0.borrow_mut();
        (k.never, k.device) = (true, found_device());
    }
    let mut w = window_with(&host);
    w.state_mut().execute("ui.aiSettings", json!({})).unwrap();
    w.run_steps(3);
    assert_eq!(host.0.borrow().looked, 0, "the backup is None: nothing is looked for");
    assert!(w.state().hit.rect("aiDeviceCard", 0).is_some());
    assert_eq!(w.state_mut().execute("ui.aiSettings", json!({})).unwrap()["device"]["state"], "off");
    w.state_mut().execute("ui.aiSettings", json!({"backup": "onDevice"})).unwrap();
    w.run_steps(3);
    assert!(host.0.borrow().looked >= 1);
    assert!(w.state().hit.rect("aiDeviceUse", 0).is_some(), "Use It beside the candidate");
    assert!(!super::model_ready(w.state()), "found is not confirmed: nothing is used");
    // Only an address the card lists.
    assert!(w.state_mut().execute("ui.confirmOnDevice", json!({"address": "http://127.0.0.1:9999"})).is_err());
    assert!(w.state_mut().execute("ui.confirmOnDevice", json!({})).is_err());
    w.state_mut().execute("ui.confirmOnDevice", json!({"address": "http://127.0.0.1:11437"})).unwrap();
    w.run_steps(3);
    assert_eq!(host.0.borrow().confirmed, ["http://127.0.0.1:11437"]);
    assert!(w.state().hit.rect("aiDeviceStop", 0).is_some());
    assert!(super::model_ready(w.state()));
    // Stop Using It: the confirmation goes, the backup stays the on-device model.
    w.state_mut().execute("ui.aiSettings", json!({"stopUsing": true})).unwrap();
    assert!(!super::model_ready(w.state()));
    assert_eq!(host.0.borrow().backup, "apple-on-device");
    // Look on ports: a bad list is said in our words, never the host's, and a good one is looked
    // on at once.
    let looked = host.0.borrow().looked;
    w.state_mut().execute("ui.aiSettings", json!({"ports": "11437, 99999"})).unwrap();
    assert_eq!(w.state().ai.settings.ports_problem.as_deref(), Some("Write port numbers from 1 to 65535, separated by commas."));
    w.state_mut().execute("ui.aiSettings", json!({"ports": "11437, 8080"})).unwrap();
    assert_eq!(host.0.borrow().ports, ["11437, 8080"]);
    assert!(host.0.borrow().looked > looked);
    w.run_steps(2);
    assert!(w.state().hit.rect("aiPorts", 0).is_some());
    // A primary of None turns the backup off.
    w.state_mut().execute("ui.aiSettings", json!({"kind": "none"})).unwrap();
    assert_eq!(host.0.borrow().backup, "");
    assert_eq!(w.state_mut().execute("ui.aiSettings", json!({})).unwrap()["primaryKind"], "none");
}

#[test]
fn the_gateway_question_closed_unanswered_sends_nothing_and_an_answer_is_kept() {
    let host = Host::default();
    host.set_provider(&json!({"id": "local", "kind": "local", "base_url": "http://127.0.0.1:4000/v1", "model": "m"})).unwrap();
    host.set_active("local").unwrap();
    let ask = json!({"provider": "local", "address": "http://127.0.0.1:4000/v1", "models": "d", "publishers": ["openai", "qwen"]});
    host.0.borrow_mut().question = Some(ask.clone());
    let mut app = app_with(&host);
    super::follow(&mut app);
    assert!(super::settings_ask::gateway_dialog_open(&app), "put to the owner");
    assert_eq!(app.execute("ui.aiSettings", json!({})).map(|_| ()), Ok(()));
    // `ui.aiSettings` closed it unanswered: Not Now.
    super::follow(&mut app);
    assert_eq!(host.0.borrow().dismissed, 1);
    assert!(host.0.borrow().answered.is_empty());
    assert!(app.ai.settings.gateway_waiting.is_some(), "waiting for an answer");
    // Asked again, answered: Yes, It Does.
    host.0.borrow_mut().question = Some(ask);
    super::follow(&mut app);
    assert!(super::settings_ask::gateway_dialog_open(&app), "asked again");
    let answered =
        app.execute("ui.aiSettings", json!({"gatewayProvider": "local", "gatewayAddress": "http://127.0.0.1:4000", "gatewayAnswer": true})).unwrap();
    super::follow(&mut app);
    assert_eq!(host.0.borrow().dismissed, 1, "an answer is no dismissal");
    assert_eq!(host.0.borrow().answered, [("local".to_string(), true)]);
    assert_eq!(answered["kind"], "gateway");
    assert!(app.ai.settings.gateway_waiting.is_none());
    assert!(app.execute("ui.aiSettings", json!({"gatewayAnswer": true})).is_err(), "which provider is named");
}

#[test]
fn choosing_a_gateway_asks_first_and_keeps_what_was_typed() {
    let host = Host::default();
    let mut app = app_with(&host);
    app.execute("ui.aiSettings", json!({"kind": "local", "address": "http://127.0.0.1:4000/v1", "model": "m"})).unwrap();
    let asked = app.execute("ui.aiSettings", json!({"kind": "gateway"})).unwrap();
    assert_eq!(asked["asking"], "gateway");
    assert_eq!(asked["kind"], "local", "nothing changes before the yes");
    app.dialog = None;
    let chosen = app.execute("ui.aiSettings", json!({"kind": "gateway", "confirmed": true})).unwrap();
    assert_eq!(
        (chosen["kind"].as_str(), chosen["address"].as_str(), chosen["model"].as_str()),
        (Some("gateway"), Some("http://127.0.0.1:4000/v1"), Some("m"))
    );
    assert_eq!(chosen["contextWindow"], 32_768, "the gateway's default");
    assert_eq!(host.0.borrow().providers.first().and_then(|p| p.get("kind").cloned()), Some(json!("gateway")));
}

#[test]
fn hostile_params_are_refused_without_a_panic() {
    let host = Host::default();
    let mut app = app_with(&host);
    for p in [
        json!({"kind": 5}),
        json!({"backup": "everything"}),
        json!({"backup": 1}),
        json!({"contextWindow": {"n": 1}}),
        json!({"lookAgain": "yes"}),
        json!({"gatewayAnswer": "yes"}),
    ] {
        assert!(app.execute("ui.aiSettings", p.clone()).is_err(), "{p}");
    }
    for p in [json!({"provider": 5}), json!({"provider": "nobody"}), json!([])] {
        assert!(app.execute("ui.testProvider", p.clone()).is_err(), "{p}");
    }
    assert!(app.execute("ui.confirmOnDevice", json!({"address": "x".repeat(5_000)})).is_err());
}

// ---- The answer's sources (6.6) ------------------------------------------------------------

#[test]
fn a_fallback_is_said_with_the_primarys_name_and_the_model_that_answered() {
    let mut w = testing::window("view.mail");
    w.run_steps(1);
    answers::set_open(w.state_mut(), true);
    let pane = w.state_mut().ai.answers.as_mut().expect("open");
    pane.primary = Some(("remote".into(), "big-model".into()));
    pane.answered_by = Some(("remote".into(), "big-model".into()));
    assert_eq!(answers::fallback_words(pane), None, "the primary answered");
    pane.answered_by = Some(("apple-on-device".into(), "apple-foundationmodel".into()));
    assert_eq!(answers::fallback_words(pane).as_deref(), Some("big-model didn't answer; this answer comes from apple-foundationmodel"));
    pane.primary = None;
    assert_eq!(answers::fallback_words(pane), None);
}

// ---- Narrowing and the gateway question (M1, SCN-BASE-003) ---------------------------------

/// A model request the host keeps under way: its sender is never dropped, so nothing ends it but
/// the window.
fn model_under_way(_job: crate::ModelJob) -> Receiver<mailcraft_model::ai::ModelEvent> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::mem::forget(tx);
    rx
}

/// A window with a local provider set up, whose narrowing request is under way.
fn narrowing(host: &Host) -> (Window, u64) {
    host.set_provider(&json!({"id": "local", "kind": "local", "base_url": "http://127.0.0.1:4000/v1", "model": "m"})).unwrap();
    host.set_active("local").unwrap();
    let mut w = window_with(host);
    w.state_mut().services.model_request = Some(Box::new(model_under_way));
    let asked = w.state_mut().execute("ui.findByDescription", json!({"text": "invoices from Ada", "suggest": true})).unwrap();
    assert_eq!(asked["stage"], "asking");
    let request = asked["request"].as_u64().expect("a request");
    w.run_steps(2);
    (w, request)
}

fn live(w: &Window, request: u64) -> bool {
    w.state().session.ai.get(request).is_some_and(|t| t.state.live())
}

#[test]
fn base_003_narrowing_survives_the_gateway_question() {
    let host = Host::default();
    let (mut w, request) = narrowing(&host);
    // The route check of that very request puts the question.
    host.0.borrow_mut().question =
        Some(json!({"provider": "local", "address": "http://127.0.0.1:4000/v1", "models": "d", "publishers": ["openai", "qwen"]}));
    w.run_steps(3);
    assert!(super::settings_ask::gateway_dialog_open(w.state()), "put to the owner");
    assert!(w.state().ai.narrow.is_some(), "the popover is still open");
    assert!(matches!(w.state().ai.narrow.as_ref().map(|p| p.stage), Some(super::narrow::Stage::Asking { request: r }) if r == request));
    assert!(live(&w, request), "the narrowing request is still live");
    // Answered: the question goes, and the request is still the one under way.
    w.state_mut()
        .execute("ui.aiSettings", json!({"gatewayProvider": "local", "gatewayAddress": "http://127.0.0.1:4000", "gatewayAnswer": false}))
        .unwrap();
    w.run_steps(3);
    assert!(!super::settings_ask::gateway_dialog_open(w.state()));
    assert!(w.state().ai.narrow.is_some() && live(&w, request));
}

#[test]
fn neg_m1_escape_still_cancels_narrowing() {
    let host = Host::default();
    let (mut w, request) = narrowing(&host);
    assert!(live(&w, request));
    testing::key(&mut w, egui::Key::Escape);
    assert!(!live(&w, request), "called off");
    assert!(matches!(w.state().ai.narrow.as_ref().map(|p| p.stage), Some(super::narrow::Stage::Ready)));
}

/// SCN-PRIOR-OPTIN-001 (B5): after the runtimes section grew, the Key field sat below the edge of
/// a 900-pixel window, so Add Key looked like it did nothing and a key typed went nowhere. Add Key
/// now brings the field into view with the keyboard in it.
#[test]
fn add_key_brings_the_key_field_into_view_with_the_keyboard() {
    let host = Host::default();
    host.set_provider(&json!({"id": "office", "kind": "remote", "base_url": "https://models.example/v1", "model": "m"})).unwrap();
    let mut w = window_with(&host);
    w.set_size(egui::vec2(1440.0, 900.0));
    w.run_steps(2);
    let provider = mailcraft_model::ai::ProviderId::new("office").unwrap();
    super::settings_runtimes::open_key_field(w.state_mut(), &provider).unwrap();
    w.run_steps(4);
    assert!(!w.state().ai.settings.focus_key, "the request is taken once");
    let key = w.state().hit.rect("aiKey", 0).expect("the Key field is drawn");
    assert!(key.min.y >= 0.0 && key.max.y <= 900.0, "the Key field is in the window: {key:?}");
    assert!(w.ctx.memory(|m| m.focused()).is_some(), "the Key field has the keyboard");
}

/// A negative control: opening Options without Add Key takes no keyboard and asks no scroll.
#[test]
fn neg_options_alone_doesnt_take_the_keyboard_for_the_key() {
    let host = Host::default();
    let mut w = window_with(&host);
    w.state_mut().execute("ui.aiSettings", json!({})).unwrap();
    w.run_steps(3);
    assert!(!w.state().ai.settings.focus_key);
    assert!(w.ctx.memory(|m| m.focused()).is_none());
}
