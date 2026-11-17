// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Passages near a vector (an embedding of a question). On the desktop the store is
//! `super::vectors::VectorIndex`, kept in a derived file beside the mailbox once the host has run
//! the embedding jobs; its hits are fused with the lexical ones by
//! [`super::hybrid::HybridRetriever`] ([`mailcraft_model::ai::fuse`]). Where none is kept (the
//! browser, a test), [`NoVectors`] finds nothing and retrieval is lexical alone.

use mailcraft_model::Id;
use mailcraft_model::ai::AiError;

/// A passage near the vector asked about.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VectorHit {
    pub message: Id,
    /// The passage's place among the message's ([`mailcraft_model::ai::Chunk::ordinal`]).
    pub chunk: u16,
    /// Higher is nearer.
    pub similarity: f32,
}

/// Passages by their vectors.
pub trait VectorStore {
    /// How many numbers a vector has; 0 when the store holds none.
    fn dims(&self) -> usize;
    /// The `k` passages nearest `query`, nearest first. Refused for a vector of another length.
    fn nearest(&self, query: &[f32], k: usize) -> Result<Vec<VectorHit>, AiError>;
}

/// No vectors: finds nothing.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoVectors;

impl VectorStore for NoVectors {
    fn dims(&self) -> usize {
        0
    }

    fn nearest(&self, _query: &[f32], _k: usize) -> Result<Vec<VectorHit>, AiError> {
        Ok(Vec::new())
    }
}
