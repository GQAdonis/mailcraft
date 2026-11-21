// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The runtimes an answer may come from, the owner's chain of them, what happens when one fails,
//! and which account's mail may go to which (spec `AI-SETTINGS-SPEC.md` §2, §3 and §9, as amended
//! for this phase). Pure data, no I/O, and it builds for the browser.
//!
//! - **Runtimes** ([`RuntimeId`], [`RuntimeKind`]): `on_device`, `compat:<provider-id>`,
//!   `chatgpt`, `claude_code`, `codex` and `uar`. Nothing else: a kind this version doesn't know
//!   (`opencode` among them) can't be held, so it is dropped where it is read and never taken for
//!   one that stays on this computer.
//! - **Where** ([`Where`], [`where_of`]): `local` (the text never leaves this computer) or `leaves`
//!   (it goes to a vendor or a service). It follows from the runtime's type and from what
//!   MailCraft itself set up or checked, never from anything a runtime says about itself.
//! - **The chain** ([`ChainEntry`], [`Chain`]): the owner's order, each entry on or off, at most
//!   [`MAX_CHAIN`] entries and each runtime once. Building one is fallible and never panics; one
//!   read from a file ([`Chain::from_value`]) keeps what can be used and drops the rest.
//! - **Fallback** ([`Fallback`]): `next`, `ask` or `stop` when a runtime fails.
//! - **Consent** ([`Consents`], [`may_send`]): per account and per runtime. Mail of an account goes
//!   to a runtime that `leaves` only when the owner listed that runtime for that account. Turning
//!   a runtime on grants nothing.
//!
//! No runtime here holds a Claude or Codex key: both run on the owner's own login in their CLI.
//! A [`ChainEntry`] has no `auth` field, and only `compat:*` and `chatgpt` have a credential entry
//! ([`RuntimeId::credential_entry`]).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use super::AiError;

/// Most entries in a chain.
pub const MAX_CHAIN: usize = 16;
/// Most bytes of a `compat` provider's id (as `mailcraft-ai` takes it).
pub const MAX_PROVIDER_ID: usize = 64;
/// Most bytes of a model's name.
pub const MAX_MODEL: usize = 64;
/// Most bytes of a runtime's path.
pub const MAX_PATH: usize = 4_096;
/// Most bytes of an account's address.
pub const MAX_ACCOUNT: usize = 320;
/// Most accounts with a consent.
pub const MAX_CONSENT_ACCOUNTS: usize = 1_024;
/// Most runtimes one account's consent lists.
pub const MAX_CONSENTED_RUNTIMES: usize = 64;
/// Most items looked at in a list read from a file or a caller (a chain, a consent): what is past
/// it is dropped unread, so that a hostile list costs a bounded amount of work.
pub const MAX_SCAN: usize = 4_096;

const COMPAT_PREFIX: &str = "compat:";

/// The id of a `compat` provider (an OpenAI-compatible server the owner set up): letters, digits,
/// `.`, `-` and `_`, 1 to [`MAX_PROVIDER_ID`] bytes, as `mailcraft-ai` takes a provider's id.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProviderId(String);

impl ProviderId {
    /// The id, checked.
    ///
    /// # Errors
    /// [`AiError::Invalid`] when it is empty, too long, or holds anything but letters, digits,
    /// `.`, `-` and `_`.
    pub fn new(id: &str) -> Result<ProviderId, AiError> {
        let ok = !id.is_empty() && id.len() <= MAX_PROVIDER_ID && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'));
        if ok { Ok(ProviderId(id.to_string())) } else { Err(AiError::Invalid("that isn't a model provider's id".into())) }
    }

    /// The id as written.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A runtime's type, without the provider a `compat` runtime names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeKind {
    /// Apple's model on this computer (through apfel).
    OnDevice,
    /// An OpenAI-compatible provider the owner set up (on this computer, elsewhere or a gateway).
    Compat,
    /// The owner's ChatGPT plan, signed in by MailCraft (Sign in with ChatGPT).
    #[serde(rename = "chatgpt")]
    ChatGpt,
    /// Claude Code, on the owner's own login in that CLI.
    ClaudeCode,
    /// Codex, on the owner's own ChatGPT login in that CLI.
    Codex,
    /// The Universal Agent Runtime (`~/.uar/bin/universal-agent-runtime`).
    Uar,
}

