// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The OpenAI-compatible requests and answers, as this crate's own types: what is written to
//! `/v1/chat/completions` and `/v1/embeddings`, and what is read back.
//!
//! What is written is checked first (counts and sizes). What is read back is hostile: it is read
//! leniently (unknown fields are ignored, a missing one is none) but within limits, and nothing
//! in it is trusted beyond being text or numbers. `Debug` shows lengths, never the text: a request
//! holds mail and an answer is about it.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{AiError, tidy};

/// Most messages in one request.
pub const MAX_MESSAGES: usize = 256;
/// Longest message, in bytes.
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
/// Longest request body, in bytes.
pub const MAX_REQUEST_BYTES: usize = 4 * 1024 * 1024;
/// Longest answer taken, in bytes of text.
pub const MAX_ANSWER_BYTES: usize = 256 * 1024;
/// Longest JSON schema sent, and longest structured answer read, in bytes.
pub const MAX_SCHEMA_BYTES: usize = 64 * 1024;
pub const MAX_JSON_ANSWER_BYTES: usize = 64 * 1024;
/// Most texts embedded in one request, and the longest of them, in bytes.
pub const MAX_EMBED_INPUTS: usize = 256;
pub const MAX_EMBED_INPUT_BYTES: usize = 64 * 1024;
/// Most dimensions of an embedding: as many as the engine keeps (`retrieval::vectors::MAX_DIMS`).
pub const MAX_DIMENSIONS: usize = 4_096;
/// The most tokens an answer may be asked to have.
const MAX_TOKENS_CEILING: u32 = 65_536;
/// Longest name (a model's, a finish reason) kept of an answer.
const MAX_NAME: usize = 200;
/// Longest name of a JSON schema.
const MAX_SCHEMA_NAME: usize = 64;

/// Who says a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

/// One message of a conversation with a model.
#[derive(Clone, PartialEq, Eq)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Message {
        Message { role: Role::System, content: content.into() }
    }

    pub fn user(content: impl Into<String>) -> Message {
        Message { role: Role::User, content: content.into() }
    }

    pub fn assistant(content: impl Into<String>) -> Message {
        Message { role: Role::Assistant, content: content.into() }
    }
}

impl fmt::Debug for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Message").field("role", &self.role).field("content_len", &self.content.len()).finish()
    }
}

/// The form the answer must take (`response_format`).
#[derive(Clone, Debug, PartialEq)]
pub enum ResponseFormat {
    /// Any JSON object.
    JsonObject,
    /// JSON that conforms to `schema` (a JSON Schema object), named `name`.
    JsonSchema { name: String, schema: Value, strict: bool },
}

/// A chat request: the model is the provider's ([`crate::ProviderConfig::model`]).
#[derive(Clone, Default, PartialEq)]
pub struct ChatRequest {
    pub messages: Vec<Message>,
    /// The most tokens the answer may have: always worth saying, since some servers otherwise
    /// take the whole remaining window.
    pub max_tokens: Option<u32>,
    /// 0 to 2.
    pub temperature: Option<f32>,
    pub seed: Option<u64>,
    pub response_format: Option<ResponseFormat>,
}

impl fmt::Debug for ChatRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatRequest")
            .field("messages", &self.messages.len())
            .field("bytes", &self.messages.iter().map(|m| m.content.len()).sum::<usize>())
            .field("max_tokens", &self.max_tokens)
            .field("structured", &self.response_format.is_some())
            .finish()
    }
}

/// What an answer cost, as the provider counts it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

/// A model's whole answer.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ChatAnswer {
    /// The answer's text: data to show, never instructions.
    pub text: String,
    /// Why the model stopped (`stop`, `length`…), tidied.
    pub finish_reason: Option<String>,
    /// The model that answered, as the provider names it, tidied.
    pub model: Option<String>,
    pub usage: Option<Usage>,
}

