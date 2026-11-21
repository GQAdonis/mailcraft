// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Finding the runtimes on this computer (B3, tasks 13.2–13.7).
//!
//! A program started from the Dock or the Start menu doesn't inherit the owner's shell PATH, so
//! the PATH is read from the owner's login shell, once a session ([`login_path`]); each runtime's
//! candidates (`mailcraft_ai::runtime::detect`) are walked here, in Rust, never through `sh -c`.
//!
//! - **Only safe files are run** ([`check_executable`], also called by the spawn of every run):
//!   an absolute path, a regular file once its links are followed, owned by the owner or root,
//!   writable by nobody else, and the same for its directory (and, for a link, the link's
//!   directory). Anything else is [`Status::Broken`] with the reason, and nothing runs.
//! - **`--version` is hostile** ([`probe_version`]): argv only, an empty private directory to run
//!   in, no stdin, only the environment of [`ENV_ALLOW`], at most [`VERSION_TIMEOUT`] and
//!   [`VERSION_CAP`] bytes; a failure is `Broken`, never `Missing`.
//! - **No prompt, no credential, no network** but the UAR probe ([`probe_uar`]): a `GET` of its
//!   agent card on `127.0.0.1` through `mailcraft_net` (loopback checked on the address connected
//!   to, no redirect, [`UAR_TIMEOUT`], [`UAR_CAP`]).
//! - **The cache** in `ai.json` (`detected`) is written for the Options page and never used for a
//!   decision; a look runs at most once a minute unless the owner asks again ([`Rate`]).
//!
//! The callers (`ai.rescan` and the Options page, and the spawn's re-check) arrive with their own
//! changes.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use mailcraft_ai::runtime::detect::{self as pure, Os, Places, Program, Source, Version};
use mailcraft_model::ai::{RuntimeKind, RuntimePath};
use mailcraft_net::https::{self, Deadlines, Flow, Piece, StreamRequest};

use crate::ai::settings::{AiSettings, Shared};
use crate::ai::settings_v3::Detected;

/// How long the login shell has to print its PATH.
pub(crate) const SHELL_TIMEOUT: Duration = Duration::from_secs(10);
/// The most bytes of the login shell's output read.
pub(crate) const SHELL_CAP: usize = 64 * 1024;
/// How long `--version` has.
pub(crate) const VERSION_TIMEOUT: Duration = Duration::from_secs(5);
/// The most bytes of `--version` read.
pub(crate) const VERSION_CAP: usize = 4 * 1024;
/// How long the UAR probe has, all of it.
pub(crate) const UAR_TIMEOUT: Duration = Duration::from_secs(2);
/// The most bytes of UAR's agent card read.
pub(crate) const UAR_CAP: u64 = 1024 * 1024;
/// Where the managed UAR answers.
pub(crate) const UAR_CARD_URL: &str = "http://127.0.0.1:1906/.well-known/agent-card.json";
/// How often a look may run by itself.
pub(crate) const RATE: Duration = Duration::from_secs(60);
/// The variables a probe is given (the PATH is the login shell's, set apart). Nothing else of
/// MailCraft's environment, so no key or token, reaches the program.
pub(crate) const ENV_ALLOW: &[&str] = &["HOME", "USER", "LOGNAME", "LANG", "LC_ALL", "LC_CTYPE", "TMPDIR", "SYSTEMROOT", "USERPROFILE"];
/// The most bytes of `/etc/shells` read.
const SHELLS_CAP: u64 = 64 * 1024;
/// How often a wait looks whether a program has ended.
const POLL: Duration = Duration::from_millis(10);

/// Why a file is not run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// The path isn't absolute.
    NotAbsolute,
    /// There is no file there (or a link points nowhere).
    Missing,
    /// It isn't a regular file (a directory, a pipe, a device).
    NotRegular,
    /// Its owner is neither the owner of MailCraft nor root.
    Owner,
    /// Someone else may change it (group- or world-writable).
    Writable,
    /// Its directory's owner is neither the owner nor root.
    ParentOwner,
    /// Someone else may change its directory.
    ParentWritable,
    /// It couldn't be looked at.
    Unreadable,
}

