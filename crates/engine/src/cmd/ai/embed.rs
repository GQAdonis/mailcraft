// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The embedding jobs, as for a Send/Receive: the engine never talks to an embeddings endpoint.
//!
//! - `ai.embedRequest` ([`embed_request`]) answers the next batch of passages that have no vector
//!   yet (or whose message changed), each with its message, and the accounts whose mail the batch
//!   carries, so that the host sends it only to an endpoint the owner allowed that mail to go to.
//!   Messages of accounts the host doesn't name are left out.
//! - `ai.embedApply` ([`embed_apply`]) takes the vectors the host got for that batch, checks them
//!   as anything from outside (as many as the passages, the same length, finite numbers, a length
//!   of 1 to 4,096) and keeps them ([`crate::retrieval::vectors`]).
//!
//! Only when the owner set up an embeddings endpoint does a host ask: the engine only answers.
//! Both are the host's alone, never an agent's (`crate::agent`). The passages are the messages'
//! visible text, never what a sender hid, never an attachment. Desktop only: the browser keeps no
//! vectors, and both are refused there.

use serde_json::Value;
#[cfg(not(target_arch = "wasm32"))]
use serde_json::json;

#[cfg(not(target_arch = "wasm32"))]
use crate::cmd::bad;
#[cfg(not(target_arch = "wasm32"))]
use crate::p;
use crate::{CmdResult, Session};

/// `ai.embedRequest`'s params.
pub const REQUEST_PARAMS: &str = r#"{"model": string (the embeddings model's name: another than the vectors kept were made with starts them again), "accounts"?: [string] (addresses of the accounts whose mail may go to the endpoint; left out: every account), "max"?: 1–64 passages (16)} — answers {"batch": number|null, "model", "items": [{"message", "chunk", "text"}], "accounts": [string] (whose mail the batch carries), "remaining": number (about: messages that may go without vectors yet), "full": bool, "untrusted": string}; host only"#;
/// `ai.embedApply`'s params.
pub const APPLY_PARAMS: &str = r#"{"batch": number, "model": string, "vectors": [[number]] (one a passage of the batch, in its order, each of 1–4096 finite numbers)} — answers {"stored": number, "vectors": number, "dims": number}; host only"#;

/// Most passages a batch carries, and what it carries when the host doesn't say.
#[cfg(not(target_arch = "wasm32"))]
const MAX_BATCH: u64 = 64;
#[cfg(not(target_arch = "wasm32"))]
const DEFAULT_BATCH: u64 = 16;

/// The model named by `model`: 1 to 200 characters.
#[cfg(not(target_arch = "wasm32"))]
fn model_of(v: &Value) -> Result<String, crate::CmdError> {
    let model = p::req_str(v, "model")?.trim();
    if model.is_empty() || model.len() > crate::retrieval::vectors::MAX_EMBEDDER || model.chars().any(char::is_control) {
        return Err(bad("`model` is the embeddings model's name, 1 to 200 characters"));
    }
    Ok(model.to_string())
}

