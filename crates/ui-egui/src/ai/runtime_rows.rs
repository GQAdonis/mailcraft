// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The runtime chain as File › Options › AI assistance shows it (B11, 21.2): what each row says,
//! in words of its own, from what the host keeps and found ([`crate::control::AiRuntimes`]). Pure:
//! nothing here draws, asks a host or runs anything, so that the chip, the badge, the summary line
//! and the rows read the same facts through the same functions and can never disagree.
//!
//! Nothing a program printed is shown: a status is one of the words of [`Status`], a reason one of
//! [`Reason`], and a path, a version or an address is shown only as the host checked it.

use std::collections::BTreeMap;

use mailcraft_model::ai::{RuntimeId, RuntimeKind, Where};
use serde_json::Value;

use crate::control::{RuntimeSettings, RuntimeState, RuntimeStatus};

/// The commands shown to copy: constants, never translated and never run by MailCraft.
pub(crate) const CLAUDE_LOGIN: &str = "claude auth login";
pub(crate) const CLAUDE_LOGOUT: &str = "claude auth logout";
pub(crate) const CODEX_LOGIN: &str = "codex login";
pub(crate) const CODEX_LOGOUT: &str = "codex logout";

/// Longest identity, plan or folder text taken from the host.
const MOST_SAID: usize = 160;

/// Which runtimes this computer offers at all: Apple's on-device model only on a Mac, Codex
/// never on Windows (it runs only inside a system sandbox, and Windows gives none).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Platform {
    pub(crate) mac: bool,
    pub(crate) windows: bool,
}

impl Platform {
    pub(crate) fn here() -> Platform {
        Platform { mac: cfg!(target_os = "macos"), windows: cfg!(target_os = "windows") }
    }

    /// Whether `kind` has a row here.
    pub(crate) fn offers(self, kind: RuntimeKind) -> bool {
        match kind {
            RuntimeKind::OnDevice => self.mac,
            RuntimeKind::Codex => !self.windows,
            _ => true,
        }
    }
}

/// Why a runtime's program can't be used, as the host's checks say it: each with its remedy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Reason {
    NotAbsolute,
    NoProgram,
    NotRegular,
    OtherOwner,
    OtherOwnerFolder,
    OthersWrite,
    /// Other users can change the folder it is in (named, when the host said which).
    OthersWriteFolder(Option<String>),
    Unlookable,
    ClaudeUnreadable,
    UarRefused,
    VersionFailed,
    /// The host said it can't be used and not why.
    Unsaid,
}

impl Reason {
    /// `code` as the host writes it; anything else is [`Reason::Unsaid`].
    pub(crate) fn parse(code: &str, folder: Option<String>) -> Reason {
        match code {
            "notAbsolute" => Reason::NotAbsolute,
            "noProgram" => Reason::NoProgram,
            "notRegular" => Reason::NotRegular,
            "otherOwner" => Reason::OtherOwner,
            "otherOwnerFolder" => Reason::OtherOwnerFolder,
            "othersWrite" => Reason::OthersWrite,
            "othersWriteFolder" => Reason::OthersWriteFolder(folder),
            "unlookable" => Reason::Unlookable,
            "claudeUnreadable" => Reason::ClaudeUnreadable,
            "uarRefused" => Reason::UarRefused,
            "versionFailed" => Reason::VersionFailed,
            _ => Reason::Unsaid,
        }
    }

    /// The reason, in a few words.
    pub(crate) fn words(&self) -> String {
        match self {
            Reason::NotAbsolute => tl!("the path isn't absolute"),
            Reason::NoProgram => tl!("there is no program there"),
            Reason::NotRegular => tl!("it isn't a regular file"),
            Reason::OtherOwner => tl!("it belongs to another user"),
            Reason::OtherOwnerFolder => tl!("its folder belongs to another user"),
            Reason::OthersWrite => tl!("other users can change it"),
            Reason::OthersWriteFolder(_) => tl!("other users can change its folder"),
            Reason::Unlookable => tl!("it couldn't be looked at"),
            Reason::ClaudeUnreadable => tl!("Claude Code didn't say whether you're signed in"),
            Reason::UarRefused => tl!("UAR refused MailCraft"),
            Reason::VersionFailed => tl!("it didn't answer its version check"),
            Reason::Unsaid => tl!("it didn't behave as it should"),
        }
        .to_string()
    }

