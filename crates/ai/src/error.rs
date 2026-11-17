// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What went wrong talking to a model provider, in words that never carry a key or mail: our own
//! words, and of what a server or the system said only a tidied line with the key blanked out.

use std::fmt;

/// Longest text taken from a server or the system into an error.
const MAX_SHOWN: usize = 300;

/// Why a request to a model provider did not give an answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AiError {
    /// No provider is set up, or none for this use (no embedding model named).
    NotConfigured,
    /// The provider's settings, or the request, can't be used: what is wrong, in our words.
    Config(String),
    /// A remote provider was to carry mail of an account the owner hasn't allowed it for.
    /// Nothing was sent.
    NoConsent,
    /// Nothing was sent: the address may not be spoken to (plain HTTP to another computer).
    Refused(String),
    /// The provider couldn't be reached (a closed port, a name that isn't found, a broken connection).
    Unreachable(String),
    /// The provider's certificate wasn't accepted; nothing of the request went.
    Certificate(String),
    /// The provider didn't begin to answer, or stopped answering, in its time.
    TimedOut(String),
    /// The owner (or the host for the owner) stopped the request.
    Cancelled,
    /// The provider answered with a status other than a success; what it said, tidied.
    Status { status: u16, message: String },
    /// The provider answered with an error in its answer (or in the middle of a stream).
    Provider(String),
    /// The answer isn't what was asked for, or isn't readable.
    Malformed(String),
    /// The answer is longer than is taken.
    TooLarge,
}

impl AiError {
    /// Whether the request was stopped on purpose.
    pub fn is_cancelled(&self) -> bool {
        matches!(self, AiError::Cancelled)
    }

    /// Whether the provider ran out of time.
    pub fn is_timeout(&self) -> bool {
        matches!(self, AiError::TimedOut(_))
    }

    /// Whether nothing was sent because of the owner's settings or choices (no provider, no
    /// consent, an address that may not be spoken to, settings that can't be used).
    pub fn is_refused_before_sending(&self) -> bool {
        matches!(self, AiError::NotConfigured | AiError::Config(_) | AiError::NoConsent | AiError::Refused(_))
    }
}

impl fmt::Display for AiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AiError::NotConfigured => f.write_str("no model provider is set up for this"),
            AiError::Config(why) => write!(f, "the model provider's settings can't be used: {why}"),
            AiError::NoConsent => {
                f.write_str("this account's mail isn't sent to the remote model provider: the owner hasn't allowed it for this account")
            }
            AiError::Refused(why) => write!(f, "the model provider was not contacted: {why}"),
            AiError::Unreachable(why) => write!(f, "couldn't reach the model provider: {why}"),
            AiError::Certificate(why) => write!(f, "the model provider's certificate wasn't accepted: {why}"),
            AiError::TimedOut(why) => write!(f, "the model provider took too long: {why}"),
            AiError::Cancelled => f.write_str("the request was stopped"),
            AiError::Status { status, message } if message.is_empty() => write!(f, "the model provider answered {status}"),
            AiError::Status { status, message } => write!(f, "the model provider answered {status}: {message}"),
            AiError::Provider(message) => write!(f, "the model provider said: {message}"),
            AiError::Malformed(why) => write!(f, "the model provider's answer couldn't be read: {why}"),
            AiError::TooLarge => f.write_str("the model provider's answer is too long"),
        }
    }
}

impl std::error::Error for AiError {}

/// Text from a server or the system made fit for an error: `secret` (a key) blanked out, one
/// line, no control characters, no longer than [`MAX_SHOWN`] characters.
pub(crate) fn tidy(text: &str, secret: Option<&str>) -> String {
    let scrubbed = match secret {
        Some(secret) if !secret.is_empty() && text.contains(secret) => text.replace(secret, "***"),
        _ => text.to_string(),
    };
    let words: Vec<&str> = scrubbed.split(|c: char| c.is_whitespace() || c.is_control()).filter(|w| !w.is_empty()).collect();
    let line = words.join(" ");
    let line = match secret {
        Some(secret) if !secret.is_empty() && line.contains(secret) => line.replace(secret, "***"),
        _ => line,
    };
    if line.chars().count() <= MAX_SHOWN { line } else { line.chars().take(MAX_SHOWN).chain(['…']).collect() }
}
