// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File › Options › AI assistance (#ai): which model answers first (the primary), what answers
//! when it can't (the backup, an on-device model the owner confirmed, or none), and which
//! accounts' mail may go to a model that sends it to the internet. This module holds what is
//! typed and the window commands; the page is drawn in [`super::settings_page`] and
//! [`super::settings_device`], and what is drawn is worked out in [`super::settings_state`].
//!
//! - **Nothing is sent until a model is set up.** "Not set up" (never chosen) and None (chosen)
//!   are different: None stops everything at once, the backup with it, and keeps the key until
//!   Remove Key.
//! - **On this computer**: an `http` address only when it is this computer (127.0.0.1, ::1,
//!   localhost); anything else `https`. No account needs the owner's permission, and there is no
//!   key.
//! - **A gateway on this computer**: an address on this computer that sends what it receives to a
//!   service on the internet. Chosen after a question; each account's mail goes only once the
//!   owner allowed it, as for a service on the internet.
//! - **Over the internet**: `https` only, chosen after a question (default answer no); the key
//!   goes to the system's credential store alone, through the host ([`crate::AiProviders`]), on a
//!   thread of its own, and is never shown back. Each account's mail goes only once the owner
//!   allowed it in a question of its own ([`super::settings_ask`]), and Stop takes that back at once.
//! - **Test Connection** asks the server for its list of models, with the kept key and nothing
//!   else; the result is said from the failure's kind, never the server's words.
//! - **The backup** is the on-device model at one address the owner confirmed (Use It). A server
//!   that was found but not confirmed is sent nothing. Nothing is looked for while the backup is
//!   None, and otherwise only when the page opens or Look Again is chosen.
//! - **The gateway question** (a server on this computer listing models of several publishers)
//!   is the owner's: Yes, It Does; No, It Doesn't; or Not Now, which sends nothing and leaves it
//!   unanswered.
//!
//! The settings are the owner's: an agent can neither set up a model, keep a key, test one,
//! confirm the on-device model, nor answer the questions (`ui.aiSettings`, `ui.aiConsent`,
//! `ui.testProvider` and `ui.confirmOnDevice` are never for agents).

use std::sync::mpsc::{Receiver, TryRecvError};

use mailcraft_model::ai::AiKind;
use serde_json::{Value, json};

pub(crate) use super::settings_ask::{consent_command, follow_gateway};
use super::settings_state::{self as st, DEFAULT_LOCAL_WINDOW, DEFAULT_REMOTE_WINDOW, GatewayAsk, TestFailure, TestState, WindowInput, WindowSource};
use super::{Provider, Providers, flag, text};
use crate::MailApp;
use crate::dialogs::{Dialog, Question};

/// Most characters of an address, a model's name, a key.
pub(crate) const MAX_ADDRESS: usize = 2_000;
pub(crate) const MAX_MODEL: usize = 200;
pub(crate) const MAX_KEY: usize = 4_096;
/// The backup's name as the host knows it.
pub(crate) const ON_DEVICE: &str = "apple-on-device";

/// Where the model runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Kind {
    /// No model: nothing is sent.
    #[default]
    None,
    Local,
    /// A gateway on this computer that sends what it receives to the internet.
    Gateway,
    Remote,
}

impl Kind {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Kind::None => "none",
            Kind::Local => "local",
            Kind::Gateway => "gateway",
            Kind::Remote => "remote",
        }
    }

    /// A kind by its code (`ui.aiSettings`'s `kind`); `None` for one this version doesn't know.
    pub(crate) fn from_code(code: &str) -> Option<Kind> {
        match code {
            "none" => Some(Kind::None),
            "local" => Some(Kind::Local),
            "gateway" => Some(Kind::Gateway),
            "remote" => Some(Kind::Remote),
            _ => None,
        }
    }

    /// The kind of a provider the host has, by its `kind`.
    pub(crate) fn of(p: &Provider) -> Kind {
        if p.remote() {
            Kind::Remote
        } else if p.gateway() {
            Kind::Gateway
        } else {
            Kind::Local
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Kind::None => tl!("None: turn off AI assistance"),
            Kind::Local => tl!("A model on this computer"),
            Kind::Gateway => tl!("A gateway on this computer"),
            Kind::Remote => tl!("A model service on the internet"),
        }
    }

    /// Where mail goes for this kind, as the note under the Kind row says.
    pub(crate) fn note(self) -> &'static str {
        match self {
            Kind::None => tl!("Nothing is sent anywhere, and the backup isn't used either."),
            Kind::Local => tl!(
                "MailCraft sends only to the address on this computer, and needs no permission for each account. Your mail still goes to the program at that address, so use one you trust."
            ),
            Kind::Gateway => tl!(
                "The gateway runs on this computer but sends your mail on to a service on the internet. Each account's mail goes only after you allow it below."
            ),
            Kind::Remote => {
                tl!("Your mail goes to the address below over an encrypted connection, and only from the accounts you allow.")
            }
        }
    }

    /// Whether a provider of this kind is sent an account's mail only once the owner allowed it.
    pub(crate) fn needs_consent(self) -> bool {
        matches!(self, Kind::Gateway | Kind::Remote)
    }

    /// The context window used when nothing else says.
    pub(crate) fn default_window(self) -> u32 {
        match self {
            Kind::Gateway | Kind::Remote => DEFAULT_REMOTE_WINDOW,
            Kind::None | Kind::Local => DEFAULT_LOCAL_WINDOW,
        }
    }

    /// The id of a provider of this kind that the owner makes from scratch here (letters only:
    /// it names the key in the credential store). Providers the host already has keep their own.
    fn new_id(self) -> &'static str {
        match self {
            Kind::None => "",
            Kind::Local => "local",
            Kind::Gateway => "gateway",
            Kind::Remote => "remote",
        }
    }
}

