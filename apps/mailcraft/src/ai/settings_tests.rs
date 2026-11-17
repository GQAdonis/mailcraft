// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `ai.json`: read as hostile input, never holding a key, and consents that only the owner gives
//! and that go when a provider's address changes.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use mailcraft_ai::{ApiKey, ProviderConfig, ProviderKind, Timeouts};
use serde_json::{Value, json};

use super::settings::{AiSettings, FILE, MAX_PROVIDERS, Shared, load, save};

const KEY: &str = "sk-test-not-a-real-key-0123456789";

fn local(id: &str) -> ProviderConfig {
    ProviderConfig { id: id.into(), base_url: "http://127.0.0.1:11437".into(), model: "apple-foundationmodel".into(), ..ProviderConfig::default() }
}

fn remote(id: &str) -> ProviderConfig {
    ProviderConfig {
        id: id.into(),
        kind: ProviderKind::Remote,
        base_url: "https://api.example.org/v1".into(),
        model: "remote-model".into(),
        pinned_sha256: "ab".repeat(32),
        ..ProviderConfig::default()
    }
}

fn file_of(value: &Value) -> AiSettings {
    AiSettings::from_json(value.to_string().as_bytes())
}

#[test]
fn what_cant_be_read_is_no_provider_at_all() {
    for junk in [&b""[..], b"null", b"[]", b"{\"providers\": 7}", b"\xff\xfe", b"{\"providers\": [", b"{\"active\": \"x\"}"] {
        assert_eq!(AiSettings::from_json(junk), AiSettings::default(), "{:?}", String::from_utf8_lossy(junk));
    }
}

#[test]
fn providers_that_cant_be_used_are_dropped_and_the_first_of_a_name_kept() {
    let read = file_of(&json!({
        "version": 1,
        "active": "apfel",
        "providers": [
            serde_json::to_value(local("apfel")).unwrap(),
            {"id": "apfel", "base_url": "http://127.0.0.1:9999", "model": "second"},
            {"id": "elsewhere", "base_url": "http://192.0.2.1:11434", "model": "m"},
            {"id": "plain-remote", "kind": "remote", "base_url": "http://api.example.org", "model": "m"},
            {"id": "bad id!", "base_url": "http://127.0.0.1:1", "model": "m"},
            {"id": "odd", "base_url": "http://127.0.0.1:1", "model": "m", "consented_accounts": [1, 2]},
            "not a provider",
            serde_json::to_value(remote("remote-1")).unwrap(),
        ],
    }));
    assert_eq!(read.providers.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["apfel", "remote-1"]);
    assert_eq!(read.provider("apfel").unwrap().model, "apple-foundationmodel");
    assert_eq!(read.active, "apfel");
    // An active provider that isn't there is none.
    assert_eq!(file_of(&json!({"active": "gone", "providers": [serde_json::to_value(local("apfel")).unwrap()]})).active, "");
    // At most so many.
    let many: Vec<Value> = (0..40).map(|n| serde_json::to_value(local(&format!("p{n}"))).unwrap()).collect();
    assert_eq!(file_of(&json!({"providers": many})).providers.len(), MAX_PROVIDERS);
}

#[test]
fn a_key_is_never_written_or_read_from_the_file() {
    let mut settings = AiSettings::default();
    settings.set_provider(ProviderConfig { api_key: Some(ApiKey::new(KEY).unwrap()), ..remote("remote-1") }).unwrap();
    assert!(settings.providers[0].api_key.is_none(), "not even kept in memory with the settings");
    settings.providers[0].api_key = Some(ApiKey::new(KEY).unwrap());
    let written = settings.to_json().to_string();
    assert!(!written.contains(KEY) && !written.contains("api_key"), "{written}");
    // A key someone put in the file is not taken.
    let mut planted = settings.to_json();
    planted["providers"][0]["api_key"] = json!(KEY);
    assert!(file_of(&planted).providers[0].api_key.is_none());
}

