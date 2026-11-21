// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The managed UAR sidecar: spawn, handshake, shutdown (B9).
//!
//! **Managed instance** ([`Sidecar::start`]), offered only when the installed program passes the
//! entry check ([`managed_support`]; `universal-agent-runtime` 1.4.0-rc.5 does not, so for it UAR is
//! external-only):
//! 1. the program is checked again ([`super::detect::check_executable`]);
//! 2. a fresh 0700 folder holds the empty working folder, a temporary folder, the instance's own
//!    `HOME` and the 0600 config MailCraft wrote ([`ManagedConfig`]: tools off, session search off,
//!    exactly one model endpoint, no key);
//! 3. argv is the config's path, `--port 0` and the three native tools off, refused when it holds
//!    the launch token or any secret ([`check_argv`]); the environment is B4's allow-list alone
//!    ([`build_env`]); it runs under the OS sandbox and in a process group of its own;
//! 4. a new 32-byte launch token goes on stdin, which stays open; it lives in memory only
//!    ([`LaunchToken`]);
//! 5. the first line on stdout must be `READY:<port>` within 20 s; anything else, or nothing,
//!    kills the group and refuses the instance before any request;
//! 6. the capabilities at `127.0.0.1:<port>` must offer AG-UI `uar.agui/1`; the model list is
//!    read so that a model beyond the configured one makes the instance leave;
//! 7. every request carries `Authorization: Bearer <token>`;
//! 8. a tool event in a run's AG-UI stream kills the whole group, drops the text, and tells the
//!    owner before the run returns;
//! 9. shutdown closes stdin, waits a grace period, then kills the group; each start has a new
//!    generation and a reply from an earlier one is rejected ([`UarSlot::accept`]).
//!
//! **External instance** ([`discover`], [`run_external`]): one already listening on
//! `127.0.0.1:1906`, used only once the owner chose it, always `leaves`; its tools can't be turned
//! off (`EXTERNAL_CAVEAT`). MailCraft never reads UAR's credential or config files, so it sends no
//! bearer; a 401 or 403 is `uar_refused` ("UAR refused MailCraft").
//!
//! All HTTP goes through `mailcraft_net` (plain HTTP only to loopback, checked on the address
//! connected to, no redirect). Only kinds are logged: never a token, a port, a path, the prompt or
//! anything an instance wrote.
//!
//! The long-lived sidecar can't go through `spawn::run`, which owns a one-shot run's whole life;
//! it is built from the same pieces (the executable check, the run folder's rules, the env
//! allow-list, the argv check, the sandbox policy and wrapper, a process session or job object).

#![cfg_attr(
    not(test),
    expect(dead_code, reason = "the managed sidecar: this version runs only an external UAR the owner started (W7), never one of its own")
)]

use std::io::{Read, Write as _};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use mailcraft_ai::runtime::contain::{BaseEnv, Judge, Limits, MAX_STDOUT_BYTES, Step, build_env, check_argv};
use mailcraft_ai::runtime::uar::{
    AgUiDetector, CAPABILITIES_PATH, Card, MAX_READY_LINE, MODELS_PATH, ManagedConfig, ManagedSupport, READY_TIMEOUT, RUNS_PATH, TOKEN_BYTES,
    capabilities_ok, models_beyond, parse_card, parse_ready, run_body, token_text,
};
use mailcraft_model::ai::runtime::{ManagedUar, ProviderId, UarEndpoint};
use mailcraft_net::https::{self, Deadlines, Flow, Piece, StreamRequest};
use mailcraft_net::sse;
use process_wrap::std::{ChildWrapper, CommandWrap};
use zeroize::Zeroizing;

use super::detect::{UAR_CAP, UAR_TIMEOUT};
use super::sandbox::{self, Policy, Sandbox};
use super::spawn::{HostEnv, Notice, RunEnd};

/// How long a managed instance has to end once its stdin is closed, before its group is killed.
pub(crate) const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);
/// How long a request to a managed instance's capabilities or model list may take.
pub(crate) const CHECK_TIMEOUT: Duration = Duration::from_secs(2);
/// The most bytes of a capabilities answer or a model list read.
pub(crate) const CHECK_CAP: u64 = 1024 * 1024;
/// How long `--help` may take, and how much of it is read.
const HELP_TIMEOUT: Duration = Duration::from_secs(5);
const HELP_CAP: usize = 64 * 1024;
/// The config's file name in the instance's folder.
pub(crate) const CONFIG_FILE: &str = "uar-config.yaml";
/// How often a shutdown looks whether the instance has ended.
const POLL: Duration = Duration::from_millis(25);
/// How much is read from a pipe at once.
const CHUNK: usize = 16 * 1024;

