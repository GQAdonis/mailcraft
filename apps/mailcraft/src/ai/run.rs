// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! One model request, run on a thread of its own (#ai): the engine's request in the provider's
//! terms, sent to the provider the owner made active, its answer handed back piece by piece as
//! the engine's events.
//!
//! Every failure carries its kind ([`ErrorKind`]), every early exit included: no provider
//! (`not_configured`), settings that can't be used (`settings`), no consent (`no_consent`), the
//! key (`credential_store`), a request that can't be written (`malformed`, `too_large`). What the
//! owner is told is in our words alone: what kind of failure, a status number, how long was
//! waited, and the provider's address as the owner configured it (scheme, host and port). What
//! the provider itself said of an error goes apart, as `providerText`: data for the details,
//! never instructions.
//!
//! The order is the owner's: nothing is read from the credential store before the consent check
//! has passed, and a server set up as on this computer is sent nothing before its model list has
//! been checked ([`super::route`]).
//!
//! A structured request climbs down the ladder of `mailcraft_ai::structured` on the same
//! provider: any 400 to a request with a `response_format` asks again at the next level, and the
//! level that worked is kept for the provider for the session. A streamed answer with a schema
//! streams its JSON and hands on its text as it grows.
//!
//! **The chain** (#ai, ai-provider-chain 5.3, ai-runtime-chain 20.2–20.3): the owner's runtimes
//! in the order of the chain ([`walk`]), each admitted at its own hop (on, and allowed by every
//! account of the request when it `leaves`: [`route::admit`]) and asked only when its status says
//! it can be used. A failure before the first byte of an answer, with a kind that may fall back,
//! goes on as the owner's fallback policy says: `next` tries the next eligible runtime, `ask`
//! asks the owner first and starts nothing before the answer, `stop` shows the failure. A tool-use
//! stop is always told before any later hop. The confirmed on-device model (only an address the
//! owner confirmed, its identity checked: [`super::device`]) is asked with the request made again
//! for its window by the engine's own pure builder. Every answer says who gave it (`answeredBy`);
//! one made again names the messages of its new prompt (`sources`); the walk's provenance (who
//! answered, who was passed over or failed, and why) goes to the window's [`desk::Desk`].

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mailcraft_ai::structured::{at_level, read_answer, steps_down};
use mailcraft_ai::{AiError, Carries, ChatRequest, Level, Message, ProviderConfig, ResponseFormat, Role, Timeouts};
use mailcraft_host::SecretStore;
use mailcraft_model::Id;
use mailcraft_model::ai::{
    AiKind, AnswerExtractor, AnsweredBy, ChainEntry, ErrorKind, Fallback, ModelEvent, ModelRequest, Role as ModelRole, RuntimeId, clean_provider_text,
};
use mailcraft_ui_egui::ModelJob;
use serde_json::{Value, json};

use super::device;
use super::keys::{self, Broker};
use super::route::{self, Admitted, HopNote, HopOutcome, Provenance, Skip, Unusable};
use super::settings::{AiSettings, Shared};

/// The CLIs, the ChatGPT plan and an external UAR, each through its adapter.
pub(crate) mod cli;
/// What the walks tell and ask the owner, for the window.
pub(crate) mod desk;

pub(crate) use desk::{DeskWatch, desk};

/// Longest schema taken from a request, in bytes (the engine's own cap).
const MAX_SCHEMA_BYTES: usize = 16 * 1024;
/// Longest address shown in an error.
const MAX_SHOWN_ADDRESS: usize = 200;
/// How many times a structured answer that isn't JSON is asked for at the same level.
const TRIES_A_LEVEL: usize = 2;
/// What the owner is told when the credential store doesn't answer.
pub(crate) const STORE_SILENT: &str =
    "The system's credential store didn't answer. It may be asking you to allow MailCraft: answer it, then ask again.";

/// Why a request ended without an answer: its kind, our words for the owner, and what the
/// provider itself said, when it said something. (`Debug` shows the kind and lengths alone.)
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Failure {
    pub(crate) kind: ErrorKind,
    pub(crate) message: String,
    pub(crate) provider_text: Option<String>,
}

impl std::fmt::Debug for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Failure").field("kind", &self.kind).field("bytes", &self.message.len()).finish()
    }
}

