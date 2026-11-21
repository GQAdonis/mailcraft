// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `ai.json` version 2 to version 3, every row of spec §3.1 and its precedence rule, the original
//! kept as `ai.v2.json` first and the migration written once, a malformed file, and the keys moved
//! to their new names with a read-back (SCN-RT-MIGRATE-001). Synthetic settings and an in-memory
//! credential store only: never the owner's keychain, never another program's credentials.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::Mutex;

use mailcraft_ai::{ApiKey, ProviderConfig, ProviderKind};
use mailcraft_host::{SecretError, SecretStore};
use mailcraft_model::ai::{ChainEntry, RuntimeId};
use serde_json::{Value, json};

use super::keys::{Kept, Moved, load_from, move_legacy};
use super::settings::{AiSettings, Backup, FILE, Shared, V2_COPY, device_address};
use super::settings_migrate::{migrate, rename_keys};
use super::settings_v3::OnDeviceFirst;

/// Not anybody's key.
const KEY: &str = "sk-test-not-a-real-key-0123456789";
const IDENTITY: &str = "apfel:apple-foundationmodel:0123456789abcdef";

fn local(id: &str) -> ProviderConfig {
    ProviderConfig { id: id.into(), base_url: "http://127.0.0.1:11437".into(), model: "apple-foundationmodel".into(), ..ProviderConfig::default() }
}

fn remote(id: &str, accounts: &[&str]) -> ProviderConfig {
    ProviderConfig {
        id: id.into(),
        kind: ProviderKind::Remote,
        base_url: format!("https://{id}.example.org/v1"),
        model: "remote-model".into(),
        consented_accounts: accounts.iter().map(|a| (*a).to_string()).collect(),
        ..ProviderConfig::default()
    }
}

fn v(provider: &ProviderConfig) -> Value {
    serde_json::to_value(provider).unwrap()
}

/// A version 2 file, as version 2 wrote one.
fn v2(primary: &str, backup: Value, never_configured: bool, providers: &[ProviderConfig]) -> Value {
    json!({
        "version": 2,
        "primary": primary,
        "active": primary,
        "never_configured": never_configured,
        "backup": backup,
        "on_device": {"address": device_address(11437), "identity": IDENTITY, "context_window": 4096},
        "on_device_ports": [11437],
        "providers": providers.iter().map(v).collect::<Vec<_>>(),
        "keys_kept_for": [],
    })
}

fn on(runtime: RuntimeId) -> ChainEntry {
    ChainEntry::new(runtime).with_enabled(true)
}

fn off(runtime: RuntimeId) -> ChainEntry {
    ChainEntry::new(runtime)
}

fn compat(id: &str) -> RuntimeId {
    RuntimeId::compat(id).unwrap()
}

/// What the chain of `settings` is.
fn chain(settings: &AiSettings) -> Vec<ChainEntry> {
    settings.chain.entries().to_vec()
}

/// Written as version 3 and read back: the same.
fn round_trips(settings: &AiSettings) {
    let written = settings.to_json();
    assert_eq!(written["version"], json!(3));
    assert_eq!(&AiSettings::from_json(written.to_string().as_bytes()), settings, "{written}");
}

// ---- Every row of the table ----

#[test]
fn primary_and_the_confirmed_on_device_backup_keep_their_order() {
    let settings = migrate(&v2("p", json!("apple-on-device"), false, &[remote("p", &[])]));
    assert_eq!(chain(&settings), [on(compat("p")), on(RuntimeId::OnDevice)]);
    // Never moved first by itself: that is the owner's one-time question.
    assert_eq!(settings.on_device_first, OnDeviceFirst::Unasked);
    assert_eq!((settings.active.as_str(), settings.backup, settings.never_configured), ("p", Backup::AppleOnDevice, false));
    assert_eq!(settings.backup_binding().map(|b| b.identity.as_str()), Some(IDENTITY));
    round_trips(&settings);
}

#[test]
fn primary_and_a_provider_backup_are_both_on_in_that_order() {
    let settings = migrate(&v2("p", json!("b"), false, &[remote("p", &[]), remote("b", &[]), remote("other", &[])]));
    assert_eq!(chain(&settings), [on(compat("p")), on(compat("b"))]);
    assert_eq!(settings.active, "p");
    round_trips(&settings);
}

