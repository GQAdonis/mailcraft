// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Find by Description, the Answers pane and File › Options › AI assistance (#ai), driven as an
//! owner's script drives them and drawn headlessly: nothing panics, the list changes only when a
//! proposal is accepted, nothing is asked of a model without a description, and the settings and
//! the per-account question go only where the owner said.

use std::cell::RefCell;
use std::sync::mpsc::Receiver;

use egui::Key;
use mailcraft_engine::Session;
use serde_json::{Value, json};

use super::settings::{Kind, address_problem};
use crate::people_view::testing::{self, Window};
use crate::{AiProviders, MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

fn window() -> Window {
    let mut w = testing::window("view.mail");
    let _ = w.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    w.run_steps(2);
    w
}

/// A request's id from what a command answered.
fn request_of(v: &Value) -> Option<u64> {
    v.get("request").and_then(Value::as_u64)
}

/// The host's side of a model request, by hand: `event` taken in as `MailApp::model_tick` would.
fn came(app: &mut MailApp, request: u64, event: Value) {
    app.execute("ai.apply", json!({"requestId": request, "event": event})).unwrap();
}

#[test]
fn find_by_description_opens_in_mail_and_draws_every_stage() {
    let mut w = window();
    assert!(w.state().hit.rect("findByDescriptionButton", 0).is_some(), "the button is in the search box");
    let opened = w.state_mut().execute("ui.findByDescription", json!({"open": true})).unwrap();
    assert_eq!(opened["stage"], "ready");
    w.run_steps(3);
    assert!(w.state().hit.rect("findByDescription", 0).is_some(), "the panel is drawn");
    // Opening it closes Advanced Search, and opening Advanced Search closes it.
    w.state_mut().execute("ui.advancedSearch", json!({"open": true})).unwrap();
    w.run_steps(2);
    assert!(w.state().ai.narrow.is_none());
    w.state_mut().execute("ui.findByDescription", json!({"open": true})).unwrap();
    w.run_steps(2);
    assert!(w.state().ai.narrow.is_some() && w.state().advanced_search.is_none());
    // Nothing is asked without a description.
    assert!(w.state_mut().execute("ui.findByDescription", json!({"suggest": true})).is_err());
    assert!(w.state().session.ai.all().is_empty());
}

#[test]
fn a_proposal_changes_nothing_until_it_is_accepted() {
    let mut w = window();
    let before = w.state().session.view.search.clone();
    let asked = w.state_mut().execute("ui.findByDescription", json!({"text": "invoices from Ada with a PDF", "suggest": true})).unwrap();
    assert_eq!(asked["stage"], "asking");
    let request = request_of(&asked).expect("a request");
    w.run_steps(3);
    came(w.state_mut(), request, json!({"type": "done", "text": "{\"query\": \"from:ada has:attachment\"}"}));
    w.run_steps(3);
    let panel = w.state().ai.narrow.clone().expect("open");
    assert_eq!(panel.stage, super::narrow::Stage::Proposal);
    assert_eq!(panel.query, "from:ada has:attachment");
    assert_eq!(w.state().session.view.search, before, "the list is as it was");
    // Accepted: the search, in place of the one there was.
    w.state_mut().execute("ui.findByDescription", json!({"accept": true})).unwrap();
    w.run_steps(2);
    assert!(w.state().ai.narrow.is_none());
    assert_eq!(w.state().session.view.search, "from:ada has:attachment");
}

#[test]
fn escape_stops_a_request_and_then_closes_the_panel() {
    let mut w = window();
    let asked = w.state_mut().execute("ui.findByDescription", json!({"text": "from Ada", "suggest": true})).unwrap();
    let request = request_of(&asked).unwrap();
    w.run_steps(2);
    testing::key(&mut w, Key::Escape);
    assert!(!w.state().session.ai.get(request).is_some_and(|t| t.state.live()), "called off");
    assert_eq!(w.state().ai.narrow.as_ref().map(|p| p.text.as_str()), Some("from Ada"), "the description stays");
    testing::key(&mut w, Key::Escape);
    assert!(w.state().ai.narrow.is_none());
}

#[test]
fn a_failed_suggestion_says_why_in_the_windows_words() {
    let mut w = window();
    let asked = w.state_mut().execute("ui.findByDescription", json!({"text": "from Ada", "suggest": true})).unwrap();
    let request = request_of(&asked).unwrap();
    came(
        w.state_mut(),
        request,
        json!({"type": "error", "kind": "unreachable", "message": "Couldn't reach the model provider at http://127.0.0.1:1."}),
    );
    w.run_steps(3);
    let stage = w.state().ai.narrow.as_ref().map(|p| p.stage);
    assert_eq!(stage, Some(super::narrow::Stage::Trouble(super::errors::Trouble::Unreachable)));
}

#[test]
fn the_answers_pane_streams_stops_and_starts_clean() {
    let mut w = window();
    let asked = w.state_mut().execute("ui.answers", json!({"question": "when is the venue deposit due?"})).unwrap();
    assert_eq!(asked["open"], true);
    w.run_steps(3);
    assert!(w.state().hit.rect("answers", 0).is_some(), "the pane is drawn");
    if let Some(request) = request_of(&asked) {
        came(
            w.state_mut(),
            request,
            json!({"type": "delta", "text": "The deposit is due **Friday** [1]. <img src=\"https://tracker.example/p.gif\"> "}),
        );
        w.run_steps(3);
        assert_eq!(w.state_mut().execute("ui.answers", json!({})).unwrap()["open"], false, "toggled closed");
        // Opened again, it starts clean: nothing of the last answer.
        let reopened = w.state_mut().execute("ui.answers", json!({"open": true})).unwrap();
        assert_eq!(reopened["stage"], "idle");
        assert!(!w.state().session.ai.get(request).is_some_and(|t| t.state.live()), "closing called it off");
        // Asked again, then stopped: what came stays.
        let again = w.state_mut().execute("ui.answers", json!({"question": "when is the venue deposit due?"})).unwrap();
        let request = request_of(&again).unwrap();
        came(w.state_mut(), request, json!({"type": "delta", "text": "Friday [1]"}));
        w.run_steps(2);
        assert_eq!(w.state_mut().execute("ui.answers", json!({"stop": true})).unwrap()["stage"], "stopped");
        w.run_steps(3);
        assert!(w.state().ai.answers.as_ref().is_some_and(|p| p.text.contains("Friday")));
    } else {
        assert_eq!(asked["stage"], "nothingFound", "nothing found: nothing asked");
        assert!(w.state().session.ai.all().is_empty());
    }
}

#[test]
fn a_finished_answer_names_only_sources_that_were_sent() {
    let mut w = window();
    let asked = w.state_mut().execute("ui.answers", json!({"question": "meeting"})).unwrap();
    let Some(request) = request_of(&asked) else { return };
    came(w.state_mut(), request, json!({"type": "done", "text": "It is on Monday [1], not [7]."}));
    w.run_steps(4);
    let pane = w.state().ai.answers.as_ref().expect("open");
    assert_eq!(pane.stage, super::answers::Stage::Done);
    assert!(pane.cited.iter().all(|c| c.number == 1));
    assert!(pane.cited.len() <= 3);
}

/// What the host keeps of the providers, in memory: no key is ever handed back.
#[derive(Default)]
struct FakeHost {
    providers: RefCell<Vec<Value>>,
    active: RefCell<String>,
}

fn sent<T: Send + 'static>(value: T) -> Receiver<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = tx.send(value);
    rx
}

