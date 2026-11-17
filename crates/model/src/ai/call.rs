// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Model calls, as the engine and a host exchange them: the engine never talks to a model.
//!
//! As for a Send/Receive (`sync.request`, `sync.apply`): `ai.request` records a
//! [`ModelRequest`] among the [`Requests`] under way and hands it to the host; the host sends it
//! to the model the owner configured and hands back what comes, piece by piece
//! ([`ModelEvent`]: some more text, the end, or an error) through `ai.apply`; `ai.cancel` calls
//! it off, and whatever comes for it afterwards is let go of.
//!
//! **What a model says is hostile input.** Its text is kept only after [`clean_text`]: control
//! characters (but the line break and the tab) and the characters that reverse the direction of
//! text are dropped, and it is capped at [`MAX_RESPONSE_BYTES`]. Nothing of it is run, fetched,
//! sent or taken for a command: it is text for the owner to read.
//!
//! **Bounded:** at most [`MAX_LIVE`] requests under way and [`MAX_KEPT`] kept (the oldest
//! finished go first), [`MAX_MESSAGES`] messages of at most [`MAX_PROMPT_BYTES`] in all in a
//! request, [`MAX_SOURCES`] source messages, and [`MAX_EVENTS`] events a request.
//!
//! **An answer request carries what its prompt was made from** ([`ModelRequest::answer`]): when
//! the provider that was to answer fails before its first byte, the host makes the prompt again
//! for its backup's context window ([`ModelRequest::rebuilt_for`], the same pure builder the
//! engine used) and says, with the end, which messages that prompt held
//! (`ModelEvent::Done::sources`) and who answered ([`AnsweredBy`]).
//!
//! **An answer asked for as JSON** (a request of kind `answer` with a schema) is read at its end
//! by [`super::read_answer_json`]: its text and its citations are kept apart. A document that
//! can't be read fails the request as [`ErrorKind::Malformed`], keeping what was shown.

use serde::{Deserialize, Serialize};

use super::AiError;
use super::answer::{answer_schema, read_answer_json};
use super::extract::extract_answer;
use super::prompt::{AnswerInput, MAX_MARKER_BYTES, MAX_QUESTION_CHARS, answer_tokens_for};
use crate::Id;

/// Most requests under way at once.
pub const MAX_LIVE: usize = 4;
/// Most requests kept, under way or finished.
pub const MAX_KEPT: usize = 16;
/// Most messages of a conversation with a model.
pub const MAX_MESSAGES: usize = 32;
/// Most bytes of all the messages of a request together.
pub const MAX_PROMPT_BYTES: usize = 512 * 1024;
/// Most messages of the mailbox whose text a request carries.
pub const MAX_SOURCES: usize = 64;
/// Most bytes of a model's answer kept.
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024;
/// Most events taken for one request.
pub const MAX_EVENTS: u32 = 20_000;
/// Longest error kept, in bytes.
const MAX_ERROR_BYTES: usize = 600;
/// Longest of a provider's own words about an error kept, in bytes ([`clean_provider_text`]).
pub const MAX_PROVIDER_TEXT_BYTES: usize = 300;
/// Longest JSON schema a request carries, in bytes.
const MAX_SCHEMA_BYTES: usize = 16 * 1024;
/// Longest provider id or model name of [`AnsweredBy`] kept, in bytes.
pub const MAX_ANSWERED_BY_BYTES: usize = 128;
/// What a request for an answer whose JSON can't be read fails with: the words for the owner.
pub const SOURCES_UNREADABLE: &str = "MailCraft couldn't read which messages this came from.";
/// Most tokens a request lets the model write, and what it lets when it doesn't say.
const MAX_TOKENS: u32 = 8_192;
const DEFAULT_TOKENS: u32 = 1_024;

/// What a request is for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AiKind {
    /// An answer to the owner's question, from the messages given.
    #[default]
    Answer,
    /// A search proposed from the owner's words, for the owner to accept.
    Narrow,
}

impl AiKind {
    pub fn parse(s: &str) -> Option<AiKind> {
        match s {
            "answer" => Some(AiKind::Answer),
            "narrow" => Some(AiKind::Narrow),
            _ => None,
        }
    }
}