/// What is wrong with an address, in words for the owner; `None` when it may be used.
pub(crate) fn address_problem(kind: Kind, address: &str) -> Option<&'static str> {
    let address = address.trim();
    if address.is_empty() || kind == Kind::None {
        return None;
    }
    let Some((scheme, rest)) = address.split_once("://") else {
        return Some(tl!("That isn't an address: write it as http://127.0.0.1:11434/v1 or https://…"));
    };
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Some(tl!("That isn't an address: write it as http://127.0.0.1:11434/v1 or https://…"));
    }
    if kind == Kind::Remote && scheme != "https" {
        return Some(tl!("A model service on the internet needs an https address."));
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.is_empty() || authority.contains('@') {
        return Some(tl!("That isn't an address: write it as http://127.0.0.1:11434/v1 or https://…"));
    }
    if kind == Kind::Gateway && !loopback(authority) {
        return Some(tl!("A gateway on this computer needs an address on this computer (127.0.0.1, ::1 or localhost)."));
    }
    if scheme == "http" && !loopback(authority) {
        return Some(tl!("On this computer, an http address must be this computer (127.0.0.1, ::1 or localhost). Use https for anything else."));
    }
    None
}

/// Whether `authority` (a host and maybe a port) is this computer.
fn loopback(authority: &str) -> bool {
    let host = if let Some(inside) = authority.strip_prefix('[') {
        inside.split(']').next().unwrap_or_default()
    } else {
        authority.rsplit_once(':').map_or(authority, |(host, _)| host)
    };
    let host = host.trim().to_ascii_lowercase();
    host == "localhost"
        || host.parse::<std::net::Ipv4Addr>().is_ok_and(|ip| ip.is_loopback())
        || host.parse::<std::net::Ipv6Addr>().is_ok_and(|ip| ip.is_loopback())
}

/// What is being typed on the page, and what the host was asked.
#[derive(Default)]
pub(crate) struct Draft {
    /// The fields were filled from the host's settings.
    pub(crate) loaded: bool,
    pub(crate) kind: Kind,
    /// The provider being edited, by the host's own id (a new one: [`Kind::new_id`]).
    pub(crate) id: String,
    pub(crate) address: String,
    pub(crate) model: String,
    pub(crate) embedding: String,
    /// The context window as typed (digits).
    pub(crate) window: String,
    pub(crate) window_source: WindowSource,
    /// The number typed was outside the range and was set to this.
    pub(crate) window_clamped: Option<u32>,
    /// What was typed isn't a number: not saved.
    pub(crate) window_bad: bool,
    /// A key being typed: never kept past [`save_key`], never shown back.
    pub(crate) key: String,
    pub(crate) key_shown: bool,
    /// Whether the credential store keeps a key for the provider (`None`: not known yet).
    pub(crate) key_kept: Option<bool>,
    pub(crate) key_asked: Option<Receiver<Result<bool, String>>>,
    key_saving: Option<Receiver<Result<(), String>>>,
    pub(crate) key_forgetting: Option<Receiver<Result<(), String>>>,
    /// The credential store couldn't be used.
    pub(crate) store_trouble: bool,
    /// What is wrong with what was typed, if anything.
    pub(crate) problem: Option<&'static str>,
    /// The page scrolls to the section at its next frame.
    pub(crate) scroll: bool,
    /// Test Connection: where it is, its answer on the way, and the provider it tested.
    pub(crate) test: TestState,
    test_rx: Option<Receiver<Result<Value, String>>>,
    test_of: String,
    /// "Type a name…" was chosen in the model picker: the field again.
    pub(crate) type_name: bool,
    /// Level 1, from the last test: the server lists hosted-service models (its address, an id).
    pub(crate) refused: Option<(String, Option<String>)>,
    /// The on-device model being looked for, and a confirmation on its way.
    pub(crate) looking: Option<Receiver<Result<Value, String>>>,
    confirming: Option<Receiver<Result<(), String>>>,
    /// Stop Using It takes the keyboard at the next frame (after Use It).
    pub(crate) focus_stop: bool,
    /// The pass the page was last drawn in: a gap means it was opened again.
    pub(crate) seen_pass: Option<u64>,
    /// The gateway question this window is showing, by provider.
    pub(crate) gateway_shown: Option<String>,
    /// Not Now was chosen for the gateway question about this address: waiting for an answer.
    pub(crate) gateway_waiting: Option<String>,
    /// "Look on ports" as typed, and what is wrong with it (in our words) when the host refused it.
    pub(crate) ports: String,
    pub(crate) ports_problem: Option<String>,
    /// The on-device model was looked for in this session (the host never looks by itself).
    pub(crate) looked: bool,
}

