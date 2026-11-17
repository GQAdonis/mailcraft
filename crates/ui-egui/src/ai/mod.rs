// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The window's AI surfaces (#ai): Find by Description under the search box ([`narrow`]), the
//! Answers pane at the right of Mail ([`answers`]), and File › Options › AI assistance with the
//! per-account question ([`settings`]).
//!
//! The window only collects what the owner wants and runs a command: `ai.narrow`,
//! `ai.narrowResult`, `view.search`, `ai.ask`, `ai.answerResult`, `ai.cancel` (the engine's), and
//! the providers' settings through [`crate::AiProviders`] (the host's). It never talks to a model:
//! a request the engine made waits in the session, and `MailApp::model_tick` hands it to the host.
//!
//! What a model says is untrusted text: an answer is drawn as display blocks
//! ([`mailcraft_html::markdown_blocks`], [`answer_text`]), never as HTML, and nothing it names is
//! fetched. Why a request failed is said in the window's own words ([`errors`]), never the
//! server's or the model's.
//!
//! Window commands (`UI_COMMANDS`), none of them for agents: `ui.findByDescription`,
//! `ui.answers`, `ui.aiSettings`, `ui.aiConsent`, `ui.testProvider`, `ui.confirmOnDevice`.

pub(crate) mod answer_text;
pub(crate) mod answers;
pub(crate) mod errors;
pub(crate) mod narrow;
pub(crate) mod settings;
pub(crate) mod settings_ask;
pub(crate) mod settings_device;
pub(crate) mod settings_page;
pub(crate) mod settings_state;

use std::hash::{BuildHasher, Hasher};

use mailcraft_model::ai::{AiKind, RequestState};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::MailApp;

/// Longest address shown, in characters.
const MAX_SHOWN_ADDRESS: usize = 160;

/// The window's AI surfaces, as they stand.
#[derive(Default)]
pub(crate) struct AiUi {
    /// Find by Description, while its panel is open under the search box.
    pub(crate) narrow: Option<narrow::Panel>,
    /// The Answers pane, while it is open.
    pub(crate) answers: Option<answers::Pane>,
    /// File › Options › AI assistance: what is being typed there, and what the host is asked.
    pub(crate) settings: settings::Draft,
    /// What the host said of the providers, and in which pass: asked once a frame
    /// ([`Providers::of`]), and again after the window changed them ([`providers_changed`]).
    providers: std::cell::RefCell<Option<(u64, Providers)>>,
}

/// The window changed the providers' settings: the next look asks the host again.
pub(crate) fn providers_changed(app: &mut MailApp) {
    *app.ai.providers.get_mut() = None;
}

/// A model provider as the host hands it over ([`crate::AiProviders::providers`]): never with a
/// key. Anything missing or of the wrong kind reads as empty.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub(crate) struct Provider {
    pub(crate) id: String,
    /// `local`, `gateway` or `remote`.
    pub(crate) kind: String,
    pub(crate) base_url: String,
    pub(crate) model: String,
    pub(crate) embedding_model: String,
    pub(crate) pinned_sha256: String,
    pub(crate) consented_accounts: Vec<String>,
    /// How many tokens the model takes, question and answer together (0: not said).
    pub(crate) context_window: u32,
}

impl Provider {
    pub(crate) fn remote(&self) -> bool {
        self.kind == "remote"
    }

    /// A gateway on this computer that sends what it receives to the internet.
    pub(crate) fn gateway(&self) -> bool {
        self.kind == "gateway"
    }

    /// Whether an account's mail goes to it only once the owner allowed it: a service on the
    /// internet, or a gateway to one.
    pub(crate) fn needs_consent(&self) -> bool {
        self.remote() || self.gateway()
    }

    /// Whether the owner allowed it the mail of `account` (an address).
    pub(crate) fn allows(&self, account: &str) -> bool {
        let account = account.trim();
        self.consented_accounts.iter().any(|a| a.trim().eq_ignore_ascii_case(account))
    }

    /// Its address as it is shown.
    pub(crate) fn address(&self) -> String {
        shown_address(&self.base_url)
    }

    /// The host it is on, as it is shown.
    pub(crate) fn host(&self) -> String {
        shown_host(&self.base_url)
    }

    /// Its model's name as it is shown: at most 40 characters.
    pub(crate) fn model_name(&self) -> String {
        cut(self.model.trim(), 40)
    }
}

/// What the host holds of the providers: the one in use (`active`, empty for none) and all of
/// them, each with the JSON it came as (so that what this window doesn't know of is kept when one
/// is changed).
#[derive(Clone, Debug, Default)]
pub(crate) struct Providers {
    pub(crate) active: String,
    pub(crate) all: Vec<(Provider, Value)>,
    /// The backup: "" (none) or "apple-on-device".
    pub(crate) backup: String,
    /// No primary was ever chosen (as against None, chosen by the owner).
    pub(crate) never_configured: bool,
    /// The level-2 gateway question waiting for the owner, if any.
    pub(crate) gateway: Option<settings_state::GatewayAsk>,
    /// What the host says of the on-device model.
    pub(crate) device: settings_state::Device,
    /// The ports the owner added to look for the on-device model on (beside 11434).
    pub(crate) device_ports: Vec<u16>,
}

