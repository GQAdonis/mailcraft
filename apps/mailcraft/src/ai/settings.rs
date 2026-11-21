// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The AI settings the owner chose (#ai), kept in `ai.json` in the settings folder, version 3
//! (spec `AI-SETTINGS-SPEC.md` §3, as amended for this phase):
//! `{"version": 3, "never_configured": bool, "fallback": "next" | "ask" | "stop", "chain":
//! [ChainEntry], "on_device_first": "unasked" | "yes" | "no", "on_device": {address, identity,
//! context_window}?, "on_device_ports": [port], "providers": [ProviderConfig], "consents":
//! {account: [runtime id]}, "keys_kept_for": [name], "keys_withdrawn": [provider id],
//! "local_model_lists": [{address, models}], "chatgpt"?: {"ext_agent_host_id": "<43 base64url>"},
//! "detected": {kind: {path, version, checked}}}`. Reading and writing version 3 is
//! [`super::settings_v3`]; a file of version 1 or 2 is migrated to it once
//! ([`super::settings_migrate`]), with the original kept beside it as `ai.v2.json`.
//!
//! **The chain and the primary.** The owner's chain ([`AiSettings::chain`]) is what is kept. Until
//! requests are routed by the chain, the primary (the first enabled `compat` entry, `active`) and
//! the backup (an enabled `on_device` entry) are read from it, and the setters that change them
//! ([`AiSettings::set_active`], [`AiSettings::set_backup`], [`AiSettings::remove`]) change the
//! chain to match ([`AiSettings::set_chain`] goes the other way). An empty primary is either never
//! configured (`never_configured`: the owner never chose) or the owner's explicit None, which is
//! kept as such: a primary of None also turns the backup off (every entry of the chain is off),
//! and nothing is sent. The on-device model is used only through an address the owner confirmed
//! ([`OnDeviceBinding`]), never a candidate merely found.
//!
//! The file never holds a key or a sign-in (`ProviderConfig::api_key` is never written; there is
//! no Claude Code or Codex key at all; the ChatGPT plan's issued client id and refresh token live
//! in the credential entry `ai.chatgpt`, and only its non-secret host id is here), and is read as
//! hostile input: at most 256 KB of it, at most 16 providers, each checked
//! (`ProviderConfig::check`) and dropped when it can't be used, the first of an id kept, a runtime
//! this version doesn't know (`opencode` among them) dropped and never taken for one on this
//! computer, a `version` other than the number 3 (or 1 or 2, which are migrated) read as no
//! settings at all. Nothing in it is logged.
//!
//! Beside the providers, `keys_kept_for` names (by runtime, never the key) every runtime a secret
//! was put in the credential store for (`compat:<id>`, `chatgpt`), and, until it is moved, a key
//! still under its version 2 name (`ai-key:<id>`), so that Remove MailCraft's data finds each one
//! ([`key_owners`]) even after its provider was taken away.
//!
//! Consents are the owner's alone, per account and per runtime ([`AiSettings::consents`]): they
//! come from [`AiSettings::set_consent`] and [`AiSettings::set_runtime_consent`], never with a
//! provider's settings (each provider's `consented_accounts` is read from them, for the code that
//! still asks a provider). A provider whose address (or kind) changes loses them, its trusted
//! certificate and its key (`keys_withdrawn` names it until a key is saved again, and its entry is
//! taken out of the credential store); a provider elsewhere whose embeddings model changes loses
//! its consents too, so that the owner is asked again about mail embedded in the background.
//!
//! `local_model_lists` keeps the owner's "no" to the gateway question (a server set up as on this
//! computer that lists models of several publishers: does it send mail to the internet?), by the
//! server's address and a fingerprint of its model list; a "yes" makes the provider a gateway.
//!
//! Beside what is kept, [`Shared`] holds what is known for the session only: the level of the
//! structured-output ladder each provider answered at, the servers whose model list was checked,
//! and the gateway question waiting for the owner ([`GatewayQuestion`]), which the window reads and
//! answers ([`Shared::gateway_question`], [`Shared::answer_gateway`],
//! [`Shared::dismiss_gateway_question`]).

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use mailcraft_ai::on_device::is_identity;
use mailcraft_ai::{Ladders, ProviderConfig, ProviderKind, clamp_context_window, default_context_window, is_fingerprint, valid_id};
use mailcraft_model::ai::{Chain, ChainEntry, Consents, Fallback, MAX_CHAIN, RuntimeId, RuntimeKind};
use serde::Serialize;
use serde_json::{Value, json};

use super::settings_v3::{Detected, HostId, OnDeviceFirst};

/// The file's name in the settings folder.
pub(crate) const FILE: &str = "ai.json";
/// The backup's name in the file.
pub(crate) const APPLE_ON_DEVICE: &str = "apple-on-device";
/// Most ports the owner adds to look for the on-device model on.
pub(crate) const MAX_PORTS: usize = 8;
/// Longest on-device address kept.
const MAX_DEVICE_ADDRESS: usize = 64;
/// The most of the file that is read.
pub(crate) const MAX_FILE: u64 = 256 * 1024;
/// Most providers kept.
pub(crate) const MAX_PROVIDERS: usize = 16;
/// What the file calls the list of providers a key was kept for, and the most it names.
pub(crate) const NOTED: &str = "keys_kept_for";
/// What the file calls the providers whose kept key must not be used (it was for another address).
pub(crate) const WITHDRAWN: &str = "keys_withdrawn";
pub(crate) const MAX_NOTED: usize = 1_000;
/// What the file calls the servers the owner said keep mail on this computer, and the most kept.
pub(crate) const LOCAL_LISTS: &str = "local_model_lists";
const MAX_LOCAL_LISTS: usize = 64;
/// Longest address kept with an answer.
const MAX_ADDRESS: usize = 2_000;
/// Most servers whose model list is known checked in a session.
const MAX_CHECKED: usize = 64;
/// Longest account address, and most accounts a provider is allowed.
const MAX_ACCOUNT: usize = 320;
const MAX_ACCOUNTS: usize = 1_000;

