// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The owner's model-provider control methods: they reach the host, never take a key, are
//! refused for an agent, and a hostile param is an error.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, channel};

use mailcraft_engine::Session;
use serde_json::{Value, json};

use super::{ControlRequest, handle};
use crate::{AiProviders, MailApp, Services};

const NOW: i64 = 1_791_993_600;

/// A host that only remembers what it was told.
struct Fake(Rc<RefCell<Vec<String>>>);

impl AiProviders for Fake {
    fn providers(&self) -> Value {
        json!({"active": "", "providers": []})
    }
    fn set_provider(&self, p: &Value) -> Result<(), String> {
        self.0.borrow_mut().push(format!("set {p}"));
        Ok(())
    }
    fn remove_provider(&self, id: &str) -> Result<(), String> {
        self.0.borrow_mut().push(format!("remove {id}"));
        Ok(())
    }
    fn set_active(&self, id: &str) -> Result<(), String> {
        self.0.borrow_mut().push(format!("active {id}"));
        Ok(())
    }
    fn set_consent(&self, id: &str, account: &str, allowed: bool) -> Result<(), String> {
        self.0.borrow_mut().push(format!("consent {id} {account} {allowed}"));
        Ok(())
    }
    fn save_key(&self, _: &str, _: String) -> Receiver<Result<(), String>> {
        channel().1
    }
    fn forget_key(&self, _: &str) -> Receiver<Result<(), String>> {
        channel().1
    }
    fn has_key(&self, _: &str) -> Receiver<Result<bool, String>> {
        channel().1
    }
}

fn app(calls: &Rc<RefCell<Vec<String>>>) -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app.services.ai_providers = Some(Box::new(Fake(calls.clone())));
    app
}

fn call(app: &mut MailApp, method: &str, params: Value, agent: bool) -> Value {
    let mut req = ControlRequest::new(method, params).0;
    if agent {
        req.agent = Some(Vec::new());
    }
    match handle(app, &egui::Context::default(), &req) {
        super::Outcome::Done(v) => v,
        _ => json!({"ok": false, "error": "not an answer"}),
    }
}

#[test]
fn the_owner_reaches_the_host() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let mut app = app(&calls);
    assert_eq!(call(&mut app, "ai.providers", json!({}), false)["ok"], true);
    for (m, p) in [
        ("ai.setProvider", json!({"provider": {"id": "p", "kind": "local", "consented_accounts": ["a@example.org"]}})),
        ("ai.setActive", json!({"id": "p"})),
        ("ai.setActive", json!({"id": ""})),
        ("ai.setConsent", json!({"id": "p", "account": "a@example.org", "allowed": false})),
        ("ai.removeProvider", json!({"id": "p"})),
    ] {
        assert_eq!(call(&mut app, m, p, false)["ok"], true, "{m}");
    }
    let calls = calls.borrow();
    assert_eq!(calls.len(), 5);
    assert!(!calls[0].contains("consented_accounts"), "consents come only from setConsent");
    assert_eq!(calls[3], "consent p a@example.org false");
}

#[test]
fn an_agent_is_refused_and_the_host_is_not_called() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let mut app = app(&calls);
    for m in ["ai.providers", "ai.setProvider", "ai.setActive", "ai.setConsent", "ai.removeProvider"] {
        let a = call(&mut app, m, json!({"id": "p", "account": "a", "allowed": true, "provider": {"id": "p"}}), true);
        assert_eq!(a["ok"], false, "{m}");
    }
    assert!(calls.borrow().is_empty());
}

#[test]
fn a_key_is_never_taken() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let mut app = app(&calls);
    for k in ["key", "apiKey", "API_KEY", "secret", "token"] {
        let a = call(&mut app, "ai.setProvider", json!({"provider": {"id": "p", k: "sk-not-a-real-key"}}), false);
        assert_eq!(a["ok"], false, "{k}");
        assert!(!a.to_string().contains("sk-not"), "the key is never echoed");
    }
    assert!(calls.borrow().is_empty());
}

