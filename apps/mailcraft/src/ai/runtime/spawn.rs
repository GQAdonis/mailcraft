// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Spawning a runtime contained (B4, tasks 14.4–14.5, 14.9).
//!
//! One spawner for every CLI runtime. For each run:
//! - the program is checked again ([`super::detect::check_executable`]) and the file it leads to
//!   is what runs;
//! - a fresh 0700 folder holds the empty working folder, the 0600 instructions file, the sandbox
//!   profile, a temporary folder and, when the recipe asks, the run's own `HOME`; it is deleted
//!   after the run whatever its end;
//! - argv comes from the recipe's constants and checked options; it is refused when it holds the
//!   prompt, the instructions, a mail text, or a key or token ([`check_argv`]);
//! - the environment is built from nothing ([`build_env`]);
//! - the prompt goes on stdin, at once or, when the detector asks, only once the runtime's init
//!   event was checked;
//! - the runtime runs in a process group and session of its own (a job object on Windows), so a
//!   cancel, a time-out (120 s, or 30 s without output) or a kill verdict kills it with every
//!   process it started (`process-wrap`, library PG-a);
//! - under the OS sandbox ([`super::sandbox`]); a recipe that may not run without one ends
//!   `no_sandbox` and nothing is spawned;
//! - its stdout is read line by line by its adapter's [`StreamDetector`] through the [`Judge`];
//!   a tool event kills the group, drops the text, ends the run `tool_use_stopped`, and the owner's
//!   notice is reported before [`run`] returns, so it is shown before any fallback starts.
//!
//! The answer is handed back whole when the run ends cleanly: nothing of a run that is killed is
//! ever shown. Only kinds are logged, never what a runtime wrote, the prompt, or a path of the
//! owner's.

use std::io::{Read, Write as _};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use mailcraft_ai::runtime::contain::{BaseEnv, Judge, Limits, LineSplitter, Piece, Step, StreamDetector, build_env, check_argv};
use mailcraft_model::ai::ErrorKind;
use process_wrap::std::{ChildWrapper, CommandWrap};

use super::sandbox::{self, Guard, Policy, Sandbox};

/// How often the run looks at its cancel flag and its clocks.
const POLL: Duration = Duration::from_millis(25);
/// How long the last lines of a runtime that has ended are waited for.
const EXIT_GRACE: Duration = Duration::from_millis(300);
/// How long the end of stderr is waited for once the group is dead.
const PIPE_GRACE: Duration = Duration::from_millis(500);
/// How many pieces of output may wait to be judged.
const QUEUE: usize = 16;
/// How much is read from a pipe at once.
const CHUNK: usize = 64 * 1024;

/// One argument of a recipe's argv.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Arg {
    /// A constant or a checked option (a model's name the owner chose from a list).
    Lit(String),
    /// The path of the run's 0600 instructions file.
    InstructionsFile,
    /// The path of the run's empty working folder.
    #[cfg_attr(not(test), expect(dead_code, reason = "no adapter names its working folder on argv yet"))]
    WorkDir,
}

/// Whose `HOME` a runtime runs with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Home {
    /// The owner's own (Claude Code: its sign-in is found there; MailCraft never reads it).
    Real,
    /// The run's own, inside its run folder.
    PerRun,
}

/// How one runtime is run: what its adapter declares.
#[derive(Clone, Debug)]
pub(crate) struct Recipe {
    /// The runtime's name, as logged.
    pub(crate) name: &'static str,
    /// Whether it may run where the computer has no sandbox (Claude Code, whose tools are removed
    /// before any run) or not (Codex).
    pub(crate) may_run_unsandboxed: bool,
    pub(crate) home: Home,
    pub(crate) args: Vec<Arg>,
    /// The variables it is given beyond the base (Codex: `CODEX_HOME`).
    pub(crate) env: Vec<(String, String)>,
    /// Its own config paths, which the sandbox lets it read.
    pub(crate) read_paths: Vec<PathBuf>,
    /// The paths the sandbox lets it write besides its run folder.
    pub(crate) write_paths: Vec<PathBuf>,
    /// What it may never write, even beneath its write paths: its executable configuration.
    pub(crate) deny_write: Vec<Guard>,
}