impl Failure {
    pub(crate) fn new(kind: ErrorKind, message: impl Into<String>) -> Failure {
        Failure { kind, message: message.into(), provider_text: None }
    }

    /// What the host hands the engine.
    pub(crate) fn event(self) -> ModelEvent {
        ModelEvent::Error { kind: self.kind, message: self.message, provider_text: self.provider_text }
    }
}

/// The provider's request for the engine's `request`: its messages, its length and temperature,
/// and, when it carries a schema, `response_format: json_schema`.
pub(crate) fn chat_request(request: &ModelRequest) -> Result<ChatRequest, Failure> {
    let messages = request
        .messages
        .iter()
        .map(|m| Message {
            role: match m.role {
                ModelRole::System => Role::System,
                ModelRole::User => Role::User,
                ModelRole::Assistant => Role::Assistant,
            },
            content: m.content.clone(),
        })
        .collect();
    let response_format = match &request.schema {
        None => None,
        Some(text) => {
            if text.len() > MAX_SCHEMA_BYTES {
                return Err(Failure::new(ErrorKind::TooLarge, "The request's answer format is too large."));
            }
            let not_schema = || Failure::new(ErrorKind::Malformed, "The request's answer format isn't a JSON schema.");
            let schema: serde_json::Value = serde_json::from_str(text).map_err(|_| not_schema())?;
            if !schema.is_object() {
                return Err(not_schema());
            }
            let name = match request.kind {
                AiKind::Narrow => "mail_search",
                AiKind::Answer => "mail_answer",
            };
            Some(ResponseFormat::JsonSchema { name: name.into(), schema, strict: true })
        }
    };
    Ok(ChatRequest { messages, max_tokens: Some(request.max_tokens), temperature: Some(request.temperature), seed: None, response_format })
}