/// `ai.embedRequest`.
#[cfg(not(target_arch = "wasm32"))]
pub fn embed_request(s: &mut Session, v: &Value) -> CmdResult {
    use std::collections::BTreeSet;

    use mailcraft_model::ai::{chunks, searchable_text};

    use crate::retrieval::vectors::CHUNKS_A_MESSAGE;

    /// What the host calls mail in no account.
    const LOCAL_MAIL: &str = "local";
    use crate::words::fingerprint;

    p::only(v, &["model", "accounts", "max"])?;
    if !v.is_object() {
        return Err(bad("the parameters are an object with `model`"));
    }
    let model = model_of(v)?;
    let max = match v.get("max") {
        None => DEFAULT_BATCH,
        Some(_) => p::u64(v, "max").filter(|n| (1..=MAX_BATCH).contains(n)).ok_or_else(|| bad("`max` is a whole number from 1 to 64"))?,
    };
    let allowed: Option<Vec<String>> = match v.get("accounts") {
        None => None,
        Some(Value::Array(list)) if list.len() <= 1_000 => {
            let wrong = || bad("`accounts` is a list of the accounts' addresses");
            Some(list.iter().map(|a| a.as_str().map(|e| e.trim().to_lowercase()).ok_or_else(wrong)).collect::<Result<_, _>>()?)
        }
        Some(_) => return Err(bad("`accounts` is a list of the accounts' addresses")),
    };
    // Another model: what was kept goes.
    if s.vectors_mut().embedder() != model {
        s.vectors_mut().reset(&model);
    }
    // Messages gone or changed since they were embedded: their vectors go.
    let stale: Vec<u64> = s
        .vectors()
        .map(|index| index.messages().filter(|(id, fp)| s.mailbox.message(*id).is_none_or(|m| fingerprint(m) != *fp)).map(|(id, _)| id).collect())
        .unwrap_or_default();
    for id in stale {
        s.vectors_mut().remove(id);
    }
    let full = s.vectors().is_some_and(|i| i.full());
    // The account a message's mail is of, by address: `local` for mail in no account (as the host
    // names it).
    let account_of =
        |m: &mailcraft_model::Message| s.mailbox.account(m.account).map_or_else(|| LOCAL_MAIL.to_string(), |a| a.email.trim().to_lowercase());
    let carried = |email: &String| allowed.as_ref().is_none_or(|a| a.contains(email));
    // How many of the messages that may go have no vectors yet, counted without reading them:
    // those of the accounts named, less those held (every one held is current after the sweep).
    let held: usize =
        s.vectors().map(|i| i.messages().filter(|(id, _)| s.mailbox.message(*id).is_some_and(|m| carried(&account_of(m)))).count()).unwrap_or(0);
    let mut items: Vec<(u64, u64, u16, String)> = Vec::new();
    let mut accounts: BTreeSet<String> = BTreeSet::new();
    let mut may_go = 0usize;
    let mut filled = false;
    let room = usize::try_from(max).unwrap_or(16);
    for m in s.mailbox.messages.values() {
        let email = account_of(m.as_ref());
        if !carried(&email) {
            continue;
        }
        may_go = may_go.saturating_add(1);
        // Once the batch is filled, only counted: nothing more is read or fingerprinted.
        if full || filled || items.len() >= room {
            continue;
        }
        let fp = fingerprint(m);
        if s.vectors().is_some_and(|i| i.current(m.id, fp)) {
            continue;
        }
        let Ok(loaded) = s.loaded_message_where(m.id, |_| false) else { continue };
        let text = searchable_text(&loaded.subject, &loaded.body.text);
        let passages: Vec<_> = chunks(m.id, &text).into_iter().take(CHUNKS_A_MESSAGE).collect();
        // A message's passages go in one batch (it counts as embedded once they are back), but
        // for one alone larger than the batch.
        if !items.is_empty() && items.len().saturating_add(passages.len()) > room {
            filled = true;
            continue;
        }
        for c in passages.into_iter().take(room) {
            items.push((m.id, fp, c.ordinal, c.text));
        }
        accounts.insert(email);
    }
    let remaining = may_go.saturating_sub(held);
    s.vectors_mut().flush();
    if items.is_empty() {
        // Nothing left to embed: the load is over, and the room it didn't use is let go of.
        s.vectors_mut().shrink_to_fit();
        return Ok(
            json!({"batch": null, "model": model, "items": [], "accounts": [], "remaining": remaining, "full": full, "untrusted": super::UNTRUSTED}),
        );
    }
    // The passages still to come, counted: room for them all at once rather than growing.
    s.vectors_mut().plan(remaining.saturating_mul(CHUNKS_A_MESSAGE));
    let listed: Vec<Value> = items.iter().map(|(id, _, chunk, text)| json!({"message": id, "chunk": chunk, "text": text})).collect();
    let batch = s.vectors_mut().hand_out(items.into_iter().map(|(id, fp, chunk, _)| (id, fp, chunk)).collect()).id;
    Ok(
        json!({"batch": batch, "model": model, "items": listed, "accounts": accounts, "remaining": remaining, "full": false, "untrusted": super::UNTRUSTED}),
    )
}

