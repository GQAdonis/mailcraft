// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File › Options › AI assistance as the runtime chain (B11, 21.7), in a window: every new
//! control is found by its accessible name and reached with Tab, the order changes with the
//! keyboard (and `ai.setRuntimeOrder` runs with the new order), turning a runtime on allows no
//! account anything, Escape closes the details panel and gives the keyboard back to Details, the
//! consent dialog's default is Don't Allow and Enter answers no, and Codex's `codex login` is
//! shown and never run.

use std::cell::RefCell;
use std::rc::Rc;

use egui::Key;
use egui_kittest::kittest::{NodeT, Queryable};
use mailcraft_model::ai::{Chain, ChainEntry, Consents, Fallback, RuntimeId, Where};
use serde_json::json;

use crate::control::{AiRuntimes, RuntimeSettings, RuntimeState, RuntimeStatus, SignIn, SignOut};
use crate::people_view::testing::{self, Window};

/// What the test host keeps and what was asked of it.
#[derive(Default)]
pub(crate) struct Kept {
    pub(crate) settings: RuntimeSettings,
    pub(crate) states: Vec<(RuntimeId, RuntimeState)>,
    pub(crate) calls: Vec<String>,
    pub(crate) sign_in_fails: bool,
}

/// A runtimes host in memory: the settings as the window leaves them, and every call noted.
#[derive(Clone, Default)]
pub(crate) struct Host(pub(crate) Rc<RefCell<Kept>>);

impl Host {
    pub(crate) fn set_state(&self, id: &str, state: RuntimeState) {
        let id = RuntimeId::parse(id).unwrap();
        let mut k = self.0.borrow_mut();
        k.states.retain(|(r, _)| r != &id);
        k.states.push((id, state));
    }

    pub(crate) fn order(&self) -> Vec<String> {
        self.0.borrow().settings.chain.entries().iter().map(|e| e.runtime().to_string()).collect()
    }

    pub(crate) fn enabled(&self, id: &str) -> bool {
        self.0.borrow().settings.chain.get(&RuntimeId::parse(id).unwrap()).is_some_and(ChainEntry::enabled)
    }

    pub(crate) fn calls(&self) -> Vec<String> {
        self.0.borrow().calls.clone()
    }
}

impl AiRuntimes for Host {
    fn settings(&self) -> RuntimeSettings {
        self.0.borrow().settings.clone()
    }
    fn save(&self, s: &RuntimeSettings) -> Result<(), String> {
        let mut k = self.0.borrow_mut();
        k.calls.push("save".into());
        k.settings = s.clone();
        Ok(())
    }
    fn status(&self, runtime: &RuntimeId) -> RuntimeStatus {
        let state = self.0.borrow().states.iter().find(|(r, _)| r == runtime).map_or(RuntimeState::Ready, |(_, s)| *s);
        RuntimeStatus { state, ..Default::default() }
    }
    fn where_of(&self, runtime: &RuntimeId) -> Where {
        runtime.kind().fixed_where().unwrap_or(Where::Leaves)
    }
    fn rescan(&self) -> Result<(), String> {
        self.0.borrow_mut().calls.push("rescan".into());
        Ok(())
    }
    fn sign_in(&self, how: SignIn) -> Result<(), String> {
        let mut k = self.0.borrow_mut();
        k.calls.push(format!("signIn {how:?}"));
        if k.sign_in_fails { Err("no terminal".into()) } else { Ok(()) }
    }
    fn sign_out(&self, what: &SignOut) -> Result<(), String> {
        self.0.borrow_mut().calls.push(format!("signOut {what:?}"));
        Ok(())
    }
}

