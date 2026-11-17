// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Reading the messages themselves: the [`LexicalIndex`] where no word index is kept (the
//! browser) and for the messages the word index hasn't read yet; and where a retriever reads a
//! message's text ([`SessionTexts`]).
//!
//! Each message is read one at a time (its text brought from disk for the time it is looked at,
//! when it is kept there), and at most [`MAX_WORDS_A_MESSAGE`] of its words are looked at, as the
//! search box reads it ([`mailcraft_model::words::texts`]).

use mailcraft_model::ai::searchable_text;
use mailcraft_model::{Id, words};

use super::{LexicalIndex, Term, TermHits, Texts};
use crate::Session;

/// Most words of one message looked at.
pub const MAX_WORDS_A_MESSAGE: usize = 200_000;

/// The messages `ids` of a session, read as they are.
pub struct SessionScan<'a> {
    session: &'a Session,
    ids: Vec<Id>,
}

impl<'a> SessionScan<'a> {
    pub fn new(session: &'a Session, ids: Vec<Id>) -> SessionScan<'a> {
        SessionScan { session, ids }
    }

    /// Every message of the session.
    pub fn all(session: &'a Session) -> SessionScan<'a> {
        let ids = session.mailbox.messages.keys().copied().collect();
        SessionScan { session, ids }
    }
}

impl LexicalIndex for SessionScan<'_> {
    fn docs(&self) -> usize {
        self.ids.len()
    }

    fn lookup(&self, terms: &[Term]) -> TermHits {
        let mut per_term: Vec<Vec<(Id, u8)>> = vec![Vec::new(); terms.len()];
        if terms.is_empty() {
            return TermHits { docs: self.ids.len(), per_term };
        }
        let mut masks = vec![0u8; terms.len()];
        for id in &self.ids {
            let Ok(m) = self.session.loaded_message_where(*id, |_| false) else { continue };
            masks.fill(0);
            let mut seen = 0usize;
            words::texts(&m, &m.body.text, |field, text| {
                words::each(text, |w| {
                    for (t, mask) in terms.iter().zip(masks.iter_mut()) {
                        if t.finds(w) {
                            *mask |= field;
                        }
                    }
                    seen = seen.saturating_add(1);
                    seen >= MAX_WORDS_A_MESSAGE
                })
            });
            for (list, mask) in per_term.iter_mut().zip(&masks) {
                if *mask != 0 {
                    list.push((*id, *mask));
                }
            }
        }
        TermHits { docs: self.ids.len(), per_term }
    }
}

/// A session's messages, as a retriever reads their text and date.
pub struct SessionTexts<'a> {
    pub session: &'a Session,
}

impl Texts for SessionTexts<'_> {
    fn text(&self, id: Id) -> Option<String> {
        let m = self.session.loaded_message_where(id, |_| false).ok()?;
        Some(searchable_text(&m.subject, &m.body.text))
    }

    fn date(&self, id: Id) -> i64 {
        self.session.mailbox.message(id).map_or(0, |m| m.date)
    }

    fn has(&self, id: Id) -> bool {
        self.session.mailbox.messages.contains_key(&id)
    }
}