impl Draft {
    /// The fields as the host's settings have them, for `kind` (or the kind in use).
    pub(crate) fn load(&mut self, providers: &Providers, kind: Option<Kind>) {
        let kind = kind.unwrap_or_else(|| providers.active().map_or(Kind::None, Kind::of));
        let p = provider_for(providers, kind).unwrap_or_default();
        self.id = if p.id.is_empty() { kind.new_id().to_string() } else { p.id.clone() };
        self.kind = kind;
        self.address = p.base_url;
        self.model = p.model;
        self.embedding = p.embedding_model;
        let window = Some(p.context_window).filter(|n| (st::MIN_WINDOW..=st::MAX_WINDOW).contains(n));
        self.window_source = if window.is_some_and(|n| n != kind.default_window()) { WindowSource::Owner } else { WindowSource::Default };
        self.window = window.unwrap_or(kind.default_window()).to_string();
        self.window_clamped = None;
        self.window_bad = false;
        self.key.clear();
        self.key_shown = false;
        self.key_kept = None;
        self.key_asked = None;
        self.problem = None;
        self.type_name = false;
        self.loaded = true;
    }

    /// The context window that will be used: what was typed, else what the server said, else the
    /// kind's default.
    pub(crate) fn window_tokens(&self) -> u32 {
        match st::parse_window(&self.window) {
            WindowInput::Ok(n) | WindowInput::Clamped(n) => n,
            WindowInput::Empty | WindowInput::Bad => self.server_window().unwrap_or(self.kind.default_window()),
        }
    }

    /// What the server said of the window: of the model chosen, else of the server.
    pub(crate) fn server_window(&self) -> Option<u32> {
        let TestState::Ok { models, context_window } = &self.test else { return None };
        models.iter().find(|m| m.id == self.model.trim()).and_then(|m| m.context_window).or(*context_window)
    }

    /// The model picker is shown (a test listed models, and the owner didn't ask to type one).
    pub(crate) fn picking(&self) -> bool {
        !self.type_name && !self.test.models().is_empty()
    }
}

/// The provider of `kind` the page shows: the one in use when it is of that kind, else the first
/// of that kind the host has, whatever its id.
fn provider_for(providers: &Providers, kind: Kind) -> Option<Provider> {
    let of_kind = |p: &Provider| kind != Kind::None && Kind::of(p) == kind;
    providers.active().filter(|p| of_kind(p)).or_else(|| providers.all.iter().map(|(p, _)| p).find(|p| of_kind(p))).cloned()
}

