// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What an agent may do through the window (`mailcraft-cli mcp --connect`): the bridge marks
//! every request it sends as an agent's (`ControlRequest::agent`, with what the owner allowed it
//! in advance), and those go through [`mailcraft_engine::agent`]'s gate here. A command that is
//! not for agents, or names a file, is refused; one that needs the owner is asked about in a
//! question over the window ("An agent wants to send …"), whose default answer is no. It runs
//! only on the owner's yes; nobody's answer within [`WAIT_MS`] is a no. While a question is up,
//! no input from the control channel reaches the window. Other agent commands still run, and
//! the yes reaches only what the question showed ([`agent::bind`], kept in the question's own
//! record with the windows as they were): what changed meanwhile makes it do nothing.
//!
//! Every request the gate settles goes into the owner's activity log ([`activity`]), memory only,
//! with who decided: ids and counts, never what the agent sent or got back.

use std::collections::{HashSet, VecDeque};
use std::sync::mpsc::Sender;

use egui::vec2;
use mailcraft_engine::agent::{self, Class, Verdict};
use serde_json::{Value, json};

use crate::MailApp;
use crate::control::{AgentClient, ControlRequest, Outcome};
use crate::theme::{Tokens, semibold};
use crate::widgets::primary_button;

/// How long a question waits for the owner (the control channel gives up at 60 s).
pub const WAIT_MS: f64 = 50_000.0;

/// Most questions waiting at once.
const MOST: usize = 8;

#[path = "agent_log.rs"]
pub mod activity;
use activity::Decided;

/// A question for the owner about what an agent wants.
pub struct Asking {
    pub id: String,
    pub params: Value,
    pub class: Class,
    /// What else it needs, which the question names and a yes covers ([`agent::also`], #473).
    also: Vec<Class>,
    /// The messages an undo or redo would take away, which the question names, and the steps
    /// as they were when it was asked ([`mailcraft_engine::Session::undo_steps_moved`], #473).
    taken: HashSet<u64>,
    steps: u64,
    /// The contacts, events, tasks, calendars and categories it would take away
    /// ([`mailcraft_engine::Session::undo_takes`]): a yes reaches none that came since.
    items: HashSet<String>,
    /// What it would do, after "An agent wants to".
    pub what: String,
    allow: Vec<Class>,
    reply: Sender<Value>,
    deadline: f64,
    /// The mailbox it is about (`Session::openings`): its ids are that mailbox's (#503).
    opening: u64,
    /// The messages a Delete or Move names, with the folder each was in when the owner was asked
    /// (#503).
    counted: Vec<(u64, u64)>,
    /// What the question showed, kept here and never in the agent's params: the yes reaches that
    /// alone ([`agent::bind`], AU3-24).
    shown: agent::bind::Shown,
    /// And the windows as it saw them (AU3-20).
    windows: crate::windows::Seen,
    /// Who the agent said it was, for the activity log when the question is settled; never shown
    /// in the question and never used to decide it.
    client: Option<AgentClient>,
}

/// The questions waiting, the first one on screen (the queue itself, through `Deref`), and the
/// owner's activity log of what agents did ([`activity`]), which is never shown to an agent.
#[derive(Default)]
pub struct Questions {
    waiting: VecDeque<Asking>,
    /// What agents did, for the Agents panel: memory only, never in `ui.inspect`.
    pub activity: activity::Log,
}

impl std::ops::Deref for Questions {
    type Target = VecDeque<Asking>;
    fn deref(&self) -> &Self::Target {
        &self.waiting
    }
}

impl std::ops::DerefMut for Questions {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.waiting
    }
}

/// Puts what the gate did with an agent's request in the activity log: `command` only when it
/// is one MailCraft knows, and of the answer only whether it failed and how many things it
/// named. `Pending` (a question still waiting) is put there when it is settled.
pub(crate) fn note(app: &mut MailApp, client: Option<&AgentClient>, command: &str, class: Option<Class>, decided: Decided, out: &Outcome) {
    let Outcome::Done(v) = out else { return };
    let failed = v.get("ok").and_then(Value::as_bool) != Some(true);
    let count = if failed { None } else { v.get("result").and_then(activity::count_of) };
    app.asking.activity.record(activity::Entry {
        at_ms: crate::now_ms(),
        client: client.cloned(),
        command: activity::known_command(command),
        class,
        decided,
        failed,
        count,
    });
}

/// The control methods that only look at the window's state, never at mail, and that an agent
/// sends after almost every step: not put in the activity log, where they would push out what
/// the agent did.
const QUIET: &[&str] = &["engine.commands", "ui.inspect", "ui.alerts", "ui.parity", "app.status"];