/// Who says a message of a conversation with a model.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    System,
    #[default]
    User,
    Assistant,
}

impl Role {
    pub fn parse(s: &str) -> Option<Role> {
        match s {
            "system" => Some(Role::System),
            "user" => Some(Role::User),
            "assistant" => Some(Role::Assistant),
            _ => None,
        }
    }
}

/// One message of a conversation with a model.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

/// What a host is to send to a model. Made by the engine ([`Requests::start`]); the host maps it
/// to its provider's request.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ModelRequest {
    /// The engine's number for it, which `ai.apply` and `ai.cancel` name.
    pub id: u64,
    pub kind: AiKind,
    pub messages: Vec<ChatMessage>,
    /// The messages of the mailbox whose text is in `messages`: the only ones an answer may
    /// cite ([`super::validate_citations`]).
    pub sources: Vec<Id>,
    /// The answer is wanted as it is written (streamed), not only at the end.
    pub stream: bool,
    /// Most tokens the model may write (1 to 8,192).
    pub max_tokens: u32,
    /// 0 to 2.
    pub temperature: f32,
    /// A JSON schema the answer is to follow, when one is wanted.
    pub schema: Option<String>,
    /// For an answer: what its prompt was made from (the question, the passages of the messages
    /// of `sources`, best first, and the marker), so that a host can make it again for another
    /// model ([`ModelRequest::rebuilt_for`]). Mail, as `messages` is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer: Option<AnswerInput>,
}

impl ModelRequest {
    /// A request, checked (`id` 0: [`Requests::start`] numbers it): 1 to [`MAX_MESSAGES`]
    /// messages of at most [`MAX_PROMPT_BYTES`] together, at most [`MAX_SOURCES`] sources, a
    /// schema of at most 16 KB. The text is cleaned ([`clean_text`]); the number of tokens and
    /// the temperature are brought within their range.
    pub fn new(
        kind: AiKind,
        messages: Vec<ChatMessage>,
        sources: Vec<Id>,
        stream: bool,
        max_tokens: Option<u32>,
        temperature: Option<f32>,
        schema: Option<String>,
    ) -> Result<ModelRequest, AiError> {
        if messages.is_empty() {
            return Err(AiError::Invalid("a model request has at least one message".into()));
        }
        if messages.len() > MAX_MESSAGES {
            return Err(AiError::TooMany { what: "messages", max: MAX_MESSAGES });
        }
        let total = messages.iter().fold(0usize, |n, m| n.saturating_add(m.content.len()));
        if total > MAX_PROMPT_BYTES {
            return Err(AiError::Invalid(format!("the messages of a model request are larger than {MAX_PROMPT_BYTES} bytes")));
        }
        if sources.len() > MAX_SOURCES {
            return Err(AiError::TooMany { what: "source messages", max: MAX_SOURCES });
        }
        if sources.contains(&0) {
            return Err(AiError::Invalid("a source is a message id above 0".into()));
        }
        if schema.as_ref().is_some_and(|s| s.len() > MAX_SCHEMA_BYTES) {
            return Err(AiError::Invalid(format!("the schema is larger than {MAX_SCHEMA_BYTES} bytes")));
        }
        let messages = messages.into_iter().map(|m| ChatMessage { role: m.role, content: clean_text(&m.content, MAX_PROMPT_BYTES).0 }).collect();
        let mut sources = sources;
        let mut seen = std::collections::HashSet::new();
        sources.retain(|id| seen.insert(*id));
        let temperature = temperature.filter(|t| t.is_finite()).unwrap_or(0.0).clamp(0.0, 2.0);
        let max_tokens = max_tokens.unwrap_or(DEFAULT_TOKENS).clamp(1, MAX_TOKENS);
        Ok(ModelRequest { id: 0, kind, messages, sources, stream, max_tokens, temperature, schema, answer: None })
    }