/// `ai.embedApply`.
#[cfg(not(target_arch = "wasm32"))]
pub fn embed_apply(s: &mut Session, v: &Value) -> CmdResult {
    use crate::retrieval::vectors::MAX_DIMS;

    p::only(v, &["batch", "model", "vectors"])?;
    if !v.is_object() {
        return Err(bad("the parameters are an object with `batch`, `model` and `vectors`"));
    }
    let id = p::u64(v, "batch").filter(|n| *n != 0).ok_or_else(|| bad("`batch` is the number `ai.embedRequest` answered"))?;
    let model = model_of(v)?;
    let wrong = || bad("`vectors` is a list of vectors, one a passage of the batch, each of 1 to 4096 finite numbers of one length");
    let list = v.get("vectors").and_then(Value::as_array).filter(|l| l.len() as u64 <= MAX_BATCH).ok_or_else(wrong)?;
    let mut vectors: Vec<Vec<f32>> = Vec::with_capacity(list.len());
    for x in list {
        let numbers = x.as_array().filter(|n| !n.is_empty() && n.len() <= MAX_DIMS).ok_or_else(wrong)?;
        let vector: Vec<f32> =
            numbers.iter().map(|n| n.as_f64().map(|f| f as f32).filter(|f| f.is_finite()).ok_or_else(wrong)).collect::<Result<_, _>>()?;
        vectors.push(vector);
    }
    let dims = vectors.first().map_or(0, Vec::len);
    if vectors.iter().any(|x| x.len() != dims) {
        return Err(wrong());
    }
    let index = s.vectors_mut();
    if index.embedder() != model {
        return Err(bad("the vectors are of another model than the batch asked for"));
    }
    let expected =
        index.waiting(id).map(|b| b.items.len()).ok_or_else(|| bad(format!("no batch {id} is waiting (another was asked for since, or none)")))?;
    if expected != vectors.len() {
        return Err(bad(format!("batch {id} has {expected} passages, and {} vectors came", vectors.len())));
    }
    let batch = index.take_batch(id).ok_or_else(|| bad(format!("no batch {id} is waiting")))?;
    // Vectors of another length than those kept: the embedder changed its mind; start again.
    if index.dims() != 0 && index.dims() != dims {
        index.reset(&model);
    }
    let mut stored = 0usize;
    let mut first_error = None;
    for ((message, fp, chunk), vector) in batch.items.iter().zip(&vectors) {
        match index.insert(*message, *fp, *chunk, vector) {
            Ok(()) => stored = stored.saturating_add(1),
            Err(e) => {
                if first_error.is_none() {
                    first_error = Some(e);
                }
            }
        }
    }
    if index.wants_compacting() {
        index.compact();
    }
    index.flush();
    if stored == 0
        && let Some(e) = first_error
    {
        return Err(bad(e.to_string()));
    }
    Ok(json!({"stored": stored, "vectors": index.len(), "dims": index.dims()}))
}

/// Not in the browser: it keeps no vectors.
#[cfg(target_arch = "wasm32")]
pub fn embed_request(_s: &mut Session, _v: &Value) -> CmdResult {
    Err(crate::CmdError::Disabled("vectors are kept only on the desktop".into()))
}

/// Not in the browser: it keeps no vectors.
#[cfg(target_arch = "wasm32")]
pub fn embed_apply(_s: &mut Session, _v: &Value) -> CmdResult {
    Err(crate::CmdError::Disabled("vectors are kept only on the desktop".into()))
}
