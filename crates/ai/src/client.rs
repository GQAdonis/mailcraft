// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The calls: chat (whole or streamed), structured chat and embeddings, each one request to the
//! provider through `mailcraft_net::https::stream`. They block; the host runs them on a thread of
//! its own. Each checks the settings and the owner's consent before anything is sent.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mailcraft_net::NetError;
use mailcraft_net::https::{self, Deadlines, Flow, Piece, StreamRequest};
use mailcraft_net::sse;
use serde_json::Value;

use crate::config::{Carries, ProviderConfig, ProviderKind, Timeouts};
use crate::error::{AiError, tidy};
use crate::models::{self, MAX_MODELS_BYTES, ModelList};
use crate::on_device::{self, Health};
use crate::wire::{self, ChatAnswer, ChatRequest, Chunk, Embeddings, MAX_ANSWER_BYTES};

/// The most body bytes taken of a whole chat answer, a streamed one, and an embeddings answer.
const MAX_CHAT_RESPONSE: u64 = 2 * 1024 * 1024;
const MAX_STREAM_RESPONSE: u64 = 16 * 1024 * 1024;
const MAX_EMBED_RESPONSE: u64 = 48 * 1024 * 1024;
/// The most body bytes read of an answer that isn't a success (its error, in JSON).
const MAX_ERROR_RESPONSE: u64 = 16 * 1024;
/// The caps of a streamed answer's events.
const STREAM_LIMITS: sse::Limits = sse::Limits { max_line: 256 * 1024, max_event: 512 * 1024, max_total: MAX_STREAM_RESPONSE, max_events: 1_000_000 };
const CHAT: &str = "chat/completions";
const EMBEDDINGS: &str = "embeddings";
const MODELS: &str = "models";
const HEALTH: &str = "health";
/// The longest a model list is waited for, whatever the provider's own waits: it is asked before
/// a request (the route check) and for Test connection, and is small.
const MODELS_WAIT_SECS: u32 = 15;

/// The deadlines of a request to a provider with `timeouts` (clamped to their ceilings).
pub fn deadlines(timeouts: &Timeouts) -> Deadlines {
    let timeouts = timeouts.clamped();
    let seconds = |s: u32| Duration::from_secs(u64::from(s));
    Deadlines {
        connect: seconds(timeouts.connect_secs),
        first_byte: seconds(timeouts.first_byte_secs),
        idle: seconds(timeouts.idle_secs),
        total: seconds(timeouts.total_secs),
    }
}

/// Asks `config`'s model `request` and returns its whole answer.
///
/// `carries` says whose mail is in the request: a remote provider is sent nothing unless the owner
/// allowed it for each of those accounts. `cancel` stops the request at the next read once set.
pub fn chat(config: &ProviderConfig, carries: Carries<'_>, request: &ChatRequest, cancel: &AtomicBool) -> Result<ChatAnswer, AiError> {
    prepare(config, carries)?;
    let body = wire::chat_body(&config.model, request, false)?;
    let mut collected = Vec::new();
    exchange(config, "POST", &config.endpoint(CHAT), &body, "application/json", MAX_CHAT_RESPONSE, cancel, &mut |got: Got<'_>| {
        if let Got::Body(bytes) = got {
            collected.extend_from_slice(bytes);
        }
        Ok(true)
    })?;
    wire::parse_completion(&collected)
}

/// [`chat`] with the answer streamed: `on_text` is handed each new piece of text as it comes
/// (returning `false` stops the request: [`AiError::Cancelled`]), and the whole answer is
/// returned at the end. A server that answers whole although asked to stream is read as whole,
/// and its text handed over at once.
pub fn chat_stream(
    config: &ProviderConfig,
    carries: Carries<'_>,
    request: &ChatRequest,
    cancel: &AtomicBool,
    on_text: &mut dyn FnMut(&str) -> bool,
) -> Result<ChatAnswer, AiError> {
    prepare(config, carries)?;
    let body = wire::chat_body(&config.model, request, true)?;
    let mut reading = Streaming::default();
    exchange(config, "POST", &config.endpoint(CHAT), &body, "text/event-stream", MAX_STREAM_RESPONSE, cancel, &mut |got: Got<'_>| {
        reading.take(got, on_text)
    })?;
    reading.finish(on_text)
}

