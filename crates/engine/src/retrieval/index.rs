// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The word index kept beside the mailbox file ([`crate::words::Index`]) as a [`LexicalIndex`].
//! Not in the browser, which keeps no word index.

use mailcraft_model::Id;

use super::{LexicalIndex, Term, TermHits};
use crate::words::Index;

/// Most different words of the index one word of a query finds (a short prefix finds many).
pub const MAX_EXPANSIONS: usize = 512;

impl LexicalIndex for Index {
    fn docs(&self) -> usize {
        self.len()
    }

    fn lookup(&self, terms: &[Term]) -> TermHits {
        let per_term = terms
            .iter()
            .map(|t| {
                let mut list: Vec<(Id, u8)> = Vec::new();
                self.term_parts(&t.word, t.prefix, MAX_EXPANSIONS, |id, mask| list.push((id, mask)));
                list
            })
            .collect();
        TermHits { docs: self.len(), per_term }
    }
}