impl RuntimeKind {
    /// Every kind, in the order the spec names them.
    pub const ALL: [RuntimeKind; 6] =
        [RuntimeKind::OnDevice, RuntimeKind::Compat, RuntimeKind::ChatGpt, RuntimeKind::ClaudeCode, RuntimeKind::Codex, RuntimeKind::Uar];

    /// The kind's name, as JSON writes it.
    pub fn name(self) -> &'static str {
        match self {
            RuntimeKind::OnDevice => "on_device",
            RuntimeKind::Compat => "compat",
            RuntimeKind::ChatGpt => "chatgpt",
            RuntimeKind::ClaudeCode => "claude_code",
            RuntimeKind::Codex => "codex",
            RuntimeKind::Uar => "uar",
        }
    }

    /// Where a runtime of this kind sends text whatever its settings: `Some` for every kind but
    /// `compat` and `uar`, whose place depends on what MailCraft set up ([`where_of`]).
    pub fn fixed_where(self) -> Option<Where> {
        match self {
            RuntimeKind::OnDevice => Some(Where::Local),
            RuntimeKind::ChatGpt | RuntimeKind::ClaudeCode | RuntimeKind::Codex => Some(Where::Leaves),
            RuntimeKind::Compat | RuntimeKind::Uar => None,
        }
    }

    /// Whether MailCraft keeps a secret for a runtime of this kind in the system's credential
    /// store: a `compat` provider's key and the ChatGPT plan's sign-in. Never Claude Code or Codex,
    /// which use the owner's own login in their CLI.
    pub fn keeps_credential(self) -> bool {
        matches!(self, RuntimeKind::Compat | RuntimeKind::ChatGpt)
    }

    /// Whether an entry of this kind takes a model's name ([`ChainEntry::with_model`]). A `compat`
    /// provider's model is in its own settings; the model on this computer has none to choose.
    pub fn takes_model(self) -> bool {
        matches!(self, RuntimeKind::ChatGpt | RuntimeKind::ClaudeCode | RuntimeKind::Codex | RuntimeKind::Uar)
    }

    /// Whether an entry of this kind takes the owner's path to its program ([`ChainEntry::with_path`]).
    pub fn takes_path(self) -> bool {
        matches!(self, RuntimeKind::ClaudeCode | RuntimeKind::Codex | RuntimeKind::Uar)
    }
}

/// A runtime, by id. As JSON (and as text): `"on_device"`, `"compat:<provider-id>"`,
/// `"chatgpt"`, `"claude_code"`, `"codex"` or `"uar"`. Any other text is refused, so no runtime
/// this version doesn't know (`"opencode"` among them) can be held.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RuntimeId {
    /// Apple's model on this computer.
    OnDevice,
    /// The OpenAI-compatible provider with this id.
    Compat(ProviderId),
    /// The owner's ChatGPT plan.
    ChatGpt,
    /// Claude Code.
    ClaudeCode,
    /// Codex.
    Codex,
    /// The Universal Agent Runtime.
    Uar,
}

impl RuntimeId {
    /// The runtime `text` names.
    ///
    /// # Errors
    /// [`AiError::Invalid`] for any text but a runtime's id (an unknown kind, `opencode`, a
    /// `compat:` without a valid provider id, too long, …).
    pub fn parse(text: &str) -> Result<RuntimeId, AiError> {
        let unknown = || AiError::Invalid("that isn't a runtime this version knows".into());
        if text.len() > COMPAT_PREFIX.len().saturating_add(MAX_PROVIDER_ID) {
            return Err(unknown());
        }
        Ok(match text {
            "on_device" => RuntimeId::OnDevice,
            "chatgpt" => RuntimeId::ChatGpt,
            "claude_code" => RuntimeId::ClaudeCode,
            "codex" => RuntimeId::Codex,
            "uar" => RuntimeId::Uar,
            _ => match text.strip_prefix(COMPAT_PREFIX) {
                Some(id) => RuntimeId::Compat(ProviderId::new(id).map_err(|_| unknown())?),
                None => return Err(unknown()),
            },
        })
    }