/// The generation of the last start, process-wide.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Why a managed instance was refused. Every refusal after the spawn has killed its group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SidecarError {
    /// The program failed the executable check.
    ProgramRefused,
    /// This computer has no sandbox and the instance may not run without one.
    NoSandbox,
    /// The instance's folder or config couldn't be written.
    Folder,
    /// The operating system gave no random bytes for the launch token.
    Random,
    /// The spawn was refused before anything ran (argv, environment, sandbox paths).
    Refused(&'static str),
    /// It didn't start.
    Spawn,
    /// It printed nothing within the readiness time.
    NoReadiness,
    /// Its first line wasn't `READY:<port>`, or it ended first.
    BadReadiness,
    /// Its capabilities couldn't be read, or don't offer `uar.agui/1`.
    Capabilities,
}

impl SidecarError {
    /// The error's kind, the only thing of it that is logged.
    pub(crate) fn kind(self) -> &'static str {
        match self {
            SidecarError::ProgramRefused => "program_refused",
            SidecarError::NoSandbox => "no_sandbox",
            SidecarError::Folder => "run_folder",
            SidecarError::Random => "no_random",
            SidecarError::Refused(kind) => kind,
            SidecarError::Spawn => "spawn",
            SidecarError::NoReadiness => "no_readiness",
            SidecarError::BadReadiness => "bad_readiness",
            SidecarError::Capabilities => "capabilities",
        }
    }
}

/// The launch token: in memory only, overwritten when it is let go of, never in `Debug`.
pub(crate) struct LaunchToken(Zeroizing<String>);

impl LaunchToken {
    /// A new token from the operating system's random source.
    fn new() -> Result<LaunchToken, SidecarError> {
        let mut bytes = Zeroizing::new([0u8; TOKEN_BYTES]);
        ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut *bytes).map_err(|_| SidecarError::Random)?;
        Ok(LaunchToken(Zeroizing::new(token_text(&bytes))))
    }

    fn as_str(&self) -> &str {
        self.0.as_str()
    }

    fn bearer(&self) -> Zeroizing<String> {
        Zeroizing::new(format!("Bearer {}", self.as_str()))
    }
}

impl std::fmt::Debug for LaunchToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LaunchToken(<redacted>)")
    }
}

/// What the installed program's `--help` (no prompt, nothing served, its config untouched) says
/// it can do. A program that can't be run or prints nothing can do nothing.
pub(crate) fn managed_support(program: &Path) -> ManagedSupport {
    let Ok(real) = super::detect::check_executable(program) else {
        return ManagedSupport::default();
    };
    let mut command = std::process::Command::new(real);
    command.arg("--help").env_clear();
    match super::detect::run_capped(command, HELP_CAP, HELP_TIMEOUT) {
        Ok(ran) => ManagedSupport::from_help(&String::from_utf8_lossy(&ran.out)),
        Err(_) => ManagedSupport::default(),
    }
}

/// One managed start.
pub(crate) struct SidecarSpec<'a> {
    /// The program as detection found it (checked again here).
    pub(crate) program: PathBuf,
    /// The one model endpoint MailCraft chose.
    pub(crate) config: &'a ManagedConfig,
    /// Whether MailCraft itself verified that endpoint's resolved peer is a loopback address (D7).
    pub(crate) peer_verified_loopback: bool,
    pub(crate) host: &'a HostEnv,
    pub(crate) sandbox: &'a Sandbox,
    /// UAR (native tools off) may run where the computer has no sandbox (spec §7.1).
    pub(crate) may_run_unsandboxed: bool,
    /// How long the readiness line is waited for ([`READY_TIMEOUT`]; tests shorten it).
    pub(crate) readiness: Duration,
    /// Every key and token MailCraft holds: none may be on argv.
    pub(crate) secrets: &'a [String],
}

