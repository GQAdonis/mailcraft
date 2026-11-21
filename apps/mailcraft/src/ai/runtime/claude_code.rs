// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Claude Code host recipe and status probe (B5, tasks 15.5–15.6).
//!
//! The owner's unmodified `claude` runs on the owner's own login with the **real HOME** (a run's
//! own HOME is "Not logged in"), through B4's spawner ([`super::spawn::run`]): argv from
//! [`mailcraft_ai::runtime::claude_code::run_args`], the environment built from nothing (no
//! `ANTHROPIC_*` or `CLAUDE_*` variable can reach it, `build_env` refuses them), the prompt on
//! stdin, the instructions in the run's 0600 file, an empty 0700 working folder.
//!
//! **MailCraft never reads the login.** Nothing here opens `~/.claude`, `~/.claude.json` or the
//! Keychain: their paths are only handed to the sandbox, so that the CLI, in its own process, may
//! read them (SCN-RT-AUTH-001 audits this under a profile that kills MailCraft on any such read).
//!
//! The sandbox (W7 entry checks on Claude Code 2.1.326):
//! - reads: the CLI's install folder (added by the spawner), `~/.claude`, `~/.claude.json`, and on
//!   macOS `~/Library/Keychains` (without it the CLI reports "Not logged in": it reads its login
//!   from the login Keychain);
//! - writes: the run folder, the CLI's per-user temporary folders (`/tmp/claude-<uid>`,
//!   `/tmp/cc-socks…`, `/tmp/cc-daemon-<uid>`: the CLI won't start without them, and only
//!   `CLAUDE_CODE_TMPDIR`, which may not reach a runtime, moves them), and `~/.claude` and the
//!   Keychain folder, so that a refreshed sign-in is kept and the owner's login is never broken.
//!   `~/.claude.json` is rewritten by the CLI through a temporary file beside it in HOME, which
//!   a sandbox path can't allow without allowing all of HOME; runs work without it (checked).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use mailcraft_ai::runtime::claude_code::{
    self as pure, AuthStatus, ClaudeArg, ClaudeStreamDetector, MAX_STATUS_BYTES, RunOutcome, STATUS_ARGS, StatusProblem,
};
use mailcraft_ai::runtime::contain::{Gate, Limits, StreamDetector, Verdict};
use mailcraft_model::ai::ErrorKind;

use super::sandbox::{Guard, Sandbox};
use super::spawn::{Arg, Home, HostEnv, Notice, Recipe, RunEnd, RunSpec, run};

/// The runtime's name, as logged.
pub(crate) const NAME: &str = "claude_code";

/// When a run's prompt is written.
///
/// DEVIATION (W7 entry check, for the lead): Claude Code 2.1.326 writes its `init` event only
/// after it has read its first input, in text and in `--input-format stream-json` alike (an SDK
/// `initialize` control request is answered without `tools` or `mcp_servers`, and with the
/// account). Under [`Gate::AfterInit`] the spawner would wait for an init that never comes, and
/// every run would time out. Until the lead decides (a two-step stdin write in the spawner, or
/// this), the prompt is written at once; the detector still kills a run whose init lists a tool or
/// an MCP server before any of its answer is kept, and `--tools ""`, `--strict-mcp-config` and
/// `--safe-mode` keep the init empty.
pub(crate) const RUN_GATE: Gate = Gate::Now;

/// How long the status probe has, all of it.
pub(crate) const STATUS_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a status is believed before it is asked again.
pub(crate) const STATUS_TTL: Duration = Duration::from_secs(5 * 60);

/// The user id this program runs as, read from a file it makes (no `unsafe` call needed).
#[cfg(unix)]
fn own_uid() -> Option<u32> {
    use std::os::unix::fs::MetadataExt as _;
    tempfile::tempfile().and_then(|f| f.metadata()).ok().map(|m| m.uid())
}

/// The CLI's per-user temporary folders. It uses `/tmp` whatever `TMPDIR` says.
fn cli_temp_dirs() -> Vec<PathBuf> {
    #[cfg(unix)]
    {
        let tmp = if cfg!(target_os = "macos") { "/private/tmp" } else { "/tmp" };
        let mut out = vec![PathBuf::from(format!("{tmp}/cc-socks"))];
        if let Some(uid) = own_uid() {
            for name in [format!("claude-{uid}"), format!("cc-socks-{uid}"), format!("cc-daemon-{uid}")] {
                out.push(PathBuf::from(tmp).join(name));
            }
        }
        out
    }
    #[cfg(not(unix))]
    {
        Vec::new()
    }
}

