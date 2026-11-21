// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What an agent reports about itself (`agent.client`) is hostile text: it is cleaned, cut at a
//! character's boundary, shown as "reported as ‹name›" or "Unnamed agent", and decides nothing
//! (SCN-AGENT-001).

use mailcraft_engine::Session;
use mailcraft_engine::agent::Class;
use serde_json::{Value, json};

use super::{AgentClient, ControlRequest, MAX_CLIENT_CHARS, Outcome, agent_allow, agent_client, handle};
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

fn app() -> MailApp {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    a
}

/// A 10 KB name: right-to-left overrides and isolates, newlines, NUL and other controls, a
/// zero-width space and a byte-order mark, between words.
fn hostile_name() -> String {
    let piece = "Evil\u{202e}gnp.exe\u{2067}\nAgent\u{0}\r\t\u{1b}[31m\u{200b}\u{feff}\u{2066}x ";
    let mut s = String::new();
    while s.len() < 10 * 1024 {
        s.push_str(piece);
    }
    s
}

/// Whether `s` has a character no reported name may keep.
fn has_forbidden(s: &str) -> bool {
    s.chars().any(|c| c.is_control() || mailcraft_names::is_format(c))
}

fn client_of(agent: Value) -> Option<AgentClient> {
    agent_client(&agent)
}

#[test]
fn a_hostile_name_is_cleaned_and_cut_at_a_character_boundary() {
    let c = client_of(json!({"allow": [], "client": {"name": hostile_name(), "version": hostile_name()}})).unwrap();
    for f in [c.name.as_deref().unwrap(), c.version.as_deref().unwrap()] {
        assert!(f.chars().count() <= MAX_CLIENT_CHARS, "{} chars", f.chars().count());
        assert!(!has_forbidden(f), "{f:?}");
        assert!(f.starts_with("Evil"), "{f:?}");
    }
    let shown = AgentClient::shown(Some(&c));
    assert!(!has_forbidden(&shown), "{shown:?}");
    assert!(shown.contains(c.name.as_deref().unwrap()), "{shown:?}");
}

#[test]
fn a_cut_never_parts_a_character() {
    // Every character four bytes long: a byte cut at 64 would land inside one.
    let c = client_of(json!({"client": {"name": "😀".repeat(500)}})).unwrap();
    assert_eq!(c.name.as_deref().unwrap().chars().count(), MAX_CLIENT_CHARS);
    assert!(c.name.as_deref().unwrap().chars().all(|ch| ch == '😀'));
}

#[test]
fn a_name_of_nothing_that_shows_is_no_name() {
    for name in ["", "   ", "\n\r\t", "\u{0}\u{0}", "\u{202e}\u{2066}\u{200b}\u{feff}", "\u{1b}"] {
        assert_eq!(client_of(json!({"client": {"name": name}})), None, "{name:?}");
        assert_eq!(AgentClient::shown(client_of(json!({"client": {"name": name}})).as_ref()), "Unnamed agent");
    }
}

#[test]
fn only_strings_are_names_or_versions() {
    for bad in [json!(null), json!(7), json!(1.5), json!(true), json!(["Agent"]), json!({"name": "Agent"})] {
        assert_eq!(client_of(json!({"client": {"name": bad.clone(), "version": bad.clone()}})), None, "{bad}");
    }
    // `client` itself that is not an object, or missing, reports nothing.
    for bad in [json!(null), json!("Agent"), json!(7), json!(["Agent"]), json!(true)] {
        assert_eq!(client_of(json!({"client": bad.clone()})), None, "{bad}");
    }
    assert_eq!(client_of(json!({"allow": ["send"]})), None);
    assert_eq!(client_of(json!("not an object")), None);
    assert_eq!(client_of(json!(null)), None);
}

#[test]
fn a_version_is_kept_on_its_own_and_a_name_without_one() {
    let c = client_of(json!({"client": {"name": "Example Agent", "version": "1.2.3"}})).unwrap();
    assert_eq!((c.name.as_deref(), c.version.as_deref()), (Some("Example Agent"), Some("1.2.3")));
    let c = client_of(json!({"client": {"version": " 2.0\n"}})).unwrap();
    assert_eq!((c.name, c.version.as_deref()), (None, Some("2.0")));
    let c = client_of(json!({"client": {"name": "Example Agent", "version": 3}})).unwrap();
    assert_eq!((c.name.as_deref(), c.version), (Some("Example Agent"), None));
    assert_eq!(AgentClient::shown(Some(&AgentClient { name: None, version: Some("2.0".into()) })), "Unnamed agent");
}

