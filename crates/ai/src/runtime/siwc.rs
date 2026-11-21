// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Sign in with ChatGPT: tokens and the refresh decision (B7). Pure: no I/O, builds everywhere.
//!
//! - **Tokens** ([`AccessToken`], [`RefreshToken`], [`IssuedClientId`]) hide their text from
//!   `Debug` and are never `Serialize`d. The access token lives in memory alone ([`AccessGrant`]);
//!   the issued client id, the refresh token and the account id are the credential entry
//!   `ai.chatgpt` ([`StoredSignIn`]), written to the system's credential store by the host and
//!   nowhere else.
//! - **When to refresh** ([`needs_refresh`]): when there is no access token in memory (after a
//!   restart the first request refreshes first) or it runs out within [`REFRESH_MARGIN_SECS`];
//!   and once after an HTTP 401 ([`refresh_after_unauthorized`]).
//! - **What a failed refresh means** ([`classify`]): `invalid_grant`, `invalid_client` and
//!   `unauthorized_client` end the sign-in ([`RefreshFailure::Terminal`]); 408, 429 (with its
//!   `Retry-After`), any other 4xx without one of those, 5xx and network errors keep it and are
//!   retried after a backoff ([`RefreshFailure::Transient`]).

use std::fmt;
use std::time::Duration;

use serde_json::{Value, json};

/// An access token running out within this many seconds is refreshed first.
pub const REFRESH_MARGIN_SECS: i64 = 60;
/// The grant errors that end a sign-in.
pub const TERMINAL_GRANT_ERRORS: [&str; 3] = ["invalid_grant", "invalid_client", "unauthorized_client"];
/// The first wait after a transient failure, doubled each time up to [`MAX_BACKOFF`].
pub const FIRST_BACKOFF: Duration = Duration::from_secs(2);
/// The longest wait between two refreshes that failed for a passing reason.
pub const MAX_BACKOFF: Duration = Duration::from_secs(5 * 60);
/// The longest `Retry-After` that is honoured.
pub const MAX_RETRY_AFTER: Duration = Duration::from_secs(60 * 60);
/// What the owner is told when OpenAI ended the sign-in (interface text; the host translates it).
pub const SIGN_IN_ENDED_TEXT: &str = "OpenAI ended this sign-in. Sign in with ChatGPT again to keep using your ChatGPT plan.";
/// What the owner is told after signing out (B11 shows it beside the Sign out button's result).
pub const SIGN_OUT_POINTER_TEXT: &str =
    "MailCraft has forgotten this ChatGPT sign-in on this computer. You can also remove MailCraft from the connected apps in your ChatGPT settings.";

/// The longest secret of any kind that is kept.
const MAX_SECRET: usize = 16 * 1024;
/// The longest issued client id and account id that are kept.
const MAX_ID: usize = 256;
/// The largest credential entry that is read.
const MAX_ENTRY: usize = 32 * 1024;
/// The most characters of an identity that are shown.
const MAX_SHOWN: usize = 128;

macro_rules! secret {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, Eq)]
        pub struct $name(String);

        impl $name {
            /// The text, when it is visible ASCII of a sane length.
            pub fn new(text: &str) -> Option<$name> {
                usable(text, MAX_SECRET).then(|| $name(text.to_string()))
            }

            /// The text, to send. Never log or show it.
            pub fn expose(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "(<{} bytes>)"), self.0.len())
            }
        }
    };
}

secret!(
    /// An access token: in memory alone, never stored.
    AccessToken
);
secret!(
    /// A refresh token: in the credential entry alone.
    RefreshToken
);

/// The client id OpenAI issued at the sign-in, bound to the owner and workspace. Not a password,
/// but kept like one: in the credential entry, hidden from `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct IssuedClientId(String);

impl IssuedClientId {
    /// The id, when it is letters, digits, `-`, `_`, `.` and `:` of a sane length.
    pub fn new(text: &str) -> Option<IssuedClientId> {
        let ok =
            !text.is_empty() && text.len() <= MAX_ID && text.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'));
        ok.then(|| IssuedClientId(text.to_string()))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for IssuedClientId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "IssuedClientId(<{} bytes>)", self.0.len())
    }
}

fn usable(text: &str, max: usize) -> bool {
    !text.is_empty() && text.len() <= max && text.bytes().all(|b| matches!(b, b' '..=b'~'))
}

/// The access token held in memory and when it runs out (seconds since 1970).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessGrant {
    pub token: AccessToken,
    pub expires_at: i64,
}

impl AccessGrant {
    /// Whether it is good for at least [`REFRESH_MARGIN_SECS`] more at `now`.
    pub fn is_fresh(&self, now: i64) -> bool {
        self.expires_at.saturating_sub(now) >= REFRESH_MARGIN_SECS
    }
}

/// Whether a request must refresh first: no access token in memory, or one running out within
/// [`REFRESH_MARGIN_SECS`].
pub fn needs_refresh(grant: Option<&AccessGrant>, now: i64) -> bool {
    grant.is_none_or(|g| !g.is_fresh(now))
}

/// Whether a request answered with HTTP 401 is refreshed and tried again: once, never twice.
pub fn refresh_after_unauthorized(already_refreshed_for_401: bool) -> bool {
    !already_refreshed_for_401
}

/// The credential entry `ai.chatgpt`: the issued client id, the refresh token and the account id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredSignIn {
    pub client_id: IssuedClientId,
    pub refresh_token: RefreshToken,
    /// Letters, digits, `-` and `_`; empty when the sign-in didn't say.
    pub account_id: String,
}