impl SidecarSpec<'_> {
    /// The readiness time, never longer than [`READY_TIMEOUT`].
    fn readiness(&self) -> Duration {
        self.readiness.clamp(Duration::from_millis(50), READY_TIMEOUT)
    }
}

/// The instance's 0700 folder. Deleted when it is dropped.
struct SidecarDir {
    _dir: tempfile::TempDir,
    root: PathBuf,
    work: PathBuf,
    tmp: PathBuf,
    home: PathBuf,
    config: PathBuf,
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

fn private_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir(path)?;
    restrict_dir(path)
}

impl SidecarDir {
    fn create(config: &[u8]) -> std::io::Result<SidecarDir> {
        let dir = tempfile::Builder::new().prefix("mailcraft-uar-").tempdir()?;
        restrict_dir(dir.path())?;
        let root = std::fs::canonicalize(dir.path())?;
        let (work, tmp, home) = (root.join("work"), root.join("tmp"), root.join("home"));
        for d in [&work, &tmp, &home] {
            private_dir(d)?;
        }
        let config_path = root.join(CONFIG_FILE);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        options.open(&config_path)?.write_all(config)?;
        Ok(SidecarDir { _dir: dir, root, work, tmp, home, config: config_path })
    }
}

/// The argv a managed instance is started with (after the program).
fn sidecar_args(config: &Path) -> Option<Vec<String>> {
    let config = config.to_str()?.to_owned();
    let mut args = vec!["--config".to_owned(), config, "--port".to_owned(), "0".to_owned()];
    for flag in ["--native-file-tools", "--native-web-fetch", "--native-terminal-exec"] {
        args.push(flag.to_owned());
        args.push("false".to_owned());
    }
    Some(args)
}

/// A reply from a managed instance, tagged with the generation that made it.
#[derive(Debug)]
pub(crate) struct Reply {
    pub(crate) generation: u64,
    pub(crate) end: RunEnd,
}

/// A running managed instance. Its group is killed when it is dropped.
pub(crate) struct Sidecar {
    child: Box<dyn ChildWrapper>,
    killed: bool,
    stdin: Option<ChildStdin>,
    port: u16,
    token: LaunchToken,
    generation: u64,
    runs: u64,
    fact: ManagedUar,
    _dir: SidecarDir,
}

impl std::fmt::Debug for Sidecar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sidecar").field("generation", &self.generation).field("killed", &self.killed).finish_non_exhaustive()
    }
}

fn spawn_group(program: &Path, args: &[String], env: &[(String, String)], work: &Path) -> std::io::Result<Box<dyn ChildWrapper>> {
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
    command.spawn()
}

/// Reads the first line (at most [`MAX_READY_LINE`] bytes), hands it over, then reads the rest of
/// the stream and drops it, so the instance never blocks on a full pipe. Nothing is logged.
fn read_ready(mut pipe: impl Read, tx: mpsc::SyncSender<Option<Vec<u8>>>) {
    let mut line = Vec::new();
    let mut buf = vec![0u8; CHUNK];
    let mut sent = false;
    loop {
        match pipe.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if sent {
                    continue;
                }
                let piece = buf.get(..n).unwrap_or(&[]);
                match piece.iter().position(|b| *b == b'\n') {
                    Some(pos) => {
                        line.extend_from_slice(piece.get(..pos).unwrap_or(&[]));
                        let ok = line.len() <= MAX_READY_LINE;
                        let _ = tx.send(ok.then(|| std::mem::take(&mut line)));
                        sent = true;
                    }
                    None => {
                        line.extend_from_slice(piece);
                        if line.len() > MAX_READY_LINE {
                            let _ = tx.send(None);
                            sent = true;
                        }
                    }
                }
            }
        }
    }
    if !sent {
        let _ = tx.send(None);
    }
}

fn drain(mut pipe: impl Read) {
    let mut buf = vec![0u8; CHUNK];
    while matches!(pipe.read(&mut buf), Ok(n) if n > 0) {}
}