/// What answers when the primary can't, before any of its answer arrives.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Backup {
    /// Nothing else is tried.
    #[default]
    None,
    /// Apple's on-device model, through the address the owner confirmed (and only that one).
    AppleOnDevice,
}

impl Backup {
    /// Its name, as the file and the window write it.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Backup::None => "",
            Backup::AppleOnDevice => APPLE_ON_DEVICE,
        }
    }

    /// The backup `name` names; none for anything else.
    pub(crate) fn parse(name: &str) -> Option<Backup> {
        match name.trim() {
            "" | "none" => Some(Backup::None),
            APPLE_ON_DEVICE => Some(Backup::AppleOnDevice),
            _ => None,
        }
    }
}

/// The on-device model the owner confirmed: one address on this computer, bound to the identity
/// its two answers gave then ([`mailcraft_ai::on_device::identity`]), and its context window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OnDeviceBinding {
    /// `http://127.0.0.1:<port>`.
    pub(crate) address: String,
    pub(crate) identity: String,
    pub(crate) context_window: u32,
}

/// The address of the on-device model on `port` of this computer.
pub(crate) fn device_address(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

/// The port of an on-device address as [`device_address`] writes it; none for anything else.
pub(crate) fn device_port(address: &str) -> Option<u16> {
    let port = address.trim().strip_prefix("http://127.0.0.1:")?;
    if port.is_empty() || port.len() > 5 || !port.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    port.parse::<u16>().ok().filter(|p| *p > 0)
}

impl OnDeviceBinding {
    /// A binding as the file says it, when it is one.
    pub(crate) fn read(value: &Value) -> Option<OnDeviceBinding> {
        let address = value.get("address").and_then(Value::as_str)?;
        let identity = value.get("identity").and_then(Value::as_str)?;
        if address.len() > MAX_DEVICE_ADDRESS || device_port(address).is_none() || !is_identity(identity) {
            return None;
        }
        let context_window =
            value.get("context_window").and_then(Value::as_u64).map_or(default_context_window(ProviderKind::Local), clamp_context_window);
        Some(OnDeviceBinding { address: address.trim().to_string(), identity: identity.to_string(), context_window })
    }

    pub(crate) fn to_json(&self) -> Value {
        json!({"address": self.address, "identity": self.identity, "context_window": self.context_window})
    }
}

/// The owner's AI settings: the chain, the providers, the consents, and the primary and backup
/// read from the chain.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AiSettings {
    /// The owner's chain: the runtimes in the order they are tried, each on or off. Kept in step
    /// with `active` and `backup` (see the module's words).
    pub(crate) chain: Chain,
    /// What happens when a runtime fails.
    pub(crate) fallback: Fallback,
    /// Which runtimes each account lets its mail go to. The source of every provider's
    /// `consented_accounts`.
    pub(crate) consents: Consents,
    /// The owner's answer to the one-time question whether the on-device model goes first.
    pub(crate) on_device_first: OnDeviceFirst,
    /// The ChatGPT plan's host id: made once at the first sign-in and kept. Not a secret.
    pub(crate) chatgpt_host_id: Option<HostId>,
    /// A ChatGPT sign-in was put in the credential store (`ai.chatgpt`).
    pub(crate) chatgpt_key: bool,
    /// The providers (by id) whose key may still be under its version 2 name (`ai-key:<id>`) in
    /// the credential store, until it is moved to `ai.compat:<id>`.
    pub(crate) legacy_keys: Vec<String>,
    /// What a look for the runtimes' programs found: a cache, never used for a decision.
    pub(crate) detected: BTreeMap<String, Detected>,
    /// The primary: the provider requests go to; empty for none (nothing is sent to a provider).
    pub(crate) active: String,
    /// The owner never chose a primary (as opposed to choosing None).
    pub(crate) never_configured: bool,
    /// The owner's chain has the on-device model on and no provider before it: that is a choice
    /// of the on-device model, not the explicit None an empty primary otherwise means.
    pub(crate) device_alone: bool,
    /// What answers when the primary can't. Always [`Backup::None`] while the primary is the
    /// owner's explicit None.
    pub(crate) backup: Backup,
    /// The on-device model the owner confirmed, if any.
    pub(crate) on_device: Option<OnDeviceBinding>,
    /// The ports the owner added to look for the on-device model on (beside 11434).
    pub(crate) device_ports: Vec<u16>,
    pub(crate) providers: Vec<ProviderConfig>,
    /// The providers (by id) a key was put in the credential store for.
    pub(crate) noted: Vec<String>,
    /// The providers (by id) whose key in the credential store is not used: it was given for an
    /// address the provider no longer has.
    pub(crate) withdrawn: Vec<String>,
    /// The owner's "no, it keeps mail here" to the gateway question: (models address, list
    /// fingerprint), oldest first.
    pub(crate) local_lists: Vec<(String, String)>,
}

impl Default for AiSettings {
    /// No file: never configured, and the backup is the on-device model (which is used only once
    /// the owner has confirmed one; until then nothing is sent to it).
    fn default() -> Self {
        let mut settings = AiSettings::bare();
        settings.sync_chain();
        settings
    }
}

/// The ports `text` names (`[11437, …]` in the file): each from 1 to 65535, each once, at most
/// [`MAX_PORTS`]; none of them when one isn't a port.
pub(crate) fn read_ports(value: Option<&Value>) -> Option<Vec<u16>> {
    let listed = value?.as_array()?;
    let mut ports = Vec::new();
    for port in listed {
        let port = port.as_u64().and_then(|p| u16::try_from(p).ok()).filter(|p| *p > 0)?;
        if !ports.contains(&port) {
            ports.push(port);
        }
    }
    (ports.len() <= MAX_PORTS).then_some(ports)
}

