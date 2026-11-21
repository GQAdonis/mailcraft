// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The owner's activity log of what agents did (agent-console 8.4–8.6, SCN-AGENT-002): at most
//! 200 entries, one for each request the gate settles with who decided, ids and counts only, and
//! nothing of it in any reply to an agent. Synthetic mail only (the sample mailbox, `example.org`).

use std::sync::mpsc::Receiver;

use mailcraft_engine::Session;
use mailcraft_engine::agent::Class;
use serde_json::{Value, json};

use super::activity::{Decided, Entry, Log, MOST_ENTRIES, count_of, known_command};
use super::{WAIT_MS, answer, expire, forget};
use crate::MailApp;
use crate::control::{AgentClient, ControlRequest, Outcome};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;
const SUBJECT: &str = "Re: Quarterly figures for the hall";
const AGENT_NAME: &str = "Example Agent";

fn app() -> MailApp {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), crate::Services::default());
    a.autosave = false;
    a.session.driver.set_auto_sync(false);
    a
}

fn client() -> Option<AgentClient> {
    crate::control::agent_client(&json!({"client": {"name": AGENT_NAME, "version": "1.0"}}))
}

/// `method` from an agent allowed `allow` that reported [`AGENT_NAME`]: its answer now, if any.
fn agent(a: &mut MailApp, method: &str, params: Value, allow: Vec<Class>) -> (Option<Value>, Receiver<Value>) {
    let (req, rx) = ControlRequest::new(method, params);
    let req = req.from_agent(allow).with_client(client());
    let out = crate::control::handle(a, &egui::Context::default(), &req);
    (if let Outcome::Done(v) = out { Some(v) } else { None }, rx)
}

fn owner(a: &mut MailApp, method: &str, params: Value) -> Option<Value> {
    let (req, _) = ControlRequest::new(method, params);
    if let Outcome::Done(v) = crate::control::handle(a, &egui::Context::default(), &req) { Some(v) } else { None }
}

/// A message being written to somebody, which an agent can be asked to send.
fn write(a: &mut MailApp) {
    a.execute("mail.new", json!({"to": "someone@example.org", "subject": SUBJECT})).unwrap();
}

fn decided(a: &MailApp) -> Vec<Decided> {
    a.asking.activity.entries().map(|e| e.decided).collect()
}

/// Everything the Agents panel could show of an entry, and everything a `Debug` of it prints.
fn rendered(e: &Entry) -> String {
    format!(
        "{e:?} | {} | {} | {} | {} | {}",
        AgentClient::shown(e.client.as_ref()),
        e.decided.label(),
        e.command.as_deref().unwrap_or(""),
        e.class.map(Class::as_str).unwrap_or(""),
        e.count.map(|n| n.to_string()).unwrap_or_default(),
    )
}

fn entry(n: u64) -> Entry {
    Entry {
        at_ms: n as f64,
        client: None,
        command: Some("mail.list".into()),
        class: None,
        decided: Decided::NoQuestion,
        failed: false,
        count: Some(n),
    }
}

// The ring.

#[test]
fn the_log_keeps_the_newest_two_hundred() {
    assert_eq!(MOST_ENTRIES, 200);
    let mut log = Log::default();
    assert!(log.is_empty());
    for n in 0..250u64 {
        log.record(entry(n));
        assert!(log.len() <= MOST_ENTRIES);
    }
    assert_eq!(log.len(), 200);
    let counts: Vec<u64> = log.entries().filter_map(|e| e.count).collect();
    assert_eq!(counts, (50..250).collect::<Vec<_>>(), "the oldest went first, in order");
    assert_eq!(log.last().and_then(|e| e.count), Some(249));
}

#[test]
fn the_log_holds_two_hundred_of_an_agents_requests() {
    let mut a = app();
    for _ in 0..(MOST_ENTRIES + 30) {
        let (now, _) = agent(&mut a, "engine.execute", json!({"command": "mail.list", "params": {}}), vec![]);
        assert_eq!(now.unwrap()["ok"], true);
    }
    assert_eq!(a.asking.activity.len(), MOST_ENTRIES);
}

#[test]
fn a_debug_of_the_log_prints_no_names() {
    let mut log = Log::default();
    log.record(Entry { client: client(), ..entry(1) });
    let shown = format!("{log:?}");
    assert!(!shown.contains(AGENT_NAME) && shown.contains('1'), "{shown}");
}

// Who decided: each of the gate's six ends, once.

