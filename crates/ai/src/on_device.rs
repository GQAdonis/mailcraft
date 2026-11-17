// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Apple's on-device model, served on this computer by apfel (#ai): how a server that says it is
//! one is told from one that isn't. Pure reading, no I/O: the host asks the two questions
//! (`GET /v1/models`, `GET /health`, no body and no key, to loopback alone) and hands the answers
//! here.
//!
//! **Identity.** A server is taken for apfel only when both answers say so:
//! - its model list names exactly one model, [`APPLE_MODEL`] (a server that lists any other
//!   model, such as Ollama on the same default port or the owner's gateway, is not a candidate);
//! - its `/health` answer has apfel's shape ([`parse_health`]): a JSON object with a `status`
//!   word and at least one of the facts apfel states (its model, whether the model is available,
//!   its context window), and, when it names a model, that model.
//!
//! What it doesn't catch: a process that takes over the port and imitates both answers exactly.
//! That would be the owner's own software on their own computer; the residual risk is named in
//! PRIVACY.md. A confirmation binds an address to the identity read then ([`identity`]), and a
//! different answer later stops its use.
//!
//! Everything here is hostile input: at most [`MAX_HEALTH_BYTES`] of `/health` are read, names
//! are short text without control characters, and a context window is clamped.

use std::fmt;

use serde_json::Value;

use crate::config::clamp_context_window;
use crate::error::AiError;
use crate::models::{ModelList, fingerprint};

/// The one model apfel serves.
pub const APPLE_MODEL: &str = "apple-foundationmodel";
/// apfel's documented default port (which Ollama also uses).
pub const DEFAULT_PORT: u16 = 11_434;
/// Most bytes of a `/health` answer read.
pub const MAX_HEALTH_BYTES: usize = 16 * 1024;
/// Longest name taken from `/health`.
const MAX_NAME: usize = 200;

/// What a `/health` answer said, as far as it has apfel's shape.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Health {
    /// The model it names, if it names one.
    pub model: Option<String>,
    /// Whether it says its model can answer now, if it says.
    pub available: Option<bool>,
    /// The context window it states, clamped to 256 – 1,048,576.
    pub context_window: Option<u32>,
}

impl fmt::Debug for Health {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Health")
            .field("model_len", &self.model.as_ref().map(String::len))
            .field("available", &self.available)
            .field("context_window", &self.context_window)
            .finish()
    }
}

/// A short name: text, no control characters, at most [`MAX_NAME`] characters.
fn short_name(value: Option<&Value>) -> Option<String> {
    let text = value?.as_str()?.trim();
    (!text.is_empty() && text.chars().count() <= MAX_NAME && !text.chars().any(char::is_control)).then(|| text.to_string())
}

/// A `/health` answer read as apfel's: refused ([`AiError::Malformed`]) when it isn't a JSON
/// object with a `status` word and at least one of apfel's facts, or when it names a model other
/// than [`APPLE_MODEL`].
pub fn parse_health(body: &[u8]) -> Result<Health, AiError> {
    if body.len() > MAX_HEALTH_BYTES {
        return Err(AiError::TooLarge);
    }
    let not_apfel = || AiError::Malformed("it doesn't answer as apfel does".into());
    let value: Value = serde_json::from_slice(body).map_err(|_| not_apfel())?;
    let object = value.as_object().ok_or_else(not_apfel)?;
    if short_name(object.get("status")).is_none() {
        return Err(not_apfel());
    }
    let model = short_name(object.get("model"));
    if object.contains_key("model") && model.as_deref() != Some(APPLE_MODEL) {
        return Err(not_apfel());
    }
    let available = ["model_available", "available"].iter().find_map(|field| object.get(*field).and_then(Value::as_bool));
    let context_window = ["context_window", "contextWindow", "context_length", "max_context"]
        .iter()
        .find_map(|field| object.get(*field).and_then(Value::as_u64))
        .map(clamp_context_window);
    if model.is_none() && available.is_none() && context_window.is_none() {
        return Err(not_apfel());
    }
    Ok(Health { model, available, context_window })
}

/// Whether `list` is apfel's: exactly one model, [`APPLE_MODEL`].
pub fn lists_only_apple(list: &ModelList) -> bool {
    !list.truncated && list.models.len() == 1 && list.models.iter().all(|m| m.id == APPLE_MODEL)
}

/// The identity of a server whose model list is `list` and whose `/health` said `health`: a
/// short name for both answers, kept with a confirmation and compared at every re-check. None
/// when the server isn't apfel.
pub fn identity(list: &ModelList, health: &Health) -> Option<String> {
    if !lists_only_apple(list) {
        return None;
    }
    let model = health.model.as_deref().unwrap_or(APPLE_MODEL);
    Some(format!("apfel:{model}:{}", fingerprint(list)))
}

/// Whether `text` could be an [`identity`] (as read back from the settings file).
pub fn is_identity(text: &str) -> bool {
    text.len() <= MAX_NAME + 32 && text.starts_with("apfel:") && text.bytes().all(|b| b.is_ascii_graphic())
}

/// The context window of an apfel server: what `/health` says, else its model list's entry.
pub fn context_window_of(list: &ModelList, health: &Health) -> Option<u32> {
    health.context_window.or_else(|| list.find(APPLE_MODEL).and_then(|m| m.context_window))
}
