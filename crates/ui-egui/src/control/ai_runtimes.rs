// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The owner's control methods for the runtimes of the chain (spec `AI-SETTINGS-SPEC.md` §10, as
//! amended for this phase): `ai.runtimes`, `ai.setRuntimeOrder`, `ai.setRuntimeEnabled`,
//! `ai.setRuntimeOption`, `ai.rescan`, `ai.signIn`, `ai.signOut`, `ai.setRuntimeKey`,
//! `ai.setFallback`, and `ai.setConsent` with a `runtime`.
//!
//! Every one is the owner's own: refused to agents here, at the gate
//! ([`crate::agent_gate::screen`]) and by the engine's class (`Access::Never`), and listed by no
//! MCP tool. Every param is hostile: a wrong shape, an unknown runtime, a string past its cap or a
//! member that could carry a secret is an error, never a panic, and leaves the settings as they
//! were. No method takes a key or a token; none hands one out.
//!
//! The work is pure: [`run_runtime`] reads the owner's [`RuntimeSettings`] from the host
//! ([`AiRuntimes`]), makes the new ones with the checked builders of
//! [`mailcraft_model::ai::runtime`] (an order is a permutation or nothing; turning a runtime on
//! leaves every consent as it was) and hands them back whole for the host to keep. What only the
//! host can do (look for the CLIs again, start a sign-in, forget a key) goes through the host too,
//! by kinds that can't name a CLI's own store ([`SignIn`], [`SignOut`]): Codex is never signed in
//! or out by MailCraft, and Claude Code only by opening the owner's terminal with
//! `claude auth login`.

use mailcraft_model::ai::{Chain, ChainEntry, Consents, Fallback, MAX_CHAIN, ProviderId, RuntimeId, RuntimeKind, RuntimePath, UarInstance, Where};
use serde_json::{Map, Value, json};

use crate::MailApp;

/// The runtimes' methods: every one the owner's own (`mailcraft_engine::agent::AI_RUNTIME_OWNER_ONLY`).
pub const RUNTIME_METHODS: [&str; 10] = [
    "ai.runtimes",
    "ai.setRuntimeOrder",
    "ai.setRuntimeEnabled",
    "ai.setRuntimeOption",
    "ai.rescan",
    "ai.signIn",
    "ai.signOut",
    "ai.setRuntimeKey",
    "ai.setFallback",
    "ai.setConsent",
];

/// The longest text taken for an account's address or a runtime's id, before it is checked.
const MOST_TEXT: usize = 512;
/// The most members of a method's params.
const MOST_MEMBERS: usize = 16;
/// The longest version shown for a runtime.
const MOST_VERSION: usize = 32;
/// Names of params that would carry a secret: refused on every method, never echoed.
const SECRET_NAMES: [&str; 11] =
    ["key", "apikey", "api_key", "secret", "token", "password", "auth", "refresh_token", "refreshtoken", "access_token", "accesstoken"];

const NO_RUNTIMES: &str = "the runtimes aren't available in this build";

/// The owner's runtime settings, whole: the chain, which account's mail may go to which runtime,
/// and what happens when one fails. Pure data; the host keeps it (the settings file) and hands it
/// back as it is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RuntimeSettings {
    pub chain: Chain,
    pub consents: Consents,
    pub fallback: Fallback,
}

/// What MailCraft found of a runtime on this computer (or of its sign-in), in words of its own:
/// never what a program printed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RuntimeState {
    /// Found, signed in where that applies, and ready.
    Ready,
    /// Found, but its owner isn't signed in.
    SignedOut,
    /// Not found on this computer.
    NotFound,
    /// Being looked for, or a sign-in is under way.
    Checking,
    /// Found but can't be used (too old, refused to start, …).
    Unavailable,
    /// Not looked at yet.
    #[default]
    Unknown,
}

impl RuntimeState {
    /// Its name, as the answer writes it.
    pub fn code(self) -> &'static str {
        match self {
            RuntimeState::Ready => "ready",
            RuntimeState::SignedOut => "signedOut",
            RuntimeState::NotFound => "notFound",
            RuntimeState::Checking => "checking",
            RuntimeState::Unavailable => "unavailable",
            RuntimeState::Unknown => "unknown",
        }
    }
}

/// What the host found of one runtime. The version is shown only when it looks like one; the
/// path is a checked absolute path.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RuntimeStatus {
    pub state: RuntimeState,
    pub version: Option<String>,
    pub path: Option<RuntimePath>,
}

