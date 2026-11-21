// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Codex host recipe and status (B6, tasks 16.4–16.6).
//!
//! Codex runs unmodified, on the owner's own ChatGPT login, with `CODEX_HOME` set to the owner's
//! Codex home: the absolute value of `CODEX_HOME` in MailCraft's own environment, else
//! `~/.codex`. MailCraft reads that variable's value and nothing inside the folder: no code here
//! opens `auth.json`, `config.toml` or any other file under it, and none runs `codex login` (the
//! argv is checked before every spawn, `mailcraft_ai::runtime::codex::check_args`). Signing in is
//! the owner running `codex login` in a terminal.
//!
//! - **Environment**: the containment allow-list, a per-run `HOME` (the entry check found the
//!   login through `CODEX_HOME` alone) and `CODEX_HOME`. Never `CODEX_API_KEY` or `OPENAI_*`
//!   (`contain::is_forbidden_env_name` refuses them whatever a recipe says).
//! - **Instructions** go ahead of the prompt on stdin (`codex::stdin_text`): `exec` has no flag
//!   that takes the instructions file's path as an argument of its own.
//! - **Sandbox required**: Codex is never spawned without the OS sandbox (`no_sandbox`), and is
//!   not offered on Windows, which has none. The profile lets it read its install folder and the
//!   Codex home, and write its run folder and the Codex home (the entry check found it keeps its
//!   state databases there, and a refreshed sign-in must be saved, or the owner's login breaks).
//! - **Status without a probe**: no status command of Codex's is run (that would be
//!   `codex login status`). Found and enabled is [`Status::Unchecked`]; a run that fails for want
//!   of a sign-in is [`Status::SignedOut`], whose words tell the owner to run `codex login`.
//!   Codex is off until the owner turns it on (`ChainEntry::new` is disabled).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use mailcraft_ai::runtime::codex::{self as pure, CodexDetector, Failure};
use mailcraft_ai::runtime::contain::Limits;
use mailcraft_model::ai::ErrorKind;

use super::sandbox::{Guard, Sandbox};
use super::spawn::{self, Arg, Home, HostEnv, Notice, Recipe, RunEnd, RunReport, RunSpec};

/// The runtime's name, as logged.
pub(crate) const NAME: &str = "codex";

/// The one variable of MailCraft's own environment the recipe reads (its value only).
pub(crate) const CODEX_HOME: &str = "CODEX_HOME";

/// The question Check in Settings sends: synthetic, with no mail in it.
#[cfg_attr(not(test), expect(dead_code, reason = "Check in Settings (B11) isn't in the window yet"))]
pub(crate) const CHECK_PROMPT: &str = "Reply with the single word OK.";

/// The instructions that go with [`CHECK_PROMPT`].
#[cfg_attr(not(test), expect(dead_code, reason = "Check in Settings (B11) isn't in the window yet"))]
pub(crate) const CHECK_INSTRUCTIONS: &str = "You are checking that you can answer. Use no tools.";

/// Whether Codex is offered on this computer: not on Windows, which has no sandbox for it.
pub(crate) fn offered() -> bool {
    !cfg!(windows)
}

fn plain_absolute(p: &Path) -> bool {
    p.is_absolute() && p.to_str().is_some_and(|s| !s.chars().any(char::is_control))
}

/// The owner's Codex home: `value` (the `CODEX_HOME` of MailCraft's own environment) when it is
/// an absolute path, else `<home>/.codex`. `None` when neither is known. Nothing is read from
/// the folder.
pub(crate) fn owner_codex_home(value: Option<&OsStr>, home: Option<&str>) -> Option<PathBuf> {
    if let Some(p) = value.map(Path::new).filter(|p| plain_absolute(p)) {
        return Some(p.to_path_buf());
    }
    let home = Path::new(home?);
    plain_absolute(home).then(|| home.join(".codex"))
}

/// [`owner_codex_home`] from MailCraft's own environment (the values of `CODEX_HOME` and `HOME`).
pub(crate) fn owner_codex_home_from_process() -> Option<PathBuf> {
    let value = std::env::var_os(CODEX_HOME);
    let home = std::env::var("HOME").ok();
    owner_codex_home(value.as_deref(), home.as_deref())
}

