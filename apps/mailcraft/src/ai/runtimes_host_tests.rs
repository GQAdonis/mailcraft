// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The desktop's host for the owner's runtime methods (ai-runtime-commands 12.8): an owner's
//! script through the control channel's runtime methods lands in `ai.json` version 3 with the
//! consents as they were; a file a newer MailCraft wrote is never written over; `where_of` is the
//! model's, over the providers set up; sign-out forgets only what MailCraft keeps. Synthetic
//! settings and an in-memory credential store only.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::Mutex;

use mailcraft_ai::{ProviderConfig, ProviderKind};
use mailcraft_host::{SecretError, SecretStore};
use mailcraft_model::ai::{Fallback, ProviderId, RuntimeId, Where};
use mailcraft_ui_egui::control::{AiRuntimes, SignOut, Step, run_runtime};
use serde_json::{Value, json};

use super::runtimes_host::{RuntimesHost, forget_chatgpt, forget_compat_key};
use super::settings::{FILE, Shared};

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

fn file() -> Value {
    json!({
        "version": 3,
        "never_configured": false,
        "fallback": "next",
        "chain": [
            {"runtime": "compat:lumen", "enabled": true},
            {"runtime": "on_device", "enabled": true},
            {"runtime": "chatgpt", "enabled": false},
            {"runtime": "compat:apfel", "enabled": false},
        ],
        "providers": [remote("lumen"), local("apfel")],
        "consents": {"avery@lumen.example": ["compat:lumen"]},
        "keys_kept_for": ["compat:lumen", "chatgpt"],
    })
}

/// A host over `ai.json` in a folder of its own, holding [`file`].
fn host_in(dir: &tempfile::TempDir) -> (RuntimesHost, std::path::PathBuf) {
    let path = dir.path().join(FILE);
    std::fs::write(&path, file().to_string()).unwrap();
    (RuntimesHost::new(Shared::open(Some(path.clone())), None), path)
}

fn on_disk(path: &std::path::Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn an_owners_script_reorders_and_enables_and_it_lands_in_ai_json_with_consents_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let (host, path) = host_in(&dir);
    let consents_before = on_disk(&path)["consents"].clone();
    let order = json!({"order": ["chatgpt", "on_device", "compat:lumen", "compat:apfel"]});
    assert!(matches!(run_runtime(&host, "ai.setRuntimeOrder", &order).unwrap(), Step::Done(_)));
    assert!(matches!(run_runtime(&host, "ai.setRuntimeEnabled", &json!({"runtime": "chatgpt", "value": true})).unwrap(), Step::Done(_)));
    assert!(matches!(run_runtime(&host, "ai.setFallback", &json!({"value": "ask"})).unwrap(), Step::Done(_)));
    let written = on_disk(&path);
    assert_eq!(written["version"], json!(3));
    let chain: Vec<(String, bool)> =
        written["chain"].as_array().unwrap().iter().map(|e| (e["runtime"].as_str().unwrap().to_string(), e["enabled"].as_bool().unwrap())).collect();
    assert_eq!(
        chain,
        [("chatgpt".to_string(), true), ("on_device".to_string(), true), ("compat:lumen".to_string(), true), ("compat:apfel".to_string(), false)]
    );
    assert_eq!(written["fallback"], json!("ask"));
    // Turning a runtime on granted nothing.
    assert_eq!(written["consents"], consents_before);
    assert!(!host.settings().consents.allows("avery@lumen.example", &RuntimeId::ChatGpt));
    // What is kept is what the host hands back.
    assert_eq!(host.settings().fallback, Fallback::Ask);
    // A refused change leaves the file as it was.
    let before = std::fs::read(&path).unwrap();
    assert!(run_runtime(&host, "ai.setRuntimeOrder", &json!({"order": ["chatgpt"]})).is_err());
    assert!(run_runtime(&host, "ai.setRuntimeEnabled", &json!({"runtime": "opencode", "value": true})).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn a_file_a_newer_mailcraft_wrote_is_never_written_over() {
    for version in [json!(99), json!("3")] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        let mut newer = file();
        newer["version"] = version.clone();
        let bytes = newer.to_string();
        std::fs::write(&path, &bytes).unwrap();
        let shared = Shared::open(Some(path.clone()));
        let host = RuntimesHost::new(shared.clone(), None);
        // Read as the safe default: nothing enabled that leaves.
        assert!(host.settings().chain.entries().iter().all(|e| !e.enabled() || host.where_of(e.runtime()) == Where::Local), "{version}");
        let error = host.save(&host.settings()).unwrap_err();
        let said: Value = serde_json::from_str(&error).unwrap();
        assert_eq!(said["kind"], json!("settings"));
        assert!(said["message"].as_str().unwrap().contains("newer version of MailCraft"));
        assert!(shared.change(|s| s.set_active("")).is_err());
        assert!(run_runtime(&host, "ai.setFallback", &json!({"value": "stop"})).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), bytes, "{version}");
        assert!(!dir.path().join("ai.v2.json").exists() && !dir.path().join("ai.json.new").exists());
    }
}

