// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The text of a structured answer, read as it arrives.
//!
//! A model asked for `{"answer": string, "citations": [...]}` and streamed writes the JSON a
//! piece at a time. [`AnswerExtractor`] takes the pieces in order and gives back, for each, the
//! decoded text of the top-level `"answer"` string that piece added, so that the answer can be
//! shown as it is written. The whole document is checked at the end by
//! [`super::read_answer_json`]; this only reads.
//!
//! - **Escapes** are decoded: `\"`, `\\`, `\/`, `\b`, `\f`, `\n`, `\r`, `\t` and `\uXXXX`, a
//!   surrogate pair as one character, a lone surrogate or a broken escape as U+FFFD. An escape
//!   cut between two pieces is finished by the next.
//! - **Only the top-level member** named `answer` (its name may be escaped too): one inside
//!   another value, or a value of another member that holds the word, is passed over. The first
//!   such string is the answer; after it, nothing more is read.
//! - **Hostile input:** any text at all is taken without a panic. Bounded: at most
//!   [`MAX_ANSWER_JSON_BYTES`] are read and [`MAX_RESPONSE_BYTES`] of answer given back, a
//!   member name is compared on its first 16 characters, and values are passed over at most 128
//!   levels deep; past a bound, nothing more is read. Input that isn't a JSON object gives no
//!   text. What is given back isn't cleaned: the caller cleans it as any model text (`clean_text`).

use super::answer::MAX_ANSWER_JSON_BYTES;
use super::call::MAX_RESPONSE_BYTES;

/// Characters of a member name kept to compare.
const MAX_KEY_CHARS: usize = 16;
/// Deepest nesting passed over.
const MAX_DEPTH: u32 = 128;
const REPLACEMENT: char = '\u{FFFD}';

/// Where in a string's escapes the reader is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Esc {
    #[default]
    None,
    /// After `\`.
    Backslash,
    /// After `\u` and `digits` hex digits.
    Unicode { digits: u8, value: u32 },
    /// After a high surrogate: a `\u` with the low one is wanted.
    AfterHigh(u32),
    /// After a high surrogate and `\`.
    AfterHighBackslash(u32),
    /// After a high surrogate, `\u` and `digits` hex digits of the low one.
    Low { high: u32, digits: u8, value: u32 },
}

/// What one character of a string gave: at most two characters, and whether the string ended.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Out {
    first: Option<char>,
    second: Option<char>,
    end: bool,
}

impl Out {
    fn one(c: char) -> Out {
        Out { first: Some(c), ..Out::default() }
    }

    /// U+FFFD, then what `rest` gave (which gives at most one character).
    fn replaced_then(rest: Out) -> Out {
        Out { first: Some(REPLACEMENT), second: rest.first, end: rest.end }
    }
}

/// The character of a UTF-16 unit that isn't a surrogate (else U+FFFD).
fn unit(value: u32) -> char {
    char::from_u32(value).unwrap_or(REPLACEMENT)
}

fn hex(c: char, value: u32) -> Option<u32> {
    c.to_digit(16).map(|d| value.saturating_mul(16).saturating_add(d))
}

