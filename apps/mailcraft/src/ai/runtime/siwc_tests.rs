// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tests and fixtures for the W7 adapters (siwc_tests): SCN-RT-SIWC-001, Sign in with ChatGPT by
//! dynamic registration against a fake OAuth server on loopback and a fake credential store.
//! Nothing here talks to OpenAI or signs in to a real account.

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mailcraft_ai::runtime::responses::ResponsesRequest;
use mailcraft_ai::runtime::siwc::{SiwcStatus, StoredSignIn};
use mailcraft_charset::base64::URL_SAFE_NO_PAD;
use mailcraft_host::{SecretError, SecretStore};
use mailcraft_model::ai::{ChainEntry, RuntimeId};
use mailcraft_net::oauth::openai::{CALLBACK_PATH, SiwcEndpoints};

use super::siwc::*;
use crate::ai::settings::AiSettings;

pub(crate) const ISSUED: &str = "app_issued-client.42";
pub(crate) const OWNER: &str = "owner@example.org";
const PATIENCE: Duration = Duration::from_secs(30);

// ---- The fake credential store ---------------------------------------------------------------

/// The system's credential store, in memory.
#[derive(Default)]
pub(crate) struct FakeStore {
    pub(crate) entries: Mutex<BTreeMap<String, String>>,
    pub(crate) writes: AtomicUsize,
}

impl SecretStore for FakeStore {
    fn get(&self, key: &str) -> Option<String> {
        self.entries.lock().unwrap().get(key).cloned()
    }
    fn set(&self, key: &str, secret: &str) -> Result<(), SecretError> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.entries.lock().unwrap().insert(key.into(), secret.into());
        Ok(())
    }
    fn delete(&self, key: &str) {
        self.entries.lock().unwrap().remove(key);
    }
}

impl FakeStore {
    pub(crate) fn entry(&self) -> Option<StoredSignIn> {
        self.get("ai.chatgpt").and_then(|text| StoredSignIn::from_entry(&text))
    }
}

// ---- The fake OAuth and Responses server -----------------------------------------------------

/// One request the fake server took: its path, headers and body.
#[derive(Clone, Debug)]
pub(crate) struct Taken {
    pub(crate) path: String,
    pub(crate) head: String,
    pub(crate) body: String,
}

impl Taken {
    pub(crate) fn form(&self) -> HashMap<String, String> {
        pairs(&self.body)
    }
}

/// What the fake answers with: a status line, a content type and a body.
pub(crate) type Answer = (&'static str, &'static str, String, Vec<(&'static str, String)>);

type Script = dyn Fn(usize, &Taken) -> Answer + Send + Sync;

/// OpenAI's token endpoint and Responses API, played on 127.0.0.1.
pub(crate) struct FakeOpenAi {
    pub(crate) port: u16,
    pub(crate) taken: Arc<Mutex<Vec<Taken>>>,
    stop: Arc<AtomicBool>,
}

impl FakeOpenAi {
    pub(crate) fn start(script: impl Fn(usize, &Taken) -> Answer + Send + Sync + 'static) -> FakeOpenAi {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let taken = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let script: Arc<Script> = Arc::new(script);
        let (kept, halt) = (taken.clone(), stop.clone());
        let counter = Arc::new(AtomicUsize::new(0));
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                if halt.load(Ordering::SeqCst) {
                    return;
                }
                let (script, kept, counter) = (script.clone(), kept.clone(), counter.clone());
                // Each request on a thread of its own, so that two at once overlap.
                std::thread::spawn(move || serve(stream, &*script, &kept, &counter));
            }
        });
        FakeOpenAi { port, taken, stop }
    }

    pub(crate) fn endpoints(&self) -> SiwcEndpoints {
        let base = format!("http://127.0.0.1:{}", self.port);
        SiwcEndpoints::at("https://auth.example.org/api/accounts/authorize", &format!("{base}/oauth/token"), &format!("{base}/v1/responses")).unwrap()
    }

    pub(crate) fn asked(&self, path: &str) -> Vec<Taken> {
        self.taken.lock().unwrap().iter().filter(|t| t.path == path).cloned().collect()
    }

    pub(crate) fn total(&self) -> usize {
        self.taken.lock().unwrap().len()
    }
}

