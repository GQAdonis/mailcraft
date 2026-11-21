// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Sign in with ChatGPT (SIWC) for an open-source program: OpenAI's dynamic per-account
//! registration, on the same audited PKCE and loopback code as Microsoft's sign-in.
//!
//! The first authorize request names no client of ours: it says `client_id=dynamic_agent_client`,
//! `agent_name_hint=MailCraft` and the installation's `ext_agent_host_id` (32 random bytes in
//! base64url, made once by [`new_host_id`] and kept by the host; not a secret). The page sends the
//! browser back to exactly `http://127.0.0.1:<port>/auth/callback` (never `localhost`; OpenAI's
//! documented path, and only the port may vary) with a code and the `client_id` OpenAI issued for
//! this owner and workspace. That id, the code, the PKCE verifier, the same redirect and the
//! resource are traded at the token endpoint for an access token (about an hour) and a refresh
//! token (rotated at every refresh). A later sign-in asks as the issued client (no name hint), and
//! a callback naming another client is refused. No client id is built in, read from settings or
//! borrowed. Each attempt sends a fresh `nonce`, and the identity token must carry it back; the
//! granted scopes must include ChatGPT plan use ([`PLAN_SCOPE`]).
//!
//! The loopback listener is bound to 127.0.0.1 before the browser is opened, on port
//! [`PREFERRED_PORT`] when it is free and else on one the system chooses, and takes **one**
//! request to `/auth/callback`: the right `state` (compared in constant
//! time) goes on to the token endpoint, a wrong one ends the sign-in with nothing traded. Anything
//! else that knocks (a favicon, another path) is answered and the wait goes on. It waits at most
//! [`SIGN_IN_PATIENCE`] and stops at the cancel flag; the port is closed when [`sign_in`] returns,
//! however it ends. The page it answers with is ours, static, and fetches nothing.
//!
//! Everything that comes back is hostile: the callback's `code`, `state` and `client_id` are capped
//! and held to a safe set of characters; the token answer is capped, read as JSON or refused; the
//! identity token's email and plan are cut to [`MAX_SHOWN_IDENTITY`] characters without control
//! characters, and its signature isn't checked (it only fills in what is shown). None of the code,
//! the verifier, the state, the issued client id or a token ever reaches an error's text or a log
//! line. The endpoints are constants ([`AUTHORIZE`], [`TOKEN`], [`RESPONSES`]) spoken to over TLS;
//! [`SiwcEndpoints::at`] (tests, a loopback fake) takes `https://` or `http://` to this machine only.
//!
//! OpenAI documents no revocation endpoint: signing out is the host forgetting what it holds.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mailcraft_charset::base64::URL_SAFE_NO_PAD;
use serde_json::Value;

use super::{DONE, FAILED, NOT_OURS, Page, Redirect, Refused, arrived_at, challenge, random, same, usable};
use crate::NetError;
use crate::http::{self, Url};

/// Where the owner signs in.
pub const AUTHORIZE: &str = "https://auth.openai.com/api/accounts/authorize";
/// Where a code or a refresh token is traded for tokens.
pub const TOKEN: &str = "https://auth.openai.com/api/accounts/oauth/token";
/// Where a ChatGPT plan's answers are asked for (the Responses API).
pub const RESPONSES: &str = "https://api.openai.com/v1/responses";
/// What the tokens are for.
pub const SCOPE: &str = "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
/// What the tokens are asked for (`resource`), at the sign-in, the trade and every refresh.
pub const RESOURCE: &str = "https://api.openai.com/v1";
/// The granted scope without which the plan can't be used: a sign-in that lacks it is refused.
pub const PLAN_SCOPE: &str = "chatgpt.tokens.use.direct";
/// The loopback port tried first (OpenAI's example); another is taken when it is in use.
pub const PREFERRED_PORT: u16 = 1455;
/// The client id of the first authorize request: OpenAI registers a client for the owner then.
pub const DYNAMIC_CLIENT_ID: &str = "dynamic_agent_client";
/// The name OpenAI is told the program goes by.
pub const AGENT_NAME_HINT: &str = "MailCraft";
/// The path of the loopback redirect.
pub const CALLBACK_PATH: &str = "/auth/callback";
/// The longest a sign-in waits for the owner.
pub const SIGN_IN_PATIENCE: Duration = Duration::from_secs(10 * 60);
/// The most characters of an email or a plan that are shown.
pub const MAX_SHOWN_IDENTITY: usize = 128;