/// A request the gate lets go on as anybody's, in the log; what it answers isn't known here.
fn note_passed(app: &mut MailApp, req: &ControlRequest, class: Option<Class>, decided: Decided) {
    if !QUIET.contains(&req.method.as_str()) {
        let passed = json!({"ok": true});
        note(app, req.client.as_ref(), &req.method, class, decided, &Outcome::Done(passed));
    }
}

/// `out`, put in the activity log as `decided` about `req`'s own method.
fn noted(app: &mut MailApp, req: &ControlRequest, class: Option<Class>, decided: Decided, out: Outcome) -> Outcome {
    note(app, req.client.as_ref(), &req.method, class, decided, &out);
    out
}

fn ok(v: Value) -> Outcome {
    Outcome::Done(json!({"ok": true, "result": v}))
}

fn err(e: impl std::fmt::Display) -> Outcome {
    Outcome::Done(json!({"ok": false, "error": e.to_string()}))
}

/// A request from an agent: `Some` when the gate settles it (refused, asked about, or run
/// here), `None` when it goes on as anybody's (looking, a screenshot into the answer).
pub fn screen(app: &mut MailApp, req: &ControlRequest, allow: &[Class]) -> Option<Outcome> {
    let p = &req.params;
    // The owner stopped agent access for the session (`ui.agentAccess`, the Agents panel): every
    // agent request is refused, whatever it is and whatever it was allowed. Looking at the
    // window's state isn't put in the log, as when access is on: it would push out the rest.
    if crate::ai::agents::stopped(app) {
        let out = err(STOPPED);
        if !QUIET.contains(&req.method.as_str()) {
            let id = match req.method.as_str() {
                "engine.execute" | "command" => p.get("command").or_else(|| p.get("id")).and_then(Value::as_str).unwrap_or(""),
                other => other,
            };
            note(app, req.client.as_ref(), id, None, Decided::Refused, &out);
        }
        return Some(out);
    }
    let given = |k: &str| p.get(k).is_some_and(|v| !v.is_null());
    match req.method.as_str() {
        "engine.commands" | "mailbox.inspect" | "ui.inspect" | "ui.alerts" | "ui.parity" | "ui.resize" | "ui.focus" | "app.status" => {
            note_passed(app, req, None, Decided::NoQuestion);
            None
        }
        "ui.screenshot" if given("path") => {
            Some(noted(app, req, None, Decided::Refused, err("agents don't name files: the screenshot comes back in the answer (leave out `path`)")))
        }
        "ui.screenshot" => {
            note_passed(app, req, None, Decided::NoQuestion);
            None
        }
        "ui.click" | "ui.move" | "ui.drag" | "ui.key" | "ui.text" => {
            if !allow.contains(&Class::Input) {
                let out = err(format!(
                    "clicking and typing in the window needs the owner: {}. Only the owner can let an agent do this, by starting the bridge with `mailcraft-cli mcp --allow input`",
                    Class::Input.what()
                ));
                Some(noted(app, req, Some(Class::Input), Decided::Refused, out))
            } else if !app.asking.is_empty() || crate::ai::owner_asks(app) {
                let out = err("the owner is being asked something: no input reaches the window until they answer");
                Some(noted(app, req, Some(Class::Input), Decided::Refused, out))
            } else if on_owner_only(app, &req.method, p) {
                let out = err("that place is the owner's to answer: an agent's click doesn't reach it");
                Some(noted(app, req, Some(Class::Input), Decided::Refused, out))
            } else {
                note_passed(app, req, Some(Class::Input), Decided::AllowedInAdvance);
                None
            }
        }
        "engine.execute" | "command" => {
            let id = p.get("command").or_else(|| p.get("id")).and_then(Value::as_str).unwrap_or("").to_string();
            let params = p.get("params").cloned().filter(|v| !v.is_null()).unwrap_or(json!({}));
            Some(gate(app, req, &id, params, allow))
        }
        m if refused_to_agents(m) => Some(noted(app, req, None, Decided::Refused, err(format!("`{m}` isn't available to agents")))),
        other => Some(gate(app, req, other, p.clone(), allow)),
    }
}

/// Whether an agent's `ui.click` or `ui.drag` would land on an owner-only AI control
/// ([`crate::ai::OWNER_ONLY_HITS`]): named by its kind, or at a point inside one as last drawn.
fn on_owner_only(app: &MailApp, method: &str, p: &Value) -> bool {
    let only = crate::ai::OWNER_ONLY_HITS;
    let at = |x: &str, y: &str| {
        let f = |k: &str| p.get(k).and_then(Value::as_f64).map(|v| v as f32).filter(|v| v.is_finite());
        f(x).zip(f(y)).map(|(x, y)| egui::pos2(x, y))
    };
    match method {
        "ui.click" => {
            if p.get("kind").and_then(Value::as_str).is_some_and(|k| only.contains(&k)) {
                return true;
            }
            at("x", "y").is_some_and(|pos| app.hit.any_at(pos, only))
        }
        "ui.drag" => [at("x", "y"), at("toX", "toY")].into_iter().flatten().any(|pos| app.hit.any_at(pos, only)),
        _ => false,
    }
}

