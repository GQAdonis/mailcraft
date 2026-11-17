// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File › Options › AI assistance (#ai), what is drawn, worked out apart from the drawing: the two
//! status lines, the result of Test Connection, the on-device model card, the context window
//! typed, and what the host says of the backup, the on-device model and the gateway question.
//!
//! Everything here reads what the host hands over as hostile: a member of the wrong kind reads
//! as missing, text is stripped of control characters and cut, lists are capped. Nothing a
//! server or a model said is read for its meaning; a failure is told by its kind alone
//! ([`ErrorKind`]), and what the server said goes to a tooltip as data.

use mailcraft_model::ai::ErrorKind;
use serde_json::Value;

/// The least and the most tokens a context window may be (the engine's `ai.ask` range).
pub(crate) const MIN_WINDOW: u32 = 256;
pub(crate) const MAX_WINDOW: u32 = 1_048_576;
/// The window used when nothing else says: on this computer, and elsewhere.
pub(crate) const DEFAULT_LOCAL_WINDOW: u32 = 4_096;
pub(crate) const DEFAULT_REMOTE_WINDOW: u32 = 32_768;
/// Most digits typed in the context window field.
pub(crate) const MAX_WINDOW_DIGITS: usize = 7;
/// Most models a test lists, and the most characters of a model's id.
pub(crate) const MAX_MODELS: usize = 200;
pub(crate) const MAX_MODEL_ID: usize = 200;
/// Most characters of an address kept from the host (to hand back to it as it was).
const MAX_ADDRESS: usize = 2_000;
/// Most candidates of the on-device card, and publishers named in the gateway question.
const MAX_CANDIDATES: usize = 8;
const MAX_PUBLISHERS: usize = 8;
/// Most characters of a publisher's name, and of a refused model's id, as they are shown.
const SHOWN_PUBLISHER: usize = 40;
const SHOWN_REFUSED_ID: usize = 60;
/// Most characters of the fingerprint of a gateway question's list of models.
const MAX_FINGERPRINT: usize = 512;
/// Most bytes of what the server said that go to the result's tooltip.
const MAX_SAID: usize = 300;

/// `text` as it may be shown: control characters dropped, trimmed, at most `max` characters.
pub(crate) fn clean(text: &str, max: usize) -> String {
    let line: String = text.chars().filter(|c| !c.is_control()).collect();
    super::cut(line.trim(), max)
}

/// `n` with the interface language's thousands separator (4,096 in English).
pub(crate) fn grouped(n: u32) -> String {
    let digits = n.to_string();
    let sep = tlc!("thousands separator in numbers", ",");
    let mut out = String::with_capacity(digits.len().saturating_add(4));
    let len = digits.chars().count();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && len.saturating_sub(i).is_multiple_of(3) {
            out.push_str(sep);
        }
        out.push(c);
    }
    out
}

/// A context window as the host or a server gives it: a whole number of tokens, clamped to the
/// range; `None` when it isn't one or is zero.
pub(crate) fn window_of(v: Option<&Value>) -> Option<u32> {
    let n = v.and_then(Value::as_u64).filter(|n| *n > 0)?;
    Some(u32::try_from(n).unwrap_or(MAX_WINDOW).clamp(MIN_WINDOW, MAX_WINDOW))
}

/// Most ports the owner adds to look on.
pub(crate) const MAX_PORTS: usize = 8;

/// The ports the host says it looks on besides 11434: each from 1 to 65535, each once, at most
/// [`MAX_PORTS`]; anything else is left out.
pub(crate) fn ports_of(v: Option<&Value>) -> Vec<u16> {
    let mut out: Vec<u16> = Vec::new();
    for p in v.and_then(Value::as_array).into_iter().flatten().take(64) {
        if let Some(port) = p.as_u64().and_then(|n| u16::try_from(n).ok()).filter(|n| *n > 0)
            && !out.contains(&port)
        {
            out.push(port);
        }
        if out.len() >= MAX_PORTS {
            break;
        }
    }
    out
}

/// The ports as the card names them: "11434" and the owner's, in order.
pub(crate) fn ports_words(ports: &[u16]) -> String {
    std::iter::once(11_434u16).chain(ports.iter().copied().filter(|p| *p != 11_434)).map(|p| p.to_string()).collect::<Vec<_>>().join(", ")
}