/// A `GET` to a loopback instance: its status and at most `cap` bytes of its body, or `None` when
/// it didn't answer in time or answered more.
fn get(url: &str, bearer: Option<&str>, timeout: Duration, cap: u64) -> Option<(u16, Vec<u8>)> {
    let mut fields: Vec<(&str, &str)> = vec![("Accept", "application/json")];
    if let Some(b) = bearer {
        fields.push(("Authorization", b));
    }
    let request = StreamRequest {
        method: "GET",
        url,
        fields: &fields,
        body: &[],
        trusted: "",
        deadlines: Deadlines { connect: timeout, first_byte: timeout, idle: timeout, total: timeout },
        max_response: cap,
        max_error_response: 0,
        cancel: None,
    };
    let mut body = Vec::new();
    let mut over = false;
    let outcome = https::stream(&request, &mut |piece| match piece {
        Piece::Head(_) => Flow::Go,
        Piece::Body(bytes) => {
            if (body.len() as u64).saturating_add(bytes.len() as u64) > cap {
                over = true;
                Flow::Stop
            } else {
                body.extend_from_slice(bytes);
                Flow::Go
            }
        }
    })
    .ok()?;
    if over || outcome.stopped || outcome.truncated {
        return None;
    }
    Some((outcome.head.status, body))
}

impl Sidecar {
    /// Starts a managed instance and checks it, or refuses it with its group dead.
    pub(crate) fn start(spec: &SidecarSpec<'_>) -> Result<Sidecar, SidecarError> {
        let real_program = super::detect::check_executable(&spec.program).map_err(|_| SidecarError::ProgramRefused)?;
        if *spec.sandbox == Sandbox::None && !spec.may_run_unsandboxed {
            return Err(SidecarError::NoSandbox);
        }
        let config = spec.config.to_bytes().map_err(|e| SidecarError::Refused(e.kind()))?;
        let dir = SidecarDir::create(&config).map_err(|_| SidecarError::Folder)?;
        let token = LaunchToken::new()?;
        let text = |p: &Path| p.to_str().map(str::to_owned);
        let base = BaseEnv {
            path: spec.host.path.clone(),
            home: text(&dir.home),
            user: spec.host.user.clone(),
            lang: spec.host.lang.clone(),
            tmpdir: text(&dir.tmp),
        };
        let env = build_env(&base, &[]).map_err(|e| SidecarError::Refused(e.kind()))?;
        let args = sidecar_args(&dir.config).ok_or(SidecarError::Refused("path_not_text"))?;
        let mut write_paths = sandbox::system_write_paths();
        write_paths.push(dir.root.clone());
        let policy = Policy::for_run(&sandbox::system_read_paths(), &spec.program, &real_program, &[], &dir.root, &write_paths, &[])
            .map_err(|e| SidecarError::Refused(e.kind()))?;
        let (program, args) =
            sandbox::wrap(spec.sandbox, &policy, &dir.root, &dir.work, &real_program, args).map_err(|e| SidecarError::Refused(e.kind()))?;
        let mut argv = Vec::with_capacity(args.len() + 1);
        argv.push(program.to_string_lossy().into_owned());
        argv.extend(args.iter().cloned());
        let mut needles: Vec<&str> = vec![token.as_str()];
        needles.extend(spec.secrets.iter().map(String::as_str));
        check_argv(&argv, &needles).map_err(|e| SidecarError::Refused(e.kind()))?;
        let child = spawn_group(&program, &args, &env, &dir.work).map_err(|_| SidecarError::Spawn)?;
        let generation = GENERATION.fetch_add(1, Ordering::Relaxed).saturating_add(1);
        let mut sidecar = Sidecar { child, killed: false, stdin: None, port: 0, token, generation, runs: 0, fact: ManagedUar::default(), _dir: dir };
        match sidecar.handshake(spec) {
            Ok(()) => {
                log::info!("ai runtime uar: managed instance ready under {}", spec.sandbox.kind());
                Ok(sidecar)
            }
            Err(e) => {
                sidecar.kill();
                log::info!("ai runtime uar: managed instance refused ({})", e.kind());
                Err(e)
            }
        }
    }

