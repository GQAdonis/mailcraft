// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The model providers the owner set up (#ai), kept in `ai.json` in the settings folder:
//! `{"version": 2, "primary": "<provider id>", "active": "<the same>", "backup": "" |
//! "apple-on-device", "never_configured": bool, "on_device": {address, identity,
//! context_window}?, "on_device_ports": [port], "providers": [ProviderConfig]}`.
//!
//! **Primary and backup.** The primary is the provider the owner chose (`primary`; `active` is
//! written beside it for an older version). An empty primary is either never configured
//! (`never_configured`: the owner never chose) or the owner's explicit None, which is kept as
//! such: a primary of None also turns the backup off, and nothing is sent. The backup is Apple's
//! on-device model or none; it is used only through an address the owner confirmed
//! ([`OnDeviceBinding`]), never a candidate merely found. Settings without these (version 1) read
//! as: never configured when they have no provider and no primary (and then the backup is the
//! on-device model, which still needs confirming), else configured with no backup; a gateway's or
//! a remote provider's window of 4,096 (version 1's default, which no field let the owner set)
//! reads as 32,768.
//!
//! The file never holds a key (`ProviderConfig::api_key` is never written), and is read as
//! hostile input: at most 256 KB of it, at most 16 providers, each checked
//! (`ProviderConfig::check`) and dropped when it can't be used, the first of an id kept, an
//! active provider that isn't there taken for none. Nothing in it is logged.
//!
//! Beside the providers, `keys_kept_for` names (by provider id, never the key) every provider a
//! key was put in the credential store for, so that Remove MailCraft's data finds each one
//! ([`key_owners`]) even after its provider was taken away.
//!
//! Consents are the owner's alone: they come from [`AiSettings::set_consent`], never with a
//! provider's settings. A provider whose address (or kind) changes loses them, its trusted
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

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use mailcraft_ai::on_device::is_identity;
use mailcraft_ai::{Ladders, ProviderConfig, ProviderKind, clamp_context_window, default_context_window, is_fingerprint, valid_id};
use serde::Serialize;
use serde_json::{Value, json};

/// The file's name in the settings folder.
pub(crate) const FILE: &str = "ai.json";
const VERSION: u64 = 2;
/// The backup's name in the file.
pub(crate) const APPLE_ON_DEVICE: &str = "apple-on-device";
/// Most ports the owner adds to look for the on-device model on.
pub(crate) const MAX_PORTS: usize = 8;
/// Longest on-device address kept.
const MAX_DEVICE_ADDRESS: usize = 64;
/// The most of the file that is read.
const MAX_FILE: u64 = 256 * 1024;
/// Most providers kept.
pub(crate) const MAX_PROVIDERS: usize = 16;
/// What the file calls the list of providers a key was kept for, and the most it names.
const NOTED: &str = "keys_kept_for";
/// What the file calls the providers whose kept key must not be used (it was for another address).
const WITHDRAWN: &str = "keys_withdrawn";
const MAX_NOTED: usize = 1_000;
/// What the file calls the servers the owner said keep mail on this computer, and the most kept.
const LOCAL_LISTS: &str = "local_model_lists";
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
    fn read(value: &Value) -> Option<OnDeviceBinding> {
        let address = value.get("address").and_then(Value::as_str)?;
        let identity = value.get("identity").and_then(Value::as_str)?;
        if address.len() > MAX_DEVICE_ADDRESS || device_port(address).is_none() || !is_identity(identity) {
            return None;
        }
        let context_window =
            value.get("context_window").and_then(Value::as_u64).map_or(default_context_window(ProviderKind::Local), clamp_context_window);
        Some(OnDeviceBinding { address: address.trim().to_string(), identity: identity.to_string(), context_window })
    }

    fn to_json(&self) -> Value {
        json!({"address": self.address, "identity": self.identity, "context_window": self.context_window})
    }
}