/// The control methods no agent may call, whatever it was allowed: quitting, the sign-in, hiding
/// the window, and every one of the owner's AI methods (the model providers and the runtimes,
/// [`crate::control::is_ai_method`]), which change where mail may go.
pub fn refused_to_agents(method: &str) -> bool {
    matches!(method, "app.quit" | "auth" | "ui.minimize") || crate::control::is_ai_method(method)
}

/// The gate for one command, its end put in the activity log (a question, when it is settled).
fn gate(app: &mut MailApp, req: &ControlRequest, id: &str, params: Value, allow: &[Class]) -> Outcome {
    let (out, class, decided) = decide(app, req, id, params, allow);
    if let Some(decided) = decided {
        note(app, req.client.as_ref(), id, class, decided, &out);
    }
    out
}

/// What the gate does with a command: its answer, the command's class, and who decided (`None`
/// while a question waits).
fn decide(app: &mut MailApp, req: &ControlRequest, id: &str, mut params: Value, allow: &[Class]) -> (Outcome, Option<Class>, Option<Decided>) {
    let refused = |out: Outcome, class: Option<Class>| (out, class, Some(Decided::Refused));
    // The question names the people the window would write to, as the form has them now.
    if id == "ui.eventWindow" && params.is_object() {
        let people = crate::windows::event_window_people(app, &params);
        params["attendees"] = json!(people);
    }
    let params = as_the_window_has_it(app, id, params);
    let access = agent::classify(&app.session, id, &params);
    let of = match access {
        agent::Access::Owner(c) | agent::Access::Asked(c) => Some(c),
        agent::Access::Safe | agent::Access::Never => None,
    };
    if let Some(why) = overwrites(app, id, &params) {
        return refused(err(why), of);
    }
    match agent::check(&app.session, id, &params, allow) {
        Verdict::Run => {
            // Safe needs nobody; anything else ran because its class was allowed in advance.
            let decided = if access == agent::Access::Safe { Decided::NoQuestion } else { Decided::AllowedInAdvance };
            (run(app, id, params, allow, None), of, Some(decided))
        }
        Verdict::Refuse(why) => refused(err(why), of),
        Verdict::Ask(class, what) => {
            if app.asking.len() >= MOST {
                return refused(err("too many questions are waiting for the owner; nothing was done"), Some(class));
            }
            let Some(shown) = agent::bind::Shown::new(&app.session, id, &params, &what) else {
                return refused(err(format!("`{id}` isn't available to agents: what it acts on can't be shown to the owner")), Some(class));
            };
            let counted = match counted(app, id, &params) {
                Ok(counted) => counted,
                Err(why) => return refused(err(why), Some(class)),
            };
            let also = agent::also(&app.session, id, &params, class);
            let (taken, items) = undo_takes(app, id);
            let steps = app.session.undo_steps_moved();
            let asking = Asking {
                id: id.to_string(),
                counted,
                params,
                class,
                also,
                taken,
                steps,
                items,
                what,
                allow: allow.to_vec(),
                reply: req.reply.clone(),
                deadline: crate::now_ms() + WAIT_MS,
                opening: app.session.openings(),
                shown,
                windows: crate::windows::seen(app),
                client: req.client.clone(),
            };
            app.asking.push_back(asking);
            (Outcome::Pending, Some(class), None)
        }
    }
}

/// `params` with what the window would lose (`unsaved`), as it has it now and never as the agent
/// says (#494): an appointment, a contact or a task with changes that aren't saved, in the dialog
/// that `ui.closeDialog` closes or `ui.dialog` puts another in place of, or in a window of its
/// own that `ui.closeWindow` closes, that another mailbox in view (`file.new`, `file.open`)
/// closes, or that closes with its appointment when that is taken away.
fn as_the_window_has_it(app: &MailApp, id: &str, mut params: Value) -> Value {
    let removes = matches!(id, "calendar.delete" | "calendar.removeCalendar" | "edit.undo" | "edit.redo");
    if !removes && !matches!(id, "ui.closeDialog" | "ui.dialog" | "ui.closeWindow" | "file.new" | "file.open") {
        if let Some(fields) = params.as_object_mut() {
            fields.remove("unsaved");
        }
        return params;
    }
    if !params.is_object() {
        params = json!({});
    }
    let closes = id == "ui.closeDialog" || params.get("name").and_then(Value::as_str).is_some_and(|n| crate::sync_panel::Panel::named(n).is_none());
    let unsaved = match &app.dialog {
        _ if id == "ui.closeWindow" => crate::windows::closing_unsaved(app, &params),
        _ if matches!(id, "file.new" | "file.open") => crate::windows::replacing_unsaved(app),
        _ if removes => crate::windows::removing_unsaved(app, id, &params),
        Some(crate::dialogs::Dialog::Form(form)) if closes => form.unsaved(),
        _ => None,
    };
    if let Some(fields) = params.as_object_mut() {
        fields.remove("unsaved");
        if let Some(unsaved) = unsaved {
            fields.insert("unsaved".into(), unsaved);
        }
    }
    params
}

