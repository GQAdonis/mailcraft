// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The walk of the owner's chain (ai-runtime-chain 20.2–20.3, 20.6–20.7; SCN-RT-CHAIN-001,
//! SCN-RT-CHAIN-002, SCN-RT-WHERE-001 on the chain's side), against fake runtimes in this process:
//! which runtimes were asked their status or sent the request, in what order against what the
//! owner was told and asked, the answer, and its provenance. A runtime that `leaves` and that an
//! account of the request hasn't allowed is never asked anything, not even its status; `ask` puts
//! the question before a second runtime starts and a no starts nothing; `stop` shows the error; a
//! tool-use stop is told before any later hop; a cancelled hop ends the walk; and what a runtime
//! says of itself never makes it local. No real vendor, no real mailbox: synthetic `example.org`
//! accounts, and fake servers on 127.0.0.1 for the `compat` walk.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use mailcraft_ai::{ProviderConfig, ProviderKind, Timeouts};
use mailcraft_model::ai::{
    AiKind, AnsweredBy, Chain, ChainEntry, ChatMessage, ErrorKind, Fallback, ModelEvent, ModelRequest, Role, RuntimeId, Where,
};
use mailcraft_ui_egui::ModelJob;

use crate::ai::keys::Broker;
use crate::ai::route::{HopOutcome, Provenance, Skip, Unusable, admit, carried};
use crate::ai::run::desk::Desk;
use crate::ai::run::{Answered, Failed, FallbackQuestion, Hop, HopEnd, OwnerAnswer, Runtimes, Watch, run, run_with, walk};
use crate::ai::settings::{AiSettings, Shared};

const ANA: &str = "ana@example.org";
const BEN: &str = "ben@example.org";

/// What a fake runtime does when it is asked.
#[derive(Clone)]
enum Plan {
    Answer(&'static str),
    Fail(ErrorKind),
    /// Fails `cancelled`, having set the job's flag, after its process group is gone ("gone" in
    /// the log).
    CancelledRun,
    /// At its hop, it turns out it can't be used.
    Unusable(Unusable),
}

type Log = Rc<RefCell<Vec<String>>>;

/// Fake runtimes: each status and each request is written to the shared log.
struct Fakes {
    log: Log,
    plans: BTreeMap<RuntimeId, Plan>,
    down: BTreeMap<RuntimeId, Unusable>,
    cancel: Option<Arc<AtomicBool>>,
}

impl Fakes {
    fn new(log: &Log, plans: &[(RuntimeId, Plan)]) -> Fakes {
        Fakes { log: log.clone(), plans: plans.iter().cloned().collect(), down: BTreeMap::new(), cancel: None }
    }
}

impl Runtimes for Fakes {
    fn status(&mut self, entry: &ChainEntry, _: &AiSettings, _: &AtomicBool) -> Result<(), Unusable> {
        self.log.borrow_mut().push(format!("status {}", entry.runtime()));
        self.down.get(entry.runtime()).map_or(Ok(()), |why| Err(*why))
    }

    fn ask(&mut self, hop: &Hop<'_>, _: &Shared, _: &mut dyn FnMut(ModelEvent) -> bool) -> HopEnd {
        let runtime = hop.admitted.entry.runtime().clone();
        self.log.borrow_mut().push(format!("ask {runtime}"));
        match self.plans.get(&runtime).cloned().unwrap_or(Plan::Fail(ErrorKind::BrokenRuntime)) {
            Plan::Answer(text) => HopEnd::Answered(Answered {
                text: Some(text.to_string()),
                by: AnsweredBy { provider: runtime.to_string(), model: String::new() },
                sources: None,
            }),
            Plan::Fail(kind) => HopEnd::Failed(Failed { failure: crate::ai::run::Failure::new(kind, kind.owner_words()), started: false }),
            Plan::CancelledRun => {
                if let Some(cancel) = &self.cancel {
                    cancel.store(true, Ordering::SeqCst);
                }
                self.log.borrow_mut().push(format!("gone {runtime}"));
                HopEnd::Failed(Failed { failure: crate::ai::run::Failure::new(ErrorKind::Cancelled, "stopped"), started: false })
            }
            Plan::Unusable(why) => HopEnd::Unusable(why),
        }
    }
}

/// A watch that writes what the owner is told and asked to the shared log, and answers the
/// fallback questions from `answers` (none left: no).
struct Owner {
    log: Log,
    answers: VecDeque<OwnerAnswer>,
    questions: Vec<FallbackQuestion>,
    provenance: Option<Provenance>,
}

impl Owner {
    fn new(log: &Log, answers: &[OwnerAnswer]) -> Owner {
        Owner { log: log.clone(), answers: answers.iter().copied().collect(), questions: Vec::new(), provenance: None }
    }
}

impl Watch for Owner {
    fn tool_use_stopped(&mut self, _: u64, runtime: &RuntimeId) {
        self.log.borrow_mut().push(format!("notice {runtime}"));
    }