/// What the owner typed in the context window field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WindowInput {
    /// Nothing typed: the default (or what the server says) is used.
    Empty,
    /// A number in range.
    Ok(u32),
    /// A number outside the range, set to the nearest end.
    Clamped(u32),
    /// Not a number: refused, not saved.
    Bad,
}

/// Reads the context window field: digits only (a thousands separator or a space is let pass).
pub(crate) fn parse_window(typed: &str) -> WindowInput {
    let typed = typed.trim();
    if typed.is_empty() {
        return WindowInput::Empty;
    }
    let digits: String = typed.chars().filter(|c| !matches!(c, ',' | '.' | ' ' | '\u{a0}' | '\u{202f}' | '\'')).collect();
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) || digits.chars().count() > MAX_WINDOW_DIGITS + 2 {
        return WindowInput::Bad;
    }
    let n = digits.parse::<u64>().unwrap_or(u64::MAX);
    if n < u64::from(MIN_WINDOW) {
        WindowInput::Clamped(MIN_WINDOW)
    } else if n > u64::from(MAX_WINDOW) {
        WindowInput::Clamped(MAX_WINDOW)
    } else {
        WindowInput::Ok(u32::try_from(n).unwrap_or(MAX_WINDOW))
    }
}

/// Where the context window shown came from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum WindowSource {
    #[default]
    Default,
    Server,
    Owner,
}

impl WindowSource {
    pub(crate) fn code(self) -> &'static str {
        match self {
            WindowSource::Default => "default",
            WindowSource::Server => "server",
            WindowSource::Owner => "owner",
        }
    }
}

/// A state's hue, made a colour when it is drawn (`Tokens`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hue {
    Blue,
    Orange,
    Red,
    Green,
    Dim,
    Accent,
}

impl Hue {
    pub(crate) fn color(self, t: &crate::theme::Tokens) -> egui::Color32 {
        match self {
            Hue::Blue => t.blue,
            Hue::Orange => t.orange,
            Hue::Red => t.red,
            Hue::Green => t.green,
            Hue::Dim => t.text_dim,
            Hue::Accent => t.accent,
        }
    }
}

// ---- The host's answers -------------------------------------------------------------------

/// The on-device model card's state, as the host says it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum DeviceState {
    Looking,
    Found,
    Using,
    Changed,
    NotFound,
    Off,
    #[default]
    Unavailable,
}

impl DeviceState {
    fn read(code: &str) -> DeviceState {
        match code {
            "looking" => DeviceState::Looking,
            "found" => DeviceState::Found,
            "using" => DeviceState::Using,
            "changed" => DeviceState::Changed,
            "notFound" => DeviceState::NotFound,
            "off" => DeviceState::Off,
            _ => DeviceState::Unavailable,
        }
    }
}

/// A server on this computer that passed the on-device model's identity check.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Candidate {
    /// As the host gave it, to be handed back to it in `confirm_on_device`.
    pub(crate) address: String,
    pub(crate) model: String,
    pub(crate) context_window: Option<u32>,
}

impl Candidate {
    /// Its address as it is shown.
    pub(crate) fn shown(&self) -> String {
        super::shown_address(&self.address)
    }
}

/// What the host says of the on-device model.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Device {
    pub(crate) state: DeviceState,
    pub(crate) candidates: Vec<Candidate>,
    pub(crate) confirmed: Option<String>,
}