#[test]
fn neg_primary_with_no_backup_is_the_one_entry() {
    for backup in [Value::Null, json!(""), json!("none"), json!("a later version's backup"), json!(7)] {
        let settings = migrate(&v2("p", backup.clone(), false, &[remote("p", &[]), remote("q", &[])]));
        assert_eq!(chain(&settings), [on(compat("p"))], "{backup}");
        round_trips(&settings);
    }
    // And with the member absent altogether.
    let mut absent = v2("p", Value::Null, false, &[remote("p", &[])]);
    absent.as_object_mut().unwrap().remove("backup");
    assert_eq!(chain(&migrate(&absent)), [on(compat("p"))]);
}

#[test]
fn the_owners_none_with_a_provider_backup_puts_the_backup_first_and_wins() {
    // Precedence: this row wins over "None, no provider backup".
    let settings = migrate(&v2("", json!("b"), false, &[remote("p", &[]), remote("b", &[])]));
    assert_eq!(chain(&settings), [on(compat("b")), off(compat("p"))]);
    assert_eq!((settings.active.as_str(), settings.never_configured), ("b", false));
    assert!(!settings.explicitly_none());
    round_trips(&settings);
}

#[test]
fn the_owners_none_without_a_provider_backup_turns_every_entry_off() {
    for backup in [json!("apple-on-device"), json!(""), Value::Null] {
        let settings = migrate(&v2("", backup.clone(), false, &[remote("p", &[]), local("l")]));
        assert_eq!(chain(&settings), [off(compat("p")), off(compat("l"))], "{backup}");
        assert!(settings.explicitly_none() && !settings.never_configured);
        assert!(settings.chain.entries().iter().all(|e| !e.enabled()));
        assert_eq!((settings.backup, settings.backup_binding()), (Backup::None, None));
        round_trips(&settings);
    }
}

#[test]
fn never_configured_stays_so_with_the_on_device_model_and_nothing_else() {
    let settings = migrate(&v2("", Value::Null, true, &[]));
    assert!(settings.never_configured);
    assert_eq!(chain(&settings), [on(RuntimeId::OnDevice)]);
    assert_eq!(settings.chain, AiSettings::default().chain);
    round_trips(&settings);
    // Version 1 (no version, no never_configured) with nothing set up reads the same.
    assert_eq!(migrate(&json!({"providers": []})), AiSettings::default());
}

#[test]
fn each_providers_consents_become_its_runtimes() {
    let settings = migrate(&v2(
        "r",
        Value::Null,
        false,
        &[remote("r", &["Owner@Example.org", "not an address", "second@example.org"]), remote("s", &["owner@example.org"]), {
            // A provider on this computer has no consent to carry.
            ProviderConfig { consented_accounts: vec!["owner@example.org".into()], ..local("l") }
        }],
    ));
    let consents: Vec<(String, Vec<String>)> =
        settings.consents.iter().map(|(a, set)| (a.to_string(), set.iter().map(ToString::to_string).collect())).collect();
    assert_eq!(
        consents,
        [
            ("owner@example.org".to_string(), vec!["compat:r".to_string(), "compat:s".to_string()]),
            ("second@example.org".to_string(), vec!["compat:r".to_string()])
        ]
    );
    assert!(settings.consents.allows("owner@example.org", &compat("r")));
    assert!(!settings.consents.allows("owner@example.org", &compat("l")));
    // The code that still asks a provider sees the same.
    assert_eq!(settings.provider("r").unwrap().consented_accounts, ["owner@example.org", "second@example.org"]);
    assert!(settings.provider("l").unwrap().consented_accounts.is_empty());
    round_trips(&settings);
}

#[test]
fn kept_keys_are_noted_under_their_old_names_and_still_read() {
    let mut file = v2("r", Value::Null, false, &[remote("r", &[])]);
    file["keys_kept_for"] = json!(["r", "gone-provider", "bad id!"]);
    let settings = migrate(&file);
    assert_eq!(settings.legacy_keys, ["r", "gone-provider"]);
    assert!(settings.noted.is_empty());
    assert!(settings.key_kept("r"));
    assert_eq!(settings.to_json()["keys_kept_for"], json!(["ai-key:r", "ai-key:gone-provider"]));
    round_trips(&settings);
}