#[test]
fn each_way_the_gate_ends_a_request_is_logged_once() {
    let mut a = app();
    // Ran: no question needed.
    let (now, _) = agent(&mut a, "engine.execute", json!({"command": "folder.select", "params": {"path": "Inbox"}}), vec![]);
    assert_eq!(now.unwrap()["ok"], true);
    assert_eq!(decided(&a), [Decided::NoQuestion]);
    // Ran: allowed in advance (`--allow send`).
    write(&mut a);
    let n = a.asking.activity.len();
    let (now, _) = agent(&mut a, "compose.send", json!({}), vec![Class::Send]);
    assert_eq!(now.unwrap()["ok"], true, "allowed in advance, no question");
    assert_eq!(decided(&a)[n..], [Decided::AllowedInAdvance]);
    assert_eq!(a.asking.activity.last().unwrap().class, Some(Class::Send));
    // You said yes: nothing is logged while the question waits, one entry when it is answered.
    write(&mut a);
    let n = a.asking.activity.len();
    let (now, rx) = agent(&mut a, "compose.send", json!({}), vec![]);
    assert!(now.is_none() && a.asking.len() == 1, "asked");
    assert_eq!(a.asking.activity.len(), n, "a waiting question isn't in the log yet");
    answer(&mut a, true);
    assert_eq!(rx.try_recv().unwrap()["ok"], true);
    assert_eq!(decided(&a)[n..], [Decided::SaidYes]);
    // Not run: you said no.
    write(&mut a);
    let n = a.asking.activity.len();
    let (_, rx) = agent(&mut a, "compose.send", json!({}), vec![]);
    answer(&mut a, false);
    assert_eq!(rx.try_recv().unwrap()["ok"], false);
    assert_eq!(decided(&a)[n..], [Decided::SaidNo]);
    // Not run: no answer in 50 s.
    let n = a.asking.activity.len();
    let (_, rx) = agent(&mut a, "compose.send", json!({}), vec![]);
    expire(&mut a, crate::now_ms() + WAIT_MS + 1.0);
    assert_eq!(rx.try_recv().unwrap()["ok"], false);
    assert_eq!(decided(&a)[n..], [Decided::NoAnswer]);
    // Refused: a command no agent may run, input without `--allow input`, the owner's own
    // commands about agents.
    let n = a.asking.activity.len();
    for (method, params) in [
        ("engine.execute", json!({"command": "sync.apply", "params": {}})),
        ("ui.click", json!({"x": 1, "y": 1})),
        ("engine.execute", json!({"command": "ui.agentAccess", "params": {"value": true}})),
        ("ui.agents", json!({})),
    ] {
        let (now, _) = agent(&mut a, method, params, vec![]);
        assert_eq!(now.unwrap()["ok"], false, "{method}");
    }
    assert_eq!(decided(&a)[n..], [Decided::Refused; 4]);
    // Every one of the six, and each entry says who it was from.
    for d in Decided::ALL {
        assert!(decided(&a).contains(&d), "{d:?}");
    }
    assert!(a.asking.activity.entries().all(|e| e.client == client()));
    assert!(
        a.asking.activity.entries().all(|e| e.decided.went_ahead() != matches!(e.decided, Decided::SaidNo | Decided::NoAnswer | Decided::Refused))
    );
}

/// The spec's example: one request runs because of `--allow send`, another because the owner
/// said yes, and the log tells them apart.
#[test]
fn allowed_in_advance_and_you_said_yes_are_told_apart() {
    let mut a = app();
    write(&mut a);
    let _ = agent(&mut a, "compose.send", json!({}), vec![Class::Send]);
    write(&mut a);
    let _ = agent(&mut a, "compose.send", json!({}), vec![]);
    answer(&mut a, true);
    let labels: Vec<&str> = a.asking.activity.entries().map(|e| e.decided.label()).collect();
    assert_eq!(labels, ["Ran: allowed in advance", "You said yes"]);
}

/// The question carries who the agent said it was, so a yes or a timeout names it.
#[test]
fn a_question_settled_later_names_the_agent() {
    let mut a = app();
    write(&mut a);
    let _ = agent(&mut a, "compose.send", json!({}), vec![]);
    expire(&mut a, crate::now_ms() + WAIT_MS + 1.0);
    let e = a.asking.activity.last().unwrap();
    assert_eq!((e.decided, e.client.as_ref().and_then(|c| c.name.as_deref())), (Decided::NoAnswer, Some(AGENT_NAME)));
    assert_eq!(AgentClient::shown(e.client.as_ref()), format!("reported as ‹{AGENT_NAME}›"));
}

/// Another mailbox opened before the owner answered: the question goes unanswered, as refused.
#[test]
fn questions_forgotten_with_their_mailbox_are_logged_as_refused() {
    let mut a = app();
    write(&mut a);
    let _ = agent(&mut a, "compose.send", json!({}), vec![]);
    let _ = agent(&mut a, "compose.send", json!({}), vec![]);
    forget(&mut a);
    assert_eq!(decided(&a), [Decided::Refused, Decided::Refused]);
}