/// The address a server's answers are kept under: its model list's address, in lower case.
pub(crate) fn route_address(config: &ProviderConfig) -> String {
    config.endpoint("models").to_ascii_lowercase()
}

impl AiSettings {
    /// Settings with nothing in them, never configured, before the chain is made to match.
    fn bare() -> AiSettings {
        AiSettings {
            chain: Chain::default(),
            fallback: Fallback::default(),
            consents: Consents::new(),
            on_device_first: OnDeviceFirst::default(),
            chatgpt_host_id: None,
            chatgpt_key: false,
            legacy_keys: Vec::new(),
            detected: BTreeMap::new(),
            active: String::new(),
            never_configured: true,
            device_alone: false,
            backup: Backup::AppleOnDevice,
            on_device: None,
            device_ports: Vec::new(),
            providers: Vec::new(),
            noted: Vec::new(),
            withdrawn: Vec::new(),
            local_lists: Vec::new(),
        }
    }

    /// Settings with nothing but what [`AiSettings::read_common`] fills in, for the readers.
    pub(super) fn empty() -> AiSettings {
        AiSettings { never_configured: false, backup: Backup::None, ..AiSettings::bare() }
    }

    /// The settings `bytes` say, as far as they can be used; none at all when they aren't JSON,
    /// are too large, or are of a version this one doesn't know.
    #[cfg_attr(not(test), allow(dead_code, reason = "the window reads the settings through `Shared`"))]
    pub(crate) fn from_json(bytes: &[u8]) -> AiSettings {
        AiSettings::parse(bytes).0
    }

    /// [`AiSettings::from_json`], and what the bytes were.
    pub(crate) fn parse(bytes: &[u8]) -> (AiSettings, Origin) {
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_FILE {
            return (AiSettings::default(), Origin::Default);
        }
        let Ok(value) = serde_json::from_slice::<Value>(bytes) else { return (AiSettings::default(), Origin::Default) };
        match super::settings_v3::shape(&value) {
            super::settings_v3::Shape::Three => (super::settings_v3::read(&value), Origin::Three),
            super::settings_v3::Shape::Older => (super::settings_migrate::migrate(&value), Origin::Migrated),
            // A version this one doesn't know: a newer MailCraft's, kept as it is.
            super::settings_v3::Shape::Unknown if value.get("version").is_some() => (AiSettings::default(), Origin::Newer),
            super::settings_v3::Shape::Unknown => (AiSettings::default(), Origin::Default),
        }
    }

    /// What every version says alike: the providers (`first_version`: a file of version 1, whose
    /// remote windows of 4,096 were no owner's choice), the confirmed on-device model, the owner's
    /// ports, the withdrawn keys and the owner's answers to the gateway question. Each checked.
    pub(super) fn read_common(&mut self, value: &Value, first_version: bool) {
        let listed = value.get("providers").and_then(Value::as_array).map_or(&[][..], Vec::as_slice);
        for entry in listed {
            if self.providers.len() >= MAX_PROVIDERS {
                break;
            }
            let Ok(mut provider) = serde_json::from_value::<ProviderConfig>(entry.clone()) else { continue };
            let says_window = entry.get("context_window").is_some();
            if !says_window
                || (first_version && provider.kind != ProviderKind::Local && provider.context_window == default_context_window(ProviderKind::Local))
            {
                provider.context_window = default_context_window(provider.kind);
            }
            if provider.check().is_err() || self.provider(&provider.id).is_some() {
                continue;
            }
            self.providers.push(ProviderConfig { api_key: None, ..provider });
        }
        self.on_device = value.get("on_device").and_then(OnDeviceBinding::read);
        self.device_ports = read_ports(value.get("on_device_ports")).unwrap_or_default();
        for id in value.get(WITHDRAWN).and_then(Value::as_array).into_iter().flatten().take(MAX_NOTED).filter_map(Value::as_str) {
            self.withdraw_key(id);
        }
        for entry in value.get(LOCAL_LISTS).and_then(Value::as_array).into_iter().flatten().take(MAX_NOTED) {
            let address = entry.get("address").and_then(Value::as_str).unwrap_or("");
            let models = entry.get("models").and_then(Value::as_str).unwrap_or("");
            self.keep_local_list(address, models);
        }
    }

    /// The `compat` runtime of provider `id`, when `id` can name one.
    pub(crate) fn compat(id: &str) -> Option<RuntimeId> {
        RuntimeId::compat(id).ok()
    }

    /// Each provider's `consented_accounts`, read from the consents: the accounts that allow its
    /// runtime, when its mail leaves this computer (one on this computer needs none).
    pub(crate) fn mirror_consents(&mut self) {
        let consents = self.consents.clone();
        for provider in &mut self.providers {
            provider.consented_accounts = match AiSettings::compat(&provider.id) {
                Some(runtime) if provider.kind.mail_leaves_this_computer() => consents.accounts_for(&runtime).map(str::to_string).collect(),
                _ => Vec::new(),
            };
        }
    }

    /// The primary and the backup, read from the chain: the first enabled `compat` entry whose
    /// provider is set up, and an enabled `on_device` entry. A primary of None (not never
    /// configured) has no backup.
    pub(super) fn read_chain(&mut self) {
        self.active = self
            .chain
            .entries()
            .iter()
            .filter(|e| e.enabled())
            .find_map(|e| e.runtime().provider().map(|p| p.as_str().to_string()).filter(|id| self.provider(id).is_some()))
            .unwrap_or_default();
        self.never_configured = self.never_configured && self.active.is_empty();
        let on_device = self.chain.get(&RuntimeId::OnDevice).is_some_and(ChainEntry::enabled);
        // Only the on-device model on (with runtimes of other kinds or not): the owner chose it.
        self.device_alone = on_device && self.active.is_empty() && !self.never_configured;
        self.backup = if on_device && !self.explicitly_none() { Backup::AppleOnDevice } else { Backup::None };
    }