/// [`chat`] for a structured answer: `request` must name a [`wire::ResponseFormat`], and the
/// answer is read as JSON (and only as JSON: what it means is the caller's to check). A provider
/// that ignores the format and answers in words is [`AiError::Malformed`]; no retry is made here.
pub fn chat_json(config: &ProviderConfig, carries: Carries<'_>, request: &ChatRequest, cancel: &AtomicBool) -> Result<Value, AiError> {
    if request.response_format.is_none() {
        return Err(AiError::Config("a structured answer needs a response format".into()));
    }
    let answer = chat(config, carries, request, cancel)?;
    wire::json_answer(&answer.text)
}

/// [`chat_stream`] for a structured answer: `request` must name a [`wire::ResponseFormat`], which
/// is sent with the request to stream. `on_text` is handed each piece of the JSON as the model
/// writes it (the caller reads the text out of it as it grows), and the whole of what was
/// written is returned at the end, unchecked: what it means, and whether it is JSON at all, is
/// the caller's to check. A refusal of the format (a 400) comes before any piece, so the caller
/// may step down its ladder.
pub fn chat_stream_json(
    config: &ProviderConfig,
    carries: Carries<'_>,
    request: &ChatRequest,
    cancel: &AtomicBool,
    on_text: &mut dyn FnMut(&str) -> bool,
) -> Result<String, AiError> {
    if request.response_format.is_none() {
        return Err(AiError::Config("a structured answer needs a response format".into()));
    }
    chat_stream(config, carries, request, cancel, on_text).map(|answer| answer.text)
}

/// The embeddings of `inputs` from `config`'s embedding model, one for each, in their order.
pub fn embed(config: &ProviderConfig, carries: Carries<'_>, inputs: &[String], cancel: &AtomicBool) -> Result<Embeddings, AiError> {
    prepare(config, carries)?;
    let model = config.embedding_model.trim();
    if model.is_empty() {
        return Err(AiError::NotConfigured);
    }
    let body = wire::embeddings_body(model, inputs)?;
    let mut collected = Vec::new();
    exchange(config, "POST", &config.endpoint(EMBEDDINGS), &body, "application/json", MAX_EMBED_RESPONSE, cancel, &mut |got: Got<'_>| {
        if let Got::Body(bytes) = got {
            collected.extend_from_slice(bytes);
        }
        Ok(true)
    })?;
    wire::parse_embeddings(&collected, inputs.len())
}

/// What `config`'s provider says it serves (`GET /v1/models`): no mail, no question, nothing of the
/// owner's but the key when one is held (Test connection with the kept key). At most 1 MB is read,
/// within the provider's own waits but never longer than 15 seconds, and the list is read as
/// hostile ([`models::parse_models`]).
pub fn models(config: &ProviderConfig, cancel: &AtomicBool) -> Result<ModelList, AiError> {
    config.check()?;
    let asked = short_waits(config);
    let collected = get(&asked, &asked.endpoint(MODELS), MAX_MODELS_BYTES, cancel)?;
    models::parse_models(&collected)
}

/// What an apfel server says of itself (`GET /health`, at the root of its address): no body, no
/// key (none is ever sent there), at most 16 KB read, read as apfel's shape or refused
/// ([`on_device::parse_health`]). Only to a provider on this computer.
pub fn health(config: &ProviderConfig, cancel: &AtomicBool) -> Result<Health, AiError> {
    config.check()?;
    if config.kind != ProviderKind::Local {
        return Err(AiError::Config("only a model on this computer is asked how it is".into()));
    }
    let asked = ProviderConfig { api_key: None, ..short_waits(config) };
    let collected = get(&asked, &asked.root_endpoint(HEALTH), on_device::MAX_HEALTH_BYTES, cancel)?;
    on_device::parse_health(&collected)
}

/// `config` with its waits no longer than [`MODELS_WAIT_SECS`].
fn short_waits(config: &ProviderConfig) -> ProviderConfig {
    let own = config.timeouts.clamped();
    let timeouts = Timeouts {
        connect_secs: own.connect_secs.min(MODELS_WAIT_SECS),
        first_byte_secs: own.first_byte_secs.min(MODELS_WAIT_SECS),
        idle_secs: own.idle_secs.min(MODELS_WAIT_SECS),
        total_secs: own.total_secs.min(MODELS_WAIT_SECS),
    };
    ProviderConfig { timeouts, ..config.clone() }
}