#[test]
fn where_is_the_models_over_the_providers_set_up() {
    let dir = tempfile::tempdir().unwrap();
    let (host, _) = host_in(&dir);
    let compat = |id: &str| RuntimeId::compat(id).unwrap();
    assert_eq!(host.where_of(&RuntimeId::OnDevice), Where::Local);
    assert_eq!(host.where_of(&compat("apfel")), Where::Local);
    assert_eq!(host.where_of(&compat("lumen")), Where::Leaves);
    // A provider that isn't set up is never taken for one on this computer.
    assert_eq!(host.where_of(&compat("nobody")), Where::Leaves);
    for runtime in [RuntimeId::ChatGpt, RuntimeId::ClaudeCode, RuntimeId::Codex, RuntimeId::Uar] {
        assert_eq!(host.where_of(&runtime), Where::Leaves, "{runtime}");
    }
}

/// A credential store in memory.
#[derive(Default)]
struct Memory(Mutex<BTreeMap<String, String>>);

impl SecretStore for Memory {
    fn get(&self, key: &str) -> Option<String> {
        self.0.lock().unwrap().get(key).cloned()
    }

    fn set(&self, key: &str, secret: &str) -> Result<(), SecretError> {
        self.0.lock().unwrap().insert(key.into(), secret.into());
        Ok(())
    }

    fn delete(&self, key: &str) {
        self.0.lock().unwrap().remove(key);
    }
}

#[test]
fn sign_out_forgets_only_what_mailcraft_keeps() {
    let dir = tempfile::tempdir().unwrap();
    let (_, path) = host_in(&dir);
    let shared = Shared::open(Some(path.clone()));
    let store = std::sync::Arc::new(Memory::default());
    for name in ["ai.compat:lumen", "ai-key:lumen", "ai.chatgpt", "ai.compat:apfel", "password:owner@example.org"] {
        store.set(name, "sk-test-not-a-real-key").unwrap();
    }
    forget_compat_key(&shared, store.as_ref(), &ProviderId::new("lumen").unwrap()).unwrap();
    forget_chatgpt(&shared, &crate::ai::runtime::siwc::ChatGptRuntime::new(store.clone())).unwrap();
    let left: Vec<String> = store.0.lock().unwrap().keys().cloned().collect();
    assert_eq!(left, ["ai.compat:apfel", "password:owner@example.org"]);
    assert_eq!(on_disk(&path)["keys_kept_for"], json!([]));
    // Without a store there is nothing to sign out of, and nothing changes.
    let host = RuntimesHost::new(shared, None);
    assert!(host.sign_out(&SignOut::ChatGpt).is_err());
}

