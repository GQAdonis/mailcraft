// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `ai.json` version 3, read as hostile input and written (spec `AI-SETTINGS-SPEC.md` §3, as
//! amended: no OpenCode, no Claude Code or Codex key, the ChatGPT plan's host id).
//!
//! - **What version.** Only the number 3 is read as version 3; 1, 2 and a file without a version
//!   are migrated ([`super::settings_migrate`]); anything else (99, a string, …) is a version this
//!   one doesn't know and is read as no settings at all, with nothing enabled that leaves this
//!   computer ([`Shape`]).
//! - **The chain** is [`Chain::from_value`]: an unknown runtime (`opencode` among them) dropped and
//!   never taken for one on this computer, a runtime named again dropped (the first kept), and an
//!   entry for a provider that isn't set up dropped; an `auth` member is ignored, never kept.
//! - **Consents** are [`Consents::from_value`]: an address or a runtime that doesn't check is
//!   dropped one by one.
//! - **`keys_kept_for`** keeps `compat:<id>` and `chatgpt`, and a key's version 2 name
//!   `ai-key:<id>` until it is moved; anything else (`claude_code`, `codex`, junk) is dropped, so
//!   nothing ever asks the credential store for a Claude Code or Codex key.
//! - **`chatgpt.ext_agent_host_id`** is kept when it is 43 base64url characters ([`HostId`]),
//!   else dropped; nothing else of `chatgpt` is read or written (the issued client id and the
//!   refresh token live in the credential entry `ai.chatgpt`).
//! - **`detected`** is a cache of what a look found, checked and capped, and never used for a
//!   decision.

use std::collections::BTreeMap;

use mailcraft_model::ai::{Chain, Consents, Fallback, RuntimeId, RuntimeKind, RuntimePath};
use serde_json::{Map, Value, json};

use super::keys::LEGACY_PREFIX;
use super::settings::{AiSettings, LOCAL_LISTS, MAX_NOTED, NOTED, OnDeviceBinding, WITHDRAWN};

/// The version this one writes.
pub(crate) const VERSION: u64 = 3;
/// Characters in a host id: 32 bytes in base64url, without padding.
pub(crate) const HOST_ID_LEN: usize = 43;
/// Longest version text and time text kept in the detection cache.
const MAX_DETECTED_TEXT: usize = 64;

/// What a parsed `ai.json` is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shape {
    /// `"version": 3`.
    Three,
    /// No version (version 1), or `"version"` 1 or 2: migrated.
    Older,
    /// Not an object, or a version this one doesn't know.
    Unknown,
}

/// What `value` is.
pub(crate) fn shape(value: &Value) -> Shape {
    if !value.is_object() {
        return Shape::Unknown;
    }
    match value.get("version") {
        None => Shape::Older,
        Some(version) => match version.as_u64() {
            Some(1 | 2) => Shape::Older,
            Some(VERSION) => Shape::Three,
            _ => Shape::Unknown,
        },
    }
}

/// The owner's answer to the one-time question whether the on-device model goes first. As JSON:
/// `"unasked"`, `"yes"` or `"no"`; anything else is unasked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum OnDeviceFirst {
    #[default]
    Unasked,
    Yes,
    No,
}

impl OnDeviceFirst {
    /// Its name, as the file writes it.
    pub(crate) fn name(self) -> &'static str {
        match self {
            OnDeviceFirst::Unasked => "unasked",
            OnDeviceFirst::Yes => "yes",
            OnDeviceFirst::No => "no",
        }
    }

    /// The answer `text` names; unasked for anything else.
    pub(crate) fn parse(text: &str) -> OnDeviceFirst {
        match text {
            "yes" => OnDeviceFirst::Yes,
            "no" => OnDeviceFirst::No,
            _ => OnDeviceFirst::Unasked,
        }
    }
}

/// The ChatGPT plan's host id (`ext_agent_host_id`): 32 random bytes in base64url without
/// padding, 43 characters, made once at the first sign-in. Not a secret; checked as hostile input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HostId(String);

impl HostId {
    /// `text` as a host id, when it is one: 43 characters of `A–Z a–z 0–9 - _`, the last of which
    /// carries no bits past the 32nd byte (as an encoder writes it).
    pub(crate) fn new(text: &str) -> Option<HostId> {
        let bytes = text.as_bytes();
        if bytes.len() != HOST_ID_LEN || !bytes.iter().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')) {
            return None;
        }
        // 43 × 6 = 258 bits for 256: the last character's two low bits are zero.
        let last = bytes.last().copied().and_then(sextet)?;
        (last & 0b11 == 0).then(|| HostId(text.to_string()))
    }

    /// The id, as it is sent and written.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// The value of a base64url character.
fn sextet(c: u8) -> Option<u8> {
    match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'-' => Some(62),
        b'_' => Some(63),
        _ => None,
    }
}

/// What a look found of a runtime's program: a cache for the Options page, never used for a
/// decision (re-derived on Look Again).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Detected {
    pub(crate) path: Option<RuntimePath>,
    pub(crate) version: Option<String>,
    pub(crate) checked: Option<String>,
}