/// Why an agent's command can't run at all while the owner is changing what it would overwrite
/// or take away (#494): what the owner typed in an appointment's window (`ui.openWindow` of
/// another time proposed, `ui.eventWindow` closing the recurrence they are filling in), or the
/// appointment, contact or task they are changing in the dialog, which another mailbox in view
/// takes away too; or the draft of the message the owner is writing, as an undo or a redo would
/// put it back. Nothing in the window can be asked about it: the form stays whatever the answer. (What takes away a mailbox or the steps to undo is refused here, before it runs:
/// `Session::run` puts back the mailbox of a command it refuses, not the rest.) A New or Open
/// that would wait for the mailbox to be saved first is refused too: it would run later, when
/// the owner may be writing.
fn overwrites(app: &MailApp, id: &str, params: &Value) -> Option<String> {
    if crate::windows::overwrites_unsaved(app, id, params) {
        return Some(format!(
            "`{id}` would overwrite what the owner typed in an appointment's window and hasn't saved; nothing was done. Try again once the owner has saved or closed it"
        ));
    }
    // The draft of the message the owner is writing, put back as it was before a later save or
    // taken away: the message would then be "in Drafts" without what was written since.
    let redo = id == "edit.redo";
    if matches!(id, "edit.undo" | "edit.redo")
        && app.session.all_composing().filter(|c| c.owner_changed).any(|c| c.draft.is_some_and(|d| app.session.undo_changes_message(redo, d)))
    {
        return Some(format!(
            "`{id}` would put back an earlier draft of the message the owner is writing, or take it away; nothing was done. Try again once the owner has closed the message"
        ));
    }
    if matches!(id, "file.new" | "file.open") && app.waits_for_save() {
        return Some(format!("`{id}` would wait for the mailbox to be saved first; nothing was done. Try again in a moment"));
    }
    let item = changing_in_the_dialog(app)?;
    (matches!(id, "file.new" | "file.open") || crate::windows::takes_away(app, id, params, item)).then(|| {
        format!(
            "`{id}` would take away what the owner is changing in the dialog and hasn't saved; nothing was done. Try again once the owner has saved or closed it"
        )
    })
}

/// The appointment, contact or task the owner is changing in the dialog, with changes that
/// aren't saved.
fn changing_in_the_dialog(app: &MailApp) -> Option<mailcraft_model::Id> {
    match &app.dialog {
        Some(crate::dialogs::Dialog::Form(form)) if form.changed() => form.editing(),
        _ => None,
    }
}

/// Run for an agent: what the command does is held to `allow` (`Session::agent`). A form in the
/// dialog with changes the owner hasn't saved stays where it is (#494): set aside while the
/// command runs, it comes back after it, and a dialog the command opened in its place isn't
/// shown. (Closing it, or another dialog in its place, was asked about: `unsaved`.) Nor does the
/// command take away an appointment the owner is changing: in that form, or in a window unless
/// the owner said yes to what the window would lose (`unsaved`).
/// `only`: of a folder's messages and folders, the command takes none but these (the owner's yes, AU3-24).
fn run(app: &mut MailApp, id: &str, params: Value, allow: &[Class], only: Option<agent::bind::Only>) -> Outcome {
    let said_yes = params.get("unsaved").is_some_and(|v| !v.is_null());
    let asked = matches!(id, "ui.closeDialog" | "ui.dialog") && said_yes;
    let owners = match &app.dialog {
        Some(crate::dialogs::Dialog::Form(form)) if form.changed() && !asked => app.dialog.take(),
        _ => None,
    };
    let mut changing = if said_yes { Vec::new() } else { crate::windows::changing_events(app) };
    // (A question names windows only, never the form set aside.)
    if let Some(crate::dialogs::Dialog::Form(form)) = &owners
        && let Some(item) = form.editing()
    {
        changing.push(item);
    }
    app.session.agent = Some(allow.to_vec());
    app.session.owner_changing = changing;
    app.session.only_shown = only;
    let r = app.execute(id, params);
    app.session.agent = None;
    app.session.owner_changing = Vec::new();
    app.session.only_shown = None;
    if let Some(form) = owners
        && let Some(opened) = app.dialog.replace(form)
    {
        return err(format!(
            "`{id}` ran, but the dialog \"{}\" it opened would have taken the place of a form the owner is filling in and hasn't saved: the form stays, and that dialog wasn't shown. Try again once the owner has closed the form",
            opened.name()
        ));
    }
    match r {
        Ok(v) => ok(v),
        Err(e) => err(e),
    }
}