/// The chain the tests start from: Claude Code on, Codex off and signed out.
pub(crate) fn host() -> Host {
    let ids = ["claude_code", "codex"];
    let chain = Chain::new(ids.iter().map(|id| ChainEntry::new(RuntimeId::parse(id).unwrap()).with_enabled(*id == "claude_code"))).unwrap();
    let h = Host::default();
    h.0.borrow_mut().settings = RuntimeSettings { chain, consents: Consents::new(), fallback: Fallback::Next };
    h.set_state("codex", RuntimeState::SignedOut);
    h
}

/// The window at File › Options › AI assistance, with `host`, and an account at ada@example.org.
pub(crate) fn window(host: &Host) -> Window {
    let mut w = testing::window("view.mail");
    let _ = w.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    let _ = w.state_mut().execute("account.add", json!({"email": "ada@example.org", "displayName": "Ada Example"}));
    w.state_mut().services.ai_runtimes = Some(Box::new(host.clone()));
    w.state_mut().execute("ui.aiSettings", json!({})).unwrap();
    w.run_steps(6);
    w
}

/// The node named `label` has the keyboard.
pub(crate) fn focused(w: &Window, label: &str) -> bool {
    w.get_by_label(label).is_focused()
}

/// The name of what has the keyboard, for a failing assertion's message.
pub(crate) fn focus_name(w: &Window) -> Option<String> {
    w.query_by(|n| n.is_focused()).and_then(|n| n.accesskit_node().label())
}

#[test]
fn every_row_control_is_named_and_reached_with_tab_in_reading_order() {
    let h = host();
    let mut w = window(&h);
    w.get_by_label("Move Claude Code down").focus();
    w.run_steps(2);
    // Up (disabled on the first row, so no stop), down, the switch, Details; then the next row:
    // up (down is disabled on the last row), its in-row Copy, the action (Codex signed out: Look
    // Again), the switch, Details.
    for next in ["Use Claude Code", "Details: Claude Code", "Move Codex up", "Copy codex login", "Look Again: Codex", "Use Codex", "Details: Codex"] {
        w.key_press(Key::Tab);
        w.run_steps(2);
        assert!(focused(&w, next), "Tab reaches {next}, not {:?}", focus_name(&w));
    }
    for name in ["Move Claude Code down", "Use Claude Code", "Details: Claude Code", "Look Again", "Add OpenAI-Compatible Provider…"] {
        let _ = w.get_by_label(name);
    }
    for radio in ["Try the next one", "Ask me first", "Stop and tell me"] {
        let _ = w.get_by_label(radio);
    }
}

#[test]
fn reorder_by_keyboard_runs_set_runtime_order_and_keeps_the_keyboard_on_the_moved_row() {
    let h = host();
    let mut w = window(&h);
    // Tab from Look Again to the first row's down button (its up button is disabled), then Enter.
    w.get_by_label("Look Again").focus();
    w.run_steps(2);
    w.key_press(Key::Tab);
    w.run_steps(2);
    assert!(focused(&w, "Move Claude Code down"), "{:?}", focus_name(&w));
    w.key_press(Key::Enter);
    w.run_steps(4);
    assert_eq!(h.order(), ["codex", "claude_code"]);
    // The moved row is now last: its down arrow is disabled, so the keyboard is on its up arrow.
    assert!(focused(&w, "Move Claude Code up"), "the keyboard stays with the moved row");
}

#[test]
fn turning_a_runtime_on_allows_no_account_and_opens_nothing() {
    let h = host();
    let mut w = window(&h);
    w.get_by_label("Use Codex").click_accesskit();
    w.run_steps(4);
    assert!(h.enabled("codex"));
    assert!(h.0.borrow().settings.consents.is_empty(), "no consent from a switch");
    assert_eq!(super::settings_runtimes::dialog_name(w.state()), None);
    assert!(w.state().dialog.is_none());
}

