// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Sign in with ChatGPT: the flow, the credential entry, sign-out (B7).
//!
//! [`ChatGptRuntime`] is the host side of the `chatgpt` runtime ("ChatGPT plan"):
//!
//! - **Sign-in** ([`ChatGptRuntime::sign_in`]): the owner presses Sign in; the installation's host
//!   id is made once and kept in `ai.json` ([`ensure_host_id`]); `mailcraft_net::oauth::openai`
//!   binds a one-shot 127.0.0.1 listener, opens OpenAI's page through `open` (the window passes
//!   `browser.rs`), checks the `state`, waits at most ten minutes and stops at Cancel. The issued
//!   client id, the refresh token and the account id go to the credential entry `ai.chatgpt`
//!   through `Services::secrets` (read back before the sign-in counts); the access token stays in
//!   this process's memory. `ai.json` holds the host id and the flag that an entry was written
//!   ([`signed_in_settings`]), never the client id or a token.
//! - **Refresh** ([`ChatGptRuntime::access_token`], [`ChatGptRuntime::after_unauthorized`]): when
//!   the access token runs out within 60 s (or there is none, after a restart), or once after an
//!   HTTP 401. One refresh at a time per runtime: callers that find one under way wait for it and
//!   use its token. The rotated refresh token is written only if the entry still holds the one the
//!   refresh started with (compare-and-set). `invalid_grant`, `invalid_client` and
//!   `unauthorized_client` end the sign-in: the entry is deleted, the runtime is off
//!   ([`ended_settings`]) and the owner is told [`SIGN_IN_ENDED_TEXT`]. Anything else keeps the
//!   entry and waits a backoff (at least `Retry-After` for a 429) before the next refresh.
//! - **Answers** ([`ChatGptRuntime::ask`]): `POST` to the Responses API with the request
//!   `mailcraft_ai::runtime::responses` builds, streamed through `mailcraft_net::https`; a tool or
//!   any non-text event stops the run, and only its kind is logged.
//! - **Sign-out** ([`ChatGptRuntime::sign_out`]): the entry is deleted, the access token dropped,
//!   the runtime turned off and `chatgpt` taken out of every account's consents. Nothing is sent to
//!   OpenAI (it documents no revocation endpoint); the owner is pointed to ChatGPT's settings
//!   ([`SIGN_OUT_POINTER_TEXT`]).
//!
//! A token, the client id or the refresh token never reaches a log line, an error's text, `Debug`
//! output, a file of ours or a command's params.

use std::fmt;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, mpsc};
use std::time::Duration;

use mailcraft_ai::runtime::contain::Step;
use mailcraft_ai::runtime::responses::{BuildError, ResponsesRequest, ResponsesStream, build_body};
use mailcraft_ai::runtime::siwc::{
    AccessGrant, AccessToken, RefreshFailure, SIGN_IN_ENDED_TEXT, SIGN_OUT_POINTER_TEXT, SiwcStatus, StoredSignIn, classify,
    refresh_after_unauthorized, shown_identity,
};
use mailcraft_host::SecretStore;
use mailcraft_model::ai::RuntimeId;
use mailcraft_net::https::{self, Deadlines, Flow, Piece, StreamRequest};
use mailcraft_net::oauth::openai::{self, SIGN_IN_PATIENCE, SignInError, SiwcEndpoints, TokenError};

use crate::ai::settings::AiSettings;

/// The most of a Responses answer that is read.
const MAX_RESPONSE: u64 = 16 * 1024 * 1024;
/// The most of an error answer that is read (and dropped).
const MAX_ERROR_RESPONSE: u64 = 16 * 1024;
/// How long a read of the credential store is waited for (it may be asking the owner).
const STORE_READ_WAIT: Duration = crate::ai::keys::READ_WAIT;
/// How long a write or a removal is waited for: longer, since the owner may be asked to allow it.
const STORE_WRITE_WAIT: Duration = Duration::from_secs(30);
/// What is said when the credential store didn't answer in time.
const STORE_SILENT: &str = "the credential store didn't answer in time";

/// The one `chatgpt` runtime of this process, made with `store` the first time it is asked for:
/// the request threads, sign-in and sign-out share it, so that its access token, its backoff, its
/// one refresh at a time and an ended sign-in hold for all of them.
pub(crate) fn shared(store: &Arc<dyn SecretStore>) -> Arc<ChatGptRuntime> {
    static SHARED: OnceLock<Arc<ChatGptRuntime>> = OnceLock::new();
    SHARED.get_or_init(|| Arc::new(ChatGptRuntime::new(store.clone()))).clone()
}