/// The longest issued client id that is taken.
const MAX_CLIENT_ID: usize = 256;
/// The longest code that is taken.
const MAX_CODE: usize = 4 * 1024;
/// The most names a callback may bring.
const MAX_FIELDS: usize = 16;
/// The longest account id that is kept.
const MAX_ACCOUNT_ID: usize = 128;
/// The longest error code from OpenAI that is kept.
const MAX_ERROR_CODE: usize = 64;
/// The largest token answer that is read.
const MAX_TOKEN_ANSWER: usize = 64 * 1024;
/// How long a token request may take.
const TOKEN_PATIENCE: Duration = Duration::from_secs(20);
/// An access token's lifetime when the answer names none, and the longest that is believed.
const DEFAULT_LIFETIME: i64 = 3_600;
const MAX_LIFETIME: i64 = 24 * 3_600;
/// The length of a host id: 32 bytes in base64url without padding.
const HOST_ID_LEN: usize = 43;
/// The longest granted-scope list that is kept.
const MAX_SCOPE: usize = 512;
/// The identity token's claims OpenAI puts its own facts under.
const AUTH_CLAIM: &str = "https://api.openai.com/auth";

/// Where a ChatGPT sign-in and its answers are had.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SiwcEndpoints {
    authorize: String,
    token: String,
    responses: String,
    /// The loopback port tried first: [`PREFERRED_PORT`] for OpenAI's, none for others (a fake).
    port: Option<u16>,
}

impl SiwcEndpoints {
    /// OpenAI's own endpoints, the only ones the program uses.
    pub fn openai() -> SiwcEndpoints {
        SiwcEndpoints { authorize: AUTHORIZE.into(), token: TOKEN.into(), responses: RESPONSES.into(), port: Some(PREFERRED_PORT) }
    }

    /// Other endpoints (a loopback fake in tests): each must be `https://`, or `http://` to this
    /// machine; anything else is refused before anything is sent.
    pub fn at(authorize: &str, token: &str, responses: &str) -> Result<SiwcEndpoints, NetError> {
        for address in [authorize, token, responses] {
            Url::parse(address)?;
        }
        Ok(SiwcEndpoints { authorize: authorize.trim().into(), token: token.trim().into(), responses: responses.trim().into(), port: None })
    }

    /// The Responses API's address.
    pub fn responses(&self) -> &str {
        &self.responses
    }
}

impl Default for SiwcEndpoints {
    fn default() -> Self {
        SiwcEndpoints::openai()
    }
}

/// What a token request brought. The secrets are hidden from `Debug`.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Grant {
    /// The access token: kept in memory alone.
    pub access_token: String,
    /// The refresh token to use next time: the new one when one came, the one used otherwise.
    pub refresh_token: String,
    /// When the access token runs out (seconds since 1970).
    pub expires_at: i64,
    /// The ChatGPT account the sign-in is for, when the identity token said (letters, digits, `-`, `_`).
    pub account_id: String,
    /// Who signed in, for showing: untrusted, at most [`MAX_SHOWN_IDENTITY`] characters.
    pub email: String,
    /// Their plan, for showing: untrusted, at most [`MAX_SHOWN_IDENTITY`] characters.
    pub plan: String,
    /// The granted scopes, space-separated (letters, digits, `.`, `_`, `-`, `:`), when the answer said.
    pub scope: String,
    /// The `nonce` the identity token carried (empty without one): compared with the one sent.
    pub nonce: String,
}

impl fmt::Debug for Grant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Grant")
            .field("access_token", &format_args!("<{} bytes>", self.access_token.len()))
            .field("refresh_token", &format_args!("<{} bytes>", self.refresh_token.len()))
            .field("expires_at", &self.expires_at)
            .field("account_id", &format_args!("<{} bytes>", self.account_id.len()))
            .finish_non_exhaustive()
    }
}

/// What a finished sign-in brought: the client id OpenAI issued, and the tokens.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SignInGrant {
    /// The client id the callback brought, bound to this owner and workspace.
    pub client_id: String,
    pub grant: Grant,
}

impl fmt::Debug for SignInGrant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SignInGrant").field("client_id", &format_args!("<{} bytes>", self.client_id.len())).field("grant", &self.grant).finish()
    }
}

