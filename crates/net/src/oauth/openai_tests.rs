// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Sign in with ChatGPT on loopback: a fake token endpoint, and an `open` that plays the owner's
//! browser by calling the redirect back. Nothing here talks to OpenAI.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::*;
use crate::fake::{FakeHttp, json_answer};

const PATIENCE: Duration = Duration::from_secs(30);
const HOST_ID: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const ISSUED: &str = "app_issued-client.42";

/// An identity token with an email, a plan and an account id (unsigned: never checked), and no nonce.
fn id_token(email: &str, plan: &str, account: &str) -> String {
    let claims = serde_json::json!({"email": email, AUTH_CLAIM: {"chatgpt_plan_type": plan, "chatgpt_account_id": account}});
    format!("e30.{}.sig", URL_SAFE_NO_PAD.encode(claims.to_string()))
}

/// The scopes a sign-in that allowed plan use is granted.
const GRANTED: &str = "chatgpt.tokens.use.direct email offline_access openid profile resource.invoke";

fn tokens(expires_in: i64) -> String {
    serde_json::json!({
        "token_type": "Bearer", "expires_in": expires_in, "access_token": "acc.ess-1", "refresh_token": "re.fresh-1",
        "id_token": id_token("owner@example.org", "plus", "acct-1"), "scope": GRANTED,
    })
    .to_string()
}

fn fields(query: &str) -> HashMap<String, String> {
    http::pairs(query).into_iter().collect()
}

fn endpoints(token: &FakeHttp) -> SiwcEndpoints {
    SiwcEndpoints::at("https://auth.example.org/api/accounts/authorize", &token.url("/oauth/token"), &token.url("/v1/responses")).unwrap()
}

/// One request to the callback port, as a browser makes it; the page it was answered with.
fn visit(port: u16, target: &str) -> String {
    let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) else { return String::new() };
    stream.set_read_timeout(Some(PATIENCE)).unwrap();
    let _ = stream.write_all(format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").as_bytes());
    let mut page = Vec::new();
    let _ = stream.read_to_end(&mut page);
    String::from_utf8_lossy(&page).into_owned()
}

#[derive(Default)]
struct Seen {
    url: String,
    pages: Vec<String>,
}

/// An `open` that plays the browser: each of `targets` (`{state}` filled in with the real one)
/// is asked of the redirect's port, in order, on a thread of its own.
fn browser(targets: &[&str]) -> (impl Fn(&str) + use<>, Arc<Mutex<Seen>>) {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let (shared, targets) = (seen.clone(), targets.iter().map(|t| t.to_string()).collect::<Vec<_>>());
    let open = move |url: &str| {
        shared.lock().unwrap().url = url.to_string();
        let asked = fields(url.split_once('?').unwrap().1);
        let port: u16 = asked["redirect_uri"].strip_prefix("http://127.0.0.1:").unwrap().strip_suffix(CALLBACK_PATH).unwrap().parse().unwrap();
        let (shared, targets, state) = (shared.clone(), targets.clone(), asked["state"].clone());
        std::thread::spawn(move || {
            for target in targets {
                let page = visit(port, &target.replace("{state}", &state));
                shared.lock().unwrap().pages.push(page);
            }
        });
    };
    (open, seen)
}

fn port_of(seen: &Arc<Mutex<Seen>>) -> u16 {
    let url = seen.lock().unwrap().url.clone();
    let asked = fields(url.split_once('?').unwrap().1);
    asked["redirect_uri"].strip_prefix("http://127.0.0.1:").unwrap().strip_suffix(CALLBACK_PATH).unwrap().parse().unwrap()
}

fn closed(port: u16) -> bool {
    TcpStream::connect_timeout(&(std::net::Ipv4Addr::LOCALHOST, port).into(), Duration::from_millis(500)).is_err()
}