/// The provider as typed on the page, as JSON the host takes; what the host keeps of it that the
/// page doesn't show is left as it is. `None` when it can't be (no kind, an address problem).
fn typed(app: &mut MailApp) -> Result<Option<(String, Value)>, String> {
    let kind = app.ai.settings.kind;
    if kind == Kind::None {
        return Ok(None);
    }
    let address = app.ai.settings.address.trim().to_string();
    if let Some(problem) = address_problem(kind, &address) {
        app.ai.settings.problem = Some(problem);
        return Err(problem.to_string());
    }
    app.ai.settings.problem = None;
    // The context window: a number out of range is set to the nearest end and said; anything
    // else that isn't a number is said and not saved.
    let d = &mut app.ai.settings;
    let window = match st::parse_window(&d.window) {
        WindowInput::Ok(n) => {
            (d.window_clamped, d.window_bad) = (None, false);
            Some(n)
        }
        WindowInput::Clamped(n) => {
            (d.window, d.window_clamped, d.window_bad, d.window_source) = (n.to_string(), Some(n), false, WindowSource::Owner);
            Some(n)
        }
        WindowInput::Empty => {
            (d.window_clamped, d.window_bad) = (None, false);
            Some(d.server_window().unwrap_or(d.kind.default_window()))
        }
        WindowInput::Bad => {
            (d.window_clamped, d.window_bad) = (None, true);
            None
        }
    };
    if address.is_empty() {
        return Ok(None);
    }
    let id = if d.id.is_empty() { kind.new_id().to_string() } else { d.id.clone() };
    let providers = Providers::of(app);
    let mut value = providers.get(&id).map(|(_, v)| v.clone()).filter(Value::is_object).unwrap_or_else(|| json!({}));
    let d = &app.ai.settings;
    if let Some(map) = value.as_object_mut() {
        map.insert("id".into(), json!(id));
        map.insert("kind".into(), json!(kind.code()));
        map.insert("base_url".into(), json!(address));
        map.insert("model".into(), json!(d.model.trim()));
        map.insert("embedding_model".into(), json!(d.embedding.trim()));
        if let Some(n) = window {
            map.insert("context_window".into(), json!(n));
            map.remove("contextWindow");
        }
    }
    Ok(Some((id, value)))
}

/// What is typed on the page, given to the host: checked first. Made the one in use when it has
/// an address.
pub(crate) fn apply(app: &mut MailApp) -> Result<(), String> {
    let Some((id, value)) = typed(app)? else { return Ok(()) };
    let host = app.services.ai_providers.as_ref().ok_or_else(|| tl!("This build can't use a model").to_string())?;
    let given = host.set_provider(&value).and_then(|()| host.set_active(&id));
    super::providers_changed(app);
    if given.is_err() {
        app.ai.settings.problem = Some(tl!("These settings can't be used. Check the address and the model's name."));
        return Err(tl!("These settings can't be used. Check the address and the model's name.").to_string());
    }
    Ok(())
}

/// The model chosen: None stops everything at once; a gateway or another computer is asked about
/// first (`confirmed`: the owner said yes).
pub(crate) fn set_kind(app: &mut MailApp, kind: Kind, confirmed: bool) -> Result<(), String> {
    let was = app.ai.settings.kind;
    if kind.needs_consent() && kind != was && !confirmed {
        return ask_kind(app, kind);
    }
    if kind != was {
        // What was under way goes to where it was set up for, or nowhere: stopped at once.
        super::cancel_live(app, None);
    }
    let providers = Providers::of(app);
    // Between a model and a gateway on this computer, the same address keeps what was typed (and
    // the host takes the consents back: a gateway starts with none).
    let same_place = matches!((was, kind), (Kind::Local, Kind::Gateway) | (Kind::Gateway, Kind::Local))
        && !app.ai.settings.address.trim().is_empty()
        && provider_for(&providers, kind).is_none();
    if same_place && app.services.ai_providers.is_some() {
        let d = &mut app.ai.settings;
        if d.window_source == WindowSource::Default {
            d.window = kind.default_window().to_string();
        }
        d.kind = kind;
        d.refused = None;
        return apply(app);
    }
    app.ai.settings.load(&providers, Some(kind));
    app.ai.settings.test = TestState::Idle;
    app.ai.settings.refused = None;
    // A provider of that kind set up before is the one in use again; else none is.
    let target = provider_for(&providers, kind).filter(|p| kind != Kind::None && !p.base_url.trim().is_empty()).map(|p| p.id);
    let done = match app.services.ai_providers.as_ref() {
        None => return Err(tl!("This build can't use a model").to_string()),
        Some(host) => {
            let done = host.set_active(target.as_deref().unwrap_or_default());
            // A primary of None turns the backup off too.
            if kind == Kind::None && providers.backup_on_device() {
                let _ = host.set_backup("");
            }
            done
        }
    };
    super::providers_changed(app);
    if kind == Kind::None {
        done.map_err(|_| tl!("The model couldn't be turned off").to_string())?;
        app.status(tl!("AI assistance is turned off. Nothing is sent anywhere."));
    } else {
        done.map_err(|_| tl!("These settings can't be used. Check the address and the model's name.").to_string())?;
    }
    Ok(())
}