/// What a request of `kind` carries, for the owner's consent: a search proposed from the owner's
/// words carries nothing of any message; an answer carries the mail of `accounts`.
pub(crate) fn carries<'a>(kind: AiKind, accounts: &'a [&'a str]) -> Carries<'a> {
    match kind {
        AiKind::Narrow => Carries::OwnerTextOnly,
        AiKind::Answer => Carries::MailOf(accounts),
    }
}

/// The provider's address as it is shown: scheme, host and port, nothing else of it.
pub(crate) fn shown_address(base: &str) -> String {
    let base = base.trim();
    let Some((scheme, rest)) = base.split_once("://") else { return "(no address)".into() };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let authority: String = authority.chars().filter(char::is_ascii_graphic).take(MAX_SHOWN_ADDRESS).collect();
    let scheme: String = scheme.chars().filter(char::is_ascii_alphabetic).take(8).collect::<String>().to_ascii_lowercase();
    format!("{scheme}://{authority}")
}

/// Why a request failed, as the owner is told: never what a server or a model said.
pub(crate) fn said(error: &AiError, address: &str, waited: Duration) -> String {
    match error {
        AiError::NotConfigured => "No model provider is set up for this.".into(),
        AiError::Config(_) => format!("The settings of the model provider at {address} can't be used."),
        AiError::NoConsent => "Nothing was sent: the model provider on another computer hasn't been allowed this account's mail.".into(),
        AiError::Refused(_) => {
            format!("The model provider at {address} was not contacted: without encryption only a server on this computer is spoken to.")
        }
        AiError::Unreachable(_) => format!("Couldn't reach the model provider at {address}."),
        AiError::Certificate(_) => format!("The certificate of the model provider at {address} wasn't accepted."),
        AiError::TimedOut(_) => format!("The model provider at {address} took too long (gave up after {} s).", waited.as_secs()),
        AiError::Cancelled => "The request was stopped.".into(),
        AiError::Status { status, .. } => format!("The model provider at {address} answered with status {status}."),
        AiError::Provider(_) => format!("The model provider at {address} reported an error."),
        AiError::Malformed(_) => format!("The answer of the model provider at {address} couldn't be read."),
        AiError::TooLarge => format!("The answer of the model provider at {address} was too long."),
    }
}

/// The kind of `error`; `shown` says whether some of the answer had already reached the owner (a
/// time-out after it is not one before the first byte).
pub(crate) fn kind_of(error: &AiError, shown: bool) -> ErrorKind {
    match error {
        AiError::NotConfigured => ErrorKind::NotConfigured,
        AiError::Config(_) | AiError::Refused(_) => ErrorKind::Settings,
        AiError::NoConsent => ErrorKind::NoConsent,
        AiError::Unreachable(_) => ErrorKind::Unreachable,
        AiError::Certificate(_) => ErrorKind::Certificate,
        AiError::TimedOut(_) => ErrorKind::TimedOut { before_first_byte: !shown },
        AiError::Cancelled => ErrorKind::Cancelled,
        AiError::Status { status, .. } => ErrorKind::Refused { status: *status },
        // An error in a successful answer: the provider refused, in its own words.
        AiError::Provider(_) => ErrorKind::Refused { status: 200 },
        AiError::Malformed(_) => ErrorKind::Malformed,
        AiError::TooLarge => ErrorKind::TooLarge,
    }
}

/// What the provider itself said of `error`, cleaned: data for the details, never instructions.
fn provider_text_of(error: &AiError) -> Option<String> {
    let said = match error {
        AiError::Status { message, .. } | AiError::Provider(message) => message.as_str(),
        _ => return None,
    };
    Some(clean_provider_text(said)).filter(|text| !text.is_empty())
}

/// `error` as the request's failure, in our words.
pub(crate) fn failure_of(error: &AiError, address: &str, waited: Duration, shown: bool) -> Failure {
    Failure { kind: kind_of(error, shown), message: said(error, address, waited), provider_text: provider_text_of(error) }
}

/// How a request went on one runtime: the whole answer (`None` when it came in pieces), who gave
/// it, and, for a request made again for the runtime's window, the messages of its new prompt.
pub(crate) struct Answered {
    pub(crate) text: Option<String>,
    pub(crate) by: AnsweredBy,
    pub(crate) sources: Option<Vec<Id>>,
}

/// Why a request failed on one runtime, and whether any of the answer had arrived first (after
/// that, nothing falls back).
pub(crate) struct Failed {
    pub(crate) failure: Failure,
    pub(crate) started: bool,
}

impl From<Failure> for Failed {
    /// A failure before anything was sent or came back.
    fn from(failure: Failure) -> Failed {
        Failed { failure, started: false }
    }
}

/// How one hop of the chain ended.
pub(crate) enum HopEnd {
    /// It answered.
    Answered(Answered),
    /// It was asked and failed.
    Failed(Failed),
    /// At its hop it turned out it can't be used (an on-device model that changed, a window too
    /// small for the request): nothing was sent to it.
    Unusable(Unusable),
}

/// One hop: the entry the settings hold now, where it sends text, and the job.
pub(crate) struct Hop<'a> {
    pub(crate) admitted: &'a Admitted,
    pub(crate) job: &'a ModelJob,
}

/// The runtimes a walk asks: their status, read at their hop and only once [`route::admit`] let
/// the hop through, and the request itself. The desktop's are [`HostRuntimes`]; tests use fakes.
pub(crate) trait Runtimes {
    /// Whether the runtime of `entry` can be used now.
    fn status(&mut self, entry: &ChainEntry, settings: &AiSettings, cancel: &AtomicBool) -> Result<(), Unusable>;
    /// Ask it. A runtime that runs as a process returns only once its process group is gone.
    fn ask(&mut self, hop: &Hop<'_>, settings: &Shared, emit: &mut dyn FnMut(ModelEvent) -> bool) -> HopEnd;
}

/// The question put to the owner under the fallback policy `ask`: "‹failed› couldn't answer
/// (‹reason›). Try ‹next›?" Only the owner answers it: no command, control method or agent can.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FallbackQuestion {
    pub(crate) request: u64,
    pub(crate) failed: RuntimeId,
    pub(crate) reason: ErrorKind,
    pub(crate) next: RuntimeId,
}

#[allow(dead_code, reason = "the window reads it once the integrator wires the chain into the window (ai-runtime-chain 20.5, 20.8)")]
impl FallbackQuestion {
    /// The question in English, the window's `tl!` source text with its two names and reason.
    pub(crate) const WORDS: &'static str = "{failed} couldn't answer ({reason}). Try {next}?";

    /// As the window reads it.
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "request": self.request,
            "failed": self.failed.to_string(),
            "reason": self.reason.name(),
            "next": self.next.to_string(),
            "words": FallbackQuestion::WORDS,
        })
    }
}