/// Why the token endpoint gave no tokens. No variant holds a token, a code or a client id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenError {
    /// The endpoint answered: its status, its `error` code (empty when it gave none; letters,
    /// digits and `_` only) and its `Retry-After`, when it gave one.
    Answered { status: u16, code: String, retry_after: Option<String> },
    /// A success that isn't the JSON with tokens it should be.
    Unreadable,
    /// What would be sent can't be (an empty or unusable refresh token or client id): the entry is damaged.
    Unusable,
    /// The endpoint couldn't be reached or the connection broke (the error's text holds no secret).
    Net(NetError),
}

impl fmt::Display for TokenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TokenError::Answered { status, code, .. } if code.is_empty() => write!(f, "OpenAI's sign-in server answered with status {status}"),
            TokenError::Answered { status, code, .. } => write!(f, "OpenAI's sign-in server said \"{code}\" (status {status})"),
            TokenError::Unreadable => f.write_str("OpenAI's sign-in server sent an answer that can't be read"),
            TokenError::Unusable => f.write_str("the ChatGPT sign-in kept on this computer can't be used"),
            TokenError::Net(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for TokenError {}

/// Why a sign-in ended without tokens. No variant holds the code, the state or the issued id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignInError {
    /// The callback's `state` wasn't the one sent: refused, nothing traded, the port closed.
    WrongState,
    /// The callback came without a usable code or client id, or with something said twice.
    BadCallback,
    /// The page came back with a no (its `error` code, letters, digits and `_` only).
    Denied(String),
    /// The owner signed in but didn't allow the plan to be used ([`PLAN_SCOPE`] wasn't granted).
    PlanNotAllowed,
    /// The identity token came back with another `nonce` than this attempt's: refused.
    WrongNonce,
    /// Called off from here.
    Cancelled,
    /// Nothing came back in [`SIGN_IN_PATIENCE`].
    TimedOut,
    /// The code couldn't be traded.
    Token(TokenError),
    /// The sign-in couldn't start (no port, no random numbers, an endpoint or host id that isn't one).
    Setup(NetError),
}