/// The providers, and the one requests go to.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AiSettings {
    /// The primary: the provider requests go to; empty for none (nothing is sent to a provider).
    pub(crate) active: String,
    /// The owner never chose a primary (as opposed to choosing None).
    pub(crate) never_configured: bool,
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
        AiSettings {
            active: String::new(),
            never_configured: true,
            backup: Backup::AppleOnDevice,
            on_device: None,
            device_ports: Vec::new(),
            providers: Vec::new(),
            noted: Vec::new(),
            withdrawn: Vec::new(),
            local_lists: Vec::new(),
        }
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
    /// The settings `bytes` say, as far as they can be used; none at all when they aren't JSON.
    pub(crate) fn from_json(bytes: &[u8]) -> AiSettings {
        let Ok(value) = serde_json::from_slice::<Value>(bytes) else { return AiSettings::default() };
        let mut settings = AiSettings::default();
        // A file of version 1 says nothing of the backup or of never having been configured.
        let first_version = value.get("never_configured").is_none();
        let listed = value.get("providers").and_then(Value::as_array).map_or(&[][..], Vec::as_slice);
        for entry in listed {
            if settings.providers.len() >= MAX_PROVIDERS {
                break;
            }
            let Ok(mut provider) = serde_json::from_value::<ProviderConfig>(entry.clone()) else { continue };
            let says_window = entry.get("context_window").is_some();
            if !says_window
                || (first_version && provider.kind != ProviderKind::Local && provider.context_window == default_context_window(ProviderKind::Local))
            {
                provider.context_window = default_context_window(provider.kind);
            }
            if provider.check().is_err() || settings.provider(&provider.id).is_some() {
                continue;
            }
            settings.providers.push(ProviderConfig { api_key: None, ..provider });
        }
        let primary = value.get("primary").or_else(|| value.get("active")).and_then(Value::as_str).unwrap_or("");
        if settings.provider(primary).is_some() {
            settings.active = primary.to_string();
        }
        settings.never_configured = match value.get("never_configured").and_then(Value::as_bool) {
            Some(never) => never && settings.active.is_empty(),
            None => settings.active.is_empty() && settings.providers.is_empty(),
        };
        settings.backup = match value.get("backup").and_then(Value::as_str) {
            // A backup this version doesn't know is none: nothing is sent to it.
            Some(name) => Backup::parse(name).unwrap_or(Backup::None),
            None if settings.never_configured => Backup::AppleOnDevice,
            None => Backup::None,
        };
        settings.on_device = value.get("on_device").and_then(OnDeviceBinding::read);
        settings.device_ports = read_ports(value.get("on_device_ports")).unwrap_or_default();
        settings.keep_backup_rule();
        for id in value.get(NOTED).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
            settings.note_key(id);
        }
        for id in value.get(WITHDRAWN).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
            settings.withdraw_key(id);
        }
        for entry in value.get(LOCAL_LISTS).and_then(Value::as_array).into_iter().flatten() {
            let address = entry.get("address").and_then(Value::as_str).unwrap_or("");
            let models = entry.get("models").and_then(Value::as_str).unwrap_or("");
            settings.keep_local_list(address, models);
        }
        settings
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
    /// kept for it, and it wasn't given for another address.
    pub(crate) fn key_kept(&self, id: &str) -> bool {
        self.noted.iter().any(|kept| kept == id) && self.key_usable(id)
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

    /// Provider `id`'s key is out of the credential store.
    pub(crate) fn unnote_key(&mut self, id: &str) {
        self.noted.retain(|kept| kept != id);
    }

    /// The settings as they are written: never a key.
    pub(crate) fn to_json(&self) -> Value {
        let providers: Vec<Value> =
            self.providers.iter().filter_map(|p| serde_json::to_value(ProviderConfig { api_key: None, ..p.clone() }).ok()).collect();
        let local_lists: Vec<Value> = self.local_lists.iter().map(|(address, models)| json!({"address": address, "models": models})).collect();
        json!({
            "version": VERSION,
            "primary": self.active,
            "active": self.active,
            "never_configured": self.never_configured,
            "backup": self.backup.name(),
            "on_device": self.on_device.as_ref().map(OnDeviceBinding::to_json),
            "on_device_ports": self.device_ports,
            "providers": providers,
            "keys_kept_for": self.noted,
            "keys_withdrawn": self.withdrawn,
            "local_model_lists": local_lists,
        })
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
                if moved {
                    self.withdraw_key(&id);
                }
                Ok(moved)
            }
            None => {
                if self.providers.len() >= MAX_PROVIDERS {
                    return Err(format!("at most {MAX_PROVIDERS} model providers can be set up"));
                }
                self.providers.push(ProviderConfig { consented_accounts: Vec::new(), ..incoming });
                Ok(false)
            }
        }
    }

    /// Take provider `id` away (and make the primary none if it was); whether it was there.
    pub(crate) fn remove(&mut self, id: &str) -> bool {
        let before = self.providers.len();
        self.providers.retain(|p| p.id != id);
        if self.active == id {
            self.active.clear();
            self.keep_backup_rule();
        }
        self.providers.len() != before
    }

    /// Requests go to provider `id` from now on ("" for the owner's explicit None, which turns
    /// the backup off too: nothing is sent at all).
    pub(crate) fn set_active(&mut self, id: &str) -> Result<(), String> {
        if !id.is_empty() && self.provider(id).is_none() {
            return Err("no model provider of that name is set up".into());
        }
        self.active = id.to_string();
        self.never_configured = false;
        self.keep_backup_rule();
        Ok(())
    }

    /// Whether the primary is the owner's explicit None.
    pub(crate) fn explicitly_none(&self) -> bool {
        !self.never_configured && self.active.is_empty()
    }

    /// A primary of None turns the backup off (owner decision 1).
    fn keep_backup_rule(&mut self) {
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
        }
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
        let provider = self.providers.iter_mut().find(|p| p.id == id).ok_or_else(|| "no model provider of that name is set up".to_string())?;
        if !provider.kind.mail_leaves_this_computer() {
            return Err("a model provider on this computer needs no consent".into());
        }
        provider.consented_accounts.retain(|kept| !kept.trim().eq_ignore_ascii_case(&account));
        if allowed {
            if provider.consented_accounts.len() >= MAX_ACCOUNTS {
                return Err(format!("a provider can be allowed at most {MAX_ACCOUNTS} accounts"));
            }
            provider.consented_accounts.push(account);
        }
        Ok(())
    }
}