/// A sign-in the host may be asked to start. There is none for Codex, whose owner runs
/// `codex login` themselves, and Claude Code's is the owner's own terminal running the fixed argv
/// `claude auth login`: MailCraft offers no claude.ai login of its own and reads nothing under
/// `~/.claude` or in the Keychain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignIn {
    /// The ChatGPT plan's own sign-in flow.
    ChatGpt,
    /// Open the owner's terminal with `claude auth login`, then watch `claude auth status`.
    ClaudeCodeTerminal,
    /// Start the Universal Agent Runtime MailCraft manages.
    ManagedUar,
}

impl SignIn {
    fn what(self) -> &'static str {
        match self {
            SignIn::ChatGpt => "Signing in to a ChatGPT plan",
            SignIn::ClaudeCodeTerminal => "Opening a terminal for `claude auth login`",
            SignIn::ManagedUar => "Starting the Universal Agent Runtime",
        }
    }
}

/// A sign-out the host may be asked for. There is none for Claude Code or Codex: their logins are
/// the owner's, in their CLIs' own stores, which MailCraft never touches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignOut {
    /// Delete the ChatGPT plan's sign-in that MailCraft keeps on this computer.
    ChatGpt,
    /// Forget the key MailCraft keeps for this `compat` provider.
    CompatKey(ProviderId),
}

/// The host's side of the runtimes (the desktop app). The window never does I/O for them itself.
/// Every method that only the desktop host can do has a body here that says it isn't available
/// yet, so a host that doesn't carry it (the browser, the tests) still builds.
pub trait AiRuntimes {
    /// The owner's settings as they are kept now.
    fn settings(&self) -> RuntimeSettings;
    /// Keep `settings` whole, in place of the ones kept. Err: nothing was changed.
    fn save(&self, settings: &RuntimeSettings) -> Result<(), String>;
    /// What was last found of `runtime`.
    fn status(&self, _runtime: &RuntimeId) -> RuntimeStatus {
        RuntimeStatus::default()
    }
    /// Where `runtime` sends the text it is given, from what MailCraft itself set up or checked
    /// (`mailcraft_model::ai::where_of`). Unknown is taken as leaving this computer.
    fn where_of(&self, runtime: &RuntimeId) -> Where {
        runtime.kind().fixed_where().unwrap_or(Where::Leaves)
    }
    /// Look for the runtimes' programs now, past the once-a-minute limit.
    fn rescan(&self) -> Result<(), String> {
        Err("looking for the runtimes' programs isn't available in this version yet".into())
    }
    /// Start a sign-in; its end shows in [`AiRuntimes::status`].
    fn sign_in(&self, how: SignIn) -> Result<(), String> {
        Err(format!("{} isn't available in this version yet", how.what()))
    }
    /// Sign out of what MailCraft itself keeps.
    fn sign_out(&self, _what: &SignOut) -> Result<(), String> {
        Err("signing out isn't available in this version yet".into())
    }
    /// Call off a sign-in started with [`AiRuntimes::sign_in`] that is still waiting (the owner's
    /// Cancel): nothing of it is kept. Nothing happens when none is under way.
    fn cancel_sign_in(&self, _how: SignIn) {}
}

/// What a runtime method came to: an answer, or the window's key field to open (never a key).
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Done(Value),
    /// Open the place where the owner types this `compat` provider's key.
    OpenKeyField(ProviderId),
}