impl Providers {
    /// What the host says now; nothing in a build without providers (the browser, the tests).
    /// Asked of the host once a frame at most (a window drawn without a context asks each time).
    pub(crate) fn of(app: &MailApp) -> Providers {
        let Some(host) = app.services.ai_providers.as_ref() else { return Providers::default() };
        let pass = app.ctx.as_ref().map(egui::Context::cumulative_pass_nr);
        if let (Some(pass), Ok(kept)) = (pass, app.ai.providers.try_borrow())
            && let Some((at, providers)) = kept.as_ref()
            && *at == pass
        {
            return providers.clone();
        }
        let mut read = Providers::read(&host.providers());
        read.device = settings_state::Device::read(&host.on_device());
        if let (Some(pass), Ok(mut kept)) = (pass, app.ai.providers.try_borrow_mut()) {
            *kept = Some((pass, read.clone()));
        }
        read
    }

    /// `{"active": id, "providers": [provider], "backup"?, "neverConfigured"?,
    /// "gatewayQuestion"?}`, read as anything from outside: what isn't a provider is left out,
    /// and at most 64 are read. A provider's window is read as `contextWindow` or
    /// `context_window`.
    pub(crate) fn read(v: &Value) -> Providers {
        let active = v.get("active").and_then(Value::as_str).unwrap_or_default().to_string();
        let all: Vec<(Provider, Value)> = v
            .get("providers")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .take(64)
                    .filter_map(|p| {
                        let mut provider = serde_json::from_value::<Provider>(p.clone()).ok()?;
                        if let Some(n) = p.get("contextWindow").and_then(Value::as_u64) {
                            provider.context_window = u32::try_from(n).unwrap_or(u32::MAX);
                        }
                        Some((provider, p.clone()))
                    })
                    .filter(|(p, _)| !p.id.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let backup = match v.get("backup").and_then(Value::as_str) {
            Some("apple-on-device") => "apple-on-device".to_string(),
            _ => String::new(),
        };
        // A host that doesn't say: never set up only when it has no provider at all.
        let never_configured = v.get("neverConfigured").and_then(Value::as_bool).unwrap_or(all.is_empty());
        let gateway = v.get("gatewayQuestion").and_then(settings_state::GatewayAsk::read);
        let device_ports = settings_state::ports_of(v.get("on_device_ports").or_else(|| v.get("devicePorts")));
        Providers { active, all, backup, never_configured, gateway, device: settings_state::Device::default(), device_ports }
    }

    /// Whether the backup is the on-device model.
    pub(crate) fn backup_on_device(&self) -> bool {
        self.backup == "apple-on-device"
    }

    /// The owner chose None for the primary (not the same as never having chosen).
    pub(crate) fn explicit_none(&self) -> bool {
        self.active.is_empty() && !self.never_configured
    }

    /// The on-device model the owner confirmed as the backup, while it passes its check.
    pub(crate) fn backup_confirmed(&self) -> Option<&settings_state::Candidate> {
        if !self.backup_on_device() || self.explicit_none() || self.device.state != settings_state::DeviceState::Using {
            return None;
        }
        self.device.confirmed_candidate()
    }

    /// The context window of what answers first: the primary's, else the confirmed backup's.
    pub(crate) fn answering_window(&self) -> Option<u32> {
        let window = match self.active() {
            Some(p) => Some(p.context_window),
            None => self.backup_confirmed().and_then(|c| c.context_window),
        };
        window.filter(|n| (settings_state::MIN_WINDOW..=settings_state::MAX_WINDOW).contains(n))
    }

    /// The provider requests go to, if any.
    pub(crate) fn active(&self) -> Option<&Provider> {
        if self.active.is_empty() {
            return None;
        }
        self.all.iter().map(|(p, _)| p).find(|p| p.id == self.active)
    }

    /// Provider `id`, with its JSON.
    pub(crate) fn get(&self, id: &str) -> Option<&(Provider, Value)> {
        self.all.iter().find(|(p, _)| p.id == id)
    }
}

/// `text` cut to `max` characters, with an ellipsis when something was left out.
pub(crate) fn cut(text: &str, max: usize) -> String {
    if text.chars().count() > max { text.chars().take(max.saturating_sub(1)).chain(['…']).collect() } else { text.to_string() }
}

/// The scheme and the host (and port) of `base`, nothing else of it: no user name, path, query or
/// fragment, nothing but printable ASCII, at most [`MAX_SHOWN_ADDRESS`] characters.
fn parts(base: &str) -> Option<(String, String)> {
    let (scheme, rest) = base.trim().split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let authority = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let authority: String = authority.chars().filter(char::is_ascii_graphic).collect();
    let scheme: String = scheme.chars().filter(char::is_ascii_alphabetic).take(8).collect::<String>().to_ascii_lowercase();
    (!authority.is_empty() && !scheme.is_empty()).then_some((scheme, authority))
}

/// `base` as it is shown to the owner: `https://api.example.org:443`.
pub(crate) fn shown_address(base: &str) -> String {
    match parts(base) {
        Some((scheme, authority)) => cut(&format!("{scheme}://{authority}"), MAX_SHOWN_ADDRESS),
        None => tl!("(no address)").to_string(),
    }
}

/// The host (and port) `base` names, as it is shown.
pub(crate) fn shown_host(base: &str) -> String {
    match parts(base) {
        Some((_, authority)) => cut(&authority, MAX_SHOWN_ADDRESS),
        None => tl!("(no address)").to_string(),
    }
}

/// Randomness for an answer's marker ([`mailcraft_model::ai::marker`]): the host's, from the
/// system's random numbers, when it gives it; else made here.
pub(crate) fn nonce(app: &MailApp) -> u64 {
    // (0: the system's random numbers failed; made here instead.)
    if let Some(n) = app.services.nonce.as_ref().map(|random| random()).filter(|n| *n != 0) {
        return n;
    }
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(crate::now_ms().to_bits());
    h.finish()
}

/// Whether a model can be asked here at all: a build that sends requests, with one set up.
pub(crate) fn model_ready(app: &MailApp) -> bool {
    let providers = Providers::of(app);
    app.services.can_ask_models() && (providers.active().is_some() || providers.backup_confirmed().is_some())
}

/// Calls off every request under way (of `kind`, or all), at once.
pub(crate) fn cancel_live(app: &mut MailApp, kind: Option<AiKind>) {
    let live: Vec<u64> =
        app.session.ai.all().iter().filter(|t| t.state.live() && kind.is_none_or(|k| t.request.kind == k)).map(|t| t.request.id).collect();
    for id in live {
        cancel(app, id);
    }
}

/// Calls request `id` off (`ai.cancel`), if it is still under way.
pub(crate) fn cancel(app: &mut MailApp, id: u64) {
    if app.session.ai.get(id).is_some_and(|t| t.state.live())
        && let Err(e) = app.execute("ai.cancel", json!({"requestId": id}))
    {
        log::debug!("model request {id} couldn't be called off: {e}");
    }
}

/// Where request `id` is; `None` once the session no longer keeps it.
pub(crate) fn state_of(app: &MailApp, id: u64) -> Option<RequestState> {
    app.session.ai.get(id).map(|t| t.state)
}

/// The window commands of the AI surfaces: `Some` for theirs.
pub(crate) fn command(app: &mut MailApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    if !p.is_object() && !p.is_null() {
        return Some(Err("the parameters are an object".into()));
    }
    Some(match id {
        "ui.findByDescription" => narrow::command(app, p),
        "ui.answers" => answers::command(app, p),
        "ui.aiSettings" => settings::command(app, p),
        "ui.aiConsent" => settings::consent_command(app, p),
        "ui.testProvider" => settings::test_command(app, p),
        "ui.confirmOnDevice" => settings::confirm_command(app, p),
        _ => return None,
    })
}

/// `p[key]` as true or false, if it is there.
pub(crate) fn flag(p: &Value, key: &str) -> Result<Option<bool>, String> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(format!("`{key}` is true or false")),
    }
}

