// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `clientInfo` from `initialize` is the client's own word: kept only as cleaned, bounded strings,
//! handed to the backend for the window (`agent.client`), and nothing else changes with it.

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use crate::client::MAX_CHARS;
use crate::{Backend, ClientInfo, Headless, Server};

fn info(params: Value) -> Option<ClientInfo> {
    ClientInfo::from_initialize(&params)
}

/// A 10 KB name with right-to-left overrides, newlines, NUL and other controls.
fn hostile() -> String {
    "Evil\u{202e}gnp.exe\n\u{0}\r\t\u{1b}[31m\u{2066}\u{200b}\u{feff}Agent ".repeat(400)
}

fn clean(s: &str) -> bool {
    s.chars().count() <= MAX_CHARS && !s.chars().any(|c| c.is_control() || mailcraft_names::is_format(c))
}

#[test]
fn a_hostile_client_info_is_cleaned_and_bounded() {
    let i = info(json!({"clientInfo": {"name": hostile(), "version": hostile()}})).unwrap();
    for f in [i.name.as_deref().unwrap(), i.version.as_deref().unwrap()] {
        assert!(clean(f), "{f:?}");
        assert!(f.starts_with("Evilgnp.exe"), "{f:?}");
    }
    // Cut at a character's boundary, never inside one.
    let i = info(json!({"clientInfo": {"name": "é😀".repeat(300)}})).unwrap();
    assert_eq!(i.name.as_deref().unwrap().chars().count(), MAX_CHARS);
}

#[test]
fn only_strings_count_and_nothing_left_is_nothing() {
    for bad in [json!(null), json!(7), json!(true), json!(["x"]), json!({"x": 1}), json!(""), json!("\u{202e}\n\u{0}")] {
        assert_eq!(info(json!({"clientInfo": {"name": bad.clone(), "version": bad.clone()}})), None, "{bad}");
    }
    for bad in [json!(null), json!("Agent"), json!(1), json!([])] {
        assert_eq!(info(json!({"clientInfo": bad.clone()})), None, "{bad}");
    }
    assert_eq!(info(json!({})), None);
    assert_eq!(info(json!(null)), None);
    assert_eq!(info(json!([1, 2])), None);
}

#[test]
fn a_version_alone_or_a_name_alone_is_passed_on_alone() {
    let i = info(json!({"clientInfo": {"name": "Example Agent", "version": "1.2.3"}})).unwrap();
    assert_eq!(i.to_value(), json!({"name": "Example Agent", "version": "1.2.3"}));
    let i = info(json!({"clientInfo": {"name": "Example Agent", "version": 3}})).unwrap();
    assert_eq!(i.to_value(), json!({"name": "Example Agent"}));
    let i = info(json!({"clientInfo": {"version": "2.0"}})).unwrap();
    assert_eq!(i.to_value(), json!({"version": "2.0"}));
}

#[test]
fn debug_never_carries_the_name() {
    let i = info(json!({"clientInfo": {"name": "Example Agent", "version": "9.9"}})).unwrap();
    let d = format!("{i:?}");
    assert!(!d.contains("Example") && !d.contains("9.9"), "{d}");
}

/// A backend that keeps what it was told about the client, and answers every call.
struct Recorder(Arc<Mutex<Vec<Option<ClientInfo>>>>);

impl Backend for Recorder {
    fn call(&mut self, _method: &str, _params: Value) -> Result<Value, String> {
        Ok(json!({}))
    }
    fn has_ui(&self) -> bool {
        true
    }
    fn describe(&self) -> String {
        "recorder".into()
    }
    fn set_client(&mut self, client: Option<ClientInfo>) {
        if let Ok(mut seen) = self.0.lock() {
            seen.push(client);
        }
    }
}

fn initialize(s: &mut Server, params: Value) -> Value {
    serde_json::from_str(&s.handle_line(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": params}).to_string()).unwrap()).unwrap()
}

#[test]
fn initialize_hands_the_client_to_the_backend_and_answers_as_ever() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut s = Server::new(Box::new(Recorder(seen.clone())));
    let named = initialize(&mut s, json!({"protocolVersion": "2025-06-18", "clientInfo": {"name": hostile(), "version": "1.0"}}));
    let plain = initialize(&mut s, json!({"protocolVersion": "2025-06-18"}));
    // The answer is the same with a name or without: nothing of it comes back.
    assert_eq!(named, plain);
    assert!(!named.to_string().contains("Evil"), "{named}");
    // A client that sends no clientInfo (or a broken one) is served; it clears what was there.
    let broken = initialize(&mut s, json!({"clientInfo": "Example"}));
    assert!(broken["result"]["protocolVersion"].is_string(), "{broken}");
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 3);
    assert!(seen[0].as_ref().is_some_and(|c| c.name.as_deref().is_some_and(clean) && c.version.as_deref() == Some("1.0")));
    assert_eq!(seen[1], None);
    assert_eq!(seen[2], None);
}

#[test]
fn headless_ignores_the_client() {
    let mut s = Server::new(Box::new(Headless::default()));
    let r = initialize(&mut s, json!({"clientInfo": {"name": hostile(), "version": [1]}}));
    assert!(r["result"]["serverInfo"]["name"] == "mailcraft", "{r}");
    let r: Value = serde_json::from_str(&s.handle_line(r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#).unwrap()).unwrap();
    assert_eq!(r["result"], json!({}));
}