/// The owner's own script is no agent: nothing of it is logged.
#[test]
fn neg_owner_scripts_are_not_logged() {
    let mut a = app();
    write(&mut a);
    assert_eq!(owner(&mut a, "engine.execute", json!({"command": "mail.list", "params": {}})).unwrap()["ok"], true);
    assert_eq!(owner(&mut a, "engine.execute", json!({"command": "compose.send"})).unwrap()["ok"], true);
    assert_eq!(owner(&mut a, "mailbox.inspect", json!({})).unwrap()["ok"], true);
    assert!(a.asking.activity.is_empty());
}

/// An agent's polls of the window's state aren't logged; what reads mail is.
#[test]
fn polls_are_quiet_and_reads_are_logged() {
    let mut a = app();
    for method in ["ui.inspect", "app.status", "engine.commands", "ui.alerts", "ui.parity"] {
        let _ = agent(&mut a, method, json!({}), vec![]);
    }
    assert!(a.asking.activity.is_empty());
    let _ = agent(&mut a, "mailbox.inspect", json!({}), vec![]);
    let e = a.asking.activity.last().unwrap();
    assert_eq!((e.command.as_deref(), e.decided), (Some("mailbox.inspect"), Decided::NoQuestion));
}

// No mail in the log (SCN-AGENT-002).

/// The words of the sample mailbox that must never be in the log: every subject, address and
/// name, and the first line of every body.
fn fixture_text(a: &MailApp) -> Vec<String> {
    let mut words = Vec::new();
    for m in a.session.mailbox.messages.values() {
        words.push(m.subject.clone());
        for who in std::iter::once(&m.from).chain(&m.to).chain(&m.cc) {
            words.push(who.email.clone());
            words.push(who.name.clone());
        }
        words.push(m.body.text.lines().map(str::trim).find(|l| l.chars().count() >= 12).unwrap_or("").to_string());
    }
    words.retain(|w| w.trim().chars().count() >= 6);
    words.sort_unstable();
    words.dedup();
    words
}

#[test]
fn the_log_never_holds_message_text() {
    let mut a = app();
    let first = a.session.mailbox.messages.values().find(|m| !m.draft).unwrap().clone();
    let query = first.subject.split_whitespace().max_by_key(|w| w.len()).unwrap().to_string();
    let replies = [
        agent(&mut a, "engine.execute", json!({"command": "mail.list", "params": {}}), vec![]).0,
        agent(&mut a, "engine.execute", json!({"command": "view.search", "params": {"query": query}}), vec![]).0,
        agent(&mut a, "engine.execute", json!({"command": "mail.get", "params": {"id": first.id}}), vec![]).0,
        agent(&mut a, "mail.get", json!({"id": first.id}), vec![]).0,
        agent(&mut a, "mailbox.inspect", json!({}), vec![]).0,
        // A command that doesn't exist, named with mail's own words: kept as nothing.
        agent(&mut a, "engine.execute", json!({"command": first.subject, "params": {"q": first.from.email}}), vec![]).0,
        agent(&mut a, &first.from.email, json!({"query": query}), vec![]).0,
    ];
    // The agent did get the mail: the test reads what it must not leak.
    assert!(replies.iter().flatten().any(|r| r.to_string().contains(&first.subject)), "the agent read the message");
    assert!(a.asking.activity.len() >= 6);
    assert!(a.asking.activity.entries().any(|e| e.count.is_some()), "counts are kept");
    assert!(a.asking.activity.entries().any(|e| e.command.is_none()), "unknown names are dropped");
    let mut words = fixture_text(&a);
    words.push(query);
    assert!(words.len() > 10, "the fixture has words to look for");
    for e in a.asking.activity.entries() {
        let shown = rendered(e);
        for w in &words {
            assert!(!shown.contains(w.as_str()), "the log holds {w:?}: {shown}");
        }
    }
}

// Agents can't read the log.