impl fmt::Display for SignInError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SignInError::WrongState => f.write_str("the sign-in came back from somewhere else and was refused"),
            SignInError::BadCallback => f.write_str("the sign-in page came back without what a sign-in needs"),
            SignInError::Denied(code) => write!(f, "the sign-in page said \"{code}\""),
            SignInError::PlanNotAllowed => f.write_str("you signed in, but didn't allow MailCraft to use your ChatGPT plan"),
            SignInError::WrongNonce => f.write_str("the sign-in came back for another attempt and was refused"),
            SignInError::Cancelled => f.write_str("the sign-in was cancelled"),
            SignInError::TimedOut => f.write_str("the sign-in wasn't finished in time"),
            SignInError::Token(error) => write!(f, "{error}"),
            SignInError::Setup(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for SignInError {}

/// A new host id (`ext_agent_host_id`): 32 random bytes in base64url without padding.
pub fn new_host_id() -> Result<String, NetError> {
    random(32)
}

/// Whether `text` is shaped like a host id (43 base64url characters).
pub fn is_host_id(text: &str) -> bool {
    text.len() == HOST_ID_LEN && text.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

/// The redirect for a port: exactly `http://127.0.0.1:<port>/auth/callback`.
pub fn redirect_uri(port: u16) -> String {
    format!("http://127.0.0.1:{port}{CALLBACK_PATH}")
}

/// What one attempt sends that must come back the same: the redirect, the state, the nonce and
/// the PKCE challenge.
pub struct Attempt<'a> {
    pub redirect_uri: &'a str,
    pub state: &'a str,
    pub nonce: &'a str,
    pub code_challenge: &'a str,
}

/// The address of the sign-in page: dynamic registration (`issued` is `None`) or the client
/// OpenAI issued earlier, PKCE S256, the state, the nonce, the scopes and the resource.
pub fn authorize_address(endpoints: &SiwcEndpoints, attempt: &Attempt<'_>, host_id: &str, issued: Option<&str>) -> String {
    let mut fields = vec![
        ("response_type", "code"),
        ("client_id", issued.unwrap_or(DYNAMIC_CLIENT_ID)),
        ("redirect_uri", attempt.redirect_uri),
        ("scope", SCOPE),
        ("resource", RESOURCE),
        ("state", attempt.state),
        ("nonce", attempt.nonce),
        ("code_challenge", attempt.code_challenge),
        ("code_challenge_method", "S256"),
    ];
    // The name hint is for the first registration alone.
    if issued.is_none() {
        fields.push(("agent_name_hint", AGENT_NAME_HINT));
    }
    fields.push(("ext_agent_host_id", host_id));
    let address = endpoints.authorize.trim();
    format!("{address}{}{}", if address.contains('?') { '&' } else { '?' }, http::form(&fields))
}

/// Sign the owner in to their ChatGPT plan on OpenAI's page.
///
/// `host_id` is the installation's kept host id ([`new_host_id`]); `issued` is the client id
/// OpenAI issued at an earlier sign-in on this computer, if one is kept. `open` is given the page's
/// address once the loopback port is listening, and must not wait for the sign-in. Blocks for as
/// long as the owner takes, `patience` and [`SIGN_IN_PATIENCE`] at most; `cancel` ends the wait
/// within a fraction of a second. The port is closed when this returns.
pub fn sign_in(
    endpoints: &SiwcEndpoints,
    host_id: &str,
    issued: Option<&str>,
    open: &dyn Fn(&str),
    cancel: &AtomicBool,
    patience: Duration,
) -> Result<SignInGrant, SignInError> {
    if !is_host_id(host_id) {
        return Err(SignInError::Setup(NetError::Config("the ChatGPT host id isn't one".into())));
    }
    // A kept id that isn't one is not sent: the sign-in registers afresh.
    let issued = issued.filter(|id| client_id_ok(id) && *id != DYNAMIC_CLIENT_ID);
    Url::parse(&endpoints.authorize).map_err(SignInError::Setup)?;
    let token = Url::parse(&endpoints.token).map_err(SignInError::Setup)?;
    if cancel.load(Ordering::Relaxed) {
        return Err(SignInError::Cancelled);
    }
    let verifier = random(32).map_err(SignInError::Setup)?;
    let state = random(32).map_err(SignInError::Setup)?;
    let nonce = random(32).map_err(SignInError::Setup)?;
    // Bound before the browser is sent anywhere, to 127.0.0.1 alone.
    let landing = match endpoints.port {
        Some(port) => Redirect::listen_exact_preferring(port),
        None => Redirect::listen_exact(),
    }
    .map_err(SignInError::Setup)?;
    let redirect = redirect_uri(landing.port);
    log::debug!("waiting on a loopback port for the ChatGPT sign-in to come back");
    let code_challenge = challenge(&verifier);
    let attempt = Attempt { redirect_uri: &redirect, state: &state, nonce: &nonce, code_challenge: &code_challenge };
    open(&authorize_address(endpoints, &attempt, host_id, issued));
    let arrived =
        landing.wait_for(&state, cancel, patience.min(SIGN_IN_PATIENCE), |request: &[u8], state: &str| judge_callback(request, state, issued));
    // One request, then the port is closed whatever it brought.
    drop(landing);
    let (code, client_id) = match arrived {
        Ok(Arrived::Code { code, client_id }) => (code, client_id),
        Ok(Arrived::WrongState) => return Err(SignInError::WrongState),
        Ok(Arrived::Broken) => return Err(SignInError::BadCallback),
        Ok(Arrived::Denied(code)) => return Err(SignInError::Denied(code)),
        Err(refused) => {
            return Err(match refused.error {
                NetError::SignInCancelled => SignInError::Cancelled,
                NetError::SignInTimedOut => SignInError::TimedOut,
                other => SignInError::Setup(other),
            });
        }
    };
    let fields = [
        ("grant_type", "authorization_code"),
        ("client_id", client_id.as_str()),
        ("code", code.as_str()),
        ("code_verifier", verifier.as_str()),
        ("redirect_uri", redirect.as_str()),
        ("resource", RESOURCE),
    ];
    let grant = trade(&token, &fields, "", &[code.as_str(), verifier.as_str(), client_id.as_str()]).map_err(SignInError::Token)?;
    check_grant(&grant, &nonce)?;
    Ok(SignInGrant { client_id, grant })
}

/// A sign-in's tokens are taken only when the plan's scope was granted and the identity token,
/// when one came, carries this attempt's nonce.
pub(super) fn check_grant(grant: &Grant, nonce: &str) -> Result<(), SignInError> {
    if !grant.scope.split_ascii_whitespace().any(|s| s == PLAN_SCOPE) {
        return Err(SignInError::PlanNotAllowed);
    }
    if !grant.nonce.is_empty() && !same(&grant.nonce, nonce) {
        return Err(SignInError::WrongNonce);
    }
    Ok(())
}

/// Trade `refresh_token` for new tokens, as the client OpenAI issued (`client_id`). The refresh
/// token that comes back is the rotated one; the caller keeps it in place of the one it used.
pub fn refresh(endpoints: &SiwcEndpoints, client_id: &str, refresh_token: &str) -> Result<Grant, TokenError> {
    let token = Url::parse(&endpoints.token).map_err(TokenError::Net)?;
    if !client_id_ok(client_id) || refresh_token.is_empty() || !usable(refresh_token) {
        return Err(TokenError::Unusable);
    }
    let fields = [("grant_type", "refresh_token"), ("client_id", client_id), ("refresh_token", refresh_token), ("resource", RESOURCE)];
    trade(&token, &fields, refresh_token, &[refresh_token, client_id])
}

/// What the one request to the callback brought.
enum Arrived {
    Code { code: String, client_id: String },
    WrongState,
    Broken,
    Denied(String),
}

/// The page a request to the loopback port is answered with and, for the one request to the
/// callback, what it brought. Another method or path is answered and waited past.
fn judge_callback(request: &[u8], state: &str, issued: Option<&str>) -> (&'static Page, Option<Result<Arrived, Refused>>) {
    let fields = match arrived_at(request, CALLBACK_PATH) {
        Ok(fields) => fields,
        Err(page) => return (page, None),
    };
    let (page, arrived) = read_callback(&fields, state, issued);
    (page, Some(Ok(arrived)))
}

/// What the callback's names and values say, once it is known to be the one request. A first
/// registration's callback must name the issued client; a later one may leave it out (the kept
/// id is used), and one naming another client is refused.
fn read_callback(fields: &[(String, String)], state: &str, issued: Option<&str>) -> (&'static Page, Arrived) {
    let count = |name: &str| fields.iter().filter(|(n, _)| n == name).count();
    let once = |name: &str| if count(name) == 1 { fields.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str()) } else { None };
    let sent = once("state").unwrap_or("");
    if state.is_empty() || !same(sent, state) {
        return (&NOT_OURS, Arrived::WrongState);
    }
    if fields.len() > MAX_FIELDS || ["code", "client_id", "error"].iter().any(|name| count(name) > 1) {
        return (&FAILED, Arrived::Broken);
    }
    if let Some(error) = once("error") {
        let code = error_code(error);
        log::info!("the ChatGPT sign-in page came back with a no: \"{code}\"");
        return (&FAILED, Arrived::Denied(code));
    }
    let client_id = match (once("client_id"), issued) {
        (Some(named), Some(kept)) if named == kept => kept,
        (Some(_), Some(_)) => return (&FAILED, Arrived::Broken),
        (Some(named), None) if client_id_ok(named) && named != DYNAMIC_CLIENT_ID => named,
        (None, Some(kept)) => kept,
        _ => return (&FAILED, Arrived::Broken),
    };
    match once("code") {
        Some(code) if code_ok(code) => (&DONE, Arrived::Code { code: code.to_string(), client_id: client_id.to_string() }),
        _ => (&FAILED, Arrived::Broken),
    }
}