#[test]
fn hostile_params_are_errors_not_panics() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let mut app = app(&calls);
    let long = "x".repeat(100_000);
    let junk = [
        json!(null),
        json!(7),
        json!("s"),
        json!([1]),
        json!({}),
        json!({"id": 5, "account": [], "allowed": "yes", "provider": 3}),
        json!({"id": long, "account": long, "allowed": true, "provider": {"id": long}}),
        json!({"id": "a\u{0}b", "account": "x", "allowed": true}),
        json!({"provider": {"id": ""}}),
        json!({"provider": {"id": "p", "pad": long}}),
    ];
    for m in ["ai.setProvider", "ai.setActive", "ai.setConsent", "ai.removeProvider"] {
        for p in &junk {
            assert_eq!(call(&mut app, m, p.clone(), false)["ok"], false, "{m} {p:.40}");
        }
    }
    assert!(calls.borrow().is_empty());
}

#[test]
fn without_a_host_it_is_an_error() {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    assert_eq!(call(&mut app, "ai.providers", json!({}), false)["ok"], false);
}

// ---- The runtimes (ai-runtime-commands 12.3–12.7). Synthetic accounts only (`example.org`).

use std::cell::Cell;

use mailcraft_model::ai::{Chain, ChainEntry, Consents, Fallback, RuntimeId, RuntimePath, UarInstance, Where};

use super::{AiRuntimes, PROVIDER_METHODS, RUNTIME_METHODS, RuntimeSettings, RuntimeState, RuntimeStatus, SignIn, SignOut, Step, run_runtime};
use crate::agent_gate::activity::Decided;

const ACCOUNT: &str = "owner@example.org";

fn rt(id: &str) -> RuntimeId {
    RuntimeId::parse(id).unwrap()
}

/// A chain of `ids`, the first on.
fn chain(ids: &[&str]) -> Chain {
    Chain::new(ids.iter().enumerate().map(|(n, id)| ChainEntry::new(rt(id)).with_enabled(n == 0))).unwrap()
}

fn settings() -> RuntimeSettings {
    RuntimeSettings {
        chain: chain(&["on_device", "compat:office", "chatgpt", "claude_code", "codex", "uar"]),
        consents: Consents::new().with(ACCOUNT, &rt("compat:office"), true).unwrap(),
        fallback: Fallback::Next,
    }
}

/// A host that keeps the settings in memory and notes every call that would do something.
struct Host {
    kept: RefCell<RuntimeSettings>,
    saves: Cell<usize>,
    calls: RefCell<Vec<String>>,
    /// Whether the host can sign in, sign out and look again (else the defaults: not yet).
    able: bool,
}

impl Host {
    fn new(able: bool) -> Host {
        Host { kept: RefCell::new(settings()), saves: Cell::new(0), calls: RefCell::new(Vec::new()), able }
    }
}

impl AiRuntimes for Host {
    fn settings(&self) -> RuntimeSettings {
        self.kept.borrow().clone()
    }
    fn save(&self, s: &RuntimeSettings) -> Result<(), String> {
        self.saves.set(self.saves.get() + 1);
        *self.kept.borrow_mut() = s.clone();
        Ok(())
    }
    fn status(&self, runtime: &RuntimeId) -> RuntimeStatus {
        match runtime {
            RuntimeId::ClaudeCode => RuntimeStatus {
                state: RuntimeState::Ready,
                version: Some("2.1.4".into()),
                path: Some(RuntimePath::new("/opt/example/bin/claude").unwrap()),
            },
            // What a program printed is never shown.
            RuntimeId::Codex => RuntimeStatus {
                state: RuntimeState::SignedOut,
                version: Some("logged in as someone@example.org token=sk-not-a-real-token".into()),
                path: None,
            },
            _ => RuntimeStatus::default(),
        }
    }
    fn where_of(&self, runtime: &RuntimeId) -> Where {
        match runtime {
            RuntimeId::Compat(_) => Where::Leaves,
            RuntimeId::Uar => Where::Local,
            other => other.kind().fixed_where().unwrap_or(Where::Leaves),
        }
    }
    fn rescan(&self) -> Result<(), String> {
        self.calls.borrow_mut().push("rescan".into());
        if self.able { Ok(()) } else { Err("not yet".into()) }
    }
    fn sign_in(&self, how: SignIn) -> Result<(), String> {
        self.calls.borrow_mut().push(format!("signIn {how:?}"));
        if self.able { Ok(()) } else { Err("not yet".into()) }
    }
    fn sign_out(&self, what: &SignOut) -> Result<(), String> {
        self.calls.borrow_mut().push(format!("signOut {what:?}"));
        if self.able { Ok(()) } else { Err("not yet".into()) }
    }
}