/// The body of a `GET` of `url`, at most `max` bytes.
fn get(config: &ProviderConfig, url: &str, max: usize, cancel: &AtomicBool) -> Result<Vec<u8>, AiError> {
    let max = u64::try_from(max).unwrap_or(u64::MAX);
    let mut collected = Vec::new();
    exchange(config, "GET", url, &[], "application/json", max, cancel, &mut |got: Got<'_>| {
        if let Got::Body(bytes) = got {
            collected.extend_from_slice(bytes);
        }
        Ok(true)
    })?;
    Ok(collected)
}

/// The settings are usable and the owner allows what the request carries: else nothing is sent.
fn prepare(config: &ProviderConfig, carries: Carries<'_>) -> Result<(), AiError> {
    config.check()?;
    config.may_send(carries)
}

/// What a successful answer hands over: its `Content-Type`, then its body in pieces.
enum Got<'a> {
    Head(Option<&'a str>),
    Body(&'a [u8]),
}

/// What the handler of a successful answer said of a piece: go on, or stop (all that is needed
/// came, or it failed). Its failure is the request's.
fn flow(said: Result<bool, AiError>, failure: &mut Option<AiError>) -> Flow {
    match said {
        Ok(true) => Flow::Go,
        Ok(false) => Flow::Stop,
        Err(error) => {
            *failure = Some(error);
            Flow::Stop
        }
    }
}

/// One `method` (POST, or GET with an empty body) of `body` to `url`, one of the provider's
/// addresses ([`ProviderConfig::endpoint`]). A success's head and body go to `on_success`
/// (at most `max` bytes of body), which says whether it wants more; any other answer is an
/// [`AiError::Status`] with what the provider said in it. The key goes in the `Authorization`
/// header alone and is blanked out of anything said back.
#[allow(clippy::too_many_arguments)]
fn exchange(
    config: &ProviderConfig,
    method: &str,
    url: &str,
    body: &[u8],
    accept: &str,
    max: u64,
    cancel: &AtomicBool,
    on_success: &mut dyn FnMut(Got<'_>) -> Result<bool, AiError>,
) -> Result<(), AiError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(AiError::Cancelled);
    }
    let secret = config.api_key.as_ref().map(|key| key.expose());
    let bearer = secret.map(|key| format!("Bearer {key}"));
    let mut fields: Vec<(&str, &str)> = vec![("Accept", accept)];
    if !body.is_empty() {
        fields.push(("Content-Type", "application/json"));
    }
    if let Some(bearer) = bearer.as_deref() {
        fields.push(("Authorization", bearer));
    }
    let request = StreamRequest {
        method,
        url,
        fields: &fields,
        body,
        trusted: config.pinned_sha256.trim(),
        deadlines: deadlines(&config.timeouts),
        max_response: max,
        max_error_response: MAX_ERROR_RESPONSE,
        cancel: Some(cancel),
    };
    let mut status = 0u16;
    let mut failure: Option<AiError> = None;
    let mut error_body: Vec<u8> = Vec::new();
    let outcome = https::stream(&request, &mut |piece: Piece<'_>| match piece {
        Piece::Head(head) => {
            status = head.status;
            if (200..300).contains(&head.status) { flow(on_success(Got::Head(head.content_type.as_deref())), &mut failure) } else { Flow::Go }
        }
        Piece::Body(bytes) if (200..300).contains(&status) => flow(on_success(Got::Body(bytes)), &mut failure),
        Piece::Body(bytes) => {
            error_body.extend_from_slice(bytes);
            Flow::Go
        }
    })
    .map_err(|error| from_net(&error, secret))?;
    if let Some(failure) = failure {
        return Err(failure);
    }
    let status = outcome.head.status;
    if !(200..300).contains(&status) {
        let said = serde_json::from_slice::<Value>(&error_body).ok().and_then(|value| wire::error_message(&value));
        let message = match said {
            Some(said) => tidy(&said, secret),
            None if (300..400).contains(&status) => "it sent the request elsewhere, which is not followed".to_string(),
            None => String::new(),
        };
        return Err(AiError::Status { status, message });
    }
    Ok(())
}