/// Whether a code is one that can be sent back: of a sane length, and only the characters codes are made of.
fn code_ok(code: &str) -> bool {
    !code.is_empty()
        && code.len() <= MAX_CODE
        && code.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'+' | b'/' | b'='))
}

/// Whether an issued client id can be kept and sent: of a sane length, letters, digits, `-`, `_`, `.` and `:`.
fn client_id_ok(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_CLIENT_ID && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
}

/// An error code fit to keep and show: letters, digits and `_`, cut short.
fn error_code(text: &str) -> String {
    text.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(MAX_ERROR_CODE).collect()
}

/// POST the form to the token endpoint and read the answer. `secrets` never reach an error.
fn trade(token: &Url, fields: &[(&str, &str)], earlier_refresh: &str, secrets: &[&str]) -> Result<Grant, TokenError> {
    let body = http::form(fields);
    let headers = [("Accept", "application/json"), ("Content-Type", "application/x-www-form-urlencoded")];
    let request =
        http::Request { method: "POST", headers: &headers, body: Some(body.as_bytes()), patience: TOKEN_PATIENCE, max_body: MAX_TOKEN_ANSWER };
    let reply = http::request(token, &request).map_err(|e| TokenError::Net(secrets.iter().fold(e, |e, secret| e.without(secret))))?;
    read_grant(reply.status, &reply.body, reply.retry_after.as_deref(), earlier_refresh, mailcraft_time::now())
}

