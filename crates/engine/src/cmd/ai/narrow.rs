// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Find by Description: `ai.narrow` asks a model for a search in MailCraft's query language from
//! the owner's words, `ai.narrowResult` reads what came back. Neither changes the list: the owner
//! looks at the proposed search and accepts it with `view.search`, or not.
//!
//! - **Only the owner's words go**, with a description of the query language and today's date
//!   in the owner's zone ([`mailcraft_model::ai::narrow`]): no mail. The answer is asked for as JSON (`{"query": …}`,
//!   [`NARROW_SCHEMA`]) at temperature 0.
//! - **The proposal is refined and checked** before it is offered ([`settle`]): what the owner's
//!   words say plainly (a sender, dates, attachments) wins over the model's reading, words that
//!   only describe the asking are dropped, and what is left must be a search MailCraft can read;
//!   when nothing is left, or the last try fails, the owner's own words are the search. One that isn't a search MailCraft can read is asked for again once, with
//!   what was wrong in the engine's own words; a second is an error.
//! - The owner's and the host's alone, never an agent's (`crate::agent`).

use mailcraft_model::ai::{AiKind, ModelRequest, NARROW_SCHEMA, RequestState, narrow_messages, narrow_retry_messages, settle};
use serde_json::{Value, json};

use super::requests::{as_json, failed, request_id};
use crate::cmd::bad;
use crate::{CmdError, CmdResult, Session, p};

/// Most tokens the model may write for a search.
const NARROW_TOKENS: u32 = 256;
/// Times a proposal that isn't a search is asked for again.
const MAX_RETRIES: u8 = 1;

/// `ai.narrow`'s params.
pub const NARROW_PARAMS: &str = r#"{"text": string (1–1000 characters: the owner's description)} — answers {"request": ModelRequest} for the host to send (kind "narrow", the owner's words, the query language and today's date only, no mail); owner and host only"#;
/// `ai.narrowResult`'s params.
pub const RESULT_PARAMS: &str = r#"{"requestId": number (a finished "narrow" request)} — answers {"query": string|null, "valid": bool, "error"?: string, "retry"?: ModelRequest (asked again once, for the host to send)}, or for a failed request {"state": "failed", "kind": string, "message": string, "status"?: number, "beforeFirstByte"?: bool}; the list doesn't change: `view.search` with the query, when the owner accepts it"#;

/// The request that asks for a search for `description`.
fn narrow_request(messages: Vec<mailcraft_model::ai::ChatMessage>) -> Result<ModelRequest, CmdError> {
    ModelRequest::new(AiKind::Narrow, messages, Vec::new(), false, Some(NARROW_TOKENS), Some(0.0), Some(NARROW_SCHEMA.to_string()))
        .map_err(|e| bad(e.to_string()))
}

/// `ai.narrow`.
pub fn narrow(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["text"])?;
    if !v.is_object() {
        return Err(bad("the parameters are an object with `text`"));
    }
    let text = p::req_str(v, "text")?;
    let messages = narrow_messages(text, s.today()).map_err(|e| bad(e.to_string()))?;
    let started = s.ai.start(narrow_request(messages)?).map_err(|e| bad(e.to_string()))?;
    Ok(json!({"request": as_json(&started)?}))
}

/// `ai.narrowResult`.
pub fn narrow_result(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["requestId"])?;
    let id = request_id(v)?;
    let t = s.ai.get(id).ok_or_else(|| bad(format!("no model request {id}")))?;
    if t.request.kind != AiKind::Narrow {
        return Err(bad(format!("model request {id} isn't a search")));
    }
    match t.state {
        RequestState::Done => {}
        RequestState::Pending | RequestState::Streaming => return Err(bad(format!("model request {id} is still under way"))),
        RequestState::Cancelled => return Err(bad(format!("model request {id} was called off"))),
        RequestState::Failed => return Ok(failed(t)),
    }
    let retries = t.retries;
    // The owner's words: the message after the instructions ([`narrow_messages`]).
    let description = t.request.messages.get(1).map(|m| m.content.clone());
    let why = match settle(&t.text, description.as_deref().unwrap_or(""), s.today(), retries >= MAX_RETRIES) {
        Ok(query) => return Ok(json!({"query": query, "valid": true})),
        Err(why) => why,
    };
    if retries >= MAX_RETRIES {
        return Ok(json!({"query": null, "valid": false, "error": why.to_string()}));
    }
    let Some(description) = description else { return Ok(json!({"query": null, "valid": false, "error": why.to_string()})) };
    let messages = narrow_retry_messages(&description, s.today(), &why).map_err(|e| bad(e.to_string()))?;
    let retry = s.ai.start_retry(narrow_request(messages)?, retries.saturating_add(1)).map_err(|e| bad(e.to_string()))?;
    Ok(json!({"query": null, "valid": false, "error": why.to_string(), "retry": as_json(&retry)?}))
}
