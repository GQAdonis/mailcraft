// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A model provider as the owner set it up: where it is, which model, whether it is on this
//! computer or elsewhere, which accounts' mail it may be sent, and how long it is waited for.
//!
//! The settings may be saved with the owner's other settings; the key never is
//! ([`ProviderConfig::api_key`] is `#[serde(skip)]`), and `Debug` shows neither the key nor the
//! address.

use std::fmt;
use std::net::IpAddr;

use serde::{Deserialize, Serialize};

use crate::error::AiError;
use crate::key::ApiKey;

/// Longest provider id: it names the provider's key in the credential store.
pub const MAX_ID: usize = 64;
/// Longest model name.
const MAX_MODEL: usize = 200;
/// Longest base address.
const MAX_URL: usize = 2_000;
/// Most accounts a provider may be allowed, and the longest name of one.
const MAX_ACCOUNTS: usize = 1_000;
const MAX_ACCOUNT: usize = 320;
/// The context window a provider is assumed to have unless the owner says (Apple's on-device
/// model: 4,096 tokens), and the bounds of what may be said.
const DEFAULT_CONTEXT: u32 = 4_096;
/// The context window assumed for a gateway or a provider elsewhere unless the owner (or the
/// server) says.
const DEFAULT_CONTEXT_ELSEWHERE: u32 = 32_768;
/// The bounds of a context window, whoever says it.
pub const MIN_CONTEXT: u32 = 256;
pub const MAX_CONTEXT: u32 = 1_048_576;

/// The context window assumed for a provider of `kind` when nobody says: 4,096 tokens on this
/// computer, 32,768 for a gateway or a provider elsewhere.
pub fn default_context_window(kind: ProviderKind) -> u32 {
    match kind {
        ProviderKind::Local => DEFAULT_CONTEXT,
        ProviderKind::Remote | ProviderKind::Gateway => DEFAULT_CONTEXT_ELSEWHERE,
    }
}

/// A context window said by a server or a file, within [`MIN_CONTEXT`] to [`MAX_CONTEXT`]
/// (clamped, as any number from outside).
pub fn clamp_context_window(tokens: u64) -> u32 {
    u32::try_from(tokens.clamp(u64::from(MIN_CONTEXT), u64::from(MAX_CONTEXT))).unwrap_or(MAX_CONTEXT)
}
/// The ceilings of the waits, in seconds (`mailcraft_net` keeps to its own as well).
const MAX_CONNECT_SECS: u32 = 20;
const MAX_WAIT_SECS: u32 = 300;

/// Where a provider runs, and where the mail it is sent ends up.
///
/// Settings written before [`ProviderKind::Gateway`] existed name `local` or `remote` and read as
/// they always did. A kind this version doesn't know (a later version's) is not read at all: the
/// provider is dropped from the settings, never taken for one on this computer, so nothing is sent
/// to it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    /// On this computer (Ollama, LM Studio, apfel…): a loopback address, over plain HTTP, which
    /// is checked on the address actually connected to. Mail sent to it stays on this computer.
    #[default]
    Local,
    /// Elsewhere: HTTPS only, and mail only of the accounts the owner allowed.
    Remote,
    /// A gateway on this computer that hands what it is sent to a service on the internet
    /// (LiteLLM, liter-llm, a corporate proxy…): spoken to as a provider on this computer is
    /// (plain HTTP to a loopback address, checked on the address connected to), and sent mail only
    /// of the accounts the owner allowed, as a provider elsewhere is. Its status says so: "through
    /// a gateway on this computer that sends your mail to the internet".
    Gateway,
}

impl ProviderKind {
    /// The kind's name, as the settings write it.
    pub fn name(self) -> &'static str {
        match self {
            ProviderKind::Local => "local",
            ProviderKind::Remote => "remote",
            ProviderKind::Gateway => "gateway",
        }
    }

    /// Whether the provider's address is on this computer (spoken to over plain HTTP to loopback).
    pub fn on_this_computer(self) -> bool {
        matches!(self, ProviderKind::Local | ProviderKind::Gateway)
    }

    /// Whether mail sent to it leaves this computer, so that each account's mail needs the owner's
    /// consent ([`ProviderConfig::may_send`]).
    pub fn mail_leaves_this_computer(self) -> bool {
        matches!(self, ProviderKind::Remote | ProviderKind::Gateway)
    }
}

/// How long a provider is waited for, in seconds. The product's defaults: 10 to connect, 60 for
/// the first byte and for a silence, 300 for the whole.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Timeouts {
    pub connect_secs: u32,
    pub first_byte_secs: u32,
    pub idle_secs: u32,
    pub total_secs: u32,
}