#[test]
fn no_reply_to_an_agent_carries_the_log() {
    let mut a = app();
    write(&mut a);
    let _ = agent(&mut a, "engine.execute", json!({"command": "mail.list", "params": {}}), vec![]);
    let _ = agent(&mut a, "compose.send", json!({}), vec![Class::Send]);
    let _ = agent(&mut a, "engine.execute", json!({"command": "sync.apply", "params": {}}), vec![]);
    assert!(a.asking.activity.len() >= 3);
    // (Not the one word "Refused", which a command's own label may hold.)
    let labels: Vec<&str> = Decided::ALL.iter().filter(|d| **d != Decided::Refused).map(|d| d.label()).collect();
    for (method, params) in [
        ("ui.inspect", json!({})),
        ("engine.commands", json!({})),
        ("app.status", json!({})),
        ("ui.alerts", json!({})),
        ("mailbox.inspect", json!({})),
        ("ui.agents", json!({})),
        ("ui.agentAccess", json!({"value": true})),
        ("engine.execute", json!({"command": "ui.agents"})),
        ("engine.execute", json!({"command": "ui.agentAccess", "params": {"value": true}})),
    ] {
        let (reply, _) = agent(&mut a, method, params, Class::ALL.to_vec());
        let reply = reply.map(|r| r.to_string()).unwrap_or_default();
        assert!(!reply.contains(AGENT_NAME) && !reply.contains("reported as"), "{method}: {reply}");
        for l in &labels {
            assert!(!reply.contains(l), "{method} says {l:?}");
        }
        for key in ["activity", "agentLog", "\"decided\""] {
            assert!(!reply.contains(key), "{method}: {key}");
        }
    }
    // `ui.agents` and `ui.agentAccess` are refused to an agent, whatever it was allowed.
    for id in ["ui.agents", "ui.agentAccess"] {
        let (reply, _) = agent(&mut a, "engine.execute", json!({"command": id, "params": {"value": true}}), Class::ALL.to_vec());
        assert_eq!(reply.unwrap()["ok"], false, "{id}");
    }
    // The question in `ui.inspect` names the command and never who asked.
    write(&mut a);
    let _ = agent(&mut a, "compose.send", json!({}), vec![]);
    let inspected = owner(&mut a, "ui.inspect", json!({})).unwrap().to_string();
    assert!(inspected.contains("agentQuestion") && !inspected.contains(AGENT_NAME), "{inspected}");
}

/// No MCP tool returns the log: the bridge has no tool for the window's commands about agents,
/// and every tool it has is one of the control methods above.
#[test]
fn no_mcp_tool_returns_the_log() {
    for source in [include_str!("../../mcp/src/server.rs"), include_str!("../../mcp/src/backend.rs"), include_str!("../../mcp/src/lib.rs")] {
        for word in ["ui.agents", "ui.agentAccess", "agent_log", ".activity"] {
            assert!(!source.contains(word), "the MCP server names {word}");
        }
    }
}

// What an entry keeps.

#[test]
fn only_known_command_ids_are_kept() {
    for id in ["mail.list", "compose.send", "ui.agents", "ui.agentAccess", "ui.click", "mailbox.inspect", "sync.apply"] {
        assert_eq!(known_command(id).as_deref(), Some(id));
    }
    for id in ["", "Re: Invoice", "ada@example.org", "mail.list ", "mail.list\u{202e}", &"x".repeat(10_000)] {
        assert_eq!(known_command(id), None, "{id:?}");
    }
}

#[test]
fn counts_are_numbers_only() {
    assert_eq!(count_of(&json!([1, 2, 3])), Some(3));
    assert_eq!(count_of(&json!({"messages": [{"subject": "x"}], "rows": [1, 2]})), Some(1));
    assert_eq!(count_of(&json!({"rows": [1, 2]})), Some(2));
    assert_eq!(count_of(&json!({"count": 7})), Some(7));
    assert_eq!(count_of(&json!({"total": 9, "subject": "x"})), Some(9));
    for v in [json!(null), json!("seven"), json!({"count": -1}), json!({"count": "7"}), json!({"subject": "x"}), json!(1e300)] {
        assert_eq!(count_of(&v), None, "{v}");
    }
}

/// Hostile answers and names never panic the log: deep, huge and odd values.
#[test]
fn hostile_answers_and_names_never_panic() {
    let mut a = app();
    let deep = (0..64).fold(json!([]), |v, _| json!([v]));
    for v in [deep, json!({"messages": "x".repeat(100_000)}), json!({"count": u64::MAX}), json!({"ids": (0..10_000).collect::<Vec<_>>()})] {
        let _ = count_of(&v);
        super::note(&mut a, client().as_ref(), "\u{0}\r\n", None, Decided::NoQuestion, &Outcome::Done(json!({"ok": true, "result": v})));
    }
    super::note(&mut a, None, "mail.list", None, Decided::NoQuestion, &Outcome::Pending);
    assert_eq!(a.asking.activity.len(), 4, "a pending answer isn't logged");
    assert!(a.asking.activity.entries().all(|e| e.command.is_none()));
    let name = "\u{202e}\n".repeat(5_000);
    let (req, _) = ControlRequest::new("engine.execute", json!({"command": "mail.list"}));
    let hostile = crate::control::agent_client(&json!({"client": {"name": name}}));
    let out = crate::control::handle(&mut a, &egui::Context::default(), &req.from_agent(vec![]).with_client(hostile));
    assert!(matches!(out, Outcome::Done(_)));
}