#[test]
fn the_migration_makes_no_host_id() {
    let mut file = v2("p", Value::Null, false, &[remote("p", &[])]);
    // Not even one planted in a version 2 file.
    file["chatgpt"] = json!({"ext_agent_host_id": "A".repeat(43)});
    let settings = migrate(&file);
    assert_eq!(settings.chatgpt_host_id, None);
    assert!(settings.to_json().get("chatgpt").is_none());
}

#[test]
fn a_malformed_version_2_file_is_no_settings_and_never_a_panic() {
    let junk: [&[u8]; 9] = [
        b"",
        b"{\"version\": 2, \"providers\": [",
        b"\xff\xfe\x00",
        b"{\"version\": 2, \"providers\": 7, \"primary\": [1], \"backup\": {}, \"never_configured\": \"yes\"}",
        b"{\"version\": 2, \"keys_kept_for\": 3, \"on_device\": [], \"on_device_ports\": \"x\"}",
        b"{\"version\": 1, \"active\": null, \"providers\": [null, 1, \"x\", {\"id\": 5}]}",
        b"[1, 2]",
        b"null",
        b"{\"version\": 2, \"primary\": \"x\", \"providers\": [{\"id\": \"x\", \"kind\": \"opencode\", \"base_url\": \"http://127.0.0.1:1\", \"model\": \"m\"}]}",
    ];
    for bytes in junk {
        let settings = AiSettings::from_json(bytes);
        assert!(settings.providers.is_empty() && settings.active.is_empty(), "{:?}", String::from_utf8_lossy(bytes));
        assert!(settings.chain.entries().iter().all(|e| e.runtime() == &RuntimeId::OnDevice), "nothing that leaves");
        round_trips(&settings);
    }
}

// ---- Written once, the original first ----

#[test]
fn a_well_formed_version_2_file_is_migrated_once_kept_as_ai_v2_json_and_used() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(FILE);
    let original = serde_json::to_vec_pretty(&v2("p", json!("apple-on-device"), false, &[remote("p", &["owner@example.org"])])).unwrap();
    std::fs::write(&path, &original).unwrap();

    let shared = Shared::open(Some(path.clone()));
    // The original is kept, as it was.
    assert_eq!(std::fs::read(dir.path().join(V2_COPY)).unwrap(), original);
    // The file is version 3 now, and holds no key.
    let written: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(written["version"], json!(3));
    assert!(written.get("primary").is_none() && written.get("backup").is_none());
    // The migrated settings are used.
    let now = shared.snapshot();
    assert_eq!(now.active().map(|p| p.id.as_str()), Some("p"));
    assert_eq!(now.backup_binding().map(|b| b.address.clone()), Some(device_address(11437)));
    assert_eq!(now.provider("p").unwrap().consented_accounts, ["owner@example.org"]);

    // Opened again: read as version 3, nothing migrated twice, the copy untouched.
    std::fs::write(dir.path().join(V2_COPY), b"the copy, as the first migration left it").unwrap();
    let again = Shared::open(Some(path.clone()));
    assert_eq!(again.snapshot(), now);
    assert_eq!(std::fs::read(dir.path().join(V2_COPY)).unwrap(), b"the copy, as the first migration left it");
    assert!(!dir.path().join("ai.json.new").exists() && !dir.path().join("ai.v2.json.new").exists());
}

#[test]
fn a_copy_left_by_an_earlier_try_is_kept_as_the_original() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(FILE);
    std::fs::write(dir.path().join(V2_COPY), b"the original").unwrap();
    std::fs::write(&path, serde_json::to_vec(&v2("p", Value::Null, false, &[remote("p", &[])])).unwrap()).unwrap();
    Shared::open(Some(path.clone()));
    assert_eq!(std::fs::read(dir.path().join(V2_COPY)).unwrap(), b"the original");
    assert_eq!(serde_json::from_slice::<Value>(&std::fs::read(&path).unwrap()).unwrap()["version"], json!(3));
}