impl Device {
    /// `{"state", "candidates": [{"address", "model", "contextWindow"?}], "confirmed"}`.
    pub(crate) fn read(v: &Value) -> Device {
        let state = DeviceState::read(v.get("state").and_then(Value::as_str).unwrap_or_default());
        let candidates = v
            .get("candidates")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .take(MAX_CANDIDATES)
                    .filter_map(|c| {
                        let address = c.get("address").and_then(Value::as_str)?;
                        let address: String = address.chars().filter(|c| !c.is_control()).take(MAX_ADDRESS).collect();
                        (!address.trim().is_empty()).then(|| Candidate {
                            address,
                            model: clean(c.get("model").and_then(Value::as_str).unwrap_or_default(), MAX_MODEL_ID),
                            context_window: window_of(c.get("contextWindow")),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let confirmed = v
            .get("confirmed")
            .and_then(Value::as_str)
            .map(|a| a.chars().filter(|c| !c.is_control()).take(MAX_ADDRESS).collect::<String>())
            .filter(|a| !a.trim().is_empty());
        Device { state, candidates, confirmed }
    }

    /// The candidate at the confirmed address, when the host lists it.
    pub(crate) fn confirmed_candidate(&self) -> Option<&Candidate> {
        let at = self.confirmed.as_deref()?;
        self.candidates.iter().find(|c| c.address == at)
    }
}

/// The level-2 question waiting for the owner: does this server send mail to the internet?
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct GatewayAsk {
    pub(crate) provider: String,
    /// As shown: scheme, host and port.
    pub(crate) address: String,
    /// The fingerprint of the server's list of models the question is about, as the host gave
    /// it, to be handed back with the answer (a changed list is a new question).
    pub(crate) models: String,
    /// The first distinct publishers the server named, each cut to 40 characters.
    pub(crate) publishers: Vec<String>,
}

impl GatewayAsk {
    /// `{"provider", "address", "models", "publishers": [name…]}`; `None` without a provider.
    pub(crate) fn read(v: &Value) -> Option<GatewayAsk> {
        let provider = clean(v.get("provider").and_then(Value::as_str)?, 512);
        if provider.is_empty() {
            return None;
        }
        let address = super::shown_address(v.get("address").and_then(Value::as_str).unwrap_or_default());
        let mut publishers: Vec<String> = Vec::new();
        for p in v.get("publishers").and_then(Value::as_array).into_iter().flatten().take(64) {
            let name = clean(p.as_str().unwrap_or_default(), SHOWN_PUBLISHER);
            if !name.is_empty() && !publishers.iter().any(|q| q.eq_ignore_ascii_case(&name)) {
                publishers.push(name);
            }
            if publishers.len() >= MAX_PUBLISHERS {
                break;
            }
        }
        let models: String =
            v.get("models").and_then(Value::as_str).unwrap_or_default().chars().filter(|c| !c.is_control()).take(MAX_FINGERPRINT).collect();
        Some(GatewayAsk { provider, address, models, publishers })
    }

    /// The question's text.
    pub(crate) fn text(&self) -> String {
        let a = |i: usize| self.publishers.get(i).map_or("", String::as_str);
        let lead = match self.publishers.len() {
            0 | 1 => crate::i18n::fmt(tl!("{address} lists models from several publishers."), &[("address", &self.address)]),
            2 => crate::i18n::fmt(
                tl!("{address} lists models from several publishers, such as {a} and {b}."),
                &[("address", &self.address), ("a", a(0)), ("b", a(1))],
            ),
            _ => crate::i18n::fmt(
                tl!("{address} lists models from several publishers, such as {a}, {b} and {c}."),
                &[("address", &self.address), ("a", a(0)), ("b", a(1)), ("c", a(2))],
            ),
        };
        let rest = tl!(
            "A server like that may pass what it receives to services on the internet.\n\nIf it does, MailCraft treats it as a gateway: each account's mail goes to it only after you allow that account.\n\nIf it doesn't, MailCraft uses it as a model on this computer.\n\nYou're asked once for this server and this list of models."
        );
        format!("{lead} {rest}")
    }
}

// ---- Test Connection -----------------------------------------------------------------------

/// Why a test failed, by the kind the host named.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TestFailure {
    Unreachable,
    /// Refused with 401 or 403: the key.
    KeyRefused,
    Refused(u16),
    TimedOut,
    Certificate,
    Malformed,
    CredentialStore,
    /// Level 1: a "local" server lists models of services on the internet (the first such id,
    /// when the host named it).
    HostedModels(Option<String>),
    /// Level 2: models from several publishers; the question is waiting.
    SeveralPublishers,
    /// Anything else (no kind, or one a test doesn't give).
    Other,
}

impl TestFailure {
    /// As `ui.aiSettings` answers it.
    pub(crate) fn code(&self) -> &'static str {
        match self {
            TestFailure::Unreachable => "unreachable",
            TestFailure::KeyRefused | TestFailure::Refused(_) => "refused",
            TestFailure::TimedOut => "timed_out",
            TestFailure::Certificate => "certificate",
            TestFailure::Malformed => "malformed",
            TestFailure::CredentialStore => "credential_store",
            TestFailure::HostedModels(_) => "settings",
            TestFailure::SeveralPublishers => "gatewayQuestion",
            TestFailure::Other => "other",
        }
    }
}

/// A model the server offered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Offered {
    pub(crate) id: String,
    pub(crate) context_window: Option<u32>,
}

/// Where Test Connection is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum TestState {
    #[default]
    Idle,
    Testing,
    Ok {
        models: Vec<Offered>,
        context_window: Option<u32>,
    },
    /// Failed, with what the server said (for the tooltip: data, never read).
    Failed {
        why: TestFailure,
        said: String,
    },
}