impl Refusal {
    /// The reason, as the Options page says it.
    pub(crate) fn reason(self) -> &'static str {
        match self {
            Refusal::NotAbsolute => "the path isn't absolute",
            Refusal::Missing => "there is no program there",
            Refusal::NotRegular => "it isn't a regular file",
            Refusal::Owner => "it belongs to another user",
            Refusal::Writable => "other users can change it",
            Refusal::ParentOwner => "its folder belongs to another user",
            Refusal::ParentWritable => "other users can change its folder",
            Refusal::Unreadable => "it couldn't be looked at",
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.reason())
    }
}

/// The user id this program runs as, read from a file it makes (no `unsafe` call needed).
#[cfg(unix)]
fn own_uid() -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    static UID: OnceLock<Option<u32>> = OnceLock::new();
    *UID.get_or_init(|| tempfile::tempfile().and_then(|f| f.metadata()).ok().map(|m| m.uid()))
}

/// Whether `meta` is owned by the owner or root and writable by nobody else.
#[cfg(unix)]
fn owned_safely(meta: &std::fs::Metadata, owner: Refusal, writable: Refusal) -> Result<(), Refusal> {
    use std::os::unix::fs::MetadataExt;
    let uid = meta.uid();
    if uid != 0 && Some(uid) != own_uid() {
        return Err(owner);
    }
    if meta.mode() & 0o022 != 0 {
        return Err(writable);
    }
    Ok(())
}

/// On Windows the owner and the access list aren't read here (see the report): the path must be
/// absolute and a regular file.
#[cfg(not(unix))]
fn owned_safely(_meta: &std::fs::Metadata, _owner: Refusal, _writable: Refusal) -> Result<(), Refusal> {
    Ok(())
}

/// What a failed look at a file is.
fn looked(error: &std::io::Error) -> Refusal {
    if error.kind() == std::io::ErrorKind::NotFound { Refusal::Missing } else { Refusal::Unreadable }
}

/// Whether the directory `dir` (links followed) is safe to hold a program.
fn check_dir(dir: &Path) -> Result<(), Refusal> {
    let real = std::fs::canonicalize(dir).map_err(|_| Refusal::Unreadable)?;
    let meta = std::fs::metadata(&real).map_err(|_| Refusal::Unreadable)?;
    if !meta.is_dir() {
        return Err(Refusal::NotRegular);
    }
    owned_safely(&meta, Refusal::ParentOwner, Refusal::ParentWritable)
}

/// The program at `path`, when it is safe to run: absolute; a regular file once links are
/// followed; owned by the owner or root and not group- or world-writable; and the same for its
/// directory and, when `path` is a link, for the link's directory. Returns the file the links
/// lead to, which is what is run. Called by detection and again before every spawn.
pub(crate) fn check_executable(path: &Path) -> Result<PathBuf, Refusal> {
    if !path.is_absolute() {
        return Err(Refusal::NotAbsolute);
    }
    let link = std::fs::symlink_metadata(path).map_err(|e| looked(&e))?;
    let real = std::fs::canonicalize(path).map_err(|e| looked(&e))?;
    let meta = std::fs::metadata(&real).map_err(|e| looked(&e))?;
    if !meta.is_file() {
        return Err(Refusal::NotRegular);
    }
    owned_safely(&meta, Refusal::Owner, Refusal::Writable)?;
    check_dir(real.parent().ok_or(Refusal::NotRegular)?)?;
    if link.file_type().is_symlink() {
        check_dir(path.parent().ok_or(Refusal::NotRegular)?)?;
    }
    Ok(real)
}

/// What running a program within bounds gave.
#[derive(Debug, Default)]
pub(crate) struct Ran {
    /// What it printed, at most the cap.
    pub(crate) out: Vec<u8>,
    /// It printed more than the cap (and was stopped).
    pub(crate) overflow: bool,
    /// It didn't finish in time (and was stopped).
    pub(crate) timed_out: bool,
    /// How it ended, when it ended by itself.
    pub(crate) status: Option<ExitStatus>,
}

/// Waits for `child` until `deadline`: its status, or `None` when it is still running.
fn wait_until(child: &mut Child, deadline: Instant) -> Option<ExitStatus> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL),
            _ => return None,
        }
    }
}