#[test]
fn nothing_is_written_over_version_2_until_its_copy_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(FILE);
    let original = serde_json::to_vec(&v2("p", Value::Null, false, &[remote("p", &[])])).unwrap();
    std::fs::write(&path, &original).unwrap();
    // Something that isn't a file is in the copy's place.
    std::fs::create_dir(dir.path().join(V2_COPY)).unwrap();
    let shared = Shared::open(Some(path.clone()));
    assert_eq!(std::fs::read(&path).unwrap(), original, "version 2 is still there");
    assert_eq!(shared.snapshot().active, "p", "the migrated settings are used meanwhile");
    assert!(shared.change(|s| s.set_active("")).is_err(), "and nothing is written over the original");
    assert_eq!(std::fs::read(&path).unwrap(), original);
    // Once the copy can be kept, the next change writes it first, then version 3.
    std::fs::remove_dir(dir.path().join(V2_COPY)).unwrap();
    shared.change(|s| s.set_active("")).unwrap();
    assert_eq!(std::fs::read(dir.path().join(V2_COPY)).unwrap(), original);
    assert_eq!(serde_json::from_slice::<Value>(&std::fs::read(&path).unwrap()).unwrap()["version"], json!(3));
}

#[test]
fn a_malformed_file_is_not_written_over() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(FILE);
    std::fs::write(&path, b"{\"version\": 2, \"providers\": [").unwrap();
    let shared = Shared::open(Some(path.clone()));
    assert_eq!(shared.snapshot(), AiSettings::default());
    assert_eq!(std::fs::read(&path).unwrap(), b"{\"version\": 2, \"providers\": [");
    assert!(!dir.path().join(V2_COPY).exists());
}

// ---- Keys: copied, read back, only then the old one taken away ----

/// A credential store in memory, which can be told to give back something else for one name, or
/// to refuse to take one away.
#[derive(Default)]
struct Store {
    entries: Mutex<BTreeMap<String, String>>,
    garbled: Mutex<Option<String>>,
    stuck: Mutex<Option<String>>,
    /// Run on every read, with the name read (to save a key "while" a move runs).
    on_read: Mutex<Option<Box<dyn Fn(&Store, &str) + Send>>>,
}

impl Store {
    fn with(entries: &[(&str, &str)]) -> Store {
        let store = Store::default();
        store.entries.lock().unwrap().extend(entries.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())));
        store
    }

    fn names(&self) -> Vec<String> {
        self.entries.lock().unwrap().keys().cloned().collect()
    }

    fn value(&self, name: &str) -> Option<String> {
        self.entries.lock().unwrap().get(name).cloned()
    }
}

impl SecretStore for Store {
    fn get(&self, key: &str) -> Option<String> {
        let kept = self.entries.lock().unwrap().get(key).cloned();
        if self.garbled.lock().unwrap().as_deref() == Some(key) {
            return kept.map(|_| "something else".into());
        }
        if let Some(hook) = self.on_read.lock().unwrap().as_ref() {
            hook(self, key);
        }
        kept
    }

    fn set(&self, key: &str, secret: &str) -> Result<(), SecretError> {
        self.entries.lock().unwrap().insert(key.to_string(), secret.to_string());
        Ok(())
    }

    fn delete(&self, key: &str) {
        self.entries.lock().unwrap().remove(key);
    }

    fn remove(&self, key: &str) -> Result<(), SecretError> {
        if self.stuck.lock().unwrap().as_deref() == Some(key) {
            return Err(SecretError::Refused);
        }
        self.delete(key);
        Ok(())
    }
}

/// Settings migrated from a file whose `keys_kept_for` names `r`, kept nowhere.
fn with_old_key() -> Shared {
    let shared = Shared::open(None);
    let mut file = v2("r", Value::Null, false, &[remote("r", &["owner@example.org"])]);
    file["keys_kept_for"] = json!(["r"]);
    let migrated = migrate(&file);
    shared
        .change(|s| {
            *s = migrated;
            Ok(())
        })
        .unwrap();
    shared
}