/// The tokens in a token endpoint's answer, or why there are none.
pub(super) fn read_grant(status: u16, body: &[u8], retry_after: Option<&str>, earlier_refresh: &str, now: i64) -> Result<Grant, TokenError> {
    let json = serde_json::from_slice::<Value>(body).ok().filter(Value::is_object);
    let text = |name: &str| json.as_ref().and_then(|json| json.get(name)).and_then(Value::as_str).unwrap_or("");
    let retry_after = retry_after.map(|r| http::tidy(r).chars().take(64).collect::<String>());
    let error = error_code(text("error"));
    if !error.is_empty() || status != 200 {
        return Err(TokenError::Answered { status, code: error, retry_after });
    }
    if json.is_none() {
        return Err(TokenError::Unreadable);
    }
    let kind = text("token_type");
    if !kind.is_empty() && !kind.eq_ignore_ascii_case("bearer") {
        return Err(TokenError::Unreadable);
    }
    let access = text("access_token");
    let refresh = match text("refresh_token") {
        "" => earlier_refresh,
        new => new,
    };
    if access.is_empty() || !usable(access) || refresh.is_empty() || !usable(refresh) {
        return Err(TokenError::Unreadable);
    }
    let lifetime = match json.as_ref().and_then(|json| json.get("expires_in")) {
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().filter(|s| s.is_finite()).map(|s| s as i64)),
        Some(Value::String(s)) => s.trim().parse::<i64>().ok(),
        _ => None,
    };
    let lifetime = lifetime.unwrap_or(DEFAULT_LIFETIME).clamp(0, MAX_LIFETIME);
    let (email, plan, account_id, nonce) = identity(text("id_token"));
    let scope = text("scope");
    let scope = if scope.len() <= MAX_SCOPE && scope.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b' ' | b'.' | b'_' | b'-' | b':')) {
        scope.to_string()
    } else {
        String::new()
    };
    Ok(Grant {
        access_token: access.into(),
        refresh_token: refresh.into(),
        expires_at: now.saturating_add(lifetime),
        account_id,
        email,
        plan,
        scope,
        nonce,
    })
}

/// The email, the plan, the account id and the nonce in an identity token (a JWT whose middle part
/// is JSON). Its signature isn't checked: read as hostile, for showing, for the credential entry
/// and for the nonce comparison.
fn identity(id_token: &str) -> (String, String, String, String) {
    let mut parts = id_token.split('.');
    let payload = match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(_), Some(payload), Some(_), None) if id_token.len() <= MAX_TOKEN_ANSWER => payload,
        _ => return Default::default(),
    };
    let claims = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok().and_then(|json| serde_json::from_slice::<Value>(&json).ok());
    let Some(claims) = claims.filter(Value::is_object) else { return Default::default() };
    let auth = claims.get(AUTH_CLAIM);
    let claim = |name: &str| claims.get(name).and_then(Value::as_str).unwrap_or("");
    let ours = |name: &str| auth.and_then(|a| a.get(name)).and_then(Value::as_str).unwrap_or("");
    let account: String = ours("chatgpt_account_id").to_string();
    let account = if account.len() <= MAX_ACCOUNT_ID && account.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')) {
        account
    } else {
        String::new()
    };
    let nonce = claim("nonce");
    let nonce = if nonce.len() <= MAX_ACCOUNT_ID && nonce.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')) {
        nonce.to_string()
    } else {
        // A nonce that can't be ours is kept as one that can't match.
        "x".to_string()
    };
    (shown_identity(claim("email")), shown_identity(ours("chatgpt_plan_type")), account, nonce)
}

/// Untrusted text for showing: control characters and direction overrides out, one line, at most
/// [`MAX_SHOWN_IDENTITY`] characters.
pub fn shown_identity(text: &str) -> String {
    text.chars().filter(|c| !super::unprintable(*c)).take(MAX_SHOWN_IDENTITY).collect::<String>().trim().to_string()
}

#[cfg(test)]
#[path = "openai_tests.rs"]
mod openai_tests;
