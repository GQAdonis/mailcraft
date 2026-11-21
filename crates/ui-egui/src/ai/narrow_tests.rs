// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Find by Description's badge (A7, ai-narrow-badge) and the proposal's room (W0): the badge says
//! where the description went in words, from the provider that answered, with the host alone and
//! never a user name, password, path, query, fragment or key; an address that can't be read
//! names the kind alone and nothing panics; its text keeps the 11 pt floor and its accessible
//! label is its words; and the proposal's field and Use This Search are drawn inside the panel,
//! on screen, whatever height the panel had while it asked.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::Receiver;

use egui::accesskit::Role;
use egui_kittest::kittest::Queryable as _;
use serde_json::{Value, json};

use super::{Route, Stage, badge_host};
use crate::AiProviders;
use crate::chrome::type_floor_tests::{assert_type_floor, text_count};
use crate::people_view::testing::{self, Window};

// ---- The host and the words ----------------------------------------------------------------

#[test]
fn the_badge_names_each_kind_in_words() {
    assert_eq!(Route::of("local", "http://127.0.0.1:11434/v1").words(), "On this computer");
    assert_eq!(Route::of("gateway", "http://127.0.0.1:4000/v1").words(), "Gateway: 127.0.0.1:4000");
    assert_eq!(Route::of("remote", "https://models.example/v1").words(), "Remote: models.example");
    // A kind this window doesn't know is never taken for this computer.
    assert_eq!(Route::of("something-new", "https://models.example").words(), "Remote: models.example");
}

#[test]
fn the_badge_shows_the_host_alone_never_a_secret() {
    // The brief's own case.
    assert_eq!(badge_host("https://user:secret@models.example/v1?key=abc").as_deref(), Some("models.example"));
    assert_eq!(Route::of("remote", "https://user:secret@models.example/v1?key=abc").words(), "Remote: models.example");
    for (url, host) in [
        ("https://models.example/v1/chat?api_key=sk-123#frag", "models.example"),
        ("https://u@models.example", "models.example"),
        ("https://a:b@c@models.example/x", "models.example"),
        ("HTTPS://Models.Example:443/v1", "models.example"),
        ("http://models.example:80", "models.example"),
        ("https://models.example:8443/v1", "models.example:8443"),
        ("http://[::1]:4000/v1", "[::1]:4000"),
        ("https://models.example\\@evil.example/", "models.example"),
        ("  https://models.example./  ", "models.example"),
    ] {
        let shown = badge_host(url);
        assert_eq!(shown.as_deref(), Some(host), "{url}");
        let words = Route::of("gateway", url).words();
        for secret in ["user", "secret", "key", "sk-123", "abc", "frag", "/v1", "?", "#", "@"] {
            assert!(!words.contains(secret), "{url}: {words:?} shows {secret:?}");
        }
    }
}

#[test]
fn an_unreadable_address_names_the_kind_alone_and_never_panics() {
    for url in [
        "",
        "models.example",
        "://models.example",
        "https://",
        "https://user:pass@",
        "https://user:pass@/v1",
        "https://models.example:99999",
        "https://models.example:-1",
        "https://models.example:12ab",
        "https://mod els.example",
        "https://models..example",
        "https://.example",
        "https://[::1",
        "https://[zz]:80",
        "https://[::1]junk",
        "ht tp://models.example",
        "https://\u{202e}elpmaxe.sledom",
        "https://modèles.example",
        "https://\u{0}",
    ] {
        assert_eq!(badge_host(url), None, "{url:?}");
        assert_eq!(Route::of("remote", url).words(), "Remote", "{url:?}");
        assert_eq!(Route::of("gateway", url).words(), "Gateway", "{url:?}");
    }
    // Long and odd input: an answer or none, never a panic, never more than the cap.
    let long = format!("https://{}.example/{}", "a".repeat(100_000), "é".repeat(10_000));
    let _ = badge_host(&long);
    let many = format!("https://{}example", "a.".repeat(120));
    if let Some(host) = badge_host(&many) {
        assert!(host.chars().count() <= 64, "cut to the cap: {host}");
    }
    for junk in ["https://é@é:é/é", "https://@", "https://:@:", "https://[]", "https://[]:1", "a://b:65535"] {
        let _ = badge_host(junk);
    }
}