    /// What the owner can do about it.
    pub(crate) fn remedy(&self) -> String {
        match self {
            Reason::NotAbsolute => tl!("Give the program's full path below.").to_string(),
            Reason::NoProgram => tl!("Check the path below, or clear it to look in the usual places.").to_string(),
            Reason::NotRegular => tl!("Give the path of the program itself below.").to_string(),
            Reason::OtherOwner | Reason::OtherOwnerFolder => {
                tl!("MailCraft runs only programs that belong to you or to the system. Install your own copy, then choose Look Again.").to_string()
            }
            Reason::OthersWrite => {
                tl!("Another user of this computer could replace it. Make it writable only by you, or install a copy that is.").to_string()
            }
            Reason::OthersWriteFolder(folder) => crate::i18n::fmt(
                tl!(
                    "Another user of this computer could replace what's in {folder}. Make the folder writable only by you, or install a copy in a folder that is, then choose Look Again."
                ),
                &[("folder", folder.as_deref().unwrap_or(tl!("its folder")))],
            ),
            Reason::Unlookable => tl!("MailCraft couldn't read it. Check its permissions, then choose Look Again.").to_string(),
            Reason::ClaudeUnreadable => {
                tl!("Claude Code didn't say whether you're signed in. Update Claude Code, then choose Look Again.").to_string()
            }
            Reason::UarRefused => tl!("UAR refused MailCraft. Check the runtime's own settings, then choose Look Again.").to_string(),
            Reason::VersionFailed | Reason::Unsaid => tl!("It didn't answer its version check. Reinstall it, then choose Look Again.").to_string(),
        }
    }
}

/// A runtime's state in the page's words: the only status words there are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    Ready,
    SignInNotChecked,
    Off,
    SignedOut,
    SignInExpired,
    SignInEnded,
    SigningIn,
    NeedsKey,
    NotInstalled,
    NotRunning,
    Running,
    NotTurnedOn,
    Looking,
    NotCheckedYet,
    CantRunHere,
    Broken(Reason),
}

/// How a tag looks: a notice wash in a hue, or the quiet pill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tone {
    Green,
    Orange,
    Red,
    Quiet,
}

impl Status {
    /// The host's word for it, for the stand-in and a widened host.
    pub(crate) fn parse(code: &str, reason: Reason) -> Option<Status> {
        Some(match code {
            "ready" => Status::Ready,
            "signInNotChecked" | "unchecked" => Status::SignInNotChecked,
            "off" => Status::Off,
            "signedOut" => Status::SignedOut,
            "signInExpired" => Status::SignInExpired,
            "signInEnded" => Status::SignInEnded,
            "signingIn" | "waiting" => Status::SigningIn,
            "needsKey" => Status::NeedsKey,
            "notInstalled" | "notFound" => Status::NotInstalled,
            "notRunning" => Status::NotRunning,
            "running" => Status::Running,
            "notTurnedOn" => Status::NotTurnedOn,
            "looking" | "checking" => Status::Looking,
            "notCheckedYet" | "unknown" => Status::NotCheckedYet,
            "cantRunHere" | "noSandbox" => Status::CantRunHere,
            "broken" | "unavailable" => Status::Broken(reason),
            _ => return None,
        })
    }