impl AiProviders for FakeHost {
    fn providers(&self) -> Value {
        json!({"active": *self.active.borrow(), "providers": *self.providers.borrow()})
    }
    fn set_provider(&self, provider: &Value) -> Result<(), String> {
        let id = provider.get("id").and_then(Value::as_str).ok_or("no id")?.to_string();
        let mut list = self.providers.borrow_mut();
        let was = list.iter().position(|p| p.get("id").and_then(Value::as_str) == Some(id.as_str()));
        let mut provider = provider.clone();
        // A new address (or a new provider) loses the consents, as the host does: they are never
        // taken from what is handed in.
        let mut kept = json!([]);
        if let Some(at) = was {
            let old = list.remove(at);
            if old.get("base_url") == provider.get("base_url") {
                kept = old.get("consented_accounts").cloned().unwrap_or(json!([]));
            }
        }
        provider["consented_accounts"] = kept;
        list.push(provider);
        Ok(())
    }
    fn remove_provider(&self, id: &str) -> Result<(), String> {
        self.providers.borrow_mut().retain(|p| p.get("id").and_then(Value::as_str) != Some(id));
        Ok(())
    }
    fn set_active(&self, id: &str) -> Result<(), String> {
        *self.active.borrow_mut() = id.to_string();
        Ok(())
    }
    fn set_consent(&self, id: &str, account: &str, allowed: bool) -> Result<(), String> {
        let mut list = self.providers.borrow_mut();
        let p = list.iter_mut().find(|p| p.get("id").and_then(Value::as_str) == Some(id)).ok_or("no provider")?;
        let mut accounts: Vec<String> = p
            .get("consented_accounts")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default();
        accounts.retain(|a| !a.eq_ignore_ascii_case(account));
        if allowed {
            accounts.push(account.to_string());
        }
        p["consented_accounts"] = json!(accounts);
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

/// A host's model thread that ends at once, sending nothing.
fn no_model(_job: crate::ModelJob) -> Receiver<mailcraft_model::ai::ModelEvent> {
    std::sync::mpsc::channel().1
}

fn app_with_host() -> MailApp {
    let services = Services {
        ai_providers: Some(Box::new(FakeHost::default())),
        // Requests are taken and let go of at once: nothing is sent anywhere.
        model_request: Some(Box::new(no_model)),
        ..Default::default()
    };
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), services);
    app.autosave = false;
    app
}