    /// The runtime of a `compat` provider.
    ///
    /// # Errors
    /// [`AiError::Invalid`] when `provider` isn't a provider's id.
    pub fn compat(provider: &str) -> Result<RuntimeId, AiError> {
        ProviderId::new(provider).map(RuntimeId::Compat)
    }

    /// Its kind.
    pub fn kind(&self) -> RuntimeKind {
        match self {
            RuntimeId::OnDevice => RuntimeKind::OnDevice,
            RuntimeId::Compat(_) => RuntimeKind::Compat,
            RuntimeId::ChatGpt => RuntimeKind::ChatGpt,
            RuntimeId::ClaudeCode => RuntimeKind::ClaudeCode,
            RuntimeId::Codex => RuntimeKind::Codex,
            RuntimeId::Uar => RuntimeKind::Uar,
        }
    }

    /// The `compat` provider it names, if it is one.
    pub fn provider(&self) -> Option<&ProviderId> {
        match self {
            RuntimeId::Compat(id) => Some(id),
            _ => None,
        }
    }

    /// The name of its entry in the system's credential store (`ai.compat:<id>`, `ai.chatgpt`), or
    /// `None` for a runtime MailCraft keeps no secret for (on this computer, Claude Code, Codex,
    /// UAR, whose launch token lives in memory only).
    pub fn credential_entry(&self) -> Option<String> {
        self.kind().keeps_credential().then(|| format!("ai.{self}"))
    }
}

impl fmt::Display for RuntimeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RuntimeId::Compat(id) => write!(f, "{COMPAT_PREFIX}{id}"),
            other => f.write_str(other.kind().name()),
        }
    }
}

impl FromStr for RuntimeId {
    type Err = AiError;
    fn from_str(s: &str) -> Result<RuntimeId, AiError> {
        RuntimeId::parse(s)
    }
}

impl Serialize for RuntimeId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for RuntimeId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<RuntimeId, D::Error> {
        let text = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        RuntimeId::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// Where a runtime sends the text it is given. As JSON: `"local"` or `"leaves"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Where {
    /// The text never leaves this computer.
    Local,
    /// The text goes to a vendor or a service: mail only of the accounts that allowed the runtime.
    Leaves,
}

/// What happens when a runtime of the chain fails. As JSON: `"next"`, `"ask"` or `"stop"`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fallback {
    /// Try the next eligible runtime (a stop for tool use is still told).
    #[default]
    Next,
    /// Ask the owner whether to try the next one.
    Ask,
    /// Show the failure; try nothing else.
    Stop,
}

impl Fallback {
    /// The policy `text` names; `None` for anything else.
    pub fn parse(text: &str) -> Option<Fallback> {
        match text {
            "next" => Some(Fallback::Next),
            "ask" => Some(Fallback::Ask),
            "stop" => Some(Fallback::Stop),
            _ => None,
        }
    }

    /// Its name, as JSON writes it.
    pub fn name(self) -> &'static str {
        match self {
            Fallback::Next => "next",
            Fallback::Ask => "ask",
            Fallback::Stop => "stop",
        }
    }
}

/// Which Universal Agent Runtime an entry means. As JSON: `"managed"` or `"external"`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UarInstance {
    /// One MailCraft starts and configures itself.
    #[default]
    Managed,
    /// One already running that the owner chose: MailCraft can't contain its tools, so it leaves.
    External,
}

/// A model's name for a runtime: 1 to [`MAX_MODEL`] bytes of `A-Z a-z 0-9 . _ : / -`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ModelName(String);

impl ModelName {
    /// The name, checked.
    ///
    /// # Errors
    /// [`AiError::Invalid`] for an empty or too long name, or one with another character.
    pub fn new(name: &str) -> Result<ModelName, AiError> {
        let ok = !name.is_empty()
            && name.len() <= MAX_MODEL
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'/' | b'-'));
        if ok { Ok(ModelName(name.to_string())) } else { Err(AiError::Invalid("that isn't a model's name".into())) }
    }

    /// The name as written.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The owner's path to a runtime's program: absolute (`/…`, `C:\…` or `C:/…`), at most
