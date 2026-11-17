// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A provider's settings: what may be set up, who may be sent what (SCN-PRIVACY-001,
//! SCN-OPTIN-001), and that neither saving nor `Debug` ever shows the key.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::config::{Carries, ProviderConfig, ProviderKind, Timeouts, is_loopback_host, valid_id};
use crate::error::AiError;
use crate::key::ApiKey;

fn local(url: &str) -> ProviderConfig {
    ProviderConfig { id: "apfel".into(), base_url: url.into(), model: "apple-foundationmodel".into(), ..ProviderConfig::default() }
}

fn remote(url: &str, accounts: &[&str]) -> ProviderConfig {
    ProviderConfig {
        id: "mailcraft-boundary-run1".into(),
        kind: ProviderKind::Remote,
        base_url: url.into(),
        model: "some-model".into(),
        consented_accounts: accounts.iter().map(|a| a.to_string()).collect(),
        api_key: Some(ApiKey::new("sk-test-not-a-real-key-0123456789").unwrap()),
        ..ProviderConfig::default()
    }
}

#[test]
fn a_local_provider_must_be_on_this_computer() {
    for url in
        ["http://127.0.0.1:11437", "http://localhost:11434/v1", "http://[::1]:1234", "http://[::ffff:127.0.0.1]:8080/v1", "http://127.0.0.2:8080"]
    {
        assert_eq!(local(url).check(), Ok(()), "{url}");
    }
    for url in [
        "http://192.0.2.1:11434",
        "http://[::ffff:192.0.2.1]:11434",
        "https://models.example.org/v1",
        "http://localhost.example.org/v1",
        "ftp://127.0.0.1/v1",
        "127.0.0.1:11434",
        "",
        "http://user:pw@127.0.0.1/v1",
        "http://127.0.0.1/v1?key=x",
        "http://127.0.0.1/v1#x",
        "http://127.0.0.1 /v1",
        // Over TLS too, a provider on this computer goes the loopback-checked way, plain.
        "https://127.0.0.1:8443/v1",
        "https://localhost:8443",
    ] {
        assert!(matches!(local(url).check(), Err(AiError::Config(_))), "{url}");
    }
}

#[test]
fn a_remote_provider_is_spoken_to_over_https_only() {
    assert_eq!(remote("https://api.example.org/v1", &[]).check(), Ok(()));
    assert_eq!(remote("https://127.0.0.1:8443/v1", &[]).check(), Ok(()));
    for url in ["http://api.example.org/v1", "http://127.0.0.1:8443/v1"] {
        assert!(matches!(remote(url, &[]).check(), Err(AiError::Config(_))), "{url}");
    }
}

#[test]
fn names_models_fingerprints_and_windows_are_checked() {
    let fine = remote("https://api.example.org", &[]);
    for id in ["", "has space", "has:colon", "slash/no", &"x".repeat(65)] {
        assert!(matches!(ProviderConfig { id: id.to_string(), ..fine.clone() }.check(), Err(AiError::Config(_))), "{id:?}");
    }
    assert!(valid_id("mailcraft-boundary-20261116.a_1") && !valid_id("a\nb"));
    assert!(matches!(ProviderConfig { model: " ".into(), ..fine.clone() }.check(), Err(AiError::Config(_))));
    assert!(matches!(ProviderConfig { embedding_model: "bad\u{7}".into(), ..fine.clone() }.check(), Err(AiError::Config(_))));
    let pin = "AB".repeat(32);
    assert_eq!(ProviderConfig { pinned_sha256: pin.clone(), ..fine.clone() }.check(), Ok(()));
    for bad in ["abc", &"zz".repeat(32), &"ab".repeat(33)] {
        assert!(matches!(ProviderConfig { pinned_sha256: bad.to_string(), ..fine.clone() }.check(), Err(AiError::Config(_))), "{bad}");
    }
    // A fingerprint means TLS.
    assert!(matches!(ProviderConfig { pinned_sha256: pin, ..local("http://127.0.0.1:1") }.check(), Err(AiError::Config(_))));
    for window in [0, 255, 2_000_000] {
        assert!(matches!(ProviderConfig { context_window: window, ..fine.clone() }.check(), Err(AiError::Config(_))), "{window}");
    }
    let many = ProviderConfig { consented_accounts: vec!["a@example.org".into(); 1_001], ..fine.clone() };
    assert!(matches!(many.check(), Err(AiError::Config(_))));
}