#[test]
fn addresses_are_checked_for_where_the_model_runs() {
    assert_eq!(address_problem(Kind::Local, "http://127.0.0.1:11434/v1"), None);
    assert_eq!(address_problem(Kind::Local, "http://localhost:1234"), None);
    assert_eq!(address_problem(Kind::Local, "http://[::1]:8080/v1"), None);
    assert_eq!(address_problem(Kind::Local, "https://models.example/v1"), None);
    assert!(address_problem(Kind::Local, "http://192.0.2.1/v1").is_some());
    assert!(address_problem(Kind::Local, "http://127.0.0.1.example/v1").is_some());
    assert!(address_problem(Kind::Remote, "http://models.example/v1").is_some());
    assert!(address_problem(Kind::Remote, "https://user@models.example/v1").is_some());
    assert!(address_problem(Kind::Remote, "models.example").is_some());
    assert!(address_problem(Kind::Remote, "javascript://x").is_some());
    assert_eq!(address_problem(Kind::Remote, ""), None);
}

#[test]
fn a_service_on_the_internet_is_asked_about_first_and_each_account_too() {
    let mut app = app_with_host();
    // Asked first; nothing changes until the owner's yes.
    let first = app.execute("ui.aiSettings", json!({"kind": "remote"})).unwrap();
    assert_eq!(first["kind"], "none");
    assert_eq!(first["asking"], "remote");
    app.dialog = None;
    let chosen = app.execute("ui.aiSettings", json!({"kind": "remote", "confirmed": true})).unwrap();
    assert_eq!(chosen["kind"], "remote");
    // Plain http is refused for another computer.
    let refused = app.execute("ui.aiSettings", json!({"address": "http://models.example/v1"})).unwrap();
    assert!(refused["problem"].is_string());
    assert_eq!(refused["active"], "");
    let used = app.execute("ui.aiSettings", json!({"address": "https://models.example/v1", "model": "m"})).unwrap();
    assert_eq!(used["problem"], Value::Null);
    assert_eq!(used["active"], "remote");
    let Some(account) = app.session.mailbox.accounts.first().map(|a| a.email.clone()) else { return };
    // The question, whose default answer is no.
    app.execute("ui.aiConsent", json!({"account": account})).unwrap();
    assert!(matches!(&app.dialog, Some(crate::dialogs::Dialog::Confirm { command, .. }) if command == "ui.aiConsent"));
    app.dialog = None;
    assert_eq!(app.execute("ui.aiConsent", json!({"account": account, "allowed": true})).unwrap()["allowed"], false, "not without the yes");
    app.dialog = None;
    assert_eq!(app.execute("ui.aiConsent", json!({"account": account, "allowed": true, "confirmed": true})).unwrap()["allowed"], true);
    assert_eq!(app.execute("ui.aiConsent", json!({"account": account, "allowed": false})).unwrap()["allowed"], false);
    // A new address: the permissions given for the old one are gone.
    app.execute("ui.aiConsent", json!({"account": account, "allowed": true, "confirmed": true})).unwrap();
    app.execute("ui.aiSettings", json!({"address": "https://other.example/v1"})).unwrap();
    assert_eq!(super::Providers::of(&app).active().map(|p| p.allows(&account)), Some(false));
    // None: nothing is in use, and what was under way is called off.
    assert_eq!(app.execute("ui.aiSettings", json!({"kind": "none"})).unwrap()["active"], "");
}