/// A host that carries only what every host must: the rest is "not available yet".
struct Bare(RefCell<RuntimeSettings>);

impl AiRuntimes for Bare {
    fn settings(&self) -> RuntimeSettings {
        self.0.borrow().clone()
    }
    fn save(&self, s: &RuntimeSettings) -> Result<(), String> {
        *self.0.borrow_mut() = s.clone();
        Ok(())
    }
}

fn done(r: Result<Step, String>) -> Value {
    match r {
        Ok(Step::Done(v)) => v,
        other => panic!("not an answer: {other:?}"),
    }
}

fn enabled(h: &Host, id: &str) -> bool {
    h.kept.borrow().chain.get(&rt(id)).unwrap().enabled()
}

#[test]
fn an_owner_script_with_well_formed_params_gets_the_change_made() {
    let h = Host::new(true);
    // Codex on; no account's consent changes.
    let before = h.kept.borrow().consents.clone();
    done(run_runtime(&h, "ai.setRuntimeEnabled", &json!({"runtime": "codex", "value": true})));
    assert!(enabled(&h, "codex"));
    assert_eq!(h.kept.borrow().consents, before);
    // Turning ChatGPT on lists it in no account's consent.
    done(run_runtime(&h, "ai.setRuntimeEnabled", &json!({"runtime": "chatgpt", "value": true})));
    assert!(!h.kept.borrow().consents.allows(ACCOUNT, &rt("chatgpt")));
    // Until the owner allows it.
    let v = done(run_runtime(&h, "ai.setConsent", &json!({"runtime": "chatgpt", "account": ACCOUNT, "allowed": true})));
    assert_eq!(v, json!({"runtime": "chatgpt", "allowed": true}));
    assert!(h.kept.borrow().consents.allows(ACCOUNT, &rt("chatgpt")));
    // A permutation of the chain.
    let order = json!(["codex", "claude_code", "uar", "chatgpt", "compat:office", "on_device"]);
    done(run_runtime(&h, "ai.setRuntimeOrder", &json!({"order": order})));
    let ids: Vec<String> = h.kept.borrow().chain.ids().iter().map(ToString::to_string).collect();
    assert_eq!(json!(ids), order);
    // Options.
    done(run_runtime(&h, "ai.setRuntimeOption", &json!({"runtime": "claude_code", "model": "sonnet", "path": "/usr/local/bin/claude"})));
    let s = h.settings();
    let e = s.chain.get(&rt("claude_code")).unwrap();
    assert_eq!(e.model().map(|m| m.as_str()), Some("sonnet"));
    assert_eq!(e.path().map(RuntimePath::as_str), Some("/usr/local/bin/claude"));
    done(run_runtime(&h, "ai.setRuntimeOption", &json!({"runtime": "claude_code", "model": null})));
    assert!(h.settings().chain.get(&rt("claude_code")).unwrap().model().is_none());
    assert!(h.settings().chain.get(&rt("claude_code")).unwrap().path().is_some(), "left out: unchanged");
    // Which Universal Agent Runtime: the owner's own running one, then MailCraft's again.
    done(run_runtime(&h, "ai.setRuntimeOption", &json!({"runtime": "uar", "instance": "external"})));
    assert_eq!(h.settings().chain.get(&rt("uar")).unwrap().instance(), Some(UarInstance::External));
    done(run_runtime(&h, "ai.setRuntimeOption", &json!({"runtime": "uar", "instance": "managed"})));
    assert_eq!(h.settings().chain.get(&rt("uar")).unwrap().instance(), Some(UarInstance::Managed));
    // The fallback.
    for f in ["ask", "stop", "next"] {
        let v = done(run_runtime(&h, "ai.setFallback", &json!({"value": f})));
        assert_eq!(v["fallback"], f);
    }
    assert_eq!(h.saves.get(), 11);
}