    /// The tag's words.
    pub(crate) fn words(&self) -> &'static str {
        match self {
            Status::Ready => tl!("Ready"),
            Status::SignInNotChecked => tl!("Sign-in not checked"),
            Status::Off => tl!("Off"),
            Status::SignedOut => tl!("Signed out"),
            Status::SignInExpired => tl!("Sign-in expired"),
            Status::SignInEnded => tl!("Sign-in ended"),
            Status::SigningIn => tl!("Signing in…"),
            Status::NeedsKey => tl!("Needs a key"),
            Status::NotInstalled => tl!("Not installed"),
            Status::NotRunning => tl!("Not running"),
            Status::Running => tl!("Running"),
            Status::NotTurnedOn => tl!("Not turned on"),
            Status::Looking => tl!("Looking…"),
            Status::NotCheckedYet => tl!("Not checked yet"),
            Status::CantRunHere => tl!("Can't run here"),
            Status::Broken(_) => tl!("Broken"),
        }
    }

    /// The tag's look and its icon (none for a few quiet ones, whose words say it).
    pub(crate) fn look(&self) -> (Tone, Option<&'static str>) {
        match self {
            Status::Ready => (Tone::Green, Some("check")),
            Status::SignInNotChecked | Status::Running | Status::NotTurnedOn => (Tone::Quiet, Some("info")),
            Status::Off | Status::NotInstalled | Status::NotRunning | Status::NotCheckedYet => (Tone::Quiet, None),
            Status::SignedOut | Status::SignInExpired | Status::SignInEnded | Status::NeedsKey => (Tone::Orange, Some("warning")),
            Status::SigningIn => (Tone::Quiet, Some("pending")),
            Status::Looking => (Tone::Quiet, Some("syncProgress")),
            Status::CantRunHere | Status::Broken(_) => (Tone::Red, Some("highImportance")),
        }
    }

    /// Whether a question may go to a runtime in this state.
    pub(crate) fn usable(&self) -> bool {
        matches!(self, Status::Ready | Status::SignInNotChecked)
    }

    /// Whether its switch may be turned at all.
    pub(crate) fn switchable(&self) -> bool {
        !matches!(self, Status::NotInstalled | Status::CantRunHere | Status::Broken(_))
    }
}

/// What the host can say of a runtime beyond [`RuntimeStatus`]: the page's words, whether Claude
/// Code is on a plan, the ChatGPT identity, why it is broken, whether its last run was stopped
/// for a tool. Read as anything from outside; every member may be missing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Extra {
    pub(crate) status: Option<Status>,
    pub(crate) uses_plan: Option<bool>,
    pub(crate) email: Option<String>,
    pub(crate) plan: Option<String>,
    pub(crate) tool_stopped: bool,
    pub(crate) opted_in: bool,
}

impl Extra {
    /// `{"status", "reason", "folder", "usesPlan", "email", "plan", "toolStopped", "optedIn"}`.
    pub(crate) fn read(v: &Value) -> Extra {
        let said = |k: &str| v.get(k).and_then(Value::as_str).map(|s| super::settings_state::clean(s, MOST_SAID)).filter(|s| !s.is_empty());
        let reason = Reason::parse(v.get("reason").and_then(Value::as_str).unwrap_or_default(), said("folder"));
        Extra {
            status: v.get("status").and_then(Value::as_str).and_then(|c| Status::parse(c, reason)),
            uses_plan: v.get("usesPlan").and_then(Value::as_bool),
            email: said("email"),
            plan: said("plan"),
            tool_stopped: v.get("toolStopped").and_then(Value::as_bool).unwrap_or(false),
            opted_in: v.get("optedIn").and_then(Value::as_bool).unwrap_or(false),
        }
    }
}

/// A `compat` provider as the providers' settings have it: its kind and its address.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CompatFacts {
    /// `local`, `gateway` or `remote`.
    pub(crate) kind: String,
    pub(crate) base_url: String,
}

/// What is known of one runtime of the chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RowFacts {
    pub(crate) id: RuntimeId,
    pub(crate) enabled: bool,
    pub(crate) status: RuntimeStatus,
    pub(crate) place: Where,
    pub(crate) extra: Extra,
    pub(crate) compat: Option<CompatFacts>,
    pub(crate) model: Option<String>,
}

/// Everything the page reads, gathered once a frame.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Facts {
    pub(crate) settings: RuntimeSettings,
    pub(crate) rows: Vec<RowFacts>,
    pub(crate) platform: Option<Platform>,
    /// The runtime a sign-in of the window's is waiting for.
    pub(crate) signing_in: Option<RuntimeId>,
}

/// Where a runtime sends text, as its row says it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WhereTag {
    pub(crate) words: String,
    pub(crate) local: bool,
}

/// A row's one action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    SignIn,
    CancelSignIn,
    LookAgain,
    AddKey,
    HowToInstall,
    UseUar,
    UseDevice,
}

impl Action {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Action::SignIn => tl!("Sign In…"),
            Action::CancelSignIn => tl!("Cancel Sign-in"),
            Action::LookAgain => tl!("Look Again"),
            Action::AddKey => tl!("Add Key…"),
            Action::HowToInstall => tl!("How to Install"),
            Action::UseUar => tl!("Use It…"),
            Action::UseDevice => tl!("Use It"),
        }
    }
}