fn run(token: &FakeHttp, targets: &[&str]) -> (Result<SignInGrant, SignInError>, Arc<Mutex<Seen>>) {
    let (open, seen) = browser(targets);
    let result = sign_in(&endpoints(token), HOST_ID, None, &open, &AtomicBool::new(false), PATIENCE);
    (result, seen)
}

fn run_as(token: &FakeHttp, issued: &str, targets: &[&str]) -> (Result<SignInGrant, SignInError>, Arc<Mutex<Seen>>) {
    let (open, seen) = browser(targets);
    let result = sign_in(&endpoints(token), HOST_ID, Some(issued), &open, &AtomicBool::new(false), PATIENCE);
    (result, seen)
}

#[test]
fn the_authorize_address_asks_for_dynamic_registration_with_pkce() {
    let redirect = redirect_uri(49_152);
    let attempt = Attempt { redirect_uri: &redirect, state: "st-ate", nonce: "no-nce", code_challenge: "chal-lenge" };
    let address = authorize_address(&SiwcEndpoints::openai(), &attempt, HOST_ID, None);
    let (base, query) = address.split_once('?').unwrap();
    assert_eq!(base, "https://auth.openai.com/api/accounts/authorize");
    let asked = fields(query);
    let expected = [
        ("response_type", "code"),
        ("client_id", "dynamic_agent_client"),
        ("redirect_uri", "http://127.0.0.1:49152/auth/callback"),
        ("scope", "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct"),
        ("resource", "https://api.openai.com/v1"),
        ("state", "st-ate"),
        ("nonce", "no-nce"),
        ("code_challenge", "chal-lenge"),
        ("code_challenge_method", "S256"),
        ("agent_name_hint", "MailCraft"),
        ("ext_agent_host_id", HOST_ID),
    ];
    assert_eq!(asked.len(), expected.len(), "nothing more is asked: {asked:?}");
    for (name, value) in expected {
        assert_eq!(asked[name], value, "{name}");
    }
}

#[test]
fn the_endpoints_are_openais_over_tls_and_microsoft_keeps_localhost() {
    let e = SiwcEndpoints::openai();
    assert_eq!((e.authorize.as_str(), e.token.as_str(), e.responses()), (AUTHORIZE, TOKEN, RESPONSES));
    assert!([AUTHORIZE, TOKEN, RESPONSES].iter().all(|a| a.starts_with("https://")));
    assert!(SiwcEndpoints::at("http://auth.example.org/a", TOKEN, RESPONSES).is_err(), "plain HTTP off this machine is refused");
    assert_eq!(super::super::REGISTERED_REDIRECT, "http://localhost");
    assert_eq!(redirect_uri(1), "http://127.0.0.1:1/auth/callback");
    assert_eq!(TOKEN, "https://auth.openai.com/api/accounts/oauth/token");
    assert_eq!((e.port, SiwcEndpoints::at(AUTHORIZE, TOKEN, RESPONSES).unwrap().port), (Some(1455), None), "a fake never takes 1455");
}

#[test]
fn a_returning_sign_in_asks_as_the_issued_client_without_the_name_hint() {
    let redirect = redirect_uri(49_152);
    let attempt = Attempt { redirect_uri: &redirect, state: "st-ate", nonce: "no-nce", code_challenge: "chal-lenge" };
    let asked = fields(authorize_address(&SiwcEndpoints::openai(), &attempt, HOST_ID, Some(ISSUED)).split_once('?').unwrap().1);
    assert_eq!(asked["client_id"], ISSUED);
    assert!(!asked.contains_key("agent_name_hint"));
    assert_eq!(asked["ext_agent_host_id"], HOST_ID);
}

#[test]
fn a_returning_callback_may_leave_the_client_out_and_the_kept_one_is_used() {
    let token = FakeHttp::answering(json_answer("200 OK", &tokens(3_600)));
    let (result, _) = run_as(&token, ISSUED, &[&format!("{CALLBACK_PATH}?code=co.de-1&state={{state}}")]);
    assert_eq!(result.unwrap().client_id, ISSUED);
    let form = fields(token.request(0).split_once("\r\n\r\n").unwrap().1);
    assert_eq!((form["client_id"].as_str(), form["resource"].as_str()), (ISSUED, RESOURCE));
}

