// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use proptest::prelude::*;
use serde_json::{Value, json};

use super::AiError;
use super::runtime::*;

fn id(text: &str) -> RuntimeId {
    RuntimeId::parse(text).unwrap()
}

fn on(text: &str) -> ChainEntry {
    ChainEntry::new(id(text)).with_enabled(true)
}

fn provider(name: &str, kind: CompatKind) -> CompatProvider {
    CompatProvider { id: ProviderId::new(name).unwrap(), kind }
}

fn providers() -> Vec<CompatProvider> {
    vec![provider("ollama", CompatKind::Local), provider("openrouter", CompatKind::Remote), provider("litellm", CompatKind::Gateway)]
}

fn managed(name: &str, verified: bool) -> ManagedUar {
    ManagedUar { endpoints: vec![UarEndpoint { provider: ProviderId::new(name).unwrap(), peer_verified_loopback: verified }], reported_beyond: false }
}

#[test]
fn runtime_ids_are_read_and_written_as_their_text() {
    for text in ["on_device", "compat:ollama", "compat:a.b-c_d", "chatgpt", "claude_code", "codex", "uar"] {
        let runtime = id(text);
        assert_eq!(runtime.to_string(), text);
        assert_eq!(serde_json::to_value(&runtime).unwrap(), json!(text));
        assert_eq!(serde_json::from_value::<RuntimeId>(json!(text)).unwrap(), runtime);
    }
    assert_eq!(id("compat:ollama").kind(), RuntimeKind::Compat);
    assert_eq!(id("compat:ollama").provider().map(ProviderId::as_str), Some("ollama"));
    assert_eq!(RuntimeId::compat("ollama").unwrap(), id("compat:ollama"));
    // OpenCode is cut: an unknown kind, like any other text, can't be held.
    let long = format!("compat:{}", "x".repeat(65));
    for bad in ["opencode", "", "compat:", "compat:a b", "compat:ä", "Chatgpt", " uar", "local", "compat:x\u{0}", long.as_str()] {
        assert!(RuntimeId::parse(bad).is_err(), "{bad:?}");
        assert!(serde_json::from_value::<RuntimeId>(json!(bad)).is_err(), "{bad:?}");
    }
    assert!(serde_json::from_value::<RuntimeId>(json!(5)).is_err());
    assert!(RuntimeId::parse(&"x".repeat(1_000_000)).is_err());
}

#[test]
fn only_compat_and_chatgpt_keep_a_credential() {
    assert_eq!(id("compat:openrouter").credential_entry().as_deref(), Some("ai.compat:openrouter"));
    assert_eq!(id("chatgpt").credential_entry().as_deref(), Some("ai.chatgpt"));
    for none in ["on_device", "claude_code", "codex", "uar"] {
        assert_eq!(id(none).credential_entry(), None, "{none}");
    }
}

#[test]
fn chain_keeps_order_and_drops_duplicates() {
    let chain = Chain::new([on("claude_code"), on("on_device"), ChainEntry::new(id("claude_code"))]).unwrap();
    assert_eq!(chain.ids(), [id("claude_code"), id("on_device")]);
    // The first of the two is kept, on.
    assert!(chain.get(&id("claude_code")).unwrap().enabled());
}

#[test]
fn a_chain_has_at_most_sixteen_runtimes_and_building_one_never_panics() {
    let many: Vec<ChainEntry> = (0..17).map(|n| ChainEntry::new(RuntimeId::compat(&format!("p{n}")).unwrap())).collect();
    assert!(matches!(Chain::new(many.clone()), Err(AiError::TooMany { max: MAX_CHAIN, .. })));
    let sixteen = Chain::new(many.iter().take(MAX_CHAIN).cloned()).unwrap();
    assert_eq!(sixteen.len(), MAX_CHAIN);
    assert!(sixteen.with_entry(on("codex")).is_err(), "a full chain takes no more");
    // Any number of the same runtime is one entry; a list without end is cut off.
    assert_eq!(Chain::new(std::iter::repeat_n(on("uar"), 1_000)).unwrap().len(), 1);
    assert!(Chain::new(std::iter::repeat(on("uar"))).is_err());
}

