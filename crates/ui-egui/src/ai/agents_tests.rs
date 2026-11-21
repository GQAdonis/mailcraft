// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Agents button and panel and Stop Agent Access (agent-console 8.7–8.9; SCN-AGENT-001,
//! SCN-AGENT-002 and the access and panel scenarios): a hostile reported name is drawn cleaned
//! and decides nothing; stopping access refuses every agent request and answers the questions
//! waiting no, and turning it back on lets agents in again; an agent can neither open the panel
//! nor turn access back on; the owner's own scripts are not touched; the panel draws no mail; and
//! its controls have names, focus rings and keep to the type floor. Synthetic mail only.

use std::sync::mpsc::Receiver;

use egui_kittest::kittest::Queryable;
use mailcraft_engine::Session;
use mailcraft_engine::agent::Class;
use serde_json::{Value, json};

use super::agents::{self, Seen};
use crate::MailApp;
use crate::agent_gate::activity::{Decided, Entry, Log};
use crate::chrome::type_floor_tests::{assert_type_floor, text_count};
use crate::control::{AgentClient, ControlRequest, Outcome, agent_client};
use crate::people_view::testing::{self, Window};
use crate::theme::Tokens;

const NOW: i64 = 1_791_993_600;
const AGENT_NAME: &str = "Example Agent";

fn app() -> MailApp {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), crate::Services::default());
    a.autosave = false;
    a.session.driver.set_auto_sync(false);
    a
}

fn named(name: &str) -> Option<AgentClient> {
    agent_client(&json!({"client": {"name": name, "version": "1.0"}}))
}

/// `method` from an agent allowed `allow` that reported `client`: its answer now, if any.
fn agent_as(a: &mut MailApp, client: Option<AgentClient>, method: &str, params: Value, allow: Vec<Class>) -> (Option<Value>, Receiver<Value>) {
    let (req, rx) = ControlRequest::new(method, params);
    let req = req.from_agent(allow).with_client(client);
    let out = crate::control::handle(a, &egui::Context::default(), &req);
    (if let Outcome::Done(v) = out { Some(v) } else { None }, rx)
}

fn agent(a: &mut MailApp, method: &str, params: Value, allow: Vec<Class>) -> (Option<Value>, Receiver<Value>) {
    agent_as(a, named(AGENT_NAME), method, params, allow)
}

fn owner(a: &mut MailApp, method: &str, params: Value) -> Option<Value> {
    let (req, _) = ControlRequest::new(method, params);
    if let Outcome::Done(v) = crate::control::handle(a, &egui::Context::default(), &req) { Some(v) } else { None }
}

/// A message being written, which an agent can be asked to send (a question for the owner).
fn write(a: &mut MailApp) {
    a.execute("mail.new", json!({"to": "someone@example.org", "subject": "Re: Hall booking"})).unwrap();
}

fn send(a: &mut MailApp) -> Receiver<Value> {
    let (now, rx) = agent(a, "engine.execute", json!({"command": "compose.send"}), vec![]);
    assert!(now.is_none(), "asked first");
    rx
}

fn decided(a: &MailApp) -> Vec<Decided> {
    a.asking.activity.entries().map(|e| e.decided).collect()
}

// ---- Stop Agent Access ----------------------------------------------------------------------

#[test]
fn stopping_access_answers_the_questions_waiting_no_and_refuses_the_next() {
    let mut a = app();
    write(&mut a);
    let (first, second) = (send(&mut a), send(&mut a));
    assert_eq!(a.asking.len(), 2);
    a.execute("ui.agentAccess", json!({"value": false})).unwrap();
    assert!(a.asking.is_empty(), "the questions are gone");
    for rx in [first, second] {
        let v = rx.try_recv().unwrap();
        assert_eq!(v["ok"], false);
        assert!(v["error"].as_str().unwrap().contains("stopped agent access"), "{v}");
    }
    assert_eq!(decided(&a), vec![Decided::Refused, Decided::Refused], "logged as refused, not as the owner's no");
    assert!(a.session.compose.is_some(), "nothing was sent");
    // Every agent request after it is refused, a look at the window's state too, with a reason.
    for (method, params) in [
        ("engine.execute", json!({"command": "mail.list"})),
        ("mail.list", json!({})),
        ("ui.inspect", json!({})),
        ("ui.screenshot", json!({})),
        ("ui.click", json!({"x": 1, "y": 1})),
    ] {
        let (r, _) = agent(&mut a, method, params, Class::ALL.to_vec());
        let r = r.expect("settled at once");
        assert_eq!(r["ok"], false, "{method}");
        assert!(r["error"].as_str().unwrap().contains("stopped agent access"), "{r}");
        assert!(r.get("result").is_none(), "only ok and error: {r}");
    }
    assert!(a.synthetic.is_empty(), "no input reached the window");
    // The command an agent named is logged as refused; a look at the window isn't logged.
    let last: Vec<&Entry> = a.asking.activity.entries().skip(2).collect();
    assert_eq!(last.len(), 4, "{last:?}");
    assert!(last.iter().all(|e| e.decided == Decided::Refused));
    assert_eq!(last.first().and_then(|e| e.command.as_deref()), Some("mail.list"));
}