impl Drop for FakeOpenAi {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

fn serve(stream: TcpStream, script: &Script, kept: &Mutex<Vec<Taken>>, counter: &AtomicUsize) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let (mut head, mut length) = (String::new(), 0usize);
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = value.trim().parse().unwrap_or(0);
        }
        head.push_str(&line);
        if line == "\r\n" {
            break;
        }
    }
    let mut body = vec![0u8; length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    let path = head.split_whitespace().nth(1).unwrap_or("").to_string();
    let taken = Taken { path, head, body: String::from_utf8_lossy(&body).into_owned() };
    let n = counter.fetch_add(1, Ordering::SeqCst);
    kept.lock().unwrap().push(taken.clone());
    let (status, kind, body, extra) = script(n, &taken);
    let extra: String = extra.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect();
    let answer = format!("HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n{body}", body.len());
    let mut stream = stream;
    let _ = stream.write_all(answer.as_bytes());
}

pub(crate) fn json(status: &'static str, body: &str) -> Answer {
    (status, "application/json", body.to_string(), Vec::new())
}

/// An identity token (unsigned: never checked).
pub(crate) fn id_token() -> String {
    let claims = serde_json::json!({"email": OWNER, "https://api.openai.com/auth": {"chatgpt_plan_type": "plus", "chatgpt_account_id": "acct-1"}});
    format!("e30.{}.sig", URL_SAFE_NO_PAD.encode(claims.to_string()))
}

/// A token answer: access token `access`, refresh token `refresh`, good for `expires_in` seconds.
pub(crate) fn tokens(access: &str, refresh: &str, expires_in: i64) -> Answer {
    json("200 OK", &serde_json::json!({"token_type": "Bearer", "access_token": access, "refresh_token": refresh, "expires_in": expires_in, "id_token": id_token(), "scope": "chatgpt.tokens.use.direct email offline_access openid profile resource.invoke"}).to_string())
}

pub(crate) fn pairs(query: &str) -> HashMap<String, String> {
    query.split('&').filter_map(|p| p.split_once('=')).map(|(k, v)| (decode(k), decode(v))).collect()
}

fn decode(text: &str) -> String {
    let bytes = text.replace('+', " ").into_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap();
            out.push(u8::from_str_radix(hex, 16).unwrap());
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap()
}

// ---- The owner's browser ---------------------------------------------------------------------

/// What the "browser" was sent to.
#[derive(Default)]
pub(crate) struct Seen {
    pub(crate) url: String,
}

impl Seen {
    pub(crate) fn asked(&self) -> HashMap<String, String> {
        pairs(self.url.split_once('?').unwrap().1)
    }
    pub(crate) fn port(&self) -> u16 {
        self.asked()["redirect_uri"].strip_prefix("http://127.0.0.1:").unwrap().strip_suffix(CALLBACK_PATH).unwrap().parse().unwrap()
    }
}

fn visit(port: u16, target: &str) {
    if let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) {
        stream.set_read_timeout(Some(PATIENCE)).unwrap();
        let _ = stream.write_all(format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").as_bytes());
        let _ = stream.read_to_end(&mut Vec::new());
    }
}

/// An `open` that plays the browser coming back with `query` (`{state}` is the real state).
pub(crate) fn browser(query: &str) -> (impl Fn(&str) + Send + Sync + use<>, Arc<Mutex<Seen>>) {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let (shared, query) = (seen.clone(), query.to_string());
    let open = move |url: &str| {
        shared.lock().unwrap().url = url.to_string();
        let (port, state) = {
            let seen = shared.lock().unwrap();
            (seen.port(), seen.asked()["state"].clone())
        };
        let target = format!("{CALLBACK_PATH}?{}", query.replace("{state}", &state));
        std::thread::spawn(move || visit(port, &target));
    };
    (open, seen)
}

