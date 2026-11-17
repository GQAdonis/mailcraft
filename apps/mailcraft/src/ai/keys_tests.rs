// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A provider's key goes to the credential store under its provider's name and nowhere else, and
//! is never shown: not in `Debug`, an error, or the provider's saved settings (SCN-OPTIN-001).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mailcraft_ai::{ApiKey, ProviderConfig, ProviderKind};
use mailcraft_host::{SecretError, SecretStore};

use super::keys::{Broker, KeyError, PREFIX, READ_WAIT, entry_name, forget, load, save, with_key};

/// Not anybody's key.
const KEY: &str = "sk-test-not-a-real-key-0123456789";

/// A credential store in memory.
#[derive(Default)]
struct Memory {
    entries: Mutex<BTreeMap<String, String>>,
    refuse: bool,
}

impl SecretStore for Memory {
    fn get(&self, key: &str) -> Option<String> {
        self.entries.lock().unwrap().get(key).cloned()
    }

    fn set(&self, key: &str, secret: &str) -> Result<(), SecretError> {
        if self.refuse {
            return Err(SecretError::Refused);
        }
        self.entries.lock().unwrap().insert(key.to_string(), secret.to_string());
        Ok(())
    }

    fn delete(&self, key: &str) {
        self.entries.lock().unwrap().remove(key);
    }
}

fn provider(id: &str) -> ProviderConfig {
    ProviderConfig {
        id: id.into(),
        kind: ProviderKind::Remote,
        base_url: "https://api.example.org/v1".into(),
        model: "remote-model".into(),
        consented_accounts: vec!["owner@example.org".into()],
        ..ProviderConfig::default()
    }
}

#[test]
fn a_key_is_filed_under_its_provider_and_comes_back() {
    let store = Memory::default();
    let key = ApiKey::new(KEY).unwrap();
    save(&store, "mailcraft-boundary-20261116a", &key).unwrap();
    assert_eq!(entry_name("mailcraft-boundary-20261116a").unwrap(), "ai-key:mailcraft-boundary-20261116a");
    let entries = store.entries.lock().unwrap().clone();
    assert_eq!(entries.keys().collect::<Vec<_>>(), ["ai-key:mailcraft-boundary-20261116a"]);
    assert!(entries.keys().all(|name| name.starts_with(PREFIX) && !name.contains(KEY)));
    assert_eq!(load(&store, "mailcraft-boundary-20261116a").unwrap(), Some(key.clone()));
    assert_eq!(load(&store, "another-provider").unwrap(), None);
    // Clean-up takes it away by the provider's name.
    forget(&store, "mailcraft-boundary-20261116a").unwrap();
    assert!(store.entries.lock().unwrap().is_empty());
    assert_eq!(load(&store, "mailcraft-boundary-20261116a").unwrap(), None);
}

#[test]
fn a_provider_name_that_could_reach_another_entry_names_no_key() {
    let store = Memory::default();
    let key = ApiKey::new(KEY).unwrap();
    for id in ["", "password:owner@example.org", "a/b", "a b", "x#0", &"x".repeat(65)] {
        assert_eq!(entry_name(id), Err(KeyError::BadProvider), "{id:?}");
        assert_eq!(save(&store, id, &key), Err(KeyError::BadProvider));
        assert_eq!(load(&store, id), Err(KeyError::BadProvider));
        assert_eq!(forget(&store, id), Err(KeyError::BadProvider));
    }
    assert!(store.entries.lock().unwrap().is_empty());
}

#[test]
fn the_key_is_never_shown_or_saved_with_the_settings() {
    let store = Memory::default();
    save(&store, "remote-1", &ApiKey::new(KEY).unwrap()).unwrap();
    let store = shared(store);
    let config = with_key(provider("remote-1"), true, &Broker::new(READ_WAIT), Some(&store)).unwrap();
    assert_eq!(config.api_key.as_ref().map(ApiKey::expose), Some(KEY));
    // Debug of the key, of the settings holding it, and of an error.
    let shown = format!("{:?} {config:?} {:?}", config.api_key, KeyError::Store(SecretError::Other("the store said no")));
    assert!(!shown.contains(KEY) && shown.contains("ApiKey(***)"), "{shown}");
    // What is saved of the settings (`ui.json`) has no key.
    let saved = serde_json::to_string(&config).unwrap();
    assert!(!saved.contains(KEY), "{saved}");
    // Errors in words hold no key either.
    for error in [KeyError::BadProvider, KeyError::Damaged, KeyError::Store(SecretError::Refused)] {
        assert!(!format!("{error} {error:?}").contains(KEY));
    }
}

