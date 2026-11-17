// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Server-Sent Events: the `text/event-stream` format of the WHATWG HTML standard, in which a
//! model server streams its answer (#ai).
//!
//! The bytes come in pieces of any size, cut anywhere ([`Parser::feed`]); the events come out
//! whole. Lines end in CR LF, LF or CR alike (a CR LF cut between two pieces is one line end);
//! a line that begins with a colon is a comment; `data:` lines add up to one event's data, joined
//! by line feeds; `event:` names the event, `id:` its id (kept for the events after it, as the
//! standard says); `retry:` and anything else is ignored. A byte order mark at the start goes,
//! and what isn't UTF-8 becomes U+FFFD. A stream that ends without the empty line after its last
//! event still gives that event ([`Parser::finish`]), where the standard drops it: what is in it
//! is read as any other.
//!
//! What a server sends here is as hostile as anything else: a line, an event, the whole stream
//! and the count of events are capped ([`Limits`]), and a stream over a cap is an error, which
//! the parser then keeps saying. Nothing here panics, allocates without bound or runs on.
//!
//! [`Event::is_done`] tells the `[DONE]` that OpenAI-compatible servers end a stream with.

use std::borrow::Cow;
use std::fmt;

/// The UTF-8 byte order mark.
const BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

/// How much a stream may hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Bytes in one line, without its line end.
    pub max_line: usize,
    /// Bytes of one event's data, its joining line feeds included.
    pub max_event: usize,
    /// Bytes of the whole stream.
    pub max_total: u64,
    /// Events in the whole stream.
    pub max_events: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { max_line: 64 * 1024, max_event: 256 * 1024, max_total: 16 * 1024 * 1024, max_events: 200_000 }
    }
}

/// One event.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Event {
    /// What its `event:` line named, if it had one (`message` by the standard's default).
    pub kind: Option<String>,
    /// Its data: the values of its `data:` lines, joined by line feeds.
    pub data: String,
    /// The last id the stream gave, if any.
    pub id: Option<String>,
}

impl Event {
    /// Whether this is the `[DONE]` an OpenAI-compatible server ends its stream with.
    pub fn is_done(&self) -> bool {
        self.data.trim() == "[DONE]"
    }
}

impl fmt::Debug for Event {
    /// The kind and the data's length: the data is a model's answer about someone's mail.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Event").field("kind", &self.kind).field("data_len", &self.data.len()).finish()
    }
}

/// Why a stream was given up on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SseError {
    /// A line is longer than [`Limits::max_line`].
    LineTooLong,
    /// An event's data is longer than [`Limits::max_event`].
    EventTooLarge,
    /// The stream is longer than [`Limits::max_total`].
    StreamTooLarge,
    /// The stream has more events than [`Limits::max_events`].
    TooManyEvents,
}

impl fmt::Display for SseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            SseError::LineTooLong => "the server's stream has a line that is too long",
            SseError::EventTooLarge => "the server's stream has an event that is too large",
            SseError::StreamTooLarge => "the server's stream is too long",
            SseError::TooManyEvents => "the server's stream has too many events",
        })
    }
}

impl std::error::Error for SseError {}

/// Reads an event stream from bytes that come in pieces.
#[derive(Clone)]
pub struct Parser {
    limits: Limits,
    /// The line read so far.
    line: Vec<u8>,
    /// The last byte was a CR: an LF next is part of the same line end.
    after_cr: bool,
    /// The first line has ended (a byte order mark is looked for in it alone).
    begun: bool,
    /// The event read so far.
    data: String,
    has_data: bool,
    kind: Option<String>,
    /// The last id, which every event after it carries.
    id: Option<String>,
    total: u64,
    events: u64,
    /// Once given up on, the stream stays given up on.
    failed: Option<SseError>,
}

impl fmt::Debug for Parser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Parser").field("total", &self.total).field("events", &self.events).field("failed", &self.failed).finish()
    }
}

impl Default for Parser {
    fn default() -> Self {
        Parser::new(Limits::default())
    }
}