#[test]
fn consents_come_from_the_owner_alone_and_go_when_the_address_changes() {
    let mut settings = AiSettings::default();
    // Consents handed in with a provider's settings are not taken.
    settings.set_provider(ProviderConfig { consented_accounts: vec!["sneaky@example.org".into()], ..remote("remote-1") }).unwrap();
    assert!(settings.provider("remote-1").unwrap().consented_accounts.is_empty());
    settings.set_consent("remote-1", " Owner@Example.org ", true).unwrap();
    settings.set_consent("remote-1", "owner@example.org", true).unwrap();
    assert_eq!(settings.provider("remote-1").unwrap().consented_accounts, ["owner@example.org"]);
    // The same address again (another model, other waits): the consents and the pin stay.
    let changed =
        ProviderConfig { model: "other-model".into(), timeouts: Timeouts { first_byte_secs: 5, ..Timeouts::default() }, ..remote("remote-1") };
    settings.set_provider(changed).unwrap();
    let kept = settings.provider("remote-1").unwrap();
    assert_eq!((kept.model.as_str(), kept.consented_accounts.len(), kept.pinned_sha256.len()), ("other-model", 1, 64));
    // Another address: the consents and the trusted certificate go.
    settings.set_provider(ProviderConfig { base_url: "https://other.example.org/v1".into(), ..remote("remote-1") }).unwrap();
    let moved = settings.provider("remote-1").unwrap();
    assert!(moved.consented_accounts.is_empty() && moved.pinned_sha256.is_empty());
    // Revoking.
    settings.set_consent("remote-1", "owner@example.org", true).unwrap();
    settings.set_consent("remote-1", "OWNER@example.org", false).unwrap();
    assert!(settings.provider("remote-1").unwrap().consented_accounts.is_empty());
    // A provider on this computer needs none; nonsense is refused.
    settings.set_provider(local("apfel")).unwrap();
    assert!(settings.set_consent("apfel", "owner@example.org", true).is_err());
    assert!(settings.set_consent("missing", "owner@example.org", true).is_err());
    for account in ["", "  ", "a\nb", &"x".repeat(400)] {
        assert!(settings.set_consent("remote-1", account, true).is_err(), "{account:?}");
    }
}

#[test]
fn a_provider_that_changes_kind_loses_its_consents_too() {
    let mut settings = AiSettings::default();
    let loopback_remote = ProviderConfig { base_url: "https://127.0.0.1:8443/v1".into(), ..remote("p") };
    settings.set_provider(loopback_remote.clone()).unwrap();
    settings.set_consent("p", "owner@example.org", true).unwrap();
    let local =
        ProviderConfig { kind: ProviderKind::Local, base_url: "http://127.0.0.1:8443/v1".into(), pinned_sha256: String::new(), ..loopback_remote };
    assert!(settings.set_provider(local).unwrap(), "moved");
    assert!(settings.provider("p").unwrap().consented_accounts.is_empty());
}

#[test]
fn a_provider_that_moves_has_its_key_withdrawn_until_a_new_one_is_saved() {
    let mut settings = AiSettings::default();
    assert!(!settings.set_provider(remote("r")).unwrap(), "a new provider moved nowhere");
    settings.note_key("r");
    assert!(settings.key_usable("r"));
    // Another model at the same address: the key stays.
    assert!(!settings.set_provider(ProviderConfig { model: "other".into(), ..remote("r") }).unwrap());
    assert!(settings.key_usable("r"));
    // Another address: it is not used any more, and that is kept across a restart.
    assert!(settings.set_provider(ProviderConfig { base_url: "https://other.example.org/v1".into(), ..remote("r") }).unwrap());
    assert!(!settings.key_usable("r"));
    assert!(!AiSettings::from_json(settings.to_json().to_string().as_bytes()).key_usable("r"));
    // Still noted, so that a removal finds the entry if it couldn't be taken out.
    assert_eq!(settings.noted, ["r"]);
    settings.restore_key("r");
    assert!(settings.key_usable("r"));
}

