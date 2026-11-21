// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The agent's question as it is before the Agents panel (agent-console 8.1, SCN-AGENT-003).
//!
//! These tests hold the modal "An Agent Is Asking" to what it does today: a yes bound to what
//! was shown, a 50-second wait that ends in a no, No as the default button, Enter and Escape
//! answering no, no input from the control channel while it is open, at most eight questions in
//! the order they were asked, the question in `ui.inspect` as `agentQuestion`, and the six ways
//! an agent's request ends, each with what the agent is told. The client name, the activity log
//! and the panel come later and must leave every one of them green; a test here is never edited
//! to make a later part pass. (The yes's binding command by command is in
//! `agent_gate_binding_tests.rs`, and Escape and Enter over menus, dialogs and File in
//! `on_top_tests.rs`: not repeated here.)

use std::sync::mpsc::Receiver;

use egui::Key;
use egui_kittest::kittest::Queryable;
use mailcraft_engine::Session;
use mailcraft_engine::agent::Class;
use mailcraft_model::FolderKind;
use serde_json::{Value, json};

use super::{MOST, WAIT_MS, answer, expire};
use crate::MailApp;
use crate::control::{ControlRequest, Outcome};
use crate::people_view::testing::{self, Window};

const NOW: i64 = 1_791_993_600;
const SUBJECT: &str = "Re: Invoice";

/// The sample mailbox, no window.
fn app() -> MailApp {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), crate::Services::default());
    a.autosave = false;
    a.session.driver.set_auto_sync(false);
    a
}

/// A headless window on the sample mailbox, with nothing else open.
fn window() -> Window {
    let mut h = testing::window("view.mail");
    let _ = h.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    h.run_steps(3);
    h
}

/// `method` sent by an agent allowed `allow` in advance: its answer when it has one now, and
/// where the later answer comes.
fn agent(a: &mut MailApp, method: &str, params: Value, allow: Vec<Class>) -> (Option<Value>, Receiver<Value>) {
    let (req, rx) = ControlRequest::new(method, params);
    let out = crate::control::handle(a, &egui::Context::default(), &req.from_agent(allow));
    (if let Outcome::Done(v) = out { Some(v) } else { None }, rx)
}

/// The same from the owner's own script (no `agent` field).
fn owner(a: &mut MailApp, ctx: &egui::Context, method: &str, params: Value) -> Option<Value> {
    let (req, _) = ControlRequest::new(method, params);
    if let Outcome::Done(v) = crate::control::handle(a, ctx, &req) { Some(v) } else { None }
}

/// A message being written to somebody, with an attachment, that an agent can be asked to send.
fn write(a: &mut MailApp) {
    a.execute("mail.new", json!({"to": "x@attacker.example", "subject": SUBJECT})).unwrap();
    a.execute("compose.attach", json!({"name": "notes.txt", "data": "AAAA"})).unwrap();
}

/// The messages called [`SUBJECT`] in the Outbox or Sent Items.
fn sent(a: &MailApp) -> usize {
    let out: Vec<u64> = a.session.mailbox.folders.iter().filter(|f| matches!(f.kind, FolderKind::Outbox | FolderKind::Sent)).map(|f| f.id).collect();
    a.session.mailbox.messages.values().filter(|m| out.contains(&m.folder) && m.subject == SUBJECT).count()
}

fn outbox(a: &MailApp) -> u64 {
    a.session.mailbox.folders.iter().find(|f| f.kind == FolderKind::Outbox).unwrap().id
}

/// The keys of an answer an agent gets: it is told whether it ran and why, and nothing else.
fn keys(v: &Value) -> Vec<&str> {
    let mut k: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    k.sort_unstable();
    k
}

fn refusal(v: &Value) -> &str {
    assert_eq!(keys(v), ["error", "ok"], "{v}");
    assert_eq!(v["ok"], false, "{v}");
    v["error"].as_str().unwrap()
}

/// An agent asks to send the message being written, and the question is up.
fn agent_asks_to_send(h: &mut Window) -> Receiver<Value> {
    let (req, rx) = ControlRequest::new("engine.execute", json!({"command": "compose.send"}));
    let ctx = h.ctx.clone();
    let out = crate::control::handle(h.state_mut(), &ctx, &req.from_agent(vec![]));
    assert!(matches!(out, Outcome::Pending), "asked first");
    h.run_steps(3);
    rx
}