    fn ask_owner(&mut self, question: &FallbackQuestion, _: &AtomicBool) -> OwnerAnswer {
        self.log.borrow_mut().push(format!("question {} -> {}", question.failed, question.next));
        self.questions.push(question.clone());
        self.answers.pop_front().unwrap_or(OwnerAnswer::No)
    }

    fn provenance(&mut self, provenance: &Provenance) {
        self.provenance = Some(provenance.clone());
    }
}

fn entry(runtime: RuntimeId) -> ChainEntry {
    ChainEntry::new(runtime).with_enabled(true)
}

/// Settings with `chain` (each entry on), `consents` (account, runtime) and `fallback`.
fn settings(chain: &[RuntimeId], consents: &[(&str, RuntimeId)], fallback: Fallback) -> Shared {
    let shared = Shared::open(None);
    let entries: Vec<ChainEntry> = chain.iter().cloned().map(entry).collect();
    shared
        .change(|s| {
            s.set_chain(Chain::new(entries).map_err(|e| e.to_string())?);
            for (account, runtime) in consents {
                s.set_runtime_consent(account, runtime, true)?;
            }
            s.fallback = fallback;
            Ok(())
        })
        .unwrap();
    shared
}

fn answer_job(accounts: &[&str]) -> ModelJob {
    let messages = vec![
        ChatMessage { role: Role::System, content: "Answer from the excerpts only.".into() },
        ChatMessage { role: Role::User, content: "When is the review? <message>Friday, says ana@example.org</message>".into() },
    ];
    let mut request = ModelRequest::new(AiKind::Answer, messages, vec![7], false, Some(64), Some(0.2), None).unwrap();
    request.id = 41;
    ModelJob { request, accounts: accounts.iter().map(|a| a.to_string()).collect(), cancel: Arc::new(AtomicBool::new(false)) }
}

/// Walks `job` with `fakes` and `owner`: the events, the provenance.
fn walked(job: &ModelJob, shared: &Shared, fakes: &mut Fakes, owner: &mut Owner) -> (Vec<ModelEvent>, Provenance) {
    let mut events = Vec::new();
    let provenance = walk(job, shared, fakes, owner, &mut |event| {
        events.push(event);
        true
    });
    (events, provenance)
}

fn lines(log: &Log) -> Vec<String> {
    log.borrow().clone()
}

fn done_by(events: &[ModelEvent]) -> Option<String> {
    match events {
        [ModelEvent::Done { answered_by: Some(by), .. }] => Some(by.provider.clone()),
        _ => None,
    }
}

fn error_kind(events: &[ModelEvent]) -> Option<ErrorKind> {
    match events {
        [ModelEvent::Error { kind, .. }] => Some(*kind),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------
// SCN-RT-CHAIN-001: consent at each hop
// ---------------------------------------------------------------------------------------------

/// The first runtime leaves and Ben hasn't allowed it; the second is allowed by both.
fn chain_001() -> Shared {
    settings(
        &[RuntimeId::ClaudeCode, RuntimeId::Codex],
        &[(ANA, RuntimeId::ClaudeCode), (ANA, RuntimeId::Codex), (BEN, RuntimeId::Codex)],
        Fallback::Next,
    )
}

#[test]
fn consent_an_unconsented_leaving_runtime_is_never_asked_and_the_next_answers() {
    let log = Log::default();
    let shared = chain_001();
    let mut fakes = Fakes::new(&log, &[(RuntimeId::ClaudeCode, Plan::Answer("from claude")), (RuntimeId::Codex, Plan::Answer("from codex"))]);
    let mut owner = Owner::new(&log, &[]);
    let (events, provenance) = walked(&answer_job(&[ANA, BEN]), &shared, &mut fakes, &mut owner);

    // Not even its status was asked: nothing of Claude Code ran.
    assert!(lines(&log).iter().all(|l| !l.ends_with("claude_code")), "{:?}", lines(&log));
    assert_eq!(lines(&log), ["status codex", "ask codex"]);
    assert_eq!(done_by(&events).as_deref(), Some("codex"));
    assert_eq!(provenance.answered_by, Some(RuntimeId::Codex));
    assert_eq!(provenance.skipped().collect::<Vec<_>>(), [(&RuntimeId::ClaudeCode, Skip::NoConsent)]);
    assert_eq!(provenance.failed().count(), 0);
    assert_eq!(owner.provenance, Some(provenance.clone()));
    let json = provenance.to_json();
    assert_eq!(json["answeredBy"], "codex");
    assert_eq!(json["passedOver"][0]["runtime"], "claude_code");
    assert_eq!(json["passedOver"][0]["skipped"], "no_consent");
}

#[test]
fn consent_is_read_again_at_each_hop() {
    // Ana allows both; Ben's yes to Codex is taken back while Claude Code runs and fails.
    let log = Log::default();
    let shared = settings(
        &[RuntimeId::ClaudeCode, RuntimeId::Codex],
        &[(ANA, RuntimeId::ClaudeCode), (BEN, RuntimeId::ClaudeCode), (ANA, RuntimeId::Codex), (BEN, RuntimeId::Codex)],
        Fallback::Next,
    );
    struct Withdrawing(Fakes, Shared);
    impl Runtimes for Withdrawing {
        fn status(&mut self, entry: &ChainEntry, s: &AiSettings, c: &AtomicBool) -> Result<(), Unusable> {
            self.0.status(entry, s, c)
        }
        fn ask(&mut self, hop: &Hop<'_>, s: &Shared, emit: &mut dyn FnMut(ModelEvent) -> bool) -> HopEnd {
            let end = self.0.ask(hop, s, emit);
            self.1.change(|s| s.set_runtime_consent(BEN, &RuntimeId::Codex, false)).unwrap();
            end
        }
    }
    let fakes = Fakes::new(&log, &[(RuntimeId::ClaudeCode, Plan::Fail(ErrorKind::Unreachable)), (RuntimeId::Codex, Plan::Answer("x"))]);
    let mut runtimes = Withdrawing(fakes, shared.clone());
    let mut owner = Owner::new(&log, &[]);
    let job = answer_job(&[ANA, BEN]);
    let mut events = Vec::new();
    let provenance = walk(&job, &shared, &mut runtimes, &mut owner, &mut |e| {
        events.push(e);
        true
    });
    assert_eq!(lines(&log), ["status claude_code", "ask claude_code"], "Codex was never asked once Ben said no");
    assert_eq!(error_kind(&events), Some(ErrorKind::Unreachable));
    assert_eq!(provenance.answered_by, None);
    assert_eq!(provenance.notes.len(), 2);
    assert_eq!(provenance.notes[1].outcome, HopOutcome::Skipped(Skip::NoConsent));
}

#[test]
fn consent_disabled_and_unusable_entries_are_passed_over_with_their_reasons() {
    let log = Log::default();
    let shared = settings(
        &[RuntimeId::ChatGpt, RuntimeId::ClaudeCode, RuntimeId::Codex],
        &[(ANA, RuntimeId::ChatGpt), (ANA, RuntimeId::ClaudeCode), (ANA, RuntimeId::Codex)],
        Fallback::Next,
    );
    shared.change(|s| s.chain.with_enabled(&RuntimeId::ChatGpt, false).map(|c| s.chain = c).map_err(|e| e.to_string())).unwrap();
    let mut fakes = Fakes::new(&log, &[(RuntimeId::Codex, Plan::Answer("codex"))]);
    fakes.down.insert(RuntimeId::ClaudeCode, Unusable::SignedOut);
    let mut owner = Owner::new(&log, &[]);
    let (events, provenance) = walked(&answer_job(&[ANA]), &shared, &mut fakes, &mut owner);
    assert_eq!(lines(&log), ["status claude_code", "status codex", "ask codex"]);
    assert_eq!(done_by(&events).as_deref(), Some("codex"));
    assert_eq!(
        provenance.skipped().collect::<Vec<_>>(),
        [(&RuntimeId::ChatGpt, Skip::Disabled), (&RuntimeId::ClaudeCode, Skip::Unusable(Unusable::SignedOut))]
    );
}

#[test]
fn consent_a_request_naming_no_account_goes_to_no_leaving_runtime() {
    let log = Log::default();
    let shared = chain_001();
    let mut fakes = Fakes::new(&log, &[(RuntimeId::Codex, Plan::Answer("codex"))]);
    let mut owner = Owner::new(&log, &[]);
    let (events, provenance) = walked(&answer_job(&[]), &shared, &mut fakes, &mut owner);
    assert!(lines(&log).is_empty(), "{:?}", lines(&log));
    assert_eq!(error_kind(&events), Some(ErrorKind::NoConsent));
    assert_eq!(provenance.skipped().map(|(_, s)| s).collect::<Vec<_>>(), [Skip::NoConsent, Skip::NoConsent]);
}

#[test]
fn neg_consent_all_consented_the_first_answers_and_nothing_is_skipped() {
    let log = Log::default();
    let shared = settings(
        &[RuntimeId::ClaudeCode, RuntimeId::Codex],
        &[(ANA, RuntimeId::ClaudeCode), (BEN, RuntimeId::ClaudeCode), (ANA, RuntimeId::Codex), (BEN, RuntimeId::Codex)],
        Fallback::Next,
    );
    let mut fakes = Fakes::new(&log, &[(RuntimeId::ClaudeCode, Plan::Answer("from claude")), (RuntimeId::Codex, Plan::Answer("from codex"))]);
    let mut owner = Owner::new(&log, &[]);
    let (events, provenance) = walked(&answer_job(&[ANA, BEN]), &shared, &mut fakes, &mut owner);
    assert_eq!(lines(&log), ["status claude_code", "ask claude_code"]);
    assert_eq!(done_by(&events).as_deref(), Some("claude_code"));
    assert_eq!(provenance.answered_by, Some(RuntimeId::ClaudeCode));
    assert!(provenance.notes.is_empty(), "{:?}", provenance.notes);
}

// ---------------------------------------------------------------------------------------------
// SCN-RT-CHAIN-002: the fallback policy
// ---------------------------------------------------------------------------------------------

fn both_allowed(fallback: Fallback) -> Shared {
    settings(&[RuntimeId::ClaudeCode, RuntimeId::Codex], &[(ANA, RuntimeId::ClaudeCode), (ANA, RuntimeId::Codex)], fallback)
}

fn first_fails(log: &Log, kind: ErrorKind) -> Fakes {
    Fakes::new(log, &[(RuntimeId::ClaudeCode, Plan::Fail(kind)), (RuntimeId::Codex, Plan::Answer("from codex"))])
}

#[test]
fn policy_ask_names_both_runtimes_and_starts_nothing_before_the_owners_yes() {
    let log = Log::default();
    let shared = both_allowed(Fallback::Ask);
    let mut fakes = first_fails(&log, ErrorKind::Unreachable);
    let mut owner = Owner::new(&log, &[OwnerAnswer::Yes]);
    let (events, provenance) = walked(&answer_job(&[ANA]), &shared, &mut fakes, &mut owner);
    assert_eq!(owner.questions.len(), 1);
    let question = &owner.questions[0];
    assert_eq!((question.failed.clone(), question.next.clone(), question.reason), (RuntimeId::ClaudeCode, RuntimeId::Codex, ErrorKind::Unreachable));
    assert_eq!(question.request, 41);
    let json = question.to_json();
    assert_eq!((json["failed"].as_str(), json["next"].as_str(), json["reason"].as_str()), (Some("claude_code"), Some("codex"), Some("unreachable")));
    // The second run started only after the question was answered.
    assert_eq!(lines(&log), ["status claude_code", "ask claude_code", "status codex", "question claude_code -> codex", "ask codex"]);
    assert_eq!(done_by(&events).as_deref(), Some("codex"));
    assert_eq!(provenance.failed().collect::<Vec<_>>(), [(&RuntimeId::ClaudeCode, ErrorKind::Unreachable)]);
}

#[test]
fn policy_ask_answered_no_starts_no_second_run_and_shows_the_error() {
    let log = Log::default();
    let shared = both_allowed(Fallback::Ask);
    let mut fakes = first_fails(&log, ErrorKind::Unreachable);
    let mut owner = Owner::new(&log, &[OwnerAnswer::No]);
    let (events, provenance) = walked(&answer_job(&[ANA]), &shared, &mut fakes, &mut owner);
    assert!(!lines(&log).contains(&"ask codex".to_string()), "{:?}", lines(&log));
    assert_eq!(error_kind(&events), Some(ErrorKind::Unreachable));
    assert_eq!(provenance.answered_by, None);
}

#[test]
fn policy_stop_shows_the_error_and_starts_no_second_run() {
    let log = Log::default();
    let shared = both_allowed(Fallback::Stop);
    let mut fakes = first_fails(&log, ErrorKind::Unreachable);
    let mut owner = Owner::new(&log, &[OwnerAnswer::Yes]);
    let (events, _) = walked(&answer_job(&[ANA]), &shared, &mut fakes, &mut owner);
    assert_eq!(lines(&log), ["status claude_code", "ask claude_code"]);
    assert!(owner.questions.is_empty());
    assert_eq!(error_kind(&events), Some(ErrorKind::Unreachable));
}

#[test]
fn policy_next_tries_the_next_eligible_runtime() {
    let log = Log::default();
    let shared = both_allowed(Fallback::Next);
    let mut fakes = first_fails(&log, ErrorKind::SignedOut);
    let mut owner = Owner::new(&log, &[]);
    let (events, provenance) = walked(&answer_job(&[ANA]), &shared, &mut fakes, &mut owner);
    assert!(owner.questions.is_empty());
    assert_eq!(done_by(&events).as_deref(), Some("codex"));
    assert_eq!(provenance.failed().collect::<Vec<_>>(), [(&RuntimeId::ClaudeCode, ErrorKind::SignedOut)]);
}

#[test]
fn policy_a_tool_use_stop_under_next_is_told_before_the_second_run_starts() {
    let log = Log::default();
    let shared = both_allowed(Fallback::Next);
    let mut fakes = first_fails(&log, ErrorKind::ToolUseStopped);
    let mut owner = Owner::new(&log, &[]);
    let (events, provenance) = walked(&answer_job(&[ANA]), &shared, &mut fakes, &mut owner);
    assert_eq!(lines(&log), ["status claude_code", "ask claude_code", "notice claude_code", "status codex", "ask codex"]);
    assert_eq!(done_by(&events).as_deref(), Some("codex"));
    assert_eq!(provenance.answered_by, Some(RuntimeId::Codex));
    assert_eq!(provenance.failed().collect::<Vec<_>>(), [(&RuntimeId::ClaudeCode, ErrorKind::ToolUseStopped)]);
}

#[test]
fn policy_a_tool_use_stop_is_told_under_ask_and_stop_too() {
    for (fallback, answers) in [(Fallback::Ask, vec![OwnerAnswer::No]), (Fallback::Stop, vec![])] {
        let log = Log::default();
        let shared = both_allowed(fallback);
        let mut fakes = first_fails(&log, ErrorKind::ToolUseStopped);
        let mut owner = Owner::new(&log, &answers);
        let (events, _) = walked(&answer_job(&[ANA]), &shared, &mut fakes, &mut owner);
        let log = lines(&log);
        let notice = log.iter().position(|l| l == "notice claude_code").expect("told");
        assert!(log.iter().position(|l| l.starts_with("question")).is_none_or(|q| q > notice), "{log:?}");
        assert!(!log.contains(&"ask codex".to_string()), "{log:?}");
        assert_eq!(error_kind(&events), Some(ErrorKind::ToolUseStopped));
    }
}

#[test]
fn policy_a_kind_that_doesnt_fall_back_ends_the_walk_under_next() {
    let log = Log::default();
    let shared = both_allowed(Fallback::Next);
    let mut fakes = first_fails(&log, ErrorKind::Malformed);
    let mut owner = Owner::new(&log, &[]);
    let (events, _) = walked(&answer_job(&[ANA]), &shared, &mut fakes, &mut owner);
    assert_eq!(lines(&log), ["status claude_code", "ask claude_code"]);
    assert_eq!(error_kind(&events), Some(ErrorKind::Malformed));
}

#[test]
fn policy_cancel_ends_the_walk_once_the_group_is_gone_and_tells_nothing() {
    let log = Log::default();
    let shared = both_allowed(Fallback::Next);
    let job = answer_job(&[ANA]);
    let mut fakes = Fakes::new(&log, &[(RuntimeId::ClaudeCode, Plan::CancelledRun), (RuntimeId::Codex, Plan::Answer("from codex"))]);
    fakes.cancel = Some(job.cancel.clone());
    let mut owner = Owner::new(&log, &[]);
    let (events, _) = walked(&job, &shared, &mut fakes, &mut owner);
    assert_eq!(lines(&log), ["status claude_code", "ask claude_code", "gone claude_code"]);
    assert!(events.is_empty(), "a cancelled request ends with neither done nor error: {events:?}");
}

#[test]
fn policy_ask_with_no_next_eligible_runtime_asks_nothing() {
    let log = Log::default();
    let shared = settings(&[RuntimeId::ClaudeCode, RuntimeId::Codex], &[(ANA, RuntimeId::ClaudeCode)], Fallback::Ask);
    let mut fakes = first_fails(&log, ErrorKind::Unreachable);
    let mut owner = Owner::new(&log, &[OwnerAnswer::Yes]);
    let (events, provenance) = walked(&answer_job(&[ANA]), &shared, &mut fakes, &mut owner);
    assert!(owner.questions.is_empty());
    assert_eq!(error_kind(&events), Some(ErrorKind::Unreachable));
    assert_eq!(provenance.skipped().collect::<Vec<_>>(), [(&RuntimeId::Codex, Skip::NoConsent)]);
}

#[test]
fn policy_a_runtime_unusable_at_its_hop_is_passed_over() {
    let log = Log::default();
    let shared = both_allowed(Fallback::Next);
    let mut fakes =
        Fakes::new(&log, &[(RuntimeId::ClaudeCode, Plan::Unusable(Unusable::NotRunning)), (RuntimeId::Codex, Plan::Answer("from codex"))]);
    let mut owner = Owner::new(&log, &[]);
    let (events, provenance) = walked(&answer_job(&[ANA]), &shared, &mut fakes, &mut owner);
    assert_eq!(done_by(&events).as_deref(), Some("codex"));
    assert_eq!(provenance.skipped().collect::<Vec<_>>(), [(&RuntimeId::ClaudeCode, Skip::Unusable(Unusable::NotRunning))]);
}

#[test]
fn neg_policy_all_consented_and_the_first_succeeds_no_question() {
    for fallback in [Fallback::Ask, Fallback::Stop, Fallback::Next] {
        let log = Log::default();
        let shared = both_allowed(fallback);
        let mut fakes = Fakes::new(&log, &[(RuntimeId::ClaudeCode, Plan::Answer("from claude")), (RuntimeId::Codex, Plan::Answer("from codex"))]);
        let mut owner = Owner::new(&log, &[OwnerAnswer::Yes]);
        let (events, provenance) = walked(&answer_job(&[ANA]), &shared, &mut fakes, &mut owner);
        assert!(owner.questions.is_empty());
        assert_eq!(lines(&log), ["status claude_code", "ask claude_code"]);
        assert_eq!(done_by(&events).as_deref(), Some("claude_code"));
        assert!(provenance.notes.is_empty());
    }
}

// ---------------------------------------------------------------------------------------------
// The owner's desk: only the owner's answer starts the next run
// ---------------------------------------------------------------------------------------------

fn question() -> FallbackQuestion {
    FallbackQuestion { request: 7, failed: RuntimeId::ClaudeCode, reason: ErrorKind::Unreachable, next: RuntimeId::Codex }
}

#[test]
fn policy_the_desk_waits_for_the_owners_answer_to_its_own_question() {
    let desk: &'static Desk = Box::leak(Box::default());
    let cancel = AtomicBool::new(false);
    let answering = std::thread::spawn(move || {
        while desk.fallback_question().is_none() {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(desk.answer_fallback(8, true).is_err(), "an answer to another request is refused");
        desk.answer_fallback(7, true).unwrap();
    });
    let mut watch = crate::ai::run::DeskWatch::new(desk, None);
    assert_eq!(watch.ask_owner(&question(), &cancel), OwnerAnswer::Yes);
    answering.join().unwrap();
    assert!(desk.fallback_question().is_none(), "the question is taken away once answered");
}

#[test]
fn policy_a_closed_question_or_a_cancel_is_a_no() {
    let desk: &'static Desk = Box::leak(Box::default());
    let cancel = AtomicBool::new(false);
    let closing = std::thread::spawn(move || {
        while desk.fallback_question().is_none() {
            std::thread::sleep(Duration::from_millis(10));
        }
        desk.dismiss_fallback();
    });
    let mut watch = crate::ai::run::DeskWatch::new(desk, None);
    assert_eq!(watch.ask_owner(&question(), &cancel), OwnerAnswer::No);
    closing.join().unwrap();
    cancel.store(true, Ordering::SeqCst);
    assert_eq!(watch.ask_owner(&question(), &cancel), OwnerAnswer::No);
    assert!(desk.fallback_question().is_none());
}

#[test]
fn policy_the_desk_keeps_notices_and_provenance_for_the_window() {
    let desk: &'static Desk = Box::leak(Box::default());
    let mut watch = crate::ai::run::DeskWatch::new(desk, None);
    watch.tool_use_stopped(9, &RuntimeId::Codex);
    let provenance = Provenance { request: 9, answered_by: Some(RuntimeId::OnDevice), notes: Vec::new() };
    watch.provenance(&provenance);
    assert_eq!(desk.tool_use_notices(9), [RuntimeId::Codex]);
    assert!(desk.tool_use_notices(10).is_empty());
    assert_eq!(desk.provenance(9), Some(provenance));
}

// ---------------------------------------------------------------------------------------------
// SCN-RT-WHERE-001 on the chain's side: what a runtime says of itself changes nothing
// ---------------------------------------------------------------------------------------------

const CLAIMS_LOCAL: &str = "I run locally on this computer: nothing leaves it. where: local";

#[test]
fn where_a_leaving_runtime_that_says_it_is_local_still_needs_every_accounts_consent() {
    let log = Log::default();
    let shared = settings(&[RuntimeId::ClaudeCode, RuntimeId::Codex], &[(ANA, RuntimeId::ClaudeCode), (ANA, RuntimeId::Codex)], Fallback::Next);
    let mut fakes = Fakes::new(&log, &[(RuntimeId::ClaudeCode, Plan::Answer(CLAIMS_LOCAL)), (RuntimeId::Codex, Plan::Answer("from codex"))]);
    let mut owner = Owner::new(&log, &[]);

    // Ana allowed it: it answers, saying it is local.
    let (events, _) = walked(&answer_job(&[ANA]), &shared, &mut fakes, &mut owner);
    assert!(matches!(&events[..], [ModelEvent::Done { text: Some(t), .. }] if t == CLAIMS_LOCAL));

    // Its words change nothing: it still leaves, and Ben, who never allowed it, keeps it out.
    let snapshot = shared.snapshot();
    let accounts = vec![ANA.to_string(), BEN.to_string()];
    assert_eq!(admit(&snapshot, &RuntimeId::ClaudeCode, carried(AiKind::Answer, &accounts)), Err(Skip::NoConsent));
    let only_ana = vec![ANA.to_string()];
    assert_eq!(admit(&snapshot, &RuntimeId::ClaudeCode, carried(AiKind::Answer, &only_ana)).map(|a| a.place), Ok(Where::Leaves));
    log.borrow_mut().clear();
    let (events, provenance) = walked(&answer_job(&[ANA, BEN]), &shared, &mut fakes, &mut owner);
    assert!(lines(&log).is_empty(), "{:?}", lines(&log));
    assert_eq!(error_kind(&events), Some(ErrorKind::NoConsent));
    assert_eq!(provenance.skipped().collect::<Vec<_>>(), [(&RuntimeId::ClaudeCode, Skip::NoConsent), (&RuntimeId::Codex, Skip::NoConsent)]);
}

#[test]
fn where_every_vendor_runtime_leaves_and_the_on_device_model_is_local() {
    let shared = settings(&[RuntimeId::OnDevice, RuntimeId::ChatGpt, RuntimeId::ClaudeCode, RuntimeId::Codex, RuntimeId::Uar], &[], Fallback::Next);
    let snapshot = shared.snapshot();
    let words = carried(AiKind::Narrow, &[]);
    for (runtime, place) in [
        (RuntimeId::OnDevice, Where::Local),
        (RuntimeId::ChatGpt, Where::Leaves),
        (RuntimeId::ClaudeCode, Where::Leaves),
        (RuntimeId::Codex, Where::Leaves),
        (RuntimeId::Uar, Where::Leaves),
    ] {
        assert_eq!(admit(&snapshot, &runtime, words).map(|a| a.place), Ok(place), "{runtime}");
    }
    // Mail of an account that allowed nothing goes only to the one that stays here.
    let accounts = vec![ANA.to_string()];
    let mail = carried(AiKind::Answer, &accounts);
    assert!(admit(&snapshot, &RuntimeId::OnDevice, mail).is_ok());
    for runtime in [RuntimeId::ChatGpt, RuntimeId::ClaudeCode, RuntimeId::Codex, RuntimeId::Uar] {
        assert_eq!(admit(&snapshot, &runtime, mail), Err(Skip::NoConsent), "{runtime}");
    }
}

// ---------------------------------------------------------------------------------------------
// The desktop's walk over `compat` providers (fake servers on 127.0.0.1)
// ---------------------------------------------------------------------------------------------

/// A fake server that counts the connections it is sent and answers each chat with `text`.
fn server(text: &'static str) -> (u16, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            seen.fetch_add(1, Ordering::SeqCst);
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let mut buf = vec![0u8; 64 * 1024];
            let mut got = Vec::new();
            while let Ok(n) = stream.read(&mut buf) {
                if n == 0 {
                    break;
                }
                got.extend_from_slice(&buf[..n]);
                let text_so_far = String::from_utf8_lossy(&got);
                if let Some(head_end) = text_so_far.find("\r\n\r\n") {
                    let length = text_so_far[..head_end]
                        .lines()
                        .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                        .unwrap_or(0);
                    if got.len() >= head_end + 4 + length {
                        break;
                    }
                }
            }
            let request = String::from_utf8_lossy(&got).to_string();
            let body = if request.starts_with("GET /v1/models") {
                r#"{"object":"list","data":[{"id":"test-model","owned_by":"library"}]}"#.to_string()
            } else {
                serde_json::json!({"choices": [{"message": {"content": text}, "finish_reason": "stop"}]}).to_string()
            };
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    (port, count)
}

fn compat(id: &str, kind: ProviderKind, port: u16) -> ProviderConfig {
    let base_url = match kind {
        ProviderKind::Remote => format!("https://127.0.0.1:{port}/v1"),
        ProviderKind::Local | ProviderKind::Gateway => format!("http://127.0.0.1:{port}/v1"),
    };
    ProviderConfig {
        id: id.into(),
        kind,
        base_url,
        model: "test-model".into(),
        timeouts: Timeouts { connect_secs: 2, first_byte_secs: 5, idle_secs: 5, total_secs: 20 },
        ..ProviderConfig::default()
    }
}

#[test]
fn consent_a_gateway_no_account_allowed_is_never_connected_to_and_the_local_server_answers() {
    let (gateway_port, gateway_hits) = server("from the gateway");
    let (local_port, local_hits) = server("from this computer");
    let shared = Shared::open(None);
    shared
        .change(|s| {
            s.set_provider(compat("gateway", ProviderKind::Gateway, gateway_port))?;
            s.set_provider(compat("here", ProviderKind::Local, local_port))?;
            let chain =
                Chain::new([entry(RuntimeId::compat("gateway").unwrap()), entry(RuntimeId::compat("here").unwrap())]).map_err(|e| e.to_string())?;
            s.set_chain(chain);
            Ok(())
        })
        .unwrap();
    let mut owner = Owner::new(&Log::default(), &[]);
    let mut events = Vec::new();
    run_with(&answer_job(&[ANA]), &shared, &Broker::new(Duration::from_secs(1)), None, &mut owner, &mut |e| {
        events.push(e);
        true
    });
    assert_eq!(gateway_hits.load(Ordering::SeqCst), 0, "the gateway was never connected to");
    assert!(local_hits.load(Ordering::SeqCst) >= 1);
    assert!(
        matches!(&events[..], [ModelEvent::Done { text: Some(t), answered_by: Some(by), .. }] if t == "from this computer" && by.provider == "here"),
        "{events:?}"
    );
    let provenance = owner.provenance.unwrap();
    assert_eq!(provenance.answered_by, RuntimeId::compat("here").ok());
    assert_eq!(provenance.skipped().collect::<Vec<_>>(), [(&RuntimeId::compat("gateway").unwrap(), Skip::NoConsent)]);
}

#[test]
fn neg_consent_a_gateway_every_account_allowed_answers_first() {
    let (gateway_port, gateway_hits) = server("from the gateway");
    let (local_port, local_hits) = server("from this computer");
    let shared = Shared::open(None);
    shared
        .change(|s| {
            s.set_provider(compat("gateway", ProviderKind::Gateway, gateway_port))?;
            s.set_provider(compat("here", ProviderKind::Local, local_port))?;
            let chain =
                Chain::new([entry(RuntimeId::compat("gateway").unwrap()), entry(RuntimeId::compat("here").unwrap())]).map_err(|e| e.to_string())?;
            s.set_chain(chain);
            s.set_runtime_consent(ANA, &RuntimeId::compat("gateway").unwrap(), true)
        })
        .unwrap();
    let mut events = Vec::new();
    run(&answer_job(&[ANA]), &shared, &Broker::new(Duration::from_secs(1)), None, &mut |e| {
        events.push(e);
        true
    });
    assert!(gateway_hits.load(Ordering::SeqCst) >= 1);
    assert_eq!(local_hits.load(Ordering::SeqCst), 0);
    assert!(matches!(&events[..], [ModelEvent::Done { answered_by: Some(by), .. }] if by.provider == "gateway"), "{events:?}");
    assert!(crate::ai::run::desk().provenance(41).and_then(|p| p.answered_by).is_some());
}