/// The messages an agent's Delete or Move names (`ids` or `id`, which it must: [`agent::NEEDS_IDS`])
/// and the folder each is in, as the owner is asked about them (#503). One that isn't in the
/// mailbox is refused, never asked about: a message that comes in meanwhile, or one an undo puts
/// back, could take its id, and the yes would reach a message the owner wasn't shown.
fn counted(app: &MailApp, id: &str, params: &Value) -> Result<Vec<(u64, u64)>, String> {
    if !matches!(id, "mail.delete" | "mail.deletePermanently" | "mail.move") {
        return Ok(Vec::new());
    }
    let mb = &app.session.mailbox;
    let named = mailcraft_engine::p::ids_strict(params).ok().flatten().unwrap_or_default();
    named
        .into_iter()
        .map(|m| {
            mb.message(m).map(|x| (m, x.folder)).ok_or_else(|| format!("`{id}` names message {m}, which isn't in the mailbox: nothing was done"))
        })
        .collect()
}

/// `params` with only those of the `counted` messages that are still in the folder they were
/// counted in: one moved meanwhile (on another device, which a Send/Receive brings while the
/// question is open) or gone is left out, as the window's own question does (#503). None when none
/// is left.
fn still_where_counted(app: &MailApp, counted: &[(u64, u64)], mut params: Value) -> Option<Value> {
    if counted.is_empty() {
        return Some(params);
    }
    let mb = &app.session.mailbox;
    let kept: Vec<u64> = counted.iter().filter(|(m, f)| mb.message(*m).is_some_and(|x| x.folder == *f)).map(|(m, _)| *m).collect();
    if kept.is_empty() {
        return None;
    }
    if kept.len() < counted.len() {
        let map = params.as_object_mut()?;
        map.remove("id");
        map.insert("ids".into(), json!(kept));
    }
    Some(params)
}

/// The messages, and the contacts and the like, an agent's undo or redo would take away (none
/// for any other command), as sets: an undo of a large import is looked up in, not walked.
fn undo_takes(app: &MailApp, id: &str) -> (HashSet<u64>, HashSet<String>) {
    match id {
        "edit.undo" | "edit.redo" => {
            let (taken, items) = app.session.undo_takes(id == "edit.redo");
            (taken.into_iter().collect(), items.into_iter().collect())
        }
        _ => (HashSet::new(), HashSet::new()),
    }
}

/// The owner's answer to the question on screen.
pub fn answer(app: &mut MailApp, yes: bool) {
    let Some(a) = app.asking.pop_front() else { return };
    let out = if a.opening != app.session.openings() {
        err(format!("another mailbox was opened before the owner answered about `{}`: nothing was done", a.id))
    } else if yes {
        said_yes(app, &a)
    } else {
        err(format!("the owner said no to `{}`: nothing was done", a.id))
    };
    let decided = if yes { Decided::SaidYes } else { Decided::SaidNo };
    note(app, a.client.as_ref(), &a.id, Some(a.class), decided, &out);
    if let Outcome::Done(v) = out {
        let _ = a.reply.send(v);
    }
}

/// What the owner's yes to `a` does: it runs only when it still does what the question showed.
fn said_yes(app: &mut MailApp, a: &Asking) -> Outcome {
    // Asked again: what it would do may have changed while the question was up. So may what
    // the window would lose: another form than the one named isn't lost on this yes.
    let params = as_the_window_has_it(app, &a.id, a.params.clone());
    let Some(params) = still_where_counted(app, &a.counted, params) else {
        return err(format!("what `{}` was asked about has gone or moved since the owner was asked: nothing was done", a.id));
    };
    let lost = params.get("unsaved").filter(|u| !u.is_null());
    let other = lost.is_some() && lost != a.params.get("unsaved");
    let verdict = agent::answered(&app.session, &a.id, &params, &a.allow, a.class);
    // What else it needs now, which the yes covers only when the question named it (#473).
    let also = agent::also(&app.session, &a.id, &params, a.class);
    // Nor does it cover another step than the one asked about, or other messages, drafts,
    // contacts and the like than the question named.
    let undoing = matches!(a.id.as_str(), "edit.undo" | "edit.redo");
    let more = also.iter().any(|c| !a.also.contains(c) && !a.allow.contains(c))
        || {
            let (taken, items) = undo_takes(app, &a.id);
            !taken.is_subset(&a.taken) || !items.is_subset(&a.items)
        }
        || (undoing && (app.session.undo_steps_moved() != a.steps || agent::describe(&app.session, &a.id, &params) != a.what));
    // Nor anything but what the question showed (AU3-24): what the command would take from the
    // window, what it names, and the windows, as they were.
    let reach = if crate::windows::seen(app) != a.windows {
        Err(format!("the windows `{}` was asked about have changed since the owner was asked: nothing was done. Ask the owner again", a.id))
    } else {
        a.shown.check(&app.session, &a.id, &params)
    };
    let mut allow = a.allow.clone();
    allow.push(a.class);
    allow.extend(also);
    match verdict {
        _ if other => err(format!("what `{}` would close without saving has changed since the owner was asked; nothing was done", a.id)),
        _ if more => err(format!("`{}` would now do more than the owner was asked about; nothing was done", a.id)),
        _ if let Some(why) = overwrites(app, &a.id, &params) => err(why),
        _ if let Err(why) = &reach => {
            app.status(tl!("Nothing was done: what the agent asked about changed after you were asked."));
            err(why)
        }
        Verdict::Run => {
            let only = match reach {
                Ok(agent::bind::Reach::Only(only)) => Some(only),
                _ => None,
            };
            run(app, &a.id, params, &allow, only)
        }
        Verdict::Refuse(why) => err(why),
        Verdict::Ask(..) => err(format!("`{}` would now do more than the owner was asked about; nothing was done", a.id)),
    }
}

