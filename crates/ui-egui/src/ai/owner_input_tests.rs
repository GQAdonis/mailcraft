// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The owner-only AI questions (30.3): an agent's input never answers them, whether a question is
//! up or its controls are only where the agent aims; the owner's own click still does. And the
//! Universal Agent Runtime MailCraft didn't start, whose tools stay on, is used and allowed an
//! account only after the owner ticks that they understand it can run commands because of what a
//! message says.

use egui_kittest::kittest::{NodeT, Queryable};
use mailcraft_engine::agent::Class;
use mailcraft_model::ai::{Chain, ChainEntry, Consents, Fallback, RuntimeId, UarInstance, Where};
use serde_json::{Value, json};

use super::runtime_consent::{consent_text, uar_understood};
use super::runtime_rows::{Extra, RowFacts};
use super::settings_runtimes::dialog_name;
use super::settings_runtimes_tests::{Host, focused, host, window};
use crate::MailApp;
use crate::control::{ControlRequest, Outcome, RuntimeSettings, RuntimeStatus as Found};
use crate::people_view::testing::Window;

/// An agent's request, allowed input in advance: what the window answered.
fn agent(a: &mut MailApp, method: &str, params: Value) -> Value {
    let (req, _rx) = ControlRequest::new(method, params);
    match crate::control::handle(a, &egui::Context::default(), &req.from_agent(vec![Class::Input])) {
        Outcome::Done(v) => v,
        _ => Value::Null,
    }
}

/// The owner's own script's request.
fn owner(a: &mut MailApp, method: &str, params: Value) -> Value {
    let (req, _rx) = ControlRequest::new(method, params);
    match crate::control::handle(a, &egui::Context::default(), &req) {
        Outcome::Done(v) => v,
        _ => Value::Null,
    }
}

fn refused(v: &Value) -> bool {
    v["ok"] == false
}

/// The middle of `kind` as last drawn.
fn centre(w: &Window, kind: &str) -> egui::Pos2 {
    w.state().hit.rect(kind, 0).unwrap_or_else(|| panic!("{kind} is drawn")).center()
}

/// The test host with the Universal Agent Runtime in the chain as well: off and as MailCraft would
/// manage it, or `external` and on.
fn with_uar(external: bool) -> Host {
    let h = host();
    let uar = ChainEntry::new(RuntimeId::Uar);
    let uar = if external { uar.with_instance(UarInstance::External).unwrap().with_enabled(true) } else { uar };
    let chain = Chain::new([ChainEntry::new(RuntimeId::ClaudeCode).with_enabled(true), ChainEntry::new(RuntimeId::Codex), uar]).unwrap();
    h.0.borrow_mut().settings = RuntimeSettings { chain, consents: Consents::new(), fallback: Fallback::Next };
    h
}

#[test]
fn an_agents_click_on_allow_is_refused_and_the_owners_own_click_allows() {
    let h = host();
    let mut w = window(&h);
    w.get_by_label("Use Codex").click_accesskit();
    w.run_steps(3);
    w.get_by_label("Allow…: ada@example.org mail to Codex").click_accesskit();
    w.run_steps(4);
    assert_eq!(dialog_name(w.state()), Some("aiRuntimeConsent"));
    let at = centre(&w, "aiConsentAllow");
    for (method, params) in [
        ("ui.click", json!({"kind": "aiConsentAllow", "id": 0})),
        ("ui.click", json!({"x": at.x, "y": at.y})),
        ("ui.drag", json!({"x": 1.0, "y": 1.0, "toX": at.x, "toY": at.y})),
        ("ui.key", json!({"key": "Enter"})),
        ("ui.text", json!({"text": " "})),
    ] {
        let v = agent(w.state_mut(), method, params.clone());
        assert!(refused(&v), "{method} {params}: {v}");
        assert!(v["error"].as_str().unwrap_or("").contains("the owner is being asked"), "{v}");
    }
    assert!(w.state().synthetic.is_empty(), "nothing of the agent's was queued");
    w.run_steps(3);
    assert!(!h.0.borrow().settings.consents.allows("ada@example.org", &RuntimeId::Codex), "the agent allowed nothing");
    // The owner's own click.
    w.get_by_label("Allow").click_accesskit();
    w.run_steps(4);
    assert!(h.0.borrow().settings.consents.allows("ada@example.org", &RuntimeId::Codex));
}