/// Runs `command` with no stdin and stderr dropped, reading at most `cap` bytes of stdout for at
/// most `timeout`; a program that prints more or takes longer is killed.
pub(crate) fn run_capped(mut command: Command, cap: usize, timeout: Duration) -> std::io::Result<Ran> {
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    let started = Instant::now();
    let deadline = started.checked_add(timeout).unwrap_or(started);
    let mut child = command.spawn()?;
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(std::io::Error::other("no output pipe"));
    };
    let limit = cap.saturating_add(1);
    let (sender, receiver) = mpsc::channel::<Vec<u8>>();
    let reader = std::thread::Builder::new().name("ai-detect-read".into()).spawn(move || {
        let mut buffer = [0u8; 4096];
        let mut total = 0usize;
        while total < limit {
            match stdout.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    total = total.saturating_add(n);
                    if sender.send(buffer.get(..n).unwrap_or_default().to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    if reader.is_err() {
        let _ = child.kill();
        let _ = child.wait();
        return Err(std::io::Error::other("no reader"));
    }
    let mut ran = Ran::default();
    let mut ended = false;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            ran.timed_out = true;
            break;
        }
        match receiver.recv_timeout(left) {
            Ok(piece) => {
                ran.out.extend_from_slice(&piece);
                if ran.out.len() > cap {
                    ran.out.truncate(cap);
                    ran.overflow = true;
                    break;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                ran.timed_out = true;
                break;
            }
            Err(RecvTimeoutError::Disconnected) => {
                ended = true;
                break;
            }
        }
    }
    ran.status = if ended { wait_until(&mut child, deadline) } else { None };
    if ran.status.is_none() {
        let _ = child.kill();
        let _ = child.wait();
        ran.timed_out |= !ran.overflow;
    }
    Ok(ran)
}

/// `command` given only the variables of [`ENV_ALLOW`] that are set, and `path` as its PATH.
fn allow_env(command: &mut Command, path: &str) {
    command.env_clear();
    for name in ENV_ALLOW {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    if !path.is_empty() {
        command.env("PATH", path);
    }
}

/// The shell to read the PATH with: `shell` when it is an absolute path listed in `shells` (the
/// text of `/etc/shells`) that `exists`; else `/bin/sh`.
pub(crate) fn choose_shell(shell: Option<&str>, shells: &str, exists: impl Fn(&Path) -> bool) -> PathBuf {
    let listed = |s: &str| shells.lines().map(str::trim).any(|line| !line.starts_with('#') && line == s);
    match shell {
        Some(s) if s.starts_with('/') && !s.chars().any(char::is_control) && listed(s) && exists(Path::new(s)) => PathBuf::from(s),
        _ => PathBuf::from("/bin/sh"),
    }
}

/// The PATH a shell printed: its last line (a profile that prints a greeting before it is left
/// out), split with only the absolute entries kept.
pub(crate) fn path_from_shell_output(out: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(out);
    let last = text.trim_end_matches(['\n', '\r']).rsplit(['\n', '\r']).next().unwrap_or_default();
    pure::split_path(last, Os::Unix)
}

/// Reads the PATH from `shell` as a login shell, within [`SHELL_TIMEOUT`] and [`SHELL_CAP`].
pub(crate) fn read_shell_path(shell: &Path) -> Option<Vec<String>> {
    let dir = tempfile::Builder::new().prefix("mailcraft-detect-").tempdir().ok()?;
    let mut command = Command::new(shell);
    command.args(["-lc", "printf %s \"$PATH\""]).current_dir(dir.path());
    allow_env(&mut command, "");
    command.env("SHELL", shell);
    let ran = run_capped(command, SHELL_CAP, SHELL_TIMEOUT).ok()?;
    if ran.timed_out || ran.overflow || !ran.status.is_some_and(|s| s.success()) {
        return None;
    }
    Some(path_from_shell_output(&ran.out))
}

/// The login shell's PATH (macOS, Linux, BSD) or the registry's (Windows), read once a session.
/// When it can't be had, MailCraft's own PATH, split the same way.
pub(crate) fn login_path() -> &'static [String] {
    static PATH: OnceLock<Vec<String>> = OnceLock::new();
    PATH.get_or_init(|| {
        let read = read_system_path();
        if read.is_empty() { pure::split_path(&std::env::var("PATH").unwrap_or_default(), Os::this()) } else { read }
    })
}

#[cfg(unix)]
fn read_system_path() -> Vec<String> {
    let shells = std::fs::File::open("/etc/shells")
        .and_then(|f| {
            let mut text = String::new();
            f.take(SHELLS_CAP).read_to_string(&mut text).map(|_| text)
        })
        .unwrap_or_default();
    let shell = std::env::var("SHELL").ok();
    let chosen = choose_shell(shell.as_deref(), &shells, |p| std::fs::metadata(p).is_ok_and(|m| m.is_file()));
    read_shell_path(&chosen).unwrap_or_default()
}

#[cfg(windows)]
fn read_system_path() -> Vec<String> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    fn path_of(root: RegKey, key: &str) -> Option<String> {
        root.open_subkey(key).and_then(|k| k.get_value::<String, _>("Path")).ok()
    }
    let system = path_of(RegKey::predef(HKEY_LOCAL_MACHINE), r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment");
    let user = path_of(RegKey::predef(HKEY_CURRENT_USER), "Environment");
    let joined: Vec<String> = [system, user].into_iter().flatten().map(|t| expand_percent(&t, |name| std::env::var(name).ok())).collect();
    pure::split_path(&joined.join(";"), Os::Windows)
}

#[cfg(not(any(unix, windows)))]
fn read_system_path() -> Vec<String> {
    Vec::new()
}

/// `text` with each `%NAME%` that `lookup` knows replaced (Windows' `REG_EXPAND_SZ`); an
/// unknown or unclosed one is kept as it is. At most 32 KB come out.
#[cfg(any(windows, test))]
pub(crate) fn expand_percent(text: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    const MAX: usize = 32 * 1024;
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        out.push_str(rest.get(..start).unwrap_or_default());
        let after = rest.get(start + 1..).unwrap_or_default();
        match after.find('%') {
            Some(end) => {
                let name = after.get(..end).unwrap_or_default();
                match lookup(name).filter(|_| !name.is_empty()) {
                    Some(value) => out.push_str(&value),
                    None => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = after.get(end + 1..).unwrap_or_default();
            }
            None => {
                out.push('%');
                rest = after;
            }
        }
        if out.len() > MAX {
            break;
        }
    }
    out.push_str(rest);
    out.chars().take(MAX).collect()
}

/// Why a version probe failed, and what it printed, cleaned and capped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProbeFailure {
    pub(crate) reason: &'static str,
    pub(crate) raw: Option<String>,
}

impl ProbeFailure {
    fn new(reason: &'static str, out: Option<&[u8]>) -> ProbeFailure {
        ProbeFailure { reason, raw: out.map(|bytes| pure::parse_version(bytes).raw).filter(|r| !r.is_empty()) }
    }
}

/// Runs `<exe> --version`: argv only, in an empty private directory, no stdin, the environment of
/// [`ENV_ALLOW`] with `path_env` as its PATH, within [`VERSION_TIMEOUT`] and [`VERSION_CAP`].
pub(crate) fn probe_version(exe: &Path, path_env: &str) -> Result<Version, ProbeFailure> {
    let dir = tempfile::Builder::new().prefix("mailcraft-detect-").tempdir().map_err(|_| ProbeFailure::new("no folder to run it in", None))?;
    let mut command = Command::new(exe);
    command.arg("--version").current_dir(dir.path());
    allow_env(&mut command, path_env);
    let ran = run_capped(command, VERSION_CAP, VERSION_TIMEOUT).map_err(|_| ProbeFailure::new("it couldn't be started", None))?;
    if ran.overflow {
        return Err(ProbeFailure::new("it printed too much for --version", Some(&ran.out)));
    }
    if ran.timed_out {
        return Err(ProbeFailure::new("it didn't answer --version in time", Some(&ran.out)));
    }
    if !ran.status.is_some_and(|s| s.success()) {
        return Err(ProbeFailure::new("--version failed", Some(&ran.out)));
    }
    // A version line has no escape sequence or other control character (a line ending aside).
    if ran.out.iter().any(|&b| b.is_ascii_control() && !matches!(b, b'\n' | b'\r' | b'\t')) {
        return Err(ProbeFailure::new("--version printed control characters", Some(&ran.out)));
    }
    let version = pure::parse_version(&ran.out);
    if version.semver.is_none() {
        return Err(ProbeFailure::new("--version printed no version", Some(&ran.out)));
    }
    Ok(version)
}

/// Whether UAR answers at `url` (only `http://127.0.0.1:…`): a `GET` through `mailcraft_net`
/// (loopback checked on the address connected to, no redirect followed) within [`UAR_TIMEOUT`],
/// whose answer is a 200 with a JSON object of at most [`UAR_CAP`] bytes. Nothing else is sent.
pub(crate) fn probe_uar(url: &str) -> bool {
    if !url.starts_with("http://127.0.0.1:") {
        return false;
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
    let mut body: Vec<u8> = Vec::new();
    let outcome = https::stream(&request, &mut |piece| match piece {
        Piece::Head(head) if head.status == 200 => Flow::Go,
        Piece::Head(_) => Flow::Stop,
        Piece::Body(bytes) => {
            if (body.len() as u64).saturating_add(bytes.len() as u64) > UAR_CAP {
                Flow::Stop
            } else {
                body.extend_from_slice(bytes);
                Flow::Go
            }
        }
    });
    match outcome {
        Ok(done) if done.head.status == 200 && !done.stopped && !done.truncated => {
            serde_json::from_slice::<serde_json::Value>(&body).is_ok_and(|v| v.is_object())
        }
        _ => false,
    }
}

/// A runtime that was found and answered `--version`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Found {
    /// The file run (links followed).
    pub(crate) path: PathBuf,
    /// The version (`2.1.326`).
    pub(crate) version: String,
    /// What `--version` printed, cleaned, at most 64 characters.
    pub(crate) raw: String,
    pub(crate) source: Source,
}

/// A runtime that is there but can't be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Broken {
    /// The path looked at.
    pub(crate) path: PathBuf,
    pub(crate) source: Source,
    /// Why, in words.
    pub(crate) reason: String,
    /// What `--version` printed, cleaned and capped, when it ran.
    pub(crate) raw: Option<String>,
}