/// The owner's answer to a [`FallbackQuestion`]. A question closed, timed out or called off is a no.
#[allow(dead_code, reason = "the window reads it once the integrator wires the chain into the window (ai-runtime-chain 20.5, 20.8)")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OwnerAnswer {
    Yes,
    No,
}

/// What the walk tells the owner while it goes, and asks: the desktop's is [`DeskWatch`].
pub(crate) trait Watch {
    /// A runtime was stopped for trying to use a tool: told before any later hop starts.
    fn tool_use_stopped(&mut self, request: u64, runtime: &RuntimeId);
    /// Ask the owner whether to try the next runtime. Nothing starts until this returns.
    fn ask_owner(&mut self, question: &FallbackQuestion, cancel: &AtomicBool) -> OwnerAnswer;
    /// The walk is over: who answered and who was passed over.
    fn provenance(&mut self, provenance: &Provenance);
}

/// Who `config` is, for "Answered by …".
fn answered_by(config: &ProviderConfig) -> AnsweredBy {
    AnsweredBy { provider: config.id.clone(), model: config.model.trim().to_string() }
}

/// Runs `job` down the owner's chain ([`walk`]) with the desktop's runtimes: a kept key read
/// through `broker` from `store`. Each event goes to `emit`, which says whether anybody still
/// listens. Ends with one `done` (saying who answered) or `error`; a request that was called off
/// ends with neither (it is the request's Cancelled state, not a failure).
#[cfg_attr(not(test), allow(dead_code, reason = "the window's requests go through run_with, which wakes the window"))]
pub(crate) fn run(
    job: &ModelJob,
    settings: &Shared,
    broker: &Broker,
    store: Option<&Arc<dyn SecretStore>>,
    emit: &mut dyn FnMut(ModelEvent) -> bool,
) {
    let mut watch = DeskWatch::new(desk(), None);
    run_with(job, settings, broker, store, &mut watch, emit);
}

/// [`run`], telling and asking the owner through `watch`.
pub(crate) fn run_with(
    job: &ModelJob,
    settings: &Shared,
    broker: &Broker,
    store: Option<&Arc<dyn SecretStore>>,
    watch: &mut dyn Watch,
    emit: &mut dyn FnMut(ModelEvent) -> bool,
) {
    let mut runtimes = HostRuntimes::new(broker, store);
    walk(job, settings, &mut runtimes, watch, emit);
}

/// How a walk ended.
enum Ended {
    Answered,
    Failed,
    Cancelled,
}

