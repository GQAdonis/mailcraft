// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The engine's side of a model call, as for a Send/Receive: the engine never talks to a model.
//!
//! - `ai.request` ([`request`]) records a [`ModelRequest`] among the requests under way and
//!   answers it, numbered, for the host to send to the model the owner configured.
//! - `ai.apply` ([`apply`]) takes what the host got back for it, one piece at a time: some more
//!   text (`delta`), the end (`done`, with the whole text when the host has it) or an error.
//! - `ai.cancel` ([`cancel`]) calls a request off; what comes for it afterwards is let go of.
//!
//! All three are the owner's and the host's alone, never an agent's (`crate::agent`). Their
//! parameters are checked as anything from outside: a request or an event that isn't what it
//! should be is refused, an id no request has is refused, and nothing is kept past the caps of
//! [`mailcraft_model::ai::call`]. What a model says is cleaned there before it is kept, and
//! nothing of it is run, fetched or sent.
//!
//! These work on the session's [`Requests`] (`Session::ai`).

use mailcraft_model::Id;
use mailcraft_model::ai::{AiKind, Applied, ChatMessage, ErrorKind, ModelEvent, ModelRequest, Requests, Role, Tracked};
use serde_json::{Value, json};

use crate::cmd::bad;
use crate::{CmdError, CmdResult, p};

/// `ai.request`'s params, for the command's description.
pub const REQUEST_PARAMS: &str = r#"{"kind": "answer"|"narrow", "messages": [{"role": "system"|"user"|"assistant", "content": string}] (1–32, 512 KB in all), "sources"?: [message id] (at most 64: the only messages an answer may cite), "stream"?: bool, "maxTokens"?: 1–8192, "temperature"?: 0–2, "schema"?: string (a JSON schema)} — answers {"request": ModelRequest} for the host to send; owner and host only"#;
/// `ai.apply`'s params.
pub const APPLY_PARAMS: &str = r#"{"requestId": number, "event": {"type": "delta", "text": string} | {"type": "done", "text"?: string (for an answer asked for as JSON, the whole document), "answeredBy"?: {"provider": string, "model": string} (who answered), "sources"?: [message id] (the messages of a prompt made again for a backup, each one of the request's sources)} | {"type": "error", "kind": "not_configured"|"no_consent"|"unreachable"|{"refused": {"status": 100–599}}|{"timed_out": {"before_first_byte": bool}}|"certificate"|"settings"|"credential_store"|"malformed"|"broken_off"|"too_large"|"cancelled", "message": string (words for the owner), "providerText"?: string (what the provider said: kept as data, at most 300 bytes, one line)}} — answers {"state": "streaming"|"done"|"failed"} or {"ignored": true} for a request called off; host only"#;
/// `ai.cancel`'s params.
pub const CANCEL_PARAMS: &str = r#"{"requestId": number} — answers {"cancelled": bool}; whatever comes for it later is let go of"#;

/// `ai.request` on `reqs`.
pub fn request(reqs: &mut Requests, v: &Value) -> CmdResult {
    p::only(v, &["kind", "messages", "sources", "stream", "maxTokens", "temperature", "schema"])?;
    if !v.is_object() {
        return Err(bad("the parameters are an object with `kind` and `messages`"));
    }
    let kind = p::str(v, "kind").and_then(AiKind::parse).ok_or_else(|| bad("`kind` is \"answer\" or \"narrow\""))?;
    let messages = messages_of(v)?;
    let sources = sources_of(v)?;
    let stream = p::opt_bool(v, "stream")?.unwrap_or(false);
    let max_tokens = match v.get("maxTokens") {
        None => None,
        Some(_) => {
            Some(p::u64(v, "maxTokens").and_then(|n| u32::try_from(n).ok()).ok_or_else(|| bad("`maxTokens` is a whole number from 1 to 8192"))?)
        }
    };
    let temperature = match v.get("temperature") {
        None => None,
        Some(x) => Some(x.as_f64().filter(|t| t.is_finite()).ok_or_else(|| bad("`temperature` is a number from 0 to 2"))? as f32),
    };
    let schema = match v.get("schema") {
        None => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err(bad("`schema` is a JSON schema, as a string")),
    };
    let made = ModelRequest::new(kind, messages, sources, stream, max_tokens, temperature, schema).map_err(|e| bad(e.to_string()))?;
    let started = reqs.start(made).map_err(|e| bad(e.to_string()))?;
    Ok(json!({"request": as_json(&started)?}))
}