/// The question before a gateway or a model service on the internet is chosen.
fn ask_kind(app: &mut MailApp, kind: Kind) -> Result<(), String> {
    if app.dialog.is_some() {
        return Err(tl!("Close the dialog that is open first").to_string());
    }
    let (title, text) = if kind == Kind::Gateway {
        (
            tl!("Use a Gateway That Sends Mail to the Internet?"),
            tl!(
                "A gateway on this computer passes what it receives to a service on the internet. What you type in Find by Description and your questions go to it.\n\nNo mail goes there until you allow an account, one at a time, on this page.\n\nThe service behind the gateway may keep what it receives under its own terms. MailCraft can't see where the gateway sends it."
            ),
        )
    } else {
        (
            tl!("Use a Model Service on the Internet?"),
            tl!(
                "What you type in Find by Description and your questions will go to the address you enter next, over an encrypted connection.\n\nNo mail goes there until you allow an account, one at a time, on this page.\n\nThe service may keep what it receives under its own terms. MailCraft can't see what it does with it."
            ),
        )
    };
    let ask = Question {
        title: title.to_string(),
        text: text.to_string(),
        action: tl!("Use It").to_string(),
        with: Value::Null,
        more: Vec::new(),
        note: None,
        cancel: Some(tl!("Not Now").to_string()),
        tick: None,
    };
    app.dialog = Some(Dialog::Confirm { command: "ui.aiSettings".to_string(), params: json!({"kind": kind.code()}), ask });
    Ok(())
}

/// The backup chosen: the on-device model (looked for at once) or none.
pub(crate) fn set_backup(app: &mut MailApp, on_device: bool) -> Result<(), String> {
    let host = app.services.ai_providers.as_ref().ok_or_else(|| tl!("This build can't use a model").to_string())?;
    if on_device && Providers::of(app).explicit_none() {
        return Err(tl!("AI assistance is turned off.").to_string());
    }
    let set = host.set_backup(if on_device { ON_DEVICE } else { "" });
    super::providers_changed(app);
    set.map_err(|_| tl!("The backup couldn't be changed.").to_string())?;
    if on_device {
        look(app);
    } else {
        app.ai.settings.looking = None;
        super::cancel_live(app, Some(AiKind::Answer));
        app.status(tl!("The on-device model is no longer the backup."));
    }
    Ok(())
}

/// Looks for the on-device model now (never while the backup is None).
pub(crate) fn look(app: &mut MailApp) {
    let Some(host) = app.services.ai_providers.as_ref() else { return };
    if !Providers::of(app).backup_on_device() {
        return;
    }
    app.ai.settings.looking = Some(host.discover_on_device());
    app.ai.settings.looked = true;
}

/// On the first question of the session, while the backup isn't None: the on-device model is
/// looked for (the host never looks from inside a request).
pub(crate) fn look_before_first_question(app: &mut MailApp) {
    let providers = Providers::of(app);
    if !app.ai.settings.looked && app.ai.settings.looking.is_none() && providers.backup_on_device() && !providers.explicit_none() {
        look(app);
    }
}

/// "Look on ports": the owner's list given to the host, which says what is wrong with it; then
/// looked on at once.
pub(crate) fn set_ports(app: &mut MailApp) -> Result<(), String> {
    let host = app.services.ai_providers.as_ref().ok_or_else(|| tl!("This build can't use a model").to_string())?;
    let typed = app.ai.settings.ports.trim().to_string();
    let set = host.set_device_ports(&typed);
    super::providers_changed(app);
    match set {
        Ok(()) => {
            app.ai.settings.ports_problem = None;
            look(app);
            Ok(())
        }
        // The host's words are English and its own: the page says it in the owner's language.
        Err(_) => {
            let words = tl!("Write port numbers from 1 to 65535, separated by commas.").to_string();
            app.ai.settings.ports_problem = Some(words.clone());
            Err(words)
        }
    }
}

/// "Stop Using It": the confirmed address is forgotten at once, and an answer under way from the
/// backup is called off.
pub(crate) fn stop_using(app: &mut MailApp) -> Result<(), String> {
    let host = app.services.ai_providers.as_ref().ok_or_else(|| tl!("This build can't use a model").to_string())?;
    let stopped = host.stop_using_on_device();
    super::providers_changed(app);
    stopped.map_err(|_| tl!("The on-device model couldn't be let go of.").to_string())?;
    super::cancel_live(app, Some(AiKind::Answer));
    app.status(tl!("The on-device model is no longer the backup."));
    Ok(())
}