#[test]
fn a_chain_is_reordered_only_by_a_permutation() {
    let chain = Chain::new([on("on_device"), on("chatgpt"), on("compat:ollama")]).unwrap();
    let order = [id("compat:ollama"), id("on_device"), id("chatgpt")];
    assert_eq!(chain.reordered(&order).unwrap().ids(), order);
    for bad in [
        vec![id("on_device"), id("chatgpt")],
        vec![id("on_device"), id("chatgpt"), id("chatgpt")],
        vec![id("on_device"), id("chatgpt"), id("codex")],
        vec![id("on_device"), id("chatgpt"), id("compat:ollama"), id("codex")],
        vec![],
    ] {
        assert!(chain.reordered(&bad).is_err(), "{bad:?}");
    }
    assert_eq!(chain.ids(), [id("on_device"), id("chatgpt"), id("compat:ollama")], "unchanged");
}

#[test]
fn turning_a_runtime_on_changes_the_chain_only() {
    let chain = Chain::new([ChainEntry::new(id("chatgpt"))]).unwrap();
    let consents = Consents::new();
    let turned_on = chain.with_enabled(&id("chatgpt"), true).unwrap();
    assert!(turned_on.get(&id("chatgpt")).unwrap().enabled());
    assert!(chain.with_enabled(&id("codex"), true).is_err(), "not in the chain");
    // Being on grants nothing.
    let accounts = vec!["avery@lumen.example".to_string()];
    assert!(!may_send(&id("chatgpt"), Where::Leaves, Carries::MailOf(&accounts), &consents));
    assert!(consents.is_empty());
    assert_eq!(turned_on.without(&id("chatgpt")).len(), 0);
}

#[test]
fn entries_take_only_their_own_options() {
    let claude = ChainEntry::new(id("claude_code"));
    let claude = claude.with_model(Some("sonnet")).unwrap().with_path(Some("/Users/avery/.local/bin/claude")).unwrap();
    assert_eq!(claude.model().map(ModelName::as_str), Some("sonnet"));
    assert_eq!(claude.path().map(RuntimePath::as_str), Some("/Users/avery/.local/bin/claude"));
    assert_eq!(claude.instance(), None);
    assert!(ChainEntry::new(id("codex")).with_path(Some("C:\\Tools\\codex.exe")).is_ok());
    assert!(ChainEntry::new(id("on_device")).with_model(Some("x")).is_err());
    assert!(ChainEntry::new(id("compat:ollama")).with_path(Some("/bin/x")).is_err());
    assert!(ChainEntry::new(id("chatgpt")).with_instance(UarInstance::External).is_err());
    assert_eq!(ChainEntry::new(id("uar")).instance(), Some(UarInstance::Managed));
    for bad in ["", "a b", "x".repeat(65).as_str(), "gpt;rm", "ä"] {
        assert!(ChainEntry::new(id("codex")).with_model(Some(bad)).is_err(), "{bad:?}");
    }
    for bad in ["claude", "./claude", "/a/../b", "/a\nb", "\\\\server\\share", "C:claude", ""] {
        assert!(ChainEntry::new(id("codex")).with_path(Some(bad)).is_err(), "{bad:?}");
    }
    assert!(RuntimePath::new(&format!("/{}", "a".repeat(MAX_PATH))).is_err());
}