/// What the host knows of its own environment for a run. Nothing else of MailCraft's environment
/// reaches a runtime.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct HostEnv {
    /// The login-shell PATH found by detection.
    pub(crate) path: Option<String>,
    /// The owner's home, for a recipe that runs with it.
    pub(crate) home: Option<String>,
    pub(crate) user: Option<String>,
    pub(crate) lang: Option<String>,
}

impl HostEnv {
    /// The owner's home, user name and language from MailCraft's own environment, and `path`.
    pub(crate) fn from_process(path: Option<String>) -> HostEnv {
        let var = |n: &str| std::env::var(n).ok().filter(|v| !v.is_empty());
        HostEnv { path, home: var("HOME"), user: var("USER").or_else(|| var("LOGNAME")).or_else(|| var("USERNAME")), lang: var("LANG") }
    }
}

/// One run.
pub(crate) struct RunSpec<'a> {
    /// The program as detection found it (checked again here).
    pub(crate) program: PathBuf,
    pub(crate) recipe: &'a Recipe,
    /// The instructions, written to the 0600 file.
    pub(crate) instructions: &'a str,
    /// The prompt, written on stdin.
    pub(crate) prompt: &'a str,
    /// Everything else no argv may hold: the mail texts that went into the prompt, every key and
    /// token MailCraft holds.
    pub(crate) secrets: &'a [String],
    pub(crate) host: &'a HostEnv,
    pub(crate) sandbox: &'a Sandbox,
    pub(crate) limits: Limits,
}

/// What the owner is told while the run ends, before [`run`] returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Notice {
    /// "The runtime tried to use a tool; MailCraft stopped it."
    ToolUseStopped,
}

/// How a run ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RunEnd {
    /// The whole answer.
    Answer(String),
    /// A tool event (its kind) stopped the run; its text was dropped.
    Stopped { kind: &'static str },
    /// The run took too long or was silent too long. Nothing of it was shown.
    TimedOut,
    /// The run was called off.
    Cancelled,
    /// The recipe needs a sandbox and this computer has none; nothing was spawned.
    NoSandbox,
    /// The spawn was refused before anything ran (argv, environment, sandbox paths).
    Refused { kind: &'static str },
    /// The program was refused, didn't start, or didn't behave as its kind does.
    Broken { kind: &'static str },
    /// It wrote more than is taken.
    TooLarge,
}

impl RunEnd {
    /// The error the chain reports, `None` for an answer.
    pub(crate) fn error_kind(&self) -> Option<ErrorKind> {
        Some(match self {
            RunEnd::Answer(_) => return None,
            RunEnd::Stopped { .. } => ErrorKind::ToolUseStopped,
            // Nothing of a run is shown before it ends, so nothing was shown.
            RunEnd::TimedOut => ErrorKind::TimedOut { before_first_byte: true },
            RunEnd::Cancelled => ErrorKind::Cancelled,
            RunEnd::NoSandbox => ErrorKind::NoSandbox,
            RunEnd::Refused { .. } => ErrorKind::Settings,
            RunEnd::Broken { .. } => ErrorKind::BrokenRuntime,
            RunEnd::TooLarge => ErrorKind::TooLarge,
        })
    }
}

/// A run's end and what its stderr said (capped; never logged, for the adapter to read a "not
/// signed in").
#[derive(Debug)]
pub(crate) struct RunReport {
    pub(crate) end: RunEnd,
    pub(crate) stderr: Vec<u8>,
}

impl RunReport {
    fn bare(end: RunEnd) -> RunReport {
        RunReport { end, stderr: Vec::new() }
    }
}

/// The run's 0700 folder. Deleted when it is dropped, and explicitly at the end of every run.
struct RunDir {
    dir: tempfile::TempDir,
    root: PathBuf,
    work: PathBuf,
    tmp: PathBuf,
    home: Option<PathBuf>,
    instructions: PathBuf,
}

/// The instructions file's name in the run folder (never in the working folder, which stays empty).
pub(crate) const INSTRUCTIONS_FILE: &str = "instructions.txt";

fn private_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir(path)?;
    restrict_dir(path)
}

fn restrict_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

impl RunDir {
    fn create(instructions: &str, per_run_home: bool) -> std::io::Result<RunDir> {
        let dir = tempfile::Builder::new().prefix("mailcraft-run-").tempdir()?;
        restrict_dir(dir.path())?;
        let root = std::fs::canonicalize(dir.path())?;
        let work = root.join("work");
        let tmp = root.join("tmp");
        private_dir(&work)?;
        private_dir(&tmp)?;
        let home = if per_run_home {
            let h = root.join("home");
            private_dir(&h)?;
            Some(h)
        } else {
            None
        };
        let instructions_path = root.join(INSTRUCTIONS_FILE);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        options.open(&instructions_path)?.write_all(instructions.as_bytes())?;
        Ok(RunDir { dir, root, work, tmp, home, instructions: instructions_path })
    }

    fn close(self) {
        if self.dir.close().is_err() {
            log::warn!("ai runtime: a run folder couldn't be removed");
        }
    }
}

/// The runtime's process group (session, job). Killed, with every process in it, when dropped.
struct Group {
    child: Box<dyn ChildWrapper>,
    /// The group was killed and the runtime reaped: signalling it again could reach a group that
    /// reused the number, so it isn't.
    killed: bool,
}

impl Group {
    fn spawn(program: &Path, args: &[String], env: &[(String, String)], work: &Path) -> std::io::Result<Group> {
        let mut command = CommandWrap::with_new(program, |c| {
            c.args(args)
                .env_clear()
                .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
                .current_dir(work)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
        });
        #[cfg(unix)]
        command.wrap(process_wrap::std::ProcessSession);
        #[cfg(windows)]
        command.wrap(process_wrap::std::JobObject);
        Ok(Group { child: command.spawn()?, killed: false })
    }

    /// Kills the whole group and reaps the runtime. Harmless when everything has ended.
    fn kill(&mut self) {
        if self.killed {
            return;
        }
        self.killed = true;
        let _ = self.child.start_kill();
        let _ = self.child.wait();
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        self.kill();
    }
}

enum Out {
    Piece(Piece),
    TooMuch,
    End,
}

fn read_stdout(mut pipe: impl Read, cap: usize, line_cap: usize, tx: mpsc::SyncSender<Out>) {
    let mut splitter = LineSplitter::new(line_cap);
    let mut buf = vec![0u8; CHUNK];
    let mut total = 0usize;
    loop {
        match pipe.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                total = total.saturating_add(n);
                if total > cap {
                    let _ = tx.send(Out::TooMuch);
                    return;
                }
                for piece in splitter.push(buf.get(..n).unwrap_or(&[])) {
                    if tx.send(Out::Piece(piece)).is_err() {
                        return;
                    }
                }
            }
        }
    }
    if let Some(piece) = splitter.finish() {
        let _ = tx.send(Out::Piece(piece));
    }
    let _ = tx.send(Out::End);
}