#[test]
fn the_runtime_list_holds_the_listed_fields_and_no_secret() {
    let h = Host::new(true);
    let v = done(run_runtime(&h, "ai.runtimes", &json!({})));
    let rows = v["runtimes"].as_array().unwrap();
    assert_eq!(rows.len(), 6);
    for row in rows {
        let mut keys: Vec<&str> = row.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["enabled", "id", "name", "path", "status", "version", "where"], "{row}");
    }
    let claude = rows.iter().find(|r| r["id"] == "claude_code").unwrap();
    assert_eq!(claude["status"], "ready");
    assert_eq!(claude["version"], "2.1.4");
    assert_eq!(claude["path"], "/opt/example/bin/claude");
    assert_eq!(claude["where"], "leaves");
    let codex = rows.iter().find(|r| r["id"] == "codex").unwrap();
    assert_eq!(codex["version"], Value::Null, "a program's output is not a version");
    let text = v.to_string();
    assert!(!text.contains("sk-not") && !text.contains("someone@example.org") && !text.contains(ACCOUNT), "{text}");
    assert_eq!(v["fallback"], "next");
    // Null params are no params.
    assert_eq!(done(run_runtime(&h, "ai.runtimes", &Value::Null)), v);
}

#[test]
fn the_order_changes_only_by_permutation() {
    let h = Host::new(true);
    let before = h.settings();
    for order in [
        json!(["on_device", "compat:office"]),
        json!(["on_device", "compat:office", "chatgpt", "claude_code", "codex", "codex"]),
        json!(["on_device", "compat:office", "chatgpt", "claude_code", "codex", "uar", "compat:other"]),
        json!(["on_device", "compat:office", "chatgpt", "claude_code", "codex", "opencode"]),
        json!(["on_device", "compat:office", "chatgpt", "claude_code", "codex", 7]),
        json!([]),
        json!("on_device"),
        json!(vec!["uar"; 10_000]),
    ] {
        assert!(run_runtime(&h, "ai.setRuntimeOrder", &json!({"order": order})).is_err(), "{order:.80}");
    }
    assert_eq!(h.settings(), before);
    assert_eq!(h.saves.get(), 0);
}

#[test]
fn unknown_runtimes_and_stray_options_are_refused() {
    let h = Host::new(true);
    let before = h.settings();
    for (m, p) in [
        ("ai.setRuntimeEnabled", json!({"runtime": "opencode", "value": true})),
        ("ai.setRuntimeEnabled", json!({"runtime": "compat:nowhere", "value": true})),
        ("ai.setRuntimeEnabled", json!({"runtime": "CODEX", "value": true})),
        ("ai.setRuntimeEnabled", json!({"runtime": "codex", "value": "yes"})),
        ("ai.setRuntimeOption", json!({"runtime": "codex", "auth": "oauth"})),
        ("ai.setRuntimeOption", json!({"runtime": "codex", "model": "a b"})),
        ("ai.setRuntimeOption", json!({"runtime": "codex", "path": "relative/codex"})),
        ("ai.setRuntimeOption", json!({"runtime": "codex", "path": "/opt/../etc/codex"})),
        ("ai.setRuntimeOption", json!({"runtime": "on_device", "model": "x"})),
        ("ai.setRuntimeOption", json!({"runtime": "compat:office", "path": "/bin/x"})),
        ("ai.setRuntimeOption", json!({"runtime": "codex", "instance": "external"})),
        ("ai.setRuntimeOption", json!({"runtime": "codex"})),
        ("ai.setRuntimeOption", json!({"runtime": "uar", "instance": "somewhere"})),
        ("ai.setRuntimeOption", json!({"runtime": "uar", "instance": 1})),
        ("ai.setFallback", json!({"value": "retry"})),
        ("ai.setConsent", json!({"runtime": "opencode", "account": ACCOUNT, "allowed": true})),
        ("ai.setConsent", json!({"runtime": "chatgpt", "account": "not an address", "allowed": true})),
        // A runtime that keeps mail on this computer needs no consent.
        ("ai.setConsent", json!({"runtime": "on_device", "account": ACCOUNT, "allowed": true})),
        ("ai.setConsent", json!({"runtime": "uar", "account": ACCOUNT, "allowed": true})),
        ("ai.runtimes", json!({"unexpected": true})),
        ("ai.noSuchThing", json!({})),
    ] {
        assert!(run_runtime(&h, m, &p).is_err(), "{m} {p}");
    }
    assert_eq!(h.settings(), before);
    assert_eq!(h.saves.get(), 0);
    assert!(h.calls.borrow().is_empty());
}

