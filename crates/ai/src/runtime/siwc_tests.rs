// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The ChatGPT sign-in's pure parts: redaction, the refresh decision, the classification.

use super::*;

fn grant(expires_at: i64) -> AccessGrant {
    AccessGrant { token: AccessToken::new("acc.ess").unwrap(), expires_at }
}

#[test]
fn tokens_and_the_entry_never_show_their_secrets() {
    let entry = StoredSignIn::new("app_issued", "re.fresh-secret", "acct-1").unwrap();
    let debug = format!("{entry:?} {:?}", grant(0));
    for secret in ["app_issued", "re.fresh-secret", "acc.ess"] {
        assert!(!debug.contains(secret), "{debug}");
    }
}

#[test]
fn the_entry_round_trips_and_hostile_entries_are_refused() {
    let entry = StoredSignIn::new("app_issued", "re.fresh", "acct-1").unwrap();
    assert_eq!(StoredSignIn::from_entry(&entry.to_entry()), Some(entry));
    for bad in [
        "",
        "{}",
        "[1]",
        r#"{"v":2,"client_id":"a","refresh_token":"b"}"#,
        r#"{"v":1,"client_id":"a b","refresh_token":"b"}"#,
        r#"{"v":1,"client_id":"a","refresh_token":""}"#,
    ] {
        assert_eq!(StoredSignIn::from_entry(bad), None, "{bad}");
    }
    assert_eq!(StoredSignIn::from_entry(&"x".repeat(40_000)), None);
    assert_eq!(StoredSignIn::new("a", "b", "../x").unwrap().account_id, "");
}

#[test]
fn refresh_at_expiry_minus_60_seconds_and_after_a_restart() {
    assert!(needs_refresh(None, 1_000), "no token in memory: refresh first");
    assert!(needs_refresh(Some(&grant(1_059)), 1_000), "59 s left");
    assert!(!needs_refresh(Some(&grant(1_060)), 1_000), "60 s left");
    assert!(needs_refresh(Some(&grant(i64::MIN)), i64::MAX));
    assert!(refresh_after_unauthorized(false));
    assert!(!refresh_after_unauthorized(true), "once, never twice");
}

#[test]
fn only_the_three_grant_errors_are_terminal() {
    for code in TERMINAL_GRANT_ERRORS {
        for status in [Some(400), Some(401), Some(200), None] {
            assert_eq!(classify(status, code, None, 0), RefreshFailure::Terminal { code }, "{code} {status:?}");
        }
    }
}

#[test]
fn neg_transient_failures_keep_the_sign_in_and_back_off() {
    let wait = |f: RefreshFailure| match f {
        RefreshFailure::Transient { wait, kind } => (wait, kind),
        RefreshFailure::Terminal { code } => panic!("terminal {code}"),
    };
    assert_eq!(wait(classify(Some(429), "", Some("120"), 0)), (Duration::from_secs(120), TransientKind::RateLimited));
    assert_eq!(wait(classify(Some(429), "", Some("1"), 3)), (Duration::from_secs(16), TransientKind::RateLimited), "the backoff when it is longer");
    assert_eq!(wait(classify(Some(429), "", Some("999999"), 0)).0, MAX_RETRY_AFTER);
    assert_eq!(wait(classify(Some(408), "", None, 0)), (FIRST_BACKOFF, TransientKind::Timeout));
    assert_eq!(wait(classify(Some(400), "invalid_request", None, 1)), (Duration::from_secs(4), TransientKind::ClientError));
    assert_eq!(wait(classify(Some(503), "", None, 0)).1, TransientKind::ServerError);
    assert_eq!(wait(classify(None, "", None, 0)).1, TransientKind::Network);
    assert_eq!(backoff(u32::MAX), MAX_BACKOFF);
    assert_eq!(retry_after_wait("Wed, 21 Oct 2026 07:28:00 GMT"), None);
}

#[test]
fn identities_are_cut_and_cleaned_and_the_status_says_whether_it_is_on() {
    assert_eq!(shown_identity(&format!("{}\u{7}", "é".repeat(300))).chars().count(), 128);
    assert_eq!(shown_identity("a\u{202E}b\nc"), "abc");
    assert!(SiwcStatus::SignedIn.is_on());
    assert!(!SiwcStatus::SignInEnded.is_on() && !SiwcStatus::SignedOut.is_on() && !SiwcStatus::Waiting.is_on());
    assert!(SIGN_OUT_POINTER_TEXT.contains("ChatGPT settings"));
}