/// `work` on a thread of its own, waited for at most `wait`: `None` when it didn't answer in time
/// (it goes on by itself, and what it brings is dropped), or couldn't be started.
fn bounded<T: Send + 'static>(wait: Duration, work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = mpsc::sync_channel(1);
    let spawned = std::thread::Builder::new().name("credential store".into()).spawn(move || {
        let _ = tx.send(work());
    });
    if spawned.is_err() {
        return None;
    }
    rx.recv_timeout(wait).ok()
}

/// The name of the credential entry: `ai.chatgpt`.
pub(crate) fn entry_name() -> String {
    RuntimeId::ChatGpt.credential_entry().unwrap_or_else(|| "ai.chatgpt".to_string())
}

/// The installation's host id, made once (32 random bytes, base64url) and kept in the settings.
pub(crate) fn ensure_host_id(settings: &mut AiSettings) -> Result<String, SiwcError> {
    if let Some(id) = &settings.chatgpt_host_id {
        return Ok(id.as_str().to_string());
    }
    let id = openai::new_host_id().map_err(|_| SiwcError::Setup)?;
    settings.set_chatgpt_host_id(&id).map_err(|_| SiwcError::Setup)?;
    Ok(id)
}

/// The settings once a sign-in was put in the credential store: they note the entry (so that
/// "Remove MailCraft's data" deletes it), and nothing secret.
pub(crate) fn signed_in_settings(settings: &mut AiSettings) {
    settings.chatgpt_key = true;
}

/// The settings once OpenAI ended the sign-in: no entry, and the runtime off in the chain.
pub(crate) fn ended_settings(settings: &mut AiSettings) {
    settings.chatgpt_key = false;
    turn_off(settings);
}

/// The settings once the owner signed out: as [`ended_settings`], and no account consents to
/// `chatgpt` any more.
pub(crate) fn signed_out_settings(settings: &mut AiSettings) {
    ended_settings(settings);
    settings.consents = settings.consents.without_runtime(&RuntimeId::ChatGpt);
    settings.mirror_consents();
}

fn turn_off(settings: &mut AiSettings) {
    if let Ok(chain) = settings.chain.with_enabled(&RuntimeId::ChatGpt, false) {
        settings.chain = chain;
    }
}

/// What went wrong. The text never holds a token, the client id or anything OpenAI sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SiwcError {
    /// The sign-in on OpenAI's page didn't finish.
    SignIn(SignInError),
    /// No sign-in is kept: the owner has to sign in.
    NotSignedIn,
    /// OpenAI ended the sign-in ([`SIGN_IN_ENDED_TEXT`]); the entry is gone and the runtime off.
    Ended,
    /// A refresh failed for a passing reason; the next is not tried before `wait` is over.
    Waiting { wait: Duration, kind: &'static str },
    /// The credential store refused or failed (its own text is free of secrets).
    Store(String),
    /// The host id couldn't be made or kept.
    Setup,
}

impl fmt::Display for SiwcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SiwcError::SignIn(error) => write!(f, "{error}"),
            SiwcError::NotSignedIn => f.write_str("sign in with ChatGPT first"),
            SiwcError::Ended => f.write_str(SIGN_IN_ENDED_TEXT),
            SiwcError::Waiting { wait, .. } => {
                write!(f, "OpenAI couldn't renew the ChatGPT sign-in just now; trying again in {} s", wait.as_secs().max(1))
            }
            SiwcError::Store(error) => write!(f, "the credential store: {error}"),
            SiwcError::Setup => f.write_str("this computer couldn't make the ChatGPT host id"),
        }
    }
}

impl std::error::Error for SiwcError {}