#[test]
fn a_key_is_moved_to_its_new_name_after_it_reads_back() {
    let store = Store::with(&[("ai-key:r", KEY), ("password:owner@example.org", "not touched")]);
    let shared = with_old_key();
    rename_keys(&shared, &store);
    assert_eq!(store.names(), ["ai.compat:r", "password:owner@example.org"]);
    assert_eq!(store.value("ai.compat:r").as_deref(), Some(KEY));
    let now = shared.snapshot();
    assert_eq!((now.noted.clone(), now.legacy_keys.clone()), (vec!["r".to_string()], Vec::new()));
    assert_eq!(now.kept_key("r"), Kept::New);
    assert_eq!(now.to_json()["keys_kept_for"], json!(["compat:r"]));
    assert!(now.key_kept("r"));
    assert_eq!(load_from(&store, "r", shared.snapshot().legacy_keys.contains(&"r".to_string())).unwrap(), Some(ApiKey::new(KEY).unwrap()));
    // Nothing of the key in the settings or their Debug.
    assert!(!now.to_json().to_string().contains(KEY) && !format!("{now:?}").contains(KEY));
    // Done is done: a second run changes nothing.
    rename_keys(&shared, &store);
    assert_eq!(store.names(), ["ai.compat:r", "password:owner@example.org"]);
}

#[test]
fn a_key_whose_copy_doesnt_read_back_stays_under_its_old_name_and_still_works() {
    let store = Store::with(&[("ai-key:r", KEY)]);
    *store.garbled.lock().unwrap() = Some("ai.compat:r".into());
    let shared = with_old_key();
    rename_keys(&shared, &store);
    // The old one is kept; the copy that didn't read back is taken away again.
    assert_eq!(store.value("ai-key:r").as_deref(), Some(KEY));
    assert_eq!(store.value("ai.compat:r"), None);
    // keys_kept_for names both until the old one is gone.
    let now = shared.snapshot();
    assert_eq!(now.to_json()["keys_kept_for"], json!(["compat:r", "ai-key:r"]));
    assert!(now.key_kept("r"));
    // The key still works on the next start: read from its old name, which requests are told of.
    assert_eq!(now.kept_key("r"), Kept::Either);
    *store.garbled.lock().unwrap() = None;
    assert_eq!(load_from(&store, "r", shared.snapshot().legacy_keys.contains(&"r".to_string())).unwrap(), Some(ApiKey::new(KEY).unwrap()));
    // And the next start moves it.
    rename_keys(&shared, &store);
    assert_eq!(store.names(), ["ai.compat:r"]);
    assert_eq!(shared.snapshot().to_json()["keys_kept_for"], json!(["compat:r"]));
}

#[test]
fn an_old_name_that_cant_be_taken_away_is_noted_until_it_can() {
    let store = Store::with(&[("ai-key:r", KEY)]);
    *store.stuck.lock().unwrap() = Some("ai-key:r".into());
    assert_eq!(move_legacy(&store, "r").unwrap(), Moved::OldKept);
    let shared = with_old_key();
    rename_keys(&shared, &store);
    assert_eq!(shared.snapshot().to_json()["keys_kept_for"], json!(["compat:r", "ai-key:r"]));
    *store.stuck.lock().unwrap() = None;
    rename_keys(&shared, &store);
    assert_eq!(store.names(), ["ai.compat:r"]);
    assert_eq!(shared.snapshot().to_json()["keys_kept_for"], json!(["compat:r"]));
}

#[test]
fn an_old_name_with_nothing_under_it_is_no_longer_noted() {
    let store = Store::default();
    let shared = with_old_key();
    rename_keys(&shared, &store);
    assert!(store.names().is_empty(), "nothing is made up");
    let now = shared.snapshot();
    assert!(now.legacy_keys.is_empty() && now.noted.is_empty() && !now.key_kept("r"));
}

#[test]
fn a_key_already_under_its_new_name_is_not_overwritten_by_the_old_one() {
    // The owner saved a key since: it is noted under its new name alone, and the old one is
    // never moved over it.
    let store = Store::with(&[("ai-key:r", KEY), ("ai.compat:r", "sk-test-a-newer-key-9876543210")]);
    let shared = with_old_key();
    shared
        .change(|s| {
            s.note_key("r");
            s.drop_legacy_key("r");
            Ok(())
        })
        .unwrap();
    rename_keys(&shared, &store);
    assert_eq!(store.value("ai.compat:r").as_deref(), Some("sk-test-a-newer-key-9876543210"));
    assert_eq!(
        load_from(&store, "r", shared.snapshot().legacy_keys.contains(&"r".to_string())).unwrap(),
        Some(ApiKey::new("sk-test-a-newer-key-9876543210").unwrap())
    );
}

