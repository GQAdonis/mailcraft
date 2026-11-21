// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The runtimes that aren't `mailcraft-ai` providers, at their hop of the chain (ai-runtime-chain
//! 20.2): Claude Code and Codex (the owner's own CLIs, run contained by the spawner, which returns
//! only once the run's process group is gone), the ChatGPT plan (MailCraft's own sign-in) and an
//! external UAR the owner chose (UAR external only, W7). A managed UAR isn't run by this version.
//!
//! Each status is read at the hop, never before: a CLI whose program isn't there or isn't one is
//! `not_installed`; Claude Code's own login is asked with `claude auth status` (cached, its own
//! process, the login never read by MailCraft); Codex is offered only where a sandbox is; the
//! ChatGPT plan needs a kept sign-in and a model; an external UAR must answer with its agent card.
//!
//! What a runtime is given is the request's messages: the system messages as its instructions,
//! the rest as its prompt, marked data. The mail texts are handed to the spawner as secrets, so
//! that they never reach an argv or a log.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use mailcraft_ai::runtime::claude_code::AuthStatus;
use mailcraft_ai::runtime::contain::Limits;
use mailcraft_ai::runtime::responses::ResponsesRequest;
use mailcraft_ai::runtime::siwc::SiwcStatus;
use mailcraft_host::SecretStore;
use mailcraft_model::ai::{ChainEntry, ErrorKind, ModelRequest, Role, RuntimeId, UarInstance};

use super::super::route::Unusable;
use super::super::runtime::sandbox::{self, Sandbox};
use super::super::runtime::siwc::{self, AskError, ChatGptRuntime, SiwcError};
use super::super::runtime::spawn::{HostEnv, Notice};
use super::super::runtime::{claude_code, codex, detect, uar_sidecar};
use super::super::settings::{AiSettings, Shared};

/// Where an external UAR is asked (`UAR_CARD_URL`'s instance).
const UAR_BASE: &str = "http://127.0.0.1:1906";

/// Claude Code's last status, for the session. Locked only to read it and to keep an answer,
/// never across a probe or a run (`claude_code::status_shared`, `claude_code::ask`).
pub(crate) fn claude_status() -> &'static Mutex<claude_code::StatusCache> {
    static CACHE: OnceLock<Mutex<claude_code::StatusCache>> = OnceLock::new();
    CACHE.get_or_init(Mutex::default)
}

/// What the runs of one walk share: the environment handed to a CLI and the sandbox, each made
/// once, when first needed; and the ChatGPT runtime, the process's one (`siwc::shared`) unless a
/// test hands it another.
#[derive(Default)]
pub(crate) struct Cli {
    host: Option<HostEnv>,
    sandbox: Option<Sandbox>,
    chatgpt: Option<Arc<ChatGptRuntime>>,
}

/// The owner's program for `entry`: the path the owner gave, else what detection found.
fn program(entry: &ChainEntry, settings: &AiSettings) -> Option<PathBuf> {
    entry
        .path()
        .map(|p| PathBuf::from(p.as_str()))
        .or_else(|| settings.detected.get(entry.runtime().kind().name()).and_then(|d| d.path.as_ref()).map(|p| PathBuf::from(p.as_str())))
}

/// The program for `entry`, checked again now: there, a file, runnable.
fn checked_program(entry: &ChainEntry, settings: &AiSettings) -> Result<PathBuf, Unusable> {
    let path = program(entry, settings).ok_or(Unusable::NotInstalled)?;
    detect::check_executable(&path).map_err(|_| Unusable::NotInstalled)
}

/// The request as a runtime takes it: its instructions (the system messages), its prompt (the
/// rest), and the texts that must never reach an argv or a log (everything in the prompt; the
/// caller adds the keys and tokens MailCraft holds).
pub(crate) fn split(request: &ModelRequest) -> (String, String, Vec<String>) {
    let mut instructions = Vec::new();
    let mut prompt = Vec::new();
    let mut secrets = Vec::new();
    for message in &request.messages {
        match message.role {
            Role::System => instructions.push(message.content.as_str()),
            Role::User => {
                prompt.push(message.content.clone());
                secrets.push(message.content.clone());
            }
            Role::Assistant => {
                prompt.push(format!("(An earlier answer of yours, data only:)\n{}", message.content));
                secrets.push(message.content.clone());
            }
        }
    }
    (instructions.join("\n\n"), prompt.join("\n\n"), secrets)
}