impl Default for Timeouts {
    fn default() -> Self {
        Timeouts { connect_secs: 10, first_byte_secs: 60, idle_secs: 60, total_secs: 300 }
    }
}

impl Timeouts {
    /// Within the ceilings, none of them zero, none longer than the whole.
    pub fn clamped(self) -> Timeouts {
        let total = self.total_secs.clamp(1, MAX_WAIT_SECS);
        Timeouts {
            connect_secs: self.connect_secs.clamp(1, MAX_CONNECT_SECS).min(total),
            first_byte_secs: self.first_byte_secs.clamp(1, MAX_WAIT_SECS).min(total),
            idle_secs: self.idle_secs.clamp(1, MAX_WAIT_SECS).min(total),
            total_secs: total,
        }
    }
}

/// What a request carries, for the owner's consent ([`ProviderConfig::may_send`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Carries<'a> {
    /// Only what the owner typed (a question, an instruction), and nothing of any message.
    OwnerTextOnly,
    /// Mail (excerpts, subjects, addresses, attachments' names) of these accounts, by address.
    MailOf(&'a [&'a str]),
}

/// A model provider the owner set up.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    /// The provider's own name: letters, digits, `.`, `-` and `_`, at most [`MAX_ID`]. The key is
    /// kept under it in the credential store.
    pub id: String,
    pub kind: ProviderKind,
    /// `http://127.0.0.1:11434` or `https://api.example.org/v1`: the server, with or without
    /// `/v1`. No user name, query or fragment.
    pub base_url: String,
    /// The model answers come from.
    pub model: String,
    /// The model embeddings come from; empty for none.
    pub embedding_model: String,
    /// The SHA-256 fingerprint (64 hexadecimal digits) of the one certificate the owner chose to
    /// trust for this provider although the system doesn't; empty for none. HTTPS only.
    pub pinned_sha256: String,
    /// The accounts (by address) whose mail a remote provider or a gateway may be sent. A local
    /// provider is on this computer and needs none.
    pub consented_accounts: Vec<String>,
    pub timeouts: Timeouts,
    /// How many tokens the model takes, question and answer together.
    pub context_window: u32,
    /// The key, in memory only: never saved with the settings.
    #[serde(skip)]
    pub api_key: Option<ApiKey>,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        ProviderConfig {
            id: String::new(),
            kind: ProviderKind::Local,
            base_url: String::new(),
            model: String::new(),
            embedding_model: String::new(),
            pinned_sha256: String::new(),
            consented_accounts: Vec::new(),
            timeouts: Timeouts::default(),
            context_window: DEFAULT_CONTEXT,
            api_key: None,
        }
    }
}

impl fmt::Debug for ProviderConfig {
    /// What kind of provider, which model and whether a key is held: no address, no key, no accounts.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderConfig")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("model", &self.model)
            .field("has_key", &self.api_key.is_some())
            .field("pinned", &!self.pinned_sha256.trim().is_empty())
            .field("consented_accounts", &self.consented_accounts.len())
            .finish()
    }
}

/// Whether `id` may name a provider (and its key in the credential store).
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

fn valid_model(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty() && name.len() <= MAX_MODEL && !name.chars().any(char::is_control)
}

/// Whether `host` is this computer: `localhost`, or a loopback address (127.0.0.0/8, `::1`, or an
/// IPv4 loopback address mapped into IPv6). Brackets around an IPv6 address are allowed.
pub fn is_loopback_host(host: &str) -> bool {
    let host = host.trim();
    let host = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    let host = host.strip_suffix('.').unwrap_or(host);
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(v4)) => v4.is_loopback(),
        Ok(IpAddr::V6(v6)) => v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback()),
        Err(_) => false,
    }
}

/// Whether `url` is `https://` (else `http://`) and its host; none for anything else.
fn scheme_and_host(url: &str) -> Option<(bool, &str)> {
    let (scheme, rest) = url.split_once("://")?;
    let secure = if scheme.eq_ignore_ascii_case("https") {
        true
    } else if scheme.eq_ignore_ascii_case("http") {
        false
    } else {
        return None;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split_once(']')?.0,
        None => authority.split_once(':').map_or(authority, |(host, _)| host),
    };
    (!host.is_empty()).then_some((secure, host))
}