/// The walk of the owner's chain for `job` (ai-runtime-chain 20.2–20.3). For each entry, in the
/// order the chain had when the walk began, and with the settings read again at that hop:
///
/// 1. [`route::admit`]: an entry that is off or gone, or that `leaves` while an account of the
///    request hasn't allowed it, is passed over and never sent anything, not even a status probe;
/// 2. the runtime's status: one that can't be used now is passed over;
/// 3. after a failed hop under `ask`, the owner is asked whether to try this one, and nothing
///    starts before the answer (a no ends the walk);
/// 4. the hop is admitted once more (consent may have been withdrawn meanwhile), then asked.
///
/// A failure ends the walk when some of its answer had come, when its kind doesn't fall back,
/// when the request was called off (the hop returns only once its process group is gone), or
/// under `stop`; under `next` the next eligible entry is tried. A tool-use stop is told to the
/// owner before any later hop, whatever the policy. What the owner is told at the end is the
/// first failure (the runtime the owner put first), unless a later one had started its answer;
/// with nothing asked at all, the first reason a runtime was passed over.
pub(crate) fn walk(
    job: &ModelJob,
    settings: &Shared,
    runtimes: &mut dyn Runtimes,
    watch: &mut dyn Watch,
    emit: &mut dyn FnMut(ModelEvent) -> bool,
) -> Provenance {
    let request = job.request.id;
    let carries = route::carried(job.request.kind, &job.accounts);
    let order = settings.read(|s| s.chain.ids());
    let mut provenance = Provenance { request, ..Provenance::default() };
    let mut first: Option<Failed> = None;
    let mut waiting: Option<(RuntimeId, ErrorKind)> = None;
    let cancelled = || job.cancel.load(Ordering::Relaxed);
    let skip = |provenance: &mut Provenance, runtime: &RuntimeId, skip: Skip| {
        // The kind and the reason only (never an id, an address or anything a runtime said).
        log::info!("the chain passed over a {} runtime: {skip:?}", runtime.kind().name());
        provenance.notes.push(HopNote { runtime: runtime.clone(), outcome: HopOutcome::Skipped(skip) });
    };
    let mut ended = Ended::Failed;
    for runtime in &order {
        if cancelled() {
            ended = Ended::Cancelled;
            break;
        }
        let admitted = match route::admit(&settings.snapshot(), runtime, carries) {
            Ok(admitted) => admitted,
            Err(why) => {
                skip(&mut provenance, runtime, why);
                continue;
            }
        };
        if let Err(why) = runtimes.status(&admitted.entry, &settings.snapshot(), &job.cancel) {
            skip(&mut provenance, runtime, Skip::Unusable(why));
            continue;
        }
        if let Some((failed, reason)) = waiting.take() {
            let question = FallbackQuestion { request, failed, reason, next: runtime.clone() };
            let answer = watch.ask_owner(&question, &job.cancel);
            if cancelled() {
                ended = Ended::Cancelled;
                break;
            }
            if answer == OwnerAnswer::No {
                break;
            }
        }
        // Read again: the status probe or the owner's question may have taken a while.
        let admitted = match route::admit(&settings.snapshot(), runtime, carries) {
            Ok(admitted) => admitted,
            Err(why) => {
                skip(&mut provenance, runtime, why);
                continue;
            }
        };
        match runtimes.ask(&Hop { admitted: &admitted, job }, settings, emit) {
            HopEnd::Answered(answered) => {
                provenance.answered_by = Some(runtime.clone());
                if !provenance.notes.is_empty() {
                    // Kinds and reasons only: no id, address or anything a runtime said.
                    let why: Vec<String> = provenance.notes.iter().map(|n| format!("{}: {:?}", n.runtime.kind().name(), n.outcome)).collect();
                    log::info!(
                        "the chain passed over {} runtime(s) ({}); a {} runtime answered",
                        provenance.notes.len(),
                        why.join(", "),
                        runtime.kind().name()
                    );
                }
                finish(answered, emit);
                ended = Ended::Answered;
                break;
            }
            HopEnd::Unusable(why) => skip(&mut provenance, runtime, Skip::Unusable(why)),
            HopEnd::Failed(failed) => {
                let kind = failed.failure.kind;
                if kind == ErrorKind::ToolUseStopped {
                    watch.tool_use_stopped(request, runtime);
                }
                log::info!("a {} runtime of the chain failed: {}", runtime.kind().name(), kind.name());
                provenance.notes.push(HopNote { runtime: runtime.clone(), outcome: HopOutcome::Failed(kind) });
                let ends = failed.started || !kind.falls_back() || cancelled();
                if first.is_none() || failed.started {
                    first = Some(failed);
                }
                if kind == ErrorKind::Cancelled || cancelled() {
                    ended = Ended::Cancelled;
                    break;
                }
                if ends {
                    break;
                }
                match settings.read(|s| s.fallback) {
                    Fallback::Next => {}
                    Fallback::Ask => waiting = Some((runtime.clone(), kind)),
                    Fallback::Stop => break,
                }
            }
        }
    }
    match ended {
        Ended::Answered | Ended::Cancelled => {}
        Ended::Failed => give_up(first.map_or_else(|| nothing_answered(&provenance), |f| f.failure), emit),
    }
    watch.provenance(&provenance);
    provenance
}

/// The failure of a walk that asked no runtime: the first reason one was passed over that says
/// more than "off" or "not set up".
fn nothing_answered(provenance: &Provenance) -> Failure {
    let telling = provenance.skipped().map(|(_, skip)| skip).find(|skip| !matches!(skip, Skip::Disabled | Skip::Unusable(Unusable::NotSetUp)));
    match telling {
        Some(Skip::NoConsent) => Failure::new(ErrorKind::NoConsent, said(&AiError::NoConsent, "", Duration::ZERO)),
        Some(Skip::Unusable(why)) => Failure::new(why.kind(), why.kind().owner_words()),
        Some(Skip::Disabled) | None => Failure::new(ErrorKind::NotConfigured, "No model provider is set up."),
    }
}