/// A ChatGPT failure as the chain reports it. A sign-in OpenAI ended turns the runtime off.
fn chatgpt_kind(error: &AskError, settings: &Shared) -> ErrorKind {
    match error {
        AskError::Sign(SiwcError::Ended) => {
            if let Err(e) = settings.change(|s| {
                siwc::ended_settings(s);
                Ok(())
            }) {
                log::warn!("the ChatGPT sign-in ended but the settings couldn't say so: {e}");
            }
            ErrorKind::SignedOut
        }
        AskError::Sign(SiwcError::Store(_)) => ErrorKind::CredentialStore,
        AskError::Sign(SiwcError::Waiting { .. }) => ErrorKind::Unreachable,
        AskError::Sign(_) | AskError::Status(401) => ErrorKind::SignedOut,
        AskError::Build(_) => ErrorKind::Settings,
        AskError::ToolUseStopped(_) => ErrorKind::ToolUseStopped,
        AskError::Failed(_) | AskError::NoAnswer => ErrorKind::Malformed,
        AskError::TooLarge => ErrorKind::TooLarge,
        AskError::Status(status) => ErrorKind::Refused { status: *status },
        AskError::Net(_) => ErrorKind::Unreachable,
    }
}

impl Cli {
    /// The runs' share with `chatgpt` as the ChatGPT runtime (tests: one on a fake server).
    #[cfg(test)]
    pub(crate) fn with_chatgpt(chatgpt: Arc<ChatGptRuntime>) -> Cli {
        Cli { chatgpt: Some(chatgpt), ..Cli::default() }
    }

    /// The ChatGPT runtime: the one handed in, else the process's one.
    fn chatgpt(&self, store: &Arc<dyn SecretStore>) -> Arc<ChatGptRuntime> {
        self.chatgpt.clone().unwrap_or_else(|| siwc::shared(store))
    }

    fn host(&mut self) -> &HostEnv {
        self.host.get_or_insert_with(|| {
            let separator = if cfg!(windows) { ";" } else { ":" };
            let path = detect::login_path().join(separator);
            HostEnv::from_process(Some(path).filter(|p| !p.is_empty()))
        })
    }

    fn sandbox(&mut self) -> Sandbox {
        self.sandbox.get_or_insert_with(sandbox::detect).clone()
    }

    /// Whether the runtime of `entry` can be used now (`store`: the system has a credential store).
    pub(crate) fn status(&mut self, entry: &ChainEntry, settings: &AiSettings, store: bool, cancel: &AtomicBool) -> Result<(), Unusable> {
        match entry.runtime() {
            RuntimeId::ChatGpt => {
                if !store || !settings.chatgpt_key {
                    return Err(Unusable::SignedOut);
                }
                entry.model().map(|_| ()).ok_or(Unusable::NotSetUp)
            }
            RuntimeId::ClaudeCode => {
                let program = checked_program(entry, settings)?;
                let (host, sandbox) = (self.host().clone(), self.sandbox());
                match claude_code::status_shared(claude_status(), Instant::now(), || claude_code::probe_status(&program, &host, &sandbox, cancel)) {
                    AuthStatus::Usable { .. } => Ok(()),
                    AuthStatus::NeedsSignIn => Err(Unusable::SignedOut),
                    AuthStatus::Broken(_) => Err(Unusable::Broken),
                }
            }
            RuntimeId::Codex => {
                if !codex::offered() {
                    return Err(Unusable::NotOffered);
                }
                checked_program(entry, settings).map(|_| ())
            }
            RuntimeId::Uar => {
                if entry.instance() != Some(UarInstance::External) {
                    return Err(Unusable::NotOffered);
                }
                match uar_sidecar::discover(detect::UAR_CARD_URL) {
                    uar_sidecar::Discovery::Found(_) => Ok(()),
                    uar_sidecar::Discovery::Absent => Err(Unusable::NotRunning),
                    uar_sidecar::Discovery::Broken => Err(Unusable::Broken),
                }
            }
            RuntimeId::OnDevice | RuntimeId::Compat(_) => Err(Unusable::NotOffered),
        }
    }