    /// This answer request with what its prompt was made from, checked: a request of kind
    /// `answer`; a question of 1 to 2,000 characters; a marker of 1 to 64 bytes; at most
    /// [`MAX_SOURCES`] passages of at most [`MAX_PROMPT_BYTES`] together, each of a message of
    /// `sources`.
    pub fn with_answer(mut self, input: AnswerInput) -> Result<ModelRequest, AiError> {
        if self.kind != AiKind::Answer {
            return Err(AiError::Invalid("only an answer request carries passages".into()));
        }
        let question = input.question.trim();
        if question.is_empty() || question.chars().nth(MAX_QUESTION_CHARS).is_some() {
            return Err(AiError::Invalid(format!("an answer's question is 1 to {MAX_QUESTION_CHARS} characters")));
        }
        if input.marker.trim().is_empty() || input.marker.len() > MAX_MARKER_BYTES {
            return Err(AiError::Invalid(format!("an answer's marker is 1 to {MAX_MARKER_BYTES} bytes")));
        }
        if input.passages.len() > MAX_SOURCES {
            return Err(AiError::TooMany { what: "passages", max: MAX_SOURCES });
        }
        let mut bytes = 0usize;
        for p in &input.passages {
            if p.message == 0 || !self.sources.contains(&p.message) {
                return Err(AiError::Invalid(format!("a passage of message {} isn't of a source of the request", p.message)));
            }
            bytes = bytes.saturating_add(p.subject.len()).saturating_add(p.from.len()).saturating_add(p.date.len()).saturating_add(p.text.len());
        }
        if bytes > MAX_PROMPT_BYTES {
            return Err(AiError::Invalid(format!("the passages of an answer are larger than {MAX_PROMPT_BYTES} bytes")));
        }
        self.answer = Some(input);
        Ok(self)
    }

    /// This answer request made again for a model whose context window is `context_window`
    /// tokens (a backup's), by the same builder ([`super::answer_prompt`]) from what it carries
    /// ([`ModelRequest::answer`]): its own id, kind, streaming and temperature; the messages and
    /// the sources of the new prompt (those of the passages that fit, best first, which are
    /// what `ModelEvent::Done::sources` then names); as many tokens for the answer as the window
    /// keeps (never more than before); and, when it had one, the answer schema for the new
    /// number of messages. Pure: nothing waits and nothing is sent. Refused for a request that
    /// carries no passages, or a window with no room for them.
    pub fn rebuilt_for(&self, context_window: usize) -> Result<ModelRequest, AiError> {
        let input = self.answer.as_ref().ok_or_else(|| AiError::Invalid("this request carries no passages to make its prompt from".into()))?;
        let prompt = input.prompt(context_window)?;
        let tokens = u32::try_from(answer_tokens_for(context_window)).unwrap_or(MAX_TOKENS).min(self.max_tokens.max(1));
        let schema = self.schema.as_ref().map(|_| answer_schema(prompt.sources.len()));
        let mut again = ModelRequest::new(self.kind, prompt.messages, prompt.sources, self.stream, Some(tokens), Some(self.temperature), schema)?;
        again.id = self.id;
        again.answer = self.answer.clone();
        Ok(again)
    }
}

/// Who answered a request, as the host knows it: the provider's id in the owner's settings and
/// the model's name. Shown with the answer ("Answered by …"). Not mail: written out in full.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AnsweredBy {
    pub provider: String,
    pub model: String,
}

impl AnsweredBy {
    /// As it may be kept: each one line of at most [`MAX_ANSWERED_BY_BYTES`] bytes, without
    /// control characters or direction marks. `None` when both are empty.
    pub fn cleaned(&self) -> Option<AnsweredBy> {
        let by = AnsweredBy { provider: one_line(&self.provider, MAX_ANSWERED_BY_BYTES), model: one_line(&self.model, MAX_ANSWERED_BY_BYTES) };
        (!by.provider.is_empty() || !by.model.is_empty()).then_some(by)
    }
}