/// What a look found of one runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    /// Nowhere it was looked for.
    Missing,
    Detected(Found),
    Broken(Broken),
}

/// One runtime and what was found of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Detection {
    pub(crate) program: Program,
    pub(crate) status: Status,
    /// For UAR: whether its agent card answered on loopback.
    pub(crate) listening: Option<bool>,
}

/// What a look found, and every program it ran (each with `--version` only).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Report {
    pub(crate) detections: Vec<Detection>,
    pub(crate) spawned: Vec<PathBuf>,
}

impl Report {
    /// What was found of `program`.
    #[cfg(test)]
    pub(crate) fn status(&self, program: Program) -> Option<&Status> {
        self.detections.iter().find(|d| d.program == program).map(|d| &d.status)
    }
}

/// What a look goes by.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Context {
    /// The owner's path per runtime (from the chain), as hostile text.
    pub(crate) overrides: BTreeMap<Program, String>,
    /// The PATH to walk, absolute entries only.
    pub(crate) path: Vec<String>,
    /// The owner's home, when absolute.
    pub(crate) home: Option<String>,
    /// npm's global `bin`, when known.
    pub(crate) npm_bin: Option<String>,
    /// UAR's agent card, or `None` for no probe.
    pub(crate) uar_card_url: Option<String>,
    /// Whether the system's own folders are looked in (always, but in tests).
    pub(crate) system_dirs: bool,
}