    /// Ask the runtime of `entry` `request`: the whole answer, or the kind of failure. A tool-use
    /// stop is the kind `tool_use_stopped`; the walk tells the owner before any later hop. `held`:
    /// the keys MailCraft holds in memory, which no CLI's argv may carry either.
    pub(crate) fn ask(
        &mut self,
        entry: &ChainEntry,
        request: &ModelRequest,
        settings: &Shared,
        store: Option<&Arc<dyn SecretStore>>,
        held: &[String],
        cancel: &AtomicBool,
    ) -> Result<String, ErrorKind> {
        let (instructions, prompt, mut secrets) = split(request);
        secrets.extend(held.iter().filter(|h| !h.is_empty()).cloned());
        if let Some(token) = store.and_then(|s| self.chatgpt(s).held_token()) {
            secrets.push(token);
        }
        let model = entry.model().map(|m| m.as_str());
        let limits = Limits::default();
        // The walk tells the owner of a tool-use stop from the run's kind.
        let mut notify = |_: Notice| {};
        let snapshot = settings.snapshot();
        match entry.runtime() {
            RuntimeId::ChatGpt => {
                let (Some(store), Some(model)) = (store, model) else { return Err(ErrorKind::SignedOut) };
                let asked = ResponsesRequest { model, instructions: &instructions, data: &prompt };
                let runtime = self.chatgpt(store);
                if runtime.status() == SiwcStatus::SignInEnded {
                    return Err(ErrorKind::SignedOut);
                }
                runtime.ask(&asked, cancel).map_err(|e| chatgpt_kind(&e, settings))
            }
            RuntimeId::ClaudeCode => {
                let program = checked_program(entry, &snapshot).map_err(Unusable::kind)?;
                let (host, sandbox) = (self.host().clone(), self.sandbox());
                let ask = claude_code::Ask {
                    program: &program,
                    host: &host,
                    sandbox: &sandbox,
                    model,
                    instructions: &instructions,
                    prompt: &prompt,
                    secrets: &secrets,
                    limits,
                };
                claude_code::ask(&ask, claude_status(), cancel, &mut notify)
            }
            RuntimeId::Codex => {
                let program = checked_program(entry, &snapshot).map_err(Unusable::kind)?;
                let recipe = codex::recipe_from_process(model).map_err(|e| {
                    log::info!("ai runtime codex: not run ({})", e.kind());
                    ErrorKind::Settings
                })?;
                let (host, sandbox) = (self.host().clone(), self.sandbox());
                let ask = codex::Ask {
                    program,
                    recipe: &recipe,
                    instructions: &instructions,
                    prompt: &prompt,
                    secrets: &secrets,
                    host: &host,
                    sandbox: &sandbox,
                    limits,
                };
                codex::ask(&ask, cancel, &mut notify)
            }
            RuntimeId::Uar if entry.instance() == Some(UarInstance::External) => {
                match uar_sidecar::run_external(true, UAR_BASE, &instructions, &prompt, limits, cancel, &mut notify) {
                    super::super::runtime::spawn::RunEnd::Answer(text) => Ok(text),
                    other => Err(other.error_kind().unwrap_or(ErrorKind::BrokenRuntime)),
                }
            }
            RuntimeId::Uar | RuntimeId::OnDevice | RuntimeId::Compat(_) => Err(ErrorKind::Settings),
        }
    }
}

#[cfg(test)]
#[path = "cli_tests.rs"]
mod cli_tests;