#[test]
fn access_back_on_lets_agents_in_again() {
    let mut a = app();
    a.execute("ui.agentAccess", json!({"value": false})).unwrap();
    assert_eq!(agent(&mut a, "engine.execute", json!({"command": "mail.list"}), vec![]).0.unwrap()["ok"], false);
    let on = a.execute("ui.agentAccess", json!({"value": true})).unwrap();
    assert_eq!(on["value"], true);
    let r = agent(&mut a, "engine.execute", json!({"command": "mail.list"}), vec![]).0.unwrap();
    assert_eq!(r["ok"], true, "{r}");
    // And a question is asked again rather than refused.
    write(&mut a);
    let _ = send(&mut a);
    assert_eq!(a.asking.len(), 1);
    // Left out, the switch flips.
    assert_eq!(a.execute("ui.agentAccess", json!({})).unwrap()["value"], false);
    assert!(a.asking.is_empty());
    assert_eq!(a.execute("ui.agentAccess", json!({})).unwrap()["value"], true);
}

#[test]
fn an_agent_cant_open_the_panel_or_turn_access_back_on() {
    let mut a = app();
    // Access on: both are the owner's alone.
    for (id, params) in [("ui.agents", json!({"open": true})), ("ui.agentAccess", json!({"value": false})), ("ui.agentAccess", json!({}))] {
        assert_eq!(mailcraft_engine::agent::access(id), Some(mailcraft_engine::agent::Access::Never));
        for allow in [vec![], Class::ALL.to_vec()] {
            let (r, _) = agent(&mut a, "engine.execute", json!({"command": id, "params": params}), allow.clone());
            assert_eq!(r.unwrap()["ok"], false, "{id}");
            let (r, _) = agent(&mut a, id, params.clone(), allow);
            assert_eq!(r.unwrap()["ok"], false, "{id}");
        }
    }
    assert!(!a.ai.agents.open);
    assert!(!agents::stopped(&a));
    // Access stopped: an agent can't turn it back on.
    a.execute("ui.agentAccess", json!({"value": false})).unwrap();
    for (method, params) in
        [("engine.execute", json!({"command": "ui.agentAccess", "params": {"value": true}})), ("ui.agentAccess", json!({"value": true}))]
    {
        let (r, _) = agent(&mut a, method, params, Class::ALL.to_vec());
        assert_eq!(r.unwrap()["ok"], false);
        assert!(agents::stopped(&a), "access stays off");
    }
}

#[test]
fn the_owners_own_scripts_are_neither_refused_nor_logged() {
    let mut a = app();
    a.execute("ui.agentAccess", json!({"value": false})).unwrap();
    let before = a.asking.activity.len();
    assert_eq!(owner(&mut a, "engine.execute", json!({"command": "mail.list"})).unwrap()["ok"], true);
    assert_eq!(owner(&mut a, "ui.inspect", json!({})).unwrap()["ok"], true);
    write(&mut a);
    assert_eq!(owner(&mut a, "engine.execute", json!({"command": "compose.send"})).unwrap()["ok"], true, "the owner's send isn't an agent's");
    assert_eq!(a.asking.activity.len(), before);
    // The owner turns it back on from a script too.
    assert_eq!(owner(&mut a, "engine.execute", json!({"command": "ui.agentAccess", "params": {"value": true}})).unwrap()["ok"], true);
    assert!(!agents::stopped(&a));
}

#[test]
fn the_switch_and_the_panel_take_junk_without_a_panic() {
    let mut a = app();
    for junk in [json!("x"), json!(5), json!([]), json!({"value": "no"}), json!({"value": 0}), json!({"open": "yes"}), json!({"open": null})] {
        let _ = a.execute("ui.agentAccess", junk.clone());
        let _ = a.execute("ui.agents", junk);
    }
    assert!(a.execute("ui.agentAccess", json!({"value": "off"})).is_err());
    assert!(a.execute("ui.agents", json!({"open": 1})).is_err());
    assert_eq!(a.execute("ui.agents", json!({"open": true})).unwrap()["open"], true);
    assert_eq!(a.execute("ui.agents", json!({})).unwrap()["open"], false, "left out, it toggles");
}