/// Run runtime method `method` with hostile params `p` against `host`. Nothing is kept unless the
/// whole change checks; an error leaves the settings as they were.
///
/// # Errors
/// The words to answer with: an unknown method or runtime, a param that doesn't check, a member
/// that could carry a secret, a change the host refused.
pub fn run_runtime(host: &dyn AiRuntimes, method: &str, p: &Value) -> Result<Step, String> {
    let p = params(p)?;
    match method {
        "ai.runtimes" => {
            only(p, &[])?;
            Ok(Step::Done(list(host)))
        }
        "ai.setRuntimeOrder" => {
            only(p, &["order"])?;
            let order = order(p)?;
            let s = host.settings();
            let chain = s.chain.reordered(&order).map_err(|e| e.to_string())?;
            keep(host, RuntimeSettings { chain, ..s })
        }
        "ai.setRuntimeEnabled" => {
            only(p, &["runtime", "value"])?;
            let runtime = runtime(p)?;
            let value = flag(p, "value")?;
            let s = host.settings();
            // The consents go back as they came: turning a runtime on grants nothing.
            let chain = s.chain.with_enabled(&runtime, value).map_err(|e| e.to_string())?;
            keep(host, RuntimeSettings { chain, ..s })
        }
        "ai.setRuntimeOption" => {
            only(p, &["runtime", "model", "path", "instance"])?;
            let runtime = runtime(p)?;
            let s = host.settings();
            let chain = with_options(&s.chain, &runtime, p)?;
            keep(host, RuntimeSettings { chain, ..s })
        }
        "ai.rescan" => {
            only(p, &[])?;
            host.rescan()?;
            Ok(Step::Done(list(host)))
        }
        "ai.signIn" => {
            only(p, &["runtime"])?;
            sign_in(host, &runtime(p)?)
        }
        "ai.signOut" => {
            only(p, &["runtime"])?;
            sign_out(host, &runtime(p)?)
        }
        "ai.setRuntimeKey" => {
            only(p, &["runtime"])?;
            key_field(host, &runtime(p)?)
        }
        "ai.setFallback" => {
            only(p, &["value"])?;
            let text = text(p, "value")?;
            let fallback = Fallback::parse(text).ok_or("`value` is next, ask or stop")?;
            let s = host.settings();
            keep(host, RuntimeSettings { fallback, ..s })
        }
        "ai.setConsent" => {
            only(p, &["runtime", "account", "allowed"])?;
            consent(host, p)
        }
        _ => Err("unknown runtime method".into()),
    }
}

/// [`run_runtime`] for the window: against the host the window was given, and the key field
/// opened in the window itself.
pub(super) fn on_app(app: &mut MailApp, method: &str, p: &Value) -> Result<Value, String> {
    let step = match host_of(app) {
        Some(host) => run_runtime(host, method, p)?,
        None => return Err(NO_RUNTIMES.into()),
    };
    finish(app, step)
}

/// The window's host for the runtimes.
///
/// Integrator (12.8): once `Services` carries `pub ai_runtimes: Option<Box<dyn AiRuntimes>>`,
/// this is `app.services.ai_runtimes.as_deref()`. Until then the methods answer that the
/// runtimes aren't available in this build, and change nothing.
fn host_of(app: &MailApp) -> Option<&dyn AiRuntimes> {
    app.services.ai_runtimes.as_deref()
}

/// What a step leaves the window to do: the key field is File › Options at AI assistance, where
/// the owner types a key; nothing of a key goes through here.
pub(crate) fn finish(app: &mut MailApp, step: Step) -> Result<Value, String> {
    match step {
        Step::Done(v) => Ok(v),
        Step::OpenKeyField(provider) => crate::ai::settings_runtimes::open_key_field(app, &provider),
    }
}

/// The params: an object of at most [`MOST_MEMBERS`] members, none named for a secret.
fn params(p: &Value) -> Result<&Map<String, Value>, String> {
    let empty = || -> &'static Map<String, Value> {
        static EMPTY: std::sync::OnceLock<Map<String, Value>> = std::sync::OnceLock::new();
        EMPTY.get_or_init(Map::new)
    };
    let p = match p {
        Value::Null => return Ok(empty()),
        Value::Object(o) => o,
        _ => return Err("params are an object".into()),
    };
    if p.len() > MOST_MEMBERS {
        return Err("too many params".into());
    }
    if p.keys().any(|k| is_secret_name(k)) {
        return Err("keys and tokens are never params: the window takes a key in File › Options, and Claude Code and Codex use your own login".into());
    }
    Ok(p)
}

/// Whether a param's name is one that would carry a secret (`apiKey`, `API_KEY`, `auth`, …).
fn is_secret_name(name: &str) -> bool {
    let folded: String = name.chars().take(64).filter(|c| *c != '-').flat_map(char::to_lowercase).collect();
    SECRET_NAMES.contains(&folded.as_str())
}

/// Every member of `p` is one of `known` (no `auth` option, no stray member).
fn only(p: &Map<String, Value>, known: &[&str]) -> Result<(), String> {
    match p.keys().find(|k| !known.contains(&k.as_str())) {
        // The name is not echoed: it is the caller's text.
        Some(_) if known.is_empty() => Err("this method takes no params".into()),
        Some(_) => Err(format!("unknown param: this method takes {}", known.iter().map(|k| format!("`{k}`")).collect::<Vec<_>>().join(", "))),
        None => Ok(()),
    }
}