/// Why no recipe could be made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecipeError {
    /// The owner's Codex home isn't known or isn't a plain absolute path.
    NoCodexHome,
    /// The argv was refused (a model's name that can't be passed).
    Argv(pure::CodexError),
}

impl RecipeError {
    pub(crate) fn kind(self) -> &'static str {
        match self {
            RecipeError::NoCodexHome => "codex_home_unknown",
            RecipeError::Argv(e) => e.kind(),
        }
    }
}

/// What of the Codex home Codex runs as code or takes as its instructions, never written by a run
/// even though the folder is writable: `config.toml`, `AGENTS.md`, its skills and its agents.
pub(crate) fn guarded(codex_home: &Path) -> Vec<Guard> {
    vec![
        Guard::File(codex_home.join("config.toml")),
        Guard::File(codex_home.join("AGENTS.md")),
        Guard::Tree(codex_home.join("skills")),
        Guard::Tree(codex_home.join("agents")),
    ]
}

/// The recipe of one run: the plan's argv (and `-m` when the owner chose a model), a per-run
/// `HOME`, `CODEX_HOME` set to `codex_home`, sandbox required.
pub(crate) fn recipe(codex_home: &Path, model: Option<&str>) -> Result<Recipe, RecipeError> {
    let home_text = codex_home.to_str().filter(|_| plain_absolute(codex_home)).ok_or(RecipeError::NoCodexHome)?;
    let args = pure::argv(model).map_err(RecipeError::Argv)?;
    Ok(Recipe {
        name: NAME,
        may_run_unsandboxed: false,
        home: Home::PerRun,
        args: args.into_iter().map(Arg::Lit).collect(),
        env: vec![(CODEX_HOME.to_owned(), home_text.to_owned())],
        read_paths: vec![codex_home.to_path_buf()],
        write_paths: vec![codex_home.to_path_buf()],
        deny_write: guarded(codex_home),
    })
}

/// The recipe with the owner's Codex home from MailCraft's own environment.
pub(crate) fn recipe_from_process(model: Option<&str>) -> Result<Recipe, RecipeError> {
    recipe(&owner_codex_home_from_process().ok_or(RecipeError::NoCodexHome)?, model)
}

/// One question to Codex.
pub(crate) struct Ask<'a> {
    /// The program as detection found it.
    pub(crate) program: PathBuf,
    pub(crate) recipe: &'a Recipe,
    pub(crate) instructions: &'a str,
    pub(crate) prompt: &'a str,
    /// The mail texts in the prompt and every key and token MailCraft holds (never on argv).
    pub(crate) secrets: &'a [String],
    pub(crate) host: &'a HostEnv,
    pub(crate) sandbox: &'a Sandbox,
    pub(crate) limits: Limits,
}

/// The arguments of a recipe as text (the literal ones; Codex's recipe has no others).
fn literal_args(recipe: &Recipe) -> Option<Vec<&str>> {
    recipe
        .args
        .iter()
        .map(|a| match a {
            Arg::Lit(s) => Some(s.as_str()),
            Arg::InstructionsFile | Arg::WorkDir => None,
        })
        .collect()
}

/// Asks Codex one question contained. The answer, or the error the chain reports. A tool event
/// tells `notify` before this returns.
pub(crate) fn ask(a: &Ask<'_>, cancel: &AtomicBool, notify: &mut dyn FnMut(Notice)) -> Result<String, ErrorKind> {
    if !offered() {
        return Err(ErrorKind::NoSandbox);
    }
    // No spawn of Codex ever signs in or out, or asks for approvals: checked here, before anything runs.
    let Some(args) = literal_args(a.recipe) else {
        log::info!("ai runtime {NAME}: refused (codex_arg_not_literal)");
        return Err(ErrorKind::Settings);
    };
    if let Err(e) = pure::check_args(&args) {
        log::warn!("ai runtime {NAME}: refused ({})", e.kind());
        return Err(ErrorKind::Settings);
    }
    let stdin = pure::stdin_text(a.instructions, a.prompt);
    let spec = RunSpec {
        program: a.program.clone(),
        recipe: a.recipe,
        instructions: a.instructions,
        prompt: &stdin,
        secrets: a.secrets,
        host: a.host,
        sandbox: a.sandbox,
        limits: a.limits,
    };
    let mut detector = CodexDetector::new();
    let report = spawn::run(&spec, &mut detector, cancel, notify);
    outcome(&report, detector.failure())
}