impl TestState {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            TestState::Idle => "idle",
            TestState::Testing => "testing",
            TestState::Ok { .. } => "ok",
            TestState::Failed { .. } => "failed",
        }
    }

    /// The models offered, after a successful test.
    pub(crate) fn models(&self) -> &[Offered] {
        match self {
            TestState::Ok { models, .. } => models,
            _ => &[],
        }
    }
}

/// What the host answered for a test: `Ok({"models": [...], "contextWindow"?})`, or
/// `Err("{\"kind\": …, \"message\": …, \"providerText\"?: …, \"refusedModel\"?: …}")`.
/// `waiting` says whether the host has a gateway question pending for the provider tested
/// (level 2).
pub(crate) fn test_result(answer: Result<Value, String>, waiting: bool) -> TestState {
    match answer {
        Ok(v) => {
            let mut models: Vec<Offered> = Vec::new();
            for m in v.get("models").and_then(Value::as_array).into_iter().flatten().take(MAX_MODELS * 2) {
                let (id, window) = match m {
                    Value::String(id) => (id.as_str(), None),
                    other => (other.get("id").and_then(Value::as_str).unwrap_or_default(), window_of(other.get("contextWindow"))),
                };
                let id = clean(id, MAX_MODEL_ID);
                if !id.is_empty() && !models.iter().any(|o| o.id == id) {
                    models.push(Offered { id, context_window: window });
                }
                if models.len() >= MAX_MODELS {
                    break;
                }
            }
            TestState::Ok { models, context_window: window_of(v.get("contextWindow")) }
        }
        Err(e) => {
            let v: Value = serde_json::from_str(&e).unwrap_or(Value::Null);
            let kind = v.get("kind").and_then(|k| serde_json::from_value::<ErrorKind>(k.clone()).ok());
            let why = match kind {
                Some(ErrorKind::Unreachable) => TestFailure::Unreachable,
                Some(ErrorKind::Refused { status: 401 | 403 }) => TestFailure::KeyRefused,
                Some(ErrorKind::Refused { status }) => TestFailure::Refused(status),
                Some(ErrorKind::TimedOut { .. }) => TestFailure::TimedOut,
                Some(ErrorKind::Certificate) => TestFailure::Certificate,
                Some(ErrorKind::Malformed | ErrorKind::TooLarge) => TestFailure::Malformed,
                Some(ErrorKind::CredentialStore) => TestFailure::CredentialStore,
                Some(ErrorKind::Settings) if waiting => TestFailure::SeveralPublishers,
                Some(ErrorKind::Settings) => TestFailure::HostedModels(
                    v.get("refusedModel").and_then(Value::as_str).map(|id| clean(id, SHOWN_REFUSED_ID)).filter(|id| !id.is_empty()),
                ),
                Some(ErrorKind::NotConfigured | ErrorKind::NoConsent | ErrorKind::Cancelled) | None => TestFailure::Other,
            };
            // Only the provider's own words (`providerText`) go under "The server said:"; our own
            // `message` never does.
            let said = v.get("providerText").and_then(Value::as_str).map(|m| clean(m, MAX_SAID)).unwrap_or_default();
            TestState::Failed { why, said }
        }
    }
}