/// [`MAX_PATH`] bytes, no control character and no `..` part. Checked here as text only: the host
/// checks the file again at every start of the program.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RuntimePath(String);

impl RuntimePath {
    /// The path, checked.
    ///
    /// # Errors
    /// [`AiError::Invalid`] for a path that isn't absolute, is too long, or holds a control
    /// character or a `..` part.
    pub fn new(path: &str) -> Result<RuntimePath, AiError> {
        let bad = || AiError::Invalid("that isn't an absolute path to a program".into());
        if path.is_empty() || path.len() > MAX_PATH || path.chars().any(char::is_control) {
            return Err(bad());
        }
        let bytes = path.as_bytes();
        let drive = matches!((bytes.first(), bytes.get(1), bytes.get(2)), (Some(l), Some(b':'), Some(b'\\' | b'/')) if l.is_ascii_alphabetic());
        if !(path.starts_with('/') || drive) {
            return Err(bad());
        }
        if path.split(['/', '\\']).any(|part| part == "..") {
            return Err(bad());
        }
        Ok(RuntimePath(path.to_string()))
    }

    /// The path as written.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One runtime of the owner's chain: which, whether it is on, and its options. There is no
/// `auth` and no key: Claude Code and Codex run on the owner's own login in their CLI.
///
/// As JSON: `{"runtime": "<id>", "enabled": bool, "model"?: "<name>", "path"?: "<absolute>",
/// "instance"?: "managed" | "external"}`; `instance` only for `uar` (always written for it).
/// Read from a file with [`ChainEntry::from_value`]: an entry whose runtime isn't known is
/// refused; an option that doesn't check, or that the runtime doesn't take, is dropped; any
/// other member (`auth` among them) is ignored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChainEntry {
    runtime: RuntimeId,
    enabled: bool,
    model: Option<ModelName>,
    path: Option<RuntimePath>,
    instance: Option<UarInstance>,
}

impl ChainEntry {
    /// An entry for `runtime`, off, with no options (a `uar` entry is the managed instance).
    pub fn new(runtime: RuntimeId) -> ChainEntry {
        let instance = (runtime.kind() == RuntimeKind::Uar).then_some(UarInstance::Managed);
        ChainEntry { runtime, enabled: false, model: None, path: None, instance }
    }

    /// The runtime.
    pub fn runtime(&self) -> &RuntimeId {
        &self.runtime
    }

    /// Whether the owner turned it on. Being on grants no consent.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// The model the owner chose, if any.
    pub fn model(&self) -> Option<&ModelName> {
        self.model.as_ref()
    }

    /// The owner's path to its program, if any (else the one found on this computer).
    pub fn path(&self) -> Option<&RuntimePath> {
        self.path.as_ref()
    }

    /// Which UAR it means: `Some` for a `uar` entry only.
    pub fn instance(&self) -> Option<UarInstance> {
        self.instance
    }

    /// The same entry, on or off.
    pub fn with_enabled(&self, enabled: bool) -> ChainEntry {
        ChainEntry { enabled, ..self.clone() }
    }

    /// The same entry with this model (`None`: the runtime's own default).
    ///
    /// # Errors
    /// [`AiError::Invalid`] when the name doesn't check or the runtime takes no model.
    pub fn with_model(&self, model: Option<&str>) -> Result<ChainEntry, AiError> {
        let model = match model {
            None => None,
            Some(_) if !self.runtime.kind().takes_model() => return Err(AiError::Invalid(format!("{} takes no model", self.runtime))),
            Some(name) => Some(ModelName::new(name)?),
        };
        Ok(ChainEntry { model, ..self.clone() })
    }

    /// The same entry with this path to its program (`None`: the one found on this computer).
    ///
    /// # Errors
    /// [`AiError::Invalid`] when the path doesn't check or the runtime takes no path.
    pub fn with_path(&self, path: Option<&str>) -> Result<ChainEntry, AiError> {
        let path = match path {
            None => None,
            Some(_) if !self.runtime.kind().takes_path() => return Err(AiError::Invalid(format!("{} takes no path", self.runtime))),
            Some(p) => Some(RuntimePath::new(p)?),
        };
        Ok(ChainEntry { path, ..self.clone() })
    }