/// Why an answer didn't come.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AskError {
    /// The sign-in can't give a token.
    Sign(SiwcError),
    /// The request couldn't be made.
    Build(BuildError),
    /// The answer asked for a tool or held something that isn't text: the run was stopped (its kind).
    ToolUseStopped(&'static str),
    /// The response failed, was cut short or was an error event (its kind).
    Failed(&'static str),
    /// The answer was larger than is kept.
    TooLarge,
    /// The Responses API answered with this status.
    Status(u16),
    /// The stream ended without an answer.
    NoAnswer,
    /// The connection (its text is free of the request).
    Net(String),
}

impl fmt::Display for AskError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AskError::Sign(error) => write!(f, "{error}"),
            AskError::Build(error) => write!(f, "{error}"),
            AskError::ToolUseStopped(_) => f.write_str("the answer tried to use a tool, so it was stopped"),
            AskError::Failed(_) => f.write_str("ChatGPT couldn't finish the answer"),
            AskError::TooLarge => f.write_str("the answer was too long"),
            AskError::Status(status) => write!(f, "ChatGPT answered with status {status}"),
            AskError::NoAnswer => f.write_str("ChatGPT gave no answer"),
            AskError::Net(error) => f.write_str(error),
        }
    }
}

impl std::error::Error for AskError {}

/// Who signed in, for showing: untrusted text, cut and cleaned.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Identity {
    pub(crate) email: String,
    pub(crate) plan: String,
}

/// What a runtime holds in memory.
#[derive(Debug, Default)]
struct Memory {
    access: Option<AccessGrant>,
    status: SiwcStatus,
    /// Transient refresh failures in a row.
    failures: u32,
    /// No refresh before this time (seconds since 1970).
    not_before: i64,
}

/// The clock the runtime reads (seconds since 1970); tests move it.
pub(crate) type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// The `chatgpt` runtime's host side. One per app; shared between request threads.
pub(crate) struct ChatGptRuntime {
    store: Arc<dyn SecretStore>,
    endpoints: SiwcEndpoints,
    clock: Clock,
    memory: Mutex<Memory>,
    /// Held for a refresh, a sign-in's write and a sign-out: one at a time.
    refreshing: Mutex<()>,
    /// How long the credential store's reads and its writes are waited for.
    store_wait: (Duration, Duration),
}