/// A provider the host already has keeps its own id: the page edits it, its key and its
/// permissions under that id, and makes no other.
#[test]
fn a_provider_the_host_has_keeps_its_own_id() {
    let host = FakeHost::default();
    host.set_provider(&json!({"id": "mailcraft-boundary-7", "kind": "remote", "base_url": "https://models.example/v1", "model": "m"})).unwrap();
    host.set_active("mailcraft-boundary-7").unwrap();
    let services = Services { ai_providers: Some(Box::new(host)), model_request: Some(Box::new(no_model)), ..Default::default() };
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), services);
    app.autosave = false;
    let shown = app.execute("ui.aiSettings", json!({})).unwrap();
    assert_eq!((shown["kind"].as_str(), shown["id"].as_str()), (Some("remote"), Some("mailcraft-boundary-7")));
    let changed = app.execute("ui.aiSettings", json!({"model": "other"})).unwrap();
    assert_eq!(changed["active"], "mailcraft-boundary-7");
    let providers = super::Providers::of(&app);
    assert_eq!(providers.all.len(), 1, "no provider of the window's own was made");
    assert_eq!(providers.active().map(|p| p.model.as_str()), Some("other"));
    if let Some(account) = app.session.mailbox.accounts.first().map(|a| a.email.clone()) {
        app.execute("ui.aiConsent", json!({"account": account, "allowed": true, "confirmed": true})).unwrap();
        assert_eq!(super::Providers::of(&app).active().map(|p| p.allows(&account)), Some(true));
    }
}

#[test]
fn an_account_the_mailbox_doesnt_have_is_refused() {
    let mut app = app_with_host();
    assert!(app.execute("ui.aiConsent", json!({"account": "nobody@elsewhere.example", "allowed": true, "confirmed": true})).is_err());
    assert!(app.execute("ui.aiConsent", json!({})).is_err());
    assert!(app.execute("ui.aiSettings", json!({"kind": "everywhere"})).is_err());
}

#[test]
fn the_options_page_draws_with_and_without_a_host() {
    let mut w = window();
    w.state_mut().execute("ui.aiSettings", json!({})).unwrap();
    w.run_steps(3);
    assert!(w.state().ui.backstage);
    w.state_mut().services.ai_providers = Some(Box::new(FakeHost::default()));
    w.state_mut().services.model_request = Some(Box::new(no_model));
    w.state_mut().execute("ui.aiSettings", json!({"kind": "local", "address": "http://192.0.2.1/v1"})).unwrap();
    w.run_steps(3);
    assert!(w.state().ai.settings.problem.is_some());
    w.state_mut().execute("ui.aiSettings", json!({"kind": "remote", "confirmed": true, "address": "https://models.example/v1"})).unwrap();
    w.run_steps(3);
    assert!(w.state().hit.rect("aiKey", 0).is_some(), "the key's field is drawn");
}

/// None of the AI surfaces' window commands is an agent's.
#[test]
fn no_agent_drives_the_ai_surfaces() {
    for id in ["ui.findByDescription", "ui.answers", "ui.aiSettings", "ui.aiConsent"] {
        assert_eq!(mailcraft_engine::agent::access(id), Some(mailcraft_engine::agent::Access::Never), "{id}");
    }
}