fn inspect(h: &mut Window) -> Value {
    let ctx = h.ctx.clone();
    owner(h.state_mut(), &ctx, "ui.inspect", json!({})).unwrap()["result"].clone()
}

// The numbers.

/// A question waits 50 seconds for the owner (the control channel gives up at 60), and at most
/// eight wait at once.
#[test]
fn the_wait_is_fifty_seconds_and_at_most_eight_questions_wait() {
    assert_eq!(WAIT_MS, 50_000.0);
    assert_eq!(MOST, 8);
    let mut a = app();
    write(&mut a);
    let before = crate::now_ms();
    let _ = agent(&mut a, "compose.send", json!({}), vec![]);
    let deadline = a.asking.front().unwrap().deadline;
    assert!(deadline >= before + WAIT_MS && deadline <= crate::now_ms() + WAIT_MS, "the wait starts when the agent asks");
}

// The 50-second timeout answers no.

/// Just short of 50 seconds the question is still up and the agent has no answer; at 50 seconds
/// it is a no, nothing is sent, and the agent is told nobody answered in time.
#[test]
fn neg_nobody_answering_for_fifty_seconds_is_a_no_and_nothing_runs() {
    let mut a = app();
    write(&mut a);
    let (now, rx) = agent(&mut a, "compose.send", json!({}), vec![]);
    assert!(now.is_none());
    let deadline = a.asking.front().unwrap().deadline;
    expire(&mut a, deadline - 1.0);
    assert_eq!(a.asking.len(), 1, "still waiting");
    assert!(rx.try_recv().is_err(), "no answer yet");
    expire(&mut a, deadline);
    assert!(a.asking.is_empty());
    let v = rx.try_recv().unwrap();
    assert_eq!(refusal(&v), "the owner didn't answer about `compose.send` in time: nothing was done");
    assert_eq!(sent(&a), 0, "nothing sent");
    assert!(a.session.compose.is_some(), "the message stays open");
}

/// Time running out answers the questions whose time is up, the first first, and leaves a later
/// one on screen.
#[test]
fn time_running_out_answers_only_the_questions_whose_time_is_up() {
    let mut a = app();
    let ids: Vec<u64> = a.session.mailbox.messages.keys().copied().take(2).collect();
    let folder = outbox(&a);
    let (_, first) = agent(&mut a, "mail.copy", json!({"folder": folder, "ids": [ids[0]]}), vec![]);
    let (_, second) = agent(&mut a, "mail.copy", json!({"folder": folder, "ids": [ids[1]]}), vec![]);
    assert_eq!(a.asking.len(), 2);
    let deadline = a.asking[0].deadline;
    a.asking[1].deadline = deadline + 10_000.0;
    expire(&mut a, deadline);
    assert!(refusal(&first.try_recv().unwrap()).contains("in time"));
    assert!(second.try_recv().is_err(), "the later question waits");
    assert_eq!(a.asking.len(), 1);
    assert_eq!(a.session.mailbox.count(folder), 0);
}

// The six ways an agent's request ends, and what the agent is told.

/// Ran, no question needed: answered at once with the command's result, and nobody is asked.
#[test]
fn a_command_that_needs_nobody_runs_at_once_and_the_agent_gets_its_result() {
    let mut a = app();
    let id = *a.session.mailbox.messages.keys().next().unwrap();
    let (now, rx) = agent(&mut a, "engine.execute", json!({"command": "mail.flag", "params": {"id": id}}), vec![]);
    let v = now.expect("answered at once");
    assert_eq!(keys(&v), ["ok", "result"], "{v}");
    assert_eq!(v["ok"], true);
    assert!(a.asking.is_empty(), "nobody asked");
    assert!(rx.try_recv().is_err(), "no second answer");
}

/// Ran, allowed in advance (`--allow send`): no question, answered at once, and it is done.
#[test]
fn a_command_allowed_in_advance_runs_at_once_without_a_question() {
    let mut a = app();
    write(&mut a);
    let (now, _) = agent(&mut a, "engine.execute", json!({"command": "compose.send"}), vec![Class::Send]);
    let v = now.expect("answered at once");
    assert_eq!(keys(&v), ["ok", "result"], "{v}");
    assert_eq!(v["ok"], true);
    assert!(a.asking.is_empty(), "nobody asked");
    assert_eq!(sent(&a), 1);
}