impl Parser {
    /// A parser for a new stream, within `limits`.
    pub fn new(limits: Limits) -> Parser {
        Parser {
            limits,
            line: Vec::new(),
            after_cr: false,
            begun: false,
            data: String::new(),
            has_data: false,
            kind: None,
            id: None,
            total: 0,
            events: 0,
            failed: None,
        }
    }

    /// Reads `bytes`, the next piece of the stream, and adds the events it completes to `out`.
    /// After an error every call gives the same error, and `out` is not added to.
    pub fn feed(&mut self, bytes: &[u8], out: &mut Vec<Event>) -> Result<(), SseError> {
        if let Some(error) = self.failed {
            return Err(error);
        }
        let read = self.read(bytes, out);
        if let Err(error) = read {
            self.failed = Some(error);
        }
        read
    }

    /// The stream has ended: what is left of its last line and its last event is read, and that
    /// event added to `out` when it has data.
    pub fn finish(&mut self, out: &mut Vec<Event>) -> Result<(), SseError> {
        if let Some(error) = self.failed {
            return Err(error);
        }
        let ended = self.close(out);
        if let Err(error) = ended {
            self.failed = Some(error);
        }
        ended
    }

    fn close(&mut self, out: &mut Vec<Event>) -> Result<(), SseError> {
        if !self.line.is_empty() {
            self.end_line(out)?;
        }
        self.dispatch(out)
    }

    fn read(&mut self, bytes: &[u8], out: &mut Vec<Event>) -> Result<(), SseError> {
        self.total = self.total.saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
        if self.total > self.limits.max_total {
            return Err(SseError::StreamTooLarge);
        }
        for &byte in bytes {
            if self.after_cr {
                self.after_cr = false;
                if byte == b'\n' {
                    continue;
                }
            }
            match byte {
                b'\n' => self.end_line(out)?,
                b'\r' => {
                    self.after_cr = true;
                    self.end_line(out)?;
                }
                _ => {
                    if self.line.len() >= self.limits.max_line {
                        return Err(SseError::LineTooLong);
                    }
                    self.line.push(byte);
                }
            }
        }
        Ok(())
    }

    /// A line has ended: it is a field of the event under way, a comment, or (empty) the event's end.
    fn end_line(&mut self, out: &mut Vec<Event>) -> Result<(), SseError> {
        let line = std::mem::take(&mut self.line);
        let mut bytes: &[u8] = &line;
        if !self.begun {
            self.begun = true;
            bytes = bytes.strip_prefix(BOM).unwrap_or(bytes);
        }
        if bytes.is_empty() {
            return self.dispatch(out);
        }
        if bytes.first() == Some(&b':') {
            return Ok(());
        }
        let text: Cow<'_, str> = String::from_utf8_lossy(bytes);
        let (field, value) = match text.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (text.as_ref(), ""),
        };
        match field {
            "data" => {
                let grown = self.data.len().saturating_add(value.len()).saturating_add(1);
                if grown > self.limits.max_event {
                    return Err(SseError::EventTooLarge);
                }
                self.data.push_str(value);
                self.data.push('\n');
                self.has_data = true;
            }
            "event" => self.kind = Some(value.to_string()),
            "id" if !value.contains('\0') => self.id = Some(value.to_string()),
            // `retry:`, and every field the standard doesn't know.
            _ => {}
        }
        Ok(())
    }

    /// The event under way has ended: given out when it has data, forgotten either way.
    fn dispatch(&mut self, out: &mut Vec<Event>) -> Result<(), SseError> {
        let kind = self.kind.take();
        if !self.has_data {
            self.data.clear();
            return Ok(());
        }
        self.has_data = false;
        let mut data = std::mem::take(&mut self.data);
        if data.ends_with('\n') {
            data.pop();
        }
        self.events = self.events.saturating_add(1);
        if self.events > self.limits.max_events {
            return Err(SseError::TooManyEvents);
        }
        out.push(Event { kind, data, id: self.id.clone() });
        Ok(())
    }
}

#[cfg(test)]
#[path = "sse_tests.rs"]
mod sse_tests;
