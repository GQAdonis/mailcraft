// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `ai.json` version 3 as hostile input (SCN-RT-SETTINGS-001): too large, a version this one
//! doesn't know, an unknown runtime kind (`opencode` among them) dropped and never taken for one
//! on this computer, a runtime named twice, junk consents and `keys_kept_for`, no Claude Code or
//! Codex key; and, as the negative control, a well-formed file that loads and is used. Synthetic
//! settings only.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use mailcraft_ai::{ProviderConfig, ProviderKind};
use mailcraft_model::ai::{Carries, ChainEntry, CompatKind, CompatProvider, Fallback, ProviderId, RuntimeId, Where, may_send, where_of};
use serde_json::{Value, json};

use super::settings::{AiSettings, Backup, FILE, MAX_FILE, load};
use super::settings_v3::OnDeviceFirst;

const IDENTITY: &str = "apfel:apple-foundationmodel:0123456789abcdef";

fn remote(id: &str) -> Value {
    serde_json::to_value(ProviderConfig {
        id: id.into(),
        kind: ProviderKind::Remote,
        base_url: format!("https://{id}.example.org/v1"),
        model: "remote-model".into(),
        ..ProviderConfig::default()
    })
    .unwrap()
}

fn local(id: &str) -> Value {
    serde_json::to_value(ProviderConfig {
        id: id.into(),
        base_url: "http://127.0.0.1:11437".into(),
        model: "apple-foundationmodel".into(),
        ..ProviderConfig::default()
    })
    .unwrap()
}

/// A well-formed version 3 file.
fn good() -> Value {
    json!({
        "version": 3,
        "never_configured": false,
        "fallback": "ask",
        "chain": [
            {"runtime": "compat:lumen", "enabled": true},
            {"runtime": "on_device", "enabled": true},
            {"runtime": "chatgpt", "enabled": true, "model": "default"},
            {"runtime": "claude_code", "enabled": false, "model": "sonnet", "path": "/opt/example/bin/claude"},
            {"runtime": "uar", "enabled": false, "instance": "external"},
            {"runtime": "compat:apfel", "enabled": false},
        ],
        "on_device_first": "no",
        "on_device": {"address": "http://127.0.0.1:11437", "identity": IDENTITY, "context_window": 4096},
        "on_device_ports": [11437],
        "providers": [remote("lumen"), local("apfel")],
        "consents": {"avery@lumen.example": ["chatgpt", "compat:lumen"]},
        "keys_kept_for": ["compat:lumen", "chatgpt"],
        "keys_withdrawn": [],
        "local_model_lists": [],
        "detected": {"claude_code": {"path": "/opt/example/bin/claude", "version": "2.1.326", "checked": "2026-11-17T10:00:00Z"}},
    })
}

fn read(value: &Value) -> AiSettings {
    AiSettings::from_json(value.to_string().as_bytes())
}

/// What `settings` would let leave this computer: every enabled entry that isn't local.
fn enabled_leaving(settings: &AiSettings) -> Vec<RuntimeId> {
    let providers: Vec<CompatProvider> = settings
        .providers
        .iter()
        .map(|p| CompatProvider {
            id: ProviderId::new(&p.id).unwrap(),
            kind: match p.kind {
                ProviderKind::Local => CompatKind::Local,
                ProviderKind::Remote => CompatKind::Remote,
                _ => CompatKind::Gateway,
            },
        })
        .collect();
    settings.chain.entries().iter().filter(|e| e.enabled() && where_of(e, &providers, None) == Where::Leaves).map(|e| e.runtime().clone()).collect()
}

