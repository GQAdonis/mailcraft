// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A message's visible text in passages ([`Chunk`]): what a retrieval hit shows and an answer
//! cites.
//!
//! The text is the subject, a blank line and the body's text ([`searchable_text`]); never the
//! words the sender hid in the HTML (`Message::hidden_text`), which are never shown or quoted.
//! It is cut at paragraph breaks into passages of at most [`MAX_CHUNK_CHARS`] characters (a
//! longer paragraph at a line break or a space, else at the limit), each known by its message
//! and its place in that text (byte offsets, always at character boundaries). At most
//! [`MAX_CHUNKS`] passages of a message, read from its first [`MAX_TEXT_BYTES`].

use serde::{Deserialize, Serialize};

use crate::Id;

/// Most characters of one passage.
pub const MAX_CHUNK_CHARS: usize = 1_200;
/// Most passages of one message.
pub const MAX_CHUNKS: usize = 32;
/// Most bytes of a message's text that are cut into passages.
pub const MAX_TEXT_BYTES: usize = 1 << 20;

/// A passage of a message's text ([`searchable_text`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Chunk {
    pub message: Id,
    /// Its place among the message's passages (the first is 0).
    pub ordinal: u16,
    /// Where it starts and ends in the message's text, in bytes.
    pub start: usize,
    pub end: usize,
    pub text: String,
}

/// `text` cut to at most `max` bytes, at a character boundary.
pub(crate) fn cap(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut at = max;
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    text.get(..at).unwrap_or("")
}

/// The text of a message that passages are cut from: its subject, a blank line, its body's
/// text; at most [`MAX_TEXT_BYTES`]. Never the words the sender hid.
pub fn searchable_text(subject: &str, body: &str) -> String {
    let subject = cap(subject.trim(), MAX_TEXT_BYTES);
    let room = MAX_TEXT_BYTES.saturating_sub(subject.len()).saturating_sub(2);
    let body = cap(body, room);
    let mut out = String::with_capacity(subject.len().saturating_add(body.len()).saturating_add(2));
    out.push_str(subject);
    out.push_str("\n\n");
    out.push_str(body);
    out
}

/// The passages of `text` (a message's [`searchable_text`]), in order; passages of nothing but
/// white space are left out.
pub fn chunks(message: Id, text: &str) -> Vec<Chunk> {
    let text = cap(text, MAX_TEXT_BYTES);
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut ordinal: u16 = 0;
    // At most one step a passage, and each step moves on by at least one character.
    while start < text.len() && out.len() < MAX_CHUNKS {
        let Some(rest) = text.get(start..) else { break };
        let len = chunk_len(rest);
        if len == 0 {
            break;
        }
        let end = start.saturating_add(len).min(text.len());
        if let Some(piece) = text.get(start..end)
            && !piece.trim().is_empty()
        {
            out.push(Chunk { message, ordinal, start, end, text: piece.to_string() });
            ordinal = ordinal.saturating_add(1);
        }
        start = end;
    }
    out
}

/// How many bytes of `rest` the next passage takes: up to [`MAX_CHUNK_CHARS`] characters,
/// ending after the last paragraph break in them, else the last line break, else the last
/// space (any of them past the first third, so that no passage is tiny), else at the limit.
/// More than 0 whenever `rest` isn't empty.
fn chunk_len(rest: &str) -> usize {
    let limit = rest.char_indices().nth(MAX_CHUNK_CHARS).map_or(rest.len(), |(i, _)| i);
    if limit >= rest.len() {
        return rest.len();
    }
    let Some(window) = rest.get(..limit) else { return limit };
    let least = limit / 3;
    for sep in ["\n\n", "\n", " "] {
        if let Some(at) = window.rfind(sep) {
            let end = at.saturating_add(sep.len());
            if end > least {
                return end;
            }
        }
    }
    limit
}
