// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Lexical and semantic retrieval together: the lexical ranking ([`LexicalRetriever::rank`]) and
//! the passages nearest the question's vector ([`VectorStore::nearest`]) fused by reciprocal rank
//! ([`mailcraft_model::ai::fuse`]).
//!
//! The question's vector comes from the host (the embeddings endpoint the owner set up, given the
//! question alone); without one, or without vectors kept, retrieval is lexical alone and the
//! caller says so. The same filters (a person, a conversation, a folder) hold for both lists, and
//! a message gone since its vectors were kept is left out.

use std::collections::HashMap;

use mailcraft_model::Id;
use mailcraft_model::ai::{AiError, Chunk, MAX_LIMIT, RRF_K, RetrievalRequest, RetrievalResult, ScoredHit, chunks, fuse};

use super::lexical::LexicalRetriever;
use super::vector::VectorStore;
use super::{Retriever, Texts};

/// How many of each list are fused.
pub const FUSE_DEPTH: usize = 50;
/// How many passages are asked of the vectors (several may be one message's).
pub const VECTOR_DEPTH: usize = 200;

/// Lexical and semantic retrieval fused.
pub struct HybridRetriever<'a> {
    pub lexical: LexicalRetriever<'a>,
    pub vectors: &'a dyn VectorStore,
    pub texts: &'a dyn Texts,
    /// The question's vector, from the same embedder as the vectors kept.
    pub query: &'a [f32],
}

impl Retriever for HybridRetriever<'_> {
    fn retrieve(&self, request: &RetrievalRequest) -> Result<RetrievalResult, AiError> {
        let ranked = self.lexical.rank(request);
        let terms: Vec<String> = ranked.terms.iter().map(|t| t.word.clone()).collect();
        if ranked.allowed.as_ref().is_some_and(std::collections::HashSet::is_empty) {
            return Ok(RetrievalResult { hits: Vec::new(), terms });
        }
        let lexical: Vec<Id> = ranked.list.iter().take(FUSE_DEPTH).map(|(id, _)| *id).collect();
        let mut semantic: Vec<Id> = Vec::new();
        let mut passage: HashMap<Id, u16> = HashMap::new();
        for hit in self.vectors.nearest(self.query, VECTOR_DEPTH)? {
            if semantic.len() >= FUSE_DEPTH {
                break;
            }
            let allowed = ranked.allowed.as_ref().is_none_or(|a| a.contains(&hit.message));
            if !allowed || passage.contains_key(&hit.message) || !self.texts.has(hit.message) {
                continue;
            }
            passage.insert(hit.message, hit.chunk);
            semantic.push(hit.message);
        }
        let limit = request.limit.clamp(1, MAX_LIMIT);
        let hits = fuse(&[lexical.clone(), semantic], RRF_K)
            .into_iter()
            .take(limit)
            .map(|(id, score)| {
                let chunk = match passage.get(&id) {
                    Some(ordinal) if !lexical.contains(&id) => self.chunk_at(id, *ordinal),
                    _ => self.lexical.best_chunk(id, &ranked.terms),
                };
                ScoredHit { message: id, score: score as f32, chunk }
            })
            .collect();
        Ok(RetrievalResult { hits, terms })
    }
}

impl HybridRetriever<'_> {
    /// Passage `ordinal` of message `id`; a passage naming the message alone when it isn't there.
    fn chunk_at(&self, id: Id, ordinal: u16) -> Chunk {
        self.texts
            .text(id)
            .and_then(|text| chunks(id, &text).into_iter().find(|c| c.ordinal == ordinal))
            .unwrap_or_else(|| Chunk { message: id, ..Default::default() })
    }
}