#[test]
fn stopping_access_is_never_saved() {
    let mut a = app();
    a.execute("ui.agentAccess", json!({"value": false})).unwrap();
    let saved = serde_json::to_string(&a.ui).unwrap();
    assert!(!saved.to_ascii_lowercase().contains("agent"), "{saved}");
}

// ---- What the panel works out ---------------------------------------------------------------

fn entry(at: f64, client: Option<AgentClient>, decided: Decided, class: Option<Class>) -> Entry {
    Entry { at_ms: at, client, command: Some("mail.list".into()), class, decided, failed: false, count: Some(3) }
}

#[test]
fn recently_active_counts_each_reported_agent_with_what_it_was_allowed() {
    let mut log = Log::default();
    let (ada, ben) = (named("Ada's helper"), named("Ben's helper"));
    log.record(entry(1_000.0, ada.clone(), Decided::NoQuestion, None));
    log.record(entry(2_000.0, ben.clone(), Decided::AllowedInAdvance, Some(Class::Send)));
    log.record(entry(3_000.0, ada.clone(), Decided::SaidNo, Some(Class::Delete)));
    log.record(entry(4_000.0, None, Decided::Refused, None));
    log.record(entry(5_000.0, ben.clone(), Decided::AllowedInAdvance, Some(Class::Send)));
    log.record(entry(6_000.0, ben.clone(), Decided::AllowedInAdvance, Some(Class::Input)));
    let seen = agents::recently_active(&log);
    assert_eq!(
        seen,
        vec![
            Seen { client: ben, last_ms: 6_000.0, requests: 3, ran: 3, allowed: vec![Class::Send, Class::Input] },
            Seen { client: None, last_ms: 4_000.0, requests: 1, ran: 0, allowed: vec![] },
            Seen { client: ada, last_ms: 3_000.0, requests: 2, ran: 1, allowed: vec![] },
        ]
    );
    assert!(agents::recent(&log, 6_000.0 + agents::RECENT_MS));
    assert!(!agents::recent(&log, 6_001.0 + agents::RECENT_MS));
    assert!(!agents::recent(&Log::default(), 0.0));
}

#[test]
fn verdicts_say_who_decided_and_counts_only_for_what_ran() {
    let mut e = entry(0.0, None, Decided::AllowedInAdvance, Some(Class::Send));
    assert_eq!(agents::outcome_words(&e), "Ran: allowed in advance · 3 results");
    e.decided = Decided::SaidYes;
    assert_eq!(agents::outcome_words(&e), "You said yes · 3 results");
    e.count = Some(1);
    assert_eq!(agents::outcome_words(&e), "You said yes · 1 result");
    e.failed = true;
    assert_eq!(agents::outcome_words(&e), "You said yes, with an error");
    e.decided = Decided::SaidNo;
    assert_eq!(agents::outcome_words(&e), "Not run: you said no");
    assert_eq!(agents::ago(10.0 * 60_000.0, 0.0), "10 minutes ago");
    assert_eq!(agents::ago(30_000.0, 0.0), "just now");
    assert_eq!(agents::ago(0.0, 30_000.0), "just now", "a clock that went back");
    assert_eq!(agents::ago(f64::INFINITY, 0.0), "just now");
    assert_eq!(agents::ago(125.0 * 60_000.0, 0.0), "2 hours ago");
    for c in Class::ALL {
        assert!(!agents::class_words(c).is_empty());
    }
}

// ---- The panel in the window ----------------------------------------------------------------

/// Every text the last frame drew in `region`.
fn texts(output: &egui::FullOutput, region: egui::Rect) -> Vec<String> {
    fn walk(shape: &egui::Shape, region: egui::Rect, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, region, out)),
            egui::Shape::Text(t) if t.visual_bounding_rect().intersects(region) => out.push(t.galley.job.text.clone()),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for c in &output.shapes {
        walk(&c.shape, region, &mut out);
    }
    out
}

fn window() -> Window {
    let mut w = testing::window("view.mail");
    let _ = w.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    w.state_mut().session.driver.set_auto_sync(false);
    w.run_steps(3);
    w
}

