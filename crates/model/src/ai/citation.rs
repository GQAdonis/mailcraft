// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What an answer names as its sources, and the check that it names only what it was given.
//!
//! A model may name any number (it was told about message 12; it says 13, or 18446744073709551615).
//! [`validate_citations`] refuses a citation of a message that wasn't among those given to the
//! model, a span that runs backwards or past any text, and more than [`MAX_CITATIONS`]; the
//! same citation twice is kept once.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use super::AiError;
use super::chunk::MAX_TEXT_BYTES;
use crate::Id;

/// Most citations one answer keeps.
pub const MAX_CITATIONS: usize = 16;

/// A passage an answer names: the message, and where in its text ([`super::searchable_text`]),
/// in bytes. `start == end == 0` names the message as a whole.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Citation {
    pub message: Id,
    pub start: usize,
    pub end: usize,
}

impl Citation {
    /// The message as a whole.
    pub fn message(message: Id) -> Citation {
        Citation { message, start: 0, end: 0 }
    }
}

/// `cited`, checked against `provided` (the messages whose text was given to the model), in
/// their order without repeats. Refused whole when one names a message that wasn't provided
/// ([`AiError::UnknownCitation`]), runs backwards or past [`MAX_TEXT_BYTES`], or when there are
/// more than [`MAX_CITATIONS`] different ones.
pub fn validate_citations(cited: &[Citation], provided: &[Id]) -> Result<Vec<Citation>, AiError> {
    let given: HashSet<Id> = provided.iter().copied().collect();
    let mut seen: HashSet<Citation> = HashSet::new();
    let mut out = Vec::new();
    for c in cited {
        if c.message == 0 || !given.contains(&c.message) {
            return Err(AiError::UnknownCitation(c.message));
        }
        if c.start > c.end || c.end > MAX_TEXT_BYTES {
            return Err(AiError::Invalid(format!("the citation of message {} names no passage of it", c.message)));
        }
        if seen.insert(*c) {
            if out.len() >= MAX_CITATIONS {
                return Err(AiError::TooMany { what: "citations", max: MAX_CITATIONS });
            }
            out.push(*c);
        }
    }
    Ok(out)
}