impl fmt::Debug for ChatAnswer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatAnswer")
            .field("text_len", &self.text.len())
            .field("finish_reason", &self.finish_reason)
            .field("model", &self.model)
            .field("usage", &self.usage)
            .finish()
    }
}

/// One piece of a streamed answer.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Chunk {
    pub text: String,
    pub finish_reason: Option<String>,
    pub model: Option<String>,
    pub usage: Option<Usage>,
}

impl fmt::Debug for Chunk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Chunk").field("text_len", &self.text.len()).field("finish_reason", &self.finish_reason).finish()
    }
}

/// Embeddings, one for each text asked about, in the order asked.
#[derive(Clone, Default, PartialEq)]
pub struct Embeddings {
    pub vectors: Vec<Vec<f32>>,
    pub model: Option<String>,
    pub usage: Option<Usage>,
}

impl fmt::Debug for Embeddings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Embeddings")
            .field("count", &self.vectors.len())
            .field("dimensions", &self.vectors.first().map_or(0, Vec::len))
            .field("model", &self.model)
            .finish()
    }
}

fn config(why: &str) -> AiError {
    AiError::Config(why.to_string())
}

fn malformed(why: &str) -> AiError {
    AiError::Malformed(why.to_string())
}

fn valid_schema_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= MAX_SCHEMA_NAME && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

impl ChatRequest {
    /// Whether the request can be sent: 1 to [`MAX_MESSAGES`] messages, none longer than
    /// [`MAX_MESSAGE_BYTES`], a sensible answer length and temperature, and a schema that is one.
    pub fn check(&self) -> Result<(), AiError> {
        if self.messages.is_empty() || self.messages.len() > MAX_MESSAGES {
            return Err(config("a request holds 1 to 256 messages"));
        }
        if self.messages.iter().any(|m| m.content.len() > MAX_MESSAGE_BYTES) {
            return Err(config("a message is too long to send"));
        }
        if self.max_tokens.is_some_and(|n| n == 0 || n > MAX_TOKENS_CEILING) {
            return Err(config("the answer's length must be 1 to 65,536 tokens"));
        }
        if self.temperature.is_some_and(|t| !t.is_finite() || !(0.0..=2.0).contains(&t)) {
            return Err(config("the temperature must be between 0 and 2"));
        }
        if let Some(ResponseFormat::JsonSchema { name, schema, .. }) = &self.response_format {
            if !valid_schema_name(name) {
                return Err(config("the schema's name must be 1 to 64 letters, digits, dashes or underscores"));
            }
            let size = serde_json::to_vec(schema).map_or(usize::MAX, |bytes| bytes.len());
            if !schema.is_object() || size > MAX_SCHEMA_BYTES {
                return Err(config("the schema must be a JSON object of at most 64 KB"));
            }
        }
        Ok(())
    }
}

#[derive(Serialize)]
struct WireMessage<'a> {
    role: Role,
    content: &'a str,
}

#[derive(Serialize)]
struct WireChat<'a> {
    model: &'a str,
    messages: Vec<WireMessage<'a>>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    seed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<Value>,
}

/// `response_format` as it is written.
fn format_value(format: &ResponseFormat) -> Value {
    let mut outer = Map::new();
    match format {
        ResponseFormat::JsonObject => {
            outer.insert("type".into(), Value::from("json_object"));
        }
        ResponseFormat::JsonSchema { name, schema, strict } => {
            let mut inner = Map::new();
            inner.insert("name".into(), Value::from(name.as_str()));
            inner.insert("strict".into(), Value::Bool(*strict));
            inner.insert("schema".into(), schema.clone());
            outer.insert("type".into(), Value::from("json_schema"));
            outer.insert("json_schema".into(), Value::Object(inner));
        }
    }
    Value::Object(outer)
}