/// SCN-RT-BADGE-001, negative control: a well-formed chain whose first runtime that can answer is
/// the on-device model says "On this computer", and no runtime that leaves (off, or signed out)
/// is named in its place.
#[test]
fn neg_badge_a_well_formed_on_device_chain_says_on_this_computer() {
    use mailcraft_model::ai::{Chain, ChainEntry, Consents, Fallback, RuntimeId, Where};

    use crate::ai::runtime_rows::{Extra, Facts, Platform, RowFacts};
    use crate::control::{RuntimeSettings, RuntimeState, RuntimeStatus};

    fn row(id: &str, enabled: bool, state: RuntimeState, place: Where) -> RowFacts {
        RowFacts {
            id: RuntimeId::parse(id).unwrap(),
            enabled,
            status: RuntimeStatus { state, ..Default::default() },
            place,
            extra: Extra::default(),
            compat: None,
            model: None,
        }
    }
    for rows in [
        vec![row("on_device", true, RuntimeState::Ready, Where::Local)],
        vec![
            row("chatgpt", false, RuntimeState::Ready, Where::Leaves),
            row("claude_code", true, RuntimeState::SignedOut, Where::Leaves),
            row("on_device", true, RuntimeState::Ready, Where::Local),
        ],
    ] {
        let chain = Chain::new(rows.iter().map(|r| ChainEntry::new(r.id.clone()).with_enabled(r.enabled))).unwrap();
        let facts = Facts {
            settings: RuntimeSettings { chain, consents: Consents::new(), fallback: Fallback::Next },
            rows,
            platform: Some(Platform { mac: true, windows: false }),
            signing_in: None,
        };
        let route = Route::of_chain(&facts).expect("the on-device model answers");
        assert_eq!(route.words(), "On this computer");
        for leaving in ["ChatGPT", "Claude Code", "Remote", "Gateway"] {
            assert!(!route.words().contains(leaving), "{}", route.words());
        }
    }
}

// ---- The window ----------------------------------------------------------------------------

/// A host in memory with one provider in use.
#[derive(Clone, Default)]
struct Host(Rc<RefCell<Value>>);

fn sent<T: Send + 'static>(value: T) -> Receiver<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = tx.send(value);
    rx
}

impl AiProviders for Host {
    fn providers(&self) -> Value {
        self.0.borrow().clone()
    }
    fn set_provider(&self, _provider: &Value) -> Result<(), String> {
        Ok(())
    }
    fn remove_provider(&self, _id: &str) -> Result<(), String> {
        Ok(())
    }
    fn set_active(&self, _id: &str) -> Result<(), String> {
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
}

/// A model request the host keeps under way: nothing ends it but `ai.apply`.
fn model_under_way(_job: crate::ModelJob) -> Receiver<mailcraft_model::ai::ModelEvent> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::mem::forget(tx);
    rx
}

/// The window with `provider` in use, asked for a search, and the proposal back from the provider
/// `answered_by` names (none: the host said nobody).
fn proposing(provider: Value, answered_by: Option<&str>) -> Window {
    proposing_in(provider, answered_by, Look::LIGHT)
}

/// A look of the window: theme, high contrast and interface size.
#[derive(Clone, Copy, Debug)]
struct Look {
    name: &'static str,
    theme: &'static str,
    contrast: bool,
    size: u32,
}

impl Look {
    const LIGHT: Look = Look { name: "light", theme: "light", contrast: false, size: 100 };
    const ALL: [Look; 4] = [
        Look::LIGHT,
        Look { name: "dark", theme: "dark", contrast: false, size: 100 },
        Look { name: "hc", theme: "light", contrast: true, size: 100 },
        Look { name: "200", theme: "light", contrast: false, size: 200 },
    ];
}

/// [`proposing`] in `look`. At 200 % the window is twice as large, so that it is as wide in
/// points as at 100 % and the search box (and the panel under it) is drawn.
fn proposing_in(provider: Value, answered_by: Option<&str>, look: Look) -> Window {
    let id = provider.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
    let host = Host(Rc::new(RefCell::new(json!({"active": id, "providers": [provider], "backup": "apple-on-device"}))));
    let mut w = testing::window("view.mail");
    let _ = w.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    w.state_mut().services.ai_providers = Some(Box::new(host));
    w.state_mut().services.model_request = Some(Box::new(model_under_way));
    w.state_mut().execute("ui.theme", json!({"value": look.theme})).unwrap();
    w.state_mut().execute("ui.highContrast", json!({"on": look.contrast})).unwrap();
    w.state_mut().execute("ui.interfaceSize", json!({"value": look.size})).unwrap();
    let scale = look.size as f32 / 100.0;
    w.set_size(egui::vec2(1280.0 * scale, 800.0 * scale));
    w.run_steps(4);
    let asked = w.state_mut().execute("ui.findByDescription", json!({"text": "invoices from Ada with a PDF", "suggest": true})).unwrap();
    assert_eq!(asked["stage"], "asking");
    let request = asked["request"].as_u64().expect("a request");
    // The panel is drawn at its asking height first: what cut the proposal off.
    w.run_steps(4);
    let mut done = json!({"type": "done", "text": "{\"query\": \"from:ada has:attachment invoice\"}"});
    if let Some(by) = answered_by {
        done["answeredBy"] = json!({"provider": by, "model": "m"});
    }
    w.state_mut().execute("ai.apply", json!({"requestId": request, "event": done})).unwrap();
    w.run_steps(6);
    assert_eq!(w.state().ai.narrow.as_ref().map(|p| p.stage), Some(Stage::Proposal));
    w
}

fn local() -> Value {
    json!({"id": "local", "kind": "local", "base_url": "http://127.0.0.1:11434/v1", "model": "m"})
}