#[test]
fn neg_hostile_a_well_formed_file_loads_and_is_used() {
    let settings = read(&good());
    assert_eq!(
        settings.chain.ids().iter().map(ToString::to_string).collect::<Vec<_>>(),
        ["compat:lumen", "on_device", "chatgpt", "claude_code", "uar", "compat:apfel"]
    );
    assert_eq!((settings.active.as_str(), settings.backup), ("lumen", Backup::AppleOnDevice));
    assert_eq!(settings.backup_binding().map(|b| b.identity.as_str()), Some(IDENTITY));
    assert_eq!((settings.fallback, settings.on_device_first), (Fallback::Ask, OnDeviceFirst::No));
    assert!(settings.consents.allows("Avery@Lumen.example", &RuntimeId::ChatGpt));
    assert_eq!(settings.provider("lumen").unwrap().consented_accounts, ["avery@lumen.example"]);
    assert!(settings.key_kept("lumen") && settings.chatgpt_key);
    let claude = settings.chain.get(&RuntimeId::ClaudeCode).unwrap();
    assert_eq!((claude.model().map(|m| m.as_str()), claude.path().map(|p| p.as_str())), (Some("sonnet"), Some("/opt/example/bin/claude")));
    assert_eq!(settings.detected.get("claude_code").and_then(|d| d.version.clone()).as_deref(), Some("2.1.326"));
    // Written and read back: the same.
    assert_eq!(read(&settings.to_json()), settings);
    // And used: consent per account and per runtime.
    let mail = ["avery@lumen.example".to_string()];
    assert!(may_send(&RuntimeId::ChatGpt, Where::Leaves, Carries::MailOf(&mail), &settings.consents));
    let other = ["someone@example.org".to_string()];
    assert!(!may_send(&RuntimeId::ChatGpt, Where::Leaves, Carries::MailOf(&other), &settings.consents));
}

#[test]
fn a_file_too_large_is_no_settings() {
    let mut big = good();
    big["pad"] = json!("x".repeat(usize::try_from(MAX_FILE).unwrap()));
    let bytes = big.to_string();
    assert!(bytes.len() as u64 > MAX_FILE);
    assert_eq!(AiSettings::from_json(bytes.as_bytes()), AiSettings::default());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(FILE);
    std::fs::write(&path, bytes).unwrap();
    assert_eq!(load(&path), AiSettings::default());
}

#[test]
fn a_version_this_one_doesnt_know_is_the_safe_default() {
    for version in [json!(99), json!("3"), json!(3.5), json!(-1), json!(null), json!([3]), json!({"v": 3}), json!(0)] {
        let mut file = good();
        file["version"] = version.clone();
        let settings = read(&file);
        assert_eq!(settings, AiSettings::default(), "{version}");
        assert!(enabled_leaving(&settings).is_empty(), "nothing that leaves is on: {version}");
    }
}

#[test]
fn an_unknown_runtime_kind_is_dropped_and_never_local() {
    let mut file = good();
    file["chain"] = json!([
        {"runtime": "opencode", "enabled": true, "model": "openai/gpt-5"},
        {"runtime": "OPENCODE", "enabled": true},
        {"runtime": "compat:", "enabled": true},
        {"runtime": "compat:nobody-set-up", "enabled": true},
        {"runtime": "local", "enabled": true},
        {"runtime": "on_device ", "enabled": true},
        {"runtime": 7, "enabled": true},
        {"enabled": true},
        "on_device",
        {"runtime": "compat:lumen", "enabled": true},
    ]);
    let settings = read(&file);
    assert_eq!(settings.chain.ids(), [RuntimeId::compat("lumen").unwrap()]);
    assert_eq!(settings.active, "lumen");
    // Not the on-device model, and not a provider on this computer.
    assert_eq!(settings.backup, Backup::None);
    assert!(!settings.to_json().to_string().contains("opencode"));
    // A chain of nothing but OpenCode is a chain of nothing: no primary, nothing sent.
    file["chain"] = json!([{"runtime": "opencode", "enabled": true}]);
    let settings = read(&file);
    assert!(settings.chain.is_empty() && settings.explicitly_none());
}