// ---- 30.3: sign-out, status, sign-in through the desktop host ----------------------------------

mod wiring {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use mailcraft_ai::runtime::siwc::SiwcStatus;
    use mailcraft_host::SecretStore;
    use mailcraft_model::ai::RuntimeId;
    use mailcraft_ui_egui::control::{AiRuntimes, RuntimeState, SignIn, Step, run_runtime};
    use serde_json::json;

    use crate::ai::runtime::siwc::ChatGptRuntime;
    use crate::ai::runtime::siwc_tests::{FakeOpenAi, FakeStore, ISSUED, OWNER, browser, clock, settings_with_chatgpt, signed_in, tokens};
    use crate::ai::runtimes_host::{Opener, RuntimesHost, TerminalOpener, forget_chatgpt};
    use crate::ai::settings::Shared;

    fn eventually(what: &str, done: impl Fn() -> bool) {
        let until = Instant::now() + Duration::from_secs(20);
        while !done() {
            assert!(Instant::now() < until, "{what} didn't happen");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn shared_with(settings: &crate::ai::settings::AiSettings) -> Shared {
        let shared = Shared::open(None);
        let copy = settings.clone();
        shared
            .change(move |s| {
                *s = copy;
                Ok(())
            })
            .unwrap();
        shared
    }

    fn no_browser() -> Opener {
        Arc::new(|_: &str| false)
    }

    fn no_terminal() -> TerminalOpener {
        Arc::new(|_: &[String]| false)
    }

    fn signed_in_host() -> (RuntimesHost, Arc<ChatGptRuntime>, Arc<FakeStore>, Shared, FakeOpenAi) {
        let server = FakeOpenAi::start(|_, _| tokens("acc.ess-1", "re.fresh-1", 3_600));
        let store = Arc::new(FakeStore::default());
        let mut settings = settings_with_chatgpt();
        settings.set_runtime_consent("other@example.org", &RuntimeId::ChatGpt, true).unwrap();
        let runtime = signed_in(&server, &store, &Arc::default(), &mut settings);
        let shared = shared_with(&settings);
        let dyn_store: Arc<dyn SecretStore> = store.clone();
        let host = RuntimesHost::new(shared.clone(), Some(dyn_store)).with_parts(Some(runtime.clone()), no_browser(), no_terminal());
        (host, runtime, store, shared, server)
    }

    fn assert_signed_out(runtime: &ChatGptRuntime, store: &FakeStore, shared: &Shared) {
        assert_eq!(runtime.status(), SiwcStatus::SignedOut, "signed out in memory");
        assert!(runtime.held_token().is_none(), "the access token was dropped");
        assert!(store.get("ai.chatgpt").is_none(), "the entry is deleted");
        let s = shared.snapshot();
        assert!(!s.chatgpt_key);
        assert!(!s.chain.get(&RuntimeId::ChatGpt).unwrap().enabled(), "the runtime is off");
        assert!(s.consents.accounts_for(&RuntimeId::ChatGpt).next().is_none(), "no account's consent is left");
        assert!(!s.consents.allows(OWNER, &RuntimeId::ChatGpt));
    }

    #[test]
    fn chatgpt_sign_out_turns_the_runtime_off_takes_the_consents_back_and_signs_out_in_memory() {
        let (_, runtime, store, shared, server) = signed_in_host();
        let before = server.total();
        forget_chatgpt(&shared, &runtime).unwrap();
        assert_signed_out(&runtime, &store, &shared);
        assert_eq!(server.total(), before, "nothing is sent to OpenAI");
    }

    #[test]
    fn chatgpt_sign_out_through_the_owners_method_does_the_same() {
        let (host, runtime, store, shared, _server) = signed_in_host();
        assert_eq!(host.status(&RuntimeId::ChatGpt).state, RuntimeState::Ready);
        assert!(matches!(run_runtime(&host, "ai.signOut", &json!({"runtime": "chatgpt"})).unwrap(), Step::Done(_)));
        eventually("the sign-out", || store.get("ai.chatgpt").is_none() && !shared.snapshot().chatgpt_key);
        eventually("the runtime off", || !shared.snapshot().chain.get(&RuntimeId::ChatGpt).unwrap().enabled());
        assert_signed_out(&runtime, &store, &shared);
        assert_eq!(host.status(&RuntimeId::ChatGpt).state, RuntimeState::SignedOut);
    }

    #[test]
    fn status_answers_from_what_is_known_without_waiting() {
        let (host, _runtime, _store, _shared, _server) = signed_in_host();
        for runtime in [RuntimeId::ChatGpt, RuntimeId::ClaudeCode, RuntimeId::Codex, RuntimeId::Uar, RuntimeId::OnDevice] {
            let at = Instant::now();
            let _ = host.status(&runtime);
            assert!(at.elapsed() < Duration::from_millis(250), "{runtime} took {:?}", at.elapsed());
        }
        // Nothing found yet and nothing of Claude Code asked: never "ready" by guess.
        assert_ne!(host.status(&RuntimeId::ClaudeCode).state, RuntimeState::Ready);
    }

    #[test]
    fn chatgpt_sign_in_runs_on_a_thread_and_is_kept() {
        let server = FakeOpenAi::start(|_, _| tokens("acc.ess-1", "re.fresh-1", 3_600));
        let store = Arc::new(FakeStore::default());
        let runtime = Arc::new(ChatGptRuntime::with(store.clone(), server.endpoints(), clock(&Arc::default())));
        let shared = shared_with(&settings_with_chatgpt());
        let (play, seen) = browser(&format!("code=co.de-1&state={{state}}&client_id={ISSUED}"));
        let opened = Arc::new(AtomicUsize::new(0));
        let count = opened.clone();
        let open: Opener = Arc::new(move |url: &str| {
            count.fetch_add(1, Ordering::SeqCst);
            play(url);
            true
        });
        let dyn_store: Arc<dyn SecretStore> = store.clone();
        let host = RuntimesHost::new(shared.clone(), Some(dyn_store)).with_parts(Some(runtime.clone()), open, no_terminal());

        let at = Instant::now();
        host.sign_in(SignIn::ChatGpt).unwrap();
        assert!(at.elapsed() < Duration::from_millis(500), "the window isn't held for the sign-in");
        eventually("the sign-in", || shared.snapshot().chatgpt_key && runtime.status() == SiwcStatus::SignedIn);
        eventually("the end of the sign-in", || host.status(&RuntimeId::ChatGpt).state == RuntimeState::Ready);
        assert_eq!(opened.load(Ordering::SeqCst), 1);
        assert!(seen.lock().unwrap().url.contains("dynamic_agent_client"));
        assert_eq!(store.entry().unwrap().refresh_token.expose(), "re.fresh-1");
        assert!(shared.snapshot().chatgpt_host_id.is_some(), "the host id is kept for the next sign-in");
        let file = shared.snapshot().to_json().to_string();
        assert!(!file.contains("re.fresh-1") && !file.contains("acc.ess-1") && !file.contains(ISSUED));
    }

    #[test]
    fn neg_a_browser_that_cant_be_started_ends_the_chatgpt_sign_in_at_once() {
        let server = FakeOpenAi::start(|_, _| tokens("acc.ess-1", "re.fresh-1", 3_600));
        let store = Arc::new(FakeStore::default());
        let runtime = Arc::new(ChatGptRuntime::with(store.clone(), server.endpoints(), clock(&Arc::default())));
        let shared = shared_with(&settings_with_chatgpt());
        let dyn_store: Arc<dyn SecretStore> = store.clone();
        let host = RuntimesHost::new(shared.clone(), Some(dyn_store)).with_parts(Some(runtime.clone()), no_browser(), no_terminal());
        host.sign_in(SignIn::ChatGpt).unwrap();
        eventually("the sign-in to end", || host.status(&RuntimeId::ChatGpt).state == RuntimeState::SignedOut);
        assert!(store.entries.lock().unwrap().is_empty(), "nothing is kept");
        assert_eq!(server.total(), 0, "nothing was asked of OpenAI");
        assert!(!shared.snapshot().chatgpt_key);
    }

    #[test]
    fn the_owners_cancel_ends_a_chatgpt_sign_in_that_waits_for_the_browser() {
        let server = FakeOpenAi::start(|_, _| tokens("acc.ess-1", "re.fresh-1", 3_600));
        let store = Arc::new(FakeStore::default());
        let runtime = Arc::new(ChatGptRuntime::with(store.clone(), server.endpoints(), clock(&Arc::default())));
        let shared = shared_with(&settings_with_chatgpt());
        // The page opens, and the owner never finishes in the browser.
        let open: Opener = Arc::new(|_: &str| true);
        let dyn_store: Arc<dyn SecretStore> = store.clone();
        let host = RuntimesHost::new(shared.clone(), Some(dyn_store)).with_parts(Some(runtime.clone()), open, no_terminal());
        host.sign_in(SignIn::ChatGpt).unwrap();
        host.cancel_sign_in(SignIn::ChatGpt);
        eventually("the sign-in to end", || host.status(&RuntimeId::ChatGpt).state == RuntimeState::SignedOut);
        assert!(store.entries.lock().unwrap().is_empty(), "nothing is kept");
        assert_eq!(server.total(), 0, "nothing was asked of OpenAI");
        assert!(!shared.snapshot().chatgpt_key);
        // Calling off what isn't under way does nothing.
        host.cancel_sign_in(SignIn::ManagedUar);
        host.cancel_sign_in(SignIn::ChatGpt);
    }

    #[test]
    fn claude_codes_sign_in_only_opens_the_owners_terminal() {
        let opened = Arc::new(Mutex::new(Vec::<Vec<String>>::new()));
        let kept = opened.clone();
        let terminal: TerminalOpener = Arc::new(move |path: &[String]| {
            kept.lock().unwrap().push(path.to_vec());
            true
        });
        let host = RuntimesHost::new(Shared::open(None), None).with_parts(None, no_browser(), terminal);
        match host.sign_in(SignIn::ClaudeCodeTerminal) {
            Ok(()) => eventually("the terminal", || opened.lock().unwrap().len() == 1),
            // No terminal on this computer: the owner is told the command to run, nothing is opened.
            Err(said) => {
                assert!(said.contains("claude auth login"), "{said}");
                assert!(opened.lock().unwrap().is_empty());
            }
        }
        let fixed = crate::ai::runtime::terminal::launches();
        assert!(fixed.iter().all(|l| l.args.iter().any(|a| a.contains("claude auth login")) || l.args.ends_with(&["claude", "auth", "login"])));
    }

    #[test]
    fn codex_is_never_signed_in_by_mailcraft_and_a_managed_uar_isnt_started() {
        let terminal_used = Arc::new(AtomicBool::new(false));
        let used = terminal_used.clone();
        let terminal: TerminalOpener = Arc::new(move |_: &[String]| {
            used.store(true, Ordering::SeqCst);
            true
        });
        let host = RuntimesHost::new(Shared::open(None), None).with_parts(None, no_browser(), terminal);
        let Step::Done(answer) = run_runtime(&host, "ai.signIn", &json!({"runtime": "codex"})).unwrap() else { panic!("no answer") };
        assert_eq!(answer["runYourself"], json!("codex login"));
        assert!(host.sign_in(SignIn::ManagedUar).is_err());
        assert!(host.sign_in(SignIn::ChatGpt).is_err(), "no credential store: no ChatGPT sign-in");
        assert!(!terminal_used.load(Ordering::SeqCst));
    }
}