fn gateway() -> Value {
    json!({"id": "gw", "kind": "gateway", "base_url": "http://127.0.0.1:4000/v1", "model": "m"})
}

fn remote() -> Value {
    json!({"id": "far", "kind": "remote", "base_url": "https://user:secret@models.example/v1?key=abc", "model": "m"})
}

/// The badge's words, as the panel keeps them.
fn badge_of(w: &Window) -> Option<String> {
    w.state().ai.narrow.as_ref().and_then(|p| p.route.as_ref()).map(Route::words)
}

#[test]
fn the_badge_follows_the_provider_that_answered() {
    for (provider, by, words) in [
        (local(), Some("local"), "On this computer"),
        (gateway(), Some("gw"), "Gateway: 127.0.0.1:4000"),
        (remote(), Some("far"), "Remote: models.example"),
        // The host said nothing: the provider in use.
        (remote(), None, "Remote: models.example"),
        // The on-device backup answered for the remote primary: this computer.
        (remote(), Some("apple-on-device"), "On this computer"),
    ] {
        let mut w = proposing(provider, by);
        assert_eq!(badge_of(&w).as_deref(), Some(words), "{by:?}");
        let answered = w.state_mut().execute("ui.findByDescription", json!({"open": true})).unwrap();
        assert_eq!(answered["badge"], json!(words), "the command says the badge too");
        assert!(w.query_by_label(words).is_some(), "{words}: the accessible label is the words");
        // Nothing of the address but its host anywhere in the tree.
        for secret in ["secret", "user:", "key=abc", "/v1"] {
            assert!(w.query_by_label_contains(secret).is_none(), "{secret} is never shown");
        }
    }
}

#[test]
fn a_search_the_owner_writes_has_no_badge() {
    let mut w = proposing(remote(), Some("far"));
    w.state_mut().execute("ui.findByDescription", json!({"query": "from:ada"})).unwrap();
    w.run_steps(3);
    assert_eq!(badge_of(&w), None);
    assert!(w.state().hit.rect("narrowBadge", 0).is_none());
}

#[test]
fn the_proposal_and_use_this_search_are_drawn_inside_the_panel() {
    for (provider, by, look) in [(local(), Some("local"), Look::LIGHT), (remote(), Some("far"), Look::ALL[3])] {
        let mut w = proposing_in(provider, by, look);
        // And some frames later still: the panel doesn't settle back to its asking height.
        w.run_steps(30);
        let screen = w.ctx.content_rect();
        let panel = w.state().hit.rect("findByDescription", 0).expect("the panel");
        let shown = panel.intersect(screen);
        let use_it = w.state().hit.rect("useThisSearch", 0).expect("Use This Search is drawn");
        assert!(shown.contains_rect(use_it), "Use This Search {use_it:?} is inside the panel {shown:?}");
        let field = w
            .query_all_by_role(Role::TextInput)
            .find(|f| f.value().as_deref() == Some("from:Ada attachment:pdf invoice"))
            .map(|f| f.rect())
            .expect("the proposal's field");
        assert!(shown.contains_rect(field), "the field {field:?} is inside the panel {shown:?}");
        assert!(field.max.y <= use_it.min.y, "the field comes before the button");
        // The labels as decided.
        for label in ["Use This Search", "Try Again", "Cancel"] {
            assert!(w.query_all_by_label(label).next().is_some(), "{label}");
        }
        // The badge, too, is inside the panel.
        let badge = w.state().hit.rect("narrowBadge", 0).expect("the badge");
        assert!(shown.contains_rect(badge), "the badge {badge:?} is inside the panel {shown:?}");
    }
}

#[test]
fn the_badge_keeps_the_type_floor_in_every_look() {
    for look in Look::ALL {
        let w = proposing_in(gateway(), Some("gw"), look);
        let badge = w.state().hit.rect("narrowBadge", 0).unwrap_or_else(|| panic!("{look:?}: the badge"));
        assert!(text_count(w.output(), badge) > 0, "{look:?}: the badge's words are drawn");
        assert_type_floor(&w, badge, 11.0);
    }
}

#[test]
fn escape_still_closes_a_proposal_with_a_badge() {
    let mut w = proposing(local(), Some("local"));
    testing::key(&mut w, egui::Key::Escape);
    assert!(w.state().ai.narrow.is_none(), "Escape closes the proposal");
}

/// The pictures for the change, when `MAILCRAFT_SHOTS_DIR` names a folder: the proposal with each
/// kind's badge, in light, dark, high contrast and at 200 %.
#[test]
fn shots() {
    let Ok(dir) = std::env::var("MAILCRAFT_SHOTS_DIR") else { return };
    let dir = std::path::Path::new(&dir);
    for (name, provider, by) in [("local", local(), "local"), ("gateway", gateway(), "gw"), ("remote", remote(), "far")] {
        for look in Look::ALL {
            let mut w = proposing_in(provider.clone(), Some(by), look);
            w.render().unwrap().save(dir.join(format!("{}-narrow-{name}.png", look.name))).unwrap();
        }
    }
}
