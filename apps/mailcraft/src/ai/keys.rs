// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A model provider's API key, kept in the system's credential store through
//! `Services::secrets` (the window's [`SecretStore`]) and nowhere else: never the mailbox file,
//! `ui.json`, a command's params, a command line, an address, a log line or an error's text.
//!
//! The key is filed under its runtime's credential entry, `ai.compat:<provider id>`
//! (`RuntimeId::credential_entry`, beside the store's other entries, which are `password:…` and
//! `refresh:…`), so it is found by the provider it belongs to and taken away with it. Version 2
//! filed it under `ai-key:<provider id>` ([`LEGACY_PREFIX`]): such a key is moved to its new name
//! with a read-back before the old one goes ([`move_legacy`]), and until then it is read from its
//! old name when the new one holds none and the settings say it may be there ([`Kept::Either`],
//! [`load_from`]); taking a key away takes both ([`forget`]). A
//! provider's id is the owner's (or, for a boundary run, `mailcraft-boundary-<runId>`, which its
//! clean-up deletes by that name); only letters, digits, `.`, `-` and `_` make one, so it can't
//! reach another entry. There is never an entry for Claude Code or Codex.
//!
//! **A read never hangs a request** (the credential store may wait for the owner to answer a
//! system prompt, or never answer at all). Reads for model requests go through one [`Broker`]:
//! one credential thread for the whole app, fed by a queue, each read waited for at most
//! [`READ_WAIT`]. While a read is under way every other one fails at once ([`KeyError::Busy`]),
//! so no thread is added per request. A key that arrives after its request gave up (the owner
//! answered the system's "allow" late) is kept in this process's memory, never on disk, and the
//! next request uses it; when nothing came, the next request asks the store again once the thread
//! is free ("Ask again"). Only a provider the settings note a kept key for is ever read
//! ([`with_key`]), and a provider on this computer has no key.
//!
//! **A late key is kept only for the key it was read as.** Each provider has a generation that
//! [`Broker::forget_late`] moves on (the owner saved, forgot or withdrew its key, or its address
//! changed); a read carries the generation it started in, and a key that comes after its request
//! gave up is kept only when that generation is still the provider's. A read that only looks
//! whether a key is there ([`Broker::read_kept`] with `keep_late` false) keeps nothing.

use std::collections::BTreeMap;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use mailcraft_ai::{ApiKey, ProviderConfig, ProviderKind, valid_id};
use mailcraft_host::{SecretError, SecretStore};
use mailcraft_model::ai::RuntimeId;

/// The longest a request waits for the credential store.
pub(crate) const READ_WAIT: Duration = Duration::from_secs(5);
/// How often a request whose read found the credential thread busy looks again.
const BUSY_LOOK: Duration = Duration::from_millis(20);
/// Most keys kept in memory after their request gave up.
const MAX_LATE: usize = 64;
/// Most providers whose generation is noted one by one (beyond that, every provider's moves on).
const MAX_GENERATIONS: usize = 1_024;

/// What the name of a provider's key begins with in the credential store.
#[cfg_attr(not(test), allow(dead_code, reason = "the name is made by `RuntimeId::credential_entry`"))]
pub(crate) const PREFIX: &str = "ai.compat:";
/// What it began with in version 2.
pub(crate) const LEGACY_PREFIX: &str = "ai-key:";

/// Why a key could not be kept, read or taken away. Its words never hold the key.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum KeyError {
    /// The provider's id can't name an entry.
    BadProvider,
    /// The credential store refused or failed (its own error is already free of secrets).
    Store(SecretError),
    /// What the store holds for the provider isn't a key (damaged, or written by something else).
    Damaged,
    /// Another read of the store is under way: this one wasn't started.
    Busy,
    /// The store didn't answer in [`READ_WAIT`] (it may be asking the owner to allow MailCraft).
    TimedOut,
    /// The credential thread couldn't be started or stopped working.
    Unavailable,
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyError::BadProvider => f.write_str("the provider's name can't name a key in the credential store"),
            KeyError::Store(error) => write!(f, "{error}"),
            KeyError::Damaged => f.write_str("what the credential store holds for this provider isn't a key"),
            KeyError::Busy => f.write_str("the credential store is still answering another question"),
            KeyError::TimedOut => f.write_str("the credential store didn't answer in time"),
            KeyError::Unavailable => f.write_str("the credential store couldn't be asked"),
        }
    }
}