/// The CLI's own config paths beneath `home`, which only the CLI reads.
fn config_paths(home: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let dir = home.join(".claude");
    let mut read = vec![dir.clone(), home.join(".claude.json")];
    let mut write = vec![dir];
    if cfg!(target_os = "macos") {
        let keychains = home.join("Library").join("Keychains");
        read.push(keychains.clone());
        write.push(keychains);
    }
    write.extend(cli_temp_dirs());
    (read, write)
}

/// What of `~/.claude` the CLI runs as code or takes as its instructions, never written by a
/// run even though the folder is writable (`settings*.json` and the files a rewrite of them goes
/// through, `CLAUDE.md`, and its hooks, plugins, agents, skills and commands). Each settings file
/// is also named by itself, for a sandbox that can't match the start of a name (bubblewrap).
pub(crate) fn guarded(home: &Path) -> Vec<Guard> {
    let dir = home.join(".claude");
    let mut out = vec![
        Guard::Prefix(dir.join("settings")),
        Guard::File(dir.join("settings.json")),
        Guard::File(dir.join("settings.local.json")),
        Guard::File(dir.join("CLAUDE.md")),
    ];
    out.extend(["hooks", "plugins", "agents", "skills", "commands"].iter().map(|d| Guard::Tree(dir.join(d))));
    out
}

/// Why a recipe couldn't be made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecipeError {
    /// The owner's home isn't known or isn't an absolute path.
    NoHome,
    /// The owner's model isn't a model's name.
    BadModel,
}

impl RecipeError {
    pub(crate) fn kind(self) -> &'static str {
        match self {
            RecipeError::NoHome => "no_home",
            RecipeError::BadModel => "bad_model",
        }
    }
}

fn home_of(host: &HostEnv) -> Result<PathBuf, RecipeError> {
    let home = host.home.as_deref().map(PathBuf::from).ok_or(RecipeError::NoHome)?;
    if home.is_absolute() { Ok(home) } else { Err(RecipeError::NoHome) }
}

fn recipe_with(host: &HostEnv, args: Vec<Arg>) -> Result<Recipe, RecipeError> {
    let home = home_of(host)?;
    let (read_paths, write_paths) = config_paths(&home);
    Ok(Recipe { name: NAME, may_run_unsandboxed: true, home: Home::Real, args, env: Vec::new(), read_paths, write_paths, deny_write: guarded(&home) })
}

/// A run's recipe: the real HOME, no variable beyond the base, the CLI's config paths for the
/// sandbox, and the adapter's argv with the owner's model when chosen.
pub(crate) fn run_recipe(host: &HostEnv, model: Option<&str>) -> Result<Recipe, RecipeError> {
    let args = pure::run_args(model)
        .map_err(|_| RecipeError::BadModel)?
        .into_iter()
        .map(|a| match a {
            ClaudeArg::Lit(s) => Arg::Lit(s.to_owned()),
            ClaudeArg::InstructionsFile => Arg::InstructionsFile,
            ClaudeArg::Model(m) => Arg::Lit(m),
        })
        .collect();
    recipe_with(host, args)
}

/// The status probe's recipe: the same environment and sandbox as a run, `auth status --json`.
pub(crate) fn status_recipe(host: &HostEnv) -> Result<Recipe, RecipeError> {
    recipe_with(host, STATUS_ARGS.iter().map(|s| Arg::Lit((*s).to_owned())).collect())
}

// ---------------------------------------------------------------------------------------------
// Status (15.6)
// ---------------------------------------------------------------------------------------------

/// Keeps the status's lines for the parser; shows nothing. The CLI prints its status as pretty
/// JSON on several lines, and exits 1 when signed out, so the lines are read whatever the end.
struct StatusCollector(Rc<RefCell<Vec<u8>>>);

impl StreamDetector for StatusCollector {
    fn gate(&self) -> Gate {
        Gate::Now
    }

    fn line(&mut self, bytes: &[u8]) -> Verdict {
        // Bounded by the probe's stdout cap ([`status_limits`]); the parser checks the size again.
        let mut kept = self.0.borrow_mut();
        kept.extend_from_slice(bytes);
        kept.push(b'\n');
        Verdict::Note("status_line")
    }
}