    /// The same `uar` entry for this instance.
    ///
    /// # Errors
    /// [`AiError::Invalid`] for an entry that isn't `uar`.
    pub fn with_instance(&self, instance: UarInstance) -> Result<ChainEntry, AiError> {
        if self.runtime.kind() != RuntimeKind::Uar {
            return Err(AiError::Invalid(format!("{} has no instance", self.runtime)));
        }
        Ok(ChainEntry { instance: Some(instance), ..self.clone() })
    }

    /// An entry read from a file or a caller: hostile input.
    ///
    /// # Errors
    /// [`AiError::Invalid`] when `value` isn't an object or its `runtime` isn't one this version
    /// knows. Everything else that doesn't check is dropped: `enabled` that isn't `true` is off; a
    /// model, a path or an instance that doesn't check or doesn't apply is left out.
    pub fn from_value(value: &Value) -> Result<ChainEntry, AiError> {
        let object = value.as_object().ok_or_else(|| AiError::Invalid("a chain entry is an object".into()))?;
        let runtime = object
            .get("runtime")
            .and_then(Value::as_str)
            .ok_or_else(|| AiError::Invalid("a chain entry names its runtime".into()))
            .and_then(RuntimeId::parse)?;
        let base = ChainEntry::new(runtime).with_enabled(object.get("enabled").and_then(Value::as_bool) == Some(true));
        let with_model = match object.get("model").and_then(Value::as_str) {
            Some(name) => base.with_model(Some(name)).unwrap_or(base),
            None => base,
        };
        let with_path = match object.get("path").and_then(Value::as_str) {
            Some(p) => with_model.with_path(Some(p)).unwrap_or(with_model),
            None => with_model,
        };
        let instance = object.get("instance").and_then(Value::as_str).and_then(|i| match i {
            "managed" => Some(UarInstance::Managed),
            "external" => Some(UarInstance::External),
            _ => None,
        });
        Ok(match instance {
            Some(i) => with_path.with_instance(i).unwrap_or(with_path),
            None => with_path,
        })
    }
}

#[derive(Serialize)]
struct WrittenEntry<'a> {
    runtime: &'a RuntimeId,
    enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    instance: Option<UarInstance>,
}

impl Serialize for ChainEntry {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        WrittenEntry {
            runtime: &self.runtime,
            enabled: self.enabled,
            model: self.model.as_ref().map(ModelName::as_str),
            path: self.path.as_ref().map(RuntimePath::as_str),
            instance: self.instance,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ChainEntry {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<ChainEntry, D::Error> {
        let value = Value::deserialize(deserializer)?;
        ChainEntry::from_value(&value).map_err(serde::de::Error::custom)
    }
}

/// The owner's chain: the runtimes in the order they are tried, at most [`MAX_CHAIN`], each once.
/// As JSON: an array of [`ChainEntry`]. Read with [`Chain::from_value`] (or `Deserialize`, which
/// is the same): entries that can't be used are dropped, a runtime named again is dropped, and
/// what is past the [`MAX_CHAIN`]th entry kept is dropped.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Chain(Vec<ChainEntry>);

impl Chain {
    /// A chain of `entries` in their order. A runtime named again is dropped (the first is kept).
    ///
    /// # Errors
    /// [`AiError::TooMany`] for more than [`MAX_CHAIN`] runtimes, or more than [`MAX_SCAN`]
    /// entries given.
    pub fn new(entries: impl IntoIterator<Item = ChainEntry>) -> Result<Chain, AiError> {
        let mut kept: Vec<ChainEntry> = Vec::new();
        for (n, entry) in entries.into_iter().enumerate() {
            if n >= MAX_SCAN {
                return Err(AiError::TooMany { what: "chain entries given", max: MAX_SCAN });
            }
            if kept.iter().any(|k| k.runtime == entry.runtime) {
                continue;
            }
            if kept.len() >= MAX_CHAIN {
                return Err(AiError::TooMany { what: "runtimes in the chain", max: MAX_CHAIN });
            }
            kept.push(entry);
        }
        Ok(Chain(kept))
    }