#[test]
fn neg_a_returning_callback_naming_another_client_is_refused_and_nothing_is_traded() {
    let token = FakeHttp::answering(json_answer("200 OK", &tokens(3_600)));
    let (result, _) = run_as(&token, ISSUED, &[&format!("{CALLBACK_PATH}?code=co.de-1&state={{state}}&client_id=app_someone-else")]);
    assert_eq!(result.unwrap_err(), SignInError::BadCallback);
    assert_eq!(token.asked(), 0);
}

#[test]
fn neg_a_sign_in_that_didnt_allow_plan_use_is_refused() {
    let body = serde_json::json!({"access_token": "a", "refresh_token": "r", "scope": "openid profile email"}).to_string();
    let token = FakeHttp::answering(json_answer("200 OK", &body));
    let (result, _) = run(&token, &[&format!("{CALLBACK_PATH}?code=co.de-1&state={{state}}&client_id={ISSUED}")]);
    assert_eq!(result.unwrap_err(), SignInError::PlanNotAllowed);
}

#[test]
fn neg_an_identity_token_with_another_nonce_is_refused() {
    let claims = serde_json::json!({"email": "owner@example.org", "nonce": "someone-elses"});
    let id = format!("e30.{}.sig", URL_SAFE_NO_PAD.encode(claims.to_string()));
    let body = serde_json::json!({"access_token": "a", "refresh_token": "r", "scope": GRANTED, "id_token": id}).to_string();
    let token = FakeHttp::answering(json_answer("200 OK", &body));
    let (result, _) = run(&token, &[&format!("{CALLBACK_PATH}?code=co.de-1&state={{state}}&client_id={ISSUED}")]);
    assert_eq!(result.unwrap_err(), SignInError::WrongNonce);
}

#[test]
fn a_host_id_is_32_random_bytes_in_base64url() {
    let (a, b) = (new_host_id().unwrap(), new_host_id().unwrap());
    assert!(is_host_id(&a) && is_host_id(&b) && a != b);
    assert!(!is_host_id("short") && !is_host_id(&"=".repeat(43)));
}

#[test]
fn the_flow_trades_the_code_with_the_issued_client_id_and_the_verifier() {
    let token = FakeHttp::answering(json_answer("200 OK", &tokens(3_600)));
    let target = format!("{CALLBACK_PATH}?code=co.de-1&state={{state}}&client_id={ISSUED}");
    let (result, seen) = run(&token, &["/favicon.ico", &target]);
    let got = result.unwrap();
    assert_eq!(got.client_id, ISSUED);
    assert_eq!((got.grant.access_token.as_str(), got.grant.refresh_token.as_str()), ("acc.ess-1", "re.fresh-1"));
    assert_eq!((got.grant.email.as_str(), got.grant.plan.as_str(), got.grant.account_id.as_str()), ("owner@example.org", "plus", "acct-1"));
    assert_eq!(token.asked(), 1);
    let request = token.request(0);
    assert!(request.starts_with("POST /oauth/token "));
    let form = fields(request.split_once("\r\n\r\n").unwrap().1);
    let asked = fields(seen.lock().unwrap().url.split_once('?').unwrap().1);
    assert_eq!(form["grant_type"], "authorization_code");
    assert_eq!(form["client_id"], ISSUED, "the issued id, not the dynamic one");
    assert_eq!(form["code"], "co.de-1");
    assert_eq!(form["redirect_uri"], asked["redirect_uri"]);
    assert_eq!(form["resource"], RESOURCE);
    assert_eq!(challenge(&form["code_verifier"]), asked["code_challenge"], "the verifier that goes with the challenge");
    assert!(closed(port_of(&seen)), "the port is closed once the sign-in is done");
    let debug = format!("{got:?}");
    assert!(!debug.contains("acc.ess-1") && !debug.contains("re.fresh-1") && !debug.contains(ISSUED));
}

