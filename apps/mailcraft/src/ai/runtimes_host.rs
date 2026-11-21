// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The desktop's host for the owner's runtime methods (`ai.runtimes`, `ai.setRuntimeOrder`, …;
//! `mailcraft_ui_egui::control::AiRuntimes`, ai-runtime-commands 12.8): the chain, the consents
//! and the fallback as `ai.json` version 3 keeps them ([`super::settings`]).
//!
//! - **Settings** are read from the shared settings and kept whole: the chain, the consents and
//!   the fallback in one write (beside the file, then in its place), or nothing at all. A file a
//!   newer MailCraft wrote is never written over.
//! - **Where** a runtime sends text is `mailcraft_model::ai::where_of` over the providers the
//!   owner set up, never anything a runtime says of itself. There is no managed UAR configuration
//!   yet, so UAR always leaves.
//! - **Status** never waits: it is answered from what is already known (the programs detection
//!   found, Claude Code's last `auth status`, the ChatGPT runtime's own state, the last look for
//!   an external UAR) and a look that is due is started on a thread of its own, its end shown at
//!   the next `ai.runtimes`.
//! - **Rescan** looks for the programs on a thread of its own (`detect::look`, its cache kept
//!   through the shared settings), asks Claude Code's status again and looks for an external UAR.
//! - **Sign-in**: the ChatGPT plan's own flow on a thread (OpenAI's page in the owner's browser,
//!   waited for at most ten minutes); Claude Code's is the owner's terminal opened with the fixed
//!   argv `claude auth login`, then its status asked until it is usable: MailCraft runs no login
//!   itself and reads nothing of Claude Code's. Codex is never signed in by MailCraft (the
//!   window answers "run `codex login` yourself" before the host is asked).
//! - **Sign-out** forgets only what MailCraft itself keeps: a `compat` provider's key (under both
//!   its names) or the ChatGPT plan's sign-in (`ai.chatgpt`, the runtime signed out in memory, off
//!   in the chain and no account's consent left). The credential store may wait for the owner, so
//!   it is asked on a thread of its own, never the window's. Claude Code and Codex have no
//!   sign-out here: their logins are the owner's, in their own CLIs.
//!
//! Every method here is the owner's own: the control channel refuses them to agents.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use mailcraft_ai::ProviderKind;
use mailcraft_ai::runtime::claude_code::AuthStatus;
use mailcraft_ai::runtime::siwc::SiwcStatus;
use mailcraft_host::SecretStore;
use mailcraft_model::ai::{ChainEntry, CompatKind, CompatProvider, ProviderId, RuntimeId, RuntimeKind, RuntimePath, Where, where_of};
use mailcraft_ui_egui::control::{AiRuntimes, RuntimeSettings, RuntimeState, RuntimeStatus, SignIn, SignOut};

use super::keys::{self, Broker, KeyError};
use super::run::cli::claude_status;
use super::runtime::sandbox;
use super::runtime::siwc::{self, ChatGptRuntime};
use super::runtime::spawn::HostEnv;
use super::runtime::{claude_code, codex, detect, terminal, uar_sidecar};
use super::settings::{AiSettings, Shared};

/// Opens a page in the owner's browser: whether a browser could be started.
pub(crate) type Opener = Arc<dyn Fn(&str) -> bool + Send + Sync>;
/// Opens the owner's terminal running `claude auth login`: whether it opened.
pub(crate) type TerminalOpener = Arc<dyn Fn(&[String]) -> bool + Send + Sync>;

/// What is under way or was last found, for `status` to answer from without waiting.
#[derive(Default)]
struct Under {
    /// A look for the programs is running.
    scanning: bool,
    /// A probe of Claude Code's status is running.
    probing_claude: bool,
    /// A look for an external UAR is running.
    probing_uar: bool,
    /// What the last look for an external UAR found: listening or not (none: not looked yet).
    uar: Option<bool>,
    /// The ChatGPT sign-in under way, and its cancel.
    chatgpt_sign_in: Option<Arc<AtomicBool>>,
    /// The owner's terminal was opened for `claude auth login` and its status is being asked.
    claude_sign_in: Option<Arc<AtomicBool>>,
}

/// The runtimes' host: the owner's settings and the credential store, the ChatGPT runtime the
/// request threads share, and what is under way.
pub(crate) struct RuntimesHost {
    settings: Shared,
    store: Option<Arc<dyn SecretStore>>,
    chatgpt: Option<Arc<ChatGptRuntime>>,
    open: Opener,
    terminal: TerminalOpener,
    under: Arc<Mutex<Under>>,
    window: Option<egui::Context>,
}