#[test]
fn a_store_that_refuses_or_holds_something_else_is_an_error_not_a_key() {
    let refusing = Memory { refuse: true, ..Memory::default() };
    assert_eq!(save(&refusing, "remote-1", &ApiKey::new(KEY).unwrap()), Err(KeyError::Store(SecretError::Refused)));
    let damaged = Memory::default();
    damaged.entries.lock().unwrap().insert("ai-key:remote-1".into(), "not a key\nat all".into());
    assert_eq!(load(&damaged, "remote-1"), Err(KeyError::Damaged));
    let damaged = shared(damaged);
    assert_eq!(with_key(provider("remote-1"), true, &Broker::new(READ_WAIT), Some(&damaged)).map(|c| c.api_key.is_some()), Err(KeyError::Damaged));
}

#[test]
fn without_a_kept_key_the_one_in_memory_stays() {
    let in_memory = ProviderConfig { api_key: Some(ApiKey::new(KEY).unwrap()), ..provider("remote-1") };
    let empty = shared(Memory::default());
    let broker = Broker::new(READ_WAIT);
    assert!(with_key(in_memory.clone(), true, &broker, Some(&empty)).unwrap().api_key.is_some());
    assert!(with_key(in_memory, true, &broker, None).unwrap().api_key.is_some());
    assert!(with_key(provider("remote-1"), true, &broker, Some(&empty)).unwrap().api_key.is_none());
}

fn shared(store: impl SecretStore + 'static) -> Arc<dyn SecretStore> {
    Arc::new(store)
}

/// A store that counts its reads and answers each only when the test lets it (or never).
struct Slow {
    reads: AtomicUsize,
    gate: Mutex<Receiver<()>>,
    key: Option<&'static str>,
}

fn slow(key: Option<&'static str>) -> (Arc<Slow>, Sender<()>) {
    let (tx, rx) = channel();
    (Arc::new(Slow { reads: AtomicUsize::new(0), gate: Mutex::new(rx), key }), tx)
}

impl SecretStore for Slow {
    fn get(&self, _key: &str) -> Option<String> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let _ = self.gate.lock().unwrap().recv();
        self.key.map(str::to_string)
    }

    fn set(&self, _key: &str, _secret: &str) -> Result<(), SecretError> {
        Ok(())
    }

    fn delete(&self, _key: &str) {}
}

fn dyn_store(store: &Arc<Slow>) -> Arc<dyn SecretStore> {
    store.clone()
}