/// The end of an answered request.
fn finish(answered: Answered, emit: &mut dyn FnMut(ModelEvent) -> bool) {
    emit(ModelEvent::Done { text: answered.text, answered_by: Some(answered.by), sources: answered.sources });
}

/// The end of a failed request; nothing for one called off.
fn give_up(failure: Failure, emit: &mut dyn FnMut(ModelEvent) -> bool) {
    if failure.kind != ErrorKind::Cancelled {
        emit(failure.event());
    }
}

/// The desktop's runtimes: `compat` providers and the confirmed on-device model through
/// `mailcraft-ai`, and the CLIs, the ChatGPT plan and an external UAR through their adapters
/// ([`cli`]).
pub(crate) struct HostRuntimes<'a> {
    broker: &'a Broker,
    store: Option<&'a Arc<dyn SecretStore>>,
    cli: cli::Cli,
}

impl<'a> HostRuntimes<'a> {
    pub(crate) fn new(broker: &'a Broker, store: Option<&'a Arc<dyn SecretStore>>) -> HostRuntimes<'a> {
        HostRuntimes { broker, store, cli: cli::Cli::default() }
    }
}

impl Runtimes for HostRuntimes<'_> {
    fn status(&mut self, entry: &ChainEntry, settings: &AiSettings, cancel: &AtomicBool) -> Result<(), Unusable> {
        match entry.runtime() {
            RuntimeId::OnDevice => settings.backup_binding().map(|_| ()).ok_or(Unusable::NotSetUp),
            RuntimeId::Compat(id) => settings.provider(id.as_str()).map(|_| ()).ok_or(Unusable::NotSetUp),
            RuntimeId::ChatGpt | RuntimeId::ClaudeCode | RuntimeId::Codex | RuntimeId::Uar => {
                self.cli.status(entry, settings, self.store.is_some(), cancel)
            }
        }
    }

    fn ask(&mut self, hop: &Hop<'_>, settings: &Shared, emit: &mut dyn FnMut(ModelEvent) -> bool) -> HopEnd {
        let entry = &hop.admitted.entry;
        match entry.runtime() {
            RuntimeId::OnDevice => on_device(hop.job, settings, emit),
            RuntimeId::Compat(id) => match attempt(hop.job, id.as_str(), settings, self.broker, self.store, emit) {
                Ok(answered) => HopEnd::Answered(answered),
                Err(failed) => HopEnd::Failed(failed),
            },
            RuntimeId::ChatGpt | RuntimeId::ClaudeCode | RuntimeId::Codex | RuntimeId::Uar => {
                let by =
                    AnsweredBy { provider: entry.runtime().to_string(), model: entry.model().map(|m| m.as_str().to_string()).unwrap_or_default() };
                let held = self.broker.held_secrets();
                match self.cli.ask(entry, &hop.job.request, settings, self.store, &held, &hop.job.cancel) {
                    Ok(text) => HopEnd::Answered(Answered { text: Some(text), by, sources: None }),
                    Err(kind) => HopEnd::Failed(Failure::new(kind, kind.owner_words()).into()),
                }
            }
        }
    }
}

/// The request, from the settings to the answer, on `compat` provider `id`. A provider made a
/// gateway by the owner's answer to the route check's question is taken from the top once more,
/// as a gateway (and its consent checked again).
fn attempt(
    job: &ModelJob,
    id: &str,
    settings: &Shared,
    broker: &Broker,
    store: Option<&Arc<dyn SecretStore>>,
    emit: &mut dyn FnMut(ModelEvent) -> bool,
) -> Result<Answered, Failed> {
    let request = chat_request(&job.request)?;
    let accounts: Vec<&str> = job.accounts.iter().map(String::as_str).collect();
    let carries = carries(job.request.kind, &accounts);
    for _ in 0..2 {
        let snapshot = settings.snapshot();
        let Some(config) = snapshot.provider(id).cloned() else {
            return Err(Failure::new(ErrorKind::NotConfigured, "No model provider is set up.").into());
        };
        let address = shown_address(&config.base_url);
        // Before anything is read or sent: settings that can be used, and the owner's consent.
        config.check().map_err(|e| failure_of(&e, &address, Duration::ZERO, false))?;
        config.may_send(carries).map_err(|e| failure_of(&e, &address, Duration::ZERO, false))?;
        match route::check(&config, settings, &job.cancel)? {
            route::Checked::Passed => {}
            route::Checked::Reclassified => continue,
        }
        let kept = snapshot.kept_key(&config.id);
        let config = keys::with_key(config, kept, broker, store).map_err(|_| Failure::new(ErrorKind::CredentialStore, STORE_SILENT))?;
        let text = send(&config, carries, &request, &job.request, &job.cancel, settings, &address, emit)?;
        return Ok(Answered { text, by: answered_by(&config), sources: None });
    }
    Err(Failure::new(ErrorKind::Settings, "The model provider's settings changed while the request waited; ask again.").into())
}