    fn handshake(&mut self, spec: &SidecarSpec<'_>) -> Result<(), SidecarError> {
        if let Some(pipe) = self.child.stderr().take() {
            std::thread::spawn(move || drain(pipe));
        }
        let stdout = self.child.stdout().take().ok_or(SidecarError::BadReadiness)?;
        let (tx, rx) = mpsc::sync_channel(1);
        std::thread::spawn(move || read_ready(stdout, tx));
        // The token, then a line end; stdin stays open (closing it is the shutdown).
        let mut stdin = self.child.stdin().take().ok_or(SidecarError::Spawn)?;
        let line = Zeroizing::new(format!("{}\n", self.token.as_str()));
        if stdin.write_all(line.as_bytes()).and_then(|()| stdin.flush()).is_err() {
            return Err(SidecarError::BadReadiness);
        }
        self.stdin = Some(stdin);
        let port = match rx.recv_timeout(spec.readiness()) {
            Ok(Some(line)) => parse_ready(&line).ok_or(SidecarError::BadReadiness)?,
            Ok(None) | Err(mpsc::RecvTimeoutError::Disconnected) => return Err(SidecarError::BadReadiness),
            Err(mpsc::RecvTimeoutError::Timeout) => return Err(SidecarError::NoReadiness),
        };
        self.port = port;
        let bearer = self.token.bearer();
        match get(&self.url(CAPABILITIES_PATH), Some(&bearer), CHECK_TIMEOUT, CHECK_CAP) {
            Some((200, body)) if capabilities_ok(&body) => {}
            _ => return Err(SidecarError::Capabilities),
        }
        let reported_beyond = match get(&self.url(MODELS_PATH), Some(&bearer), CHECK_TIMEOUT, CHECK_CAP) {
            Some((200, body)) => models_beyond(&body, &spec.config.model),
            _ => true,
        };
        let endpoints = ProviderId::new(&spec.config.provider)
            .map(|provider| vec![UarEndpoint { provider, peer_verified_loopback: spec.peer_verified_loopback }])
            .unwrap_or_default();
        self.fact = ManagedUar { endpoints, reported_beyond };
        Ok(())
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    /// This start's generation.
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// The managed-config fact `Where` is derived from (B1's `where_of`).
    pub(crate) fn fact(&self) -> &ManagedUar {
        &self.fact
    }

    /// Whether the instance is still in use (not killed, not shut down).
    pub(crate) fn alive(&self) -> bool {
        !self.killed
    }

    /// Kills the whole group and reaps the instance. Harmless when it has ended.
    fn kill(&mut self) {
        if self.killed {
            return;
        }
        self.killed = true;
        self.stdin = None;
        let _ = self.child.start_kill();
        let _ = self.child.wait();
    }

    /// One run: the prompt as an AG-UI run, its stream read through B4's judge. A tool event kills
    /// the group before the owner is told and before this returns.
    pub(crate) fn run(&mut self, instructions: &str, prompt: &str, limits: Limits, cancel: &AtomicBool, notify: &mut dyn FnMut(Notice)) -> Reply {
        let generation = self.generation;
        if self.killed {
            return Reply { generation, end: RunEnd::Broken { kind: "sidecar_gone" } };
        }
        self.runs = self.runs.saturating_add(1);
        let run_id = format!("mailcraft-{generation}-{}", self.runs);
        let bearer = self.token.bearer();
        let end = run_agui(&self.url(RUNS_PATH), Some(&bearer), &run_id, instructions, prompt, limits, cancel);
        if let RunEnd::Stopped { kind } = end {
            self.kill();
            log::warn!("ai runtime uar: stopped on a {kind} event");
            notify(Notice::ToolUseStopped);
        }
        Reply { generation, end }
    }

    /// Closes stdin, gives the instance [`SHUTDOWN_GRACE`] to end, then kills its group. Says
    /// whether it ended by itself.
    pub(crate) fn shutdown(mut self) -> bool {
        self.shutdown_in_place(SHUTDOWN_GRACE)
    }

    fn shutdown_in_place(&mut self, grace: Duration) -> bool {
        if self.killed {
            return false;
        }
        self.stdin = None;
        let now = Instant::now();
        let deadline = now.checked_add(grace).unwrap_or(now);
        let ended = loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break true,
                Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL),
                _ => break false,
            }
        };
        // Whatever it left behind goes with its group.
        self.kill();
        ended
    }
}

impl Drop for Sidecar {
    fn drop(&mut self) {
        self.kill();
    }
}

/// The one managed instance the host keeps, and the generation its replies must carry.
#[derive(Debug, Default)]
pub(crate) struct UarSlot {
    sidecar: Option<Sidecar>,
}