#[test]
fn an_agents_click_on_the_fallback_question_is_refused_wherever_it_aims() {
    let h = host();
    let mut w = window(&h);
    w.run_steps(2);
    // The fallback question's Try It as the pane drew it (no frame runs after: the hit map stays).
    let yes = egui::Rect::from_min_size(egui::pos2(600.0, 300.0), egui::vec2(60.0, 24.0));
    w.state_mut().hit.add("answerFallbackYes", 0, yes);
    for (method, params) in [
        ("ui.click", json!({"kind": "answerFallbackYes", "id": 0})),
        ("ui.click", json!({"x": 620.0, "y": 310.0})),
        ("ui.drag", json!({"x": 10.0, "y": 10.0, "toX": 620.0, "toY": 310.0})),
    ] {
        let v = agent(w.state_mut(), method, params.clone());
        assert!(refused(&v), "{method} {params}: {v}");
        assert!(v["error"].as_str().unwrap_or("").contains("the owner's to answer"), "{v}");
    }
    assert!(w.state().synthetic.is_empty());
    // Anywhere else is the agent's as before.
    assert_eq!(agent(w.state_mut(), "ui.click", json!({"x": 20.0, "y": 20.0}))["ok"], true);
    w.state_mut().synthetic.clear();
    // The owner's own script reaches it.
    assert_eq!(owner(w.state_mut(), "ui.click", json!({"x": 620.0, "y": 310.0}))["ok"], true);
    assert!(!w.state().synthetic.is_empty());
}

#[test]
fn using_the_runtime_that_is_already_running_needs_the_owners_tick_first() {
    let h = with_uar(false);
    let mut w = window(&h);
    w.state_mut().stand_in_runtime_dialog("uar");
    w.run_steps(4);
    assert_eq!(dialog_name(w.state()), Some("aiUseUar"));
    assert!(focused(&w, "Don't Use It"), "the safe choice has the keyboard");
    let tick = w.get_by_label(uar_understood());
    assert_eq!(tick.accesskit_node().toggled(), Some(egui::accesskit::Toggled::False), "unticked to begin with");
    assert!(w.get_by_label("Use It").accesskit_node().is_disabled(), "Use It waits for the tick");
    w.get_by_label("Use It").click_accesskit();
    w.run_steps(4);
    assert!(!h.enabled("uar"), "nothing was turned on");
    // An agent can't tick it for the owner.
    let at = centre(&w, "aiUarUnderstood");
    assert!(refused(&agent(w.state_mut(), "ui.click", json!({"x": at.x, "y": at.y}))));
    w.get_by_label(uar_understood()).click_accesskit();
    w.run_steps(3);
    assert!(!w.get_by_label("Use It").accesskit_node().is_disabled());
    w.get_by_label("Use It").click_accesskit();
    w.run_steps(4);
    assert_eq!(dialog_name(w.state()), None);
    assert!(h.enabled("uar"));
    assert!(h.0.borrow().settings.consents.is_empty(), "no account is allowed by it");
}

#[test]
fn allowing_an_account_on_the_external_runtime_needs_the_owners_tick_first() {
    let h = with_uar(true);
    let mut w = window(&h);
    w.state_mut().stand_in_runtime_dialog("consent:uar:ada@example.org");
    w.run_steps(4);
    assert_eq!(dialog_name(w.state()), Some("aiRuntimeConsent"));
    assert!(focused(&w, "Don't Allow"));
    assert!(w.get_by_label("Allow").accesskit_node().is_disabled());
    w.get_by_label("Allow").click_accesskit();
    w.run_steps(4);
    assert!(!h.0.borrow().settings.consents.allows("ada@example.org", &RuntimeId::Uar));
    w.get_by_label(uar_understood()).click_accesskit();
    w.run_steps(3);
    w.get_by_label("Allow").click_accesskit();
    w.run_steps(4);
    assert!(h.0.borrow().settings.consents.allows("ada@example.org", &RuntimeId::Uar));
    // Opened again, the tick starts empty.
    w.state_mut().stand_in_runtime_dialog("consent:uar:ada@example.org");
    w.run_steps(4);
    assert!(w.get_by_label("Allow").accesskit_node().is_disabled());
}

#[test]
fn a_tick_isnt_asked_for_a_runtime_whose_tools_are_off() {
    let h = host();
    let mut w = window(&h);
    w.get_by_label("Use Codex").click_accesskit();
    w.run_steps(3);
    w.get_by_label("Allow…: ada@example.org mail to Codex").click_accesskit();
    w.run_steps(4);
    assert!(w.query_by_label(uar_understood()).is_none());
    assert!(!w.get_by_label("Allow").accesskit_node().is_disabled());
}

fn facts(id: RuntimeId) -> RowFacts {
    RowFacts { id, enabled: true, status: Found::default(), place: Where::Leaves, extra: Extra::default(), compat: None, model: None }
}

fn words(id: RuntimeId) -> String {
    let (_, parts) = consent_text(&facts(id), "ada@example.org");
    parts.into_iter().map(|(lead, words)| format!("{lead} {words}")).collect::<Vec<_>>().join("\n")
}

#[test]
fn the_consent_words_say_what_the_runtime_can_really_do() {
    let uar = words(RuntimeId::Uar);
    for said in ["can't turn off its tools", "run commands, open files or browse the web", "may already have run", "can't undo"] {
        assert!(uar.contains(said), "{said}: {uar}");
    }
    assert!(!uar.contains("It stops any answer"), "the old promise is gone: {uar}");
    let codex = words(RuntimeId::Codex);
    assert!(codex.contains("keeps it from writing your files and from reading most of them"), "{codex}");
    assert!(!codex.contains("can't reach your other files"), "{codex}");
}