    /// The chain made to match the primary and the backup: the primary's entry on and first, the
    /// on-device model on when it is the backup (added when it isn't there), entries of providers
    /// no longer set up gone, and with a primary of None every entry off. Other entries keep their
    /// place and their state. Past [`MAX_CHAIN`], disabled `compat` entries go first.
    pub(crate) fn sync_chain(&mut self) {
        let none = self.explicitly_none();
        let primary = if none { None } else { AiSettings::compat(&self.active).filter(|_| self.provider(&self.active).is_some()) };
        let device_on = self.backup == Backup::AppleOnDevice && !none;
        let mut entries: Vec<ChainEntry> = Vec::new();
        if let Some(runtime) = &primary {
            let entry = self.chain.get(runtime).cloned().unwrap_or_else(|| ChainEntry::new(runtime.clone()));
            entries.push(entry.with_enabled(true));
        }
        for entry in self.chain.entries() {
            let runtime = entry.runtime();
            if Some(runtime) == primary.as_ref() {
                continue;
            }
            match runtime {
                RuntimeId::Compat(id) if self.provider(id.as_str()).is_none() => {}
                RuntimeId::OnDevice => entries.push(entry.with_enabled(device_on)),
                _ => entries.push(entry.with_enabled(entry.enabled() && !none)),
            }
        }
        if device_on && !entries.iter().any(|e| e.runtime() == &RuntimeId::OnDevice) {
            entries.push(ChainEntry::new(RuntimeId::OnDevice).with_enabled(true));
        }
        while entries.len() > MAX_CHAIN {
            match entries.iter().rposition(|e| !e.enabled() && e.runtime().kind() == RuntimeKind::Compat) {
                Some(at) => {
                    entries.remove(at);
                }
                None => entries.truncate(MAX_CHAIN),
            }
        }
        self.chain = Chain::new(entries).unwrap_or_default();
    }

    /// The owner's chain from now on (the runtime commands): the primary and the backup are read
    /// from it. Entries of providers that aren't set up are dropped. Choosing a chain is a choice:
    /// the settings are no longer never configured.
    #[cfg_attr(not(test), allow(dead_code, reason = "for the runtime commands (ai-runtime-commands), which come next"))]
    pub(crate) fn set_chain(&mut self, chain: Chain) {
        let kept = chain.entries().iter().filter(|e| e.runtime().provider().is_none_or(|p| self.provider(p.as_str()).is_some())).cloned();
        self.chain = Chain::new(kept).unwrap_or_default();
        self.never_configured = false;
        self.read_chain();
    }

    /// Allow `runtime` to be sent `account`'s mail, or no longer (any runtime; for a `compat`
    /// provider, [`AiSettings::set_consent`] also checks that it is set up and elsewhere).
    pub(crate) fn set_runtime_consent(&mut self, account: &str, runtime: &RuntimeId, allowed: bool) -> Result<(), String> {
        self.consents = self.consents.with(account, runtime, allowed).map_err(|e| e.to_string())?;
        self.mirror_consents();
        Ok(())
    }

    /// The ChatGPT plan's host id from now on: refused when it isn't one (43 base64url characters)
    /// or another is already kept (it is made once and kept).
    #[cfg_attr(not(test), allow(dead_code, reason = "for the ChatGPT sign-in (B7), which makes the id"))]
    pub(crate) fn set_chatgpt_host_id(&mut self, id: &str) -> Result<(), String> {
        let id = HostId::new(id).ok_or_else(|| "that isn't a host id".to_string())?;
        match &self.chatgpt_host_id {
            Some(kept) if kept != &id => Err("a host id is already kept".into()),
            _ => {
                self.chatgpt_host_id = Some(id);
                Ok(())
            }
        }
    }

    /// Provider `id`'s key is no longer under its version 2 name (moved, or not there at all).
    pub(crate) fn drop_legacy_key(&mut self, id: &str) {
        self.legacy_keys.retain(|kept| kept != id);
    }

    /// Note that provider `id`'s key may still be under its version 2 name.
    pub(crate) fn note_legacy_key(&mut self, id: &str) {
        if valid_id(id) && self.legacy_keys.len() < MAX_NOTED && !self.legacy_keys.iter().any(|kept| kept == id) {
            self.legacy_keys.push(id.to_string());
        }
    }

    /// The owner said the server whose model list is at `address` (lower case) keeps mail on this
    /// computer while it lists `models` (a fingerprint). Ignored when either isn't one.
    pub(crate) fn keep_local_list(&mut self, address: &str, models: &str) {
        let address_ok = !address.is_empty() && address.len() <= MAX_ADDRESS && address.bytes().all(|b| b.is_ascii_graphic());
        if !address_ok || !is_fingerprint(models) || self.is_local_list(address, models) {
            return;
        }
        if self.local_lists.len() >= MAX_LOCAL_LISTS {
            self.local_lists.remove(0);
        }
        self.local_lists.push((address.to_string(), models.to_ascii_lowercase()));
    }

    /// Whether the owner said the server at `address` keeps mail here while it lists `models`.
    pub(crate) fn is_local_list(&self, address: &str, models: &str) -> bool {
        self.local_lists.iter().any(|(kept, list)| kept == address && list.eq_ignore_ascii_case(models))
    }

    /// Whether provider `id`'s key may be read from the credential store: the settings note a key
    /// kept for it (under either name), and it wasn't given for another address.
    pub(crate) fn key_kept(&self, id: &str) -> bool {
        (self.noted.iter().any(|kept| kept == id) || self.legacy_keys.iter().any(|kept| kept == id)) && self.key_usable(id)
    }