pub(crate) fn closed(port: u16) -> bool {
    TcpStream::connect_timeout(&(std::net::Ipv4Addr::LOCALHOST, port).into(), Duration::from_millis(500)).is_err()
}

// ---- Putting it together ---------------------------------------------------------------------

/// The real clock, moved by `offset` seconds.
pub(crate) fn clock(offset: &Arc<AtomicI64>) -> Clock {
    let offset = offset.clone();
    Arc::new(move || mailcraft_time::now() + offset.load(Ordering::SeqCst))
}

pub(crate) fn settings_with_chatgpt() -> AiSettings {
    let mut settings = AiSettings::default();
    settings.chain = settings.chain.with_entry(ChainEntry::new(RuntimeId::ChatGpt).with_enabled(true)).unwrap();
    settings.set_runtime_consent(OWNER, &RuntimeId::ChatGpt, true).unwrap();
    settings
}

/// A runtime signed in through the fake, whose access token is good for `expires_in` seconds.
pub(crate) fn signed_in(server: &FakeOpenAi, store: &Arc<FakeStore>, offset: &Arc<AtomicI64>, settings: &mut AiSettings) -> Arc<ChatGptRuntime> {
    let runtime = Arc::new(ChatGptRuntime::with(store.clone(), server.endpoints(), clock(offset)));
    let host_id = ensure_host_id(settings).unwrap();
    let (open, _) = browser(&format!("code=co.de-1&state={{state}}&client_id={ISSUED}"));
    runtime.sign_in(&host_id, &open, &AtomicBool::new(false)).unwrap();
    signed_in_settings(settings);
    runtime
}

#[test]
fn a_second_sign_in_asks_as_the_client_openai_issued_at_the_first() {
    let server = FakeOpenAi::start(|_, _| tokens("acc.ess-1", "re.fresh-1", 3_600));
    let store = Arc::new(FakeStore::default());
    let mut settings = settings_with_chatgpt();
    let runtime = signed_in(&server, &store, &Arc::default(), &mut settings);
    let host_id = ensure_host_id(&mut settings).unwrap();
    // A returning callback may leave the client out: the kept one is used.
    let (open, seen) = browser("code=co.de-2&state={state}");
    runtime.sign_in(&host_id, &open, &AtomicBool::new(false)).unwrap();
    let asked = seen.lock().unwrap().asked();
    assert_eq!(asked["client_id"], ISSUED, "not dynamic_agent_client");
    assert!(!asked.contains_key("agent_name_hint"));
    let traded = server.asked("/oauth/token");
    assert_eq!(traded.last().unwrap().form()["client_id"], ISSUED);
}