/// Another mailbox took the place of the one asked about: the questions go with it, unanswered,
/// since what they name are that mailbox's messages (#503).
pub fn forget(app: &mut MailApp) {
    let gone: Vec<Asking> = app.asking.drain(..).collect();
    for a in gone {
        let v = json!({"ok": false, "error": format!("another mailbox was opened before the owner answered about `{}`: nothing was done", a.id)});
        note(app, a.client.as_ref(), &a.id, Some(a.class), Decided::Refused, &Outcome::Done(v.clone()));
        let _ = a.reply.send(v);
    }
}

/// Why an agent's request is refused while the owner has stopped agent access.
pub(crate) const STOPPED: &str =
    "the owner stopped agent access for this session: nothing was done. Only the owner can let agents in again, in the window's Agents panel";

/// The owner stopped agent access (`ui.agentAccess`): the questions waiting are answered no, each
/// put in the activity log as refused, as [`forget`] does.
pub(crate) fn refuse_waiting(app: &mut MailApp) {
    while let Some(a) = app.asking.pop_front() {
        let v = json!({"ok": false, "error": format!("the owner stopped agent access before answering about `{}`: nothing was done", a.id)});
        note(app, a.client.as_ref(), &a.id, Some(a.class), Decided::Refused, &Outcome::Done(v.clone()));
        let _ = a.reply.send(v);
    }
}

/// Questions nobody answered in time are a no.
pub fn expire(app: &mut MailApp, now: f64) {
    while app.asking.front().is_some_and(|a| now >= a.deadline) {
        if let Some(a) = app.asking.pop_front() {
            let v = json!({"ok": false, "error": format!("the owner didn't answer about `{}` in time: nothing was done", a.id)});
            note(app, a.client.as_ref(), &a.id, Some(a.class), Decided::NoAnswer, &Outcome::Done(v.clone()));
            let _ = a.reply.send(v);
        }
    }
}

/// The buttons: going ahead, and not (the default). A rule made or switched on asks as a delete
/// does, and answering it deletes nothing yet: Delete would read as getting rid of that rule
/// (#566). One made from a message does: it runs at once on the mail from that sender.
pub(crate) fn verbs(class: Class, id: &str) -> (&'static str, &'static str) {
    match class {
        Class::Send => (tl!("Send"), tl!("Don't Send")),
        Class::Delete if matches!(id, "rules.add" | "rules.keepOnServer" | "rules.enable" | "rules.move") => (tl!("Allow"), tl!("Don't Allow")),
        Class::Delete => (tl!("Delete"), tl!("Don't Delete")),
        _ => (tl!("Allow"), tl!("Don't Allow")),
    }
}

/// The question's layer: a modal one, over everything.
fn layer() -> egui::LayerId {
    crate::keys::modal_layer(egui::Id::unique("agent-asks"))
}

/// Enter or Escape answers no to the agent's question. It is on top of everything, so nothing
/// under the question gets the same key, its repeats included (#393, #406). Only a new press
/// answers, once the question was on the screen: not a key that was already held down when it
/// came up, nor one pressed before it was drawn.
pub fn keys(app: &mut MailApp, ctx: &egui::Context) {
    if app.asking.is_empty() {
        return;
    }
    // (Not a button of the question either: the next question's would be pressed with it.)
    let (enter, escape) = (crate::keys::take_enter_out(ctx, layer()), crate::keys::take_escape(ctx, layer()));
    if enter || escape {
        answer(app, false);
        // The frame's other keys were pressed with the question on screen: none reaches what is
        // under it (Delete with the Escape that says no; #503).
        ctx.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Key { .. })));
    }
}