#[test]
fn a_runtime_named_twice_keeps_the_first() {
    let mut file = good();
    file["chain"] = json!([
        {"runtime": "claude_code", "enabled": true},
        {"runtime": "on_device", "enabled": true},
        {"runtime": "claude_code", "enabled": false, "model": "opus"},
    ]);
    let settings = read(&file);
    assert_eq!(settings.chain.ids(), [RuntimeId::ClaudeCode, RuntimeId::OnDevice]);
    assert!(settings.chain.get(&RuntimeId::ClaudeCode).is_some_and(ChainEntry::enabled));
    assert_eq!(settings.chain.get(&RuntimeId::ClaudeCode).unwrap().model(), None);
    // More than sixteen: the rest is dropped, never a panic.
    let many: Vec<Value> = (0..5_000).map(|n| json!({"runtime": format!("compat:p{n}"), "enabled": true})).collect();
    file["chain"] = Value::Array(many);
    assert!(read(&file).chain.len() <= mailcraft_model::ai::MAX_CHAIN);
}

#[test]
fn junk_consents_and_kept_keys_are_dropped_one_by_one() {
    let mut file = good();
    file["consents"] = json!({
        "avery@lumen.example": ["chatgpt", "opencode", 7, null, "compat:", "compat:lumen"],
        "not an address": ["chatgpt"],
        "x\n@y": ["chatgpt"],
        "": ["chatgpt"],
        "blank@example.org": "chatgpt",
        "nothing@example.org": [],
    });
    file["keys_kept_for"] =
        json!(["compat:lumen", "claude_code", "codex", "opencode", "chatgpt", 7, null, {"a": 1}, "ai-key:", "ai-key:bad id", "compat:x y"]);
    let settings = read(&file);
    let consents: Vec<(String, Vec<String>)> =
        settings.consents.iter().map(|(a, set)| (a.to_string(), set.iter().map(ToString::to_string).collect())).collect();
    assert_eq!(consents, [("avery@lumen.example".to_string(), vec!["compat:lumen".to_string(), "chatgpt".to_string()])]);
    assert_eq!(settings.noted, ["lumen"]);
    assert!(settings.chatgpt_key && settings.legacy_keys.is_empty());
    assert_eq!(settings.to_json()["keys_kept_for"], json!(["compat:lumen", "chatgpt"]));
    // Junk of the wrong shape altogether.
    for junk in [json!(7), json!("x"), json!([1, 2]), json!(null)] {
        let mut file = good();
        file["consents"] = junk.clone();
        file["keys_kept_for"] = junk.clone();
        let settings = read(&file);
        assert!(settings.consents.is_empty() && settings.noted.is_empty() && !settings.chatgpt_key, "{junk}");
    }
}

#[test]
fn claude_code_and_codex_carry_no_key_and_nothing_asks_for_one() {
    let mut file = good();
    file["chain"] = json!([
        {"runtime": "claude_code", "enabled": true, "auth": "api_key", "api_key": "sk-test-planted"},
        {"runtime": "codex", "enabled": false, "auth": "own_api_billing"},
    ]);
    file["keys_kept_for"] = json!(["codex", "claude_code"]);
    let settings = read(&file);
    let written = settings.to_json().to_string();
    assert!(!written.contains("auth") && !written.contains("api_key") && !written.contains("sk-test-planted"), "{written}");
    assert!(settings.noted.is_empty() && settings.legacy_keys.is_empty() && !settings.chatgpt_key);
    assert_eq!(settings.to_json()["keys_kept_for"], json!([]));
}

#[test]
fn junk_everywhere_else_is_never_a_panic() {
    let shapes = [json!(null), json!(7), json!("x"), json!([]), json!({}), json!([null, {"runtime": []}]), json!({"a": {"b": [1, 2, 3]}})];
    for field in ["never_configured", "fallback", "chain", "on_device_first", "on_device", "on_device_ports", "providers", "chatgpt", "detected"] {
        for junk in &shapes {
            let mut file = good();
            file[field] = junk.clone();
            let settings = read(&file);
            assert_eq!(read(&settings.to_json()), settings, "{field}: {junk}");
        }
    }
    // A detection cache is checked, capped, and only for programs.
    let mut file = good();
    file["detected"] = json!({
        "claude_code": {"path": "relative/claude", "version": "x\ny", "checked": "z".repeat(1_000)},
        "opencode": {"path": "/opt/example/bin/opencode"},
        "compat:lumen": {"version": "1"},
    });
    let settings = read(&file);
    let claude = settings.detected.get("claude_code").cloned().unwrap_or_default();
    assert_eq!((claude.version, claude.checked), (None, None));
    assert_eq!(settings.detected.keys().collect::<Vec<_>>(), ["claude_code"]);
}