impl UarSlot {
    /// Starts a new instance (shutting down the one before), and gives its generation.
    pub(crate) fn start(&mut self, spec: &SidecarSpec<'_>) -> Result<u64, SidecarError> {
        self.shutdown();
        let sidecar = Sidecar::start(spec)?;
        let generation = sidecar.generation();
        self.sidecar = Some(sidecar);
        Ok(generation)
    }

    /// Runs on the current instance.
    pub(crate) fn run(&mut self, instructions: &str, prompt: &str, limits: Limits, cancel: &AtomicBool, notify: &mut dyn FnMut(Notice)) -> Reply {
        match self.sidecar.as_mut() {
            Some(s) => s.run(instructions, prompt, limits, cancel, notify),
            None => Reply { generation: 0, end: RunEnd::Broken { kind: "no_sidecar" } },
        }
    }

    /// Shuts the current instance down. Says whether it ended by itself.
    pub(crate) fn shutdown(&mut self) -> bool {
        self.sidecar.take().is_some_and(Sidecar::shutdown)
    }

    /// The reply's end when it came from the instance running now; `None` for a reply of an
    /// earlier generation, or when none is running.
    pub(crate) fn accept(&self, reply: Reply) -> Option<RunEnd> {
        match &self.sidecar {
            Some(s) if s.alive() && s.generation() == reply.generation && reply.generation != 0 => Some(reply.end),
            _ => {
                log::info!("ai runtime uar: a reply of an earlier instance was dropped");
                None
            }
        }
    }

    /// The managed-config fact of the instance running now.
    pub(crate) fn fact(&self) -> Option<&ManagedUar> {
        self.sidecar.as_ref().filter(|s| s.alive()).map(Sidecar::fact)
    }
}

/// Posts one AG-UI run to `url` and reads its stream through B4's judge. Never notifies: the
/// caller kills what must die first.
fn run_agui(url: &str, bearer: Option<&str>, run_id: &str, instructions: &str, prompt: &str, limits: Limits, cancel: &AtomicBool) -> RunEnd {
    let limits = limits.clamped();
    let Ok(body) = run_body(run_id, instructions, prompt) else {
        return RunEnd::Refused { kind: "uar_encode" };
    };
    let mut fields: Vec<(&str, &str)> = vec![("Content-Type", "application/json"), ("Accept", "text/event-stream")];
    if let Some(b) = bearer {
        fields.push(("Authorization", b));
    }
    let request = StreamRequest {
        method: "POST",
        url,
        fields: &fields,
        body: &body,
        trusted: "",
        deadlines: Deadlines { connect: CHECK_TIMEOUT, first_byte: limits.idle, idle: limits.idle, total: limits.wall_clock },
        max_response: (limits.stdout_bytes as u64).min(MAX_STDOUT_BYTES as u64),
        max_error_response: 0,
        cancel: Some(cancel),
    };
    let sse_limits = sse::Limits {
        max_line: limits.line_bytes,
        max_event: limits.line_bytes,
        max_total: limits.stdout_bytes as u64,
        max_events: sse::Limits::default().max_events,
    };
    let mut parser = sse::Parser::new(sse_limits);
    let mut judge = Judge::new(AgUiDetector::new());
    let mut status = 0u16;
    let mut end: Option<RunEnd> = None;
    let mut events = Vec::new();
    let outcome = https::stream(&request, &mut |piece| match piece {
        Piece::Head(head) => {
            status = head.status;
            if head.status == 200 { Flow::Go } else { Flow::Stop }
        }
        Piece::Body(bytes) => {
            if parser.feed(bytes, &mut events).is_err() {
                end = Some(RunEnd::TooLarge);
                return Flow::Stop;
            }
            match judge_events(&mut events, &mut judge) {
                Some(e) => {
                    end = Some(e);
                    Flow::Stop
                }
                None => Flow::Go,
            }
        }
    });
    if let Some(e) = end {
        return e;
    }
    match outcome {
        Err(e) if https::is_stopped(&e) => return RunEnd::Cancelled,
        Err(e) if https::is_timeout(&e) => return RunEnd::TimedOut,
        Err(_) => return RunEnd::Broken { kind: "connection" },
        Ok(_) => {}
    }
    match status {
        200 => {}
        401 | 403 => return RunEnd::Broken { kind: "uar_refused" },
        _ => return RunEnd::Broken { kind: "http_status" },
    }
    if parser.finish(&mut events).is_err() {
        return RunEnd::TooLarge;
    }
    // The stream ended: only a `RUN_FINISHED` with text is an answer.
    judge_events(&mut events, &mut judge).unwrap_or(RunEnd::Broken { kind: "no_answer" })
}