/// One row as it is shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) id: RuntimeId,
    pub(crate) name: String,
    pub(crate) icon: &'static str,
    pub(crate) line2: String,
    pub(crate) status: Status,
    pub(crate) place: WhereTag,
    pub(crate) action: Option<Action>,
    pub(crate) enabled: bool,
    pub(crate) leaves: bool,
}

impl Facts {
    fn platform(&self) -> Platform {
        self.platform.unwrap_or_else(Platform::here)
    }

    /// The facts of the rows this computer shows, in chain order.
    pub(crate) fn shown(&self) -> impl Iterator<Item = &RowFacts> {
        let platform = self.platform();
        self.rows.iter().filter(move |r| platform.offers(r.id.kind()))
    }

    /// The rows, in chain order.
    pub(crate) fn rows(&self) -> Vec<Row> {
        self.shown().map(|f| self.row(f)).collect()
    }

    fn row(&self, f: &RowFacts) -> Row {
        let status = self.status_of(f);
        Row {
            id: f.id.clone(),
            name: name(&f.id),
            icon: icon(f.id.kind()),
            line2: line2(f, &status),
            place: where_tag(f),
            action: action(f.id.kind(), &status),
            enabled: f.enabled,
            leaves: f.place == Where::Leaves,
            status,
        }
    }

    /// Its status: the host's own word when it gave one, else read from [`RuntimeState`].
    pub(crate) fn status_of(&self, f: &RowFacts) -> Status {
        if self.signing_in.as_ref() == Some(&f.id) {
            return Status::SigningIn;
        }
        Self::found(f)
    }

    /// Its status as the host says it, whatever the window is waiting for.
    pub(crate) fn found(f: &RowFacts) -> Status {
        if let Some(s) = &f.extra.status {
            return s.clone();
        }
        let kind = f.id.kind();
        match f.status.state {
            RuntimeState::Ready if kind == RuntimeKind::Codex && !f.enabled => Status::Off,
            RuntimeState::Ready if kind == RuntimeKind::Codex => Status::SignInNotChecked,
            RuntimeState::Ready => Status::Ready,
            RuntimeState::SignedOut if kind == RuntimeKind::Compat => Status::NeedsKey,
            RuntimeState::SignedOut => Status::SignedOut,
            RuntimeState::NotFound => Status::NotInstalled,
            RuntimeState::Checking => Status::Looking,
            RuntimeState::Unavailable => Status::Broken(Reason::Unsaid),
            RuntimeState::Unknown => Status::NotCheckedYet,
        }
    }

    /// The runtime that would answer first: the first enabled, usable one, in chain order.
    pub(crate) fn first_usable(&self) -> Option<&RowFacts> {
        self.shown().find(|f| f.enabled && self.status_of(f).usable())
    }

    /// How many are on and usable.
    pub(crate) fn ready_count(&self) -> usize {
        self.shown().filter(|f| f.enabled && self.status_of(f).usable()).count()
    }

    /// The order of every runtime of the chain (hidden ones too) after the shown row at `index`
    /// moves one place up or down past the next shown row. `None` when it can't move.
    pub(crate) fn moved(&self, index: usize, down: bool) -> Option<Vec<RuntimeId>> {
        let shown: Vec<usize> = {
            let platform = self.platform();
            self.rows.iter().enumerate().filter(|(_, r)| platform.offers(r.id.kind())).map(|(i, _)| i).collect()
        };
        let from = *shown.get(index)?;
        let to = *shown.get(if down { index.checked_add(1)? } else { index.checked_sub(1)? })?;
        let mut order: Vec<RuntimeId> = self.rows.iter().map(|r| r.id.clone()).collect();
        order.swap(from, to);
        Some(order)
    }
}

/// A runtime's row name.
pub(crate) fn name(id: &RuntimeId) -> String {
    match id {
        RuntimeId::OnDevice => tl!("Apple's on-device model").to_string(),
        RuntimeId::Compat(p) => super::cut(p.as_str(), 64),
        RuntimeId::ChatGpt => tl!("ChatGPT plan").to_string(),
        RuntimeId::ClaudeCode => tl!("Claude Code").to_string(),
        RuntimeId::Codex => tl!("Codex").to_string(),
        RuntimeId::Uar => tl!("Universal Agent Runtime").to_string(),
    }
}

/// A runtime's short name, as the narrowing badge says it.
pub(crate) fn short_name(id: &RuntimeId) -> String {
    match id {
        RuntimeId::OnDevice => tl!("On this computer").to_string(),
        RuntimeId::Uar => tl!("UAR").to_string(),
        other => name(other),
    }
}