/// `ui.aiSettings`: opens File › Options at AI assistance, with what is given typed in and used.
/// See `UI_COMMANDS` for its params and answer.
pub(crate) fn command(app: &mut MailApp, p: &Value) -> Result<Value, String> {
    if p.get("kind").is_some_and(|k| !k.is_string() && !k.is_null()) {
        return Err("`kind` is none, local, gateway or remote".into());
    }
    let kind = match p.get("kind").and_then(Value::as_str) {
        None => None,
        Some(code) => Some(Kind::from_code(code).ok_or("`kind` is none, local, gateway or remote")?),
    };
    let confirmed = flag(p, "confirmed")? == Some(true);
    let address = text(p, "address", MAX_ADDRESS)?;
    let model = text(p, "model", MAX_MODEL)?;
    let embedding = text(p, "embeddingModel", MAX_MODEL)?;
    let window = match p.get("contextWindow") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s.chars().count() <= st::MAX_WINDOW_DIGITS + 2 => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.as_u64().map_or_else(|| "x".to_string(), |n| n.to_string())),
        Some(_) => return Err("`contextWindow` is a number of tokens".into()),
    };
    let backup = match p.get("backup").and_then(Value::as_str) {
        None if p.get("backup").is_none_or(Value::is_null) => None,
        Some("onDevice") => Some(true),
        Some("none") => Some(false),
        _ => return Err("`backup` is onDevice or none".into()),
    };
    let look_again = flag(p, "lookAgain")? == Some(true);
    let stop_using_it = flag(p, "stopUsing")? == Some(true);
    let ports = text(p, "ports", 200)?;
    let gateway_answer = flag(p, "gatewayAnswer")?;
    app.dialog = app.dialog.take().filter(|d| !matches!(d, Dialog::Confirm { command, .. } if command == "ui.aiSettings"));
    let providers = Providers::of(app);
    if !app.ai.settings.loaded {
        app.ai.settings.load(&providers, None);
    }
    // The answer to the gateway question stays where the owner is: File doesn't open over the
    // window, so Find by Description's request, which waits on that answer, goes on (M1,
    // SCN-BASE-003).
    if let Some(sends) = gateway_answer {
        super::settings_ask::answer_gateway(app, p, sends)?;
        return Ok(state(app));
    }
    app.ui.backstage = true;
    app.ui.backstage_page = "options".into();
    app.ai.settings.scroll = true;
    if let Some(kind) = kind
        && (app.services.ai_providers.is_some() || kind == Kind::None)
    {
        set_kind(app, kind, confirmed)?;
    }
    let typed_any = address.is_some() || model.is_some() || embedding.is_some() || window.is_some();
    let d = &mut app.ai.settings;
    if let Some(address) = address {
        d.address = address;
    }
    if let Some(model) = model {
        d.model = model;
    }
    if let Some(embedding) = embedding {
        d.embedding = embedding;
    }
    if let Some(window) = window {
        d.window = window;
        d.window_source = WindowSource::Owner;
    }
    if typed_any {
        // What is wrong is said on the page too.
        let _ = apply(app);
    }
    if let Some(on) = backup {
        set_backup(app, on)?;
    }
    if let Some(ports) = ports {
        app.ai.settings.ports = ports;
        // What is wrong is said on the page too.
        let _ = set_ports(app);
    }
    if stop_using_it {
        stop_using(app)?;
    }
    if look_again {
        look(app);
    }
    Ok(state(app))
}

/// What `ui.aiSettings` answers: the page as it stands, never a key or a key's hash.
pub(crate) fn state(app: &MailApp) -> Value {
    let d = &app.ai.settings;
    let providers = Providers::of(app);
    let never = d.kind == Kind::None && providers.never_configured;
    let card = st::card_id(providers.backup_on_device(), d.looking.is_some(), &providers.device);
    let device_code = match card {
        st::CardId::Looking => "looking",
        st::CardId::Found => "found",
        st::CardId::Using => "using",
        st::CardId::Changed => "changed",
        st::CardId::NotFound => "none",
        st::CardId::Off => "off",
        st::CardId::Unavailable => "na",
    };
    let mut test = serde_json::Map::new();
    test.insert("state".into(), json!(d.test.code()));
    match &d.test {
        TestState::Ok { models, .. } => {
            test.insert("count".into(), json!(models.len()));
        }
        TestState::Failed { why, .. } => {
            test.insert("kind".into(), json!(why.code()));
        }
        TestState::Idle | TestState::Testing => {}
    }
    let asking = match &app.dialog {
        Some(Dialog::Confirm { command, params, .. }) if command == "ui.aiSettings" => {
            if params.get("gatewayProvider").is_some() {
                Some("gatewayQuestion")
            } else {
                params.get("kind").and_then(Value::as_str).and_then(|k| ["remote", "gateway"].into_iter().find(|c| *c == k))
            }
        }
        _ => None,
    };
    json!({
        "kind": d.kind.code(),
        "primaryKind": if never { "never" } else { d.kind.code() },
        "id": d.id,
        "address": d.address,
        "model": d.model,
        "embeddingModel": d.embedding,
        "problem": d.problem,
        "active": providers.active,
        "keyKept": d.key_kept,
        "backup": if providers.backup_on_device() { "onDevice" } else { "none" },
        "test": Value::Object(test),
        "models": d.test.models().iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        "contextWindow": d.window_tokens(),
        "contextWindowSource": d.window_source.code(),
        "device": {
            "state": device_code,
            "candidates": providers.device.candidates.iter().map(|c| c.address.as_str()).collect::<Vec<_>>(),
            "confirmed": providers.device.confirmed,
        },
        "asking": asking,
    })
}

