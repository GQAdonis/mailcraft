// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Answers from the owner's mail: `ai.ask` finds the messages that answer a question
//! ([`super::retrieve`]) and asks a model for an answer from them, streamed; `ai.answerResult`
//! reads the finished answer's citations.
//!
//! - **Nothing found, nothing sent**: when no message answers the question, no model is asked.
//! - **What goes** is the prompt of [`mailcraft_model::ai::prompt`]: the instructions apart, each
//!   passage between the lines of a marker made fresh for the request, each message's number,
//!   subject, sender, date and passage of its visible text (never what the sender hid, never an
//!   attachment), at most 20, fitted to the model's context window. No tools.
//! - **Asked for as JSON**, `{"answer", "citations"}` ([`answer_schema`]: at most three
//!   citations, none required), streamed: the host shows the answer's text as it comes and
//!   hands the whole document in at the end, where it is read and checked
//!   (`mailcraft_model::ai::read_answer_json`). A provider that can't give JSON answers in plain
//!   text, and then the citations are read from the text (`[n]`, "message 2", `(2)`).
//! - **Citations** are kept only for messages that were sent ([`validate_citations`]), at most
//!   [`MAX_SHOWN`]; the others are counted as dropped. Their subjects come from the mailbox, never
//!   from the model. A finished answer that names no message and isn't the sentence that says
//!   the messages don't answer is `uncited`.
//! - **The request carries its passages** (`ModelRequest::answer`), so that the host can make the
//!   prompt again for a backup model's context window, and **says who answered** (`answeredBy`).
//! - **`contextWindow`** is the window of the provider that is to answer, which only the window
//!   and the host know; like the rest of `ai.ask`, never an agent's.
//! - The owner's and the host's alone, never an agent's (`crate::agent`). Message text is data,
//!   never instructions, and so is the answer.

use std::hash::{BuildHasher, Hasher};

use mailcraft_model::Id;
use mailcraft_model::ai::{
    AiKind, AnswerInput, Citation, DEFAULT_CONTEXT_WINDOW, ErrorKind, MAX_ANSWER_CITATIONS, MAX_PASSAGES, ModelRequest, Passage, RequestState,
    RetrievalRequest, answer_schema, answer_tokens_for, cited_numbers, is_not_found, marker, validate_citations,
};
use mailcraft_time::DateTime;
use serde_json::{Value, json};

use super::UNTRUSTED;
use super::requests::{as_json, failed, request_id};
use super::retrieve::{retrieve_now, semantic_of};
use crate::cmd::bad;
use crate::{CmdError, CmdResult, Session, p};

/// Most citations shown with an answer.
pub const MAX_SHOWN: usize = MAX_ANSWER_CITATIONS;
/// Messages looked for when the caller doesn't say.
const DEFAULT_PASSAGES: u64 = 8;
/// The smallest and the largest context window taken, in tokens (a provider's own bounds).
const MIN_WINDOW: u64 = 256;
const MAX_WINDOW: u64 = 1 << 20;
/// The answer's temperature.
const TEMPERATURE: f32 = 0.2;

/// `ai.ask`'s params.
pub const ASK_PARAMS: &str = r#"{"question": string (1–2000 characters), "limit"?: 1–20 messages (8), "contextWindow"?: tokens, 256–1048576 (4096: the window of the provider that is to answer; owner and host only), "nonce"?: number (randomness for the request's marker; the engine makes its own when left out), "queryVector"?: [number], "embedder"?: string (the question's vector and its model, as for `ai.retrieve`)} — answers {"request": ModelRequest|null (asked for as JSON {"answer", "citations"} by its `schema`, streamed, carrying `answer`: {"question", "passages": [{"message", "subject", "from", "date", "text"}], "marker"} for the host to make the prompt again for a backup), "state": "pending"|"done", "nothingFound": bool, "sources": [{"number", "id", "subject"}], "semantic": bool, "untrusted": string}; no model is asked when nothing is found; owner and host only"#;
/// `ai.answerResult`'s params.
pub const RESULT_PARAMS: &str = r#"{"requestId": number (a finished or stopped "answer" request)} — answers {"text": string, "state": string, "citations": [{"number", "id", "subject", "from"}] (at most 3, only messages that were sent), "dropped": number, "structured": bool (the citations came from the answer's JSON), "uncited": bool (finished, names no message, and isn't the sentence that says the messages don't answer), "answeredBy"?: {"provider", "model"} (as the host said), "untrusted": string}; for a failed request {"state": "failed", "kind": string, "message": string, "status"?: number (a refusal's HTTP status), "beforeFirstByte"?: bool (a time-out)}, and for an answer whose JSON couldn't be read after its text was shown also "text" and "sourcesUnreadable": true"#;