/// Who runs the service a runtime sends to, by name: for the badge's tip and the consent dialog.
pub(crate) fn vendor(f: &RowFacts) -> String {
    match f.id.kind() {
        RuntimeKind::ChatGpt | RuntimeKind::Codex => tl!("OpenAI").to_string(),
        RuntimeKind::ClaudeCode => tl!("Anthropic").to_string(),
        RuntimeKind::Compat => compat_host(f).unwrap_or_else(|| tl!("a service elsewhere").to_string()),
        RuntimeKind::OnDevice | RuntimeKind::Uar => tl!("a service elsewhere").to_string(),
    }
}

/// The icon of a runtime's row (`icons/ai.rs`): no vendor's mark, ever.
pub(crate) fn icon(kind: RuntimeKind) -> &'static str {
    match kind {
        RuntimeKind::OnDevice => "aiDevice",
        RuntimeKind::Compat => "aiProvider",
        RuntimeKind::ChatGpt => "aiPlan",
        RuntimeKind::ClaudeCode | RuntimeKind::Codex => "aiTerminal",
        RuntimeKind::Uar => "aiHub",
    }
}

fn compat_host(f: &RowFacts) -> Option<String> {
    f.compat.as_ref().and_then(|c| super::narrow::badge_host(&c.base_url))
}

fn compat_kind(f: &RowFacts) -> &str {
    f.compat.as_ref().map_or("remote", |c| c.kind.as_str())
}

/// Line 2 of a row: its description, or what its state says instead.
fn line2(f: &RowFacts, status: &Status) -> String {
    let kind = f.id.kind();
    let fmt = crate::i18n::fmt;
    match (status, kind) {
        (Status::Ready, RuntimeKind::ClaudeCode) if f.extra.uses_plan == Some(true) => tl!("Uses your Claude plan").to_string(),
        (Status::Ready, RuntimeKind::ClaudeCode) => tl!("Uses Claude Code's own sign-in").to_string(),
        (Status::Ready, RuntimeKind::ChatGpt) => match (&f.extra.email, &f.extra.plan) {
            (Some(email), Some(plan)) => fmt(tl!("Signed in as {email} ({plan})"), &[("email", email), ("plan", plan)]),
            (Some(email), None) => fmt(tl!("Signed in as {email}"), &[("email", email)]),
            _ => tl!("Signed in to ChatGPT").to_string(),
        },
        (Status::Ready, RuntimeKind::Uar) if f.extra.opted_in => tl!("Running on this computer, not started by MailCraft").to_string(),
        (Status::SignInNotChecked, _) => tl!("Uses your own Codex sign-in, which MailCraft doesn't check").to_string(),
        (Status::SignedOut, RuntimeKind::ClaudeCode) => tl!("Not signed in. Sign in with claude auth login in a terminal.").to_string(),
        (Status::SignedOut, RuntimeKind::Codex) => tl!("Not signed in. Run codex login in a terminal yourself: MailCraft never runs it.").to_string(),
        (Status::SignInExpired, _) => tl!("Your Claude Code sign-in has run out. Sign in again with claude auth login.").to_string(),
        (Status::SignInEnded, _) => tl!("OpenAI ended this sign-in. Sign in with ChatGPT again to keep using your ChatGPT plan.").to_string(),
        (Status::SigningIn, _) => tl!("Waiting for you to finish signing in").to_string(),
        (Status::NeedsKey, _) => tl!("Add the key this service asks for").to_string(),
        (Status::NotInstalled, _) => tl!("Not found on this computer").to_string(),
        (Status::NotRunning, _) => tl!("Installed, not running. MailCraft can't start this version itself.").to_string(),
        (Status::Running, _) => tl!("Running on this computer, not started by MailCraft").to_string(),
        (Status::NotTurnedOn, _) => tl!("Found on this Mac. Using it sends nothing anywhere.").to_string(),
        (Status::CantRunHere, _) => {
            fmt(tl!("This computer gives no system sandbox, and {runtime} runs only inside one."), &[("runtime", &name(&f.id))])
        }
        (Status::Broken(reason), _) => fmt(tl!("Can't use it: {reason}"), &[("reason", &reason.words())]),
        _ => description(f),
    }
}