impl fmt::Debug for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyError::BadProvider => f.write_str("BadProvider"),
            KeyError::Store(error) => write!(f, "Store({error:?})"),
            KeyError::Damaged => f.write_str("Damaged"),
            KeyError::Busy => f.write_str("Busy"),
            KeyError::TimedOut => f.write_str("TimedOut"),
            KeyError::Unavailable => f.write_str("Unavailable"),
        }
    }
}

impl std::error::Error for KeyError {}

/// The name `provider_id`'s key is filed under: its runtime's credential entry,
/// `ai.compat:<provider id>`.
pub(crate) fn entry_name(provider_id: &str) -> Result<String, KeyError> {
    if !valid_id(provider_id) {
        return Err(KeyError::BadProvider);
    }
    RuntimeId::compat(provider_id).ok().and_then(|r| r.credential_entry()).ok_or(KeyError::BadProvider)
}

/// The name version 2 filed `provider_id`'s key under: `ai-key:<provider id>`.
pub(crate) fn legacy_entry_name(provider_id: &str) -> Result<String, KeyError> {
    if valid_id(provider_id) { Ok(format!("{LEGACY_PREFIX}{provider_id}")) } else { Err(KeyError::BadProvider) }
}

/// Keeps `key` for `provider_id` in `store`, under its new name.
pub(crate) fn save(store: &dyn SecretStore, provider_id: &str, key: &ApiKey) -> Result<(), KeyError> {
    let name = entry_name(provider_id)?;
    let _writing = writes();
    store.set(&name, key.expose()).map_err(KeyError::Store)
}

/// One write of the keys' entries at a time: a key saved and a key moved from its version 2 name
/// ([`move_legacy`]) never interleave, so a move never puts an older key over a newer one.
static KEY_WRITES: Mutex<()> = Mutex::new(());

/// The right to write the keys' entries, until it is dropped.
fn writes() -> MutexGuard<'static, ()> {
    lock(&KEY_WRITES)
}

/// What `store` holds under `name`, as a key.
fn read_key(store: &dyn SecretStore, name: &str) -> Result<Option<ApiKey>, KeyError> {
    match store.read(name).map_err(KeyError::Store)? {
        None => Ok(None),
        Some(text) => ApiKey::new(&text).map(Some).map_err(|_| KeyError::Damaged),
    }
}

/// Where the settings say a provider's key is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kept {
    /// Nowhere: nothing is read.
    No,
    /// Under its new name.
    New,
    /// Under its new name, or still under its version 2 name (not moved yet, or its copy didn't
    /// read back): the old name is read when the new one holds none.
    Either,
}

impl From<bool> for Kept {
    fn from(kept: bool) -> Kept {
        if kept { Kept::New } else { Kept::No }
    }
}

/// The key kept for `provider_id` under its new name, if any. (The store may wait for the owner to
/// unlock it: a background thread asks, never the window.)
#[cfg_attr(not(test), allow(dead_code, reason = "requests read through `load_from`"))]
pub(crate) fn load(store: &dyn SecretStore, provider_id: &str) -> Result<Option<ApiKey>, KeyError> {
    load_from(store, provider_id, false)
}

/// [`load`]; and when `legacy` (the settings say the key may still be under its version 2 name),
/// the old name when the new one holds none, or holds something that isn't a key.
pub(crate) fn load_from(store: &dyn SecretStore, provider_id: &str, legacy: bool) -> Result<Option<ApiKey>, KeyError> {
    let old = legacy_entry_name(provider_id)?;
    match read_key(store, &entry_name(provider_id)?) {
        Ok(None) if legacy => read_key(store, &old),
        Err(KeyError::Damaged) if legacy => read_key(store, &old).and_then(|key| key.map(Some).ok_or(KeyError::Damaged)),
        other => other,
    }
}

