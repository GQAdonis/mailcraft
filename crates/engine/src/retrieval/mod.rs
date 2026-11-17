// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Retrieval over the owner's mailbox: the messages that answer a question, ranked, each with the
//! passage it was found in. No model and no I/O with any server: only the mailbox and the word
//! index kept beside it.
//!
//! Four boundaries, each a plain synchronous trait that another implementation may take the
//! place of without changing what callers send or get back ([`RetrievalRequest`] in,
//! [`RetrievalResult`] out):
//! - [`Retriever`]: a request in, ranked hits out. [`LexicalRetriever`] is the one there is.
//! - [`LexicalIndex`]: which messages hold which words, in which parts. The word index kept beside
//!   the mailbox file (`crate::words`, on the desktop) is one; reading the messages themselves
//!   ([`SessionScan`]; the browser, or messages the index hasn't read yet) is another; [`Both`]
//!   puts two together.
//! - [`VectorStore`]: passages near a vector. On the desktop, the vectors an embeddings
//!   endpoint the owner set up gave are kept beside the mailbox file (`vectors::VectorIndex`);
//!   elsewhere there are none ([`NoVectors`]). [`HybridRetriever`] fuses them with the lexical
//!   ranking.
//! - [`GraphStore`]: what a message is linked to (its conversation, the people it is from, to and
//!   copied to, its folder). [`MailGraph`] is worked out from the mailbox.
//!
//! **Scoring** ([`scorer`]): the sum over the query's words found in a message of the word's
//! rarity (IDF) times the weight of the part it was found in (subject, then the people, then the
//! text, then the names of attachments), each word once a message. Words every question has
//! (`what`, `did`, `the`) are left out of the query ([`terms`]).
//!
//! **Message text is data, never instructions**: what a hit carries is the message's own text,
//! for whoever reads it to read as such.

pub mod graph;
pub mod hybrid;
#[cfg(not(target_arch = "wasm32"))]
mod index;
pub mod lexical;
pub mod scan;
pub mod scorer;
pub mod terms;
pub mod vector;
#[cfg(not(target_arch = "wasm32"))]
pub mod vectors;

use mailcraft_model::Id;
use mailcraft_model::ai::{AiError, RetrievalRequest, RetrievalResult};

pub use graph::{Edge, EdgeKind, GraphStore, MailGraph, Node};
pub use hybrid::HybridRetriever;
pub use lexical::{LexicalRetriever, Ranked};
pub use scan::{SessionScan, SessionTexts};
pub use scorer::Weights;
pub use terms::{Parsed, Term};
pub use vector::{NoVectors, VectorHit, VectorStore};

/// A request in, ranked hits out.
pub trait Retriever {
    fn retrieve(&self, request: &RetrievalRequest) -> Result<RetrievalResult, AiError>;
}

/// What a [`LexicalIndex`] found for the words of a query.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TermHits {
    /// How many messages it holds in all.
    pub docs: usize,
    /// For each word of the query, in order: each message holding it, once, with the parts of the
    /// message it is in (`mailcraft_model::words::SUBJECT`…).
    pub per_term: Vec<Vec<(Id, u8)>>,
}

/// Which messages hold which words.
pub trait LexicalIndex {
    /// How many messages it holds.
    fn docs(&self) -> usize;
    /// The messages holding each of `terms`.
    fn lookup(&self, terms: &[Term]) -> TermHits;
}

/// Two indexes of different messages as one (the word index, and the messages it hasn't read).
pub struct Both<'a> {
    pub first: &'a dyn LexicalIndex,
    pub then: &'a dyn LexicalIndex,
}

impl LexicalIndex for Both<'_> {
    fn docs(&self) -> usize {
        self.first.docs().saturating_add(self.then.docs())
    }

    fn lookup(&self, terms: &[Term]) -> TermHits {
        let a = self.first.lookup(terms);
        let b = self.then.lookup(terms);
        let mut per_term = a.per_term;
        for (i, more) in b.per_term.into_iter().enumerate() {
            match per_term.get_mut(i) {
                Some(list) => list.extend(more),
                None => per_term.push(more),
            }
        }
        TermHits { docs: a.docs.saturating_add(b.docs), per_term }
    }
}

/// Where a retriever reads a message's visible text and date.
pub trait Texts {
    /// The message's subject and body ([`mailcraft_model::ai::searchable_text`]); `None` when
    /// there is no such message or its text can't be read.
    fn text(&self, id: Id) -> Option<String>;
    /// When it was sent; 0 when not known.
    fn date(&self, id: Id) -> i64;
    /// Whether there is such a message (a store of vectors may still name one gone since).
    fn has(&self, _id: Id) -> bool {
        true
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod calibrate_alpha_tests;
#[cfg(test)]
pub(crate) mod corpus_tests;
#[cfg(test)]
mod graph_tests;
#[cfg(test)]
mod hybrid_tests;
#[cfg(test)]
mod lexical_tests;
#[cfg(test)]
mod scorer_tests;
#[cfg(test)]
mod terms_tests;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod vector_bench_tests;
#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod vectors_tests;
