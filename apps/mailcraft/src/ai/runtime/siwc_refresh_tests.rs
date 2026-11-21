// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tests and fixtures for the W7 adapters (siwc_refresh_tests): SCN-RT-SIWC-002, the ChatGPT
//! sign-in's refresh at expiry − 60 s, coalesced and compare-and-set; terminal grant errors end
//! it; transient ones keep it; sign-out is local. Fake token endpoint on loopback, fake store.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Barrier};
use std::time::Duration;

use mailcraft_ai::runtime::siwc::{FIRST_BACKOFF, SIGN_IN_ENDED_TEXT, SIGN_OUT_POINTER_TEXT, SiwcStatus, StoredSignIn};
use mailcraft_host::SecretStore as _;
use mailcraft_model::ai::RuntimeId;

use super::siwc::*;
use super::siwc_tests::{FakeOpenAi, FakeStore, ISSUED, OWNER, json, settings_with_chatgpt, signed_in, tokens};

/// A fake whose sign-in brings an access token good for `first_expiry` seconds and whose refreshes
/// answer with `refresh` (`n` counts refreshes from 0).
fn fake(first_expiry: i64, refresh: impl Fn(usize) -> super::siwc_tests::Answer + Send + Sync + 'static) -> FakeOpenAi {
    let refreshes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    FakeOpenAi::start(move |_, taken| {
        if taken.form().get("grant_type").map(String::as_str) == Some("authorization_code") {
            tokens("acc.ess-1", "re.fresh-1", first_expiry)
        } else {
            refresh(refreshes.fetch_add(1, Ordering::SeqCst))
        }
    })
}

fn refreshes(server: &FakeOpenAi) -> usize {
    server.asked("/oauth/token").iter().filter(|t| t.form().get("grant_type").map(String::as_str) == Some("refresh_token")).count()
}

#[test]
fn refresh_at_expiry_and_coalesced_refreshes() {
    let server = fake(59, |_| {
        // Slow enough that the second caller arrives while the first refresh is under way.
        std::thread::sleep(Duration::from_millis(300));
        tokens("acc.ess-2", "re.fresh-2", 3_600)
    });
    let store = Arc::new(FakeStore::default());
    let runtime = signed_in(&server, &store, &Arc::default(), &mut settings_with_chatgpt());
    let barrier = Arc::new(Barrier::new(2));
    let callers: Vec<_> = (0..2)
        .map(|_| {
            let (runtime, barrier) = (runtime.clone(), barrier.clone());
            std::thread::spawn(move || {
                barrier.wait();
                runtime.access_token().map(|t| t.expose().to_string())
            })
        })
        .collect();
    let tokens: Vec<String> = callers.into_iter().map(|c| c.join().unwrap().unwrap()).collect();

    assert_eq!(refreshes(&server), 1, "exactly one refresh reaches the fake server");
    assert_eq!(tokens, vec!["acc.ess-2".to_string(), "acc.ess-2".to_string()], "both use the new access token");
    let entry = store.entry().unwrap();
    assert_eq!((entry.client_id.expose(), entry.refresh_token.expose()), (ISSUED, "re.fresh-2"), "the rotated refresh token is stored");
    let sent = server.asked("/oauth/token").into_iter().find(|t| t.form()["grant_type"] == "refresh_token").unwrap().form();
    assert_eq!((sent["client_id"].as_str(), sent["refresh_token"].as_str()), (ISSUED, "re.fresh-1"));
    // Good for an hour now: no refresh.
    runtime.access_token().unwrap();
    assert_eq!(refreshes(&server), 1);
}

#[test]
fn sixty_seconds_left_needs_no_refresh_and_a_restart_refreshes_first() {
    let server = fake(60, |_| tokens("acc.ess-2", "re.fresh-2", 3_600));
    let store = Arc::new(FakeStore::default());
    let runtime = signed_in(&server, &store, &Arc::default(), &mut settings_with_chatgpt());
    assert_eq!(runtime.access_token().unwrap().expose(), "acc.ess-1");
    assert_eq!(refreshes(&server), 0);
    // After a restart nothing is in memory: the entry's refresh token is used first.
    let restarted = ChatGptRuntime::with(store.clone(), server.endpoints(), Arc::new(mailcraft_time::now));
    assert_eq!(restarted.access_token().unwrap().expose(), "acc.ess-2");
    assert_eq!(refreshes(&server), 1);
}