/// `ui.testProvider`: `{"provider"?: id}` (left out: the one typed on the page). Asks the host to
/// read the server's list of models with the kept key; the result comes on the page.
pub(crate) fn test_command(app: &mut MailApp, p: &Value) -> Result<Value, String> {
    let wanted = text(p, "provider", 512)?;
    if app.services.ai_providers.is_none() {
        return Err(tl!("This build can't use a model").to_string());
    }
    let (id, value) = match wanted.filter(|w| *w != app.ai.settings.id) {
        Some(other) => {
            let providers = Providers::of(app);
            let (_, v) = providers.get(&other).ok_or("`provider` is the id of a model provider that is set up")?;
            (other, v.clone())
        }
        None => match typed(app)? {
            Some(typed) => typed,
            None => return Err(tl!("Write the address first").to_string()),
        },
    };
    let host = app.services.ai_providers.as_ref().ok_or_else(|| tl!("This build can't use a model").to_string())?;
    let rx = host.test_provider(&value);
    let d = &mut app.ai.settings;
    (d.test, d.test_rx, d.test_of, d.type_name) = (TestState::Testing, Some(rx), id, false);
    Ok(json!({"test": {"state": "testing"}}))
}

/// `ui.confirmOnDevice`: `{"address"}`, one address the on-device model card found: made the
/// backup. Only that one is ever sent anything.
pub(crate) fn confirm_command(app: &mut MailApp, p: &Value) -> Result<Value, String> {
    let address = text(p, "address", MAX_ADDRESS)?.ok_or("`address` is an address the on-device model card found")?;
    let providers = Providers::of(app);
    if !providers.backup_on_device() {
        return Err(tl!("Choose Apple's on-device model as the backup first").to_string());
    }
    // One the card lists, or the confirmed one whose identity changed (Use It again).
    let listed = providers.device.candidates.iter().any(|c| c.address == address) || providers.device.confirmed.as_deref() == Some(address.as_str());
    if !listed {
        return Err("`address` is an address the on-device model card found".into());
    }
    let host = app.services.ai_providers.as_ref().ok_or_else(|| tl!("This build can't use a model").to_string())?;
    app.ai.settings.confirming = Some(host.confirm_on_device(&address));
    Ok(json!({"device": {"confirming": address}}))
}

/// What came of the host's work, taken in: the credential store, a test, the on-device model.
pub(crate) fn poll(app: &mut MailApp) {
    let mut said: Option<String> = None;
    let d = &mut app.ai.settings;
    if let Some(rx) = &d.key_asked {
        match rx.try_recv() {
            Ok(Ok(kept)) => (d.key_kept, d.key_asked, d.store_trouble) = (Some(kept), None, false),
            Ok(Err(_)) | Err(TryRecvError::Disconnected) => (d.key_kept, d.key_asked, d.store_trouble) = (Some(false), None, true),
            Err(TryRecvError::Empty) => {}
        }
    }
    if let Some(rx) = &d.key_saving {
        match rx.try_recv() {
            Ok(Ok(())) => {
                (d.key_kept, d.key_saving, d.store_trouble) = (Some(true), None, false);
                said = Some(tl!("The key is kept in the system's credential store.").to_string());
            }
            Ok(Err(_)) | Err(TryRecvError::Disconnected) => {
                (d.key_saving, d.store_trouble) = (None, true);
                said = Some(tl!("The key couldn't be kept: the system's credential store isn't available.").to_string());
            }
            Err(TryRecvError::Empty) => {}
        }
    }
    if let Some(rx) = &d.key_forgetting {
        match rx.try_recv() {
            Ok(Ok(())) => {
                (d.key_kept, d.key_forgetting) = (Some(false), None);
                said = Some(tl!("The key was removed from the credential store.").to_string());
            }
            Ok(Err(_)) | Err(TryRecvError::Disconnected) => {
                d.key_forgetting = None;
                said = Some(tl!("The key couldn't be removed from the credential store.").to_string());
            }
            Err(TryRecvError::Empty) => {}
        }
    }
    if let Some(s) = said.take() {
        app.status(s);
    }
    poll_test(app);
    poll_device(app);
    let d = &app.ai.settings;
    let waiting = d.key_asked.is_some()
        || d.key_saving.is_some()
        || d.key_forgetting.is_some()
        || d.test_rx.is_some()
        || d.looking.is_some()
        || d.confirming.is_some();
    if waiting && let Some(ctx) = &app.ctx {
        crate::software::repaint_after(ctx, std::time::Duration::from_millis(200));
    }
}