/// `broker`'s read once its thread is free (it lets go of a read just after answering it).
fn when_free(broker: &Broker, store: &Arc<dyn SecretStore>, id: &str) -> Option<ApiKey> {
    let started = Instant::now();
    loop {
        match broker.read(store, id) {
            Ok(key) => return key,
            Err(KeyError::Busy) if started.elapsed() < Duration::from_secs(5) => std::thread::sleep(Duration::from_millis(10)),
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn a_store_that_never_answers_gives_up_within_six_seconds_and_adds_no_thread() {
    let (store, gate) = slow(Some(KEY));
    let broker = Broker::new(READ_WAIT);
    let started = Instant::now();
    assert_eq!(broker.read(&dyn_store(&store), "remote-1"), Err(KeyError::TimedOut));
    let waited = started.elapsed();
    assert!(waited >= Duration::from_secs(4) && waited < Duration::from_secs(6), "{waited:?}");
    // The one read is still under way: every other fails at once.
    let again = Instant::now();
    assert_eq!(broker.read(&dyn_store(&store), "remote-1"), Err(KeyError::Busy));
    assert_eq!(broker.read(&dyn_store(&store), "remote-2"), Err(KeyError::Busy));
    assert!(again.elapsed() < Duration::from_millis(500));
    assert_eq!(store.reads.load(Ordering::SeqCst), 1);
    drop(gate);
}

#[test]
fn a_key_that_comes_late_is_used_by_the_next_request_without_reading_again() {
    let (store, gate) = slow(Some(KEY));
    let broker = Broker::new(Duration::from_millis(200));
    assert_eq!(broker.read(&dyn_store(&store), "remote-1"), Err(KeyError::TimedOut));
    // The owner answers the system's question: the read finishes after its request gave up.
    gate.send(()).unwrap();
    let started = Instant::now();
    let key = loop {
        match broker.read(&dyn_store(&store), "remote-1") {
            Ok(Some(key)) => break key,
            Err(KeyError::Busy) if started.elapsed() < Duration::from_secs(5) => std::thread::sleep(Duration::from_millis(20)),
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(key.expose(), KEY);
    assert_eq!(store.reads.load(Ordering::SeqCst), 1, "kept in memory, not read again");
    assert!(!format!("{broker:?}").contains(KEY), "never shown");
    // The owner gave another key (or forgot it): the one in memory goes, and the store is asked again.
    broker.forget_late("remote-1");
    gate.send(()).unwrap();
    let again = when_free(&broker, &dyn_store(&store), "remote-1");
    assert_eq!(again.map(|k| k.expose().to_string()), Some(KEY.to_string()));
    assert_eq!(store.reads.load(Ordering::SeqCst), 2);
}

#[test]
fn ask_again_reads_once_the_thread_is_free() {
    let (store, gate) = slow(None);
    let broker = Broker::new(Duration::from_millis(100));
    assert_eq!(broker.read(&dyn_store(&store), "remote-1"), Err(KeyError::TimedOut));
    gate.send(()).unwrap();
    // Nothing came (the owner said no): nothing is kept, and the next read asks the store again.
    let started = Instant::now();
    loop {
        gate.send(()).unwrap();
        match broker.read(&dyn_store(&store), "remote-1") {
            Ok(None) => break,
            Err(KeyError::Busy | KeyError::TimedOut) if started.elapsed() < Duration::from_secs(5) => std::thread::sleep(Duration::from_millis(20)),
            other => panic!("{other:?}"),
        }
    }
    assert!(store.reads.load(Ordering::SeqCst) >= 2);
}

#[test]
fn only_a_noted_key_of_a_provider_elsewhere_is_ever_read() {
    let (store, gate) = slow(Some(KEY));
    let broker = Broker::new(Duration::from_millis(100));
    // Not noted: not read.
    assert!(with_key(provider("remote-1"), false, &broker, Some(&dyn_store(&store))).unwrap().api_key.is_none());
    // On this computer: no key, noted or not.
    let local = ProviderConfig { kind: ProviderKind::Local, base_url: "http://127.0.0.1:11434".into(), ..provider("local-1") };
    assert!(with_key(local, true, &broker, Some(&dyn_store(&store))).unwrap().api_key.is_none());
    assert_eq!(store.reads.load(Ordering::SeqCst), 0);
    // A gateway's noted key is read, like a provider elsewhere's.
    gate.send(()).unwrap();
    let gateway = ProviderConfig { kind: ProviderKind::Gateway, base_url: "http://127.0.0.1:4000".into(), ..provider("gateway-1") };
    let read = with_key(gateway, true, &Broker::new(READ_WAIT), Some(&dyn_store(&store))).unwrap();
    assert_eq!(read.api_key.as_ref().map(ApiKey::expose), Some(KEY));
    assert_eq!(store.reads.load(Ordering::SeqCst), 1);
}

#[test]
fn a_store_that_panics_is_an_error_and_the_thread_goes_on() {
    struct Panicking;
    impl SecretStore for Panicking {
        fn get(&self, _key: &str) -> Option<String> {
            panic!("a platform store failed")
        }
        fn set(&self, _key: &str, _secret: &str) -> Result<(), SecretError> {
            Ok(())
        }
        fn delete(&self, _key: &str) {}
    }
    let broker = Broker::new(READ_WAIT);
    let store: Arc<dyn SecretStore> = Arc::new(Panicking);
    assert_eq!(broker.read(&store, "remote-1"), Err(KeyError::Unavailable));
    let memory = shared(Memory::default());
    save(memory.as_ref(), "remote-1", &ApiKey::new(KEY).unwrap()).unwrap();
    // The busy flag was let go: the next read is made.
    assert!(when_free(&broker, &memory, "remote-1").is_some());
    assert_eq!(broker.read(&shared(Memory::default()), "bad/id"), Err(KeyError::BadProvider));
}