#[test]
fn forgetting_a_key_takes_both_names_away() {
    let store = Store::with(&[("ai-key:r", KEY), ("ai.compat:r", KEY)]);
    super::keys::forget(&store, "r").unwrap();
    assert!(store.names().is_empty());
}

#[test]
fn no_claude_code_or_codex_key_is_ever_asked_for() {
    let mut file = v2("p", Value::Null, false, &[remote("p", &[])]);
    file["keys_kept_for"] = json!(["p", "claude_code", "codex"]);
    let settings = migrate(&file);
    // A version 2 file named providers; one called "codex" would be a provider, never Codex the
    // runtime, and its key keeps its provider's name.
    assert!(settings.legacy_keys.iter().all(|id| super::keys::legacy_entry_name(id).unwrap().starts_with("ai-key:")));
    let written = settings.to_json().to_string();
    assert!(!written.contains("\"claude_code\"") && !written.contains("\"codex\""), "{written}");
    assert_eq!(RuntimeId::ClaudeCode.credential_entry(), None);
    assert_eq!(RuntimeId::Codex.credential_entry(), None);
}

// ---- A key saved before or while a move runs is never overwritten ----

const NEWER: &str = "sk-test-a-newer-key-9876543210";

#[test]
fn a_key_saved_before_the_move_wins_and_the_old_one_goes() {
    let store = Store::with(&[("ai-key:r", KEY), ("ai.compat:r", NEWER)]);
    assert_eq!(move_legacy(&store, "r").unwrap(), Moved::Done);
    assert_eq!(store.names(), ["ai.compat:r"]);
    assert_eq!(store.value("ai.compat:r").as_deref(), Some(NEWER));
}

#[test]
fn a_key_saved_while_the_move_reads_wins_and_the_old_one_goes() {
    let store = Store::with(&[("ai-key:r", KEY)]);
    // The owner's save lands between the move's read of the old name and its look at the new one.
    *store.on_read.lock().unwrap() = Some(Box::new(|store: &Store, key: &str| {
        if key == "ai-key:r" {
            store.entries.lock().unwrap().insert("ai.compat:r".into(), NEWER.into());
        }
    }));
    let shared = with_old_key();
    rename_keys(&shared, &store);
    *store.on_read.lock().unwrap() = None;
    assert_eq!(store.names(), ["ai.compat:r"]);
    assert_eq!(store.value("ai.compat:r").as_deref(), Some(NEWER));
    assert_eq!(shared.snapshot().to_json()["keys_kept_for"], json!(["compat:r"]));
}

#[test]
fn a_save_waits_for_a_move_under_way_and_then_wins() {
    use std::sync::Arc;
    use std::sync::mpsc::channel;
    let store = Arc::new(Store::with(&[("ai-key:r", KEY)]));
    let (entered, started) = channel::<()>();
    let (release, gate) = channel::<()>();
    let gate = Mutex::new(gate);
    let entered = Mutex::new(entered);
    // The move stops on its first read until the test lets it go.
    *store.on_read.lock().unwrap() = Some(Box::new(move |_: &Store, key: &str| {
        if key == "ai-key:r" {
            let _ = entered.lock().unwrap().send(());
            let _ = gate.lock().unwrap().recv();
        }
    }));
    let mover = {
        let store = store.clone();
        std::thread::spawn(move || move_legacy(store.as_ref(), "r"))
    };
    started.recv().unwrap();
    let saver = {
        let store = store.clone();
        std::thread::spawn(move || super::keys::save(store.as_ref(), "r", &ApiKey::new(NEWER).unwrap()))
    };
    // The save can't land while the move holds the keys.
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert_eq!(store.value("ai.compat:r"), None, "the save waited");
    release.send(()).unwrap();
    // The move reads the old name again to see it gone: with the gate closed, that read passes.
    drop(release);
    assert_eq!(mover.join().unwrap().unwrap(), Moved::Done);
    saver.join().unwrap().unwrap();
    *store.on_read.lock().unwrap() = None;
    assert_eq!(store.names(), ["ai.compat:r"]);
    assert_eq!(store.value("ai.compat:r").as_deref(), Some(NEWER), "the newer key survived");
}