#[test]
fn a_remote_provider_is_sent_mail_only_of_the_accounts_allowed() {
    let provider = remote("https://api.example.org/v1", &["owner@example.org"]);
    assert_eq!(provider.may_send(Carries::OwnerTextOnly), Ok(()));
    assert_eq!(provider.may_send(Carries::MailOf(&["owner@example.org"])), Ok(()));
    assert_eq!(provider.may_send(Carries::MailOf(&[" OWNER@example.org "])), Ok(()));
    assert_eq!(provider.may_send(Carries::MailOf(&["other@example.org"])), Err(AiError::NoConsent));
    // One account not allowed is enough to send nothing.
    assert_eq!(provider.may_send(Carries::MailOf(&["owner@example.org", "other@example.org"])), Err(AiError::NoConsent));
    assert_eq!(provider.may_send(Carries::MailOf(&[])), Err(AiError::NoConsent));
    assert_eq!(provider.may_send(Carries::MailOf(&[""])), Err(AiError::NoConsent));
    // Revoking is taking the account off the list.
    let revoked = ProviderConfig { consented_accounts: Vec::new(), ..provider };
    assert_eq!(revoked.may_send(Carries::MailOf(&["owner@example.org"])), Err(AiError::NoConsent));
    // A provider on this computer needs no list.
    assert_eq!(local("http://127.0.0.1:11437").may_send(Carries::MailOf(&["anyone@example.org"])), Ok(()));
}

#[test]
fn endpoints_are_under_v1_once() {
    assert_eq!(local("http://127.0.0.1:11437").endpoint("chat/completions"), "http://127.0.0.1:11437/v1/chat/completions");
    assert_eq!(local("http://127.0.0.1:11437/").endpoint("/embeddings"), "http://127.0.0.1:11437/v1/embeddings");
    assert_eq!(local(" http://localhost:1234/v1/ ").endpoint("chat/completions"), "http://localhost:1234/v1/chat/completions");
    assert_eq!(local("http://localhost:1234/V1").endpoint("embeddings"), "http://localhost:1234/V1/embeddings");
    assert_eq!(local("http://localhost:1234/api").endpoint("embeddings"), "http://localhost:1234/api/v1/embeddings");
}

#[test]
fn loopback_hosts() {
    for host in ["localhost", "LOCALHOST.", "127.0.0.1", "127.255.255.254", "::1", "[::1]", "::ffff:127.0.0.1", "[::ffff:7f00:1]"] {
        assert!(is_loopback_host(host), "{host}");
    }
    for host in ["", "192.0.2.1", "::ffff:192.0.2.1", "::127.0.0.1", "0.0.0.0", "::", "localhost.example.org", "127.0.0.1.example.org", "2001:db8::1"]
    {
        assert!(!is_loopback_host(host), "{host}");
    }
}

#[test]
fn the_key_is_never_saved_and_never_shown() {
    let provider = remote("https://api.example.org/v1", &["owner@example.org"]);
    let saved = serde_json::to_string(&provider).unwrap();
    assert!(!saved.contains("sk-test") && !saved.contains("api_key"), "{saved}");
    let read: ProviderConfig = serde_json::from_str(&saved).unwrap();
    assert!(read.api_key.is_none());
    assert_eq!(ProviderConfig { api_key: provider.api_key.clone(), ..read }, provider);
    let shown = format!("{provider:?}");
    for hidden in ["sk-test", "api.example.org", "owner@example.org"] {
        assert!(!shown.contains(hidden), "{hidden} in {shown}");
    }
    assert!(shown.contains("has_key: true"), "{shown}");
}