#[test]
fn the_fallback_and_the_on_device_question_read_as_they_say_or_their_default() {
    for (text, fallback) in
        [("next", Fallback::Next), ("ask", Fallback::Ask), ("stop", Fallback::Stop), ("STOP", Fallback::Next), ("", Fallback::Next)]
    {
        let mut file = good();
        file["fallback"] = json!(text);
        assert_eq!(read(&file).fallback, fallback, "{text}");
    }
    for (text, answer) in
        [("yes", OnDeviceFirst::Yes), ("no", OnDeviceFirst::No), ("unasked", OnDeviceFirst::Unasked), ("maybe", OnDeviceFirst::Unasked)]
    {
        let mut file = good();
        file["on_device_first"] = json!(text);
        assert_eq!(read(&file).on_device_first, answer, "{text}");
    }
}

#[test]
fn every_entry_off_and_never_configured_false_is_the_owners_none() {
    let mut file = good();
    file["chain"] = json!([{"runtime": "compat:lumen", "enabled": false}, {"runtime": "on_device", "enabled": false}]);
    let settings = read(&file);
    assert!(settings.explicitly_none());
    assert_eq!((settings.backup, settings.backup_binding()), (Backup::None, None));
    assert_eq!(read(&settings.to_json()), settings);
}

#[test]
fn the_setters_keep_the_chain_in_step() {
    let mut settings = read(&good());
    // Another primary: it goes first and the one before is off.
    settings.set_active("apfel").unwrap();
    let state: Vec<(String, bool)> = settings.chain.entries().iter().map(|e| (e.runtime().to_string(), e.enabled())).collect();
    assert_eq!(state[0], ("compat:apfel".to_string(), true));
    assert!(state.contains(&("compat:lumen".to_string(), false)));
    assert!(state.contains(&("chatgpt".to_string(), true)), "other runtimes keep their state");
    assert_eq!(read(&settings.to_json()).active, "apfel");
    // No backup: the on-device entry is off.
    settings.set_backup(Backup::None).unwrap();
    assert!(!settings.chain.get(&RuntimeId::OnDevice).unwrap().enabled());
    // The owner's None: every entry is off.
    settings.set_active("").unwrap();
    assert!(settings.chain.entries().iter().all(|e| !e.enabled()));
    assert!(read(&settings.to_json()).explicitly_none());
    // A provider taken away leaves the chain and every consent.
    settings.set_active("lumen").unwrap();
    assert!(settings.remove("lumen"));
    assert!(settings.chain.get(&RuntimeId::compat("lumen").unwrap()).is_none());
    assert!(settings.consents.accounts_for(&RuntimeId::compat("lumen").unwrap()).next().is_none());
    // A chain chosen whole: the primary and the backup are read from it.
    let chain = mailcraft_model::ai::Chain::new([
        ChainEntry::new(RuntimeId::OnDevice).with_enabled(true),
        ChainEntry::new(RuntimeId::compat("apfel").unwrap()).with_enabled(true),
        ChainEntry::new(RuntimeId::compat("gone").unwrap()).with_enabled(true),
    ])
    .unwrap();
    settings.set_chain(chain);
    assert_eq!((settings.active.as_str(), settings.backup), ("apfel", Backup::AppleOnDevice));
    assert!(settings.chain.get(&RuntimeId::compat("gone").unwrap()).is_none());
    assert_eq!(read(&settings.to_json()), settings);
}