/// The body of a chat request to `model`: streamed (SSE) or not.
pub fn chat_body(model: &str, request: &ChatRequest, stream: bool) -> Result<Vec<u8>, AiError> {
    request.check()?;
    let wire = WireChat {
        model: model.trim(),
        messages: request.messages.iter().map(|m| WireMessage { role: m.role, content: &m.content }).collect(),
        stream,
        max_tokens: request.max_tokens,
        temperature: request.temperature,
        seed: request.seed,
        response_format: request.response_format.as_ref().map(format_value),
    };
    let body = serde_json::to_vec(&wire).map_err(|_| config("the request couldn't be written"))?;
    if body.len() > MAX_REQUEST_BYTES {
        return Err(config("the request is too large to send"));
    }
    Ok(body)
}

#[derive(Serialize)]
struct WireEmbed<'a> {
    model: &'a str,
    input: &'a [String],
}

/// The body of an embeddings request to `model` for `inputs`: 1 to [`MAX_EMBED_INPUTS`] texts,
/// none longer than [`MAX_EMBED_INPUT_BYTES`].
pub fn embeddings_body(model: &str, inputs: &[String]) -> Result<Vec<u8>, AiError> {
    if inputs.is_empty() || inputs.len() > MAX_EMBED_INPUTS {
        return Err(config("an embeddings request holds 1 to 256 texts"));
    }
    if inputs.iter().any(|text| text.len() > MAX_EMBED_INPUT_BYTES) {
        return Err(config("a text is too long to embed"));
    }
    let body = serde_json::to_vec(&WireEmbed { model: model.trim(), input: inputs }).map_err(|_| config("the request couldn't be written"))?;
    if body.len() > MAX_REQUEST_BYTES {
        return Err(config("the request is too large to send"));
    }
    Ok(body)
}

/// What an answer's `error` says, when it has one: `{"error": {"message": …}}` or `{"error": "…"}`.
pub fn error_message(value: &Value) -> Option<String> {
    let error = value.get("error")?;
    let message = match error {
        Value::String(text) => text.as_str(),
        Value::Object(_) => error.get("message").and_then(Value::as_str).unwrap_or("an error without words"),
        Value::Null => return None,
        _ => "an error without words",
    };
    Some(tidy(message, None))
}

/// A short name from an answer (a model's, a finish reason): text alone, tidied; none when empty.
fn name_of(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(|text| tidy(text, None).chars().take(MAX_NAME).collect::<String>()).filter(|name| !name.is_empty())
}

fn usage_of(value: Option<&Value>) -> Option<Usage> {
    let usage = value.filter(|usage| usage.is_object())?;
    let count = |name: &str| usage.get(name).and_then(Value::as_u64).unwrap_or(0);
    Some(Usage { prompt_tokens: count("prompt_tokens"), completion_tokens: count("completion_tokens"), total_tokens: count("total_tokens") })
}

/// Text from where an answer keeps it: a string, or nothing (`null`, missing); anything else is not an answer.
fn text_of(value: Option<&Value>) -> Result<String, AiError> {
    match value {
        Some(Value::String(text)) => Ok(text.clone()),
        Some(Value::Null) | None => Ok(String::new()),
        Some(_) => Err(malformed("its text isn't text")),
    }
}

/// A whole (not streamed) chat answer.
pub fn parse_completion(body: &[u8]) -> Result<ChatAnswer, AiError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| malformed("it isn't the JSON a chat answer is"))?;
    if let Some(message) = error_message(&value) {
        return Err(AiError::Provider(message));
    }
    let choice =
        value.get("choices").and_then(Value::as_array).and_then(|choices| choices.first()).ok_or_else(|| malformed("it has no answer in it"))?;
    let text = text_of(choice.get("message").and_then(|message| message.get("content")))?;
    if text.len() > MAX_ANSWER_BYTES {
        return Err(AiError::TooLarge);
    }
    Ok(ChatAnswer {
        text,
        finish_reason: name_of(choice.get("finish_reason")),
        model: name_of(value.get("model")),
        usage: usage_of(value.get("usage")),
    })
}