/// The program a runtime kind is, when it is one.
pub(crate) fn program_of(kind: RuntimeKind) -> Option<Program> {
    match kind {
        RuntimeKind::ClaudeCode => Some(Program::Claude),
        RuntimeKind::Codex => Some(Program::Codex),
        RuntimeKind::Uar => Some(Program::Uar),
        RuntimeKind::OnDevice | RuntimeKind::Compat | RuntimeKind::ChatGpt => None,
    }
}

impl Context {
    /// The context of this computer: the overrides of `settings`' chain, the login PATH, `$HOME`
    /// (`%USERPROFILE%`), npm's global `bin` (`$NPM_CONFIG_PREFIX/bin`, else
    /// `~/.npm-global/bin`) and UAR's own address.
    pub(crate) fn from_system(settings: &AiSettings) -> Context {
        let overrides =
            settings.chain.entries().iter().filter_map(|e| Some((program_of(e.runtime().kind())?, e.path()?.as_str().to_string()))).collect();
        let os = Os::this();
        let absolute = |v: String| pure::is_absolute(&v, os).then_some(v);
        let home = std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).ok().and_then(absolute);
        let npm_bin = std::env::var("NPM_CONFIG_PREFIX")
            .ok()
            .and_then(absolute)
            .map(|p| format!("{}/bin", p.trim_end_matches('/')))
            .or_else(|| home.as_ref().map(|h| format!("{}/.npm-global/bin", h.trim_end_matches('/'))));
        Context { overrides, path: login_path().to_vec(), home, npm_bin, uar_card_url: Some(UAR_CARD_URL.to_string()), system_dirs: true }
    }

    /// The PATH a probe is given.
    fn path_env(&self) -> String {
        self.path.join(if cfg!(windows) { ";" } else { ":" })
    }
}