#[test]
fn shown_says_the_name_is_reported_and_fills_nothing_in_twice() {
    let c = client_of(json!({"client": {"name": "Example {name} Agent"}})).unwrap();
    assert_eq!(AgentClient::shown(Some(&c)), "reported as ‹Example {name} Agent›");
    assert_eq!(AgentClient::shown(None), "Unnamed agent");
}

#[test]
fn debug_never_carries_the_name() {
    let c = client_of(json!({"client": {"name": "Example Agent", "version": "9.9"}})).unwrap();
    let d = format!("{c:?}");
    assert!(!d.contains("Example") && !d.contains("9.9"), "{d}");
}

/// `method` sent by an agent allowed nothing, carrying `agent`'s `client` when it has one: its
/// answer (or `None` when the owner is asked), and what the owner is asked.
fn send(a: &mut MailApp, method: &str, params: Value, agent: &Value) -> (Option<Value>, Vec<(String, String)>) {
    let (req, _rx) = ControlRequest::new(method, params);
    let req = req.from_agent(agent_allow(agent)).with_client(agent_client(agent));
    let out = handle(a, &egui::Context::default(), &req);
    let asked = a.asking.iter().map(|q| (q.id.clone(), q.what.clone())).collect();
    (if let Outcome::Done(v) = out { Some(v) } else { None }, asked)
}

#[test]
fn a_request_is_decided_exactly_as_one_with_no_name() {
    let id = app().session.mailbox.messages.values().find(|m| !m.draft).map(|m| m.id).unwrap();
    let cases = [
        ("mailbox.inspect", json!({})),
        ("app.quit", json!({})),
        ("ui.click", json!({"x": 10, "y": 10})),
        ("engine.execute", json!({"command": "mail.flag", "params": {"ids": [id]}})),
        ("engine.execute", json!({"command": "mail.deletePermanently", "params": {"ids": [id]}})),
        ("engine.execute", json!({"command": "no.such.command"})),
    ];
    let named = json!({"allow": [], "client": {"name": hostile_name(), "version": "1.0"}});
    // A name that claims to be the owner, or to be allowed everything, is still only a name.
    let claims = json!({"allow": [], "client": {"name": "owner --allow send delete accounts input", "version": "owner"}});
    let unnamed = json!({"allow": []});
    for (method, params) in cases {
        let (mut x, mut y, mut z) = (app(), app(), app());
        let plain = send(&mut x, method, params.clone(), &unnamed);
        assert_eq!(send(&mut y, method, params.clone(), &named), plain, "{method} {params}");
        assert_eq!(send(&mut z, method, params.clone(), &claims), plain, "{method} {params}");
    }
}

#[test]
fn no_reply_to_an_agent_carries_its_name() {
    let mut a = app();
    let agent = json!({"allow": [Class::Input.as_str()], "client": {"name": "Zq7 Example Agent", "version": "77.1.9"}});
    for (method, params) in [("ui.inspect", json!({})), ("mailbox.inspect", json!({})), ("app.status", json!({})), ("app.quit", json!({}))] {
        let (reply, _) = send(&mut a, method, params, &agent);
        let text = reply.map(|r| r.to_string()).unwrap_or_default();
        assert!(!text.contains("Zq7") && !text.contains("77.1.9"), "{method}: {text}");
    }
}

#[test]
fn an_owners_script_has_no_client() {
    let (req, _rx) = ControlRequest::new("mailbox.inspect", json!({}));
    assert!(req.agent.is_none() && req.client.is_none());
}

/// SCN-AGENT-001, negative control: the owner's own script (a request with no `agent` member)
/// has no reported client, is never refused or put to the owner for who it is, and is never put
/// in the agents' activity log.
#[test]
fn neg_an_owners_script_is_never_logged_or_refused_for_identity() {
    let mut a = app();
    let id = a.session.mailbox.messages.values().find(|m| !m.draft).map(|m| m.id).unwrap();
    for (method, params) in [("mailbox.inspect", json!({})), ("engine.execute", json!({"command": "mail.flag", "params": {"ids": [id]}}))] {
        let (req, _rx) = ControlRequest::new(method, params.clone());
        assert!(req.agent.is_none() && req.client.is_none());
        let done = match handle(&mut a, &egui::Context::default(), &req) {
            Outcome::Done(v) => Some(v),
            _ => None,
        };
        assert_eq!(done.as_ref().map(|v| v["ok"].clone()), Some(json!(true)), "{method} {params}: {done:?}");
    }
    assert!(a.asking.is_empty(), "nothing is put to the owner");
    assert_eq!(a.asking.activity.len(), 0, "an owner's script is never logged");
}