/// One event's data of a streamed chat answer (not `[DONE]`). A chunk with no choices (the
/// usage at the end, a keep-alive) is a chunk of no text.
pub fn parse_chunk(data: &str) -> Result<Chunk, AiError> {
    if data.len() > MAX_ANSWER_BYTES {
        return Err(AiError::TooLarge);
    }
    let value: Value = serde_json::from_str(data).map_err(|_| malformed("a piece of the streamed answer isn't JSON"))?;
    if let Some(message) = error_message(&value) {
        return Err(AiError::Provider(message));
    }
    let choice = value.get("choices").and_then(Value::as_array).and_then(|choices| choices.first());
    let text = text_of(choice.and_then(|choice| choice.get("delta")).and_then(|delta| delta.get("content")))?;
    Ok(Chunk {
        text,
        finish_reason: name_of(choice.and_then(|choice| choice.get("finish_reason"))),
        model: name_of(value.get("model")),
        usage: usage_of(value.get("usage")),
    })
}

#[derive(Deserialize)]
struct WireEmbeddings {
    #[serde(default)]
    data: Vec<WireEmbedding>,
    #[serde(default)]
    model: Option<Value>,
    #[serde(default)]
    usage: Option<Value>,
    #[serde(default)]
    error: Option<Value>,
}

#[derive(Deserialize)]
struct WireEmbedding {
    embedding: Vec<f32>,
    #[serde(default)]
    index: Option<usize>,
}

/// An embeddings answer for `expected` texts: exactly that many vectors, each placed by its
/// `index` (or its place), all of the same length (1 to [`MAX_DIMENSIONS`]), every number finite.
pub fn parse_embeddings(body: &[u8], expected: usize) -> Result<Embeddings, AiError> {
    let wire: WireEmbeddings = serde_json::from_slice(body).map_err(|_| malformed("it isn't the list of embeddings asked for"))?;
    if let Some(error) = wire.error.filter(|error| !error.is_null()) {
        let mut holder = Map::new();
        holder.insert("error".into(), error);
        return Err(AiError::Provider(error_message(&Value::Object(holder)).unwrap_or_default()));
    }
    if wire.data.len() != expected {
        return Err(malformed("it has another number of embeddings than was asked for"));
    }
    let mut slots: Vec<Option<Vec<f32>>> = vec![None; expected];
    let mut dimensions: Option<usize> = None;
    for (position, item) in wire.data.into_iter().enumerate() {
        let slot = slots.get_mut(item.index.unwrap_or(position)).ok_or_else(|| malformed("an embedding is for a text that wasn't asked about"))?;
        if slot.is_some() {
            return Err(malformed("two embeddings are for the same text"));
        }
        let length = item.embedding.len();
        let unlike = dimensions.is_some_and(|d| d != length);
        if length == 0 || length > MAX_DIMENSIONS || unlike || !item.embedding.iter().all(|x| x.is_finite()) {
            return Err(malformed("an embedding isn't a vector of numbers like the others"));
        }
        dimensions = Some(length);
        *slot = Some(item.embedding);
    }
    let vectors: Option<Vec<Vec<f32>>> = slots.into_iter().collect();
    let vectors = vectors.ok_or_else(|| malformed("a text has no embedding"))?;
    let model = name_of(wire.model.as_ref());
    Ok(Embeddings { vectors, model, usage: usage_of(wire.usage.as_ref()) })
}

/// A structured answer (`response_format` JSON) as JSON, a fence around it (```json … ```)
/// allowed. Only JSON comes of it: what it means is for the caller to check, field by field.
pub fn json_answer(text: &str) -> Result<Value, AiError> {
    let trimmed = text.trim();
    let inner = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .and_then(|rest| rest.trim_end().strip_suffix("```"))
        .unwrap_or(trimmed)
        .trim();
    if inner.len() > MAX_JSON_ANSWER_BYTES {
        return Err(AiError::TooLarge);
    }
    serde_json::from_str(inner).map_err(|_| malformed("the structured answer isn't JSON"))
}