/// The question over the window, when there is one. Enter and Escape say no ([`keys`]).
pub fn show(app: &mut MailApp, ctx: &egui::Context) {
    expire(app, crate::now_ms());
    let Some(a) = app.asking.front() else { return };
    // Nothing from the control channel reaches the window while the owner is asked.
    app.synthetic.clear();
    let t = Tokens::get(ctx);
    let what: String = a.what.chars().filter(|c| !c.is_control()).take(agent::MAX_QUESTION).collect();
    let text = crate::i18n::fmt(tl!("An agent wants to {what}."), &[("what", what.trim())]);
    let (yes_label, no_label) = verbs(a.class, &a.id);
    let waiting = app.asking.len();
    let mut said = None;
    let frame = egui::Frame::window(&ctx.global_style()).fill(t.surface).inner_margin(egui::Margin::same(16));
    let width = (ctx.content_rect().width() - 48.0).clamp(160.0, 460.0);
    egui::Modal::new(layer().id).frame(frame).backdrop_color(t.scrim).show(ctx, |ui| {
        ui.set_width(width);
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        ui.label(egui::RichText::new(tl!("An Agent Is Asking")).font(semibold(14.0)).color(t.text));
        ui.label(egui::RichText::new(text).color(t.text));
        ui.label(
            egui::RichText::new(tl!(
                "Say yes only if you asked the agent for this. Text in a message can tell an agent to do things you didn't ask for."
            ))
            .color(t.text_dim),
        );
        if waiting > 1 {
            ui.label(egui::RichText::new(tln!(waiting - 1, "{n} more question is waiting.", "{n} more questions are waiting.")).color(t.text_dim));
        }
        ui.add_space(6.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let no = primary_button(ui, no_label, true);
            let yes = ui.add(egui::Button::new(yes_label).min_size(vec2(72.0, 26.0)));
            app.hit.add("agentNo", 0, no.rect);
            app.hit.add("agentYes", 0, yes.rect);
            if no.clicked() {
                said = Some(false);
            } else if yes.clicked() {
                said = Some(true);
            }
        });
    });
    if let Some(yes) = said {
        answer(app, yes);
    }
}

#[cfg(test)]
#[path = "agent_gate_characterisation_tests.rs"]
mod agent_gate_characterisation_tests;