#[test]
fn sign_in_never_takes_a_clis_credentials() {
    let h = Host::new(true);
    // Codex: nothing is spawned, `codex login` is the owner's to run.
    let v = done(run_runtime(&h, "ai.signIn", &json!({"runtime": "codex"})));
    assert_eq!(v["runYourself"], "codex login");
    assert!(v["message"].as_str().unwrap().contains("yourself"));
    assert!(h.calls.borrow().is_empty(), "nothing asked of the host for Codex");
    // Claude Code: the owner's terminal with `claude auth login`.
    let v = done(run_runtime(&h, "ai.signIn", &json!({"runtime": "claude_code"})));
    assert_eq!(v, json!({"pending": "signIn", "runtime": "claude_code"}));
    done(run_runtime(&h, "ai.signIn", &json!({"runtime": "chatgpt"})));
    done(run_runtime(&h, "ai.signIn", &json!({"runtime": "uar"})));
    assert_eq!(*h.calls.borrow(), ["signIn ClaudeCodeTerminal", "signIn ChatGpt", "signIn ManagedUar"]);
    // Nothing to sign in to.
    assert!(run_runtime(&h, "ai.signIn", &json!({"runtime": "on_device"})).is_err());
    assert!(run_runtime(&h, "ai.signIn", &json!({"runtime": "compat:office"})).unwrap_err().contains("ai.setRuntimeKey"));
    assert_eq!(h.calls.borrow().len(), 3);
    assert_eq!(h.saves.get(), 0);
}

#[test]
fn what_the_host_doesnt_carry_yet_is_an_error_not_a_panic() {
    let h = Bare(RefCell::new(settings()));
    for (m, p) in [
        ("ai.signIn", json!({"runtime": "chatgpt"})),
        ("ai.signIn", json!({"runtime": "claude_code"})),
        ("ai.signIn", json!({"runtime": "uar"})),
        ("ai.signOut", json!({"runtime": "chatgpt"})),
        ("ai.signOut", json!({"runtime": "compat:office"})),
        ("ai.rescan", json!({})),
    ] {
        let e = run_runtime(&h, m, &p).unwrap_err();
        assert!(e.contains("isn't available in this version yet"), "{m}: {e}");
    }
    // Codex needs nothing of the host.
    assert_eq!(done(run_runtime(&h, "ai.signIn", &json!({"runtime": "codex"})))["runYourself"], "codex login");
    assert_eq!(*h.0.borrow(), settings());
}