impl RuntimesHost {
    /// The desktop's host: pages open in the owner's browser (`browser.rs`, through the app's
    /// one opener), the terminal through `terminal::open_sign_in`, and the ChatGPT runtime is the
    /// process's one (`siwc::shared`).
    pub(crate) fn new(settings: Shared, store: Option<Arc<dyn SecretStore>>) -> RuntimesHost {
        let chatgpt = store.as_ref().map(siwc::shared);
        let open: Opener = Arc::new(crate::open_in_browser);
        let terminal: TerminalOpener = Arc::new(|path: &[String]| terminal::open_sign_in(path) == terminal::SignInStart::Opened);
        RuntimesHost { settings, store, chatgpt, open, terminal, under: Arc::default(), window: None }
    }

    /// The host with another ChatGPT runtime, browser and terminal (tests: fakes, never a real
    /// account, browser or terminal).
    #[cfg(test)]
    pub(crate) fn with_parts(mut self, chatgpt: Option<Arc<ChatGptRuntime>>, open: Opener, terminal: TerminalOpener) -> RuntimesHost {
        self.chatgpt = chatgpt;
        self.open = open;
        self.terminal = terminal;
        self
    }

    /// The window to wake when something under way ends.
    pub(crate) fn with_window(mut self, ctx: egui::Context) -> RuntimesHost {
        self.window = Some(ctx);
        self
    }