    /// Where provider `id`'s key may be read from: nowhere (not noted, or withdrawn), its new name,
    /// or either name while it may still be under its version 2 one.
    pub(crate) fn kept_key(&self, id: &str) -> super::keys::Kept {
        if !self.key_kept(id) {
            super::keys::Kept::No
        } else if self.legacy_keys.iter().any(|kept| kept == id) {
            super::keys::Kept::Either
        } else {
            super::keys::Kept::New
        }
    }

    /// Provider `id`'s kept key is not to be used (until a key is saved for it again).
    pub(crate) fn withdraw_key(&mut self, id: &str) {
        if valid_id(id) && self.withdrawn.len() < MAX_NOTED && !self.withdrawn.iter().any(|kept| kept == id) {
            self.withdrawn.push(id.to_string());
        }
    }

    /// A key was saved for provider `id`: it is used again.
    pub(crate) fn restore_key(&mut self, id: &str) {
        self.withdrawn.retain(|kept| kept != id);
    }

    /// Whether provider `id`'s kept key may be used.
    pub(crate) fn key_usable(&self, id: &str) -> bool {
        !self.withdrawn.iter().any(|kept| kept == id)
    }

    /// Note that a key was put in the credential store for provider `id`.
    pub(crate) fn note_key(&mut self, id: &str) {
        if valid_id(id) && self.noted.len() < MAX_NOTED && !self.noted.iter().any(|kept| kept == id) {
            self.noted.push(id.to_string());
        }
    }

    /// Provider `id`'s key is out of the credential store (under both its names).
    pub(crate) fn unnote_key(&mut self, id: &str) {
        self.noted.retain(|kept| kept != id);
        self.drop_legacy_key(id);
    }

    /// The settings as they are written (version 3): never a key, a token or a client id.
    pub(crate) fn to_json(&self) -> Value {
        super::settings_v3::write(self)
    }

    /// Provider `id`, if it is set up.
    pub(crate) fn provider(&self, id: &str) -> Option<&ProviderConfig> {
        self.providers.iter().find(|p| p.id == id)
    }

    /// The provider requests go to, if any.
    pub(crate) fn active(&self) -> Option<&ProviderConfig> {
        self.provider(&self.active)
    }

    /// Add `incoming`, or put it in the place of the provider with its id. Its consents are not
    /// taken (the existing provider's stay, a new one has none). When the address or the kind
    /// changes the consents go, and so do the trusted certificate and the key (withdrawn here;
    /// the caller takes it out of the store): whether that happened. A provider elsewhere whose
    /// embeddings model changes loses its consents as well.
    pub(crate) fn set_provider(&mut self, incoming: ProviderConfig) -> Result<bool, String> {
        let incoming = ProviderConfig { api_key: None, ..incoming };
        incoming.check().map_err(|e| e.to_string())?;
        match self.providers.iter_mut().find(|p| p.id == incoming.id) {
            Some(existing) => {
                // A new kind brings its own default window, unless the owner wrote a number.
                let incoming = if existing.kind != incoming.kind && incoming.context_window == default_context_window(existing.kind) {
                    ProviderConfig { context_window: default_context_window(incoming.kind), ..incoming }
                } else {
                    incoming
                };
                let moved = !same_address(&existing.base_url, &incoming.base_url) || existing.kind != incoming.kind;
                let embeds_otherwise =
                    incoming.kind.mail_leaves_this_computer() && existing.embedding_model.trim() != incoming.embedding_model.trim();
                let consented_accounts = if moved || embeds_otherwise { Vec::new() } else { existing.consented_accounts.clone() };
                let pinned_sha256 = if moved { String::new() } else { incoming.pinned_sha256.clone() };
                let id = incoming.id.clone();
                *existing = ProviderConfig { consented_accounts, pinned_sha256, ..incoming };
                if moved || embeds_otherwise {
                    self.forget_consents_of(&id);
                }
                if moved {
                    self.withdraw_key(&id);
                }
                self.mirror_consents();
                Ok(moved)
            }
            None => {
                if self.providers.len() >= MAX_PROVIDERS {
                    return Err(format!("at most {MAX_PROVIDERS} model providers can be set up"));
                }
                let id = incoming.id.clone();
                self.providers.push(ProviderConfig { consented_accounts: Vec::new(), ..incoming });
                self.forget_consents_of(&id);
                self.mirror_consents();
                Ok(false)
            }
        }
    }

    /// Provider `id`'s runtime is allowed no account's mail any more.
    fn forget_consents_of(&mut self, id: &str) {
        if let Some(runtime) = AiSettings::compat(id) {
            self.consents = self.consents.without_runtime(&runtime);
        }
    }

    /// Take provider `id` away (and make the primary none if it was); whether it was there.
    pub(crate) fn remove(&mut self, id: &str) -> bool {
        let before = self.providers.len();
        self.providers.retain(|p| p.id != id);
        self.forget_consents_of(id);
        if self.active == id {
            self.active.clear();
            self.keep_backup_rule();
        }
        self.sync_chain();
        self.providers.len() != before
    }

    /// Requests go to provider `id` from now on ("" for the owner's explicit None, which turns
    /// the backup off too: nothing is sent at all).
    pub(crate) fn set_active(&mut self, id: &str) -> Result<(), String> {
        if !id.is_empty() && self.provider(id).is_none() {
            return Err("no model provider of that name is set up".into());
        }
        // The primary before is no longer used (the owner's chain had one primary).
        if let Some(before) = AiSettings::compat(&self.active).filter(|_| self.active != id) {
            self.chain = self.chain.with_enabled(&before, false).unwrap_or_else(|_| self.chain.clone());
        }
        self.active = id.to_string();
        self.never_configured = false;
        // Choosing a primary, None included, is the owner's word over a chain read earlier.
        self.device_alone = false;
        self.keep_backup_rule();
        self.sync_chain();
        Ok(())
    }