impl StoredSignIn {
    /// The entry, when its parts are what they should be (the account id is dropped when it isn't one).
    pub fn new(client_id: &str, refresh_token: &str, account_id: &str) -> Option<StoredSignIn> {
        let account_ok = account_id.len() <= MAX_ID && account_id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'));
        Some(StoredSignIn {
            client_id: IssuedClientId::new(client_id)?,
            refresh_token: RefreshToken::new(refresh_token)?,
            account_id: if account_ok { account_id.to_string() } else { String::new() },
        })
    }

    /// The text written to the credential store (JSON). For the store alone.
    pub fn to_entry(&self) -> String {
        json!({"v": 1, "client_id": self.client_id.expose(), "refresh_token": self.refresh_token.expose(), "account_id": self.account_id}).to_string()
    }

    /// The entry the store holds, when it is one; `None` for anything else (damaged, too large,
    /// written by something else).
    pub fn from_entry(text: &str) -> Option<StoredSignIn> {
        if text.len() > MAX_ENTRY {
            return None;
        }
        let value = serde_json::from_str::<Value>(text).ok()?;
        let field = |name: &str| value.get(name).and_then(Value::as_str);
        if value.get("v").and_then(Value::as_u64) != Some(1) {
            return None;
        }
        StoredSignIn::new(field("client_id")?, field("refresh_token")?, field("account_id").unwrap_or(""))
    }
}

/// Why a refresh failed, and what to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefreshFailure {
    /// OpenAI ended the sign-in: delete the entry, turn the runtime off, tell the owner.
    Terminal { code: &'static str },
    /// A passing failure: keep the entry, wait at least `wait` before the next refresh.
    Transient { wait: Duration, kind: TransientKind },
}

/// What kind of passing failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransientKind {
    /// 429.
    RateLimited,
    /// 408.
    Timeout,
    /// Another 4xx without a terminal grant error.
    ClientError,
    /// 5xx, or a status that is neither.
    ServerError,
    /// The server couldn't be reached, or its answer couldn't be read.
    Network,
}

impl TransientKind {
    /// Its name, the only thing of it that is logged.
    pub fn name(self) -> &'static str {
        match self {
            TransientKind::RateLimited => "rate_limited",
            TransientKind::Timeout => "timeout",
            TransientKind::ClientError => "client_error",
            TransientKind::ServerError => "server_error",
            TransientKind::Network => "network",
        }
    }
}

/// What a failed refresh means. `status` is the token endpoint's HTTP status (`None` when it
/// couldn't be reached), `code` its `error` (empty for none), `retry_after` its `Retry-After`;
/// `attempt` counts the transient failures before this one (0 for the first).
pub fn classify(status: Option<u16>, code: &str, retry_after: Option<&str>, attempt: u32) -> RefreshFailure {
    if let Some(terminal) = TERMINAL_GRANT_ERRORS.iter().find(|t| **t == code.trim()) {
        return RefreshFailure::Terminal { code: terminal };
    }
    let wait = backoff(attempt);
    let (kind, wait) = match status {
        None => (TransientKind::Network, wait),
        Some(429) => (TransientKind::RateLimited, wait.max(retry_after.and_then(retry_after_wait).unwrap_or(Duration::ZERO))),
        Some(408) => (TransientKind::Timeout, wait),
        Some(400..=499) => (TransientKind::ClientError, wait),
        Some(_) => (TransientKind::ServerError, wait),
    };
    RefreshFailure::Transient { wait, kind }
}

/// The wait after `attempt` earlier transient failures: [`FIRST_BACKOFF`] doubled each time, at
/// most [`MAX_BACKOFF`].
pub fn backoff(attempt: u32) -> Duration {
    let factor = 1u32.checked_shl(attempt.min(16)).unwrap_or(u32::MAX);
    FIRST_BACKOFF.saturating_mul(factor).min(MAX_BACKOFF)
}

/// A `Retry-After` in seconds (a date is not read: the backoff stands), at most [`MAX_RETRY_AFTER`].
pub fn retry_after_wait(text: &str) -> Option<Duration> {
    let seconds = text.trim().parse::<u64>().ok()?;
    Some(Duration::from_secs(seconds).min(MAX_RETRY_AFTER))
}

/// Untrusted text for showing (an email, a plan): control characters and direction overrides
/// out, at most 128 characters.
pub fn shown_identity(text: &str) -> String {
    let printable = |c: &char| !c.is_control() && !matches!(*c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}');
    text.chars().filter(printable).take(MAX_SHOWN).collect::<String>().trim().to_string()
}

/// Where the ChatGPT plan's sign-in stands, as the window shows it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SiwcStatus {
    /// Never signed in, or signed out.
    #[default]
    SignedOut,
    /// Waiting for the owner on OpenAI's page.
    Waiting,
    /// Signed in: answers may be asked for.
    SignedIn,
    /// OpenAI ended the sign-in ([`SIGN_IN_ENDED_TEXT`]); the runtime is off until the owner signs in again.
    SignInEnded,
}

impl SiwcStatus {
    /// Whether the runtime may be asked for answers.
    pub fn is_on(self) -> bool {
        self == SiwcStatus::SignedIn
    }

    pub fn name(self) -> &'static str {
        match self {
            SiwcStatus::SignedOut => "signed_out",
            SiwcStatus::Waiting => "waiting",
            SiwcStatus::SignedIn => "signed_in",
            SiwcStatus::SignInEnded => "sign_in_ended",
        }
    }
}

#[cfg(test)]
#[path = "siwc_tests.rs"]
mod siwc_tests;