    fn under(&self) -> MutexGuard<'_, Under> {
        lock_under(&self.under)
    }

    /// `work` on a thread of its own named `name`, waking the window when it ends.
    fn background(&self, name: &str, work: impl FnOnce() + Send + 'static) -> Result<(), String> {
        let window = self.window.clone();
        std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                work();
                if let Some(ctx) = window {
                    ctx.request_repaint();
                }
            })
            .map(|_| ())
            .map_err(|_| format!("{name} couldn't be started"))
    }

    /// Asks Claude Code's status again on a thread, unless one is asked already or it isn't found.
    fn probe_claude(&self, settings: &AiSettings) {
        let Some(program) = detected_program(settings, RuntimeKind::ClaudeCode) else { return };
        {
            let mut under = self.under();
            if under.probing_claude {
                return;
            }
            under.probing_claude = true;
        }
        let under = self.under.clone();
        let started = self.background("Claude Code status", move || {
            let (host, sandbox) = (cli_host(), sandbox::detect());
            let cancel = AtomicBool::new(false);
            claude_code::status_shared(claude_status(), Instant::now(), || claude_code::probe_status(&program, &host, &sandbox, &cancel));
            lock_under(&under).probing_claude = false;
        });
        if started.is_err() {
            self.under().probing_claude = false;
        }
    }

    /// Looks for an external UAR on a thread (a `GET` of its agent card on loopback alone).
    fn probe_uar(&self) {
        {
            let mut under = self.under();
            if under.probing_uar {
                return;
            }
            under.probing_uar = true;
        }
        let under = self.under.clone();
        let started = self.background("UAR look", move || {
            let listening = matches!(uar_sidecar::discover(detect::UAR_CARD_URL), uar_sidecar::Discovery::Found(_));
            let mut under = lock_under(&under);
            under.uar = Some(listening);
            under.probing_uar = false;
        });
        if started.is_err() {
            self.under().probing_uar = false;
        }
    }

    fn sign_in_chatgpt(&self) -> Result<(), String> {
        let Some(runtime) = self.chatgpt.clone() else { return Err("the system's credential store isn't available".into()) };
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut under = self.under();
            if under.chatgpt_sign_in.is_some() {
                return Err("a ChatGPT sign-in is already under way".into());
            }
            under.chatgpt_sign_in = Some(cancel.clone());
        }
        let (settings, open, under) = (self.settings.clone(), self.open.clone(), self.under.clone());
        let started = self.background("ChatGPT sign-in", move || {
            sign_in_chatgpt_now(&settings, &runtime, &open, &cancel);
            lock_under(&under).chatgpt_sign_in = None;
        });
        if started.is_err() {
            self.under().chatgpt_sign_in = None;
        }
        started
    }

    fn sign_in_claude(&self) -> Result<(), String> {
        let path = detect::login_path().to_vec();
        if !terminal::can_open(&terminal::launches(), &path) {
            return Err(format!("no terminal could be opened here: run `{}` in a terminal yourself", terminal::SIGN_IN_COMMAND));
        }
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut under = self.under();
            if under.claude_sign_in.is_some() {
                return Err("a terminal is already open for `claude auth login`".into());
            }
            under.claude_sign_in = Some(cancel.clone());
        }
        let program = self.settings.read(|s| detected_program(s, RuntimeKind::ClaudeCode));
        let (open, under) = (self.terminal.clone(), self.under.clone());
        let started = self.background("Claude Code sign-in", move || {
            if open(&path) {
                if let Some(program) = program {
                    watch_claude_sign_in(&program, &cancel);
                }
            } else {
                log::info!("ai runtime claude_code: the terminal for the sign-in didn't open");
            }
            lock_under(&under).claude_sign_in = None;
        });
        if started.is_err() {
            self.under().claude_sign_in = None;
        }
        started
    }

    /// What is known of `runtime` now, without waiting for anything.
    fn known(&self, runtime: &RuntimeId, settings: &AiSettings) -> RuntimeStatus {
        let kind = runtime.kind();
        let found = settings.detected.get(kind.name());
        let path = found.and_then(|d| d.path.clone());
        let version = found.and_then(|d| d.version.clone());
        let scanning = self.under().scanning;
        let state = match runtime {
            RuntimeId::OnDevice => {
                if settings.backup_binding().is_some() {
                    RuntimeState::Ready
                } else {
                    RuntimeState::Unavailable
                }
            }
            RuntimeId::Compat(id) => {
                if settings.provider(id.as_str()).is_some() {
                    RuntimeState::Ready
                } else {
                    RuntimeState::Unavailable
                }
            }
            RuntimeId::ChatGpt => self.chatgpt_state(settings),
            RuntimeId::ClaudeCode => {
                if self.under().claude_sign_in.is_some() {
                    RuntimeState::Checking
                } else {
                    claude_state(found.is_some(), scanning)
                }
            }
            RuntimeId::Codex | RuntimeId::Uar if scanning => RuntimeState::Checking,
            // Nothing of Codex's is run to know it: found is as far as MailCraft looks.
            RuntimeId::Codex => {
                let enabled = settings.chain.get(runtime).is_some_and(ChainEntry::enabled);
                match codex::status(found.is_some(), enabled, None) {
                    codex::Status::NotOffered => RuntimeState::Unavailable,
                    codex::Status::NotFound => RuntimeState::NotFound,
                    codex::Status::SignedOut => RuntimeState::SignedOut,
                    codex::Status::Off | codex::Status::Unchecked => RuntimeState::Ready,
                }
            }
            RuntimeId::Uar => match self.under().uar {
                Some(true) => RuntimeState::Ready,
                Some(false) => RuntimeState::NotFound,
                None => RuntimeState::Unknown,
            },
        };
        RuntimeStatus { state, version, path }
    }

    fn chatgpt_state(&self, settings: &AiSettings) -> RuntimeState {
        if self.store.is_none() {
            return RuntimeState::Unavailable;
        }
        if self.under().chatgpt_sign_in.is_some() {
            return RuntimeState::Checking;
        }
        match self.chatgpt.as_ref().map(|r| r.status()) {
            Some(SiwcStatus::SignedIn) => RuntimeState::Ready,
            Some(SiwcStatus::Waiting) => RuntimeState::Checking,
            Some(SiwcStatus::SignInEnded) => RuntimeState::SignedOut,
            // Not asked yet this session: a sign-in kept from before counts until a request finds otherwise.
            Some(SiwcStatus::SignedOut) | None if settings.chatgpt_key => RuntimeState::Ready,
            Some(SiwcStatus::SignedOut) | None => RuntimeState::SignedOut,
        }
    }
}

fn lock_under(under: &Mutex<Under>) -> MutexGuard<'_, Under> {
    under.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The environment a CLI's probe is given: the login shell's PATH and the owner's home.
fn cli_host() -> HostEnv {
    let separator = if cfg!(windows) { ";" } else { ":" };
    let path = detect::login_path().join(separator);
    HostEnv::from_process(Some(path).filter(|p| !p.is_empty()))
}

/// The program detection found for `kind`, as the owner set it in the chain or as found.
fn detected_program(settings: &AiSettings, kind: RuntimeKind) -> Option<PathBuf> {
    let set = settings.chain.entries().iter().find(|e| e.runtime().kind() == kind).and_then(|e| e.path()).map(RuntimePath::as_str);
    let found = settings.detected.get(kind.name()).and_then(|d| d.path.as_ref()).map(RuntimePath::as_str);
    set.or(found).map(PathBuf::from)
}

