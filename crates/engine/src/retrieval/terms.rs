// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A query as retrieval reads it: its words, folded as search folds them
//! ([`mailcraft_model::words`]), without the words every question has (`what`, `did`, `the`),
//! each once, with a light English stem so that `deposits` finds `deposit` and `booking` finds
//! `booked`; and a `from:` written in it.
//!
//! A word of three letters or more (once stemmed) finds the words that start with it; a shorter
//! one, or a number, only itself. At most [`MAX_TERMS`] words of a query count.

use mailcraft_model::words;

/// Most words of a query that count.
pub const MAX_TERMS: usize = 32;

/// A word looked for: the word itself, or the words that start with it (`prefix`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Term {
    pub word: String,
    pub prefix: bool,
}

impl Term {
    /// Whether `word` (folded) is a word this term finds.
    pub fn finds(&self, word: &str) -> bool {
        if self.prefix { word.starts_with(self.word.as_str()) } else { word == self.word }
    }
}

/// A query, read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    pub terms: Vec<Term>,
    /// What a `from:` in the query names (the first one).
    pub from: Option<String>,
}

/// Words that say nothing of what is looked for: those of a question and the commonest of English.
const STOP: &[&str] = &[
    "a",
    "about",
    "above",
    "after",
    "again",
    "all",
    "also",
    "am",
    "an",
    "and",
    "any",
    "are",
    "as",
    "ask",
    "asked",
    "asking",
    "at",
    "be",
    "been",
    "before",
    "being",
    "between",
    "both",
    "but",
    "by",
    "can",
    "could",
    "did",
    "do",
    "does",
    "doing",
    "done",
    "during",
    "each",
    "either",
    "else",
    "ever",
    "for",
    "from",
    "get",
    "got",
    "had",
    "has",
    "have",
    "having",
    "he",
    "her",
    "here",
    "hers",
    "him",
    "his",
    "how",
    "i",
    "if",
    "in",
    "into",
    "is",
    "it",
    "its",
    "just",
    "know",
    "let",
    "me",
    "mention",
    "mentioned",
    "might",
    "more",
    "most",
    "much",
    "must",
    "my",
    "no",
    "nor",
    "not",
    "now",
    "of",
    "off",
    "on",
    "once",
    "one",
    "only",
    "or",
    "other",
    "our",
    "ours",
    "out",
    "over",
    "please",
    "regarding",
    "said",
    "say",
    "says",
    "she",
    "should",
    "so",
    "some",
    "such",
    "tell",
    "than",
    "that",
    "the",
    "their",
    "theirs",
    "them",
    "then",
    "there",
    "these",
    "they",
    "this",
    "those",
    "through",
    "to",
    "told",
    "too",
    "under",
    "until",
    "up",
    "us",
    "very",
    "was",
    "we",
    "were",
    "what",
    "whats",
    "when",
    "where",
    "which",
    "while",
    "who",
    "whom",
    "whose",
    "why",
    "will",
    "with",
    "would",
    "you",
    "your",
    "yours",
];

/// Whether `word` (folded) is one every question has.
pub fn is_stop_word(word: &str) -> bool {
    STOP.contains(&word)
}

/// `query`, read: its words to look for and a `from:` written in it.
pub fn parse(query: &str) -> Parsed {
    let mut from = None;
    let mut rest = String::new();
    for token in query.split_whitespace() {
        let lower = token.to_lowercase();
        if let Some(who) = lower.strip_prefix("from:") {
            let who = who.trim_matches(|c: char| c == '"' || c == '\'' || c == ',' || c == '?' || c == ';');
            if from.is_none() && !who.is_empty() {
                from = Some(who.to_string());
            }
            continue;
        }
        rest.push_str(token);
        rest.push(' ');
    }
    let mut terms: Vec<Term> = Vec::new();
    words::each(&rest, |w| {
        if let Some(t) = term_of(w)
            && !terms.contains(&t)
        {
            terms.push(t);
        }
        terms.len() >= MAX_TERMS
    });
    Parsed { terms, from }
}

/// The term a word of the query is looked for as; `None` for a word that says nothing.
fn term_of(word: &str) -> Option<Term> {
    if is_stop_word(word) {
        return None;
    }
    let mut chars = word.chars();
    let (Some(first), second) = (chars.next(), chars.next()) else { return None };
    // A letter or digit alone (`a`, `5`) says nothing; a Chinese character alone does.
    if second.is_none() && first.is_ascii() {
        return None;
    }
    if word.chars().all(|c| c.is_ascii_digit()) {
        return Some(Term { word: word.to_string(), prefix: false });
    }
    let root = stem(word);
    if is_stop_word(root) {
        return None;
    }
    let prefix = root.chars().count() >= 3;
    if prefix {
        return Some(Term { word: root.to_string(), prefix });
    }
    Some(Term { word: word.to_string(), prefix: word.chars().count() >= 3 })
}

/// `word` without an English ending it may have (`ies`, `ing`, `ed`, `s`), for words of plain
/// letters long enough to have one.
fn stem(word: &str) -> &str {
    if !word.is_ascii() {
        return word;
    }
    let len = word.len();
    if len > 5
        && let Some(root) = word.strip_suffix("ies")
    {
        return root;
    }
    if len > 5
        && let Some(root) = word.strip_suffix("ing")
    {
        return root;
    }
    if len > 4
        && let Some(root) = word.strip_suffix("ed")
    {
        return root;
    }
    if len > 3
        && !word.ends_with("ss")
        && !word.ends_with("us")
        && !word.ends_with("is")
        && let Some(root) = word.strip_suffix('s')
    {
        return root;
    }
    word
}