#[test]
fn a_remote_provider_whose_embeddings_model_changes_asks_for_consent_again() {
    let mut settings = AiSettings::default();
    settings.set_provider(remote("r")).unwrap();
    settings.set_consent("r", "owner@example.org", true).unwrap();
    settings.set_provider(ProviderConfig { embedding_model: "embed-1".into(), ..remote("r") }).unwrap();
    assert!(settings.provider("r").unwrap().consented_accounts.is_empty(), "embedding in the background is asked for anew");
    settings.set_consent("r", "owner@example.org", true).unwrap();
    // The same embeddings model again: the consent stays.
    settings.set_provider(ProviderConfig { embedding_model: "embed-1".into(), ..remote("r") }).unwrap();
    assert_eq!(settings.provider("r").unwrap().consented_accounts.len(), 1);
}

#[cfg(unix)]
#[test]
fn the_file_is_the_owners_alone() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(FILE);
    save(&path, &AiSettings::default()).unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
}

#[test]
fn active_and_removed() {
    let mut settings = AiSettings::default();
    settings.set_provider(local("apfel")).unwrap();
    assert!(settings.set_active("nobody").is_err());
    settings.set_active("apfel").unwrap();
    assert_eq!(settings.active().map(|p| p.id.as_str()), Some("apfel"));
    assert!(settings.remove("apfel"));
    assert!(!settings.remove("apfel"));
    assert_eq!((settings.active.as_str(), settings.active()), ("", None));
    settings.set_active("").unwrap();
    assert!(settings.set_provider(ProviderConfig { base_url: "http://192.0.2.1".into(), ..local("x") }).is_err());
}

#[test]
fn kept_in_the_settings_folder_and_read_back() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(FILE);
    assert_eq!(load(&path), AiSettings::default(), "no file, no provider");
    let shared = Shared::open(Some(path.clone()));
    shared.change(|s| s.set_provider(remote("remote-1"))).unwrap();
    shared.change(|s| s.set_consent("remote-1", "owner@example.org", true)).unwrap();
    shared.change(|s| s.set_active("remote-1")).unwrap();
    // A change that is refused changes nothing.
    assert!(shared.change(|s| s.set_active("nobody")).is_err());
    let read = load(&path);
    assert_eq!(read, shared.snapshot());
    assert_eq!(read.active, "remote-1");
    assert_eq!(read.provider("remote-1").unwrap().consented_accounts, ["owner@example.org"]);
    assert!(!std::fs::read_to_string(&path).unwrap().contains("api_key"));
    assert!(!dir.path().join("ai.json.new").exists());
    // Too large a file is not read at all.
    std::fs::write(&path, format!("{{\"providers\": [], \"pad\": \"{}\"}}", "x".repeat(300 * 1024))).unwrap();
    assert_eq!(load(&path), AiSettings::default());
    // Written whole.
    save(&path, &AiSettings::default()).unwrap();
    assert_eq!(load(&path), AiSettings::default());
}

fn gateway(id: &str) -> ProviderConfig {
    ProviderConfig {
        id: id.into(),
        kind: ProviderKind::Gateway,
        base_url: "http://127.0.0.1:4000".into(),
        model: "alibaba-token/qwen4-max".into(),
        ..ProviderConfig::default()
    }
}

#[test]
fn a_gateway_is_kept_and_needs_consent_like_a_provider_elsewhere() {
    let mut settings = AiSettings::default();
    settings.set_provider(gateway("g")).unwrap();
    settings.set_consent("g", "Owner@Example.org", true).unwrap();
    assert_eq!(settings.provider("g").unwrap().consented_accounts, ["owner@example.org"]);
    assert!(settings.set_consent("apfel", "owner@example.org", true).is_err());
    settings.set_provider(local("apfel")).unwrap();
    assert!(settings.set_consent("apfel", "owner@example.org", true).is_err(), "a local provider needs none");
    let read = file_of(&settings.to_json());
    assert_eq!(read.provider("g").map(|p| p.kind), Some(ProviderKind::Gateway));
    assert_eq!(read.provider("g").unwrap().consented_accounts, ["owner@example.org"]);
    // An unknown kind (a later version's) is dropped, never taken for local.
    let later =
        file_of(&json!({"version": 1, "active": "x", "providers": [{"id": "x", "kind": "relay", "base_url": "http://127.0.0.1:1", "model": "m"}]}));
    assert!(later.providers.is_empty() && later.active.is_empty());
}