/// Claude Code's state from its last status (never asked here).
fn claude_state(found: bool, scanning: bool) -> RuntimeState {
    if scanning {
        return RuntimeState::Checking;
    }
    let last = claude_status().lock().unwrap_or_else(PoisonError::into_inner).last();
    match last {
        Some(AuthStatus::Usable { .. }) => RuntimeState::Ready,
        Some(AuthStatus::NeedsSignIn) => RuntimeState::SignedOut,
        Some(AuthStatus::Broken(_)) => RuntimeState::Unavailable,
        None if found => RuntimeState::Unknown,
        None => RuntimeState::NotFound,
    }
}

/// The ChatGPT plan's sign-in, from start to end: the host id kept in the settings, OpenAI's page
/// opened (a browser that can't be started ends it at once instead of after ten minutes), the
/// sign-in kept in the credential store by the runtime and noted in the settings.
pub(crate) fn sign_in_chatgpt_now(settings: &Shared, runtime: &ChatGptRuntime, open: &Opener, cancel: &Arc<AtomicBool>) {
    let host_id = match settings.change(|s| siwc::ensure_host_id(s).map_err(|e| e.to_string())) {
        Ok(id) => id,
        Err(e) => {
            log::warn!("the ChatGPT sign-in couldn't start: {e}");
            return;
        }
    };
    let stop = cancel.clone();
    let opener = open.clone();
    let open_page = move |url: &str| {
        if !opener(url) {
            log::info!("no browser could be started for the ChatGPT sign-in");
            stop.store(true, Ordering::SeqCst);
        }
    };
    match runtime.sign_in(&host_id, &open_page, cancel) {
        Ok(_) => {
            if let Err(e) = settings.change(|s| {
                siwc::signed_in_settings(s);
                Ok(())
            }) {
                log::warn!("signed in with ChatGPT but the settings couldn't say so: {e}");
            }
        }
        Err(e) => log::info!("the ChatGPT sign-in didn't finish: {e}"),
    }
}

/// Asks Claude Code's status until it is usable, the owner cancels or ten minutes pass; a usable
/// answer is kept for the chain.
fn watch_claude_sign_in(program: &std::path::Path, cancel: &AtomicBool) {
    let (host, sandbox) = (cli_host(), sandbox::detect());
    let mut probe = || claude_code::probe_status(program, &host, &sandbox, cancel);
    if let terminal::PollEnd::SignedIn(status) = terminal::poll_sign_in_default(&mut probe, cancel) {
        claude_status().lock().unwrap_or_else(PoisonError::into_inner).record(Instant::now(), status);
    }
}

/// The runtime settings `settings` hold.
pub(crate) fn runtime_settings(settings: &AiSettings) -> RuntimeSettings {
    RuntimeSettings { chain: settings.chain.clone(), consents: settings.consents.clone(), fallback: settings.fallback }
}

/// The providers the owner set up, as `where_of` takes them.
fn compat_providers(settings: &AiSettings) -> Vec<CompatProvider> {
    settings
        .providers
        .iter()
        .filter_map(|p| {
            let kind = match p.kind {
                ProviderKind::Local => CompatKind::Local,
                ProviderKind::Remote => CompatKind::Remote,
                ProviderKind::Gateway => CompatKind::Gateway,
            };
            ProviderId::new(&p.id).ok().map(|id| CompatProvider { id, kind })
        })
        .collect()
}

/// Forget provider `id`'s key in `store` (both its names) and note it in `settings`.
pub(crate) fn forget_compat_key(settings: &Shared, store: &dyn SecretStore, id: &ProviderId) -> Result<(), KeyError> {
    Broker::global().forget_late(id.as_str());
    keys::forget(store, id.as_str())?;
    if let Err(e) = settings.change(|s| {
        s.unnote_key(id.as_str());
        Ok(())
    }) {
        log::warn!("a model provider's key was forgotten but the settings couldn't say so: {e}");
    }
    Ok(())
}

/// Sign out of the ChatGPT plan: `runtime` deletes the entry `ai.chatgpt` and forgets its access
/// token in memory, and `settings` turn the runtime off and take `chatgpt` out of every account's
/// consents ([`siwc::signed_out_settings`]). Nothing is sent to OpenAI. When the credential store
/// couldn't remove the entry, the settings still note it (so that "Remove MailCraft's data"
/// deletes it), and the error says so.
pub(crate) fn forget_chatgpt(settings: &Shared, runtime: &ChatGptRuntime) -> Result<(), String> {
    let removed = runtime.sign_out();
    if let Err(e) = settings.change(|s| {
        siwc::signed_out_settings(s);
        if removed.is_err() {
            s.chatgpt_key = true;
        }
        Ok(())
    }) {
        log::warn!("signed out of the ChatGPT plan but the settings couldn't say so: {e}");
    }
    removed.map_err(|e| e.to_string())
}