/// A text param, at most [`MOST_TEXT`] bytes and without a control character.
fn text<'a>(p: &'a Map<String, Value>, name: &str) -> Result<&'a str, String> {
    let Some(t) = p.get(name).and_then(Value::as_str) else { return Err(format!("missing `{name}` (text)")) };
    if t.is_empty() || t.len() > MOST_TEXT || t.chars().any(char::is_control) {
        return Err(format!("`{name}` is empty, too long or has control characters"));
    }
    Ok(t)
}

fn flag(p: &Map<String, Value>, name: &str) -> Result<bool, String> {
    p.get(name).and_then(Value::as_bool).ok_or_else(|| format!("`{name}` is true or false"))
}

/// The `runtime` param, a runtime this version knows (never `opencode`).
fn runtime(p: &Map<String, Value>) -> Result<RuntimeId, String> {
    RuntimeId::parse(text(p, "runtime")?).map_err(|_| "`runtime` isn't a runtime this version knows".to_string())
}

/// The `order` param: at most [`MAX_CHAIN`] runtimes' ids.
fn order(p: &Map<String, Value>) -> Result<Vec<RuntimeId>, String> {
    let Some(items) = p.get("order").and_then(Value::as_array) else { return Err("missing `order` (a list of runtimes)".into()) };
    if items.len() > MAX_CHAIN {
        return Err("the order must name each runtime of the chain once".into());
    }
    items
        .iter()
        .map(|v| {
            let t = v.as_str().filter(|t| t.len() <= MOST_TEXT).ok_or("`order` lists runtimes' ids")?;
            RuntimeId::parse(t).map_err(|_| "`order` names a runtime this version doesn't know".to_string())
        })
        .collect()
}

/// The chain with `runtime`'s model and path as `p` gives them: a member left out is unchanged,
/// `null` goes back to the runtime's own.
fn with_options(chain: &Chain, runtime: &RuntimeId, p: &Map<String, Value>) -> Result<Chain, String> {
    if !p.contains_key("model") && !p.contains_key("path") && !p.contains_key("instance") {
        return Err("give `model`, `path` or `instance`".into());
    }
    let mut entry: ChainEntry = chain.get(runtime).cloned().ok_or_else(|| format!("{runtime} isn't in the chain"))?;
    for name in ["model", "path"] {
        let given = match p.get(name) {
            None => continue,
            Some(Value::Null) => None,
            Some(Value::String(s)) if s.len() <= mailcraft_model::ai::runtime::MAX_PATH => Some(s.as_str()),
            Some(_) => return Err(format!("`{name}` is text or null")),
        };
        entry = if name == "model" { entry.with_model(given) } else { entry.with_path(given) }.map_err(|e| e.to_string())?;
    }
    // Which Universal Agent Runtime: the one MailCraft starts, or one the owner already runs
    // (whose tools MailCraft can't turn off: the window says so before it runs this).
    if let Some(given) = p.get("instance") {
        let instance = match given.as_str() {
            Some("managed") => UarInstance::Managed,
            Some("external") => UarInstance::External,
            _ => return Err("`instance` is managed or external".into()),
        };
        entry = entry.with_instance(instance).map_err(|e| e.to_string())?;
    }
    chain.with_entry(entry).map_err(|e| e.to_string())
}

/// Keep `settings` and answer with the list as it now is.
fn keep(host: &dyn AiRuntimes, settings: RuntimeSettings) -> Result<Step, String> {
    host.save(&settings)?;
    Ok(Step::Done(list(host)))
}

/// `ai.runtimes`: each runtime of the chain with its id, name, where, enabled, status, version
/// and path, and the fallback. Never a key, a token, an account from a CLI or a program's output.
fn list(host: &dyn AiRuntimes) -> Value {
    let s = host.settings();
    let rows: Vec<Value> = s
        .chain
        .entries()
        .iter()
        .map(|e| {
            let id = e.runtime();
            let status = host.status(id);
            let path = e.path().or(status.path.as_ref()).map(RuntimePath::as_str);
            json!({
                "id": id.to_string(),
                "name": name(id),
                "where": match host.where_of(id) { Where::Local => "local", Where::Leaves => "leaves" },
                "enabled": e.enabled(),
                "status": status.state.code(),
                "version": status.version.as_deref().filter(|v| looks_like_version(v)),
                "path": path,
            })
        })
        .collect();
    json!({"runtimes": rows, "fallback": s.fallback.name()})
}

/// A version as a program states one (`2.1.4`, `0.34.0-beta+1`), and nothing else it printed.
fn looks_like_version(v: &str) -> bool {
    !v.is_empty() && v.len() <= MOST_VERSION && v.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+'))
}