fn open_panel(w: &mut Window) -> egui::Rect {
    w.state_mut().execute("ui.agents", json!({"open": true})).unwrap();
    w.run_steps(4);
    w.state().hit.rect("agentsPanel", 0).expect("the panel is drawn")
}

/// Has the window's agent `client` do a few things: a list, a search, a read, and one refused.
fn some_activity(w: &mut Window, client: Option<AgentClient>, query: &str, id: u64) {
    let a = w.state_mut();
    let _ = agent_as(a, client.clone(), "engine.execute", json!({"command": "mail.list", "params": {}}), vec![]);
    let _ = agent_as(a, client.clone(), "engine.execute", json!({"command": "view.search", "params": {"query": query}}), vec![]);
    let _ = agent_as(a, client.clone(), "engine.execute", json!({"command": "mail.get", "params": {"id": id}}), vec![]);
    let _ = agent_as(a, client, "app.quit", json!({}), vec![]);
}

#[test]
fn scn_agent_001_a_hostile_name_is_drawn_cleaned_and_decides_nothing() {
    let mut w = window();
    let hostile: String = "\u{202E}gnp.exe\u{2066}Trusted\nAgent\r\u{0}\u{200B}".repeat(400);
    assert!(hostile.len() >= 10 * 1024 - 2000, "{} bytes", hostile.len());
    let client = agent_client(&json!({"client": {"name": hostile, "version": "9\n9"}}));
    // Decided exactly as one with no name: the same answer for the same request.
    let a = w.state_mut();
    let named = agent_as(a, client.clone(), "engine.execute", json!({"command": "mail.list", "params": {}}), vec![]).0.unwrap();
    let nameless = agent_as(a, None, "engine.execute", json!({"command": "mail.list", "params": {}}), vec![]).0.unwrap();
    assert_eq!(named, nameless);
    let refused = agent_as(a, client.clone(), "app.quit", json!({}), vec![]).0.unwrap();
    assert_eq!(refused, agent_as(a, None, "app.quit", json!({}), vec![]).0.unwrap());
    let r = open_panel(&mut w);
    let drawn = texts(w.output(), r);
    let names: Vec<&String> = drawn.iter().filter(|s| s.starts_with("reported as ‹")).collect();
    assert!(!names.is_empty(), "the name is drawn: {drawn:?}");
    for s in drawn.iter() {
        assert!(!s.chars().any(|c| c.is_control() && c != '\n'), "a control character drawn: {s:?}");
        assert!(!s.chars().any(|c| matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200B}'..='\u{200F}')), "bidi drawn: {s:?}");
    }
    for s in names {
        assert!(!s.contains('\n'), "one line: {s:?}");
        let inner = s.trim_start_matches("reported as ‹").trim_end_matches('›');
        assert!(inner.chars().count() <= crate::control::MAX_CLIENT_CHARS, "{} characters", inner.chars().count());
    }
    assert!(drawn.iter().any(|s| s == "Unnamed agent"), "the nameless one too: {drawn:?}");
}

#[test]
fn scn_agent_002_the_panel_draws_no_mail() {
    let mut w = window();
    let first = w.state().session.mailbox.messages.values().find(|m| !m.draft).unwrap().clone();
    let query = first.subject.split_whitespace().max_by_key(|word| word.len()).unwrap().to_string();
    some_activity(&mut w, named(AGENT_NAME), &query, first.id);
    some_activity(&mut w, None, &first.from.email, first.id);
    let r = open_panel(&mut w);
    let drawn = texts(w.output(), r).join("\n");
    // (The panel scrolls: the oldest rows are below its foot.)
    assert!(drawn.contains("app.quit") && drawn.contains("view.search") && drawn.contains("mail.get"), "{drawn}");
    let mut words = vec![query, first.from.email.clone()];
    for m in w.state().session.mailbox.messages.values() {
        words.push(m.subject.clone());
        for who in std::iter::once(&m.from).chain(&m.to).chain(&m.cc) {
            words.push(who.email.clone());
            words.push(who.name.clone());
        }
        words.push(m.body.text.lines().map(str::trim).find(|l| l.chars().count() >= 12).unwrap_or("").to_string());
    }
    words.retain(|word| word.trim().chars().count() >= 6);
    assert!(words.len() > 10);
    for word in &words {
        assert!(!drawn.contains(word.as_str()), "the panel drew {word:?}");
    }
}

#[test]
fn the_panel_opens_by_its_button_has_named_controls_and_closes_on_escape() {
    let mut w = window();
    let name = "Agents";
    assert!(w.state().hit.rect("agentsButton", 0).is_some(), "the button is in the title bar");
    w.get_by_label(name).click();
    w.run_steps(4);
    assert!(w.state().ai.agents.open, "a click opens it");
    let panel = w.state().hit.rect("agentsPanel", 0).expect("drawn");
    let title = w.state().hit.rect("agentsButton", 0).unwrap();
    assert!(panel.min.y > title.max.y, "under the title bar");
    let _ = w.get_by_label("Close Agents");
    let _ = w.get_by_label("Stop Agent Access");
    // Not modal: the rest of the window still answers.
    w.state_mut().execute("mail.next", json!({})).unwrap();
    testing::key(&mut w, egui::Key::Escape);
    w.run_steps(2);
    assert!(!w.state().ai.agents.open, "Escape closes it");
    // The close button too.
    open_panel(&mut w);
    w.get_by_label("Close Agents").click();
    w.run_steps(4);
    assert!(!w.state().ai.agents.open);
}

#[test]
fn the_panel_stops_access_and_says_so() {
    let mut w = window();
    write(w.state_mut());
    let rx = send(w.state_mut());
    w.state_mut().execute("ui.agentAccess", json!({"value": false})).unwrap();
    assert_eq!(rx.try_recv().unwrap()["ok"], false);
    open_panel(&mut w);
    let _ = w.get_by_label("Allow Agent Access");
    let _ = w.get_by_label("Agents: access stopped");
    w.get_by_label("Allow Agent Access").click();
    w.run_steps(4);
    assert!(!agents::stopped(w.state()));
    w.get_by_label("Stop Agent Access").click();
    w.run_steps(4);
    assert!(agents::stopped(w.state()));
}

#[test]
fn the_button_has_a_dot_and_says_so_while_agents_are_active() {
    let mut w = window();
    assert_eq!(agents::button_name(w.state()), "Agents");
    let _ = agent(w.state_mut(), "engine.execute", json!({"command": "mail.list", "params": {}}), vec![]);
    w.run_steps(3);
    let name = "Agents: active in the last 5 minutes";
    assert_eq!(agents::button_name(w.state()), name);
    let r = w.state().hit.rect("agentsButton", 0).unwrap();
    let accent = Tokens::get(&w.ctx).accent;
    let dot = shapes(w.output()).iter().any(|s| matches!(s, egui::Shape::Circle(c) if c.fill == accent && r.contains(c.center)));
    assert!(dot, "a dot in the accent on the button");
    let _ = w.get_by_label(name);
}

fn shapes(output: &egui::FullOutput) -> Vec<egui::Shape> {
    fn walk(shape: &egui::Shape, out: &mut Vec<egui::Shape>) {
        match shape {
            egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
            other => out.push(other.clone()),
        }
    }
    let mut out = Vec::new();
    for c in &output.shapes {
        walk(&c.shape, &mut out);
    }
    out
}

#[test]
fn enter_on_the_focused_button_opens_the_panel_and_focus_shows() {
    let mut w = window();
    w.get_by_label("Agents").focus();
    w.run_steps(2);
    let r = w.state().hit.rect("agentsButton", 0).unwrap();
    let ring = Tokens::get(&w.ctx).focus_ring;
    let shown = shapes(w.output()).iter().any(|s| matches!(s, egui::Shape::Rect(x) if x.stroke.color == ring && r.intersects(x.rect)));
    assert!(shown, "a focus ring on the button");
    testing::key(&mut w, egui::Key::Enter);
    w.run_steps(2);
    assert!(w.state().ai.agents.open);
}

#[test]
fn the_panel_and_the_button_keep_to_the_type_floor() {
    let mut w = window();
    let first = w.state().session.mailbox.messages.values().find(|m| !m.draft).unwrap().clone();
    some_activity(&mut w, named(AGENT_NAME), "hall", first.id);
    let panel = open_panel(&mut w);
    assert!(text_count(w.output(), panel) > 5, "the panel's words are in the region");
    assert_type_floor(&w, panel, 11.0);
    let button = w.state().hit.rect("agentsButton", 0).unwrap();
    assert!(text_count(w.output(), button) > 0);
    assert_type_floor(&w, button, 11.0);
    // Stopped, and empty, too.
    let mut w = window();
    w.state_mut().execute("ui.agentAccess", json!({"value": false})).unwrap();
    let panel = open_panel(&mut w);
    assert_type_floor(&w, panel, 11.0);
    assert!(w.query_by_label("No agent has asked for anything in this session.").is_some());
}