/// Why a model request failed, as the host that sent it names it. The engine keeps it with the
/// request ([`Tracked::error_kind`]) and answers it from `ai.answerResult` and `ai.narrowResult`,
/// so that nobody has to read English words to know what went wrong.
///
/// As JSON (`snake_case`): a kind without details is its name (`"unreachable"`), one with them
/// an object of one member (`{"refused": {"status": 400}}`,
/// `{"timed_out": {"before_first_byte": true}}`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// No provider is set up, or it has no model.
    NotConfigured,
    /// The owner hasn't let this provider be sent the mail of the account asked about.
    NoConsent,
    /// The provider couldn't be reached (no answer, connection refused, name not found).
    Unreachable,
    /// The provider answered with an HTTP error.
    Refused { status: u16 },
    /// The provider took too long: before its first byte, or after some of the answer came.
    TimedOut { before_first_byte: bool },
    /// The provider's certificate isn't trusted.
    Certificate,
    /// The provider's settings are wrong (an address or a kind that can't be used).
    Settings,
    /// The system's credential store didn't give the provider's key.
    CredentialStore,
    /// The provider answered something that isn't what was asked for.
    Malformed,
    /// The answer was larger than is taken.
    TooLarge,
    /// The request was called off.
    Cancelled,
}

impl ErrorKind {
    /// The kind's name, as JSON writes it.
    pub fn name(self) -> &'static str {
        match self {
            ErrorKind::NotConfigured => "not_configured",
            ErrorKind::NoConsent => "no_consent",
            ErrorKind::Unreachable => "unreachable",
            ErrorKind::Refused { .. } => "refused",
            ErrorKind::TimedOut { .. } => "timed_out",
            ErrorKind::Certificate => "certificate",
            ErrorKind::Settings => "settings",
            ErrorKind::CredentialStore => "credential_store",
            ErrorKind::Malformed => "malformed",
            ErrorKind::TooLarge => "too_large",
            ErrorKind::Cancelled => "cancelled",
        }
    }

    /// Whether another provider may be asked instead: a failure of this provider, and not of
    /// what was asked or of the owner's choice. `no_consent` may go to a provider on this
    /// computer only (the host decides which); a time-out only before the first byte, since
    /// part of an answer has been shown after it.
    pub fn falls_back(self) -> bool {
        match self {
            ErrorKind::NotConfigured
            | ErrorKind::NoConsent
            | ErrorKind::Unreachable
            | ErrorKind::Refused { .. }
            | ErrorKind::Certificate
            | ErrorKind::Settings
            | ErrorKind::CredentialStore => true,
            ErrorKind::TimedOut { before_first_byte } => before_first_byte,
            ErrorKind::Malformed | ErrorKind::TooLarge | ErrorKind::Cancelled => false,
        }
    }
}

/// What a host hands back for a request.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ModelEvent {
    /// More of the answer, as it is written.
    Delta { text: String },
    /// The end: the whole answer when the host has it (it takes the place of what came in
    /// pieces), else what came in pieces is the answer. For an answer asked for as JSON, `text`
    /// is the whole JSON document the model wrote (its pieces were the answer's text, read as
    /// it came by `AnswerExtractor`). `answeredBy`: who answered, when the host knows (an older
    /// host's end without it is taken as it was). `sources`: when the host made the prompt
    /// again for another model ([`ModelRequest::rebuilt_for`]), the messages of that prompt, in
    /// its order: each one of the request's sources, each once.
    Done {
        #[serde(default)]
        text: Option<String>,
        #[serde(default, rename = "answeredBy", skip_serializing_if = "Option::is_none")]
        answered_by: Option<AnsweredBy>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sources: Option<Vec<Id>>,
    },
    /// The request failed: what kind of failure, why in words for the owner, and what the
    /// provider itself said, when it said something (kept as data at most
    /// [`MAX_PROVIDER_TEXT_BYTES`] long, shown only in the details, never taken for instructions).
    Error {
        kind: ErrorKind,
        message: String,
        #[serde(default, rename = "providerText", skip_serializing_if = "Option::is_none")]
        provider_text: Option<String>,
    },
}

impl ModelEvent {
    /// The end, with `text` (or what came in pieces), saying nothing of who answered.
    pub fn done(text: Option<String>) -> ModelEvent {
        ModelEvent::Done { text, answered_by: None, sources: None }
    }
}

/// Where a request is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RequestState {
    /// Handed to the host; nothing has come yet.
    #[default]
    Pending,
    /// Some of the answer has come.
    Streaming,
    Done,
    Failed,
    /// Called off: what comes for it is let go of.
    Cancelled,
}