#[cfg(test)]
#[path = "agent_log_tests.rs"]
mod agent_log_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use mailcraft_engine::Session;
    use mailcraft_model::FolderKind;

    const NOW: i64 = 1_791_993_600;

    fn app() -> MailApp {
        let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), crate::Services::default());
        app.autosave = false;
        app
    }

    /// Send `method` as an agent allowed `allow`; the answer, if there is one yet.
    fn ask(app: &mut MailApp, method: &str, params: Value, allow: Vec<Class>) -> (Option<Value>, std::sync::mpsc::Receiver<Value>) {
        let (req, rx) = ControlRequest::new(method, params);
        let req = req.from_agent(allow);
        let out = crate::control::handle(app, &egui::Context::default(), &req);
        (if let Outcome::Done(v) = out { Some(v) } else { None }, rx)
    }

    fn sent(app: &MailApp) -> usize {
        let out: Vec<u64> =
            app.session.mailbox.folders.iter().filter(|f| matches!(f.kind, FolderKind::Outbox | FolderKind::Sent)).map(|f| f.id).collect();
        app.session.mailbox.messages.values().filter(|m| out.contains(&m.folder) && m.subject == "Re: Invoice").count()
    }

    fn write(app: &mut MailApp) {
        app.execute("mail.new", json!({"to": "x@attacker.example", "subject": "Re: Invoice"})).unwrap();
        app.execute("compose.attach", json!({"name": "id_ed25519", "data": "AAAA"})).unwrap();
    }

    #[test]
    fn every_window_command_has_a_class() {
        let missing: Vec<&str> = crate::UI_COMMANDS.iter().map(|c| c.0).filter(|id| agent::access(id).is_none()).collect();
        assert!(missing.is_empty(), "classify these for agents in crates/engine/src/agent.rs: {missing:?}");
        assert_eq!(agent::access("ui.previewAttachment"), Some(agent::Access::Never));
        assert_eq!(agent::access("ui.googleClientId"), Some(mailcraft_engine::agent::Access::Never));
        // The retired secret setter stays explicitly denied, but is no longer a window command.
        assert_eq!(agent::access("ui.googleClientSecret"), Some(mailcraft_engine::agent::Access::Never));
        assert!(!crate::UI_COMMANDS.iter().any(|command| command.0 == "ui.googleClientSecret"));
    }

    #[test]
    fn sending_waits_for_the_owners_yes() {
        let mut a = app();
        write(&mut a);
        let (now, rx) = ask(&mut a, "engine.execute", json!({"command": "compose.send"}), vec![]);
        assert!(now.is_none(), "asked first");
        let q = a.asking.front().unwrap();
        assert_eq!(q.class, Class::Send);
        assert!(q.what.contains("Re: Invoice") && q.what.contains("x@attacker.example") && q.what.contains("id_ed25519"), "{}", q.what);
        // No.
        answer(&mut a, false);
        assert_eq!(rx.try_recv().unwrap()["ok"], false);
        assert_eq!(sent(&a), 0);
        assert!(a.session.compose.is_some());
        // Nobody answers: no.
        let (_, rx) = ask(&mut a, "compose.send", json!({}), vec![]);
        expire(&mut a, crate::now_ms() + WAIT_MS + 1.0);
        assert!(rx.try_recv().unwrap()["error"].as_str().unwrap().contains("in time"));
        assert_eq!(sent(&a), 0);
        // Yes.
        let (_, rx) = ask(&mut a, "engine.execute", json!({"command": "compose.send"}), vec![]);
        answer(&mut a, true);
        assert_eq!(rx.try_recv().unwrap()["ok"], true);
        assert_eq!(sent(&a), 1);
        // Allowed in advance (`--allow send`): no question.
        write(&mut a);
        let (now, _) = ask(&mut a, "engine.execute", json!({"command": "compose.send"}), vec![Class::Send]);
        assert_eq!(now.unwrap()["ok"], true);
        assert!(a.asking.is_empty());
    }

    #[test]
    fn the_owners_own_scripts_work_as_before() {
        let mut a = app();
        write(&mut a);
        let (req, _) = ControlRequest::new("engine.execute", json!({"command": "compose.send"}));
        let out = crate::control::handle(&mut a, &egui::Context::default(), &req);
        assert!(matches!(out, Outcome::Done(ref v) if v["ok"] == true));
        assert_eq!(sent(&a), 1);
    }

    #[test]
    fn what_agents_never_get_through_the_window() {
        let mut a = app();
        write(&mut a);
        let all = Class::ALL.to_vec();
        let bat = std::env::temp_dir().join(format!("mailcraft-gate-{}.bat", std::process::id()));
        for (method, params) in [
            ("engine.execute", json!({"command": "compose.attach", "params": {"path": "/etc/passwd"}})),
            ("compose.attach", json!({"path": "/etc/passwd"})),
            ("engine.execute", json!({"command": "file.exportMessage", "params": {"id": 1, "path": bat.to_string_lossy()}})),
            ("engine.execute", json!({"command": "sync.apply", "params": {}})),
            ("engine.execute", json!({"command": "ui.copyLinkAddress", "params": {"url": "https://x.example"}})),
            ("engine.execute", json!({"command": "ui.copySelection", "params": {"pane": true}})),
            ("engine.execute", json!({"command": "ui.previewAttachment", "params": {"id": 13, "index": 0}})),
            ("engine.execute", json!({"command": "no.such"})),
            ("ui.screenshot", json!({"path": "/tmp/evil.png"})),
            ("app.quit", json!({})),
        ] {
            let (r, _) = ask(&mut a, method, params.clone(), all.clone());
            assert_eq!(r.unwrap_or_default()["ok"], false, "{method} {params}");
        }
        assert_eq!(a.session.compose.as_ref().unwrap().attachments.len(), 1);
        assert!(!a.quit_requested);
        assert!(!bat.exists());
        // Input only with `--allow input`.
        let (r, _) = ask(&mut a, "ui.click", json!({"x": 1, "y": 1}), vec![]);
        assert!(r.unwrap()["error"].as_str().unwrap().contains("--allow input"));
        assert!(a.synthetic.is_empty());
        let (r, _) = ask(&mut a, "ui.click", json!({"x": 1, "y": 1}), vec![Class::Input]);
        assert_eq!(r.unwrap()["ok"], true);
        // And none while the owner is asked.
        a.synthetic.clear();
        let _ = ask(&mut a, "compose.send", json!({}), vec![Class::Input]);
        let (r, _) = ask(&mut a, "ui.click", json!({"x": 1, "y": 1}), vec![Class::Input]);
        assert_eq!(r.unwrap()["ok"], false);
        assert!(a.synthetic.is_empty());
    }

    #[test]
    fn agents_cant_slip_mail_into_the_outbox() {
        let mut a = app();
        let outbox = a.session.mailbox.folders.iter().find(|f| f.kind == FolderKind::Outbox).unwrap().id;
        let id = *a.session.mailbox.messages.keys().next().unwrap();
        let (r, _) = ask(&mut a, "mail.copy", json!({"folder": outbox, "ids": [id]}), vec![]);
        assert!(r.is_none(), "asked first");
        answer(&mut a, false);
        assert_eq!(a.session.mailbox.count(outbox), 0);
    }
}