/// One character `c` of a JSON string's text, with the escape state `esc`.
fn string_step(esc: &mut Esc, c: char) -> Out {
    match *esc {
        Esc::None => match c {
            '"' => Out { end: true, ..Out::default() },
            '\\' => {
                *esc = Esc::Backslash;
                Out::default()
            }
            c => Out::one(c),
        },
        Esc::Backslash => {
            *esc = Esc::None;
            match c {
                'b' => Out::one('\u{8}'),
                'f' => Out::one('\u{c}'),
                'n' => Out::one('\n'),
                'r' => Out::one('\r'),
                't' => Out::one('\t'),
                'u' => {
                    *esc = Esc::Unicode { digits: 0, value: 0 };
                    Out::default()
                }
                // `"`, `\`, `/`, and anything JSON doesn't name: kept as written.
                c => Out::one(c),
            }
        }
        Esc::Unicode { digits, value } => match hex(c, value) {
            Some(value) if digits < 3 => {
                *esc = Esc::Unicode { digits: digits.saturating_add(1), value };
                Out::default()
            }
            Some(value) => {
                *esc = Esc::None;
                match value {
                    0xD800..=0xDBFF => {
                        *esc = Esc::AfterHigh(value);
                        Out::default()
                    }
                    0xDC00..=0xDFFF => Out::one(REPLACEMENT),
                    v => Out::one(unit(v)),
                }
            }
            None => {
                *esc = Esc::None;
                Out::replaced_then(string_step(esc, c))
            }
        },
        Esc::AfterHigh(high) => {
            if c == '\\' {
                *esc = Esc::AfterHighBackslash(high);
                Out::default()
            } else {
                *esc = Esc::None;
                Out::replaced_then(string_step(esc, c))
            }
        }
        Esc::AfterHighBackslash(high) => {
            if c == 'u' {
                *esc = Esc::Low { high, digits: 0, value: 0 };
                Out::default()
            } else {
                // The high surrogate alone, then the escape it was followed by.
                *esc = Esc::Backslash;
                Out::replaced_then(string_step(esc, c))
            }
        }
        Esc::Low { high, digits, value } => match hex(c, value) {
            Some(value) if digits < 3 => {
                *esc = Esc::Low { high, digits: digits.saturating_add(1), value };
                Out::default()
            }
            Some(low) => {
                *esc = Esc::None;
                match low {
                    0xDC00..=0xDFFF => {
                        let code = 0x10000u32.saturating_add((high.saturating_sub(0xD800) << 10) | low.saturating_sub(0xDC00));
                        Out::one(char::from_u32(code).unwrap_or(REPLACEMENT))
                    }
                    0xD800..=0xDBFF => {
                        // Another high one: the first stood alone.
                        *esc = Esc::AfterHigh(low);
                        Out::one(REPLACEMENT)
                    }
                    v => Out { first: Some(REPLACEMENT), second: Some(unit(v)), end: false },
                }
            }
            None => {
                *esc = Esc::None;
                Out::replaced_then(string_step(esc, c))
            }
        },
    }
}

/// Where in the document the reader is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    /// Before the top-level `{`.
    #[default]
    Start,
    /// After `{` or `,`: a member name or `}`.
    BeforeKey,
    /// In a member name.
    Key,
    /// After a member name: `:`.
    AfterKey,
    /// After `:`: a value.
    BeforeValue,
    /// In the answer string.
    Answer,
    /// In another member's string value.
    SkipString,
    /// In another member's object or list value.
    SkipNested { depth: u32, in_string: bool, escaped: bool },
    /// In another member's number, `true`, `false` or `null`.
    SkipScalar,
    /// After a value: `,` or `}`.
    AfterValue,
    /// Nothing more is read: the answer ended, the object closed, the input wasn't an object, or
    /// a bound was reached.
    Finished,
}

/// Reads the top-level `"answer"` string of a JSON document given in pieces (see the module).
#[derive(Clone, Debug, Default)]
pub struct AnswerExtractor {
    state: State,
    esc: Esc,
    /// The member name being read (its first [`MAX_KEY_CHARS`] characters).
    key: String,
    key_chars: usize,
    /// The value being read is the answer's.
    is_answer: bool,
    /// Bytes of JSON read.
    read: usize,
    /// Bytes of answer given back.
    written: usize,
    complete: bool,
    cut: bool,
}

impl AnswerExtractor {
    pub fn new() -> AnswerExtractor {
        AnswerExtractor::default()
    }

    /// Take the next `piece` of the document: the answer's text it added, decoded.
    pub fn push(&mut self, piece: &str) -> String {
        let mut out = String::new();
        for c in piece.chars() {
            if self.state == State::Finished {
                break;
            }
            let read = self.read.saturating_add(c.len_utf8());
            if read > MAX_ANSWER_JSON_BYTES {
                self.cut = true;
                self.state = State::Finished;
                break;
            }
            self.read = read;
            self.step(c, &mut out);
        }
        out
    }

    /// The answer's string was read to its closing quote.
    pub fn complete(&self) -> bool {
        self.complete
    }