impl RequestState {
    /// Still under way (pending or streaming).
    pub fn live(self) -> bool {
        matches!(self, RequestState::Pending | RequestState::Streaming)
    }
}

/// A request and what has come of it.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Tracked {
    pub request: ModelRequest,
    pub state: RequestState,
    /// The answer so far (or whole, once done), cleaned and capped.
    pub text: String,
    /// The answer was longer than [`MAX_RESPONSE_BYTES`]: what is kept is its start.
    pub truncated: bool,
    /// Why it failed.
    pub error: String,
    /// What kind of failure it was (once failed).
    pub error_kind: Option<ErrorKind>,
    /// What the provider itself said of the failure, cleaned ([`clean_provider_text`]); empty
    /// when it said nothing. Data, never instructions.
    pub provider_text: String,
    /// Events taken for it.
    pub events: u32,
    /// How many times it was asked again before it (a narrowed search whose answer wasn't a
    /// search MailCraft can read is asked again once).
    pub retries: u8,
    /// Who answered, as the host said at the end.
    pub answered_by: Option<AnsweredBy>,
    /// The messages of the prompt that was answered, when the host made it again for another
    /// model (else the request's own `sources`): see [`Tracked::sources`].
    pub sources_used: Option<Vec<Id>>,
    /// For an answer read from JSON: the numbers of the prompt it names (checked, at most
    /// three; possibly none). `None`: the answer is plain text, its citations are read from it.
    pub cited: Option<Vec<usize>>,
    /// Citations of the JSON let go of (not a message given, not a number, past three).
    pub cited_dropped: usize,
}

impl Tracked {
    /// The messages of the prompt that was answered, in its order: message `n` of the answer is
    /// `sources()[n - 1]`.
    pub fn sources(&self) -> &[Id] {
        self.sources_used.as_deref().unwrap_or(&self.request.sources)
    }

    /// An answer asked for as JSON.
    pub fn structured(&self) -> bool {
        self.request.kind == AiKind::Answer && self.request.schema.is_some()
    }
}

/// `used`, the messages a host says the answered prompt held, checked against `request`: each
/// one of its sources, each once, at least one.
fn used_sources(request: &ModelRequest, used: &[Id]) -> Result<Vec<Id>, AiError> {
    if used.is_empty() || used.len() > MAX_SOURCES {
        return Err(AiError::Invalid(format!("the messages of the answered prompt are 1 to {MAX_SOURCES} of the request's sources")));
    }
    let mut out: Vec<Id> = Vec::with_capacity(used.len());
    for id in used {
        if !request.sources.contains(id) || out.contains(id) {
            return Err(AiError::Invalid(format!("message {id} isn't once among the request's sources")));
        }
        out.push(*id);
    }
    Ok(out)
}

/// What [`Requests::apply`] did with an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applied {
    /// Taken: the request is in this state now.
    Taken(RequestState),
    /// Let go of: the request was cancelled.
    Ignored,
}

/// The model requests of a session: those under way and the last finished ones. Never saved.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Requests {
    next: u64,
    list: Vec<Tracked>,
}

impl Requests {
    /// Request `id`, if it is kept.
    pub fn get(&self, id: u64) -> Option<&Tracked> {
        self.list.iter().find(|t| t.request.id == id)
    }

    /// Every request kept, oldest first.
    pub fn all(&self) -> &[Tracked] {
        &self.list
    }

    /// How many are under way.
    pub fn live(&self) -> usize {
        self.list.iter().filter(|t| t.state.live()).count()
    }

    /// Record `request` as under way, numbered, and hand it back numbered. The oldest finished
    /// request goes when [`MAX_KEPT`] are kept; refused when [`MAX_LIVE`] are under way.
    pub fn start(&mut self, mut request: ModelRequest) -> Result<ModelRequest, AiError> {
        if self.live() >= MAX_LIVE {
            return Err(AiError::TooMany { what: "model requests under way", max: MAX_LIVE });
        }
        while self.list.len() >= MAX_KEPT {
            let Some(at) = self.list.iter().position(|t| !t.state.live()) else {
                return Err(AiError::TooMany { what: "model requests under way", max: MAX_LIVE });
            };
            self.list.remove(at);
        }
        let id = self.next.checked_add(1).ok_or_else(|| AiError::Invalid("no more model requests can be numbered".into()))?;
        self.next = id;
        request.id = id;
        self.list.push(Tracked { request: request.clone(), ..Default::default() });
        Ok(request)
    }