#[test]
fn sign_out_never_touches_a_clis_store() {
    let h = Host::new(true);
    for (id, command) in [("claude_code", "claude auth logout"), ("codex", "codex logout")] {
        let v = done(run_runtime(&h, "ai.signOut", &json!({"runtime": id})));
        assert_eq!(v["runYourself"], command);
    }
    assert!(h.calls.borrow().is_empty(), "nothing asked of the host for a CLI");
    done(run_runtime(&h, "ai.signOut", &json!({"runtime": "chatgpt"})));
    done(run_runtime(&h, "ai.signOut", &json!({"runtime": "compat:office"})));
    assert_eq!(*h.calls.borrow(), ["signOut ChatGpt", "signOut CompatKey(ProviderId(\"office\"))"]);
    assert!(run_runtime(&h, "ai.signOut", &json!({"runtime": "uar"})).is_err());
    assert!(run_runtime(&h, "ai.signOut", &json!({"runtime": "on_device"})).is_err());
}

#[test]
fn keys_are_only_typed_in_the_window_and_claude_and_codex_hold_none() {
    let h = Host::new(true);
    for id in ["claude_code", "codex", "chatgpt", "on_device", "uar", "compat:elsewhere"] {
        assert!(run_runtime(&h, "ai.setRuntimeKey", &json!({"runtime": id})).is_err(), "{id}");
    }
    let e = run_runtime(&h, "ai.setRuntimeKey", &json!({"runtime": "claude_code"})).unwrap_err();
    assert!(e.contains("your own login"), "{e}");
    match run_runtime(&h, "ai.setRuntimeKey", &json!({"runtime": "compat:office"})) {
        Ok(Step::OpenKeyField(p)) => assert_eq!(p.as_str(), "office"),
        other => panic!("{other:?}"),
    }
    // No method takes a key or a token, however it is spelled; none is echoed.
    for m in RUNTIME_METHODS {
        for k in ["key", "apiKey", "API_KEY", "api-key", "secret", "token", "Token", "password", "auth", "refresh_token", "accessToken"] {
            let e = run_runtime(&h, m, &json!({"runtime": "compat:office", k: "sk-not-a-real-key"})).unwrap_err();
            assert!(!e.contains("sk-not"), "{m} {k}: {e}");
        }
    }
    assert!(h.calls.borrow().is_empty());
    assert_eq!(h.saves.get(), 0);
}

/// A small deterministic generator of junk JSON: wrong types, nulls, huge strings, nesting.
struct Junk(u64);

impl Junk {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn value(&mut self, depth: u32) -> Value {
        let words = [
            "runtime",
            "value",
            "order",
            "model",
            "path",
            "account",
            "allowed",
            "codex",
            "claude_code",
            "compat:office",
            "on_device",
            "opencode",
            "uar",
            "chatgpt",
            "next",
            "/bin/x",
            ACCOUNT,
            "",
            "\u{0}",
            "../..",
            "compat:",
        ];
        let word = |j: &mut Junk| words[(j.next() % words.len() as u64) as usize].to_string();
        match self.next() % if depth > 4 { 6 } else { 9 } {
            0 => Value::Null,
            1 => json!(self.next().is_multiple_of(2)),
            2 => json!(self.next() as i64),
            3 => json!(f64::from(self.next() as u32) / 7.0),
            4 => json!(word(self)),
            5 => json!(word(self).repeat((self.next() % 20_000) as usize + 1)),
            6 => Value::Array((0..self.next() % 8).map(|_| self.value(depth + 1)).collect()),
            _ => Value::Object((0..self.next() % 6).map(|_| (word(self), self.value(depth + 1))).collect()),
        }
    }
}