fn read_stderr(mut pipe: impl Read, cap: usize) -> Vec<u8> {
    let mut kept = Vec::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        match pipe.read(&mut buf) {
            Ok(0) | Err(_) => return kept,
            Ok(n) => {
                let room = cap.saturating_sub(kept.len());
                kept.extend_from_slice(buf.get(..n.min(room)).unwrap_or(&[]));
            }
        }
    }
}

/// Runs one runtime contained, with `detector` reading its stream. `cancel` calls the run off;
/// `notify` is told of a tool stop before this returns.
pub(crate) fn run<D: StreamDetector>(spec: &RunSpec<'_>, detector: D, cancel: &AtomicBool, notify: &mut dyn FnMut(Notice)) -> RunReport {
    let recipe = spec.recipe;
    // 14.4: detection's checks again, at every spawn.
    let real_program = match super::detect::check_executable(&spec.program) {
        Ok(p) => p,
        Err(_) => {
            log::info!("ai runtime {}: the program was refused", recipe.name);
            return RunReport::bare(RunEnd::Broken { kind: "program_refused" });
        }
    };
    // 14.9: a recipe that needs a sandbox doesn't run without one.
    if *spec.sandbox == Sandbox::None && !recipe.may_run_unsandboxed {
        log::info!("ai runtime {}: no sandbox on this computer, not run", recipe.name);
        return RunReport::bare(RunEnd::NoSandbox);
    }
    let dir = match RunDir::create(spec.instructions, recipe.home == Home::PerRun) {
        Ok(d) => d,
        Err(_) => return RunReport::bare(RunEnd::Broken { kind: "run_folder" }),
    };
    let report = run_in(spec, &real_program, &dir, detector, cancel, notify);
    dir.close();
    report
}