/// A network error in this crate's words, with the key blanked out of whatever a server said.
fn from_net(error: &NetError, secret: Option<&str>) -> AiError {
    if https::is_stopped(error) {
        return AiError::Cancelled;
    }
    let said = tidy(&error.to_string(), secret);
    if https::is_timeout(error) {
        return AiError::TimedOut(said);
    }
    match error {
        NetError::Certificate(_) => AiError::Certificate(said),
        NetError::Config(_) => AiError::Refused(said),
        _ if error.is_unreachable() => AiError::Unreachable(said),
        _ => AiError::Malformed(said),
    }
}

/// A streamed answer as it is read: an event stream, or (for a server that answers whole) JSON.
#[derive(Default)]
struct Streaming {
    /// Whether the answer is an event stream: from its `Content-Type`, or else from its first byte.
    events_kind: Option<bool>,
    parser: sse::Parser,
    events: Vec<sse::Event>,
    /// A whole answer's body, or the first bytes of one whose kind isn't known yet.
    plain: Vec<u8>,
    answer: ChatAnswer,
    /// `[DONE]` came.
    done: bool,
}

impl Streaming {
    fn take(&mut self, got: Got<'_>, on_text: &mut dyn FnMut(&str) -> bool) -> Result<bool, AiError> {
        match got {
            Got::Head(content_type) => {
                self.parser = sse::Parser::new(STREAM_LIMITS);
                self.events_kind = content_type.map(|kind| kind.trim().to_ascii_lowercase().starts_with("text/event-stream"));
                Ok(true)
            }
            Got::Body(bytes) => match self.events_kind {
                Some(true) => self.feed(bytes, on_text),
                Some(false) => {
                    self.plain.extend_from_slice(bytes);
                    Ok(true)
                }
                None => {
                    // No `Content-Type`: a JSON answer begins with `{`, an event stream doesn't.
                    self.plain.extend_from_slice(bytes);
                    let Some(first) = self.plain.iter().find(|b| !b.is_ascii_whitespace()).copied() else { return Ok(true) };
                    self.events_kind = Some(first != b'{');
                    if first == b'{' {
                        return Ok(true);
                    }
                    let held = std::mem::take(&mut self.plain);
                    self.feed(&held, on_text)
                }
            },
        }
    }

    /// The next piece of an event stream; whether more is wanted (not once `[DONE]` came).
    fn feed(&mut self, bytes: &[u8], on_text: &mut dyn FnMut(&str) -> bool) -> Result<bool, AiError> {
        self.parser.feed(bytes, &mut self.events).map_err(|e| AiError::Malformed(e.to_string()))?;
        self.drain(on_text)?;
        Ok(!self.done)
    }

    fn drain(&mut self, on_text: &mut dyn FnMut(&str) -> bool) -> Result<(), AiError> {
        for event in std::mem::take(&mut self.events) {
            if self.done {
                break;
            }
            if event.is_done() {
                self.done = true;
                break;
            }
            let chunk = wire::parse_chunk(&event.data)?;
            self.add(chunk, on_text)?;
        }
        Ok(())
    }

    fn add(&mut self, chunk: Chunk, on_text: &mut dyn FnMut(&str) -> bool) -> Result<(), AiError> {
        if !chunk.text.is_empty() {
            if self.answer.text.len().saturating_add(chunk.text.len()) > MAX_ANSWER_BYTES {
                return Err(AiError::TooLarge);
            }
            self.answer.text.push_str(&chunk.text);
            if !on_text(&chunk.text) {
                return Err(AiError::Cancelled);
            }
        }
        if chunk.finish_reason.is_some() {
            self.answer.finish_reason = chunk.finish_reason;
        }
        if self.answer.model.is_none() {
            self.answer.model = chunk.model;
        }
        if chunk.usage.is_some() {
            self.answer.usage = chunk.usage;
        }
        Ok(())
    }

    fn finish(mut self, on_text: &mut dyn FnMut(&str) -> bool) -> Result<ChatAnswer, AiError> {
        if self.events_kind != Some(true) {
            let answer = wire::parse_completion(&self.plain)?;
            if !answer.text.is_empty() && !on_text(&answer.text) {
                return Err(AiError::Cancelled);
            }
            return Ok(answer);
        }
        if !self.done {
            self.parser.finish(&mut self.events).map_err(|e| AiError::Malformed(e.to_string()))?;
            self.drain(on_text)?;
        }
        if !self.done && self.answer.finish_reason.is_none() {
            return Err(AiError::Malformed("the answer ended before it was finished".into()));
        }
        Ok(self.answer)
    }
}