impl fmt::Debug for ChatGptRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatGptRuntime").field("status", &self.status()).finish_non_exhaustive()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl ChatGptRuntime {
    /// The runtime for OpenAI's endpoints and the system clock.
    pub(crate) fn new(store: Arc<dyn SecretStore>) -> ChatGptRuntime {
        ChatGptRuntime::with(store, SiwcEndpoints::openai(), Arc::new(mailcraft_time::now))
    }

    /// The runtime for other endpoints and another clock (tests: a loopback fake).
    pub(crate) fn with(store: Arc<dyn SecretStore>, endpoints: SiwcEndpoints, clock: Clock) -> ChatGptRuntime {
        ChatGptRuntime {
            store,
            endpoints,
            clock,
            memory: Mutex::new(Memory::default()),
            refreshing: Mutex::new(()),
            store_wait: (STORE_READ_WAIT, STORE_WRITE_WAIT),
        }
    }

    /// The runtime with other waits for the credential store (tests: a store that never answers).
    #[cfg(test)]
    pub(crate) fn with_store_wait(mut self, read: Duration, write: Duration) -> ChatGptRuntime {
        self.store_wait = (read, write);
        self
    }

    /// Where the sign-in stands.
    pub(crate) fn status(&self) -> SiwcStatus {
        lock(&self.memory).status
    }

    /// The access token held in memory, for the check that refuses a CLI's argv holding it.
    pub(crate) fn held_token(&self) -> Option<String> {
        lock(&self.memory).access.as_ref().map(|g| g.token.expose().to_string())
    }

    /// Sign in on OpenAI's page. Blocks for as long as the owner takes (ten minutes at most);
    /// `cancel` ends it. `host_id` is [`ensure_host_id`]'s. On success the caller notes the entry in
    /// the settings ([`signed_in_settings`]).
    pub(crate) fn sign_in(&self, host_id: &str, open: &dyn Fn(&str), cancel: &AtomicBool) -> Result<Identity, SiwcError> {
        let before = {
            let mut memory = lock(&self.memory);
            let before = memory.status;
            memory.status = SiwcStatus::Waiting;
            before
        };
        let restore = |status: SiwcStatus| lock(&self.memory).status = status;
        // The client OpenAI issued at an earlier sign-in here, if one is kept: a returning
        // sign-in asks as that client. A store that can't be read means a fresh registration.
        let kept = self.read_entry().ok().flatten();
        let issued = kept.as_ref().map(|entry| entry.client_id.expose().to_string());
        let signed = match openai::sign_in(&self.endpoints, host_id, issued.as_deref(), open, cancel, SIGN_IN_PATIENCE) {
            Ok(signed) => signed,
            Err(error) => {
                restore(before);
                return Err(SiwcError::SignIn(error));
            }
        };
        let Some(entry) = StoredSignIn::new(&signed.client_id, &signed.grant.refresh_token, &signed.grant.account_id) else {
            restore(before);
            return Err(SiwcError::SignIn(SignInError::Token(TokenError::Unreadable)));
        };
        let Some(token) = AccessToken::new(&signed.grant.access_token) else {
            restore(before);
            return Err(SiwcError::SignIn(SignInError::Token(TokenError::Unreadable)));
        };
        let _writing = lock(&self.refreshing);
        if let Err(error) = self.write_entry(&entry) {
            restore(before);
            return Err(error);
        }
        let mut memory = lock(&self.memory);
        *memory = Memory {
            access: Some(AccessGrant { token, expires_at: signed.grant.expires_at }),
            status: SiwcStatus::SignedIn,
            failures: 0,
            not_before: 0,
        };
        log::info!("signed in with ChatGPT");
        Ok(Identity { email: shown_identity(&signed.grant.email), plan: shown_identity(&signed.grant.plan) })
    }

    /// An access token good for at least another minute: the one in memory, or a refreshed one.
    pub(crate) fn access_token(&self) -> Result<AccessToken, SiwcError> {
        let now = (self.clock)();
        {
            let memory = lock(&self.memory);
            if memory.status == SiwcStatus::SignInEnded {
                return Err(SiwcError::Ended);
            }
            if let Some(grant) = memory.access.as_ref().filter(|g| g.is_fresh(now)) {
                return Ok(grant.token.clone());
            }
        }
        self.refresh(None)
    }

    /// A new access token after `rejected` was answered with HTTP 401 (once per request:
    /// `mailcraft_ai::runtime::siwc::refresh_after_unauthorized`).
    pub(crate) fn after_unauthorized(&self, rejected: &AccessToken) -> Result<AccessToken, SiwcError> {
        self.refresh(Some(rejected))
    }

    /// One refresh at a time; a caller that waited for another's uses its token.
    fn refresh(&self, rejected: Option<&AccessToken>) -> Result<AccessToken, SiwcError> {
        let _refreshing = lock(&self.refreshing);
        let now = (self.clock)();
        let failures = {
            let memory = lock(&self.memory);
            if memory.status == SiwcStatus::SignInEnded {
                return Err(SiwcError::Ended);
            }
            let renewed = memory.access.as_ref().filter(|g| g.is_fresh(now) && rejected.is_none_or(|r| *r != g.token));
            if let Some(grant) = renewed {
                return Ok(grant.token.clone());
            }
            if now < memory.not_before {
                let wait = Duration::from_secs(u64::try_from(memory.not_before.saturating_sub(now)).unwrap_or(0));
                return Err(SiwcError::Waiting { wait, kind: "backoff" });
            }
            memory.failures
        };
        let stored = self.read_entry()?.ok_or(SiwcError::NotSignedIn)?;
        match openai::refresh(&self.endpoints, stored.client_id.expose(), stored.refresh_token.expose()) {
            Ok(grant) => self.renewed(&stored, &grant, now),
            Err(TokenError::Unusable) => Err(SiwcError::NotSignedIn),
            Err(error) => {
                let failure = match &error {
                    TokenError::Answered { status, code, retry_after } => classify(Some(*status), code, retry_after.as_deref(), failures),
                    TokenError::Unreadable | TokenError::Net(_) | TokenError::Unusable => classify(None, "", None, failures),
                };
                self.failed(failure, &stored, now)
            }
        }
    }

    /// A refresh that worked: the rotated refresh token goes to the entry if it still holds the
    /// one the refresh started with; the access token to memory.
    fn renewed(&self, stored: &StoredSignIn, grant: &openai::Grant, now: i64) -> Result<AccessToken, SiwcError> {
        let account = if grant.account_id.is_empty() { stored.account_id.as_str() } else { grant.account_id.as_str() };
        let rotated = StoredSignIn::new(stored.client_id.expose(), &grant.refresh_token, account);
        let token = AccessToken::new(&grant.access_token);
        let (Some(rotated), Some(token)) = (rotated, token) else {
            return self.failed(classify(None, "", None, lock(&self.memory).failures), stored, now);
        };
        match self.read_entry() {
            Ok(Some(current)) if current.refresh_token == stored.refresh_token => {
                if rotated.refresh_token != stored.refresh_token && self.write_entry(&rotated).is_err() {
                    log::warn!("the renewed ChatGPT sign-in couldn't be kept in the credential store");
                }
            }
            // Signed out, or another sign-in was written, while this refresh was under way: theirs stands.
            Ok(_) => log::info!("the ChatGPT sign-in changed while it was renewed; the newer one is kept"),
            Err(_) => log::warn!("the credential store couldn't be read after renewing the ChatGPT sign-in"),
        }
        let mut memory = lock(&self.memory);
        *memory = Memory {
            access: Some(AccessGrant { token: token.clone(), expires_at: grant.expires_at }),
            status: SiwcStatus::SignedIn,
            failures: 0,
            not_before: 0,
        };
        Ok(token)
    }

    /// A refresh that failed: a terminal failure ends the sign-in (`stored`, the sign-in the refresh
    /// started with, is deleted only if the entry still holds it); a transient one keeps the entry
    /// and sets the backoff.
    fn failed(&self, failure: RefreshFailure, stored: &StoredSignIn, now: i64) -> Result<AccessToken, SiwcError> {
        match failure {
            RefreshFailure::Terminal { code } => {
                match self.read_entry() {
                    Ok(Some(current)) if current.refresh_token != stored.refresh_token => {
                        // Another sign-in was written meanwhile (another MailCraft, a sign-in):
                        // only the refresh token this refresh used was refused; the newer stands.
                        log::info!("a superseded ChatGPT sign-in was refused ({code}); the newer one is kept");
                        lock(&self.memory).access = None;
                        return Err(SiwcError::Waiting { wait: Duration::ZERO, kind: "superseded" });
                    }
                    Ok(Some(_)) => self.remove_entry("the ended ChatGPT sign-in"),
                    Ok(None) => {}
                    Err(_) => log::warn!("the credential store couldn't be read after OpenAI ended the ChatGPT sign-in"),
                }
                log::info!("OpenAI ended the ChatGPT sign-in ({code})");
                let mut memory = lock(&self.memory);
                *memory = Memory { status: SiwcStatus::SignInEnded, ..Memory::default() };
                Err(SiwcError::Ended)
            }
            RefreshFailure::Transient { wait, kind } => {
                log::info!("the ChatGPT sign-in couldn't be renewed just now ({})", kind.name());
                let mut memory = lock(&self.memory);
                memory.failures = memory.failures.saturating_add(1);
                memory.not_before = now.saturating_add(i64::try_from(wait.as_secs()).unwrap_or(i64::MAX));
                Err(SiwcError::Waiting { wait, kind: kind.name() })
            }
        }
    }

    /// Sign out: the entry is deleted, the access token dropped and the runtime off; the caller
    /// applies [`signed_out_settings`] and shows [`sign_out_pointer`]. Nothing is sent anywhere.
    pub(crate) fn sign_out(&self) -> Result<(), SiwcError> {
        let _refreshing = lock(&self.refreshing);
        *lock(&self.memory) = Memory::default();
        let store = self.store.clone();
        match bounded(self.store_wait.1, move || store.remove(&entry_name()).map_err(|e| e.to_string())) {
            Some(Ok(())) => Ok(()),
            Some(Err(error)) => Err(SiwcError::Store(error)),
            None => Err(SiwcError::Store(STORE_SILENT.into())),
        }
    }

    fn remove_entry(&self, what: &str) {
        let store = self.store.clone();
        match bounded(self.store_wait.1, move || store.remove(&entry_name()).map_err(|e| e.to_string())) {
            Some(Ok(())) => {}
            Some(Err(error)) => log::warn!("{what} couldn't be removed from the credential store: {error}"),
            None => log::warn!("{what} couldn't be removed from the credential store: {STORE_SILENT}"),
        }
    }

    /// Ask the ChatGPT plan one question. A 401 refreshes once and asks again.
    pub(crate) fn ask(&self, request: &ResponsesRequest<'_>, cancel: &AtomicBool) -> Result<String, AskError> {
        let body = build_body(request).map_err(AskError::Build)?;
        let mut token = self.access_token().map_err(AskError::Sign)?;
        let mut refreshed = false;
        loop {
            match self.ask_once(&body, &token, cancel)? {
                Asked::Unauthorized if refresh_after_unauthorized(refreshed) => {
                    refreshed = true;
                    token = self.after_unauthorized(&token).map_err(AskError::Sign)?;
                }
                Asked::Unauthorized => return Err(AskError::Status(401)),
                Asked::Answer(text) => return Ok(text),
            }
        }
    }

    fn ask_once(&self, body: &[u8], token: &AccessToken, cancel: &AtomicBool) -> Result<Asked, AskError> {
        let authorization = format!("Bearer {}", token.expose());
        let fields = [("Authorization", authorization.as_str()), ("Content-Type", "application/json"), ("Accept", "text/event-stream")];
        let request = StreamRequest {
            method: "POST",
            url: self.endpoints.responses(),
            fields: &fields,
            body,
            trusted: "",
            deadlines: Deadlines::default(),
            max_response: MAX_RESPONSE,
            max_error_response: MAX_ERROR_RESPONSE,
            cancel: Some(cancel),
        };
        let mut stream = ResponsesStream::new();
        let mut status = 0u16;
        let mut ending: Option<Step> = None;
        let mut deliver = |piece: Piece<'_>| match piece {
            Piece::Head(head) => {
                status = head.status;
                Flow::Go
            }
            Piece::Body(bytes) if status == 200 => {
                if let Some(step) = stream.feed(bytes).into_iter().next() {
                    ending.get_or_insert(step);
                }
                if stream.is_over() { Flow::Stop } else { Flow::Go }
            }
            Piece::Body(_) => Flow::Go,
        };
        let outcome = https::stream(&request, &mut deliver);
        // A stop of our own after the run ended isn't a failure.
        if let (Err(error), None) = (outcome, &ending) {
            return Err(AskError::Net(error.to_string()));
        }
        match status {
            200 => {}
            401 => return Ok(Asked::Unauthorized),
            other => return Err(AskError::Status(other)),
        }
        let ending = ending.or_else(|| stream.finish().into_iter().next());
        match ending {
            Some(Step::Done(text)) => Ok(Asked::Answer(text)),
            Some(Step::Kill(kind)) if matches!(kind, "response_failed" | "response_incomplete" | "stream_error") => {
                log::info!("the ChatGPT answer ended without one ({kind})");
                Err(AskError::Failed(kind))
            }
            Some(Step::Kill(kind)) => {
                log::info!("the ChatGPT answer was stopped: {kind}");
                Err(AskError::ToolUseStopped(kind))
            }
            Some(Step::TooLarge) => Err(AskError::TooLarge),
            Some(Step::Continue | Step::WritePrompt) | None => Err(AskError::NoAnswer),
        }
    }

    /// The entry, read within [`STORE_READ_WAIT`]: a store that doesn't answer is an error.
    fn read_entry(&self) -> Result<Option<StoredSignIn>, SiwcError> {
        let store = self.store.clone();
        match bounded(self.store_wait.0, move || store.read(&entry_name()).map_err(|e| e.to_string())) {
            Some(Ok(Some(text))) => Ok(StoredSignIn::from_entry(&text)),
            Some(Ok(None)) => Ok(None),
            Some(Err(error)) => Err(SiwcError::Store(error)),
            None => Err(SiwcError::Store(STORE_SILENT.into())),
        }
    }

    /// Writes the entry and reads it back: a store that didn't keep it, or didn't answer within
    /// [`STORE_WRITE_WAIT`], is an error.
    fn write_entry(&self, entry: &StoredSignIn) -> Result<(), SiwcError> {
        let (store, text) = (self.store.clone(), entry.to_entry());
        match bounded(self.store_wait.1, move || store.set(&entry_name(), &text).map_err(|e| e.to_string())) {
            Some(Ok(())) => {}
            Some(Err(error)) => return Err(SiwcError::Store(error)),
            None => return Err(SiwcError::Store(STORE_SILENT.into())),
        }
        match self.read_entry()? {
            Some(kept) if &kept == entry => Ok(()),
            _ => Err(SiwcError::Store("the sign-in wasn't kept".into())),
        }
    }
}

/// What one request to the Responses API brought.
enum Asked {
    Answer(String),
    Unauthorized,
}

/// What the window shows after a sign-out (B11): the owner can also remove MailCraft in ChatGPT's settings.
#[cfg_attr(not(test), expect(dead_code, reason = "the window shows `SIGN_OUT_POINTER_TEXT` itself; kept for the sign-out's test"))]
pub(crate) fn sign_out_pointer() -> &'static str {
    SIGN_OUT_POINTER_TEXT
}
