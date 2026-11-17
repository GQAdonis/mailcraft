// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An answer asked for as JSON: `{"answer": string, "citations": [integer 1..N]}`.
//!
//! - **The schema** ([`answer_schema`]): at most [`MAX_ANSWER_CITATIONS`] citations and **none
//!   required** (no `minItems`): an answer of "the messages don't say" names no message, and a
//!   citation the schema forced would be one the model made up. It keeps to the portable subset
//!   (`super::schema`), so every provider takes it.
//! - **Read as hostile input** ([`read_answer_json`]): what a model wrote is checked and capped
//!   before anything keeps it. `answer` is a string, cleaned (`clean_text`) and capped at
//!   `MAX_RESPONSE_BYTES`; `citations` is a list, and only numbers of the messages given (1 to
//!   N), each once, at most three, are kept: the rest are counted as dropped. A document that
//!   isn't such an object is refused whole, so the caller can say that it couldn't read which
//!   messages the answer came from.
//! - **Not found** ([`NOT_FOUND`], [`is_not_found`]): the one sentence the prompt asks for when
//!   the messages don't answer the question. An answer that names no message and isn't that
//!   sentence is uncited, and is shown as such.
//!
//! Streamed, the answer's text is read as it comes by [`super::extract`].

use serde_json::Value;

use super::AiError;
use super::call::{MAX_RESPONSE_BYTES, clean_text};
use super::prompt::MAX_PASSAGES;

/// Most citations an answer keeps (and the schema's `maxItems`).
pub const MAX_ANSWER_CITATIONS: usize = 3;
/// Most bytes of an answer's JSON read.
pub const MAX_ANSWER_JSON_BYTES: usize = 1024 * 1024;
/// Most entries of `citations` looked at.
const MAX_READ_CITATIONS: usize = 64;
/// The sentence the prompt asks for when the messages don't answer the question.
pub const NOT_FOUND: &str = "The messages don't say.";

/// The answer schema for the most messages a prompt gives ([`answer_schema`] of
/// [`MAX_PASSAGES`]): what the schema check holds to the portable subset.
pub const ANSWER_SCHEMA: &str = r#"{"type":"object","properties":{"answer":{"type":"string"},"citations":{"type":"array","items":{"type":"integer","minimum":1,"maximum":20},"maxItems":3}},"required":["answer","citations"],"additionalProperties":false}"#;

/// The answer schema for a prompt of `sources` messages: citations are 1 to `sources` (at least
/// 1, at most [`MAX_PASSAGES`]), at most [`MAX_ANSWER_CITATIONS`], none required.
pub fn answer_schema(sources: usize) -> String {
    let max = sources.clamp(1, MAX_PASSAGES);
    format!(
        r#"{{"type":"object","properties":{{"answer":{{"type":"string"}},"citations":{{"type":"array","items":{{"type":"integer","minimum":1,"maximum":{max}}},"maxItems":{MAX_ANSWER_CITATIONS}}}}},"required":["answer","citations"],"additionalProperties":false}}"#
    )
}

/// An answer read from its JSON: its text, whether that was cut, the numbers of the prompt it
/// names (each a message given, each once, at most three, in the order named) and how many
/// citations were let go of (not a message given, not a number, past three).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct StructuredAnswer {
    pub answer: String,
    pub truncated: bool,
    pub citations: Vec<usize>,
    pub dropped: usize,
}

/// `json`, an answer a model wrote for a prompt of `sources` messages, read and checked.
/// Refused ([`AiError::Invalid`]) when it is larger than [`MAX_ANSWER_JSON_BYTES`], isn't JSON,
/// isn't an object, has no string `answer`, or has a `citations` that isn't a list (a missing
/// or null one names no message). Other members are let go of.
pub fn read_answer_json(json: &str, sources: usize) -> Result<StructuredAnswer, AiError> {
    if json.len() > MAX_ANSWER_JSON_BYTES {
        return Err(AiError::Invalid(format!("the answer is larger than {MAX_ANSWER_JSON_BYTES} bytes")));
    }
    let value: Value = serde_json::from_str(json.trim()).map_err(|_| AiError::Invalid("the answer isn't JSON".into()))?;
    let object = value.as_object().ok_or_else(|| AiError::Invalid("the answer isn't a JSON object".into()))?;
    let answer = object.get("answer").and_then(Value::as_str).ok_or_else(|| AiError::Invalid("the answer has no text".into()))?;
    let list: &[Value] = match object.get("citations") {
        None | Some(Value::Null) => &[],
        Some(Value::Array(list)) => list.as_slice(),
        Some(_) => return Err(AiError::Invalid("the answer's citations aren't a list".into())),
    };
    let mut citations: Vec<usize> = Vec::new();
    let mut dropped = 0usize;
    for (i, c) in list.iter().enumerate() {
        if i >= MAX_READ_CITATIONS {
            dropped = dropped.saturating_add(list.len().saturating_sub(i));
            break;
        }
        let n = c.as_u64().and_then(|n| usize::try_from(n).ok()).filter(|n| (1..=sources).contains(n));
        match n {
            // The same message named again is kept once.
            Some(n) if citations.contains(&n) => {}
            Some(n) if citations.len() < MAX_ANSWER_CITATIONS => citations.push(n),
            _ => dropped = dropped.saturating_add(1),
        }
    }
    let (answer, truncated) = clean_text(answer, MAX_RESPONSE_BYTES);
    Ok(StructuredAnswer { answer, truncated, citations, dropped })
}

/// Whether `answer` is the sentence that says the messages don't answer the question
/// ([`NOT_FOUND`]), as a model writes it: any case, a curly apostrophe, "do not", and words
/// after it ("The messages don't say who signed it.").
pub fn is_not_found(answer: &str) -> bool {
    let start: String = answer.trim_start().chars().take(32).map(|c| if c == '\u{2019}' { '\'' } else { c.to_ascii_lowercase() }).collect();
    ["the messages don't say", "the messages do not say"].iter().any(|s| start.starts_with(s))
}

/// An answer is mail: written out by its lengths and numbers, never its text.
impl std::fmt::Debug for StructuredAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StructuredAnswer")
            .field("answer_bytes", &self.answer.len())
            .field("truncated", &self.truncated)
            .field("citations", &self.citations)
            .field("dropped", &self.dropped)
            .finish()
    }
}