    /// [`Requests::start`] for a request asked again after `retries` tries before it.
    pub fn start_retry(&mut self, request: ModelRequest, retries: u8) -> Result<ModelRequest, AiError> {
        let started = self.start(request)?;
        if let Some(t) = self.list.iter_mut().find(|t| t.request.id == started.id) {
            t.retries = retries;
        }
        Ok(started)
    }

    /// Take `event` for request `id`: more text, the end or an error. Refused for a request that
    /// isn't kept ([`AiError::UnknownRequest`]) or is over ([`AiError::Finished`]); let go of for
    /// one cancelled. Past [`MAX_EVENTS`] events the request fails.
    pub fn apply(&mut self, id: u64, event: ModelEvent) -> Result<Applied, AiError> {
        let t = self.list.iter_mut().find(|t| t.request.id == id).ok_or(AiError::UnknownRequest(id))?;
        match t.state {
            RequestState::Cancelled => return Ok(Applied::Ignored),
            RequestState::Done | RequestState::Failed => return Err(AiError::Finished(id)),
            RequestState::Pending | RequestState::Streaming => {}
        }
        // Checked before anything of the request changes.
        let used = match &event {
            ModelEvent::Done { sources: Some(used), .. } => Some(used_sources(&t.request, used)?),
            _ => None,
        };
        t.events = t.events.saturating_add(1);
        if t.events > MAX_EVENTS {
            t.state = RequestState::Failed;
            t.error = format!("the model sent more than {MAX_EVENTS} pieces");
            t.error_kind = Some(ErrorKind::TooLarge);
            return Ok(Applied::Taken(t.state));
        }
        match event {
            ModelEvent::Delta { text } => {
                let room = MAX_RESPONSE_BYTES.saturating_sub(t.text.len());
                let (clean, cut) = clean_text(&text, room);
                t.text.push_str(&clean);
                t.truncated |= cut;
                t.state = RequestState::Streaming;
            }
            ModelEvent::Done { text, answered_by, sources: _ } => {
                t.answered_by = answered_by.as_ref().and_then(AnsweredBy::cleaned);
                if used.is_some() {
                    t.sources_used = used;
                }
                // An answer asked for as JSON: the document, given whole or come in pieces.
                let json = match &text {
                    Some(text) => text.trim_start().starts_with('{').then(|| text.clone()),
                    None => t.text.trim_start().starts_with('{').then(|| t.text.clone()),
                };
                match json.filter(|_| t.structured()) {
                    Some(json) => {
                        let given = t.sources().len();
                        match read_answer_json(&json, given) {
                            Ok(read) => {
                                t.text = read.answer;
                                t.truncated = read.truncated;
                                t.cited = Some(read.citations);
                                t.cited_dropped = read.dropped;
                            }
                            Err(_) => {
                                // What was shown stays; with nothing shown, as much of the
                                // answer as can be read.
                                if t.text.is_empty() || t.text.trim_start().starts_with('{') {
                                    let (clean, cut) = clean_text(&extract_answer(&json), MAX_RESPONSE_BYTES);
                                    t.text = clean;
                                    t.truncated = cut;
                                }
                                t.error = SOURCES_UNREADABLE.to_string();
                                t.error_kind = Some(ErrorKind::Malformed);
                                t.state = RequestState::Failed;
                                return Ok(Applied::Taken(t.state));
                            }
                        }
                    }
                    None => {
                        if let Some(text) = text {
                            let (clean, cut) = clean_text(&text, MAX_RESPONSE_BYTES);
                            t.text = clean;
                            t.truncated = cut;
                        }
                    }
                }
                t.state = RequestState::Done;
            }
            ModelEvent::Error { kind, message, provider_text } => {
                t.error = clean_text(&message, MAX_ERROR_BYTES).0;
                t.error_kind = Some(kind);
                t.provider_text = provider_text.as_deref().map(clean_provider_text).unwrap_or_default();
                t.state = RequestState::Failed;
            }
        }
        Ok(Applied::Taken(t.state))
    }