/// A test's answer, taken in.
fn poll_test(app: &mut MailApp) {
    let Some(rx) = &app.ai.settings.test_rx else { return };
    let answer = match rx.try_recv() {
        Ok(answer) => answer,
        Err(TryRecvError::Empty) => return,
        // A host that doesn't test: said as a failure with no kind.
        Err(TryRecvError::Disconnected) => Err(String::new()),
    };
    let tested = app.ai.settings.test_of.clone();
    let waiting = app.services.ai_providers.as_ref().and_then(|h| h.gateway_question()).as_ref().and_then(GatewayAsk::read);
    let waiting = waiting.is_some_and(|q| q.provider == tested);
    let test = st::test_result(answer, waiting);
    super::providers_changed(app);
    let d = &mut app.ai.settings;
    d.test_rx = None;
    d.refused = match &test {
        TestState::Failed { why: TestFailure::HostedModels(id), .. } => Some((super::shown_address(&d.address), id.clone())),
        _ => None,
    };
    d.test = test;
    // What the server says of the window is used unless the owner typed a number.
    if d.window_source != WindowSource::Owner
        && let Some(n) = d.server_window()
    {
        d.window = n.to_string();
        d.window_source = WindowSource::Server;
    }
}

/// The on-device model looked for, or confirmed, taken in.
fn poll_device(app: &mut MailApp) {
    if let Some(rx) = &app.ai.settings.looking {
        match rx.try_recv() {
            Ok(_) | Err(TryRecvError::Disconnected) => {
                app.ai.settings.looking = None;
                super::providers_changed(app);
            }
            Err(TryRecvError::Empty) => {}
        }
    }
    let Some(rx) = &app.ai.settings.confirming else { return };
    let came = match rx.try_recv() {
        Ok(came) => came,
        Err(TryRecvError::Empty) => return,
        Err(TryRecvError::Disconnected) => Err(String::new()),
    };
    app.ai.settings.confirming = None;
    super::providers_changed(app);
    match came {
        Ok(()) => {
            let at = Providers::of(app).device.confirmed.as_deref().map(super::shown_address).unwrap_or_default();
            app.ai.settings.focus_stop = true;
            app.status(crate::i18n::fmt(
                tl!("Apple's on-device model at {address} is now the backup. Your mail stays on this computer."),
                &[("address", &at)],
            ));
        }
        Err(_) => app.status(tl!("The on-device model couldn't be made the backup. Nothing was sent to it.")),
    }
}

/// The key typed, given to the credential store (never kept here past this).
pub(crate) fn save_key(app: &mut MailApp) {
    let key = std::mem::take(&mut app.ai.settings.key);
    let key = key.trim().to_string();
    app.ai.settings.key_shown = false;
    if key.is_empty() {
        return;
    }
    if let Some(host) = app.services.ai_providers.as_ref() {
        app.ai.settings.key_saving = Some(host.save_key(&app.ai.settings.id, key));
    }
}

/// The page was opened again (not drawn the frame before): a test of a former visit is let go
/// of, and the on-device model is looked for (never while the backup is None).
pub(crate) fn visit(app: &mut MailApp, pass: Option<u64>) {
    let fresh = match (app.ai.settings.seen_pass, pass) {
        (Some(seen), Some(now)) => now > seen.saturating_add(1),
        (None, _) => true,
        (Some(_), None) => false,
    };
    app.ai.settings.seen_pass = pass.or(app.ai.settings.seen_pass);
    if !fresh {
        return;
    }
    let ports = Providers::of(app).device_ports.iter().map(u16::to_string).collect::<Vec<_>>().join(", ");
    let d = &mut app.ai.settings;
    (d.test, d.test_rx, d.type_name, d.refused, d.ports_problem, d.ports) = (TestState::Idle, None, false, None, None, ports);
    look(app);
}