/// Ran, the owner said yes: the agent waits with no answer until the owner says yes, then gets
/// the command's result, and it is done once.
#[test]
fn a_command_the_owner_says_yes_to_runs_then_and_the_agent_gets_its_result() {
    let mut a = app();
    write(&mut a);
    let (now, rx) = agent(&mut a, "engine.execute", json!({"command": "compose.send"}), vec![]);
    assert!(now.is_none(), "waits for the owner");
    assert!(rx.try_recv().is_err());
    assert_eq!(sent(&a), 0, "nothing sent before the yes");
    answer(&mut a, true);
    let v = rx.try_recv().unwrap();
    assert_eq!(keys(&v), ["ok", "result"], "{v}");
    assert_eq!(v["ok"], true);
    assert_eq!(sent(&a), 1);
    assert!(a.asking.is_empty());
}

/// Not run, the owner said no: the agent is told so in plain words, and nothing is done.
#[test]
fn a_command_the_owner_says_no_to_doesnt_run_and_the_agent_is_told() {
    let mut a = app();
    write(&mut a);
    let (_, rx) = agent(&mut a, "engine.execute", json!({"command": "compose.send"}), vec![]);
    answer(&mut a, false);
    assert_eq!(refusal(&rx.try_recv().unwrap()), "the owner said no to `compose.send`: nothing was done");
    assert_eq!(sent(&a), 0);
    assert!(a.session.compose.is_some(), "the message stays open");
}

/// Refused: a command that is never for agents, clicking without `--allow input`, a method of
/// the window that isn't for agents and a screenshot into a file are answered at once with why,
/// and nobody is asked. Allowing every class doesn't change that for what is never for agents.
#[test]
fn what_is_refused_is_answered_at_once_and_asks_nobody() {
    let mut a = app();
    write(&mut a);
    let all = Class::ALL.to_vec();
    let (v, _) = agent(&mut a, "engine.execute", json!({"command": "sync.apply", "params": {}}), all.clone());
    assert_eq!(refusal(&v.unwrap()), "`sync.apply` isn't available to agents");
    let (v, _) = agent(&mut a, "app.quit", json!({}), all.clone());
    assert_eq!(refusal(&v.unwrap()), "`app.quit` isn't available to agents");
    let (v, _) = agent(&mut a, "ui.screenshot", json!({"path": "/tmp/x.png"}), all);
    assert!(refusal(&v.unwrap()).contains("agents don't name files"));
    let (v, _) = agent(&mut a, "ui.click", json!({"x": 1, "y": 1}), vec![]);
    assert!(refusal(&v.unwrap()).contains("--allow input"));
    assert!(a.asking.is_empty(), "nobody asked");
    assert!(a.synthetic.is_empty(), "no input reached the window");
    assert!(!a.quit_requested);
}

// The queue: at most eight, in the order asked.

/// Eight questions wait; a ninth is refused at once, nothing of it is done, and the eight are
/// still waiting.
#[test]
fn a_ninth_question_is_refused_and_the_eight_keep_waiting() {
    let mut a = app();
    let folder = outbox(&a);
    let ids: Vec<u64> = a.session.mailbox.messages.keys().copied().take(9).collect();
    assert_eq!(ids.len(), 9, "the sample has nine messages to ask about");
    let mut waiting = Vec::new();
    for m in &ids[..8] {
        let (now, rx) = agent(&mut a, "mail.copy", json!({"folder": folder, "ids": [m]}), vec![]);
        assert!(now.is_none(), "message {m} asked about");
        waiting.push(rx);
    }
    assert_eq!(a.asking.len(), 8);
    let (now, _) = agent(&mut a, "mail.copy", json!({"folder": folder, "ids": [ids[8]]}), vec![]);
    assert_eq!(refusal(&now.unwrap()), "too many questions are waiting for the owner; nothing was done");
    assert_eq!(a.asking.len(), 8);
    assert!(waiting.iter().all(|rx| rx.try_recv().is_err()), "the eight are still waiting");
    assert_eq!(a.session.mailbox.count(folder), 0, "nothing copied");
}

/// The owner answers the questions in the order the agents asked them: each answer goes to the
/// first one waiting, and only to it.
#[test]
fn questions_are_answered_in_the_order_they_were_asked() {
    let mut a = app();
    let folder = outbox(&a);
    let ids: Vec<u64> = a.session.mailbox.messages.keys().copied().take(3).collect();
    let rxs: Vec<Receiver<Value>> = ids.iter().map(|m| agent(&mut a, "mail.copy", json!({"folder": folder, "ids": [m]}), vec![]).1).collect();
    for (i, m) in ids.iter().enumerate() {
        assert_eq!(a.asking.front().unwrap().params["ids"], json!([m]), "question {i} is on screen");
        answer(&mut a, false);
        assert!(rxs[i].try_recv().is_ok(), "question {i} answered");
        assert!(rxs[i + 1..].iter().all(|rx| rx.try_recv().is_err()), "the later ones wait");
    }
    assert!(a.asking.is_empty());
}