/// The result line: its icon, its hue and its words (`host` and `address` as shown).
pub(crate) fn test_line(test: &TestState, host: &str, address: &str) -> Option<(&'static str, Hue, String)> {
    let h = |template: &str| crate::i18n::fmt(template, &[("host", host), ("address", address)]);
    Some(match test {
        TestState::Idle => return None,
        TestState::Testing => ("pending", Hue::Dim, tl!("Testing…").to_string()),
        TestState::Ok { models, .. } if models.is_empty() => ("warning", Hue::Orange, h(tl!("Connected, but {host} offers no models."))),
        TestState::Ok { models, .. } => ("tick", Hue::Green, tln!(models.len(), "Connected. {n} model offered.", "Connected. {n} models offered.")),
        TestState::Failed { why, .. } => match why {
            TestFailure::Unreachable => ("highImportance", Hue::Red, h(tl!("Nothing answered at {address}."))),
            TestFailure::KeyRefused => ("highImportance", Hue::Red, h(tl!("{host} refused the key."))),
            TestFailure::Refused(status) => (
                "highImportance",
                Hue::Red,
                crate::i18n::fmt(tl!("{host} answered with an error ({status})."), &[("host", host), ("status", &status.to_string())]),
            ),
            TestFailure::TimedOut => ("highImportance", Hue::Red, h(tl!("{host} didn't answer in time."))),
            TestFailure::Certificate => ("highImportance", Hue::Red, h(tl!("{host}'s certificate isn't trusted. Nothing was sent."))),
            TestFailure::Malformed => ("highImportance", Hue::Red, h(tl!("{host} answered, but not with a list of models."))),
            TestFailure::CredentialStore => {
                ("warning", Hue::Orange, tl!("The key couldn't be read from the system's credential store. Try again.").to_string())
            }
            TestFailure::HostedModels(_) => (
                "highImportance",
                Hue::Red,
                tl!("It lists models of services on the internet, so it can't be used as a model on this computer.").to_string(),
            ),
            TestFailure::SeveralPublishers => {
                ("warning", Hue::Orange, tl!("It lists models from several publishers. Answer the question to go on.").to_string())
            }
            TestFailure::Other => ("highImportance", Hue::Red, h(tl!("The test of {address} didn't work. Try again."))),
        },
    })
}

// ---- The status lines ----------------------------------------------------------------------

/// What answers first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Primary {
    /// Never set up.
    Never,
    /// The owner chose None.
    Off,
    Local {
        model: String,
    },
    Gateway {
        model: String,
    },
    Remote {
        model: String,
        host: String,
    },
}

/// What the status lines are worked out from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StatusInput {
    pub(crate) has_host: bool,
    pub(crate) primary: Primary,
    /// The backup is the on-device model, and the owner confirmed an address that passes.
    pub(crate) backup_confirmed: bool,
    /// Level 1: the server in use as "local" lists hosted-service models (its address, the id).
    pub(crate) refused: Option<(String, Option<String>)>,
    /// Level 2: the question is waiting for an answer about this address.
    pub(crate) asking: Option<String>,
    /// The confirmed on-device model no longer passes its identity check (its address).
    pub(crate) changed: Option<String>,
}

/// Which status line it is (for the tests and `ui.inspect`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StatusId {
    NoHost,
    Never,
    NeverBackup,
    Off,
    Local,
    Gateway,
    Remote,
    Refused,
    Asking,
    Changed,
    BackupConfirmed,
    BackupNone,
}

impl StatusId {
    /// The `ui.inspect` hit kind of the line, beside its place (`aiStatus` #0 and #1).
    pub(crate) fn hit(self) -> &'static str {
        match self {
            StatusId::NoHost => "aiStatusNoHost",
            StatusId::Never => "aiStatusNever",
            StatusId::NeverBackup => "aiStatusNeverBackup",
            StatusId::Off => "aiStatusOff",
            StatusId::Local => "aiStatusLocal",
            StatusId::Gateway => "aiStatusGateway",
            StatusId::Remote => "aiStatusRemote",
            StatusId::Refused => "aiStatusRefused",
            StatusId::Asking => "aiStatusAsking",
            StatusId::Changed => "aiStatusChanged",
            StatusId::BackupConfirmed => "aiStatusBackupConfirmed",
            StatusId::BackupNone => "aiStatusBackupNone",
        }
    }
}

/// A status line: which, its icon, its hue and its words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StatusLine {
    pub(crate) id: StatusId,
    pub(crate) icon: &'static str,
    pub(crate) hue: Hue,
    pub(crate) words: String,
}

fn line(id: StatusId, icon: &'static str, hue: Hue, words: String) -> StatusLine {
    StatusLine { id, icon, hue, words }
}

fn or_a_model(model: &str) -> String {
    if model.trim().is_empty() { tl!("a model").to_string() } else { model.to_string() }
}