    /// Call off request `id`: whether it was under way (a finished one stays as it was).
    /// Refused for a request that isn't kept.
    pub fn cancel(&mut self, id: u64) -> Result<bool, AiError> {
        let t = self.list.iter_mut().find(|t| t.request.id == id).ok_or(AiError::UnknownRequest(id))?;
        if t.state.live() {
            t.state = RequestState::Cancelled;
            return Ok(true);
        }
        Ok(false)
    }

    /// Call off every request under way and forget them all (another mailbox was opened).
    pub fn clear(&mut self) {
        self.list.clear();
    }
}

/// `text` as it may be kept and shown: without control characters (but the line break and the
/// tab), without the characters that turn the direction of text around, at most `max` bytes
/// (cut at a character boundary). Whether it was cut.
pub fn clean_text(text: &str, max: usize) -> (String, bool) {
    let mut out = String::with_capacity(text.len().min(max));
    for c in text.chars() {
        let dropped =
            (c.is_control() && c != '\n' && c != '\t') || matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}');
        if dropped {
            continue;
        }
        if out.len().saturating_add(c.len_utf8()) > max {
            return (out, true);
        }
        out.push(c);
    }
    (out, false)
}

/// What a provider said of a failure, as it may be kept: control characters (the line break and
/// the tab too: it is one line of detail) become a space, the characters that turn the direction
/// of text around are dropped, and it is cut to at most [`MAX_PROVIDER_TEXT_BYTES`] bytes at a
/// character boundary. Data, never instructions.
pub fn clean_provider_text(text: &str) -> String {
    one_line(text, MAX_PROVIDER_TEXT_BYTES)
}

/// `text` as one line of at most `max` bytes: control characters become a space, direction
/// marks are dropped, cut at a character boundary, trimmed.
fn one_line(text: &str, max: usize) -> String {
    let mut out = String::with_capacity(text.len().min(max));
    for c in text.chars() {
        if matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}') {
            continue;
        }
        let c = if c.is_control() { ' ' } else { c };
        if out.len().saturating_add(c.len_utf8()) > max {
            break;
        }
        out.push(c);
    }
    out.trim().to_string()
}

// What a conversation with a model holds is mail and what the owner typed: a request, an event or
// a request kept is written out for a log or a test by its lengths and its ids, never its text.

impl std::fmt::Debug for ChatMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChatMessage").field("role", &self.role).field("bytes", &self.content.len()).finish()
    }
}

impl std::fmt::Debug for ModelRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelRequest")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("messages", &self.messages)
            .field("sources", &self.sources)
            .field("stream", &self.stream)
            .field("max_tokens", &self.max_tokens)
            .field("temperature", &self.temperature)
            .field("schema_bytes", &self.schema.as_ref().map(String::len))
            .field("answer", &self.answer)
            .finish()
    }
}

impl std::fmt::Debug for ModelEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModelEvent::Delta { text } => f.debug_struct("Delta").field("bytes", &text.len()).finish(),
            ModelEvent::Done { text, answered_by, sources } => f
                .debug_struct("Done")
                .field("bytes", &text.as_ref().map(String::len))
                .field("answered_by", answered_by)
                .field("sources", sources)
                .finish(),
            ModelEvent::Error { kind, message, provider_text } => f
                .debug_struct("Error")
                .field("kind", kind)
                .field("bytes", &message.len())
                .field("provider_bytes", &provider_text.as_ref().map(String::len))
                .finish(),
        }
    }
}

impl std::fmt::Debug for Tracked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tracked")
            .field("request", &self.request)
            .field("state", &self.state)
            .field("text_bytes", &self.text.len())
            .field("truncated", &self.truncated)
            .field("error_bytes", &self.error.len())
            .field("error_kind", &self.error_kind)
            .field("provider_bytes", &self.provider_text.len())
            .field("events", &self.events)
            .field("retries", &self.retries)
            .field("answered_by", &self.answered_by)
            .field("sources_used", &self.sources_used)
            .field("cited", &self.cited)
            .field("cited_dropped", &self.cited_dropped)
            .finish()
    }
}