#[test]
fn correct_flow_stores_secrets_only_in_the_credential_store() {
    let server = FakeOpenAi::start(|_, _| tokens("acc.ess-1", "re.fresh-1", 3_600));
    let store = Arc::new(FakeStore::default());
    let mut settings = settings_with_chatgpt();
    let runtime = ChatGptRuntime::with(store.clone(), server.endpoints(), clock(&Arc::default()));
    let host_id = ensure_host_id(&mut settings).unwrap();
    assert_eq!(ensure_host_id(&mut settings).unwrap(), host_id, "made once and kept");
    let (open, seen) = browser(&format!("code=co.de-1&state={{state}}&client_id={ISSUED}"));

    let identity = runtime.sign_in(&host_id, &open, &AtomicBool::new(false)).unwrap();
    signed_in_settings(&mut settings);

    // The authorize URL: dynamic registration, the persisted host id, PKCE S256, the exact redirect.
    let seen = seen.lock().unwrap();
    let asked = seen.asked();
    assert_eq!(asked["client_id"], "dynamic_agent_client");
    assert_eq!(asked["agent_name_hint"], "MailCraft");
    assert_eq!(asked["ext_agent_host_id"], host_id);
    assert_eq!(asked["code_challenge_method"], "S256");
    assert_eq!(asked["scope"], "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct");
    assert_eq!(asked["redirect_uri"], format!("http://127.0.0.1:{}/auth/callback", seen.port()));
    assert!(closed(seen.port()), "the listener closes after one request");
    // The token request: the issued client id and the verifier.
    let traded = server.asked("/oauth/token");
    assert_eq!(traded.len(), 1);
    let form = traded[0].form();
    assert_eq!((form["grant_type"].as_str(), form["client_id"].as_str(), form["code"].as_str()), ("authorization_code", ISSUED, "co.de-1"));
    assert_eq!(form["code_verifier"].len(), 43);
    // The credential store holds the issued client id, the refresh token and the account id.
    let entry = store.entry().unwrap();
    assert_eq!((entry.client_id.expose(), entry.refresh_token.expose(), entry.account_id.as_str()), (ISSUED, "re.fresh-1", "acct-1"));
    assert!(!store.get("ai.chatgpt").unwrap().contains("acc.ess-1"), "the access token is never stored");
    // The access token is in memory: no further request.
    assert_eq!(runtime.access_token().unwrap().expose(), "acc.ess-1");
    assert_eq!(server.total(), 1);
    assert_eq!(runtime.status(), SiwcStatus::SignedIn);
    // ai.json: the host id, the flag, and no secret.
    let file = settings.to_json().to_string();
    assert!(file.contains(&host_id));
    for secret in [ISSUED, "re.fresh-1", "acc.ess-1", "co.de-1", "acct-1"] {
        assert!(!file.contains(secret), "ai.json holds {secret}");
    }
    assert_eq!(identity, Identity { email: OWNER.into(), plan: "plus".into() });
    let debug = format!("{runtime:?} {entry:?}");
    assert!(!debug.contains("acc.ess-1") && !debug.contains("re.fresh-1") && !debug.contains(ISSUED));
}

#[test]
fn neg_wrong_state_is_refused_and_nothing_is_stored() {
    let server = FakeOpenAi::start(|_, _| tokens("acc.ess-1", "re.fresh-1", 3_600));
    let store = Arc::new(FakeStore::default());
    let mut settings = settings_with_chatgpt();
    let runtime = ChatGptRuntime::with(store.clone(), server.endpoints(), clock(&Arc::default()));
    let host_id = ensure_host_id(&mut settings).unwrap();
    let (open, seen) = browser(&format!("code=co.de-1&state=forged-state&client_id={ISSUED}"));

    let error = runtime.sign_in(&host_id, &open, &AtomicBool::new(false)).unwrap_err();

    assert!(matches!(error, SiwcError::SignIn(mailcraft_net::oauth::openai::SignInError::WrongState)), "{error:?}");
    assert_eq!(server.total(), 0, "no token request");
    assert!(store.entries.lock().unwrap().is_empty() && store.writes.load(Ordering::SeqCst) == 0, "the credential store is unchanged");
    assert!(closed(seen.lock().unwrap().port()), "the listener closed");
    assert_eq!(runtime.status(), SiwcStatus::SignedOut);
    assert!(runtime.access_token().is_err());
    assert!(!settings.chatgpt_key);
}

#[test]
fn cancel_closes_the_listener_and_stores_nothing() {
    let server = FakeOpenAi::start(|_, _| tokens("acc.ess-1", "re.fresh-1", 3_600));
    let store = Arc::new(FakeStore::default());
    let runtime = ChatGptRuntime::with(store.clone(), server.endpoints(), clock(&Arc::default()));
    let host_id = ensure_host_id(&mut AiSettings::default()).unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let port = Arc::new(Mutex::new(0u16));
    let (p, c) = (port.clone(), cancel.clone());
    let open = move |url: &str| {
        *p.lock().unwrap() = Seen { url: url.into() }.port();
        let c = c.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            c.store(true, Ordering::SeqCst);
        });
    };
    let error = runtime.sign_in(&host_id, &open, &cancel).unwrap_err();
    assert_eq!(error, SiwcError::SignIn(mailcraft_net::oauth::openai::SignInError::Cancelled));
    assert!(closed(*port.lock().unwrap()), "the port no longer accepts connections");
    assert!(store.entries.lock().unwrap().is_empty());
    assert_eq!(server.total(), 0);
}