#[test]
fn a_chain_from_a_file_keeps_what_checks_and_drops_the_rest() {
    let file = json!([
        {"runtime": "claude_code", "enabled": true, "model": "sonnet", "auth": "api_key", "path": "relative/claude"},
        {"runtime": "opencode", "enabled": true, "model": "openai/gpt-5"},
        {"runtime": "claude_code", "enabled": false},
        {"runtime": "codex", "enabled": "yes", "model": "x y"},
        {"runtime": "uar", "enabled": true, "instance": "elsewhere"},
        {"runtime": 5},
        "on_device",
        {"enabled": true},
        {"runtime": "compat:ollama", "enabled": true, "instance": "external", "model": "llama3"},
    ]);
    let chain: Chain = serde_json::from_value(file).unwrap();
    assert_eq!(chain.ids(), [id("claude_code"), id("codex"), id("uar"), id("compat:ollama")]);
    let claude = chain.get(&id("claude_code")).unwrap();
    assert!(claude.enabled() && claude.path().is_none());
    assert_eq!(claude.model().map(ModelName::as_str), Some("sonnet"));
    let codex = chain.get(&id("codex")).unwrap();
    assert!(!codex.enabled() && codex.model().is_none());
    assert_eq!(chain.get(&id("uar")).unwrap().instance(), Some(UarInstance::Managed));
    let compat = chain.get(&id("compat:ollama")).unwrap();
    assert!(compat.instance().is_none() && compat.model().is_none());
    // Written back: no auth, no unknown kind.
    let written = serde_json::to_string(&chain).unwrap();
    assert!(!written.contains("auth") && !written.contains("opencode"), "{written}");
    assert_eq!(serde_json::from_str::<Chain>(&written).unwrap(), chain, "round trip");
    // Not an array: the empty chain. More than sixteen: the first sixteen.
    assert!(Chain::from_value(&json!({"runtime": "codex"})).is_empty());
    let many: Vec<Value> = (0..40).map(|n| json!({"runtime": format!("compat:p{n}"), "enabled": true})).collect();
    assert_eq!(Chain::from_value(&Value::Array(many)).len(), MAX_CHAIN);
}

#[test]
fn scn_rt_where_001_every_runtime_type_gets_its_fixed_where() {
    let providers = providers();
    let local = managed("ollama", true);
    let cases: Vec<(ChainEntry, Option<&ManagedUar>, Where)> = vec![
        (on("on_device"), None, Where::Local),
        (on("compat:ollama"), None, Where::Local),
        (on("compat:openrouter"), None, Where::Leaves),
        (on("compat:litellm"), None, Where::Leaves),
        (on("compat:gone"), None, Where::Leaves),
        (on("chatgpt"), Some(&local), Where::Leaves),
        (on("claude_code"), Some(&local), Where::Leaves),
        (on("codex"), Some(&local), Where::Leaves),
        // Managed UAR with MailCraft's one verified loopback model: local.
        (on("uar"), Some(&local), Where::Local),
        // External UAR, whatever its configuration: leaves.
        (on("uar").with_instance(UarInstance::External).unwrap(), Some(&local), Where::Leaves),
        // Managed without a configuration MailCraft wrote: leaves.
        (on("uar"), None, Where::Leaves),
    ];
    for (entry, uar, want) in &cases {
        assert_eq!(where_of(entry, &providers, *uar), *want, "{}", entry.runtime());
    }
    // Managed with any other model: a remote or gateway provider, an unverified peer, an unknown
    // provider, two endpoints, none, or a model reported beyond the configured one.
    let mut two = managed("ollama", true);
    two.endpoints.push(UarEndpoint { provider: ProviderId::new("ollama").unwrap(), peer_verified_loopback: true });
    let beyond = ManagedUar { reported_beyond: true, ..managed("ollama", true) };
    for config in
        [managed("openrouter", true), managed("litellm", true), managed("ollama", false), managed("gone", true), two, ManagedUar::default(), beyond]
    {
        assert_eq!(where_of(&on("uar"), &providers, Some(&config)), Where::Leaves, "{config:?}");
    }
    // The kinds whose place is fixed say so.
    assert_eq!(RuntimeKind::OnDevice.fixed_where(), Some(Where::Local));
    for kind in [RuntimeKind::ChatGpt, RuntimeKind::ClaudeCode, RuntimeKind::Codex] {
        assert_eq!(kind.fixed_where(), Some(Where::Leaves));
    }
}