/// Looks for `program` as `context` says, recording in `spawned` each program it runs.
fn detect_program(program: Program, context: &Context, spawned: &mut Vec<PathBuf>) -> Status {
    let places = Places {
        override_path: context.overrides.get(&program).map(String::as_str),
        path: &context.path,
        home: context.home.as_deref(),
        npm_bin: context.npm_bin.as_deref(),
        system: context.system_dirs,
    };
    let mut refused: Option<Broken> = None;
    for candidate in pure::candidates(program, &places, Os::this()) {
        let path = PathBuf::from(&candidate.path);
        let broken = |reason: &str, raw: Option<String>| Broken { path: path.clone(), source: candidate.source, reason: reason.to_string(), raw };
        match check_executable(&path) {
            // A place that holds nothing is passed over; an override the owner named is not.
            Err(Refusal::Missing) if candidate.source != Source::Override => continue,
            Err(refusal) if candidate.source == Source::Override => return Status::Broken(broken(refusal.reason(), None)),
            Err(refusal) => {
                refused.get_or_insert_with(|| broken(refusal.reason(), None));
            }
            Ok(real) => {
                spawned.push(real.clone());
                return match probe_version(&real, &context.path_env()) {
                    Ok(version) => Status::Detected(Found {
                        path: real,
                        version: version.semver.unwrap_or_default(),
                        raw: version.raw,
                        source: candidate.source,
                    }),
                    Err(failure) => Status::Broken(broken(failure.reason, failure.raw)),
                };
            }
        }
    }
    refused.map_or(Status::Missing, Status::Broken)
}

/// Looks for every runtime as `context` says. Sends no prompt, reads no credential, and opens no
/// connection but UAR's loopback probe.
pub(crate) fn detect(context: &Context) -> Report {
    let mut report = Report::default();
    for program in Program::ALL {
        let status = detect_program(program, context, &mut report.spawned);
        let listening = (program == Program::Uar).then(|| context.uar_card_url.as_deref().is_some_and(probe_uar));
        report.detections.push(Detection { program, status, listening });
    }
    report
}

/// When looks may run: at most once every [`RATE`], unless the owner asks again (`ai.rescan`).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Rate {
    last: Option<Instant>,
}

impl Rate {
    /// Whether a look may run at `now`; when it may, it counts as run.
    pub(crate) fn allow(&mut self, now: Instant, rescan: bool) -> bool {
        let due = rescan || self.last.is_none_or(|last| now.saturating_duration_since(last) >= RATE);
        if due {
            self.last = Some(now);
        }
        due
    }
}

/// `report` written into `settings`' detection cache, checked at `checked` (seconds since 1970):
/// a runtime found keeps its path and version, a broken one its path alone, a missing one
/// nothing. A cache only, never used for a decision.
pub(crate) fn write_cache(settings: &mut AiSettings, report: &Report, checked: i64) {
    let when = format!("{}Z", mailcraft_time::DateTime::from_timestamp(checked, 0).iso());
    for detection in &report.detections {
        let key = detection.program.kind_name().to_string();
        let (path, version) = match &detection.status {
            Status::Missing => {
                settings.detected.remove(&key);
                continue;
            }
            Status::Detected(found) => (&found.path, Some(found.version.clone())),
            Status::Broken(broken) => (&broken.path, None),
        };
        let path = path.to_str().and_then(|p| RuntimePath::new(p).ok());
        settings.detected.insert(key, Detected { path, version, checked: Some(when.clone()) });
    }
}

/// A look at this computer, when [`Rate`] allows it (always for `rescan`), its cache written
/// through `settings` (one change, kept whole with the rest of the settings, never a write of the
/// file of its own). `None` when it wasn't due. Runs programs: never on the window's thread.
pub(crate) fn look(settings: &Shared, rescan: bool) -> Option<Result<Report, String>> {
    static RATE_NOW: Mutex<Rate> = Mutex::new(Rate { last: None });
    let due = RATE_NOW.lock().unwrap_or_else(PoisonError::into_inner).allow(Instant::now(), rescan);
    if !due {
        return None;
    }
    // From a copy: the settings aren't held while programs run.
    let context = Context::from_system(&settings.snapshot());
    let report = detect(&context);
    let checked = mailcraft_time::now();
    Some(
        settings
            .change(|s| {
                write_cache(s, &report, checked);
                Ok(())
            })
            .map(|()| report),
    )
}