    /// A chain read from a file or a caller: hostile input. Never fails: what isn't an array is
    /// the empty chain; an entry that doesn't read is dropped, so is a runtime named again, and
    /// what is past [`MAX_CHAIN`] entries kept or [`MAX_SCAN`] items looked at.
    pub fn from_value(value: &Value) -> Chain {
        let Some(items) = value.as_array() else { return Chain::default() };
        let mut kept: Vec<ChainEntry> = Vec::new();
        for item in items.iter().take(MAX_SCAN) {
            if kept.len() >= MAX_CHAIN {
                break;
            }
            let Ok(entry) = ChainEntry::from_value(item) else { continue };
            if !kept.iter().any(|k| k.runtime == entry.runtime) {
                kept.push(entry);
            }
        }
        Chain(kept)
    }

    /// Its entries, in order.
    pub fn entries(&self) -> &[ChainEntry] {
        &self.0
    }

    /// How many entries.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether it has none.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The entry for `runtime`.
    pub fn get(&self, runtime: &RuntimeId) -> Option<&ChainEntry> {
        self.0.iter().find(|e| &e.runtime == runtime)
    }

    /// The runtimes, in order.
    pub fn ids(&self) -> Vec<RuntimeId> {
        self.0.iter().map(|e| e.runtime.clone()).collect()
    }

    /// The same entries in the order `order` names them (`ai.setRuntimeOrder`).
    ///
    /// # Errors
    /// [`AiError::Invalid`] unless `order` names every runtime of the chain once and nothing else.
    pub fn reordered(&self, order: &[RuntimeId]) -> Result<Chain, AiError> {
        let refused = || AiError::Invalid("the order must name each runtime of the chain once".into());
        if order.len() != self.0.len() {
            return Err(refused());
        }
        let mut out: Vec<ChainEntry> = Vec::with_capacity(order.len());
        for id in order {
            if out.iter().any(|e| &e.runtime == id) {
                return Err(refused());
            }
            out.push(self.get(id).cloned().ok_or_else(refused)?);
        }
        Ok(Chain(out))
    }

    /// The same chain with `runtime` turned on or off (`ai.setRuntimeEnabled`). Consent is
    /// untouched: it is a different thing ([`Consents`]).
    ///
    /// # Errors
    /// [`AiError::Invalid`] when the chain has no entry for `runtime`.
    pub fn with_enabled(&self, runtime: &RuntimeId, enabled: bool) -> Result<Chain, AiError> {
        let entry = self.get(runtime).ok_or_else(|| AiError::Invalid(format!("{runtime} isn't in the chain")))?;
        self.with_entry(entry.with_enabled(enabled))
    }

    /// The same chain with `entry` in the place of the entry for its runtime, or added at the end.
    ///
    /// # Errors
    /// [`AiError::TooMany`] when it would be added to a full chain.
    pub fn with_entry(&self, entry: ChainEntry) -> Result<Chain, AiError> {
        let mut out = self.0.clone();
        let full = out.len() >= MAX_CHAIN;
        match out.iter_mut().find(|e| e.runtime == entry.runtime) {
            Some(slot) => *slot = entry,
            None if full => return Err(AiError::TooMany { what: "runtimes in the chain", max: MAX_CHAIN }),
            None => out.push(entry),
        }
        Ok(Chain(out))
    }

    /// The same chain without `runtime`.
    pub fn without(&self, runtime: &RuntimeId) -> Chain {
        Chain(self.0.iter().filter(|e| &e.runtime != runtime).cloned().collect())
    }
}

impl Serialize for Chain {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Chain {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Chain, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Ok(Chain::from_value(&value))
    }
}

/// An account's address as consent keys it: trimmed and in lower case; `None` when it isn't one.
pub fn account_key(account: &str) -> Option<String> {
    let a = account.trim();
    let (local, domain) = a.split_once('@')?;
    let ok = !local.is_empty()
        && !domain.is_empty()
        && !domain.contains('@')
        && a.len() <= MAX_ACCOUNT
        && !a.chars().any(|c| c.is_control() || c.is_whitespace());
    ok.then(|| a.to_ascii_lowercase())
}

/// Which runtimes each account lets its mail go to: per account (by address, in lower case) and
/// per runtime. As JSON: `{"<address>": ["<runtime id>", …], …}`. Read with
/// [`Consents::from_value`] (or `Deserialize`): an address or a runtime that doesn't check is
/// dropped one by one, and what is past the caps is dropped.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Consents(BTreeMap<String, BTreeSet<RuntimeId>>);

impl Consents {
    /// No consent at all.
    pub fn new() -> Consents {
        Consents::default()
    }