    /// Whether the primary is the owner's explicit None.
    pub(crate) fn explicitly_none(&self) -> bool {
        !self.never_configured && self.active.is_empty() && !self.device_alone
    }

    /// A primary of None turns the backup off (owner decision 1).
    pub(super) fn keep_backup_rule(&mut self) {
        if self.explicitly_none() {
            self.backup = Backup::None;
            self.on_device = None;
        }
    }

    /// The backup from now on. The on-device model can't be chosen while the primary is the
    /// owner's None; choosing none forgets the confirmed address too.
    pub(crate) fn set_backup(&mut self, backup: Backup) -> Result<(), String> {
        if backup != Backup::None && self.explicitly_none() {
            return Err("AI assistance is turned off: choose a primary first".into());
        }
        self.backup = backup;
        if backup == Backup::None {
            self.on_device = None;
            // The on-device model off as well: with no primary, that is the explicit None again.
            self.device_alone = false;
        }
        self.sync_chain();
        Ok(())
    }

    /// The confirmed on-device model, when it is the backup.
    pub(crate) fn backup_binding(&self) -> Option<&OnDeviceBinding> {
        match self.backup {
            Backup::AppleOnDevice if !self.explicitly_none() => self.on_device.as_ref(),
            _ => None,
        }
    }

    /// The ports to look for the on-device model on: 11434 and the owner's, each once.
    pub(crate) fn ports_to_look_on(&self) -> Vec<u16> {
        let mut ports = vec![mailcraft_ai::on_device::DEFAULT_PORT];
        for port in &self.device_ports {
            if !ports.contains(port) {
                ports.push(*port);
            }
        }
        ports
    }

    /// Allow remote provider or gateway `id` to be sent `account`'s mail, or no longer.
    pub(crate) fn set_consent(&mut self, id: &str, account: &str, allowed: bool) -> Result<(), String> {
        let account = account.trim().to_lowercase();
        if account.is_empty() || account.len() > MAX_ACCOUNT || account.chars().any(char::is_control) {
            return Err("that isn't an account's address".into());
        }
        let provider = self.provider(id).ok_or_else(|| "no model provider of that name is set up".to_string())?;
        if !provider.kind.mail_leaves_this_computer() {
            return Err("a model provider on this computer needs no consent".into());
        }
        let runtime = AiSettings::compat(id).ok_or_else(|| "no model provider of that name is set up".to_string())?;
        if allowed && !self.consents.allows(&account, &runtime) && self.consents.accounts_for(&runtime).count() >= MAX_ACCOUNTS {
            return Err(format!("a provider can be allowed at most {MAX_ACCOUNTS} accounts"));
        }
        self.set_runtime_consent(&account, &runtime, allowed)
    }
}

/// Whether two base addresses are the same server and path (case and a last `/` aside).
pub(crate) fn same_address(one: &str, other: &str) -> bool {
    one.trim().trim_end_matches('/').eq_ignore_ascii_case(other.trim().trim_end_matches('/'))
}

/// What the bytes of `ai.json` were.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Origin {
    /// Version 3, read.
    Three,
    /// Version 1 or 2, migrated to version 3 (not yet written).
    Migrated,
    /// Nothing that could be used (not JSON, too large, not an object): the settings with nothing
    /// in them, nothing enabled that leaves this computer.
    Default,
    /// A version this one doesn't know (99, a string, …), written by a newer MailCraft: read as
    /// [`Origin::Default`] is, and never written over ([`Shared::change`] refuses).
    Newer,
}

/// What a change answers when `ai.json` is a newer MailCraft's: a `settings` failure, as the
/// window reads one (`{"kind", "message"}`).
pub(crate) fn newer_refusal() -> String {
    json!({
        "kind": mailcraft_model::ai::ErrorKind::Settings,
        "message": "A newer version of MailCraft wrote these AI settings, so this version leaves them as they are. Nothing was changed.",
    })
    .to_string()
}

/// The bytes of the file at `path`, when there is one and it is no larger than [`MAX_FILE`].
fn read_capped(path: &Path) -> Option<Vec<u8>> {
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    if file.take(MAX_FILE.saturating_add(1)).read_to_end(&mut bytes).is_err() {
        log::warn!("the model providers' settings couldn't be read; none are used");
        return None;
    }
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_FILE {
        log::warn!("the model providers' settings are too large to read; none are used");
        return None;
    }
    Some(bytes)
}

/// The settings kept at `path`; none when there is no file, it is too large, or it can't be read.
/// A file of version 1 or 2 is read migrated, and nothing is written ([`Shared::open`] writes it).
#[cfg_attr(not(test), allow(dead_code, reason = "the app opens the settings through `Shared::open`"))]
pub(crate) fn load(path: &Path) -> AiSettings {
    read_capped(path).map(|bytes| AiSettings::from_json(&bytes)).unwrap_or_default()
}

/// Every provider id the file at `path` names, among its providers (whether or not they can be
/// used) and among those a key was kept for: whose `ai-key:<id>` entries Remove MailCraft's data
/// takes out of the credential store. A link is never followed, as the removal never follows one.
pub(crate) fn key_owners(path: &Path) -> Vec<String> {
    if !std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file()) {
        return Vec::new();
    }
    let Some(value) = read_capped(path).and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok()) else { return Vec::new() };
    let providers = value.get("providers").and_then(Value::as_array).into_iter().flatten().filter_map(|p| p.get("id").and_then(Value::as_str));
    // A version 2 file names a provider; version 3 its runtime, or its key's version 2 name.
    let noted = value
        .get(NOTED)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|name| name.strip_prefix("compat:").or_else(|| name.strip_prefix(super::keys::LEGACY_PREFIX)).unwrap_or(name));
    let mut ids: Vec<String> = Vec::new();
    for id in providers.chain(noted) {
        if valid_id(id) && ids.len() < MAX_NOTED + MAX_PROVIDERS && !ids.iter().any(|kept| kept == id) {
            ids.push(id.to_string());
        }
    }
    ids
}