/// The limits of a status probe: `timeout` in all and in silence, [`MAX_STATUS_BYTES`] of output.
pub(crate) fn status_limits(timeout: Duration) -> Limits {
    Limits { stdout_bytes: MAX_STATUS_BYTES, stderr_bytes: 4 * 1024, line_bytes: MAX_STATUS_BYTES, wall_clock: timeout, idle: timeout }
}

/// Asks `program auth status --json` through the spawner, with a run's environment and sandbox.
pub(crate) fn probe_status(program: &Path, host: &HostEnv, sandbox: &Sandbox, cancel: &AtomicBool) -> AuthStatus {
    probe_status_with(program, host, sandbox, status_limits(STATUS_TIMEOUT), cancel)
}

/// [`probe_status`] with other limits (tests).
pub(crate) fn probe_status_with(program: &Path, host: &HostEnv, sandbox: &Sandbox, limits: Limits, cancel: &AtomicBool) -> AuthStatus {
    let recipe = match status_recipe(host) {
        Ok(r) => r,
        Err(e) => {
            log::info!("ai runtime {NAME}: status not asked ({})", e.kind());
            return AuthStatus::Broken(StatusProblem::NotRun);
        }
    };
    let kept = Rc::new(RefCell::new(Vec::new()));
    let spec = RunSpec { program: program.to_path_buf(), recipe: &recipe, instructions: "", prompt: "", secrets: &[], host, sandbox, limits };
    let report = run(&spec, StatusCollector(Rc::clone(&kept)), cancel, &mut |_: Notice| {});
    let status = match report.end {
        // The stream ended (the CLI exits 1 when signed out): what it printed decides.
        RunEnd::Broken { kind: "no_answer" | "exit_status" } => pure::parse_auth_status(&kept.borrow()),
        RunEnd::TimedOut | RunEnd::Cancelled => AuthStatus::Broken(StatusProblem::Late),
        RunEnd::TooLarge => AuthStatus::Broken(StatusProblem::TooLarge),
        _ => AuthStatus::Broken(StatusProblem::NotRun),
    };
    let what = match status {
        AuthStatus::Usable { .. } => "usable",
        AuthStatus::NeedsSignIn => "needs_sign_in",
        AuthStatus::Broken(p) => p.kind(),
    };
    log::info!("ai runtime {NAME}: status {what}");
    status
}

/// The last status and when it was asked: asked again at most every [`STATUS_TTL`], or at once
/// after a run ended signed out.
#[derive(Debug, Default)]
pub(crate) struct StatusCache {
    last: Option<(Instant, AuthStatus)>,
    stale: bool,
    /// How many runs have ended signed out: a probe that began before the latest doesn't clear it.
    marks: u64,
}

impl StatusCache {
    /// Whether the status must be asked again at `now`.
    pub(crate) fn needs_probe(&self, now: Instant) -> bool {
        match self.last {
            None => true,
            Some((at, _)) => self.stale || now.saturating_duration_since(at) >= STATUS_TTL,
        }
    }

    /// The status, asking `probe` only when it must (holding the cache while it does: tests
    /// only; the app goes through [`status_shared`]).
    #[cfg(test)]
    pub(crate) fn get(&mut self, now: Instant, probe: impl FnOnce() -> AuthStatus) -> AuthStatus {
        if let (false, Some((_, status))) = (self.needs_probe(now), self.last) {
            return status;
        }
        let status = probe();
        self.last = Some((now, status));
        self.stale = false;
        status
    }

    /// A run ended signed out: the next look asks again.
    pub(crate) fn mark_signed_out(&mut self) {
        self.stale = true;
        self.marks = self.marks.wrapping_add(1);
    }

    /// The status kept, when it needn't be asked again at `now`.
    pub(crate) fn fresh(&self, now: Instant) -> Option<AuthStatus> {
        if self.needs_probe(now) { None } else { self.last.map(|(_, status)| status) }
    }

    /// `status`, asked at `at` by somebody other than [`status_shared`] (the sign-in's poll).
    pub(crate) fn record(&mut self, at: Instant, status: AuthStatus) {
        self.last = Some((at, status));
        self.stale = false;
    }