/// The request to the confirmed on-device model: an answer made again for its context window,
/// synchronously and with no wait (`ModelRequest::rebuilt_for`, the same id); anything else as it
/// was. Mail sent to it stays on this computer, so it needs no account's consent. A model that
/// isn't ready (no confirmation, or its identity changed) or a window that can't hold the request
/// is sent nothing.
fn on_device(job: &ModelJob, settings: &Shared, emit: &mut dyn FnMut(ModelEvent) -> bool) -> HopEnd {
    let Some((config, window)) = device::ready_backup(settings, Timeouts::default(), &job.cancel) else {
        return HopEnd::Unusable(Unusable::NotRunning);
    };
    let (asked, sources) = match &job.request.answer {
        Some(_) => match job.request.rebuilt_for(usize::try_from(window).unwrap_or(usize::MAX)) {
            Ok(rebuilt) => {
                let sources = rebuilt.sources.clone();
                (rebuilt, Some(sources))
            }
            Err(_) => return HopEnd::Unusable(Unusable::NoRoom),
        },
        None => (job.request.clone(), None),
    };
    let request = match chat_request(&asked) {
        Ok(request) => request,
        Err(failure) => return HopEnd::Failed(failure.into()),
    };
    let accounts: Vec<&str> = job.accounts.iter().map(String::as_str).collect();
    let carries = carries(asked.kind, &accounts);
    let address = shown_address(&config.base_url);
    match send(&config, carries, &request, &asked, &job.cancel, settings, &address, emit) {
        Ok(text) => HopEnd::Answered(Answered { text, by: answered_by(&config), sources }),
        Err(failed) => {
            device::after_error(settings);
            HopEnd::Failed(failed)
        }
    }
}

/// The request to `config`, as `asked` wants it: a streamed answer with a schema streamed through
/// the extractor (down the ladder), another structured request whole (down the ladder), a
/// streamed one in pieces, or the whole answer.
#[allow(clippy::too_many_arguments)]
fn send(
    config: &ProviderConfig,
    carries: Carries<'_>,
    request: &ChatRequest,
    asked: &ModelRequest,
    cancel: &AtomicBool,
    settings: &Shared,
    address: &str,
    emit: &mut dyn FnMut(ModelEvent) -> bool,
) -> Result<Option<String>, Failed> {
    let started_at = Instant::now();
    let mut started = false;
    // Whether the stream itself broke (as against a whole answer whose JSON can't be read).
    let mut broke = false;
    let outcome = if request.response_format.is_some() && asked.stream && asked.kind == AiKind::Answer {
        streamed_structured(config, carries, request, cancel, settings, &mut started, &mut broke, emit)
    } else if request.response_format.is_some() {
        structured(config, carries, request, cancel, settings)
    } else if asked.stream {
        let streamed = mailcraft_ai::chat_stream(config, carries, request, cancel, &mut |text: &str| {
            started |= !text.is_empty();
            emit(ModelEvent::Delta { text: text.to_string() })
        });
        broke = streamed.is_err();
        // The pieces that came are the answer.
        streamed.map(|_| None)
    } else {
        mailcraft_ai::chat(config, carries, request, cancel).map(|answer| Some(answer.text))
    };
    let waited = started_at.elapsed();
    outcome.map_err(|error| {
        log::debug!("a model request failed after {} ms", waited.as_millis());
        let mut failure = failure_of(&error, address, waited, started);
        if broke && started && failure.kind == ErrorKind::Malformed {
            failure.kind = ErrorKind::BrokenOff;
            failure.message = ErrorKind::BrokenOff.owner_words().to_string();
        }
        Failed { failure, started }
    })
}