/// Takes the key kept for `provider_id` away (the provider removed, or the owner's "forget"),
/// under both its names.
pub(crate) fn forget(store: &dyn SecretStore, provider_id: &str) -> Result<(), KeyError> {
    let name = entry_name(provider_id)?;
    let legacy = legacy_entry_name(provider_id)?;
    let _writing = writes();
    let new = store.remove(&name).map_err(KeyError::Store);
    let old = store.remove(&legacy).map_err(KeyError::Store);
    new.and(old)
}

/// What came of moving a key to its new name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Moved {
    /// It is under its new name, read back, and its old name is gone (or it was already moved).
    Done,
    /// Nothing was kept under either name.
    Nothing,
    /// The copy didn't read back as the key: the copy was taken away again (as far as it could
    /// be) and the key stays under its old name, where it is still read.
    NotReadBack,
    /// It is under its new name and read back, but its old name couldn't be taken away yet.
    OldKept,
}

/// Move `provider_id`'s key from its version 2 name to its new one: copied only when nothing is
/// under the new name, read back, and only then taken away from the old name. A key already under
/// the new name (one the owner saved since, before or while this runs) wins: the old one is only
/// taken away, never copied over it. Saving a key waits while this runs ([`save`]). What is kept
/// is copied as it is (never looked at beyond the comparison). Errors are the store's, never the
/// key.
pub(crate) fn move_legacy(store: &dyn SecretStore, provider_id: &str) -> Result<Moved, KeyError> {
    let name = entry_name(provider_id)?;
    let legacy = legacy_entry_name(provider_id)?;
    let _writing = writes();
    let Some(old) = store.read(&legacy).map_err(KeyError::Store)? else {
        return Ok(if store.read(&name).map_err(KeyError::Store)?.is_some() { Moved::Done } else { Moved::Nothing });
    };
    if store.read(&name).map_err(KeyError::Store)?.is_none() {
        store.set(&name, &old).map_err(KeyError::Store)?;
        let back = store.read(&name);
        if !matches!(&back, Ok(Some(new)) if *new == old) {
            // Nothing half-made is left for a read to find first (the old name is read then).
            let _ = store.remove(&name);
            return Ok(Moved::NotReadBack);
        }
    }
    let gone = store.remove(&legacy).is_ok() && matches!(store.read(&legacy), Ok(None));
    Ok(if gone { Moved::Done } else { Moved::OldKept })
}

/// `config` with its key in memory, for the request about to be made, read through `broker`.
///
/// Nothing is read for a provider on this computer (it has no key), for one the settings don't
/// note a usable kept key for (`kept`: `keys_kept_for` names it and its key isn't withdrawn;
/// [`Kept::Either`] while it may still be under its version 2 name), or without a store; the key
/// already in memory for this session, if any, then stays.
pub(crate) fn with_key(
    config: ProviderConfig,
    kept: impl Into<Kept>,
    broker: &Broker,
    store: Option<&Arc<dyn SecretStore>>,
) -> Result<ProviderConfig, KeyError> {
    let kept = kept.into();
    if config.kind == ProviderKind::Local || kept == Kept::No {
        return Ok(config);
    }
    let Some(store) = store else { return Ok(config) };
    // Another read under way (a question's embedding and its chat read the same key one after
    // the other) is waited out, for no longer than one read may take: never a hang, never a
    // thread more, and never "the credential store didn't answer" for a store that did.
    let started = Instant::now();
    let read = loop {
        match broker.read_kept(store, &config.id, true, kept == Kept::Either) {
            Err(KeyError::Busy) if started.elapsed() < broker.wait && broker.under_way_within_wait() => std::thread::sleep(BUSY_LOOK),
            other => break other,
        }
    };
    match read? {
        Some(api_key) => Ok(ProviderConfig { api_key: Some(api_key), ..config }),
        None => Ok(config),
    }
}