#[test]
fn neg_a_wrong_state_is_refused_nothing_is_traded_and_the_port_closes() {
    let token = FakeHttp::answering(json_answer("200 OK", &tokens(3_600)));
    let target = format!("{CALLBACK_PATH}?code=co.de-1&state=not-the-one&client_id={ISSUED}");
    let (result, seen) = run(&token, &[&target, &format!("{CALLBACK_PATH}?code=co.de-1&state={{state}}&client_id={ISSUED}")]);
    assert_eq!(result.unwrap_err(), SignInError::WrongState);
    assert_eq!(token.asked(), 0, "no token request");
    assert!(closed(port_of(&seen)), "one request, then the port is closed");
}

#[test]
fn the_listener_binds_127_0_0_1_only() {
    let token = FakeHttp::answering(json_answer("200 OK", &tokens(3_600)));
    let cancel = Arc::new(AtomicBool::new(false));
    let port = Arc::new(Mutex::new(0u16));
    let (p, c) = (port.clone(), cancel.clone());
    let open = move |url: &str| {
        let asked = fields(url.split_once('?').unwrap().1);
        *p.lock().unwrap() = asked["redirect_uri"].strip_prefix("http://127.0.0.1:").unwrap().strip_suffix(CALLBACK_PATH).unwrap().parse().unwrap();
        let (p, c) = (p.clone(), c.clone());
        std::thread::spawn(move || {
            let port = *p.lock().unwrap();
            // Not on ::1, whatever this machine has.
            assert!(TcpStream::connect_timeout(&(std::net::Ipv6Addr::LOCALHOST, port).into(), Duration::from_millis(300)).is_err());
            c.store(true, Ordering::SeqCst);
        });
    };
    assert_eq!(sign_in(&endpoints(&token), HOST_ID, None, &open, &cancel, PATIENCE).unwrap_err(), SignInError::Cancelled);
    assert!(closed(*port.lock().unwrap()), "cancel closes the port");
    assert_eq!(token.asked(), 0);
}

#[test]
fn a_sign_in_nobody_finishes_times_out() {
    let token = FakeHttp::answering(json_answer("200 OK", &tokens(3_600)));
    let started = Instant::now();
    let result = sign_in(&endpoints(&token), HOST_ID, None, &|_| {}, &AtomicBool::new(false), Duration::from_millis(200));
    assert_eq!(result.unwrap_err(), SignInError::TimedOut);
    assert!(started.elapsed() < PATIENCE);
}

#[test]
fn hostile_callbacks_are_refused_without_a_token_request() {
    let cases = [
        format!("{CALLBACK_PATH}?code=co.de&state={{state}}&client_id=bad%20id"),
        format!("{CALLBACK_PATH}?code=co%0Ade&state={{state}}&client_id={ISSUED}"),
        format!("{CALLBACK_PATH}?code=a&code=b&state={{state}}&client_id={ISSUED}"),
        format!("{CALLBACK_PATH}?code=co.de&state={{state}}&client_id={}", "x".repeat(300)),
        format!("{CALLBACK_PATH}?state={{state}}&client_id={ISSUED}"),
    ];
    for case in cases {
        let token = FakeHttp::answering(json_answer("200 OK", &tokens(3_600)));
        let (result, _) = run(&token, &[&case]);
        assert_eq!(result.unwrap_err(), SignInError::BadCallback, "{case}");
        assert_eq!(token.asked(), 0, "{case}");
    }
    let token = FakeHttp::answering(json_answer("200 OK", &tokens(3_600)));
    let (result, _) = run(&token, &[&format!("{CALLBACK_PATH}?error=access_denied%0D%0Ax&state={{state}}")]);
    assert_eq!(result.unwrap_err(), SignInError::Denied("access_deniedx".into()));
}