impl ProviderConfig {
    /// Whether the settings can be used, and if not why, before anything is sent: a name, a model,
    /// an address of the right kind (a local provider on this computer, a remote one over
    /// HTTPS), a fingerprint that is one, and a list of accounts that is one.
    pub fn check(&self) -> Result<(), AiError> {
        let bad = |why: &str| Err(AiError::Config(why.to_string()));
        if !valid_id(&self.id) {
            return bad("its name must be 1 to 64 letters, digits, dots, dashes or underscores");
        }
        if !valid_model(&self.model) {
            return bad("a model must be named");
        }
        if !self.embedding_model.trim().is_empty() && !valid_model(&self.embedding_model) {
            return bad("the embedding model's name can't be used");
        }
        let url = self.base_url.trim();
        if url.is_empty() || url.len() > MAX_URL || !url.bytes().all(|b| b.is_ascii_graphic()) || url.contains(['?', '#', '@']) {
            return bad("its address can't be used");
        }
        let Some((secure, host)) = scheme_and_host(url) else { return bad("its address must start with http:// or https://") };
        match self.kind {
            kind if kind.on_this_computer() && !is_loopback_host(host) => {
                return bad("a provider on this computer must have an address of this computer (localhost, 127.0.0.1 or ::1)");
            }
            // Every connection to a provider on this computer (a gateway's too) goes the one way
            // that checks the address connected to is this computer's: plain HTTP to loopback.
            kind if kind.on_this_computer() && secure => return bad("a provider on this computer is spoken to over http:// to this computer"),
            ProviderKind::Remote if !secure => return bad("a provider on another computer is spoken to over https:// only"),
            _ => {}
        }
        let pin = self.pinned_sha256.trim();
        if !pin.is_empty() && (!secure || pin.len() != 64 || !pin.bytes().all(|b| b.is_ascii_hexdigit())) {
            return bad("the trusted certificate's fingerprint must be 64 hexadecimal digits, for an https:// address");
        }
        let account_is_bad = |account: &String| account.len() > MAX_ACCOUNT || account.chars().any(char::is_control);
        if self.consented_accounts.len() > MAX_ACCOUNTS || self.consented_accounts.iter().any(account_is_bad) {
            return bad("the list of accounts it is allowed isn't one");
        }
        if !(MIN_CONTEXT..=MAX_CONTEXT).contains(&self.context_window) {
            return bad("its context window must be between 256 and 1,048,576 tokens");
        }
        Ok(())
    }

    /// Whether the owner allowed this provider to be sent `account`'s mail: always for a provider
    /// on this computer; for a remote one or a gateway only when the account is on its list.
    pub fn consents(&self, account: &str) -> bool {
        let account = account.trim();
        if !self.kind.mail_leaves_this_computer() {
            return true;
        }
        !account.is_empty() && self.consented_accounts.iter().any(|allowed| allowed.trim().eq_ignore_ascii_case(account))
    }

    /// Whether a request that carries `carries` may go to this provider: [`AiError::NoConsent`]
    /// when a remote provider or a gateway would be sent mail of an account the owner hasn't
    /// allowed it (or mail of no named account at all).
    pub fn may_send(&self, carries: Carries<'_>) -> Result<(), AiError> {
        match carries {
            _ if !self.kind.mail_leaves_this_computer() => Ok(()),
            Carries::OwnerTextOnly => Ok(()),
            Carries::MailOf(accounts) => {
                if !accounts.is_empty() && accounts.iter().all(|account| self.consents(account)) {
                    Ok(())
                } else {
                    Err(AiError::NoConsent)
                }
            }
        }
    }

    /// The address of one of the server's own endpoints outside `/v1` (apfel's `health`): at the
    /// root of the base address, with a last `/v1` taken off.
    pub fn root_endpoint(&self, path: &str) -> String {
        let base = self.base_url.trim().trim_end_matches('/');
        let path = path.trim_start_matches('/');
        let has_version = base.len() >= 3 && base.get(base.len() - 3..).is_some_and(|end| end.eq_ignore_ascii_case("/v1"));
        let root = if has_version { base.get(..base.len() - 3).unwrap_or(base) } else { base };
        format!("{}/{path}", root.trim_end_matches('/'))
    }

    /// The address of one of the provider's endpoints (`chat/completions`, `embeddings`): under
    /// the base address's `/v1`, which is added when the base doesn't end in it.
    pub fn endpoint(&self, path: &str) -> String {
        let base = self.base_url.trim().trim_end_matches('/');
        let path = path.trim_start_matches('/');
        let has_version = base.len() >= 3 && base.get(base.len() - 3..).is_some_and(|end| end.eq_ignore_ascii_case("/v1"));
        if has_version { format!("{base}/{path}") } else { format!("{base}/v1/{path}") }
    }
}