#[test]
fn a_key_is_read_only_when_noted_and_not_withdrawn() {
    let mut settings = AiSettings::default();
    assert!(!settings.key_kept("g"));
    settings.note_key("g");
    assert!(settings.key_kept("g"));
    settings.withdraw_key("g");
    assert!(!settings.key_kept("g"));
    settings.restore_key("g");
    settings.unnote_key("g");
    assert!(!settings.key_kept("g"));
}

#[test]
fn the_owners_no_to_the_gateway_question_is_kept_as_hostile_input_is_read() {
    let mut settings = AiSettings::default();
    settings.keep_local_list("http://127.0.0.1:1234/v1/models", "0123456789abcdef");
    for (address, models) in [("", "0123456789abcdef"), ("http://x", "nothex"), ("has space", "0123456789abcdef"), ("http://x", "0123456789abcdef0")]
    {
        settings.keep_local_list(address, models);
    }
    assert_eq!(settings.local_lists.len(), 1);
    assert!(settings.is_local_list("http://127.0.0.1:1234/v1/models", "0123456789ABCDEF"));
    let read = file_of(&settings.to_json());
    assert_eq!(read.local_lists, settings.local_lists);
    let planted = file_of(
        &json!({"local_model_lists": [7, {"address": 1}, {"address": "a\nb", "models": "0123456789abcdef"}, {"address": "http://ok", "models": "fedcba9876543210"}]}),
    );
    assert_eq!(planted.local_lists, [("http://ok".to_string(), "fedcba9876543210".to_string())]);
    for n in 0..100 {
        settings.keep_local_list(&format!("http://127.0.0.1:{n}/v1/models"), "0123456789abcdef");
    }
    assert_eq!(settings.local_lists.len(), 64, "capped, oldest first out");
}

#[test]
fn a_yes_to_the_gateway_question_makes_the_provider_a_gateway_with_no_consent() {
    let shared = Shared::open(None);
    shared.change(|s| s.set_provider(ProviderConfig { base_url: "http://127.0.0.1:4000".into(), ..local("relay") })).unwrap();
    let route_address = super::settings::route_address(&shared.snapshot().providers[0]);
    let question = super::settings::GatewayQuestion {
        provider: "relay".into(),
        address: "http://127.0.0.1:4000".into(),
        models: "0123456789abcdef".into(),
        publishers: vec!["openai".into()],
        route_address,
    };
    assert!(shared.answer_gateway("relay", "0123456789abcdef", true).is_err(), "nothing was asked");
    assert!(shared.ask_gateway(question.clone()));
    assert!(!shared.ask_gateway(super::settings::GatewayQuestion { provider: "other".into(), ..question.clone() }), "one question at a time");
    let shown = serde_json::to_value(shared.gateway_question().unwrap()).unwrap();
    assert_eq!(shown, json!({"provider": "relay", "address": "http://127.0.0.1:4000", "models": "0123456789abcdef", "publishers": ["openai"]}));
    shared.answer_gateway("relay", "0123456789abcdef", true).unwrap();
    let now = shared.snapshot();
    assert_eq!(now.provider("relay").map(|p| p.kind), Some(ProviderKind::Gateway));
    assert!(now.provider("relay").unwrap().consented_accounts.is_empty());
    assert!(now.local_lists.is_empty());
    assert!(shared.gateway_question().is_none());
}
