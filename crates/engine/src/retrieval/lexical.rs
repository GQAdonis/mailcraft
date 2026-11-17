// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The retriever there is: the query's words looked up in a [`LexicalIndex`], scored
//! ([`super::scorer`]), narrowed to a person, a conversation or a folder through a
//! [`GraphStore`], ranked, and each hit given the passage of its text that holds the most of the
//! query's words.
//!
//! - **Ranked** by score, then the newer message, then the larger id; at most the request's limit
//!   (10 unless the caller asks for another).
//! - **A relative cutoff**: only hits scoring at least [`ALPHA`] times the best are kept, and
//!   always the best one (a message that holds only the commonest of the query's words is left
//!   out when others hold them all). Fewer distractors reach a model or an agent.
//! - **Nothing found is no error**: a query whose words no message holds (or that has none) has
//!   no hits; so has a person, a conversation or a folder that isn't there.

use std::collections::HashSet;

use mailcraft_model::Id;
use mailcraft_model::ai::{AiError, Chunk, RetrievalRequest, RetrievalResult, ScoredHit, chunks};
use mailcraft_model::words;

use super::graph::{EdgeKind, GraphStore, Node};
use super::scorer::{self, Weights};
use super::terms::{self, Term};
use super::{LexicalIndex, Retriever, Texts};

/// The relative cutoff: a hit scoring under this share of the best is left out.
///
/// Frozen at 0.5 by the calibration run (the `#[ignore]` test `retrieval::calibrate_alpha`, on a
/// synthetic corpus of its own seed, never the graded one). Every hit the questions needed was
/// kept from 0.30 to 0.90, and precision levels off at 0.50–0.55 (0.66 of the hits relevant).
/// The lower end of that plateau keeps the most hits that could matter.
pub const ALPHA: f64 = 0.5;

/// Lexical retrieval over an index, a graph and the messages' text.
pub struct LexicalRetriever<'a> {
    pub index: &'a dyn LexicalIndex,
    pub graph: &'a dyn GraphStore,
    pub texts: &'a dyn Texts,
    pub weights: Weights,
    /// The relative cutoff, [`ALPHA`] unless a calibration sweeps it. Taken within 0 to 1; one
    /// that isn't a number is [`ALPHA`].
    pub alpha: f64,
}

impl<'a> LexicalRetriever<'a> {
    pub fn new(index: &'a dyn LexicalIndex, graph: &'a dyn GraphStore, texts: &'a dyn Texts) -> LexicalRetriever<'a> {
        LexicalRetriever { index, graph, texts, weights: Weights::default(), alpha: ALPHA }
    }

    /// The messages a request is narrowed to (`None`: all of them).
    fn allowed(&self, from: Option<&str>, thread: Option<Id>, folder: Option<Id>) -> Option<HashSet<Id>> {
        let mut sets: Vec<HashSet<Id>> = Vec::new();
        if let Some(who) = from {
            let mut set = HashSet::new();
            for person in self.graph.people_named(who) {
                set.extend(self.graph.messages_of(EdgeKind::From, &Node::Person(person)));
            }
            sets.push(set);
        }
        if let Some(message) = thread {
            let key = self.graph.edges_of(message).into_iter().find(|e| e.kind == EdgeKind::Thread).map(|e| e.to);
            let set: HashSet<Id> = key.map(|k| self.graph.messages_of(EdgeKind::Thread, &k)).unwrap_or_default().into_iter().collect();
            sets.push(set);
        }
        if let Some(folder) = folder {
            sets.push(self.graph.messages_of(EdgeKind::Folder, &Node::Folder(folder)).into_iter().collect());
        }
        let mut sets = sets.into_iter();
        let first = sets.next()?;
        Some(sets.fold(first, |acc, s| acc.intersection(&s).copied().collect()))
    }

    /// The passage of message `id` holding the most of `terms` (the most different ones, then the
    /// most in all, then the first); a passage naming the message alone when its text can't be read.
    pub fn best_chunk(&self, id: Id, terms: &[Term]) -> Chunk {
        let Some(text) = self.texts.text(id) else { return Chunk { message: id, ..Default::default() } };
        let mut best: Option<(usize, usize, Chunk)> = None;
        for c in chunks(id, &text) {
            let mut found = vec![false; terms.len()];
            let mut all = 0usize;
            words::each(&c.text, |w| {
                for (t, f) in terms.iter().zip(found.iter_mut()) {
                    if t.finds(w) {
                        *f = true;
                        all = all.saturating_add(1);
                    }
                }
                false
            });
            let distinct = found.iter().filter(|f| **f).count();
            let better = best.as_ref().is_none_or(|(d, a, _)| (distinct, all) > (*d, *a));
            if better {
                best = Some((distinct, all, c));
            }
        }
        best.map(|(_, _, c)| c).unwrap_or_else(|| Chunk { message: id, ..Default::default() })
    }
}

/// What a lexical ranking found, before it is cut to a request's limit.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ranked {
    /// The query's words looked for.
    pub terms: Vec<Term>,
    /// The messages that hold at least one of them and are allowed, best first, with their scores.
    pub list: Vec<(Id, f64)>,
    /// The messages the request is narrowed to (`None`: all of them).
    pub allowed: Option<HashSet<Id>>,
}

impl LexicalRetriever<'_> {
    /// Every message `request` finds, ranked (by score, then the newer, then the larger id),
    /// without the limit and without leaving out what scores far below the best.
    pub fn rank(&self, request: &RetrievalRequest) -> Ranked {
        let parsed = terms::parse(&request.query);
        let from = request.from.as_deref().or(parsed.from.as_deref());
        let allowed = self.allowed(from, request.thread, request.folder);
        if parsed.terms.is_empty() || allowed.as_ref().is_some_and(HashSet::is_empty) {
            return Ranked { terms: parsed.terms, list: Vec::new(), allowed };
        }
        let hits = self.index.lookup(&parsed.terms);
        let scores = scorer::score(&hits, &self.weights);
        let mut ranked: Vec<(Id, f64, i64)> = scores
            .into_iter()
            .filter(|(id, score)| score.is_finite() && *score > 0.0 && allowed.as_ref().is_none_or(|a| a.contains(id)))
            .map(|(id, score)| (id, score, self.texts.date(id)))
            .collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(b.2.cmp(&a.2)).then(b.0.cmp(&a.0)));
        Ranked { terms: parsed.terms, list: ranked.into_iter().map(|(id, score, _)| (id, score)).collect(), allowed }
    }
}

impl Retriever for LexicalRetriever<'_> {
    fn retrieve(&self, request: &RetrievalRequest) -> Result<RetrievalResult, AiError> {
        let ranked = self.rank(request);
        let term_words: Vec<String> = ranked.terms.iter().map(|t| t.word.clone()).collect();
        let top = ranked.list.first().map_or(0.0, |r| r.1);
        let limit = request.limit.clamp(1, mailcraft_model::ai::MAX_LIMIT);
        let alpha = if self.alpha.is_finite() { self.alpha.clamp(0.0, 1.0) } else { ALPHA };
        let floor = top * alpha;
        let hits = ranked
            .list
            .iter()
            .enumerate()
            .take_while(|(at, (_, score))| *at == 0 || *score >= floor)
            .map(|(_, hit)| hit)
            .take(limit)
            .map(|(id, score)| ScoredHit { message: *id, score: *score as f32, chunk: self.best_chunk(*id, &ranked.terms) })
            .collect();
        Ok(RetrievalResult { hits, terms: term_words })
    }
}
