// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Why a model request failed, as the owner is told (#ai).
//!
//! The host names the kind of failure ([`ErrorKind`]) with every failed request, and the engine
//! keeps it with the request and answers it from `ai.answerResult` and `ai.narrowResult`. The
//! window reads nothing but that kind: no sentence is read for what it means. The notice is
//! written here, in the interface language, with nothing in it but a status number or the
//! address the owner typed. What the provider itself said, when it said something, is shown
//! apart from the notice, in its details ([`details`]), as data: never read, never followed.

use egui::{Color32, RichText, Ui};
use mailcraft_model::ai::ErrorKind;
use serde_json::Value;

use crate::theme::{Tokens, regular};

/// Most characters of what a provider said that are shown in the details (the engine keeps at
/// most 300 bytes of it already).
const MAX_PROVIDER_SHOWN: usize = 300;

/// What went wrong with a model request, as far as the owner needs to know.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Trouble {
    /// No model is set up: nothing was sent anywhere.
    NoModel,
    /// A remote model hasn't been allowed the mail of an account the answer needed: nothing was sent.
    NoConsent,
    /// Nothing answered at the address (closed port, refused, no such host).
    Unreachable,
    /// The model took longer than the host waits: before it wrote anything, or after it began.
    TimedOut { before_first_byte: bool },
    /// The model answered with an HTTP error, by its number.
    Refused(u16),
    /// What came back stopped making sense (or broke off).
    Malformed,
    /// The answer was longer than MailCraft takes.
    TooLarge,
    /// The model's certificate wasn't accepted.
    Certificate,
    /// The settings can't be used (an address that isn't on this computer over http, …).
    Settings,
    /// The system's credential store didn't give the model's key.
    CredentialStore,
    /// The request was called off before it was finished.
    Cancelled,
    /// No kind was given (a request the session let go of, a command that failed).
    Other,
}

impl From<ErrorKind> for Trouble {
    fn from(kind: ErrorKind) -> Trouble {
        match kind {
            ErrorKind::NotConfigured => Trouble::NoModel,
            ErrorKind::NoConsent => Trouble::NoConsent,
            ErrorKind::Unreachable => Trouble::Unreachable,
            ErrorKind::Refused { status } => Trouble::Refused(status),
            ErrorKind::TimedOut { before_first_byte } => Trouble::TimedOut { before_first_byte },
            ErrorKind::Certificate => Trouble::Certificate,
            ErrorKind::Settings => Trouble::Settings,
            ErrorKind::CredentialStore => Trouble::CredentialStore,
            ErrorKind::Malformed | ErrorKind::BrokenOff => Trouble::Malformed,
            ErrorKind::TooLarge => Trouble::TooLarge,
            ErrorKind::Cancelled => Trouble::Cancelled,
            // The runtime kinds get notices of their own with the runtime chain (B10).
            ErrorKind::SignedOut | ErrorKind::ToolUseStopped | ErrorKind::NoSandbox | ErrorKind::BrokenRuntime => Trouble::Other,
        }
    }
}

impl Trouble {
    /// The trouble of a request that failed with `kind`; [`Trouble::Other`] when it has none.
    pub(crate) fn of_kind(kind: Option<ErrorKind>) -> Trouble {
        kind.map_or(Trouble::Other, Trouble::from)
    }

    /// The trouble of a failed request as `ai.answerResult` or `ai.narrowResult` answers it
    /// (`{"state": "failed", "kind", "status"?, "beforeFirstByte"?}`), or `None` when `read`
    /// isn't a failure. Only the kind's name and its details are read, never `message`; a name
    /// this version doesn't know, or a refusal without a usable status, is [`Trouble::Other`].
    pub(crate) fn of_result(read: &Value) -> Option<Trouble> {
        if read.get("state").and_then(Value::as_str) != Some("failed") {
            return None;
        }
        let trouble = match read.get("kind").and_then(Value::as_str).unwrap_or_default() {
            "not_configured" => Trouble::NoModel,
            "no_consent" => Trouble::NoConsent,
            "unreachable" => Trouble::Unreachable,
            "refused" => read
                .get("status")
                .and_then(Value::as_u64)
                .and_then(|n| u16::try_from(n).ok())
                .filter(|n| (100..=599).contains(n))
                .map_or(Trouble::Other, Trouble::Refused),
            // Said or not, a time-out with nothing written yet is the safer reading.
            "timed_out" => Trouble::TimedOut { before_first_byte: read.get("beforeFirstByte").and_then(Value::as_bool).unwrap_or(true) },
            "certificate" => Trouble::Certificate,
            "settings" => Trouble::Settings,
            "credential_store" => Trouble::CredentialStore,
            "malformed" | "broken_off" => Trouble::Malformed,
            "too_large" => Trouble::TooLarge,
            "cancelled" => Trouble::Cancelled,
            _ => Trouble::Other,
        };
        Some(trouble)
    }

    /// The notice's hue: red for what the owner has to change, orange for what may pass, blue
    /// for what is only so.
    pub(crate) fn hue(self, t: &Tokens) -> Color32 {
        match self {
            Trouble::NoModel | Trouble::NoConsent | Trouble::Cancelled => t.blue,
            Trouble::TimedOut { .. } | Trouble::Malformed | Trouble::TooLarge | Trouble::CredentialStore => t.orange,
            Trouble::Unreachable | Trouble::Refused(_) | Trouble::Certificate | Trouble::Settings | Trouble::Other => t.red,
        }
    }