impl Detected {
    fn read(value: &Value) -> Option<Detected> {
        let object = value.as_object()?;
        let text = |name: &str| {
            object
                .get(name)
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty() && t.len() <= MAX_DETECTED_TEXT && t.bytes().all(|b| b.is_ascii_graphic()))
                .map(str::to_string)
        };
        let path = object.get("path").and_then(Value::as_str).and_then(|p| RuntimePath::new(p).ok());
        Some(Detected { path, version: text("version"), checked: text("checked") })
    }

    fn to_json(&self) -> Value {
        json!({"path": self.path.as_ref().map(RuntimePath::as_str), "version": self.version, "checked": self.checked})
    }
}

/// The detection cache `value` holds: only for runtimes that are programs on this computer.
fn read_detected(value: Option<&Value>) -> BTreeMap<String, Detected> {
    let Some(object) = value.and_then(Value::as_object) else { return BTreeMap::new() };
    RuntimeKind::ALL
        .iter()
        .filter(|kind| kind.takes_path())
        .filter_map(|kind| object.get(kind.name()).and_then(Detected::read).map(|d| (kind.name().to_string(), d)))
        .collect()
}

/// The names in `keys_kept_for` taken into `settings`: `compat:<id>` (a provider's key),
/// `chatgpt` (the plan's sign-in) and `ai-key:<id>` (a key still under its version 2 name). Every
/// other item is dropped.
fn read_kept(settings: &mut AiSettings, value: Option<&Value>) {
    for name in value.and_then(Value::as_array).into_iter().flatten().take(MAX_NOTED).filter_map(Value::as_str) {
        if let Some(id) = name.strip_prefix(LEGACY_PREFIX) {
            settings.note_legacy_key(id);
            continue;
        }
        match RuntimeId::parse(name) {
            Ok(RuntimeId::Compat(id)) => settings.note_key(id.as_str()),
            Ok(RuntimeId::ChatGpt) => settings.chatgpt_key = true,
            _ => {}
        }
    }
}

/// The names `keys_kept_for` is written with.
fn write_kept(settings: &AiSettings) -> Vec<String> {
    let compat = settings.noted.iter().filter_map(|id| AiSettings::compat(id)).map(|r| r.to_string());
    let chatgpt = settings.chatgpt_key.then(|| RuntimeId::ChatGpt.to_string());
    let legacy = settings.legacy_keys.iter().map(|id| format!("{LEGACY_PREFIX}{id}"));
    compat.chain(chatgpt).chain(legacy).collect()
}

/// The settings a version 3 `value` says, as far as they can be used.
pub(crate) fn read(value: &Value) -> AiSettings {
    let mut settings = AiSettings::empty();
    settings.read_common(value, false);
    let chain = Chain::from_value(value.get("chain").unwrap_or(&Value::Null));
    // An entry for a provider that isn't set up can't be used (and its place isn't known).
    let usable = chain.entries().iter().filter(|e| e.runtime().provider().is_none_or(|p| settings.provider(p.as_str()).is_some())).cloned();
    settings.chain = Chain::new(usable).unwrap_or_default();
    settings.never_configured = value.get("never_configured").and_then(Value::as_bool) == Some(true);
    settings.read_chain();
    settings.fallback = value.get("fallback").and_then(Value::as_str).and_then(Fallback::parse).unwrap_or_default();
    settings.on_device_first = value.get("on_device_first").and_then(Value::as_str).map_or(OnDeviceFirst::Unasked, OnDeviceFirst::parse);
    settings.consents = Consents::from_value(value.get("consents").unwrap_or(&Value::Null));
    settings.mirror_consents();
    read_kept(&mut settings, value.get(NOTED));
    settings.chatgpt_host_id = value.get("chatgpt").and_then(|c| c.get("ext_agent_host_id")).and_then(Value::as_str).and_then(HostId::new);
    settings.detected = read_detected(value.get("detected"));
    settings
}

/// `settings` as version 3 writes them: never a key, a token or a client id.
pub(crate) fn write(settings: &AiSettings) -> Value {
    let providers: Vec<Value> = settings
        .providers
        .iter()
        .filter_map(|p| serde_json::to_value(mailcraft_ai::ProviderConfig { api_key: None, consented_accounts: Vec::new(), ..p.clone() }).ok())
        .collect();
    let local_lists: Vec<Value> = settings.local_lists.iter().map(|(address, models)| json!({"address": address, "models": models})).collect();
    let detected: Map<String, Value> = settings.detected.iter().map(|(kind, d)| (kind.clone(), d.to_json())).collect();
    let mut out = json!({
        "version": VERSION,
        "never_configured": settings.never_configured,
        "fallback": settings.fallback.name(),
        "chain": serde_json::to_value(&settings.chain).unwrap_or_else(|_| json!([])),
        "on_device_first": settings.on_device_first.name(),
        "on_device": settings.on_device.as_ref().map(OnDeviceBinding::to_json),
        "on_device_ports": settings.device_ports,
        "providers": providers,
        "consents": serde_json::to_value(&settings.consents).unwrap_or_else(|_| json!({})),
        NOTED: write_kept(settings),
        WITHDRAWN: settings.withdrawn,
        LOCAL_LISTS: local_lists,
        "detected": detected,
    });
    if let (Some(id), Some(object)) = (&settings.chatgpt_host_id, out.as_object_mut()) {
        object.insert("chatgpt".into(), json!({"ext_agent_host_id": id.as_str()}));
    }
    out
}