/// What a runtime is, when no state says more.
pub(crate) fn description(f: &RowFacts) -> String {
    let fmt = crate::i18n::fmt;
    match f.id.kind() {
        RuntimeKind::OnDevice => tl!("Apple Intelligence on this Mac, through apfel").to_string(),
        RuntimeKind::Compat => {
            let address = f.compat.as_ref().map(|c| super::shown_address(&c.base_url)).unwrap_or_default();
            let host = compat_host(f).unwrap_or_else(|| address.clone());
            match compat_kind(f) {
                _ if address.is_empty() && f.place == Where::Local => tl!("A model service on this computer").to_string(),
                _ if address.is_empty() => tl!("A model service elsewhere").to_string(),
                "local" => fmt(tl!("Runs at {address}"), &[("address", &address)]),
                "gateway" => fmt(tl!("A gateway at {host}"), &[("host", &host)]),
                _ => fmt(tl!("A model service at {host}"), &[("host", &host)]),
            }
        }
        RuntimeKind::ChatGpt => tl!("Sign in with ChatGPT. Answers use your ChatGPT plan and its limits.").to_string(),
        RuntimeKind::ClaudeCode => tl!("Uses your own Claude Code sign-in").to_string(),
        RuntimeKind::Codex => tl!("Uses your own Codex sign-in, which MailCraft doesn't check").to_string(),
        RuntimeKind::Uar => tl!("Runs agents on this computer and passes them to models").to_string(),
    }
}

/// Where a runtime sends text, as its row says it. "On this computer" only when the host says
/// so; everything else is said to leave.
pub(crate) fn where_tag(f: &RowFacts) -> WhereTag {
    if f.place == Where::Local {
        return WhereTag { words: tl!("On this computer").to_string(), local: true };
    }
    let fmt = crate::i18n::fmt;
    let words = match f.id.kind() {
        RuntimeKind::ChatGpt | RuntimeKind::Codex => tl!("Sends to OpenAI").to_string(),
        RuntimeKind::ClaudeCode => tl!("Sends to Anthropic").to_string(),
        RuntimeKind::Compat => match (compat_kind(f), compat_host(f)) {
            ("gateway", Some(host)) => fmt(tl!("Through the gateway at {host}"), &[("host", &host)]),
            (_, Some(host)) => fmt(tl!("Sends to {host}"), &[("host", &host)]),
            (_, None) => tl!("Leaves this computer").to_string(),
        },
        RuntimeKind::OnDevice | RuntimeKind::Uar => tl!("Leaves this computer").to_string(),
    };
    WhereTag { words, local: false }
}

/// A row's one action, by its kind and state.
fn action(kind: RuntimeKind, status: &Status) -> Option<Action> {
    match (status, kind) {
        (Status::SignedOut, RuntimeKind::Codex) => Some(Action::LookAgain),
        (Status::SignedOut | Status::SignInExpired | Status::SignInEnded, _) => Some(Action::SignIn),
        (Status::SigningIn, _) => Some(Action::CancelSignIn),
        (Status::NeedsKey, _) => Some(Action::AddKey),
        (Status::NotInstalled, _) => Some(Action::HowToInstall),
        (Status::NotRunning | Status::NotCheckedYet, _) => Some(Action::LookAgain),
        (Status::Running, _) => Some(Action::UseUar),
        (Status::NotTurnedOn, _) => Some(Action::UseDevice),
        _ => None,
    }
}

/// The accounts (by address) that allow an enabled runtime that leaves this computer, each with
/// the enabled runtimes it allows, in chain order. Only the facts' own consents count.
pub(crate) fn leaving_pairs(facts: &Facts) -> Vec<(String, Vec<RuntimeId>)> {
    let leaving: Vec<&RuntimeId> = facts.shown().filter(|f| f.enabled && f.place == Where::Leaves).map(|f| &f.id).collect();
    let mut out: BTreeMap<String, Vec<RuntimeId>> = BTreeMap::new();
    for (account, allowed) in facts.settings.consents.iter().take(1_024) {
        let account = super::settings_state::clean(account, 120);
        if account.is_empty() {
            continue;
        }
        for id in leaving.iter().filter(|id| allowed.contains(**id)) {
            let list = out.entry(account.clone()).or_default();
            if !list.contains(*id) {
                list.push((*id).clone());
            }
        }
    }
    out.into_iter().collect()
}