// The window: default No, Enter and Escape, the bound yes, input suppressed, `ui.inspect`.

/// The question's two buttons say what they do, and No is the default: the button in the
/// default's place, at the right, which Enter presses.
#[test]
fn no_is_the_default_button() {
    let mut h = window();
    write(h.state_mut());
    let _rx = agent_asks_to_send(&mut h);
    assert!(h.query_by_label("An Agent Is Asking").is_some());
    assert!(h.query_by_role_and_label(egui::accesskit::Role::Button, "Don't Send").is_some());
    // (The message being written has a Send of its own.)
    assert!(h.query_all_by_role_and_label(egui::accesskit::Role::Button, "Send").count() >= 1);
    let (no, yes) = (h.state().hit.rect("agentNo", 0).unwrap(), h.state().hit.rect("agentYes", 0).unwrap());
    assert!(no.min.x >= yes.max.x, "No is at the right: {no:?} {yes:?}");
    testing::key(&mut h, Key::Enter);
    assert_eq!(sent(h.state()), 0);
}

/// Enter answers no: the agent is told the owner said no, and nothing is sent.
#[test]
fn enter_answers_no_and_nothing_runs() {
    let mut h = window();
    write(h.state_mut());
    let rx = agent_asks_to_send(&mut h);
    testing::key(&mut h, Key::Enter);
    assert_eq!(refusal(&rx.try_recv().unwrap()), "the owner said no to `compose.send`: nothing was done");
    assert!(h.state().asking.is_empty());
    assert_eq!(sent(h.state()), 0);
}

/// Escape answers no: the agent is told the owner said no, and nothing is sent.
#[test]
fn escape_answers_no_and_nothing_runs() {
    let mut h = window();
    write(h.state_mut());
    let rx = agent_asks_to_send(&mut h);
    testing::key(&mut h, Key::Escape);
    assert_eq!(refusal(&rx.try_recv().unwrap()), "the owner said no to `compose.send`: nothing was done");
    assert!(h.state().asking.is_empty());
    assert_eq!(sent(h.state()), 0);
}

/// With two questions waiting, the window says one more is waiting, and Escape answers only the
/// one on screen: the next comes up and waits for its own answer.
#[test]
fn escape_answers_only_the_question_on_screen() {
    let mut h = window();
    let folder = outbox(h.state());
    let ids: Vec<u64> = h.state().session.mailbox.messages.keys().copied().take(2).collect();
    let first = agent(h.state_mut(), "mail.copy", json!({"folder": folder, "ids": [ids[0]]}), vec![]).1;
    let second = agent(h.state_mut(), "mail.copy", json!({"folder": folder, "ids": [ids[1]]}), vec![]).1;
    h.run_steps(3);
    assert!(h.query_by_label("1 more question is waiting.").is_some());
    testing::key(&mut h, Key::Escape);
    assert!(refusal(&first.try_recv().unwrap()).contains("said no"));
    assert!(second.try_recv().is_err(), "the next question waits");
    assert_eq!(h.state().asking.len(), 1);
    assert!(h.query_by_label("An Agent Is Asking").is_some(), "and is on screen");
    assert!(h.query_by_label("1 more question is waiting.").is_none());
    testing::key(&mut h, Key::Escape);
    assert!(refusal(&second.try_recv().unwrap()).contains("said no"));
    assert_eq!(h.state().session.mailbox.count(folder), 0);
}

/// The owner's Send on the question sends the message the question showed.
#[test]
fn the_owners_yes_on_the_question_sends_what_it_showed() {
    let mut h = window();
    write(h.state_mut());
    let rx = agent_asks_to_send(&mut h);
    let at = h.state().hit.rect("agentYes", 0).unwrap().center();
    testing::click(&mut h, at, egui::PointerButton::Primary, 1);
    assert_eq!(rx.try_recv().unwrap()["ok"], true);
    assert_eq!(sent(h.state()), 1);
    assert!(h.state().asking.is_empty());
}

