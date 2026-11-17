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