#[test]
fn settings_saved_by_an_older_or_newer_version_still_read() {
    let read: ProviderConfig = serde_json::from_str(r#"{"id":"x","base_url":"http://127.0.0.1:1","model":"m","later":1}"#).unwrap();
    assert_eq!(read.kind, ProviderKind::Local);
    assert_eq!(read.timeouts, Timeouts::default());
    assert_eq!(read.context_window, 4_096);
    let read: ProviderConfig = serde_json::from_str(r#"{"kind":"remote","timeouts":{"first_byte_secs":5}}"#).unwrap();
    assert_eq!(read.kind, ProviderKind::Remote);
    assert_eq!((read.timeouts.first_byte_secs, read.timeouts.total_secs), (5, 300));
}

#[test]
fn timeouts_stay_within_their_ceilings() {
    let wild = Timeouts { connect_secs: 0, first_byte_secs: 9_999, idle_secs: 0, total_secs: 9_999 }.clamped();
    assert_eq!(wild, Timeouts { connect_secs: 1, first_byte_secs: 300, idle_secs: 1, total_secs: 300 });
    let short = Timeouts { connect_secs: 20, first_byte_secs: 60, idle_secs: 60, total_secs: 5 }.clamped();
    assert_eq!(short, Timeouts { connect_secs: 5, first_byte_secs: 5, idle_secs: 5, total_secs: 5 });
}

fn gateway(url: &str, accounts: &[&str]) -> ProviderConfig {
    ProviderConfig {
        id: "liter-llm".into(),
        kind: ProviderKind::Gateway,
        base_url: url.into(),
        model: "alibaba-token/qwen4-max".into(),
        consented_accounts: accounts.iter().map(|a| a.to_string()).collect(),
        ..ProviderConfig::default()
    }
}

#[test]
fn a_gateway_is_on_this_computer_over_plain_http_only() {
    for url in ["http://127.0.0.1:4000", "http://localhost:4000/v1", "http://[::1]:4000"] {
        assert_eq!(gateway(url, &[]).check(), Ok(()), "{url}");
    }
    for url in ["https://127.0.0.1:4000", "http://192.0.2.1:4000", "https://gateway.example.org/v1", "http://gateway.example.org"] {
        assert!(matches!(gateway(url, &[]).check(), Err(AiError::Config(_))), "{url}");
    }
}

#[test]
fn a_gateway_needs_each_accounts_consent_as_a_remote_provider_does() {
    let none = gateway("http://127.0.0.1:4000", &[]);
    assert_eq!(none.may_send(Carries::MailOf(&["owner@example.org"])), Err(AiError::NoConsent));
    assert_eq!(none.may_send(Carries::MailOf(&[])), Err(AiError::NoConsent));
    assert_eq!(none.may_send(Carries::OwnerTextOnly), Ok(()));
    assert!(!none.consents("owner@example.org"));
    let allowed = gateway("http://127.0.0.1:4000", &["Owner@Example.org"]);
    assert_eq!(allowed.may_send(Carries::MailOf(&["owner@example.org"])), Ok(()));
    assert_eq!(allowed.may_send(Carries::MailOf(&["owner@example.org", "other@example.org"])), Err(AiError::NoConsent));
    assert!(ProviderKind::Gateway.on_this_computer() && ProviderKind::Gateway.mail_leaves_this_computer());
    assert!(ProviderKind::Local.on_this_computer() && !ProviderKind::Local.mail_leaves_this_computer());
    assert!(!ProviderKind::Remote.on_this_computer() && ProviderKind::Remote.mail_leaves_this_computer());
}

#[test]
fn older_settings_read_as_before_and_an_unknown_kind_is_never_taken_for_local() {
    let older: ProviderConfig = serde_json::from_str(r#"{"id":"apfel","base_url":"http://127.0.0.1:11434","model":"m"}"#).unwrap();
    assert_eq!(older.kind, ProviderKind::Local);
    let remote: ProviderConfig = serde_json::from_str(r#"{"id":"r","kind":"remote","base_url":"https://api.example.org","model":"m"}"#).unwrap();
    assert_eq!(remote.kind, ProviderKind::Remote);
    let written = serde_json::to_value(gateway("http://127.0.0.1:4000", &[])).unwrap();
    assert_eq!(written["kind"], "gateway");
    let back: ProviderConfig = serde_json::from_value(written).unwrap();
    assert_eq!(back.kind, ProviderKind::Gateway);
    assert!(serde_json::from_str::<ProviderConfig>(r#"{"id":"x","kind":"relay","base_url":"http://127.0.0.1:1","model":"m"}"#).is_err());
    assert_eq!([ProviderKind::Local.name(), ProviderKind::Remote.name(), ProviderKind::Gateway.name()], ["local", "remote", "gateway"]);
}