/// The yes is bound to what was shown: an agent changes the recipient of its message while the
/// owner is asked, and the owner's Send then sends nothing, the agent is told, and the window
/// says why.
#[test]
fn a_yes_to_a_message_changed_since_sends_nothing() {
    let mut h = window();
    let a = h.state_mut();
    assert_eq!(agent(a, "mail.new", json!({"to": "alice@example.org", "subject": SUBJECT}), vec![]).0.unwrap()["ok"], true);
    let rx = agent_asks_to_send(&mut h);
    let changed = agent(h.state_mut(), "compose.set", json!({"to": "mallory@example.org"}), vec![]).0.unwrap();
    assert_eq!(changed["ok"], true, "other agent commands still run: {changed}");
    h.run_steps(3);
    let at = h.state().hit.rect("agentYes", 0).unwrap().center();
    testing::click(&mut h, at, egui::PointerButton::Primary, 1);
    let v = rx.try_recv().unwrap();
    assert!(refusal(&v).contains("nothing was done"), "{v}");
    assert_eq!(sent(h.state()), 0, "nothing sent");
    let status = h.state().status_msg.as_ref().map(|(m, _)| m.clone());
    assert_eq!(status.as_deref(), Some(tl!("Nothing was done: what the agent asked about changed after you were asked.")));
}

/// While the question is up no input from the control channel reaches the window: what the
/// owner's own script queued is dropped at the next frame, so a scripted Enter doesn't answer
/// the question, and an agent allowed input is refused.
#[test]
fn no_control_channel_input_reaches_the_window_while_the_owner_is_asked() {
    let mut h = window();
    write(h.state_mut());
    let rx = agent_asks_to_send(&mut h);
    let ctx = h.ctx.clone();
    assert_eq!(owner(h.state_mut(), &ctx, "ui.key", json!({"key": "Enter"})).unwrap()["ok"], true);
    assert_eq!(owner(h.state_mut(), &ctx, "ui.click", json!({"x": 10, "y": 10})).unwrap()["ok"], true);
    assert!(!h.state().synthetic.is_empty(), "queued");
    h.run_steps(1);
    assert!(h.state().synthetic.is_empty(), "dropped");
    h.run_steps(3);
    assert_eq!(h.state().asking.len(), 1, "the question still waits");
    assert!(rx.try_recv().is_err(), "unanswered");
    let (v, _) = agent(h.state_mut(), "ui.key", json!({"key": "Enter"}), vec![Class::Input]);
    assert_eq!(refusal(&v.unwrap()), "the owner is being asked something: no input reaches the window until they answer");
    assert!(h.state().synthetic.is_empty());
    testing::key(&mut h, Key::Escape);
    assert!(refusal(&rx.try_recv().unwrap()).contains("said no"));
}

/// `ui.inspect` names the question on screen as `agentQuestion`: the command, the class it
/// needs and what it would do, and nothing more; null with no question. An agent's own
/// `ui.inspect` sees the same.
#[test]
fn the_question_on_screen_is_agent_question_in_ui_inspect() {
    let mut h = window();
    assert_eq!(inspect(&mut h)["agentQuestion"], Value::Null);
    write(h.state_mut());
    let rx = agent_asks_to_send(&mut h);
    let folder = outbox(h.state());
    let m = *h.state().session.mailbox.messages.keys().next().unwrap();
    let _second = agent(h.state_mut(), "mail.copy", json!({"folder": folder, "ids": [m]}), vec![]).1;
    h.run_steps(2);
    let what = h.state().asking.front().unwrap().what.clone();
    let q = inspect(&mut h)["agentQuestion"].clone();
    assert_eq!(keys(&q), ["command", "needs", "what"], "{q}");
    assert_eq!(q, json!({"command": "compose.send", "needs": "send", "what": what}));
    let (seen, _) = agent(h.state_mut(), "ui.inspect", json!({}), vec![]);
    assert_eq!(seen.unwrap()["result"]["agentQuestion"], q, "the agent sees the same");
    testing::key(&mut h, Key::Escape);
    assert!(rx.try_recv().is_ok());
    let next = inspect(&mut h)["agentQuestion"].clone();
    assert_eq!((next["command"].as_str(), next["needs"].as_str()), (Some("mail.copy"), Some("send")), "the next question");
    testing::key(&mut h, Key::Escape);
    assert_eq!(inspect(&mut h)["agentQuestion"], Value::Null);
}