    /// A bound was reached: the answer given back may be its start only.
    pub fn cut(&self) -> bool {
        self.cut
    }

    /// Bytes of answer given back so far.
    pub fn written(&self) -> usize {
        self.written
    }

    fn emit(&mut self, c: Option<char>, out: &mut String) {
        let Some(c) = c else { return };
        let written = self.written.saturating_add(c.len_utf8());
        if written > MAX_RESPONSE_BYTES {
            self.cut = true;
            self.state = State::Finished;
            return;
        }
        self.written = written;
        out.push(c);
    }

    fn step(&mut self, c: char, out: &mut String) {
        let space = c.is_ascii_whitespace();
        match self.state {
            State::Finished => {}
            State::Start => {
                if c == '{' {
                    self.state = State::BeforeKey;
                } else if !space {
                    self.state = State::Finished;
                }
            }
            State::BeforeKey => {
                if c == '"' {
                    self.key.clear();
                    self.key_chars = 0;
                    self.esc = Esc::None;
                    self.state = State::Key;
                } else if !(space || c == ',') {
                    // `}`: the object closed; anything else: not JSON.
                    self.state = State::Finished;
                }
            }
            State::Key => {
                let o = string_step(&mut self.esc, c);
                for k in [o.first, o.second].into_iter().flatten() {
                    if self.key_chars < MAX_KEY_CHARS {
                        self.key.push(k);
                    }
                    self.key_chars = self.key_chars.saturating_add(1);
                }
                if o.end {
                    self.is_answer = self.key_chars == "answer".len() && self.key == "answer";
                    self.state = State::AfterKey;
                }
            }
            State::AfterKey => {
                if c == ':' {
                    self.state = State::BeforeValue;
                } else if !space {
                    self.state = State::Finished;
                }
            }
            State::BeforeValue => match c {
                '"' => {
                    self.esc = Esc::None;
                    self.state = if self.is_answer { State::Answer } else { State::SkipString };
                }
                '{' | '[' => self.state = State::SkipNested { depth: 1, in_string: false, escaped: false },
                ',' => self.state = State::BeforeKey,
                '}' => self.state = State::Finished,
                _ if space => {}
                _ => self.state = State::SkipScalar,
            },
            State::Answer => {
                let o = string_step(&mut self.esc, c);
                self.emit(o.first, out);
                self.emit(o.second, out);
                if o.end && self.state == State::Answer {
                    self.complete = true;
                    self.state = State::Finished;
                }
            }
            State::SkipString => {
                if string_step(&mut self.esc, c).end {
                    self.state = State::AfterValue;
                }
            }
            State::SkipNested { depth, in_string, escaped } => {
                self.state = if in_string {
                    match (escaped, c) {
                        (true, _) => State::SkipNested { depth, in_string, escaped: false },
                        (false, '\\') => State::SkipNested { depth, in_string, escaped: true },
                        (false, '"') => State::SkipNested { depth, in_string: false, escaped: false },
                        _ => State::SkipNested { depth, in_string, escaped },
                    }
                } else {
                    match c {
                        '"' => State::SkipNested { depth, in_string: true, escaped: false },
                        '{' | '[' if depth >= MAX_DEPTH => {
                            self.cut = true;
                            State::Finished
                        }
                        '{' | '[' => State::SkipNested { depth: depth.saturating_add(1), in_string, escaped },
                        '}' | ']' if depth <= 1 => State::AfterValue,
                        '}' | ']' => State::SkipNested { depth: depth.saturating_sub(1), in_string, escaped },
                        _ => State::SkipNested { depth, in_string, escaped },
                    }
                };
            }
            State::SkipScalar => match c {
                ',' => self.state = State::BeforeKey,
                '}' => self.state = State::Finished,
                _ => {}
            },
            State::AfterValue => {
                if c == ',' {
                    self.state = State::BeforeKey;
                } else if !space {
                    self.state = State::Finished;
                }
            }
        }
    }
}

/// The answer's text so far in `json`, the document as far as it has come.
pub fn extract_answer(json: &str) -> String {
    AnswerExtractor::new().push(json)
}