/// Where the original of a migrated file is kept: `ai.v2.json` beside `ai.json`.
pub(crate) fn v2_copy(path: &Path) -> PathBuf {
    path.with_file_name(V2_COPY)
}

/// The name of the copy a migration keeps.
pub(crate) const V2_COPY: &str = "ai.v2.json";

/// Keep `original` (the bytes of a version 1 or 2 file) as [`v2_copy`], written beside it and then
/// put in its place, for one release, so that a rollback still works. A copy already there is the
/// original of an earlier try and is kept as it is.
pub(crate) fn keep_v2_copy(path: &Path, original: &[u8]) -> Result<(), String> {
    let copy = v2_copy(path);
    match std::fs::symlink_metadata(&copy) {
        Ok(meta) if meta.is_file() => Ok(()),
        // Something else by that name (a folder, a link): the original can't be kept there.
        Ok(_) => Err("the model providers' earlier settings couldn't be kept".into()),
        Err(_) => write_beside(&copy, original),
    }
}

/// Write `bytes` to `path`: beside it first, then put in its place, for the owner alone.
fn write_beside(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let couldnt = |e: std::io::Error| format!("the model providers' settings couldn't be written ({})", e.kind());
    let mut name = path.file_name().map(std::ffi::OsStr::to_os_string).unwrap_or_default();
    name.push(".new");
    let beside = path.with_file_name(name);
    // For the owner alone (on Unix), like the logs and the control key.
    let mut file = mailcraft_engine::store::create_private_replacing(&beside).map_err(couldnt)?;
    file.write_all(bytes).and_then(|()| file.sync_all()).map_err(couldnt)?;
    drop(file);
    std::fs::rename(&beside, path).map_err(couldnt)
}

/// Keep `settings` at `path`: written beside it, then put in its place.
pub(crate) fn save(path: &Path, settings: &AiSettings) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(&settings.to_json()).map_err(|_| "the model providers' settings couldn't be written".to_string())?;
    write_beside(path, &bytes)
}

/// The question put to the owner about a server set up as on this computer whose model list names
/// several publishers: "This server lists models from several publishers. Does it send your mail to
/// a service on the internet?" As the window reads it (`camelCase` JSON): which provider, its
/// address as shown (scheme, host and port), the list's fingerprint the answer is kept for, and
/// (at most eight of) the publishers the server named, as it wrote them: data, never instructions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GatewayQuestion {
    pub(crate) provider: String,
    pub(crate) address: String,
    pub(crate) models: String,
    pub(crate) publishers: Vec<String>,
    /// The model list's address the answer is kept under (not shown).
    #[serde(skip)]
    pub(crate) route_address: String,
}

/// An apfel server found on this computer: listed for the owner, never sent anything until
/// confirmed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Candidate {
    /// `http://127.0.0.1:<port>`.
    pub(crate) address: String,
    pub(crate) model: String,
    pub(crate) context_window: Option<u32>,
    /// What its answers said ([`mailcraft_ai::on_device::identity`]), bound by a confirmation.
    pub(crate) identity: String,
}

/// What is known of the on-device model this session.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DeviceSession {
    /// A look is under way.
    pub(crate) looking: bool,
    /// A look has been made this session.
    pub(crate) looked: bool,
    /// What the last look found.
    pub(crate) candidates: Vec<Candidate>,
    /// The confirmed address whose identity was checked this run and found the same (cleared
    /// after an error, so that it is checked again).
    pub(crate) verified: Option<String>,
    /// The confirmed address whose identity changed: it is not used.
    pub(crate) changed: Option<String>,
}

/// The value behind `mutex`, whether or not a thread panicked holding it.
fn lock_any<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Most windows read from servers kept for a session.
const MAX_WINDOWS: usize = 64;

/// What is known for this session only.
#[derive(Debug, Default)]
struct Session {
    /// The servers (provider id, model list's address) whose model list was checked and passed.
    checked: BTreeSet<(String, String)>,
    /// The question waiting for the owner, if any.
    question: Option<GatewayQuestion>,
    /// The on-device model.
    device: DeviceSession,
    /// The context window each server said its model has: (provider id, model list's address).
    windows: std::collections::BTreeMap<(String, String), u32>,
}

/// The settings as the window and the request threads share them, and where they are kept
/// (none: a run that keeps nothing); and what is known for the session only.
#[derive(Clone)]
pub(crate) struct Shared {
    settings: Arc<Mutex<AiSettings>>,
    path: Option<PathBuf>,
    session: Arc<Mutex<Session>>,
    ladders: Arc<Ladders>,
    /// The original of a migrated file whose copy ([`v2_copy`]) couldn't be written yet: nothing
    /// is written over it until the copy is.
    original: Arc<Mutex<Option<Vec<u8>>>>,
    /// The file is a newer MailCraft's: nothing is ever written over it.
    newer: bool,
}

impl Shared {
    /// The settings kept at `path`, or none. A file of version 1 or 2 is migrated: its original is
    /// kept as `ai.v2.json` first, then the version 3 settings are written in its place, once.
    pub(crate) fn open(path: Option<PathBuf>) -> Shared {
        let bytes = path.as_deref().and_then(read_capped);
        let (settings, origin) = bytes.as_deref().map_or((AiSettings::default(), Origin::Default), AiSettings::parse);
        let shared = Shared {
            settings: Arc::new(Mutex::new(settings)),
            path,
            session: Arc::default(),
            ladders: Arc::default(),
            original: Arc::default(),
            newer: origin == Origin::Newer,
        };
        if shared.newer {
            log::warn!("the AI settings were written by a newer version of MailCraft; they are left as they are");
        }
        if let (Origin::Migrated, Some(bytes)) = (origin, bytes) {
            *lock_any(&shared.original) = Some(bytes);
            if let Err(e) = shared.change(|_| Ok(())) {
                log::warn!("the AI settings were read in their new form but not yet written: {e}");
            }
        }
        shared
    }