/// One read for the credential thread.
struct Job {
    store: Arc<dyn SecretStore>,
    provider: String,
    /// Where the answer goes (room for one: the thread never waits on it).
    reply: SyncSender<Result<Option<ApiKey>, KeyError>>,
    /// Set (under its lock) by the request once it stopped waiting: the thread then keeps a key
    /// that comes for later instead of handing it over.
    gave_up: Arc<Mutex<bool>>,
    /// The provider's generation when the read started ([`LateKeys::generation_of`]).
    generation: u64,
    /// Whether a key that comes too late may be kept for the next request.
    keep_late: bool,
    /// Whether the key may still be under its version 2 name.
    legacy: bool,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The keys that came after their request gave up, by provider id, in this process's memory
/// alone; and each provider's generation, which a forgotten key moves on.
#[derive(Default)]
struct LateKeys {
    keys: BTreeMap<String, ApiKey>,
    /// The generation of each provider whose key was forgotten (others are at `base`).
    generations: BTreeMap<String, u64>,
    /// The generation of a provider not in `generations`.
    base: u64,
    /// The last generation handed out: each new one is higher than every one before.
    counter: u64,
}

impl LateKeys {
    /// `provider`'s generation now.
    fn generation_of(&self, provider: &str) -> u64 {
        self.generations.get(provider).copied().unwrap_or(self.base)
    }

    /// `provider`'s key in memory goes, and a read already under way for it keeps nothing.
    fn forget(&mut self, provider: &str) {
        self.keys.remove(provider);
        self.counter = self.counter.saturating_add(1);
        if self.generations.contains_key(provider) || self.generations.len() < MAX_GENERATIONS {
            self.generations.insert(provider.to_string(), self.counter);
        } else {
            // Too many to note one by one: every provider moves on (no late key is kept for any
            // read under way, which is only ever too careful).
            self.generations.clear();
            self.base = self.counter;
        }
    }

    /// Keep `key`, read for `provider` in `generation`, unless that generation is over.
    fn keep(&mut self, provider: &str, generation: u64, key: ApiKey) {
        if self.generation_of(provider) != generation {
            return;
        }
        if self.keys.contains_key(provider) || self.keys.len() < MAX_LATE {
            self.keys.insert(provider.to_string(), key);
        }
    }
}

type Late = Arc<Mutex<LateKeys>>;

/// The one way model requests read keys: one credential thread, one read at a time, each waited
/// for at most its time. (`Debug` shows how many keys are held, never one.)
#[derive(Clone)]
pub(crate) struct Broker {
    queue: Arc<Mutex<Option<Sender<Job>>>>,
    busy: Arc<AtomicBool>,
    /// When the read under way began: one still inside its wait is worth waiting out, one past
    /// it is stuck on the store (a request is then told at once).
    since: Arc<Mutex<Option<Instant>>>,
    late: Late,
    wait: Duration,
}

impl fmt::Debug for Broker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let late_keys = lock(&self.late).keys.len();
        f.debug_struct("Broker").field("busy", &self.busy.load(Ordering::SeqCst)).field("late_keys", &late_keys).finish()
    }
}

impl Broker {
    /// A broker whose reads are waited for at most `wait` (its thread starts with the first read).
    pub(crate) fn new(wait: Duration) -> Broker {
        Broker {
            queue: Arc::new(Mutex::new(None)),
            busy: Arc::new(AtomicBool::new(false)),
            since: Arc::new(Mutex::new(None)),
            late: Arc::default(),
            wait,
        }
    }

    /// Whether the read under way, if any, began less than one wait ago (so it will end soon).
    fn under_way_within_wait(&self) -> bool {
        self.busy.load(Ordering::SeqCst) && lock(&self.since).is_some_and(|since| since.elapsed() < self.wait)
    }