fn name(id: &RuntimeId) -> String {
    match id {
        RuntimeId::OnDevice => "On this computer".into(),
        RuntimeId::Compat(p) => p.to_string(),
        RuntimeId::ChatGpt => "ChatGPT plan".into(),
        RuntimeId::ClaudeCode => "Claude Code".into(),
        RuntimeId::Codex => "Codex".into(),
        RuntimeId::Uar => "Universal Agent Runtime".into(),
    }
}

/// `ai.signIn`: `{pending: "signIn"}` once the host started it, or words.
fn sign_in(host: &dyn AiRuntimes, runtime: &RuntimeId) -> Result<Step, String> {
    let how = match runtime.kind() {
        RuntimeKind::ChatGpt => SignIn::ChatGpt,
        RuntimeKind::ClaudeCode => SignIn::ClaudeCodeTerminal,
        RuntimeKind::Uar => SignIn::ManagedUar,
        // Nothing is spawned and `codex login` is never run: the owner runs it.
        RuntimeKind::Codex => {
            return Ok(Step::Done(json!({
                "runYourself": "codex login",
                "message": "Codex uses your own login. Run `codex login` in a terminal yourself: MailCraft doesn't run it and keeps nothing of it.",
            })));
        }
        RuntimeKind::OnDevice => return Err("the model on this computer needs no sign-in".into()),
        RuntimeKind::Compat => return Err(format!("{runtime} uses a key, not a sign-in: ai.setRuntimeKey opens the field for it")),
    };
    host.sign_in(how)?;
    Ok(Step::Done(json!({"pending": "signIn", "runtime": runtime.to_string()})))
}

/// `ai.signOut`: what MailCraft keeps itself, never a CLI's own store.
fn sign_out(host: &dyn AiRuntimes, runtime: &RuntimeId) -> Result<Step, String> {
    let what = match runtime {
        RuntimeId::ChatGpt => SignOut::ChatGpt,
        RuntimeId::Compat(p) => SignOut::CompatKey(p.clone()),
        RuntimeId::ClaudeCode => return Ok(Step::Done(own_cli("claude auth logout", "Claude Code"))),
        RuntimeId::Codex => return Ok(Step::Done(own_cli("codex logout", "Codex"))),
        RuntimeId::OnDevice | RuntimeId::Uar => return Err(format!("{runtime} has no sign-in to sign out of")),
    };
    host.sign_out(&what)?;
    Ok(Step::Done(list(host)))
}

fn own_cli(command: &str, who: &str) -> Value {
    json!({
        "runYourself": command,
        "message": format!("{who} uses your own login, kept by its own program. Run `{command}` in a terminal yourself: MailCraft never touches it."),
    })
}

/// `ai.setRuntimeKey`: the key field for a `compat` provider of the chain; refused for every
/// other runtime. The key itself is typed in the window, never given here.
fn key_field(host: &dyn AiRuntimes, runtime: &RuntimeId) -> Result<Step, String> {
    match runtime {
        RuntimeId::Compat(p) if host.settings().chain.get(runtime).is_some() => Ok(Step::OpenKeyField(p.clone())),
        RuntimeId::Compat(_) => Err(format!("{runtime} isn't in the chain")),
        RuntimeId::ClaudeCode => Err("Claude Code uses your own login in the Claude CLI: MailCraft holds no key for it".into()),
        RuntimeId::Codex => Err("Codex uses your own login in the Codex CLI: MailCraft holds no key for it".into()),
        RuntimeId::ChatGpt => Err("the ChatGPT plan takes no key: sign in with ai.signIn".into()),
        RuntimeId::OnDevice | RuntimeId::Uar => Err(format!("{runtime} takes no key")),
    }
}

/// `ai.setConsent {runtime, account, allowed}`: one account's yes or no for one runtime that
/// leaves this computer. A local runtime needs none.
fn consent(host: &dyn AiRuntimes, p: &Map<String, Value>) -> Result<Step, String> {
    let runtime = runtime(p)?;
    let account = text(p, "account")?;
    let allowed = flag(p, "allowed")?;
    if host.where_of(&runtime) == Where::Local {
        return Err(format!("{runtime} keeps mail on this computer: it needs no consent"));
    }
    let s = host.settings();
    let consents = s.consents.with(account, &runtime, allowed).map_err(|e| e.to_string())?;
    host.save(&RuntimeSettings { consents, ..s })?;
    Ok(Step::Done(json!({"runtime": runtime.to_string(), "allowed": allowed})))
}
