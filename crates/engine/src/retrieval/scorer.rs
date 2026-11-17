// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! How well a message matches a query: the sum over the query's words it holds of the word's
//! rarity (IDF) times the weight of the part of the message it is in (the best part, when it is
//! in several), each word once a message.
//!
//! IDF is BM25's, `ln(1 + (N - df + 0.5) / (df + 0.5))`, which stays above 0. Term frequency and
//! the length of a message aren't kept by the word index, so this is not BM25 itself.

use std::collections::HashMap;

use mailcraft_model::Id;
use mailcraft_model::words::{ATTACHMENT, BODY, CC, FROM, SUBJECT, TO};

use super::TermHits;

/// The weight of each part of a message: the subject above the people, the people above the
/// text, the text above the names of attachments.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Weights {
    pub subject: f64,
    pub people: f64,
    pub body: f64,
    pub attachment: f64,
}

impl Default for Weights {
    fn default() -> Self {
        Weights { subject: 3.0, people: 2.0, body: 1.0, attachment: 0.8 }
    }
}

impl Weights {
    /// The weight of the best of the parts `mask` names; 0 for none.
    pub fn of(&self, mask: u8) -> f64 {
        let mut w: f64 = 0.0;
        if mask & SUBJECT != 0 {
            w = w.max(self.subject);
        }
        if mask & (FROM | TO | CC) != 0 {
            w = w.max(self.people);
        }
        if mask & BODY != 0 {
            w = w.max(self.body);
        }
        if mask & ATTACHMENT != 0 {
            w = w.max(self.attachment);
        }
        if w.is_finite() { w.max(0.0) } else { 0.0 }
    }
}

/// The rarity of a word held by `df` of `docs` messages.
pub fn idf(docs: usize, df: usize) -> f64 {
    let n = docs.max(df) as f64;
    let df = df as f64;
    (1.0 + (n - df + 0.5) / (df + 0.5)).ln()
}

/// The score of each message holding at least one of the words of `hits`.
pub fn score(hits: &TermHits, weights: &Weights) -> HashMap<Id, f64> {
    let mut scores: HashMap<Id, f64> = HashMap::new();
    for list in &hits.per_term {
        // Each message once for this word, with every part it is in.
        let mut parts: HashMap<Id, u8> = HashMap::with_capacity(list.len());
        for (id, mask) in list {
            *parts.entry(*id).or_insert(0) |= *mask;
        }
        let rarity = idf(hits.docs, parts.len());
        if !rarity.is_finite() || rarity <= 0.0 {
            continue;
        }
        for (id, mask) in parts {
            let add = rarity * weights.of(mask);
            if add > 0.0 && add.is_finite() {
                *scores.entry(id).or_insert(0.0) += add;
            }
        }
    }
    scores
}