// ---- Answers --------------------------------------------------------------------------------

pub(crate) fn sse(events: &[serde_json::Value]) -> Answer {
    let body = events.iter().map(|e| format!("event: {}\ndata: {e}\n\n", e["type"].as_str().unwrap_or("x"))).collect();
    ("200 OK", "text/event-stream", body, Vec::new())
}

pub(crate) fn question() -> ResponsesRequest<'static> {
    ResponsesRequest { model: "gpt-5", instructions: "Answer from the messages.", data: "When is the review?\nFrom: a@example.org\nTomorrow." }
}

fn ask_with(script: impl Fn(usize, &Taken) -> Answer + Send + Sync + 'static) -> (Result<String, AskError>, FakeOpenAi) {
    let server =
        FakeOpenAi::start(move |n, taken| if taken.path == "/oauth/token" { tokens("acc.ess-1", "re.fresh-1", 3_600) } else { script(n, taken) });
    let store = Arc::new(FakeStore::default());
    let runtime = signed_in(&server, &store, &Arc::default(), &mut settings_with_chatgpt());
    let result = runtime.ask(&question(), &AtomicBool::new(false));
    (result, server)
}

#[test]
fn a_text_only_stream_answers_and_the_request_has_only_allowed_fields() {
    let (result, server) = ask_with(|_, _| {
        sse(&[
            serde_json::json!({"type": "response.created"}),
            serde_json::json!({"type": "response.output_text.delta", "delta": "Tomorrow."}),
            serde_json::json!({"type": "response.completed", "response": {"output": [{"type": "message", "content": [{"type": "output_text", "text": "Tomorrow."}]}]}}),
        ])
    });
    assert_eq!(result.unwrap(), "Tomorrow.");
    let asked = server.asked("/v1/responses");
    assert_eq!(asked.len(), 1);
    assert!(asked[0].head.contains("Authorization: Bearer acc.ess-1"));
    let body: serde_json::Value = serde_json::from_str(&asked[0].body).unwrap();
    assert_eq!((body["store"].clone(), body["stream"].clone()), (serde_json::json!(false), serde_json::json!(true)));
    for refused in ["tools", "temperature", "max_output_tokens", "metadata", "user"] {
        assert!(body.get(refused).is_none(), "{refused}");
    }
}

#[test]
fn a_function_call_in_the_stream_stops_the_run_with_no_answer() {
    let (result, _) = ask_with(|_, _| {
        sse(&[
            serde_json::json!({"type": "response.output_text.delta", "delta": "Sending it now"}),
            serde_json::json!({"type": "response.output_item.added", "item": {"type": "function_call", "name": "send_mail"}}),
        ])
    });
    assert_eq!(result.unwrap_err(), AskError::ToolUseStopped("tool_use"));
}

#[test]
fn a_401_refreshes_once_and_asks_again() {
    let (result, server) = ask_with(|n, _| {
        if n == 1 {
            json("401 Unauthorized", "{}")
        } else {
            sse(&[
                serde_json::json!({"type": "response.completed", "response": {"output": [{"type": "message", "content": [{"type": "output_text", "text": "Yes."}]}]}}),
            ])
        }
    });
    assert_eq!(result.unwrap(), "Yes.");
    assert_eq!(server.asked("/oauth/token").len(), 2, "the sign-in and one refresh");
    assert_eq!(server.asked("/v1/responses").len(), 2);
}

#[test]
fn a_new_runtime_starts_signed_out_and_asks_nothing_without_an_entry() {
    let runtime = ChatGptRuntime::new(Arc::new(FakeStore::default()));
    assert_eq!(runtime.status(), SiwcStatus::SignedOut);
    assert_eq!(runtime.access_token().unwrap_err(), SiwcError::NotSignedIn);
    assert_eq!(entry_name(), "ai.chatgpt");
}