    /// Whether `account` lets its mail go to `runtime`. An address that isn't one never does.
    pub fn allows(&self, account: &str, runtime: &RuntimeId) -> bool {
        account_key(account).and_then(|key| self.0.get(&key)).is_some_and(|set| set.contains(runtime))
    }

    /// The same consents with `account`'s yes (`allowed`) or no for `runtime` (`ai.setConsent`).
    ///
    /// # Errors
    /// [`AiError::Invalid`] for an address that isn't one; [`AiError::TooMany`] past
    /// [`MAX_CONSENT_ACCOUNTS`] accounts or [`MAX_CONSENTED_RUNTIMES`] runtimes for one account.
    pub fn with(&self, account: &str, runtime: &RuntimeId, allowed: bool) -> Result<Consents, AiError> {
        let key = account_key(account).ok_or_else(|| AiError::Invalid("that isn't an account's address".into()))?;
        let mut out = self.0.clone();
        if allowed {
            if !out.contains_key(&key) && out.len() >= MAX_CONSENT_ACCOUNTS {
                return Err(AiError::TooMany { what: "accounts with a consent", max: MAX_CONSENT_ACCOUNTS });
            }
            let set = out.entry(key).or_default();
            if !set.contains(runtime) && set.len() >= MAX_CONSENTED_RUNTIMES {
                return Err(AiError::TooMany { what: "runtimes one account allows", max: MAX_CONSENTED_RUNTIMES });
            }
            set.insert(runtime.clone());
        } else if let Some(set) = out.get_mut(&key) {
            set.remove(runtime);
            if set.is_empty() {
                out.remove(&key);
            }
        }
        Ok(Consents(out))
    }

    /// The same consents with `runtime` taken out of every account's (a provider removed).
    pub fn without_runtime(&self, runtime: &RuntimeId) -> Consents {
        let out = self
            .0
            .iter()
            .filter_map(|(account, set)| {
                let rest: BTreeSet<RuntimeId> = set.iter().filter(|r| *r != runtime).cloned().collect();
                (!rest.is_empty()).then(|| (account.clone(), rest))
            })
            .collect();
        Consents(out)
    }

    /// Each account (lower case) and the runtimes it allows.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &BTreeSet<RuntimeId>)> {
        self.0.iter().map(|(a, set)| (a.as_str(), set))
    }

    /// The accounts that allow `runtime`.
    pub fn accounts_for<'a>(&'a self, runtime: &'a RuntimeId) -> impl Iterator<Item = &'a str> + 'a {
        self.0.iter().filter(move |(_, set)| set.contains(runtime)).map(|(a, _)| a.as_str())
    }

    /// Whether no account allows anything.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Consents read from a file or a caller: hostile input. Never fails: what isn't an object is
    /// none; an address or a runtime that doesn't check is dropped one by one; an account with
    /// nothing left is dropped; past the caps and [`MAX_SCAN`] items, the rest is dropped.
    pub fn from_value(value: &Value) -> Consents {
        let Some(object) = value.as_object() else { return Consents::default() };
        let mut out: BTreeMap<String, BTreeSet<RuntimeId>> = BTreeMap::new();
        for (account, runtimes) in object.iter().take(MAX_SCAN) {
            let Some(key) = account_key(account) else { continue };
            let Some(list) = runtimes.as_array() else { continue };
            if !out.contains_key(&key) && out.len() >= MAX_CONSENT_ACCOUNTS {
                break;
            }
            let set = out.entry(key.clone()).or_default();
            for runtime in list.iter().take(MAX_SCAN) {
                if set.len() >= MAX_CONSENTED_RUNTIMES {
                    break;
                }
                if let Some(id) = runtime.as_str().and_then(|t| RuntimeId::parse(t).ok()) {
                    set.insert(id);
                }
            }
            if set.is_empty() {
                out.remove(&key);
            }
        }
        Consents(out)
    }
}