#[test]
fn scn_rt_where_001_a_runtime_claiming_local_stays_leaves() {
    // Nothing a run writes reaches `where_of`: whatever Claude Code's output says, its entry and
    // its fixed kind decide. An entry read from a file that says "where": "local" is the same.
    let said_local = ChainEntry::from_value(&json!({"runtime": "claude_code", "enabled": true, "where": "local", "local": true})).unwrap();
    assert_eq!(where_of(&said_local, &providers(), Some(&managed("ollama", true))), Where::Leaves);
    let uar_said_local = ChainEntry::from_value(&json!({"runtime": "uar", "instance": "external", "where": "local"})).unwrap();
    assert_eq!(where_of(&uar_said_local, &providers(), Some(&managed("ollama", true))), Where::Leaves);
}

/// SCN-RT-WHERE-001, negative control: the derivation does not call everything "leaves". A
/// well-formed on-device entry and a compat entry naming a Local provider, built or read from a
/// file, are local; and the managed UAR with its one verified loopback model is local too.
#[test]
fn neg_where_a_well_formed_on_device_or_compat_local_configuration_is_local() {
    let providers = providers();
    let from_file = |value: Value| ChainEntry::from_value(&value).unwrap();
    for entry in [
        on("on_device"),
        on("compat:ollama"),
        from_file(json!({"runtime": "on_device", "enabled": true})),
        from_file(json!({"runtime": "compat:ollama", "enabled": true})),
    ] {
        assert_eq!(where_of(&entry, &providers, None), Where::Local, "{}", entry.runtime());
        assert_eq!(where_of(&entry, &providers, Some(&managed("ollama", true))), Where::Local, "{}", entry.runtime());
    }
    assert_eq!(where_of(&on("uar"), &providers, Some(&managed("ollama", true))), Where::Local);
}

#[test]
fn enabled_runtime_without_consent_may_not_send() {
    let consents = Consents::new().with("Avery@Lumen.example", &id("compat:openrouter"), true).unwrap();
    let accounts = vec!["avery@lumen.example".to_string()];
    let mail = Carries::MailOf(&accounts);
    let chatgpt = where_of(&on("chatgpt"), &providers(), None);
    assert!(!may_send(&id("chatgpt"), chatgpt, mail, &consents));
    assert!(may_send(&id("on_device"), where_of(&on("on_device"), &providers(), None), mail, &consents));
    assert!(may_send(&id("compat:openrouter"), Where::Leaves, mail, &consents));
    // Every account must allow it; mail of no named account never leaves; the owner's words do.
    let two = vec!["avery@lumen.example".to_string(), "blake@lumen.example".to_string()];
    assert!(!may_send(&id("compat:openrouter"), Where::Leaves, Carries::MailOf(&two), &consents));
    assert!(!may_send(&id("compat:openrouter"), Where::Leaves, Carries::MailOf(&[]), &consents));
    assert!(may_send(&id("chatgpt"), Where::Leaves, Carries::OwnerTextOnly, &consents));
    // A yes is taken back.
    let taken_back = consents.with(" AVERY@lumen.example ", &id("compat:openrouter"), false).unwrap();
    assert!(!may_send(&id("compat:openrouter"), Where::Leaves, mail, &taken_back));
    assert!(taken_back.is_empty());
}

