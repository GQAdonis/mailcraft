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
//! **The chain** (#ai, ai-provider-chain 5.3): the owner's primary first; when it fails before
//! the first byte of its answer, with a kind that may fall back, the confirmed on-device model
//! (only an address the owner confirmed, its identity checked: [`super::device`]), the request
//! made again for its window by the engine's own pure builder. Every answer says who gave it
//! (`answeredBy`); one made again names the messages of its new prompt (`sources`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mailcraft_ai::structured::{at_level, read_answer, steps_down};
use mailcraft_ai::{AiError, Carries, ChatRequest, Level, Message, ProviderConfig, ResponseFormat, Role, Timeouts};
use mailcraft_host::SecretStore;
use mailcraft_model::Id;
use mailcraft_model::ai::{AiKind, AnswerExtractor, AnsweredBy, ErrorKind, ModelEvent, ModelRequest, Role as ModelRole, clean_provider_text};
use mailcraft_ui_egui::ModelJob;

use super::device;
use super::keys::{self, Broker};
use super::route;
use super::settings::Shared;

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

/// How a request went on one provider: the whole answer (`None` when it came in pieces), and who
/// gave it.
struct Answered {
    text: Option<String>,
    by: AnsweredBy,
}

/// Why a request failed on one provider, and whether any of the answer had arrived first (after
/// that, nothing falls back).
struct Failed {
    failure: Failure,
    started: bool,
}

impl From<Failure> for Failed {
    /// A failure before anything was sent or came back.
    fn from(failure: Failure) -> Failed {
        Failed { failure, started: false }
    }
}

/// What the backup made of a request the primary failed.
enum Fallback {
    /// There is no backup to ask (none chosen, none confirmed, it changed, it isn't there, or the
    /// request can't be made again for it): the primary's failure stands.
    NotTaken,
    /// It answered; `sources` are the messages of the prompt made again for it, if it was.
    Answered(Answered, Option<Vec<Id>>),
    Failed(Failed),
}

/// Who `config` is, for "Answered by …".
fn answered_by(config: &ProviderConfig) -> AnsweredBy {
    AnsweredBy { provider: config.id.clone(), model: config.model.trim().to_string() }
}

/// Runs `job` with the owner's primary provider of `settings` (a kept key read through `broker`
/// from `store`), and, when that fails before any of its answer has come with a kind that may
/// fall back ([`ErrorKind::falls_back`]), with the confirmed on-device model, the request made
/// again for its context window. Each event goes to `emit`, which says whether anybody still
/// listens. Ends with one `done` (saying who answered) or `error`; a request that was called off
/// ends with neither (it is the request's Cancelled state, not a failure).
pub(crate) fn run(
    job: &ModelJob,
    settings: &Shared,
    broker: &Broker,
    store: Option<&Arc<dyn SecretStore>>,
    emit: &mut dyn FnMut(ModelEvent) -> bool,
) {
    let Failed { failure, started } = match attempt(job, settings, broker, store, emit) {
        Ok(answered) => return finish(answered, None, emit),
        Err(failed) => failed,
    };
    if started || !failure.kind.falls_back() || job.cancel.load(Ordering::Relaxed) {
        return give_up(failure, emit);
    }
    match fallback(job, settings, emit) {
        Fallback::NotTaken => give_up(failure, emit),
        Fallback::Answered(answered, sources) => {
            log::info!("the primary model provider failed ({}); the on-device model answered", failure.kind.name());
            finish(answered, sources, emit);
        }
        // What the owner is told is the backup's failure once some of its answer was shown, else
        // the primary's (the provider the owner set up).
        Fallback::Failed(backup) => give_up(if backup.started { backup.failure } else { failure }, emit),
    }
}

/// The end of an answered request.
fn finish(answered: Answered, sources: Option<Vec<Id>>, emit: &mut dyn FnMut(ModelEvent) -> bool) {
    emit(ModelEvent::Done { text: answered.text, answered_by: Some(answered.by), sources });
}

/// The end of a failed request; nothing for one called off.
fn give_up(failure: Failure, emit: &mut dyn FnMut(ModelEvent) -> bool) {
    if failure.kind != ErrorKind::Cancelled {
        emit(failure.event());
    }
}

/// The request, from the settings to the answer, on the primary. A provider made a gateway by the
/// owner's answer to the route check's question is taken from the top once more, as a gateway.
fn attempt(
    job: &ModelJob,
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
        let Some(config) = snapshot.active().cloned() else {
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
        let kept = snapshot.key_kept(&config.id);
        let config = keys::with_key(config, kept, broker, store).map_err(|_| Failure::new(ErrorKind::CredentialStore, STORE_SILENT))?;
        let text = send(&config, carries, &request, &job.request, &job.cancel, settings, &address, emit)?;
        return Ok(Answered { text, by: answered_by(&config) });
    }
    Err(Failure::new(ErrorKind::Settings, "The model provider's settings changed while the request waited; ask again.").into())
}

/// The request again, for the confirmed on-device model: an answer made again for its context
/// window, synchronously and with no wait (`ModelRequest::rebuilt_for`, the same id); anything
/// else as it was. Mail sent to it stays on this computer, so it needs no account's consent.
fn fallback(job: &ModelJob, settings: &Shared, emit: &mut dyn FnMut(ModelEvent) -> bool) -> Fallback {
    let Some((config, window)) = device::ready_backup(settings, Timeouts::default(), &job.cancel) else { return Fallback::NotTaken };
    let (asked, sources) = match &job.request.answer {
        Some(_) => match job.request.rebuilt_for(usize::try_from(window).unwrap_or(usize::MAX)) {
            Ok(rebuilt) => {
                let sources = rebuilt.sources.clone();
                (rebuilt, Some(sources))
            }
            // No room for the passages in its window: the primary's failure stands.
            Err(_) => return Fallback::NotTaken,
        },
        None => (job.request.clone(), None),
    };
    let request = match chat_request(&asked) {
        Ok(request) => request,
        Err(failure) => return Fallback::Failed(failure.into()),
    };
    let accounts: Vec<&str> = job.accounts.iter().map(String::as_str).collect();
    let carries = carries(asked.kind, &accounts);
    let address = shown_address(&config.base_url);
    match send(&config, carries, &request, &asked, &job.cancel, settings, &address, emit) {
        Ok(text) => Fallback::Answered(Answered { text, by: answered_by(&config) }, sources),
        Err(failed) => {
            device::after_error(settings);
            Fallback::Failed(failed)
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
    let outcome = if request.response_format.is_some() && asked.stream && asked.kind == AiKind::Answer {
        streamed_structured(config, carries, request, cancel, settings, &mut started, emit)
    } else if request.response_format.is_some() {
        structured(config, carries, request, cancel, settings)
    } else if asked.stream {
        let streamed = mailcraft_ai::chat_stream(config, carries, request, cancel, &mut |text: &str| {
            started |= !text.is_empty();
            emit(ModelEvent::Delta { text: text.to_string() })
        });
        // The pieces that came are the answer.
        streamed.map(|_| None)
    } else {
        mailcraft_ai::chat(config, carries, request, cancel).map(|answer| Some(answer.text))
    };
    let waited = started_at.elapsed();
    outcome.map_err(|error| {
        log::debug!("a model request failed after {} ms", waited.as_millis());
        Failed { failure: failure_of(&error, address, waited, started), started }
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
            })?;
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
            Err(error) => return Err(error),
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