/// Randomness for a request's marker when the caller gave none.
fn fresh_nonce(s: &Session) -> u64 {
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_i64(s.now());
    h.write_usize(s.ai.all().len());
    h.finish()
}

/// A number under `k` in `v` within `min..=max` (`None`: left out).
fn number_in(v: &Value, k: &str, min: u64, max: u64) -> Result<Option<u64>, CmdError> {
    match v.get(k) {
        None => Ok(None),
        Some(_) => {
            p::u64(v, k).filter(|n| (min..=max).contains(n)).map(Some).ok_or_else(|| bad(format!("`{k}` is a whole number from {min} to {max}")))
        }
    }
}

/// `ai.ask`.
pub fn ask(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["question", "limit", "contextWindow", "nonce", "queryVector", "embedder"])?;
    if !v.is_object() {
        return Err(bad("the parameters are an object with `question`"));
    }
    let question = p::req_str(v, "question")?.trim().to_string();
    if question.is_empty() || question.chars().nth(mailcraft_model::ai::MAX_QUESTION_CHARS).is_some() {
        return Err(bad("`question` is 1 to 2000 characters"));
    }
    let limit = number_in(v, "limit", 1, MAX_PASSAGES as u64)?.unwrap_or(DEFAULT_PASSAGES);
    let window = number_in(v, "contextWindow", MIN_WINDOW, MAX_WINDOW)?
        .map_or(DEFAULT_CONTEXT_WINDOW, |n| usize::try_from(n).unwrap_or(DEFAULT_CONTEXT_WINDOW));
    let nonce = match v.get("nonce") {
        None => fresh_nonce(s),
        Some(_) => p::u64(v, "nonce").ok_or_else(|| bad("`nonce` is a whole number"))?,
    };
    let semantic = semantic_of(v)?;
    let request = RetrievalRequest::new(&question, Some(limit), None, None, None).map_err(|e| bad(e.to_string()))?;
    let (found, semantic) = retrieve_now(s, &request, semantic.as_ref())?;
    if found.hits.is_empty() {
        return Ok(json!({"request": null, "state": "done", "nothingFound": true, "sources": [], "semantic": semantic, "untrusted": UNTRUSTED}));
    }
    let passages: Vec<Passage> = found.hits.iter().filter_map(|h| passage(s, h.message, &h.chunk.text)).collect();
    let input = AnswerInput { question, passages, marker: marker(nonce) };
    let prompt = input.prompt(window).map_err(|e| bad(e.to_string()))?;
    // The request keeps the passages of the messages sent, in the prompt's order, so that a
    // prompt made again for a backup holds none but these.
    let kept: Vec<Passage> = prompt.sources.iter().filter_map(|id| input.passages.iter().find(|p| p.message == *id).cloned()).collect();
    let input = AnswerInput { passages: kept, ..input };
    let tokens = u32::try_from(answer_tokens_for(window)).unwrap_or(u32::MAX);
    let schema = Some(answer_schema(prompt.sources.len()));
    let made = ModelRequest::new(AiKind::Answer, prompt.messages, prompt.sources.clone(), true, Some(tokens), Some(TEMPERATURE), schema)
        .and_then(|r| r.with_answer(input))
        .map_err(|e| bad(e.to_string()))?;
    let started = s.ai.start(made).map_err(|e| bad(e.to_string()))?;
    let sources: Vec<Value> = prompt
        .sources
        .iter()
        .enumerate()
        .map(|(i, id)| json!({"number": i.saturating_add(1), "id": id, "subject": s.mailbox.message(*id).map(|m| m.subject.as_str()).unwrap_or_default()}))
        .collect();
    Ok(
        json!({"request": as_json(&started)?, "state": "pending", "nothingFound": false, "sources": sources, "semantic": semantic, "untrusted": UNTRUSTED}),
    )
}