fn run_in<D: StreamDetector>(
    spec: &RunSpec<'_>,
    real_program: &Path,
    dir: &RunDir,
    detector: D,
    cancel: &AtomicBool,
    notify: &mut dyn FnMut(Notice),
) -> RunReport {
    let recipe = spec.recipe;
    let limits = spec.limits.clamped();
    let refused = |kind: &'static str| {
        log::info!("ai runtime {}: spawn refused ({kind})", recipe.name);
        RunReport::bare(RunEnd::Refused { kind })
    };
    let text = |p: &Path| p.to_str().map(str::to_owned);
    let home = match recipe.home {
        Home::Real => spec.host.home.clone(),
        Home::PerRun => dir.home.as_deref().and_then(text),
    };
    let base = BaseEnv { path: spec.host.path.clone(), home, user: spec.host.user.clone(), lang: spec.host.lang.clone(), tmpdir: text(&dir.tmp) };
    let env = match build_env(&base, &recipe.env) {
        Ok(e) => e,
        Err(e) => return refused(e.kind()),
    };
    let mut args = Vec::with_capacity(recipe.args.len());
    for a in &recipe.args {
        let s = match a {
            Arg::Lit(s) => Some(s.clone()),
            Arg::InstructionsFile => text(&dir.instructions),
            Arg::WorkDir => text(&dir.work),
        };
        match s {
            Some(s) => args.push(s),
            None => return refused("path_not_text"),
        }
    }
    let mut write_paths = sandbox::system_write_paths();
    write_paths.extend(recipe.write_paths.iter().cloned());
    let policy = match Policy::for_run(
        &sandbox::system_read_paths(),
        &spec.program,
        real_program,
        &recipe.read_paths,
        &dir.root,
        &write_paths,
        &recipe.deny_write,
    ) {
        Ok(p) => p,
        Err(e) => return refused(e.kind()),
    };
    let (program, args) = match sandbox::wrap(spec.sandbox, &policy, &dir.root, &dir.work, real_program, args) {
        Ok(w) => w,
        Err(e) => return refused(e.kind()),
    };
    // 14.14: nothing of the prompt, the instructions, the mail or a secret on argv, sandbox
    // wrapper included.
    let mut argv = Vec::with_capacity(args.len() + 1);
    argv.push(program.to_string_lossy().into_owned());
    argv.extend(args.iter().cloned());
    let mut needles: Vec<&str> = vec![spec.prompt, spec.instructions];
    needles.extend(spec.secrets.iter().map(String::as_str));
    if let Err(e) = check_argv(&argv, &needles) {
        return refused(e.kind());
    }
    let mut group = match Group::spawn(&program, &args, &env, &dir.work) {
        Ok(g) => g,
        Err(_) => {
            log::info!("ai runtime {}: didn't start", recipe.name);
            return RunReport::bare(RunEnd::Broken { kind: "spawn" });
        }
    };
    log::info!("ai runtime {}: started under {}", recipe.name, spec.sandbox.kind());
    let (stderr_tx, stderr_rx) = mpsc::channel();
    if let Some(pipe) = group.child.stderr().take() {
        let cap = limits.stderr_bytes;
        std::thread::spawn(move || {
            let _ = stderr_tx.send(read_stderr(pipe, cap));
        });
    }
    let end = supervise(spec, &mut group, detector, &limits, cancel, notify);
    // Every end kills the whole group: a runtime that answered and left a process behind loses it.
    group.kill();
    drop(group);
    // Never waited for long: a process that escaped the group could hold the pipe open.
    let stderr = stderr_rx.recv_timeout(PIPE_GRACE).unwrap_or_default();
    RunReport { end, stderr }
}

/// Writes the prompt on a thread of its own (a large prompt never blocks the reading of the
/// stream), then closes stdin.
fn write_prompt(group: &mut Group, prompt: &str) -> Option<std::thread::JoinHandle<()>> {
    let mut stdin = group.child.stdin().take()?;
    let bytes = prompt.as_bytes().to_vec();
    Some(std::thread::spawn(move || {
        let _ = stdin.write_all(&bytes);
        let _ = stdin.flush();
    }))
}