    /// Whether File › Options can do something about it.
    pub(crate) fn options_help(self) -> bool {
        matches!(
            self,
            Trouble::NoModel
                | Trouble::NoConsent
                | Trouble::Unreachable
                | Trouble::Refused(_)
                | Trouble::Certificate
                | Trouble::Settings
                | Trouble::CredentialStore
        )
    }

    /// Whether part of an answer may have come before it: the answer above it is incomplete.
    pub(crate) fn broke_off(self) -> bool {
        matches!(self, Trouble::TimedOut { before_first_byte: false } | Trouble::Malformed | Trouble::TooLarge)
    }

    /// The notice's lead words and the rest, for a search being suggested (`narrowing`) or an
    /// answer, with `address` the address the owner set up (as shown: scheme, host and port).
    pub(crate) fn words(self, address: &str, narrowing: bool) -> (String, String) {
        let at = |template: &str| crate::i18n::fmt(template, &[("address", address)]);
        match self {
            Trouble::NoModel if narrowing => (
                tl!("No model is set up.").to_string(),
                tl!("To describe searches in words, set one up in File › Options › AI assistance. Until then nothing is sent anywhere.").to_string(),
            ),
            Trouble::NoModel => (
                tl!("No model is set up.").to_string(),
                tl!("To ask about your mail, set one up in File › Options › AI assistance. Until then nothing is sent anywhere.").to_string(),
            ),
            Trouble::NoConsent => (
                tl!("Nothing was sent.").to_string(),
                at(tl!(
                    "The messages that answer this are in an account whose mail isn't allowed to go to {address}. Allow it in File › Options › AI assistance, then ask again."
                )),
            ),
            Trouble::Unreachable if narrowing => {
                (tl!("Couldn't reach the model.").to_string(), at(tl!("Nothing answered at {address}. Check that it is running, then try again.")))
            }
            Trouble::Unreachable => {
                (tl!("Couldn't reach the model.").to_string(), at(tl!("Nothing answered at {address}. Check that it is running, then ask again.")))
            }
            Trouble::TimedOut { .. } if narrowing => (
                tl!("The model didn't answer.").to_string(),
                tl!("It took too long. Try again, or choose a faster model in File › Options.").to_string(),
            ),
            Trouble::TimedOut { before_first_byte: true } => (
                tl!("The model didn't answer.").to_string(),
                tl!("It took too long to start. Ask again, or choose a faster model in File › Options.").to_string(),
            ),
            Trouble::TimedOut { before_first_byte: false } => {
                (tl!("The model stopped answering.").to_string(), tl!("Nothing came for too long.").to_string())
            }
            Trouble::Refused(status) => (
                tl!("The model turned the request down.").to_string(),
                crate::i18n::fmt(tl!("{address} answered with error {status}."), &[("address", address), ("status", &status.to_string())]),
            ),
            Trouble::Malformed if narrowing => (
                tl!("No search came back.").to_string(),
                tl!("The model's answer wasn't a search MailCraft can read. Try again, describe it another way, or write the search yourself.")
                    .to_string(),
            ),
            Trouble::Malformed => (
                tl!("The answer broke off.").to_string(),
                tl!("What the model sent stopped making sense, so MailCraft stopped reading it. The part above may be incomplete.").to_string(),
            ),
            Trouble::TooLarge if narrowing => (
                tl!("No search came back.").to_string(),
                tl!("The model's answer was too long to be a search. Try again, describe it another way, or write the search yourself.").to_string(),
            ),
            Trouble::TooLarge => (
                tl!("The answer was too long.").to_string(),
                tl!("MailCraft kept only its start. Ask a narrower question to get the rest.").to_string(),
            ),
            Trouble::Certificate => (
                tl!("The model's certificate wasn't accepted.").to_string(),
                at(tl!("{address} showed a certificate MailCraft doesn't trust for it. Nothing was sent.")),
            ),
            Trouble::Settings => (
                tl!("The model's settings can't be used.").to_string(),
                at(tl!("Nothing was sent to {address}. Check the address and the key in File › Options › AI assistance.")),
            ),
            Trouble::CredentialStore => (
                tl!("Couldn't get the model's key.").to_string(),
                at(tl!(
                    "The system's credential store didn't give MailCraft the key for {address}, so nothing was sent. Unlock the store, or enter the key again in File › Options › AI assistance."
                )),
            ),
            Trouble::Cancelled => (tl!("The request was stopped.").to_string(), tl!("Nothing more will come for it.").to_string()),
            Trouble::Other => (tl!("The model request failed.").to_string(), tl!("Try again in a moment.").to_string()),
        }
    }
}

/// What the provider itself said of a failure, as it is shown: one line, nothing but printable
/// characters, at most [`MAX_PROVIDER_SHOWN`] characters. Empty when it said nothing.
pub(crate) fn provider_shown(said: &str) -> String {
    let line: String = said.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    super::cut(line.trim(), MAX_PROVIDER_SHOWN)
}

/// The details under a notice: what the provider itself said, folded away until the owner opens
/// it, and labelled as the provider's words. Drawn as plain text only: no link in it is
/// followed and nothing in it is fetched or obeyed. Nothing is drawn when it said nothing.
pub(crate) fn details(ui: &mut Ui, salt: &str, said: &str, t: &Tokens) {
    let shown = provider_shown(said);
    if shown.is_empty() {
        return;
    }
    egui::CollapsingHeader::new(RichText::new(tl!("Details")).font(regular(11.5)).color(t.text_dim))
        .id_salt(("aiTroubleDetails", salt))
        .default_open(false)
        .show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new(tl!("The model provider said:")).font(regular(11.5)).color(t.text_dim)).wrap());
            ui.add(egui::Label::new(RichText::new(shown).font(regular(11.5)).color(t.text)).wrap().selectable(true));
        });
}