/// `ai.apply` on `reqs`.
pub fn apply(reqs: &mut Requests, v: &Value) -> CmdResult {
    p::only(v, &["requestId", "event"])?;
    let id = request_id(v)?;
    let event = v.get("event").cloned().ok_or_else(|| bad("`event` is required"))?;
    let event: ModelEvent = serde_json::from_value(event).map_err(|_| bad("`event` is {\"type\": \"delta\"|\"done\"|\"error\", …}"))?;
    if let ModelEvent::Error { kind: ErrorKind::Refused { status }, .. } = &event
        && !(100..=599).contains(status)
    {
        return Err(bad("a refusal's `status` is an HTTP status, 100 to 599"));
    }
    match reqs.apply(id, event).map_err(|e| bad(e.to_string()))? {
        Applied::Ignored => Ok(json!({"ignored": true})),
        Applied::Taken(state) => Ok(json!({"state": as_json(&state)?})),
    }
}

/// `ai.cancel` on `reqs`.
pub fn cancel(reqs: &mut Requests, v: &Value) -> CmdResult {
    p::only(v, &["requestId"])?;
    let id = request_id(v)?;
    let cancelled = reqs.cancel(id).map_err(|e| bad(e.to_string()))?;
    Ok(json!({"cancelled": cancelled}))
}

/// The answer of `ai.answerResult` and `ai.narrowResult` for a failed request:
/// `{"state": "failed", "kind", "message"}`, with the kind's details beside it (`status` for a
/// refusal, `beforeFirstByte` for a time-out). The provider's own words stay with the request
/// for the owner's details and are not answered here.
pub(crate) fn failed(t: &Tracked) -> Value {
    // A request fails only with a kind ([`Requests::apply`]); one without is read as malformed.
    let kind = t.error_kind.unwrap_or(ErrorKind::Malformed);
    let mut v = serde_json::Map::new();
    v.insert("state".into(), json!("failed"));
    v.insert("kind".into(), json!(kind.name()));
    v.insert("message".into(), json!(t.error));
    match kind {
        ErrorKind::Refused { status } => {
            v.insert("status".into(), json!(status));
        }
        ErrorKind::TimedOut { before_first_byte } => {
            v.insert("beforeFirstByte".into(), json!(before_first_byte));
        }
        _ => {}
    }
    Value::Object(v)
}

/// `x` as JSON.
pub(crate) fn as_json<T: serde::Serialize>(x: &T) -> Result<Value, CmdError> {
    serde_json::to_value(x).map_err(|e| CmdError::Failed(e.to_string()))
}

/// The request named by `requestId`.
pub(crate) fn request_id(v: &Value) -> Result<u64, CmdError> {
    p::u64(v, "requestId").filter(|n| *n != 0).ok_or_else(|| bad("`requestId` is the number `ai.request` answered"))
}

/// `messages`: 1 to 32 of `{"role", "content"}` (the model's caps are checked when the request is
/// made).
fn messages_of(v: &Value) -> Result<Vec<ChatMessage>, CmdError> {
    let wrong = || bad("`messages` is a list of 1 to 32 {\"role\": \"system\"|\"user\"|\"assistant\", \"content\": string}");
    let list =
        v.get("messages").and_then(Value::as_array).filter(|a| !a.is_empty() && a.len() <= mailcraft_model::ai::MAX_MESSAGES).ok_or_else(wrong)?;
    list.iter()
        .map(|m| {
            let role = m.get("role").and_then(Value::as_str).and_then(Role::parse).ok_or_else(wrong)?;
            let content = m.get("content").and_then(Value::as_str).ok_or_else(wrong)?;
            Ok(ChatMessage { role, content: content.to_string() })
        })
        .collect()
}

/// `sources`: at most 64 message ids, each above 0.
fn sources_of(v: &Value) -> Result<Vec<Id>, CmdError> {
    let wrong = || bad("`sources` is a list of at most 64 message ids");
    match v.get("sources") {
        None => Ok(Vec::new()),
        Some(x) => {
            let list = x.as_array().filter(|a| a.len() <= mailcraft_model::ai::MAX_SOURCES).ok_or_else(wrong)?;
            list.iter().map(|id| id.as_u64().filter(|n| *n != 0).ok_or_else(wrong)).collect()
        }
    }
}