fn stopped(recipe: &Recipe, group: &mut Group, kind: &'static str, notify: &mut dyn FnMut(Notice)) -> RunEnd {
    // 14.5: the group dies first, the text is already dropped by the judge, the owner is told,
    // and only then does the run return (so the notice comes before any fallback).
    group.kill();
    log::warn!("ai runtime {}: stopped on a {kind} event", recipe.name);
    notify(Notice::ToolUseStopped);
    RunEnd::Stopped { kind }
}

fn supervise<D: StreamDetector>(
    spec: &RunSpec<'_>,
    group: &mut Group,
    detector: D,
    limits: &Limits,
    cancel: &AtomicBool,
    notify: &mut dyn FnMut(Notice),
) -> RunEnd {
    let recipe = spec.recipe;
    let mut judge = Judge::new(detector);
    let Some(stdout) = group.child.stdout().take() else {
        return RunEnd::Broken { kind: "no_stdout" };
    };
    let (tx, rx) = mpsc::sync_channel(QUEUE);
    let (cap, line_cap) = (limits.stdout_bytes, limits.line_bytes);
    let reader = std::thread::spawn(move || read_stdout(stdout, cap, line_cap, tx));
    let mut writer = if judge.prompt_at_start() { write_prompt(group, spec.prompt) } else { None };
    let start = Instant::now();
    let mut last_output = start;
    let mut exited_at: Option<Instant> = None;
    let end = loop {
        if cancel.load(Ordering::Relaxed) {
            group.kill();
            break RunEnd::Cancelled;
        }
        let now = Instant::now();
        if now.duration_since(start) >= limits.wall_clock || now.duration_since(last_output) >= limits.idle {
            group.kill();
            log::info!("ai runtime {}: timed out", recipe.name);
            break RunEnd::TimedOut;
        }
        match rx.recv_timeout(POLL) {
            Ok(Out::Piece(piece)) => {
                last_output = Instant::now();
                match judge.piece(&piece) {
                    Step::Continue => {}
                    Step::WritePrompt => {
                        if writer.is_none() {
                            writer = write_prompt(group, spec.prompt);
                        }
                    }
                    Step::Done(text) => break RunEnd::Answer(text),
                    Step::Kill(kind) => break stopped(recipe, group, kind, notify),
                    Step::TooLarge => {
                        group.kill();
                        break RunEnd::TooLarge;
                    }
                }
            }
            Ok(Out::TooMuch) => {
                group.kill();
                break RunEnd::TooLarge;
            }
            Ok(Out::End) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                break finish(spec, group, &judge, start, limits, cancel);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // A process the runtime left behind may hold stdout open after the runtime has
                // ended: a short grace for the last lines, then the exit decides.
                match exited_at {
                    None => {
                        if matches!(group.child.try_wait(), Ok(Some(_))) {
                            exited_at = Some(Instant::now());
                        }
                    }
                    Some(at) if at.elapsed() >= EXIT_GRACE => break finish(spec, group, &judge, start, limits, cancel),
                    Some(_) => {}
                }
            }
        }
    };
    group.kill();
    // The reader and the writer end with their pipes; they are not waited for, since a process
    // that escaped the group could keep a pipe open and the run must still return.
    drop(rx);
    drop(reader);
    drop(writer);
    end
}

/// The stream ended without a final answer: the runtime's exit decides.
fn finish<D: StreamDetector>(
    spec: &RunSpec<'_>,
    group: &mut Group,
    judge: &Judge<D>,
    start: Instant,
    limits: &Limits,
    cancel: &AtomicBool,
) -> RunEnd {
    let status = loop {
        match group.child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if cancel.load(Ordering::Relaxed) {
                    return RunEnd::Cancelled;
                }
                if start.elapsed() >= limits.wall_clock {
                    return RunEnd::TimedOut;
                }
                std::thread::sleep(POLL);
            }
            Err(_) => break None,
        }
    };
    let ok = status.is_some_and(|s| s.success());
    if ok && !judge.text().is_empty() {
        return RunEnd::Answer(judge.text().to_owned());
    }
    log::info!("ai runtime {}: ended without an answer", spec.recipe.name);
    RunEnd::Broken { kind: if ok { "no_answer" } else { "exit_status" } }
}

#[cfg(test)]
#[path = "spawn_test_support.rs"]
pub(crate) mod test_support;