/// A streamed answer asked for as JSON (#ai, ai-cited-answers-v2 6.4): at the level the provider
/// last answered at, the JSON streamed and its `answer` text handed on as it grows
/// ([`AnswerExtractor`]), then the whole document (the engine checks it); a 400 before any of it
/// steps down the ladder. At the plain level the answer streams as plain text, as before, with no
/// format and the request's own words. Nothing is asked again once some of the answer came.
fn streamed_structured(
    config: &ProviderConfig,
    carries: Carries<'_>,
    request: &ChatRequest,
    cancel: &AtomicBool,
    settings: &Shared,
    started: &mut bool,
    broke: &mut bool,
    emit: &mut dyn FnMut(ModelEvent) -> bool,
) -> Result<Option<String>, AiError> {
    let key = ladder_key(config);
    let mut level = settings.ladders().start(&key);
    loop {
        if level == Level::Plain {
            let plain = ChatRequest { response_format: None, ..request.clone() };
            mailcraft_ai::chat_stream(config, carries, &plain, cancel, &mut |text: &str| {
                *started |= !text.is_empty();
                emit(ModelEvent::Delta { text: text.to_string() })
            })
            .inspect_err(|_| *broke = true)?;
            settings.ladders().keep(&key, level);
            return Ok(None);
        }
        let mut extractor = AnswerExtractor::new();
        let streamed = mailcraft_ai::chat_stream_json(config, carries, &at_level(request, level), cancel, &mut |piece: &str| {
            *started |= !piece.is_empty();
            let text = extractor.push(piece);
            text.is_empty() || emit(ModelEvent::Delta { text })
        });
        match streamed {
            Ok(whole) => {
                settings.ladders().keep(&key, level);
                return whole_json(&whole, level).map(Some);
            }
            Err(error) if !*started && steps_down(&error, level) => match level.next() {
                Some(next) => level = next,
                None => return Err(error),
            },
            Err(error) => {
                *broke = true;
                return Err(error);
            }
        }
    }
}

/// The whole of a streamed structured answer as the engine is handed it: the JSON object (taken
/// out of a fence, compact) when it is one; what was written, when it begins as one (the engine
/// finds it unreadable, keeps what was shown and says so); else a malformed answer.
fn whole_json(whole: &str, level: Level) -> Result<String, AiError> {
    match read_answer(whole, level) {
        Ok(value) => Ok(value.to_string()),
        Err(_) if whole.trim_start().starts_with('{') => Ok(whole.to_string()),
        Err(error) => Err(error),
    }
}

/// The name the ladder keeps a provider's level under: its id, address and model.
fn ladder_key(config: &ProviderConfig) -> String {
    format!("{}\n{}\n{}", config.id, config.base_url.trim().to_ascii_lowercase(), config.model.trim())
}

/// A structured answer as compact JSON, asked at the level the provider last answered at and down
/// the ladder from there on a 400. At each level an answer that isn't JSON is asked for once more
/// (providers differ in how well they keep to a schema); then a clear error.
fn structured(
    config: &ProviderConfig,
    carries: Carries<'_>,
    request: &ChatRequest,
    cancel: &AtomicBool,
    settings: &Shared,
) -> Result<Option<String>, AiError> {
    let key = ladder_key(config);
    let mut level = settings.ladders().start(&key);
    loop {
        match structured_at(config, carries, request, cancel, level) {
            Ok(value) => {
                settings.ladders().keep(&key, level);
                return Ok(Some(value.to_string()));
            }
            Err(error) if steps_down(&error, level) => match level.next() {
                Some(next) => level = next,
                None => return Err(error),
            },
            Err(error) => return Err(error),
        }
    }
}

/// One level of the ladder: the answer as JSON, or why not.
fn structured_at(
    config: &ProviderConfig,
    carries: Carries<'_>,
    request: &ChatRequest,
    cancel: &AtomicBool,
    level: Level,
) -> Result<serde_json::Value, AiError> {
    let asked = at_level(request, level);
    let mut last = AiError::Malformed(String::new());
    for _ in 0..TRIES_A_LEVEL {
        let answer = mailcraft_ai::chat(config, carries, &asked, cancel)?;
        match read_answer(&answer.text, level) {
            Ok(value) => return Ok(value),
            Err(error @ (AiError::Malformed(_) | AiError::TooLarge)) => last = error,
            Err(error) => return Err(error),
        }
    }
    Err(last)
}