/// The status lines (#0: what answers first, #1: what answers when it can't).
pub(crate) fn status_lines(s: &StatusInput) -> Vec<StatusLine> {
    if !s.has_host {
        return vec![line(StatusId::NoHost, "info", Hue::Blue, tl!("This build can't use a model. Nothing is sent anywhere.").to_string())];
    }
    let changed = s.changed.as_ref().map(|address| {
        line(
            StatusId::Changed,
            "warning",
            Hue::Orange,
            crate::i18n::fmt(
                tl!("The model at {address} no longer answers as Apple's on-device model, so MailCraft stopped using it. Nothing was sent to it."),
                &[("address", address)],
            ),
        )
    });
    let first = match &s.primary {
        Primary::Off => return vec![line(StatusId::Off, "info", Hue::Blue, tl!("Turned off. Nothing is sent anywhere.").to_string())],
        Primary::Never => {
            let first = if s.backup_confirmed {
                line(
                    StatusId::NeverBackup,
                    "info",
                    Hue::Blue,
                    tl!("Using Apple's on-device model on this computer. Your mail stays here.").to_string(),
                )
            } else {
                line(StatusId::Never, "info", Hue::Blue, tl!("No model is set up. Nothing is sent anywhere.").to_string())
            };
            return std::iter::once(first).chain(changed).collect();
        }
        _ if s.refused.is_some() => {
            let (address, id) = s.refused.clone().unwrap_or_default();
            let words = match id {
                Some(id) => crate::i18n::fmt(
                    tl!(
                        "Not used: {address} lists models of services on the internet, such as {id}, so it isn't a model on this computer. Nothing was sent. Choose \"A gateway on this computer\" if it is one."
                    ),
                    &[("address", &address), ("id", &id)],
                ),
                None => crate::i18n::fmt(
                    tl!(
                        "Not used: {address} lists models of services on the internet, so it isn't a model on this computer. Nothing was sent. Choose \"A gateway on this computer\" if it is one."
                    ),
                    &[("address", &address)],
                ),
            };
            line(StatusId::Refused, "highImportance", Hue::Red, words)
        }
        _ if s.asking.is_some() => line(
            StatusId::Asking,
            "warning",
            Hue::Orange,
            crate::i18n::fmt(
                tl!("Waiting for your answer: does {address} send your mail to the internet? Nothing is sent to it until you answer."),
                &[("address", s.asking.as_deref().unwrap_or_default())],
            ),
        ),
        Primary::Local { model } => line(
            StatusId::Local,
            "info",
            Hue::Blue,
            crate::i18n::fmt(tl!("Using {model} on this computer. Your mail stays here."), &[("model", &or_a_model(model))]),
        ),
        Primary::Gateway { model } => line(
            StatusId::Gateway,
            "info",
            Hue::Blue,
            crate::i18n::fmt(
                tl!(
                    "Using {model} through a gateway on this computer that sends your mail to the internet. Mail goes there only from the accounts you allow below."
                ),
                &[("model", &or_a_model(model))],
            ),
        ),
        Primary::Remote { model, host } => line(
            StatusId::Remote,
            "info",
            Hue::Blue,
            crate::i18n::fmt(
                tl!("Using {model} at {host}. Mail goes there only from the accounts you allow below."),
                &[("model", &or_a_model(model)), ("host", host)],
            ),
        ),
    };
    let second = changed.unwrap_or_else(|| {
        if s.backup_confirmed {
            line(StatusId::BackupConfirmed, "info", Hue::Blue, tl!("If it can't answer: Apple's on-device model on this computer.").to_string())
        } else {
            line(StatusId::BackupNone, "info", Hue::Blue, tl!("If it can't answer, you'll see why. Nothing else is tried.").to_string())
        }
    });
    vec![first, second]
}

// ---- The on-device model card --------------------------------------------------------------

/// Which state the card is drawn in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CardId {
    Looking,
    Found,
    Using,
    Changed,
    NotFound,
    Off,
    Unavailable,
}