#[test]
fn the_rotated_token_never_overwrites_a_newer_entry() {
    let store = Arc::new(FakeStore::default());
    let during = store.clone();
    let server = fake(0, move |_| {
        // Another sign-in is written while this refresh is under way.
        let newer = StoredSignIn::new("app_other", "re.fresh-newer", "acct-2").unwrap();
        during.entries.lock().unwrap().insert("ai.chatgpt".into(), newer.to_entry());
        tokens("acc.ess-2", "re.fresh-2", 3_600)
    });
    let runtime = signed_in(&server, &store, &Arc::default(), &mut settings_with_chatgpt());
    runtime.access_token().unwrap();
    assert_eq!(store.entry().unwrap().refresh_token.expose(), "re.fresh-newer");
}

#[test]
fn terminal_grant_errors_end_the_sign_in() {
    for code in ["invalid_grant", "invalid_client", "unauthorized_client"] {
        let body = format!(r#"{{"error":"{code}"}}"#);
        let server = fake(0, move |_| json("400 Bad Request", &body));
        let store = Arc::new(FakeStore::default());
        let mut settings = settings_with_chatgpt();
        let runtime = signed_in(&server, &store, &Arc::default(), &mut settings);

        let error = runtime.access_token().unwrap_err();
        ended_settings(&mut settings);

        assert_eq!(error, SiwcError::Ended, "{code}");
        assert_eq!(error.to_string(), SIGN_IN_ENDED_TEXT);
        assert!(error.to_string().starts_with("OpenAI ended this sign-in"));
        assert!(store.get("ai.chatgpt").is_none(), "{code}: the entry is deleted");
        assert_eq!(runtime.status(), SiwcStatus::SignInEnded);
        assert!(!runtime.status().is_on(), "{code}: the runtime is off");
        assert!(!settings.chain.get(&RuntimeId::ChatGpt).unwrap().enabled());
        assert!(!settings.chatgpt_key);
        // Nothing more is asked of OpenAI until the owner signs in again.
        assert_eq!(runtime.access_token().unwrap_err(), SiwcError::Ended);
        assert_eq!(refreshes(&server), 1);
    }
}

#[test]
fn neg_transient_errors_keep_the_entry() {
    let cases: Vec<(super::siwc_tests::Answer, Duration)> = vec![
        (("429 Too Many Requests", "application/json", "{}".into(), vec![("Retry-After", "120".into())]), Duration::from_secs(120)),
        (json("408 Request Timeout", "{}"), FIRST_BACKOFF),
        (json("400 Bad Request", r#"{"error":"invalid_request"}"#), FIRST_BACKOFF),
        (json("400 Bad Request", "not json"), FIRST_BACKOFF),
        (json("503 Service Unavailable", "{}"), FIRST_BACKOFF),
    ];
    for (answer, at_least) in cases {
        let label = answer.0;
        let server = fake(0, move |n| if n == 0 { answer.clone() } else { tokens("acc.ess-2", "re.fresh-2", 3_600) });
        let store = Arc::new(FakeStore::default());
        let offset = Arc::new(AtomicI64::new(0));
        let runtime = signed_in(&server, &store, &offset, &mut settings_with_chatgpt());

        let error = runtime.access_token().unwrap_err();

        let SiwcError::Waiting { wait, .. } = error else { panic!("{label}: {error:?}") };
        assert!(wait >= at_least, "{label}: waits {wait:?}, at least {at_least:?}");
        assert_eq!(store.entry().unwrap().refresh_token.expose(), "re.fresh-1", "{label}: the entry is kept");
        assert_eq!(runtime.status(), SiwcStatus::SignedIn, "{label}: the runtime stays on");
        // Not tried again before the wait is over.
        assert!(matches!(runtime.access_token(), Err(SiwcError::Waiting { .. })));
        assert_eq!(refreshes(&server), 1, "{label}");
        // After it, the next refresh goes out and works.
        offset.store(i64::try_from(wait.as_secs()).unwrap() + 1, Ordering::SeqCst);
        assert_eq!(runtime.access_token().unwrap().expose(), "acc.ess-2", "{label}");
        assert_eq!(refreshes(&server), 2, "{label}");
    }
}

#[test]
fn sign_out_with_no_network() {
    let server = fake(3_600, |_| tokens("acc.ess-2", "re.fresh-2", 3_600));
    let store = Arc::new(FakeStore::default());
    let mut settings = settings_with_chatgpt();
    settings.set_runtime_consent("other@example.org", &RuntimeId::ChatGpt, true).unwrap();
    let runtime = signed_in(&server, &store, &Arc::default(), &mut settings);
    let before = server.total();

    runtime.sign_out().unwrap();
    signed_out_settings(&mut settings);

    assert_eq!(server.total(), before, "the fake server receives no request");
    assert!(store.get("ai.chatgpt").is_none(), "no ai.chatgpt entry");
    assert!(!settings.consents.allows(OWNER, &RuntimeId::ChatGpt) && settings.consents.accounts_for(&RuntimeId::ChatGpt).next().is_none());
    assert!(!settings.chain.get(&RuntimeId::ChatGpt).unwrap().enabled());
    assert!(!settings.chatgpt_key);
    assert!(settings.chatgpt_host_id.is_some(), "the host id is kept for the next sign-in");
    assert_eq!(runtime.status(), SiwcStatus::SignedOut);
    assert_eq!(runtime.access_token().unwrap_err(), SiwcError::NotSignedIn, "the access token was dropped");
    assert_eq!(server.total(), before, "and still nothing was sent");
    assert_eq!(sign_out_pointer(), SIGN_OUT_POINTER_TEXT);
    assert!(sign_out_pointer().contains("ChatGPT settings"));
}

#[test]
fn a_terminal_error_for_a_superseded_sign_in_leaves_the_newer_entry() {
    let store = Arc::new(FakeStore::default());
    let during = store.clone();
    let server = fake(0, move |_| {
        // Another sign-in is written while this refresh is under way; OpenAI refuses the old one.
        let newer = StoredSignIn::new("app_other", "re.fresh-newer", "acct-2").unwrap();
        during.entries.lock().unwrap().insert("ai.chatgpt".into(), newer.to_entry());
        json("400 Bad Request", r#"{"error":"invalid_grant"}"#)
    });
    let runtime = signed_in(&server, &store, &Arc::default(), &mut settings_with_chatgpt());

    let error = runtime.access_token().unwrap_err();

    assert!(matches!(error, SiwcError::Waiting { kind: "superseded", .. }), "{error:?}");
    assert_eq!(store.entry().unwrap().refresh_token.expose(), "re.fresh-newer", "the newer sign-in is not deleted");
    assert_ne!(runtime.status(), SiwcStatus::SignInEnded, "the runtime isn't ended for a refresh token nobody holds");
}

/// A credential store that never answers (an unanswered keychain prompt).
struct SilentStore;

impl mailcraft_host::SecretStore for SilentStore {
    fn get(&self, _: &str) -> Option<String> {
        std::thread::sleep(Duration::from_secs(5));
        None
    }
    fn set(&self, _: &str, _: &str) -> Result<(), mailcraft_host::SecretError> {
        std::thread::sleep(Duration::from_secs(5));
        Ok(())
    }
    fn delete(&self, _: &str) {
        std::thread::sleep(Duration::from_secs(5));
    }
}

#[test]
fn neg_a_store_that_never_answers_does_not_hold_a_request_forever() {
    let server = fake(0, |_| tokens("acc.ess-2", "re.fresh-2", 3_600));
    let runtime = ChatGptRuntime::with(Arc::new(SilentStore), server.endpoints(), Arc::new(mailcraft_time::now))
        .with_store_wait(Duration::from_millis(100), Duration::from_millis(100));
    let at = std::time::Instant::now();
    let error = runtime.access_token().unwrap_err();
    assert!(matches!(error, SiwcError::Store(_)), "{error:?}");
    let error = runtime.sign_out().unwrap_err();
    assert!(matches!(error, SiwcError::Store(_)), "{error:?}");
    assert!(at.elapsed() < Duration::from_secs(2), "waited {:?}", at.elapsed());
    assert_eq!(server.total(), 0, "nothing was asked of OpenAI without a sign-in");
}
