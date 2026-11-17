// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `ai.retrieve`: the messages that answer a question ([`crate::retrieval`]), read from the
//! mailbox as it is; no model is asked and nothing of the mailbox changes.
//!
//! Every parameter comes from outside (an agent, a script): `query` is required and at most
//! 4,000 characters, `limit` is a whole number of 1 or more (capped at 50), `from` a string,
//! `thread` and `folder` ids; anything else is refused. A query no message answers has no hits,
//! and is no error.
//!
//! **Semantic matching** is optional: with the question's vector (`queryVector`, from the
//! embeddings endpoint the owner set up, and the name of its model, `embedder`) and vectors kept
//! of the mailbox by that model ([`crate::retrieval::vectors`], the desktop), the lexical and the
//! semantic rankings are fused ([`crate::retrieval::HybridRetriever`]) and the answer says
//! `"semantic": true`. Otherwise retrieval is lexical alone, exactly, and the answer says
//! `"semantic": false` with a notice.

#[cfg(not(target_arch = "wasm32"))]
use mailcraft_model::Id;
use mailcraft_model::ai::{RetrievalRequest, RetrievalResult};
use serde_json::{Value, json};

use super::UNTRUSTED;
use crate::cmd::bad;
#[cfg(not(target_arch = "wasm32"))]
use crate::retrieval::{Both, HybridRetriever, Texts, VectorStore};
use crate::retrieval::{LexicalRetriever, MailGraph, Retriever, SessionScan, SessionTexts};
use crate::{CmdError, CmdResult, Session, p};

/// The parameters it takes.
const PARAMS: &[&str] = &["query", "limit", "from", "thread", "folder", "queryVector", "embedder"];
/// Most numbers of a question's vector.
const MAX_QUERY_DIMS: usize = 4_096;
/// What the answer says when matching is by words alone.
pub const LEXICAL_ONLY: &str = "Semantic matching is off: no embeddings endpoint is set up, or the mailbox isn't embedded by that model yet. Messages were matched by their words.";

/// The question's vector, and the model that made it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Semantic {
    pub embedder: String,
    pub vector: Vec<f32>,
}

/// `ai.retrieve`.
pub fn retrieve(s: &mut Session, v: &Value) -> CmdResult {
    let request = request_of(v)?;
    let semantic = semantic_of(v)?;
    let (result, used) = retrieve_now(s, &request, semantic.as_ref())?;
    Ok(answer(s, &result, used))
}

/// `request` retrieved from the mailbox as it is now: the word index (on the desktop) brought in
/// step first ([`run`]), and the vectors opened when a question's vector came. Whether the
/// semantic ranking was used.
pub fn retrieve_now(s: &mut Session, request: &RetrievalRequest, semantic: Option<&Semantic>) -> Result<(RetrievalResult, bool), CmdError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        s.words_for_retrieval();
        if semantic.is_some() {
            let _ = s.vectors_mut();
        }
    }
    run(s, request, semantic)
}

/// The request `v` names, checked.
pub fn request_of(v: &Value) -> Result<RetrievalRequest, CmdError> {
    if !v.is_object() {
        return Err(bad("the parameters are an object with `query`"));
    }
    p::only(v, PARAMS)?;
    let query = p::req_str(v, "query")?;
    let limit = match v.get("limit") {
        None => None,
        Some(_) => Some(p::u64(v, "limit").ok_or_else(|| bad("`limit` is a whole number from 1 to 50"))?),
    };
    let from = match v.get("from") {
        None => None,
        Some(Value::String(who)) => Some(who.as_str()),
        Some(_) => return Err(bad("`from` is an address or words of a name")),
    };
    let thread = p::id_strict(v, "thread")?;
    let folder = p::id_strict(v, "folder")?;
    RetrievalRequest::new(query, limit, from, thread, folder).map_err(|e| bad(e.to_string()))
}

/// The question's vector `v` names (`queryVector` and `embedder`, both or neither), checked.
pub fn semantic_of(v: &Value) -> Result<Option<Semantic>, CmdError> {
    let wrong = || bad("`queryVector` is a list of 1 to 4096 finite numbers, given with `embedder`, the name of the model that made it");
    match (v.get("queryVector"), v.get("embedder")) {
        (None, None) => Ok(None),
        (Some(Value::Array(numbers)), Some(Value::String(embedder))) => {
            let embedder = embedder.trim();
            if numbers.is_empty() || numbers.len() > MAX_QUERY_DIMS || embedder.is_empty() || embedder.len() > 200 {
                return Err(wrong());
            }
            let vector =
                numbers.iter().map(|n| n.as_f64().map(|f| f as f32).filter(|f| f.is_finite()).ok_or_else(wrong)).collect::<Result<Vec<f32>, _>>()?;
            Ok(Some(Semantic { embedder: embedder.to_string(), vector }))
        }
        _ => Err(wrong()),
    }
}