impl AiRuntimes for RuntimesHost {
    fn settings(&self) -> RuntimeSettings {
        self.settings.read(runtime_settings)
    }

    fn save(&self, settings: &RuntimeSettings) -> Result<(), String> {
        let wanted = settings.clone();
        self.settings.change(move |s| {
            s.set_chain(wanted.chain);
            s.consents = wanted.consents;
            s.mirror_consents();
            s.fallback = wanted.fallback;
            Ok(())
        })
    }

    fn where_of(&self, runtime: &RuntimeId) -> Where {
        self.settings.read(|s| {
            let entry = s.chain.get(runtime).cloned().unwrap_or_else(|| ChainEntry::new(runtime.clone()));
            where_of(&entry, &compat_providers(s), None)
        })
    }

    fn status(&self, runtime: &RuntimeId) -> RuntimeStatus {
        let settings = self.settings.snapshot();
        let status = self.known(runtime, &settings);
        // Refreshes that are due start now, on threads of their own; their ends show next time.
        match runtime {
            RuntimeId::ClaudeCode if claude_status().lock().unwrap_or_else(PoisonError::into_inner).needs_probe(Instant::now()) => {
                self.probe_claude(&settings);
            }
            RuntimeId::Uar if self.under().uar.is_none() => self.probe_uar(),
            _ => {}
        }
        status
    }

    fn rescan(&self) -> Result<(), String> {
        {
            let mut under = self.under();
            if under.scanning {
                return Ok(());
            }
            under.scanning = true;
        }
        // Claude Code's status is asked again too, once its program has been looked for.
        claude_status().lock().unwrap_or_else(PoisonError::into_inner).mark_signed_out();
        let (settings, under) = (self.settings.clone(), self.under.clone());
        let started = self.background("AI runtimes look", move || {
            if let Some(Err(e)) = detect::look(&settings, true) {
                log::warn!("the runtimes were looked for but the settings couldn't keep what was found: {e}");
            }
            if let Some(program) = detected_program(&settings.snapshot(), RuntimeKind::ClaudeCode) {
                let (host, sandbox, cancel) = (cli_host(), sandbox::detect(), AtomicBool::new(false));
                claude_code::status_shared(claude_status(), Instant::now(), || claude_code::probe_status(&program, &host, &sandbox, &cancel));
            }
            lock_under(&under).scanning = false;
        });
        if let Err(e) = started {
            self.under().scanning = false;
            return Err(e);
        }
        // And an external UAR is looked for again.
        self.under().uar = None;
        self.probe_uar();
        Ok(())
    }

    fn sign_in(&self, how: SignIn) -> Result<(), String> {
        match how {
            SignIn::ChatGpt => self.sign_in_chatgpt(),
            SignIn::ClaudeCodeTerminal => self.sign_in_claude(),
            SignIn::ManagedUar => Err("this version doesn't start a Universal Agent Runtime of its own: start yours, then look again".into()),
        }
    }

    fn cancel_sign_in(&self, how: SignIn) {
        let under = self.under();
        let cancel = match how {
            SignIn::ChatGpt => under.chatgpt_sign_in.as_ref(),
            SignIn::ClaudeCodeTerminal => under.claude_sign_in.as_ref(),
            SignIn::ManagedUar => None,
        };
        if let Some(cancel) = cancel {
            cancel.store(true, Ordering::SeqCst);
        }
    }

    fn sign_out(&self, what: &SignOut) -> Result<(), String> {
        let Some(store) = self.store.clone() else { return Err("the system's credential store isn't available".into()) };
        let (settings, what) = (self.settings.clone(), what.clone());
        if what == SignOut::ChatGpt
            && let Some(cancel) = self.under().chatgpt_sign_in.as_ref()
        {
            // A sign-in under way ends: nothing of it is kept.
            cancel.store(true, Ordering::SeqCst);
        }
        let chatgpt = self.chatgpt.clone();
        self.background("AI sign-out", move || {
            let done = match &what {
                SignOut::CompatKey(id) => forget_compat_key(&settings, store.as_ref(), id).map_err(|e| e.to_string()),
                SignOut::ChatGpt => forget_chatgpt(&settings, &chatgpt.unwrap_or_else(|| siwc::shared(&store))),
            };
            if let Err(e) = done {
                log::warn!("signing out couldn't take the entry out of the credential store: {e}");
            }
        })
    }
}