/// Whether two base addresses are the same server and path (case and a last `/` aside).
pub(crate) fn same_address(one: &str, other: &str) -> bool {
    one.trim().trim_end_matches('/').eq_ignore_ascii_case(other.trim().trim_end_matches('/'))
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
    let noted = value.get(NOTED).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str);
    let mut ids: Vec<String> = Vec::new();
    for id in providers.chain(noted) {
        if valid_id(id) && ids.len() < MAX_NOTED + MAX_PROVIDERS && !ids.iter().any(|kept| kept == id) {
            ids.push(id.to_string());
        }
    }
    ids
}

/// Keep `settings` at `path`: written beside it, then put in its place.
pub(crate) fn save(path: &Path, settings: &AiSettings) -> Result<(), String> {
    let couldnt = |e: std::io::Error| format!("the model providers' settings couldn't be written ({})", e.kind());
    let bytes = serde_json::to_vec_pretty(&settings.to_json()).map_err(|_| "the model providers' settings couldn't be written".to_string())?;
    let beside = path.with_extension("json.new");
    // For the owner alone (on Unix), like the logs and the control key.
    let mut file = mailcraft_engine::store::create_private_replacing(&beside).map_err(couldnt)?;
    file.write_all(&bytes).and_then(|()| file.sync_all()).map_err(couldnt)?;
    drop(file);
    std::fs::rename(&beside, path).map_err(couldnt)
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
}

impl Shared {
    /// The settings kept at `path`, or none.
    pub(crate) fn open(path: Option<PathBuf>) -> Shared {
        let settings = path.as_deref().map(load).unwrap_or_default();
        Shared { settings: Arc::new(Mutex::new(settings)), path, session: Arc::default(), ladders: Arc::default() }
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

    /// Change the settings with `edit` and keep them; nothing changes when `edit` refuses or the
    /// file can't be written.
    pub(crate) fn change<T>(&self, edit: impl FnOnce(&mut AiSettings) -> Result<T, String>) -> Result<T, String> {
        let mut kept = self.lock();
        let mut changed = kept.clone();
        let out = edit(&mut changed)?;
        if let Some(path) = &self.path {
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