/// `request` retrieved from the session's mailbox: from the word index kept beside the mailbox
/// file where there is one (the desktop), with the messages it hasn't read yet read as they are;
/// else every message read as it is (the browser), which finds the same. Fused with the nearest
/// passages when `semantic` came and vectors of its model are kept; whether they were.
pub fn run(s: &Session, request: &RetrievalRequest, semantic: Option<&Semantic>) -> Result<(RetrievalResult, bool), CmdError> {
    let narrowed = request.from.is_some() || request.thread.is_some() || request.folder.is_some() || request.query.to_lowercase().contains("from:");
    let graph = if narrowed {
        let threads = s.threads();
        MailGraph::build(s.mailbox.messages.values().map(|m| m.as_ref()), &threads)
    } else {
        MailGraph::default()
    };
    let texts = SessionTexts { session: s };
    run_on(s, request, semantic, &graph, &texts)
}

/// [`run`] on the desktop: the word index, and the vectors.
#[cfg(not(target_arch = "wasm32"))]
fn run_on(
    s: &Session,
    request: &RetrievalRequest,
    semantic: Option<&Semantic>,
    graph: &MailGraph,
    texts: &SessionTexts<'_>,
) -> Result<(RetrievalResult, bool), CmdError> {
    let vectors: Option<(&dyn VectorStore, &[f32])> = semantic.and_then(|q| {
        s.vectors()
            .filter(|v| v.embedder() == q.embedder && v.dims() == q.vector.len() && !v.is_empty())
            .map(|v| (v as &dyn VectorStore, q.vector.as_slice()))
    });
    if let Some(words) = s.words() {
        let guard = words.lock();
        if let Some(index) = guard.as_ref() {
            let unread: Vec<Id> = s.mailbox.messages.keys().copied().filter(|id| index.slot_of(*id).is_none()).collect();
            let rest = SessionScan::new(s, unread);
            let both = Both { first: index, then: &rest };
            return finish(LexicalRetriever::new(&both, graph, texts), vectors, texts, request);
        }
    }
    let scan = SessionScan::all(s);
    finish(LexicalRetriever::new(&scan, graph, texts), vectors, texts, request)
}

/// [`run`] in the browser: every message read as it is, and no vectors.
#[cfg(target_arch = "wasm32")]
fn run_on(
    s: &Session,
    request: &RetrievalRequest,
    _semantic: Option<&Semantic>,
    graph: &MailGraph,
    texts: &SessionTexts<'_>,
) -> Result<(RetrievalResult, bool), CmdError> {
    let scan = SessionScan::all(s);
    let result = LexicalRetriever::new(&scan, graph, texts).retrieve(request).map_err(|e| bad(e.to_string()))?;
    Ok((result, false))
}

/// `lexical` alone, or fused with `vectors` (the store and the question's vector).
#[cfg(not(target_arch = "wasm32"))]
fn finish(
    lexical: LexicalRetriever<'_>,
    vectors: Option<(&dyn VectorStore, &[f32])>,
    texts: &dyn Texts,
    request: &RetrievalRequest,
) -> Result<(RetrievalResult, bool), CmdError> {
    match vectors {
        Some((store, query)) => {
            let hybrid = HybridRetriever { lexical, vectors: store, texts, query };
            Ok((hybrid.retrieve(request).map_err(|e| bad(e.to_string()))?, true))
        }
        None => Ok((lexical.retrieve(request).map_err(|e| bad(e.to_string()))?, false)),
    }
}

/// What the command answers for `result`; `semantic`: the semantic ranking was used.
fn answer(s: &Session, result: &RetrievalResult, semantic: bool) -> Value {
    let hits: Vec<Value> = result
        .hits
        .iter()
        .map(|h| {
            let m = s.mailbox.message(h.message);
            json!({
                "id": h.message,
                "score": h.score,
                "subject": m.map(|m| m.subject.as_str()).unwrap_or_default(),
                "from": m.map(|m| m.from.email.as_str()).unwrap_or_default(),
                "fromName": m.map(|m| m.from.name.as_str()).unwrap_or_default(),
                "date": m.map_or(0, |m| m.date),
                "folder": m.map_or(0, |m| m.folder),
                "chunk": {"ordinal": h.chunk.ordinal, "start": h.chunk.start, "end": h.chunk.end, "text": h.chunk.text},
            })
        })
        .collect();
    let mut out = json!({"hits": hits, "terms": result.terms, "semantic": semantic, "untrusted": UNTRUSTED});
    if !semantic && let Some(o) = out.as_object_mut() {
        o.insert("notice".into(), Value::from(LEXICAL_ONLY));
    }
    out
}