#[test]
fn a_bad_host_id_or_endpoint_starts_nothing() {
    let token = FakeHttp::answering(json_answer("200 OK", &tokens(3_600)));
    let opened = AtomicBool::new(false);
    let open = |_: &str| opened.store(true, Ordering::SeqCst);
    assert!(matches!(sign_in(&endpoints(&token), "nope", None, &open, &AtomicBool::new(false), PATIENCE), Err(SignInError::Setup(_))));
    assert!(!opened.load(Ordering::SeqCst));
}

#[test]
fn refresh_sends_the_issued_client_id_and_takes_the_rotated_token() {
    let token = FakeHttp::answering(json_answer("200 OK", r#"{"access_token":"acc.ess-2","refresh_token":"re.fresh-2","expires_in":60}"#));
    let grant = refresh(&endpoints(&token), ISSUED, "re.fresh-1").unwrap();
    assert_eq!((grant.access_token.as_str(), grant.refresh_token.as_str()), ("acc.ess-2", "re.fresh-2"));
    let form = fields(token.request(0).split_once("\r\n\r\n").unwrap().1);
    assert_eq!((form["grant_type"].as_str(), form["client_id"].as_str(), form["refresh_token"].as_str()), ("refresh_token", ISSUED, "re.fresh-1"));
    // No new refresh token: the one used stays.
    let token = FakeHttp::answering(json_answer("200 OK", r#"{"access_token":"acc.ess-3"}"#));
    assert_eq!(refresh(&endpoints(&token), ISSUED, "re.fresh-1").unwrap().refresh_token, "re.fresh-1");
    assert_eq!(refresh(&endpoints(&token), ISSUED, "").unwrap_err(), TokenError::Unusable);
    assert_eq!(refresh(&endpoints(&token), "bad id", "re.fresh-1").unwrap_err(), TokenError::Unusable);
}

#[test]
fn token_refusals_carry_the_code_status_and_retry_after_and_never_a_secret() {
    let refused = read_grant(400, br#"{"error":"invalid_grant","error_description":"re.fresh-1 is gone"}"#, None, "re.fresh-1", 0).unwrap_err();
    assert_eq!(refused, TokenError::Answered { status: 400, code: "invalid_grant".into(), retry_after: None });
    assert!(!refused.to_string().contains("re.fresh-1"));
    let busy = read_grant(429, b"slow down", Some("30"), "", 0).unwrap_err();
    assert_eq!(busy, TokenError::Answered { status: 429, code: String::new(), retry_after: Some("30".into()) });
    assert_eq!(read_grant(200, b"<html>", None, "", 0).unwrap_err(), TokenError::Unreadable);
    assert_eq!(read_grant(200, br#"{"access_token":"a b\u0001"}"#, None, "r", 0).unwrap_err(), TokenError::Unreadable);
    assert_eq!(read_grant(200, br#"{"access_token":"a","token_type":"mac"}"#, None, "r", 0).unwrap_err(), TokenError::Unreadable);
    let long = read_grant(200, br#"{"access_token":"a","expires_in":1e300}"#, None, "r", 100).unwrap();
    assert_eq!(long.expires_at, 100 + MAX_LIFETIME);
}

#[test]
fn the_identity_is_cut_and_cleaned_and_a_bad_account_id_is_dropped() {
    let long = format!("{}\u{202E}\u{7}", "a".repeat(400));
    let body = serde_json::json!({"access_token": "a", "refresh_token": "r", "id_token": id_token(&long, "pro\nplus", "acct/../x")}).to_string();
    let grant = read_grant(200, body.as_bytes(), None, "", 0).unwrap();
    assert_eq!(grant.email.chars().count(), MAX_SHOWN_IDENTITY);
    assert_eq!(grant.plan, "proplus");
    assert_eq!(grant.account_id, "");
    let junk = serde_json::json!({"access_token": "a", "refresh_token": "r", "id_token": "not.a-jwt%%.x.y"}).to_string();
    assert_eq!(read_grant(200, junk.as_bytes(), None, "", 0).unwrap().email, "");
}