/// Hands each event's data to the judge: the run's end, once there is one.
fn judge_events(events: &mut Vec<sse::Event>, judge: &mut Judge<AgUiDetector>) -> Option<RunEnd> {
    for event in events.drain(..) {
        match judge.line(event.data.as_bytes()) {
            Step::Continue | Step::WritePrompt => {}
            Step::Done(text) => return Some(RunEnd::Answer(text)),
            Step::Kill(kind) => return Some(RunEnd::Stopped { kind }),
            Step::TooLarge => return Some(RunEnd::TooLarge),
        }
    }
    None
}

/// What the discovery probe found at an external instance's address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Discovery {
    /// Nothing answered.
    Absent,
    /// An instance answered with an agent card: its name and version, untrusted text. It says
    /// nothing about where text goes.
    Found(Card),
    /// Something answered, but not with an agent card within the cap.
    Broken,
}

/// Probes an external instance's agent card at `url` (only `http://127.0.0.1:…`) through
/// `mailcraft_net`: 2 s, 1 MB, loopback checked on the address connected to, no redirect, nothing
/// sent but the `GET`.
pub(crate) fn discover(url: &str) -> Discovery {
    if !url.starts_with("http://127.0.0.1:") {
        return Discovery::Absent;
    }
    let request = StreamRequest {
        method: "GET",
        url,
        fields: &[("Accept", "application/json")],
        body: &[],
        trusted: "",
        deadlines: Deadlines { connect: UAR_TIMEOUT, first_byte: UAR_TIMEOUT, idle: UAR_TIMEOUT, total: UAR_TIMEOUT },
        max_response: UAR_CAP,
        max_error_response: 0,
        cancel: None,
    };
    let mut body = Vec::new();
    let mut over = false;
    let outcome = https::stream(&request, &mut |piece| match piece {
        Piece::Head(head) if head.status == 200 => Flow::Go,
        Piece::Head(_) => Flow::Stop,
        Piece::Body(bytes) => {
            if (body.len() as u64).saturating_add(bytes.len() as u64) > UAR_CAP {
                over = true;
                Flow::Stop
            } else {
                body.extend_from_slice(bytes);
                Flow::Go
            }
        }
    });
    match outcome {
        // Nothing listening (or the address refused before anything was sent) is absent; anything
        // that answered and went wrong (over the cap, too slow, not HTTP) is broken.
        Err(mailcraft_net::NetError::Connect { .. } | mailcraft_net::NetError::Config(_)) => Discovery::Absent,
        Err(_) => Discovery::Broken,
        Ok(done) if done.head.status == 200 && !over && !done.stopped && !done.truncated => {
            parse_card(&body).map_or(Discovery::Broken, Discovery::Found)
        }
        Ok(_) => Discovery::Broken,
    }
}

/// One run on an external instance at `base` (`http://127.0.0.1:<port>`), only when the owner
/// chose it; otherwise nothing is sent. No bearer: MailCraft never reads UAR's credentials. A tool
/// event ends the run with its text dropped and the owner told; its tools can't be stopped.
pub(crate) fn run_external(
    chosen: bool,
    base: &str,
    instructions: &str,
    prompt: &str,
    limits: Limits,
    cancel: &AtomicBool,
    notify: &mut dyn FnMut(Notice),
) -> RunEnd {
    if !chosen {
        return RunEnd::Refused { kind: "uar_external_not_chosen" };
    }
    let port_ok = base.strip_prefix("http://127.0.0.1:").is_some_and(|p| !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()));
    if !port_ok {
        return RunEnd::Refused { kind: "uar_external_address" };
    }
    let end = run_agui(&format!("{base}{RUNS_PATH}"), None, "mailcraft-external", instructions, prompt, limits, cancel);
    if let RunEnd::Stopped { kind } = end {
        log::warn!("ai runtime uar (external): stopped on a {kind} event");
        notify(Notice::ToolUseStopped);
    }
    end
}