    fn lock(&self) -> MutexGuard<'_, AiSettings> {
        self.settings.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// What `look` makes of the settings as they are now, without copying them.
    pub(crate) fn read<T>(&self, look: impl FnOnce(&AiSettings) -> T) -> T {
        look(&self.lock())
    }

    /// The settings as they are now.
    pub(crate) fn snapshot(&self) -> AiSettings {
        self.lock().clone()
    }

    /// Change the settings with `edit` and keep them; nothing changes when `edit` refuses, the
    /// file can't be written, or it is a newer MailCraft's ([`newer_refusal`]).
    pub(crate) fn change<T>(&self, edit: impl FnOnce(&mut AiSettings) -> Result<T, String>) -> Result<T, String> {
        if self.newer && self.path.is_some() {
            return Err(newer_refusal());
        }
        let mut kept = self.lock();
        let mut changed = kept.clone();
        let out = edit(&mut changed)?;
        if let Some(path) = &self.path {
            let mut original = lock_any(&self.original);
            if let Some(bytes) = original.as_deref() {
                keep_v2_copy(path, bytes)?;
                *original = None;
            }
            drop(original);
            save(path, &changed)?;
        }
        *kept = changed;
        Ok(out)
    }

    fn session(&self) -> MutexGuard<'_, Session> {
        self.session.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The level of the structured-output ladder each provider answered at, this session.
    pub(crate) fn ladders(&self) -> &Ladders {
        &self.ladders
    }

    /// Whether `provider`'s model list at `address` was checked and passed this session.
    pub(crate) fn route_checked(&self, provider: &str, address: &str) -> bool {
        self.session().checked.contains(&(provider.to_string(), address.to_string()))
    }

    /// `provider`'s model list at `address` passed the check.
    pub(crate) fn route_passed(&self, provider: &str, address: &str) {
        let mut session = self.session();
        if session.checked.len() < MAX_CHECKED {
            session.checked.insert((provider.to_string(), address.to_string()));
        }
    }

    /// The question waiting for the owner's answer, if any.
    pub(crate) fn gateway_question(&self) -> Option<GatewayQuestion> {
        self.session().question.clone()
    }

    /// Put `question` to the owner, unless another is waiting: whether `question` is the one
    /// waiting now (it already was, or is now).
    pub(crate) fn ask_gateway(&self, question: GatewayQuestion) -> bool {
        let mut session = self.session();
        match &session.question {
            Some(waiting) => waiting.provider == question.provider && waiting.models == question.models,
            None => {
                session.question = Some(question);
                true
            }
        }
    }

    /// The owner's answer to the question about `provider`'s list `models`: `sends_to_internet`
    /// makes it a gateway (its consents start empty, so no account's mail goes until allowed), a
    /// "no" is kept for that address and that list. Refused when that isn't the question waiting.
    pub(crate) fn answer_gateway(&self, provider: &str, models: &str, sends_to_internet: bool) -> Result<(), String> {
        let question = self.gateway_question().filter(|q| q.provider == provider && q.models.eq_ignore_ascii_case(models));
        let Some(question) = question else { return Err("that question isn't being asked".into()) };
        self.change(|settings| {
            if !sends_to_internet {
                settings.keep_local_list(&question.route_address, &question.models);
                return Ok(());
            }
            let Some(existing) = settings.provider(provider).cloned() else { return Err("no model provider of that name is set up".into()) };
            if existing.kind != ProviderKind::Local || route_address(&existing) != question.route_address {
                // It was set up otherwise meanwhile: the owner's later choice stands.
                return Ok(());
            }
            settings.set_provider(ProviderConfig { kind: ProviderKind::Gateway, ..existing }).map(|_| ())
        })?;
        self.session().question = None;
        Ok(())
    }

    /// The owner closed the question without an answer: nothing is kept, nothing is sent, and the
    /// next request asks again.
    pub(crate) fn dismiss_gateway_question(&self) {
        self.session().question = None;
    }

    /// What is known of the on-device model this session.
    pub(crate) fn device(&self) -> DeviceSession {
        self.session().device.clone()
    }

    /// Change what is known of the on-device model with `edit`.
    pub(crate) fn device_edit<T>(&self, edit: impl FnOnce(&mut DeviceSession) -> T) -> T {
        edit(&mut self.session().device)
    }

    /// The context window `config`'s server said its model has, this session.
    pub(crate) fn window_said(&self, config: &ProviderConfig) -> Option<u32> {
        self.session().windows.get(&(config.id.clone(), route_address(config))).copied()
    }

    /// `config`'s server said its model has `window` tokens (already clamped).
    pub(crate) fn keep_window(&self, config: &ProviderConfig, window: u32) {
        let mut session = self.session();
        let key = (config.id.clone(), route_address(config));
        if session.windows.contains_key(&key) || session.windows.len() < MAX_WINDOWS {
            session.windows.insert(key, clamp_context_window(u64::from(window)));
        }
    }

    /// The context window used for `config`: the number the owner wrote; else what its server
    /// said; else the default for its kind. With where it came from: `owner`, `server` or
    /// `default`.
    pub(crate) fn window_of(&self, config: &ProviderConfig) -> (u32, &'static str) {
        let default = default_context_window(config.kind);
        if config.context_window != default {
            return (config.context_window, "owner");
        }
        match self.window_said(config) {
            Some(said) => (said, "server"),
            None => (default, "default"),
        }
    }
}