#[test]
fn escape_closes_the_details_panel_and_gives_details_the_keyboard_back() {
    let h = host();
    let mut w = window(&h);
    w.get_by_label("Details: Claude Code").click_accesskit();
    w.run_steps(3);
    let _ = w.get_by_label("Copy claude auth logout");
    assert!(w.state().ai.runtimes.open.is_some());
    w.key_press(Key::Escape);
    w.run_steps(3);
    assert!(w.state().ai.runtimes.open.is_none());
    assert!(focused(&w, "Details: Claude Code"));
    assert!(w.query_by_label("Copy claude auth logout").is_none());
}

#[test]
fn codex_signed_out_shows_codex_login_and_never_runs_it() {
    let h = host();
    let mut w = window(&h);
    let _ = w.get_by_label("Copy codex login");
    w.get_by_label("Look Again: Codex").click_accesskit();
    w.run_steps(3);
    let calls = h.calls();
    assert!(calls.contains(&"rescan".to_string()), "{calls:?}");
    assert!(!calls.iter().any(|c| c.starts_with("signIn")), "nothing is signed in for Codex: {calls:?}");
}

#[test]
fn the_consent_dialogs_default_is_dont_allow_and_enter_answers_no() {
    let h = host();
    let mut w = window(&h);
    w.get_by_label("Use Codex").click_accesskit();
    w.run_steps(3);
    w.get_by_label("Allow…: ada@example.org mail to Codex").click_accesskit();
    w.run_steps(4);
    assert_eq!(super::settings_runtimes::dialog_name(w.state()), Some("aiRuntimeConsent"));
    assert!(focused(&w, "Don't Allow"), "the default focus");
    w.key_press(Key::Enter);
    w.run_steps(4);
    assert_eq!(super::settings_runtimes::dialog_name(w.state()), None);
    assert!(!h.0.borrow().settings.consents.allows("ada@example.org", &RuntimeId::Codex), "Enter is no");
    // Escape is no too; only Allow says yes.
    w.get_by_label("Allow…: ada@example.org mail to Codex").click_accesskit();
    w.run_steps(4);
    w.key_press(Key::Escape);
    w.run_steps(4);
    assert_eq!(super::settings_runtimes::dialog_name(w.state()), None);
    assert!(h.0.borrow().settings.consents.is_empty());
    w.get_by_label("Allow…: ada@example.org mail to Codex").click_accesskit();
    w.run_steps(4);
    w.key_press(Key::Tab);
    w.run_steps(2);
    assert!(focused(&w, "Allow") || focused(&w, "Don't Allow"), "Tab stays in the dialog");
    w.get_by_label("Allow").click_accesskit();
    w.run_steps(4);
    assert!(h.0.borrow().settings.consents.allows("ada@example.org", &RuntimeId::Codex));
    // Stop takes it back at once.
    w.get_by_label("Stop: ada@example.org mail to Codex").click_accesskit();
    w.run_steps(4);
    assert!(!h.0.borrow().settings.consents.allows("ada@example.org", &RuntimeId::Codex));
}

#[test]
fn the_fallback_choice_runs_set_fallback() {
    let h = host();
    let mut w = window(&h);
    w.get_by_label("Stop and tell me").click_accesskit();
    w.run_steps(3);
    assert_eq!(h.0.borrow().settings.fallback, Fallback::Stop);
}

#[test]
fn the_page_is_drawn_without_a_crash_for_every_state_and_hostile_extras() {
    let h = host();
    let mut w = window(&h);
    for state in [
        RuntimeState::Ready,
        RuntimeState::SignedOut,
        RuntimeState::NotFound,
        RuntimeState::Checking,
        RuntimeState::Unavailable,
        RuntimeState::Unknown,
    ] {
        h.set_state("claude_code", state);
        w.state_mut().stand_in_runtime_details("claude_code");
        w.state_mut().stand_in_runtime_extra("claude_code", &json!({"status": "broken", "reason": "\u{0}x", "folder": [1]}));
        w.run_steps(2);
        w.state_mut().stand_in_runtime_extra("not a runtime", &json!(null));
        w.run_steps(2);
    }
}
