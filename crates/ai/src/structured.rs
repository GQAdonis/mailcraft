// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The structured-output ladder: providers differ in what `response_format` they take, so a
//! structured request is asked at the highest level a provider accepts.
//!
//! 1. [`Level::JsonSchema`]: `response_format: json_schema`, the schema itself.
//! 2. [`Level::JsonObject`]: `response_format: json_object`, with the schema and the word "json"
//!    in the system prompt (Alibaba Model Studio refuses `json_object` without that word).
//! 3. [`Level::Plain`]: no `response_format` at all; the schema is in the system prompt, and the
//!    answer is read by a tolerant reader ([`read_answer`]) that finds the JSON object in it.
//!
//! **Any HTTP 400** to a request that carries a `response_format` steps down a level on the same
//! provider ([`steps_down`]), whatever the error's words (Alibaba's "Json mode response is not
//! supported when enable_thinking is true" names no obvious word). A 401, 403, 429 or 5xx is not a
//! ladder step: it is the provider's failure, for the host to report or fall back on. A 400 to
//! the plain level is the end of the ladder. The level that worked is kept for the provider for
//! the session ([`Ladders`]), so the next request starts there.

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde_json::Value;

use crate::error::AiError;
use crate::wire::{ChatRequest, MAX_JSON_ANSWER_BYTES, Message, ResponseFormat, Role, json_answer};

/// Most providers whose level is kept.
const MAX_KEPT: usize = 64;

/// How a structured answer is asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    JsonSchema,
    JsonObject,
    Plain,
}

impl Level {
    /// The level below, if any.
    pub fn next(self) -> Option<Level> {
        match self {
            Level::JsonSchema => Some(Level::JsonObject),
            Level::JsonObject => Some(Level::Plain),
            Level::Plain => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Level::JsonSchema => "json_schema",
            Level::JsonObject => "json_object",
            Level::Plain => "plain",
        }
    }
}

/// Whether `error`, to a request asked at `level`, steps down the ladder: any 400 to a request
/// that carries a `response_format` (every level but the plain one).
pub fn steps_down(error: &AiError, level: Level) -> bool {
    level != Level::Plain && matches!(error, AiError::Status { status: 400, .. })
}

/// The words added to the system prompt below the first level: the schema, and the word "json".
fn instruction(schema: Option<&Value>, level: Level) -> String {
    let shape = schema.map(Value::to_string).unwrap_or_else(|| "{}".to_string());
    match level {
        Level::JsonSchema => String::new(),
        Level::JsonObject => format!("Answer with a json object that follows this JSON schema: {shape}"),
        Level::Plain => format!("Answer with one json object and nothing else (no words before or after it) that follows this JSON schema: {shape}"),
    }
}

/// `request` (asked with a `response_format`) as it is asked at `level`. The messages themselves
/// are kept; at the lower levels the schema's instruction is added to the (first) system message,
/// or put in one of its own at the start when there is none.
pub fn at_level(request: &ChatRequest, level: Level) -> ChatRequest {
    let schema = match &request.response_format {
        Some(ResponseFormat::JsonSchema { schema, .. }) => Some(schema),
        _ => None,
    };
    let response_format = match level {
        Level::JsonSchema => request.response_format.clone(),
        Level::JsonObject => Some(ResponseFormat::JsonObject),
        Level::Plain => None,
    };
    if level == Level::JsonSchema {
        return ChatRequest { response_format, ..request.clone() };
    }
    let added = instruction(schema, level);
    let mut placed = false;
    let mut messages: Vec<Message> = request
        .messages
        .iter()
        .map(|message| {
            if !placed && message.role == Role::System {
                placed = true;
                Message::system(format!("{}\n\n{added}", message.content))
            } else {
                message.clone()
            }
        })
        .collect();
    if !placed {
        messages.insert(0, Message::system(added));
    }
    ChatRequest { messages, response_format, ..request.clone() }
}

/// A structured answer asked at `level`, as JSON: the answer itself (a fence around it allowed);
/// at the plain level, failing that, the one JSON object written among other words (from its
/// first `{` to its last `}`).
pub fn read_answer(text: &str, level: Level) -> Result<Value, AiError> {
    // Every structured answer asked for is an object: JSON of another shape (`[1, 2]`) isn't one.
    let whole = json_answer(text).and_then(|v| if v.is_object() { Ok(v) } else { Err(AiError::Malformed("the answer isn't a JSON object".into())) });
    if whole.is_ok() || level != Level::Plain {
        return whole;
    }
    let (Some(start), Some(end)) = (text.find('{'), text.rfind('}')) else { return whole };
    // `{` and `}` are single bytes, so both ends are character boundaries.
    let Some(inner) = text.get(start..=end) else { return whole };
    if inner.len() > MAX_JSON_ANSWER_BYTES {
        return Err(AiError::TooLarge);
    }
    serde_json::from_str::<Value>(inner).ok().filter(Value::is_object).ok_or_else(|| AiError::Malformed("the answer has no JSON object in it".into()))
}

/// The level each provider was last answered at, for a session. Keyed by the caller's name for
/// the provider (its id, address and model, so a change of any is asked from the top again).
#[derive(Debug, Default)]
pub struct Ladders {
    levels: Mutex<BTreeMap<String, Level>>,
}

impl Ladders {
    /// The level to start `provider` at: where it last answered, or the top.
    pub fn start(&self, provider: &str) -> Level {
        self.levels.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).get(provider).copied().unwrap_or(Level::JsonSchema)
    }

    /// `provider` answered at `level`.
    pub fn keep(&self, provider: &str, level: Level) {
        let mut levels = self.levels.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if level == Level::JsonSchema {
            levels.remove(provider);
            return;
        }
        if !levels.contains_key(provider) && levels.len() >= MAX_KEPT {
            return;
        }
        levels.insert(provider.to_string(), level);
    }
}