/// `p[key]` as text, if it is there, at most `max` characters.
pub(crate) fn text(p: &Value, key: &str, max: usize) -> Result<Option<String>, String> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.chars().nth(max).is_none() => Ok(Some(s.clone())),
        Some(Value::String(_)) => Err(format!("`{key}` is at most {max} characters")),
        Some(_) => Err(format!("`{key}` is text")),
    }
}

/// Once a frame, before anything is drawn: the panel and the pane follow what changed under them.
/// While the owner is asked whether a model service on the internet may be used, or may be sent
/// an account's mail, no input from the control channel reaches the window: the yes is the
/// person's at the screen, never a script's or an agent's click (as for an agent's question).
pub(crate) fn follow(app: &mut MailApp) {
    if matches!(&app.dialog, Some(crate::dialogs::Dialog::Confirm { command, .. }) if command == "ui.aiConsent" || command == "ui.aiSettings") {
        app.synthetic.clear();
    }
    settings::follow_gateway(app);
    narrow::follow(app);
    answers::follow(app);
}

/// Escape for Find by Description, before the window's Escape ends the search.
pub(crate) fn escape(app: &mut MailApp, ctx: &egui::Context) {
    narrow::escape(app, ctx);
}

#[cfg(test)]
mod answer_text_tests;
#[cfg(test)]
mod errors_tests;
#[cfg(test)]
mod settings_tests;
#[cfg(test)]
mod surfaces_tests;