#[test]
fn hostile_runtime_params_never_panic_and_never_change_the_settings_unasked() {
    let h = Host::new(false);
    let long = "x".repeat(100_000);
    let fixed = [
        json!(null),
        json!(7),
        json!("s"),
        json!([1]),
        json!({}),
        json!({"runtime": 5, "value": "yes", "order": {}, "account": [], "allowed": "no"}),
        json!({"runtime": long, "account": long, "model": long, "path": long, "value": long}),
        json!({"runtime": "co\u{0}dex", "value": true}),
        json!({"runtime": format!("compat:{long}"), "value": true}),
        json!({"runtime": {"runtime": {"runtime": "codex"}}, "value": true}),
        json!({"order": vec![json!({"nested": [1, [2, [3]]]}); 64]}),
        Value::Object((0..1000).map(|n| (format!("m{n}"), json!(n))).collect()),
    ];
    let mut junk = Junk(0x9E37_79B9_7F4A_7C15);
    for m in RUNTIME_METHODS {
        for p in fixed.iter().cloned().chain((0..300).map(|_| junk.value(0))) {
            // `ai.setFallback` and `ai.setRuntimeEnabled` can be well formed by chance; anything
            // else that comes back is an answer, never a panic.
            let _ = run_runtime(&h, m, &p);
        }
    }
    for p in &fixed {
        for m in RUNTIME_METHODS {
            let r = run_runtime(&h, m, p);
            assert!(m == "ai.runtimes" || r.is_err(), "{m} {p:.60}");
        }
    }
    let s = h.settings();
    // Whatever changed came through the checked builders: the chain is still the six runtimes.
    assert_eq!(s.chain.len(), 6);
    assert!(s.chain.get(&rt("compat:office")).is_some());
}

#[test]
fn every_ai_method_is_refused_to_agents_and_logged_as_refused() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let mut a = app(&calls);
    for m in PROVIDER_METHODS.iter().chain(RUNTIME_METHODS.iter()) {
        assert!(crate::agent_gate::refused_to_agents(m), "{m} is not in the gate's refused arm");
        let v = call(&mut a, m, json!({"runtime": "codex", "value": true, "id": "p", "account": ACCOUNT, "allowed": true}), true);
        assert_eq!(v["ok"], false, "{m}");
        assert_eq!(v["error"], format!("`{m}` isn't available to agents"), "{m}");
        let last = a.asking.activity.last().unwrap();
        assert_eq!(last.decided, Decided::Refused, "{m}");
        assert_eq!(last.command.as_deref(), Some(*m));
    }
    // And through `engine.execute`, by the engine's class.
    for m in RUNTIME_METHODS {
        let v = call(&mut a, "engine.execute", json!({"command": m, "params": {"runtime": "codex", "value": true}}), true);
        assert_eq!(v["ok"], false, "{m}");
        assert_eq!(a.asking.activity.last().unwrap().decided, Decided::Refused, "{m}");
    }
    assert!(calls.borrow().is_empty());
}

#[test]
fn the_engines_owner_only_list_and_the_control_methods_agree() {
    use mailcraft_engine::agent::{AI_RUNTIME_OWNER_ONLY, Access, access};
    let mut engine: Vec<&str> = AI_RUNTIME_OWNER_ONLY.to_vec();
    let mut here: Vec<&str> = RUNTIME_METHODS.to_vec();
    engine.sort_unstable();
    here.sort_unstable();
    assert_eq!(engine, here);
    for m in RUNTIME_METHODS {
        assert_eq!(access(m), Some(Access::Never), "{m}");
        assert!(super::is_ai_method(m));
    }
}

#[test]
fn an_owner_script_without_a_runtime_host_gets_words_not_a_panic() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let mut a = app(&calls);
    for m in RUNTIME_METHODS {
        let v = call(&mut a, m, json!({"runtime": "codex", "value": true, "account": ACCOUNT, "allowed": true}), false);
        assert_eq!(v["ok"], false, "{m}");
        assert!(v["error"].as_str().unwrap().contains("aren't available in this build"), "{m}: {v}");
    }
    // The provider form of `ai.setConsent` still reaches the providers' host.
    assert_eq!(call(&mut a, "ai.setConsent", json!({"id": "p", "account": ACCOUNT, "allowed": true}), false)["ok"], true);
    assert_eq!(calls.borrow().len(), 1);
}

#[test]
fn the_key_field_opens_in_the_window() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let mut a = app(&calls);
    let h = Host::new(true);
    let step = run_runtime(&h, "ai.setRuntimeKey", &json!({"runtime": "compat:office"})).unwrap();
    let v = super::ai::runtimes_finish(&mut a, step).unwrap();
    assert_eq!(v, json!({"opened": "aiSettings", "runtime": "compat:office"}));
}