    /// The app's own broker, waiting [`READ_WAIT`].
    pub(crate) fn global() -> &'static Broker {
        static GLOBAL: OnceLock<Broker> = OnceLock::new();
        GLOBAL.get_or_init(|| Broker::new(READ_WAIT))
    }

    /// The key kept for `provider_id` in `store`: one that came late for an earlier request, or
    /// else a read on the credential thread, waited for at most this broker's time. A key that
    /// comes after this read gave up is kept for the next one.
    #[cfg_attr(not(test), allow(dead_code, reason = "requests read through `read_kept`, which knows where the key is kept"))]
    pub(crate) fn read(&self, store: &Arc<dyn SecretStore>, provider_id: &str) -> Result<Option<ApiKey>, KeyError> {
        self.read_kept(store, provider_id, true, false)
    }

    /// [`Broker::read`]; a key that comes after this read gave up is kept for the next request
    /// only when `keep_late` (the settings note a usable key for the provider); when `legacy`, the
    /// key may still be under its version 2 name ([`load_from`]).
    pub(crate) fn read_kept(
        &self,
        store: &Arc<dyn SecretStore>,
        provider_id: &str,
        keep_late: bool,
        legacy: bool,
    ) -> Result<Option<ApiKey>, KeyError> {
        entry_name(provider_id)?;
        let generation = {
            let late = lock(&self.late);
            if let Some(key) = late.keys.get(provider_id).cloned() {
                return Ok(Some(key));
            }
            late.generation_of(provider_id)
        };
        if self.busy.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            return Err(KeyError::Busy);
        }
        *lock(&self.since) = Some(Instant::now());
        let (reply, answer) = sync_channel(1);
        let gave_up = Arc::new(Mutex::new(false));
        let job = Job { store: store.clone(), provider: provider_id.to_string(), reply, gave_up: gave_up.clone(), generation, keep_late, legacy };
        if let Err(error) = self.send(job) {
            self.busy.store(false, Ordering::SeqCst);
            return Err(error);
        }
        self.wait_for(&answer, &gave_up)
    }

    /// Waits for the thread's answer; once the time is up, says so under the lock the thread
    /// decides under, so that a key is either handed over here or kept for later, never lost.
    fn wait_for(&self, answer: &Receiver<Result<Option<ApiKey>, KeyError>>, gave_up: &Mutex<bool>) -> Result<Option<ApiKey>, KeyError> {
        match answer.recv_timeout(self.wait) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => {
                let mut flag = lock(gave_up);
                *flag = true;
                // It may have come between the time running out and the lock.
                answer.try_recv().unwrap_or(Err(KeyError::TimedOut))
            }
            Err(RecvTimeoutError::Disconnected) => Err(KeyError::Unavailable),
        }
    }

    /// Hands `job` to the credential thread, starting it when there is none.
    fn send(&self, job: Job) -> Result<(), KeyError> {
        let mut queue = lock(&self.queue);
        if queue.is_none() {
            let (tx, rx) = std::sync::mpsc::channel::<Job>();
            let (busy, late) = (self.busy.clone(), self.late.clone());
            std::thread::Builder::new().name("credential store".into()).spawn(move || serve(&rx, &busy, &late)).map_err(|_| KeyError::Unavailable)?;
            *queue = Some(tx);
        }
        let sent = queue.as_ref().map(|tx| tx.send(job));
        match sent {
            Some(Ok(())) => Ok(()),
            // The thread is gone: the next read starts another.
            _ => {
                *queue = None;
                Err(KeyError::Unavailable)
            }
        }
    }

    /// The keys held in memory, for the check that refuses a CLI's argv holding one (never
    /// logged, shown or written).
    pub(crate) fn held_secrets(&self) -> Vec<String> {
        lock(&self.late).keys.values().map(|k| k.expose().to_string()).collect()
    }

    /// The owner saved, forgot or withdrew provider `id`'s key: one kept in memory isn't used,
    /// and one a read under way brings later isn't kept.
    pub(crate) fn forget_late(&self, id: &str) {
        lock(&self.late).forget(id);
    }
}

/// The credential thread: one read at a time, until its broker is gone.
fn serve(jobs: &Receiver<Job>, busy: &AtomicBool, late: &Mutex<LateKeys>) {
    for job in jobs {
        let read = catch_unwind(AssertUnwindSafe(|| load_from(job.store.as_ref(), &job.provider, job.legacy))).unwrap_or(Err(KeyError::Unavailable));
        {
            let gave_up = lock(&job.gave_up);
            let undelivered = if *gave_up {
                Some(read)
            } else {
                match job.reply.try_send(read) {
                    Ok(()) => None,
                    Err(TrySendError::Full(read) | TrySendError::Disconnected(read)) => Some(read),
                }
            };
            if let (true, Some(Ok(Some(key)))) = (job.keep_late, undelivered) {
                lock(late).keep(&job.provider, job.generation, key);
            }
        }
        busy.store(false, Ordering::SeqCst);
    }
}