/// Message `id` as the model is to read it, with `text` its passage; `None` when it is gone.
fn passage(s: &Session, id: Id, text: &str) -> Option<Passage> {
    let m = s.mailbox.message(id)?;
    let from = if m.from.name.trim().is_empty() { m.from.email.clone() } else { format!("{} <{}>", m.from.name, m.from.email) };
    let date = DateTime::from_timestamp(m.date, s.offset_at(m.date)).date.iso();
    Some(Passage { message: id, subject: m.subject.clone(), from, date, text: text.to_string() })
}

/// `ai.answerResult`.
pub fn answer_result(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["requestId"])?;
    let id = request_id(v)?;
    let t = s.ai.get(id).ok_or_else(|| bad(format!("no model request {id}")))?;
    if t.request.kind != AiKind::Answer {
        return Err(bad(format!("model request {id} isn't an answer")));
    }
    match t.state {
        RequestState::Done | RequestState::Cancelled => {}
        RequestState::Pending | RequestState::Streaming => return Err(bad(format!("model request {id} is still under way"))),
        RequestState::Failed => return Ok(failed_answer(t)),
    }
    let sources = t.sources();
    let (numbers, invented) = match &t.cited {
        Some(cited) => (cited.clone(), t.cited_dropped),
        None => cited_numbers(&t.text, sources.len()),
    };
    let named: Vec<(usize, Id)> = numbers
        .iter()
        .filter_map(|n| n.checked_sub(1).and_then(|i| sources.get(i)).map(|id| (*n, *id)))
        .take(mailcraft_model::ai::MAX_CITATIONS)
        .collect();
    let cited: Vec<Citation> = named.iter().map(|(_, id)| Citation::message(*id)).collect();
    // Only messages that were sent: by construction, and checked again.
    let kept: Vec<(usize, Id)> = match validate_citations(&cited, sources) {
        Ok(_) => named,
        Err(_) => Vec::new(),
    };
    let mut dropped = invented.saturating_add(numbers.len().saturating_sub(kept.len()));
    let mut citations = Vec::new();
    for (n, id) in kept {
        let Some(m) = s.mailbox.message(id) else {
            dropped = dropped.saturating_add(1);
            continue;
        };
        if citations.len() >= MAX_SHOWN {
            dropped = dropped.saturating_add(1);
            continue;
        }
        citations.push(json!({"number": n, "id": id, "subject": m.subject, "from": m.from.email}));
    }
    let uncited = t.state == RequestState::Done && citations.is_empty() && !t.text.trim().is_empty() && !is_not_found(&t.text);
    let mut v = json!({
        "text": t.text, "state": as_json(&t.state)?, "citations": citations, "dropped": dropped, "structured": t.cited.is_some(),
        "uncited": uncited, "untrusted": UNTRUSTED,
    });
    if let (Some(by), Some(o)) = (&t.answered_by, v.as_object_mut()) {
        o.insert("answeredBy".into(), as_json(by)?);
    }
    Ok(v)
}

/// A failed answer, as [`failed`] says it; one whose JSON couldn't be read after its text was
/// shown keeps that text, with `"sourcesUnreadable": true`.
fn failed_answer(t: &mailcraft_model::ai::Tracked) -> Value {
    let mut v = failed(t);
    if t.error_kind == Some(ErrorKind::Malformed)
        && !t.text.is_empty()
        && let Some(o) = v.as_object_mut()
    {
        o.insert("text".into(), json!(t.text));
        o.insert("sourcesUnreadable".into(), json!(true));
        o.insert("untrusted".into(), json!(UNTRUSTED));
    }
    v
}