#[test]
fn consents_are_per_account_and_runtime_and_read_safely() {
    let file = json!({
        "avery@lumen.example": ["chatgpt", "opencode", 5, null, "compat:openrouter", "chatgpt"],
        "not an address": ["chatgpt"],
        "blake@lumen.example": "chatgpt",
        "casey@lumen.example": ["opencode"],
        "": ["codex"],
    });
    let consents: Consents = serde_json::from_value(file).unwrap();
    let rows: Vec<(String, Vec<String>)> = consents.iter().map(|(a, set)| (a.to_string(), set.iter().map(ToString::to_string).collect())).collect();
    assert_eq!(rows, [("avery@lumen.example".to_string(), vec!["compat:openrouter".to_string(), "chatgpt".to_string()])]);
    assert_eq!(consents.accounts_for(&id("chatgpt")).collect::<Vec<_>>(), ["avery@lumen.example"]);
    assert!(consents.with("nobody", &id("chatgpt"), true).is_err());
    let removed = consents.without_runtime(&id("chatgpt"));
    assert!(!removed.allows("avery@lumen.example", &id("chatgpt")) && removed.allows("avery@lumen.example", &id("compat:openrouter")));
    assert_eq!(serde_json::from_value::<Consents>(serde_json::to_value(&consents).unwrap()).unwrap(), consents, "round trip");
    assert!(Consents::from_value(&json!([1, 2])).is_empty());
    // Caps.
    let mut many = Consents::new();
    for n in 0..MAX_CONSENT_ACCOUNTS {
        many = many.with(&format!("a{n}@lumen.example"), &id("chatgpt"), true).unwrap();
    }
    assert!(matches!(many.with("late@lumen.example", &id("chatgpt"), true), Err(AiError::TooMany { .. })));
}

#[test]
fn fallback_reads_its_three_values() {
    for (text, want) in [("next", Fallback::Next), ("ask", Fallback::Ask), ("stop", Fallback::Stop)] {
        assert_eq!(Fallback::parse(text), Some(want));
        assert_eq!(want.name(), text);
        assert_eq!(serde_json::from_value::<Fallback>(json!(text)).unwrap(), want);
    }
    assert_eq!(Fallback::default(), Fallback::Next);
    assert_eq!(Fallback::parse("never"), None);
    assert!(serde_json::from_value::<Fallback>(json!(1)).is_err());
}

fn junk() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::from),
        "\\PC{0,40}".prop_map(Value::String),
        prop_oneof![Just("on_device"), Just("chatgpt"), Just("claude_code"), Just("codex"), Just("uar"), Just("opencode"), Just("compat:ollama")]
            .prop_map(|s| Value::String(s.to_string())),
    ];
    leaf.prop_recursive(3, 48, 8, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..8).prop_map(Value::Array),
            proptest::collection::btree_map(
                prop_oneof![
                    Just("runtime".to_string()),
                    Just("enabled".to_string()),
                    Just("model".to_string()),
                    Just("path".to_string()),
                    Just("instance".to_string()),
                    "\\PC{0,12}"
                ],
                inner,
                0..6
            )
            .prop_map(|m| Value::Object(m.into_iter().collect())),
        ]
    })
}

proptest! {
    #[test]
    fn hostile_chains_and_consents_load_within_bounds(value in junk()) {
        let chain = Chain::from_value(&value);
        prop_assert!(chain.len() <= MAX_CHAIN);
        let ids = chain.ids();
        for (n, a) in ids.iter().enumerate() {
            prop_assert!(!ids.iter().skip(n + 1).any(|b| a == b), "a runtime twice");
        }
        // No entry read is ever anything but a known kind, so nothing unknown resolves to local.
        for entry in chain.entries() {
            let place = where_of(entry, &[], None);
            prop_assert_eq!(place == Where::Local, entry.runtime() == &RuntimeId::OnDevice);
        }
        let consents = Consents::from_value(&value);
        prop_assert!(consents.iter().count() <= MAX_CONSENT_ACCOUNTS);
        // What is written reads back the same.
        let again: Chain = serde_json::from_value(serde_json::to_value(&chain).unwrap()).unwrap();
        prop_assert_eq!(again, chain);
    }

    #[test]
    fn any_text_parses_or_is_refused(text in "\\PC{0,90}") {
        if let Ok(runtime) = RuntimeId::parse(&text) {
            prop_assert_eq!(runtime.to_string(), text);
        }
    }
}