/// Check in Settings: one synthetic question, no mail.
#[cfg_attr(not(test), expect(dead_code, reason = "Check in Settings (B11) isn't in the window yet"))]
pub(crate) fn check(a: &Ask<'_>, cancel: &AtomicBool, notify: &mut dyn FnMut(Notice)) -> Result<(), ErrorKind> {
    let synthetic = Ask {
        program: a.program.clone(),
        recipe: a.recipe,
        instructions: CHECK_INSTRUCTIONS,
        prompt: CHECK_PROMPT,
        secrets: &[],
        host: a.host,
        sandbox: a.sandbox,
        limits: a.limits,
    };
    ask(&synthetic, cancel, notify).map(|_| ())
}

fn failure_kind(f: Failure) -> ErrorKind {
    match f {
        Failure::SignedOut => ErrorKind::SignedOut,
        Failure::Unreachable => ErrorKind::Unreachable,
        Failure::Refused { status } => ErrorKind::Refused { status },
    }
}

/// What a run's end means for the chain. A run that ended badly after Codex reported a failed
/// turn is that failure; else a stderr that says Codex isn't signed in is `signed_out`.
pub(crate) fn outcome(report: &RunReport, failure: Option<Failure>) -> Result<String, ErrorKind> {
    match &report.end {
        RunEnd::Answer(text) => Ok(text.clone()),
        RunEnd::Broken { kind } if *kind == "exit_status" || *kind == "no_answer" => {
            if let Some(f) = failure {
                return Err(failure_kind(f));
            }
            let stderr = String::from_utf8_lossy(&report.stderr);
            if pure::classify(&stderr) == Failure::SignedOut {
                return Err(ErrorKind::SignedOut);
            }
            Err(ErrorKind::BrokenRuntime)
        }
        other => Err(other.error_kind().unwrap_or(ErrorKind::BrokenRuntime)),
    }
}

/// Codex's state as Settings shows it, worked out without running anything of Codex's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    /// Not offered on this computer (Windows).
    NotOffered,
    /// Detection didn't find it.
    NotFound,
    /// Found, and off (the default).
    Off,
    /// Found and on: it uses the owner's Codex sign-in, which MailCraft doesn't look at.
    Unchecked,
    /// The last run failed for want of a sign-in.
    SignedOut,
}

/// The status from what detection found, the owner's switch and the last run's error.
pub(crate) fn status(found: bool, enabled: bool, last_error: Option<ErrorKind>) -> Status {
    if !offered() {
        Status::NotOffered
    } else if !found {
        Status::NotFound
    } else if !enabled {
        Status::Off
    } else if last_error == Some(ErrorKind::SignedOut) {
        Status::SignedOut
    } else {
        Status::Unchecked
    }
}

/// What the owner is told, in English (the window's `tl!` key).
#[cfg_attr(not(test), expect(dead_code, reason = "the window words Codex's state itself (`ai.runtimes`' status codes)"))]
pub(crate) fn status_words(s: Status) -> &'static str {
    match s {
        Status::NotOffered => "Codex isn't offered on this computer: it needs the system's sandbox.",
        Status::NotFound => "Codex wasn't found on this computer.",
        Status::Off => "Codex is off.",
        Status::Unchecked => "Uses your Codex sign-in.",
        Status::SignedOut => SIGNED_OUT_WORDS,
    }
}

/// What the owner is told when Codex isn't signed in.
#[cfg_attr(not(test), expect(dead_code, reason = "the window words Codex's state itself (`ai.runtimes`' status codes)"))]
pub(crate) const SIGNED_OUT_WORDS: &str =
    "Codex isn't signed in. Open a terminal, run codex login, and sign in with your ChatGPT account. MailCraft never signs in for you.";