    /// The status the window may show: the last one asked, however old (none: never asked).
    pub(crate) fn last(&self) -> Option<AuthStatus> {
        self.last.map(|(_, status)| status)
    }
}

fn lock_cache(cache: &Mutex<StatusCache>) -> MutexGuard<'_, StatusCache> {
    cache.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The status from `cache`, asking `probe` only when it must. The lock is held only to read the
/// cache and to keep the answer, never while `probe` runs, so another request (and its cancel) is
/// never held up behind a probe. An answer that began before a run ended signed out is shown but
/// doesn't clear that mark.
pub(crate) fn status_shared(cache: &Mutex<StatusCache>, now: Instant, probe: impl FnOnce() -> AuthStatus) -> AuthStatus {
    let marks = {
        let held = lock_cache(cache);
        if let Some(status) = held.fresh(now) {
            return status;
        }
        held.marks
    };
    let status = probe();
    let mut held = lock_cache(cache);
    if held.marks == marks {
        held.last = Some((now, status));
        held.stale = false;
    }
    status
}

// ---------------------------------------------------------------------------------------------
// Runs
// ---------------------------------------------------------------------------------------------

/// A run's end and outcome as the chain sees it: the answer, or the error kind. An error the CLI
/// reported in its stream (signed out, refused, unreachable) is said instead of the run's bare
/// end; a tool stop, a time-out or a cancel always wins.
pub(crate) fn outcome_to_result(end: RunEnd, outcome: RunOutcome) -> Result<String, ErrorKind> {
    match end {
        RunEnd::Answer(text) => Ok(text),
        RunEnd::Broken { .. } => Err(match outcome {
            RunOutcome::SignedOut => ErrorKind::SignedOut,
            RunOutcome::Refused { status } => ErrorKind::Refused { status },
            RunOutcome::Unreachable => ErrorKind::Unreachable,
            RunOutcome::Clean => ErrorKind::BrokenRuntime,
        }),
        other => Err(other.error_kind().unwrap_or(ErrorKind::BrokenRuntime)),
    }
}

/// One question to Claude Code.
pub(crate) struct Ask<'a> {
    pub(crate) program: &'a Path,
    pub(crate) host: &'a HostEnv,
    pub(crate) sandbox: &'a Sandbox,
    pub(crate) model: Option<&'a str>,
    pub(crate) instructions: &'a str,
    pub(crate) prompt: &'a str,
    /// The mail texts in the prompt and every key and token MailCraft holds (never on argv).
    pub(crate) secrets: &'a [String],
    pub(crate) limits: Limits,
}

/// Runs Claude Code once with `gate`. A run that ends signed out marks `cache` stale.
pub(crate) fn ask_with_gate(
    ask: &Ask<'_>,
    gate: Gate,
    cache: &mut StatusCache,
    cancel: &AtomicBool,
    notify: &mut dyn FnMut(Notice),
) -> Result<String, ErrorKind> {
    let recipe = match run_recipe(ask.host, ask.model) {
        Ok(r) => r,
        Err(e) => {
            log::info!("ai runtime {NAME}: not run ({})", e.kind());
            return Err(ErrorKind::Settings);
        }
    };
    let detector = ClaudeStreamDetector::new(gate);
    let outcome = detector.outcome();
    let spec = RunSpec {
        program: ask.program.to_path_buf(),
        recipe: &recipe,
        instructions: ask.instructions,
        prompt: ask.prompt,
        secrets: ask.secrets,
        host: ask.host,
        sandbox: ask.sandbox,
        limits: ask.limits,
    };
    let report = run(&spec, detector, cancel, notify);
    let result = outcome_to_result(report.end, outcome.get());
    if result == Err(ErrorKind::SignedOut) {
        cache.mark_signed_out();
    }
    result
}

/// Runs Claude Code once ([`RUN_GATE`]). `cache` is locked only after the run, to mark it signed
/// out when the run ended so: never while the run goes on.
pub(crate) fn ask(ask: &Ask<'_>, cache: &Mutex<StatusCache>, cancel: &AtomicBool, notify: &mut dyn FnMut(Notice)) -> Result<String, ErrorKind> {
    let mut scratch = StatusCache::default();
    let result = ask_with_gate(ask, RUN_GATE, &mut scratch, cancel, notify);
    if result == Err(ErrorKind::SignedOut) {
        lock_cache(cache).mark_signed_out();
    }
    result
}
