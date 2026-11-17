// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A retrieval: the request (a question, how many hits, who it is from, which conversation or
//! folder), the hits ranked by relevance, each with its passage, and reciprocal rank fusion of
//! several rankings ([`fuse`]).
//!
//! Every field of a request comes from outside (an agent, a script, the window): it is checked
//! when the request is made ([`RetrievalRequest::new`]), and the limit is capped.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::AiError;
use super::chunk::Chunk;
use crate::Id;

/// Hits a request gets when it doesn't say.
pub const DEFAULT_LIMIT: usize = 10;
/// Most hits one request gets.
pub const MAX_LIMIT: usize = 50;
/// Longest question, in characters.
pub const MAX_QUERY_CHARS: usize = 4_000;
/// Longest person named by `from`, in characters.
const MAX_PERSON_CHARS: usize = 320;
/// Reciprocal rank fusion's constant (Cormack, Clarke and Büttcher, 2009).
pub const RRF_K: u32 = 60;
/// Most rankings fused, and most entries of each that count.
const MAX_LISTS: usize = 16;
const MAX_LIST_LEN: usize = 10_000;

/// What a retrieval looks for.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RetrievalRequest {
    /// The question or the words, as typed.
    pub query: String,
    /// Most hits wanted (1 to [`MAX_LIMIT`]).
    pub limit: usize,
    /// Only messages from this person: an address, or words of a name.
    pub from: Option<String>,
    /// Only messages of the conversation this message is part of.
    pub thread: Option<Id>,
    /// Only messages in this folder.
    pub folder: Option<Id>,
}

impl RetrievalRequest {
    /// A request, checked: a query of at most [`MAX_QUERY_CHARS`] characters, a limit of 1 to
    /// [`MAX_LIMIT`] (`None`: [`DEFAULT_LIMIT`]; 0 is refused, more is capped), a person of at
    /// most 320 characters with something in it, ids above 0.
    pub fn new(query: &str, limit: Option<u64>, from: Option<&str>, thread: Option<Id>, folder: Option<Id>) -> Result<Self, AiError> {
        if query.chars().nth(MAX_QUERY_CHARS).is_some() {
            return Err(AiError::Invalid(format!("the query is longer than {MAX_QUERY_CHARS} characters")));
        }
        let limit = match limit {
            None => DEFAULT_LIMIT,
            Some(0) => return Err(AiError::Invalid("the limit is at least 1".into())),
            Some(n) => usize::try_from(n).unwrap_or(MAX_LIMIT).min(MAX_LIMIT),
        };
        let from = match from.map(str::trim) {
            None => None,
            Some("") => return Err(AiError::Invalid("`from` names nobody".into())),
            Some(p) if p.chars().nth(MAX_PERSON_CHARS).is_some() => {
                return Err(AiError::Invalid(format!("`from` is longer than {MAX_PERSON_CHARS} characters")));
            }
            Some(p) => Some(p.to_string()),
        };
        if thread == Some(0) || folder == Some(0) {
            return Err(AiError::Invalid("an id is a whole number above 0".into()));
        }
        Ok(RetrievalRequest { query: query.to_string(), limit, from, thread, folder })
    }
}

/// A message found, with how well it matched and the passage that matched best.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ScoredHit {
    pub message: Id,
    /// Higher is better; only comparable within one result.
    pub score: f32,
    pub chunk: Chunk,
}

/// The hits of a retrieval, best first, at most its limit.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RetrievalResult {
    pub hits: Vec<ScoredHit>,
    /// The words of the query that were looked for (after the words every question has were
    /// left out), folded.
    pub terms: Vec<String>,
}

/// Reciprocal rank fusion of `lists` (each a ranking, best first): each message scores the sum
/// over the lists of `1 / (k + rank)` (the first rank is 1), counting only its first place in a
/// list. Best first; equal scores by id. At most 16 lists, the first 10,000 of each, count.
pub fn fuse(lists: &[Vec<Id>], k: u32) -> Vec<(Id, f64)> {
    let mut scores: HashMap<Id, f64> = HashMap::new();
    for list in lists.iter().take(MAX_LISTS) {
        let mut seen: std::collections::HashSet<Id> = std::collections::HashSet::new();
        let mut rank: u32 = 0;
        for id in list.iter().take(MAX_LIST_LEN) {
            if !seen.insert(*id) {
                continue;
            }
            rank = rank.saturating_add(1);
            let add = 1.0 / (f64::from(k) + f64::from(rank));
            *scores.entry(*id).or_insert(0.0) += add;
        }
    }
    let mut out: Vec<(Id, f64)> = scores.into_iter().collect();
    out.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    out
}