impl Serialize for Consents {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Consents {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Consents, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Ok(Consents::from_value(&value))
    }
}

/// A `compat` provider's kind, as the owner set it up (`mailcraft-ai`'s `ProviderKind`). As JSON:
/// `"local"`, `"remote"` or `"gateway"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompatKind {
    /// On this computer, at a loopback address checked on the address connected to.
    Local,
    /// Elsewhere, over TLS.
    Remote,
    /// A gateway on this computer that hands what it is sent to a service on the internet.
    Gateway,
}

/// A `compat` provider the owner set up, as far as [`where_of`] needs it: its id and its kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompatProvider {
    /// Its id.
    pub id: ProviderId,
    /// Its kind.
    pub kind: CompatKind,
}

/// The one model endpoint of a managed UAR's configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UarEndpoint {
    /// The `compat` provider MailCraft wrote into the configuration.
    pub provider: ProviderId,
    /// Whether MailCraft itself checked that provider's resolved peer is a loopback address.
    pub peer_verified_loopback: bool,
}

/// What MailCraft knows of the managed UAR's model configuration. The host makes it only from the
/// configuration MailCraft wrote itself, never from anything UAR reports, with one exception that
/// can only make things stricter: `reported_beyond`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ManagedUar {
    /// The model endpoints MailCraft's configuration names.
    pub endpoints: Vec<UarEndpoint>,
    /// Whether the instance reported any model beyond those: it then leaves, whatever it says.
    pub reported_beyond: bool,
}

/// Where `entry` sends what it is given (spec §2): `on_device` is `local`; a `compat` provider is
/// `local` when its kind is Local, and `leaves` when it is Remote, a gateway, or not among
/// `providers`; `chatgpt`, `claude_code` and `codex` always leave; `uar` leaves unless it is the
/// managed instance and `uar` is a configuration MailCraft wrote that names exactly one model
/// endpoint, a Local `compat` provider whose peer MailCraft verified as loopback, and the instance
/// reported no other model. Nothing a runtime says about itself is read here.
pub fn where_of(entry: &ChainEntry, providers: &[CompatProvider], uar: Option<&ManagedUar>) -> Where {
    let kind_of = |id: &ProviderId| providers.iter().find(|p| &p.id == id).map(|p| p.kind);
    match entry.runtime() {
        RuntimeId::OnDevice => Where::Local,
        RuntimeId::Compat(id) => match kind_of(id) {
            Some(CompatKind::Local) => Where::Local,
            Some(CompatKind::Remote | CompatKind::Gateway) | None => Where::Leaves,
        },
        RuntimeId::ChatGpt | RuntimeId::ClaudeCode | RuntimeId::Codex => Where::Leaves,
        RuntimeId::Uar => {
            let managed = entry.instance() == Some(UarInstance::Managed);
            let local = managed
                && uar.is_some_and(|config| match config.endpoints.as_slice() {
                    [only] => !config.reported_beyond && only.peer_verified_loopback && kind_of(&only.provider) == Some(CompatKind::Local),
                    _ => false,
                });
            if local { Where::Local } else { Where::Leaves }
        }
    }
}

/// What a request carries, for [`may_send`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Carries<'a> {
    /// The owner's own words only, and nothing of any mail (Find by Description's description).
    OwnerTextOnly,
    /// Mail of these accounts (by address).
    MailOf(&'a [String]),
}

/// Whether a request carrying `carries` may go to `runtime`, which is at `place` ([`where_of`]):
/// always when it is `local` or carries the owner's words only; else only when it names at least
/// one account and every account it names lists `runtime` in `consents`. Being on in the chain
/// grants nothing.
pub fn may_send(runtime: &RuntimeId, place: Where, carries: Carries<'_>, consents: &Consents) -> bool {
    match (place, carries) {
        (Where::Local, _) | (Where::Leaves, Carries::OwnerTextOnly) => true,
        (Where::Leaves, Carries::MailOf(accounts)) => !accounts.is_empty() && accounts.iter().all(|a| consents.allows(a, runtime)),
    }
}