/// The card's state: the backup's choice first (None: off), then the window's own looking, then
/// what the host says.
pub(crate) fn card_id(backup_on: bool, looking: bool, device: &Device) -> CardId {
    if device.state == DeviceState::Unavailable {
        return CardId::Unavailable;
    }
    if !backup_on {
        return CardId::Off;
    }
    if looking {
        return CardId::Looking;
    }
    match device.state {
        DeviceState::Looking => CardId::Looking,
        DeviceState::Found if device.candidates.is_empty() => CardId::NotFound,
        DeviceState::Found => CardId::Found,
        DeviceState::Using => CardId::Using,
        DeviceState::Changed => CardId::Changed,
        DeviceState::NotFound => CardId::NotFound,
        DeviceState::Off => CardId::Off,
        DeviceState::Unavailable => CardId::Unavailable,
    }
}

/// The card's icon and its hue, its title, its detail and its caption (none: not drawn). The
/// detail of [`CardId::Found`] is drawn a line per candidate, by the caller.
pub(crate) fn card_text(id: CardId, device: &Device, ports: &[u16]) -> (&'static str, Hue, String, String, Option<String>) {
    let on = ports_words(ports);
    let at = device.confirmed.as_deref().map(super::shown_address).unwrap_or_default();
    let using = device.confirmed_candidate().map(candidate_line).unwrap_or_else(|| apfel_at(&at, None));
    match id {
        CardId::Looking => (
            "pending",
            Hue::Dim,
            tl!("Looking for a model on this computer…").to_string(),
            if ports.is_empty() { tl!("On port 11434").to_string() } else { crate::i18n::fmt(tl!("On ports {ports}"), &[("ports", &on)]) },
            Some(tl!("Looking asks for the list of models and nothing else.").to_string()),
        ),
        CardId::Found => (
            "device",
            Hue::Accent,
            tl!("Apple's on-device model found").to_string(),
            String::new(),
            Some(
                tl!("MailCraft has sent it nothing. Choose Use It to let it answer when the primary can't. Only the address you choose is used.")
                    .to_string(),
            ),
        ),
        CardId::Using => (
            "tick",
            Hue::Green,
            tl!("Using Apple's on-device model as the backup").to_string(),
            using,
            Some(tl!("MailCraft checks that it is still the same model each time it starts and after any error.").to_string()),
        ),
        CardId::Changed => (
            "sealChanged",
            Hue::Orange,
            crate::i18n::fmt(tl!("The model at {address} has changed"), &[("address", &at)]),
            tl!("It no longer answers as Apple's on-device model, so MailCraft stopped using it. Nothing was sent to it.").to_string(),
            Some(tl!("Use it again only if you know why it changed.").to_string()),
        ),
        CardId::NotFound => (
            "device",
            Hue::Dim,
            tl!("No on-device model found").to_string(),
            if ports.is_empty() {
                tl!("MailCraft looked on port 11434 and found no apfel server.").to_string()
            } else {
                crate::i18n::fmt(tl!("MailCraft looked on ports {ports} and found no apfel server."), &[("ports", &on)])
            },
            Some(tl!("apfel serves Apple's on-device model on macOS. Start it, or add the port it uses below, then choose Look Again.").to_string()),
        ),
        CardId::Off => (
            "device",
            Hue::Dim,
            tl!("The backup is turned off").to_string(),
            tl!("MailCraft doesn't look for an on-device model and uses none.").to_string(),
            None,
        ),
        CardId::Unavailable => (
            "device",
            Hue::Dim,
            tl!("No on-device model on this computer").to_string(),
            tl!("MailCraft can use Apple's on-device model only on a Mac, through apfel.").to_string(),
            None,
        ),
    }
}

fn apfel_at(address: &str, window: Option<u32>) -> String {
    match window {
        Some(n) => crate::i18n::fmt(tl!("apfel at {address} · {n} tokens"), &[("address", address), ("n", &grouped(n))]),
        None => crate::i18n::fmt(tl!("apfel at {address}"), &[("address", address)]),
    }
}

/// A candidate's line: "apfel at {address} · {model} · {n} tokens" (the model and the window
/// only when the host named them). The model's id is the server's, shown as data.
pub(crate) fn candidate_line(c: &Candidate) -> String {
    let address = c.shown();
    let model = c.model.trim();
    if model.is_empty() {
        return apfel_at(&address, c.context_window);
    }
    match c.context_window {
        Some(n) => crate::i18n::fmt(tl!("apfel at {address} · {model} · {n} tokens"), &[("address", &address), ("model", model), ("n", &grouped(n))]),
        None => crate::i18n::fmt(tl!("apfel at {address} · {model}"), &[("address", &address), ("model", model)]),
    }
}
