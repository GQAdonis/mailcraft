// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Containing a runtime: env allow-list, argv check, caps, stream detectors (B4, tasks 14.2–14.3).
//!
//! A CLI runtime (Claude Code, Codex…) is a coding agent; MailCraft uses it as a text model only.
//! Everything here is pure and builds everywhere (wasm included): the host's spawner
//! (`apps/mailcraft/src/ai/runtime/spawn.rs`) does the I/O and asks these functions what to do.
//!
//! - **Environment** ([`build_env`]): built from nothing. `PATH`, `HOME`, `USER`, `LANG`, the
//!   temporary folder, and only the variables the runtime's recipe names. A recipe can never name
//!   a variable that carries a key, a token, a proxy or another vendor's settings
//!   ([`is_forbidden_env_name`]).
//! - **Argv** ([`check_argv`]): no element may hold the prompt, any mail text, or a key or token
//!   MailCraft holds; a spawn whose argv would is refused. The process list is readable by every
//!   program on the computer.
//! - **Caps** ([`Limits`]): stdout 4 MB, stderr 64 KB, 1 MB a line, 120 s in all, 30 s silent.
//! - **Detectors** ([`StreamDetector`]): each adapter reads its runtime's stream line by line and
//!   says what each line is ([`Verdict`]). A tool use, a command, a file change, an MCP call, a
//!   web search or any kind it doesn't know kills the run. A detector may hold the prompt until it
//!   has checked the runtime's init event ([`Gate::AfterInit`]). [`Judge`] is the one state
//!   machine every run goes through. Only the event's kind is ever logged, never its content:
//!   kinds are `&'static str`, so they can't be built from what a runtime wrote.

use std::collections::HashSet;
use std::time::Duration;

/// The most a runtime may write on stdout in one run.
pub const MAX_STDOUT_BYTES: usize = 4 * 1024 * 1024;
/// The most of a runtime's stderr that is kept (the rest is read and dropped).
pub const MAX_STDERR_BYTES: usize = 64 * 1024;
/// The longest line a runtime may write; a longer one kills the run.
pub const MAX_LINE_BYTES: usize = 1024 * 1024;
/// The longest answer kept from one run.
pub const MAX_ANSWER_BYTES: usize = 512 * 1024;
/// A run's whole time, unless the owner set another.
pub const DEFAULT_WALL_CLOCK: Duration = Duration::from_secs(120);
/// The longest a run may set, whatever the owner chose.
pub const MAX_WALL_CLOCK: Duration = Duration::from_secs(600);
/// The longest a runtime may write nothing before the run is stopped.
pub const DEFAULT_IDLE: Duration = Duration::from_secs(30);
/// The shortest any of the times may be (a guard against a zero that stops every run at once).
pub const MIN_TIME: Duration = Duration::from_millis(50);

/// The most argv elements a spawn may have, sandbox wrapper included.
pub const MAX_ARGV: usize = 256;
/// The most bytes of argv a spawn may have, sandbox wrapper included.
pub const MAX_ARGV_BYTES: usize = 64 * 1024;
/// A run of this many bytes in common between an argv element and the prompt, a mail text or a
/// secret refuses the spawn.
pub const ARGV_WINDOW: usize = 32;
/// A secret or text shorter than [`ARGV_WINDOW`] is looked for whole when it is at least this long
/// (shorter ones can't be told from ordinary words).
pub const MIN_NEEDLE: usize = 8;
/// The most variables a recipe may add.
pub const MAX_RECIPE_VARS: usize = 16;
/// The longest value a variable may have.
pub const MAX_ENV_VALUE_BYTES: usize = 32 * 1024;

/// Why a spawn was refused before anything ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContainError {
    /// An argv element holds the prompt, a mail text, or a key or token.
    ArgvCarriesSecret,
    /// Argv is too long, has too many elements, or holds a NUL.
    ArgvMalformed,
    /// A recipe named a variable that may never reach a runtime.
    ForbiddenVariable,
    /// A variable's name or value can't be passed (empty, NUL, `=`, too long, too many).
    MalformedVariable,
}

impl ContainError {
    /// The error's kind, the only thing of it that is logged.
    pub fn kind(self) -> &'static str {
        match self {
            ContainError::ArgvCarriesSecret => "argv_carries_secret",
            ContainError::ArgvMalformed => "argv_malformed",
            ContainError::ForbiddenVariable => "forbidden_variable",
            ContainError::MalformedVariable => "malformed_variable",
        }
    }
}

impl std::fmt::Display for ContainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.kind())
    }
}

impl std::error::Error for ContainError {}

// ---------------------------------------------------------------------------------------------
// Caps and times
// ---------------------------------------------------------------------------------------------

/// The caps and times of one run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
    pub line_bytes: usize,
    pub wall_clock: Duration,
    pub idle: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            stdout_bytes: MAX_STDOUT_BYTES,
            stderr_bytes: MAX_STDERR_BYTES,
            line_bytes: MAX_LINE_BYTES,
            wall_clock: DEFAULT_WALL_CLOCK,
            idle: DEFAULT_IDLE,
        }
    }
}

impl Limits {
    /// The limits with the owner's wall clock (seconds), clamped.
    pub fn with_wall_clock_secs(secs: u64) -> Limits {
        Limits { wall_clock: Duration::from_secs(secs), ..Limits::default() }.clamped()
    }

    /// Every cap no higher than its maximum, every time between [`MIN_TIME`] and its maximum.
    /// Lower caps than the defaults are kept (tests use them).
    pub fn clamped(self) -> Limits {
        Limits {
            stdout_bytes: self.stdout_bytes.clamp(1, MAX_STDOUT_BYTES),
            stderr_bytes: self.stderr_bytes.min(MAX_STDERR_BYTES),
            line_bytes: self.line_bytes.clamp(1, MAX_LINE_BYTES),
            wall_clock: self.wall_clock.clamp(MIN_TIME, MAX_WALL_CLOCK),
            idle: self.idle.clamp(MIN_TIME, MAX_WALL_CLOCK),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Environment
// ---------------------------------------------------------------------------------------------

/// The variables every runtime gets, and no others but its recipe's.
pub const BASE_ENV_NAMES: [&str; 6] = ["PATH", "HOME", "USER", "LANG", "TMPDIR", "TEMP"];

/// What the host knows for the base variables. `None` leaves a variable out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BaseEnv {
    /// The login-shell PATH (B3's), never the process's own when that is empty.
    pub path: Option<String>,
    /// The real home (Claude Code) or the run's own (Codex, UAR): the recipe chooses.
    pub home: Option<String>,
    pub user: Option<String>,
    pub lang: Option<String>,
    /// The run's own temporary folder, inside its run folder (the sandbox allows writes there).
    pub tmpdir: Option<String>,
}

/// Whether a variable may never reach a runtime, whatever a recipe says: keys, tokens, proxies,
/// Node's options and every vendor's own settings. `CODEX_HOME` is the one `CODEX_*` a recipe may
/// name (Codex runs on the owner's own folder, which holds its sign-in; MailCraft never reads it).
pub fn is_forbidden_env_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    if upper == "CODEX_HOME" {
        return false;
    }
    const PREFIXES: [&str; 6] = ["ANTHROPIC_", "OPENAI_", "CODEX_", "CLAUDE_", "UAR_", "LLM_"];
    const NAMES: [&str; 8] = ["NODE_OPTIONS", "HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY", "FTP_PROXY", "SOCKS_PROXY", "LD_PRELOAD"];
    PREFIXES.iter().any(|p| upper.starts_with(p))
        || NAMES.contains(&upper.as_str())
        || upper.ends_with("_API_KEY")
        || upper.ends_with("_TOKEN")
        || upper.ends_with("_SECRET")
        || upper.ends_with("_PROXY")
        || upper.starts_with("DYLD_")
        || upper.starts_with("LD_")
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && !name.as_bytes().first().is_some_and(u8::is_ascii_digit)
}

fn valid_value(value: &str) -> bool {
    value.len() <= MAX_ENV_VALUE_BYTES && !value.contains('\0')
}

/// The child's whole environment: the base variables that are known, then the recipe's. A
/// recipe's variable that is forbidden, malformed or already a base variable refuses the spawn.
pub fn build_env(base: &BaseEnv, recipe_vars: &[(String, String)]) -> Result<Vec<(String, String)>, ContainError> {
    if recipe_vars.len() > MAX_RECIPE_VARS {
        return Err(ContainError::MalformedVariable);
    }
    let mut out = Vec::with_capacity(6 + recipe_vars.len());
    let base_pairs = [
        ("PATH", &base.path),
        ("HOME", &base.home),
        ("USER", &base.user),
        ("LANG", &base.lang),
        (if cfg!(windows) { "TEMP" } else { "TMPDIR" }, &base.tmpdir),
    ];
    for (name, value) in base_pairs {
        if let Some(v) = value {
            if !valid_value(v) {
                return Err(ContainError::MalformedVariable);
            }
            out.push((name.to_owned(), v.clone()));
        }
    }
    for (name, value) in recipe_vars {
        if !valid_name(name) || !valid_value(value) {
            return Err(ContainError::MalformedVariable);
        }
        if is_forbidden_env_name(name) {
            return Err(ContainError::ForbiddenVariable);
        }
        if BASE_ENV_NAMES.iter().any(|b| b.eq_ignore_ascii_case(name)) || out.iter().any(|(n, _)| n == name) {
            return Err(ContainError::MalformedVariable);
        }
        out.push((name.clone(), value.clone()));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Argv
// ---------------------------------------------------------------------------------------------

/// Refuses an argv (program included) that holds the prompt, any mail text, or a key or token.
///
/// `needles` are everything that must not appear: the prompt, the instructions, each mail text
/// that went into the prompt, and every key and token MailCraft holds. An argv element and a
/// needle sharing [`ARGV_WINDOW`] bytes refuses the spawn; a needle shorter than that, but at
/// least [`MIN_NEEDLE`] bytes, refuses it when an element holds it whole. The check is on bytes,
/// in time linear in the needles' length.
pub fn check_argv<S: AsRef<str>, N: AsRef<str>>(argv: &[S], needles: &[N]) -> Result<(), ContainError> {
    if argv.is_empty() || argv.len() > MAX_ARGV {
        return Err(ContainError::ArgvMalformed);
    }
    let mut total = 0usize;
    for a in argv {
        let a = a.as_ref();
        total = total.saturating_add(a.len());
        if a.contains('\0') {
            return Err(ContainError::ArgvMalformed);
        }
    }
    if total > MAX_ARGV_BYTES {
        return Err(ContainError::ArgvMalformed);
    }
    // Every ARGV_WINDOW-byte run of argv, looked up as the needles slide past.
    let mut windows: HashSet<&[u8]> = HashSet::new();
    for a in argv {
        let bytes = a.as_ref().as_bytes();
        if bytes.len() >= ARGV_WINDOW {
            for w in bytes.windows(ARGV_WINDOW) {
                windows.insert(w);
            }
        }
    }
    for needle in needles {
        let n = needle.as_ref().as_bytes();
        if n.len() >= ARGV_WINDOW {
            if !windows.is_empty() && n.windows(ARGV_WINDOW).any(|w| windows.contains(w)) {
                return Err(ContainError::ArgvCarriesSecret);
            }
        } else if n.len() >= MIN_NEEDLE && argv.iter().any(|a| contains_bytes(a.as_ref().as_bytes(), n)) {
            return Err(ContainError::ArgvCarriesSecret);
        }
    }
    Ok(())
}

fn contains_bytes(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.len() >= needle.len() && hay.windows(needle.len()).any(|w| w == needle)
}

// ---------------------------------------------------------------------------------------------
// Detectors
// ---------------------------------------------------------------------------------------------

/// When the spawner writes the prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    /// At once.
    Now,
    /// Only after the detector has returned [`Verdict::InitOk`] (it checked the runtime's init
    /// event and found no tool, MCP server, skill or agent in it).
    AfterInit,
}

/// What one line of a runtime's stream is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// More of the answer.
    Text(String),
    /// The whole answer (the run may end).
    Final(String),
    /// Something to note, never shown (its kind only is logged).
    Note(&'static str),
    /// The init event was checked and lists nothing that acts.
    InitOk,
    /// A tool, command, file change, MCP call, web search, an unknown non-text kind, or a line
    /// that is too long or doesn't parse: the run is killed.
    Kill(&'static str),
}

/// Reads one runtime's stream. Every line a runtime writes is hostile.
pub trait StreamDetector {
    /// When the prompt may be written.
    fn gate(&self) -> Gate;
    /// What `bytes` (one line, without its newline) is. Never panics; anything it doesn't know
    /// that isn't plain text is [`Verdict::Kill`].
    fn line(&mut self, bytes: &[u8]) -> Verdict;
}

impl<D: StreamDetector + ?Sized> StreamDetector for Box<D> {
    fn gate(&self) -> Gate {
        (**self).gate()
    }
    fn line(&mut self, bytes: &[u8]) -> Verdict {
        (**self).line(bytes)
    }
}

/// The classes of event that kill a run; each adapter's fixtures carry one of each (CONTAIN-001).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToolClass {
    ToolUse,
    Command,
    FileChange,
    McpCall,
    WebSearch,
    /// An event of a kind the detector doesn't know that isn't plain text.
    UnknownNonText,
}

impl ToolClass {
    /// Every class, in order.
    pub const ALL: [ToolClass; 6] =
        [ToolClass::ToolUse, ToolClass::Command, ToolClass::FileChange, ToolClass::McpCall, ToolClass::WebSearch, ToolClass::UnknownNonText];

    pub fn name(self) -> &'static str {
        match self {
            ToolClass::ToolUse => "tool_use",
            ToolClass::Command => "command",
            ToolClass::FileChange => "file_change",
            ToolClass::McpCall => "mcp_call",
            ToolClass::WebSearch => "web_search",
            ToolClass::UnknownNonText => "unknown_non_text",
        }
    }
}

/// The stream's lines, split as they arrive, each capped. A line over the cap is reported once
/// and the rest of the stream is not split any more (the run is killed).
#[derive(Debug)]
pub struct LineSplitter {
    buf: Vec<u8>,
    cap: usize,
    overflowed: bool,
}

/// One piece of a split stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece {
    Line(Vec<u8>),
    /// A line longer than the cap.
    TooLong,
}

impl LineSplitter {
    pub fn new(cap: usize) -> LineSplitter {
        LineSplitter { buf: Vec::new(), cap: cap.clamp(1, MAX_LINE_BYTES), overflowed: false }
    }

    /// The whole lines `bytes` completes, in order (a trailing `\r` is dropped).
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Piece> {
        let mut out = Vec::new();
        if self.overflowed {
            return out;
        }
        let mut rest = bytes;
        while let Some(pos) = rest.iter().position(|b| *b == b'\n') {
            let (head, tail) = rest.split_at(pos);
            if self.buf.len().saturating_add(head.len()) > self.cap {
                self.overflowed = true;
                self.buf.clear();
                out.push(Piece::TooLong);
                return out;
            }
            self.buf.extend_from_slice(head);
            let mut line = std::mem::take(&mut self.buf);
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            out.push(Piece::Line(line));
            rest = tail.get(1..).unwrap_or(&[]);
        }
        if self.buf.len().saturating_add(rest.len()) > self.cap {
            self.overflowed = true;
            self.buf.clear();
            out.push(Piece::TooLong);
            return out;
        }
        self.buf.extend_from_slice(rest);
        out
    }

    /// What is left at the end of the stream (a last line without a newline).
    pub fn finish(&mut self) -> Option<Piece> {
        if self.overflowed || self.buf.is_empty() {
            return None;
        }
        let mut line = std::mem::take(&mut self.buf);
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        Some(Piece::Line(line))
    }
}

/// What the spawner does after a line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Nothing yet.
    Continue,
    /// Write the prompt now (the init event was checked).
    WritePrompt,
    /// The answer is complete.
    Done(String),
    /// Kill the whole group, drop the text, and end the run as stopped (`tool_use_stopped`).
    Kill(&'static str),
    /// The answer grew past [`MAX_ANSWER_BYTES`]: kill, and end the run as too large.
    TooLarge,
}

/// The one state machine of a run: the detector's verdicts, the gate, the answer's cap. Once it
/// has said [`Step::Kill`] or [`Step::TooLarge`] it says nothing more, and its text is gone.
#[derive(Debug)]
pub struct Judge<D> {
    detector: D,
    gate: Gate,
    init_ok: bool,
    text: String,
    ended: bool,
    answer_cap: usize,
}

impl<D: StreamDetector> Judge<D> {
    pub fn new(detector: D) -> Judge<D> {
        Judge::with_cap(detector, MAX_ANSWER_BYTES)
    }

    pub fn with_cap(detector: D, answer_cap: usize) -> Judge<D> {
        let gate = detector.gate();
        Judge { detector, gate, init_ok: false, text: String::new(), ended: false, answer_cap: answer_cap.min(MAX_ANSWER_BYTES) }
    }

    /// Whether the prompt is written at once.
    pub fn prompt_at_start(&self) -> bool {
        self.gate == Gate::Now
    }

    /// Whether the run was killed or ended.
    pub fn ended(&self) -> bool {
        self.ended
    }

    /// The text so far (empty after a kill).
    pub fn text(&self) -> &str {
        &self.text
    }

    fn kill(&mut self, kind: &'static str) -> Step {
        self.ended = true;
        self.text.clear();
        Step::Kill(kind)
    }

    fn too_large(&mut self) -> Step {
        self.ended = true;
        self.text.clear();
        Step::TooLarge
    }

    /// One piece of the stream.
    pub fn piece(&mut self, piece: &Piece) -> Step {
        match piece {
            Piece::Line(bytes) => self.line(bytes),
            Piece::TooLong => {
                if self.ended {
                    Step::Continue
                } else {
                    self.kill("line_too_long")
                }
            }
        }
    }

    /// One line of the stream.
    pub fn line(&mut self, bytes: &[u8]) -> Step {
        if self.ended {
            return Step::Continue;
        }
        if bytes.len() > MAX_LINE_BYTES {
            return self.kill("line_too_long");
        }
        // A blank line is no event (some streams send them to keep the pipe alive).
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return Step::Continue;
        }
        let held = self.gate == Gate::AfterInit && !self.init_ok;
        match self.detector.line(bytes) {
            Verdict::Kill(kind) => self.kill(kind),
            Verdict::Note(_) => Step::Continue,
            Verdict::InitOk => {
                if held {
                    self.init_ok = true;
                    Step::WritePrompt
                } else {
                    // A second init, or one where none was asked for: nothing to do.
                    Step::Continue
                }
            }
            // Under the gate, no answer may come before the init event was checked.
            Verdict::Text(_) | Verdict::Final(_) if held => self.kill("text_before_init"),
            Verdict::Text(delta) => {
                if self.text.len().saturating_add(delta.len()) > self.answer_cap {
                    return self.too_large();
                }
                self.text.push_str(&delta);
                Step::Continue
            }
            Verdict::Final(text) => {
                if text.len() > self.answer_cap {
                    return self.too_large();
                }
                self.ended = true;
                self.text = text.clone();
                Step::Done(text)
            }
        }
    }
}

/// The shared fuzz harness (14.11): feeds `data`, cut into lines, to any detector through the
/// same splitter and judge a run uses. Returns the steps, for a target to check. Never panics.
pub fn drive<D: StreamDetector>(detector: D, data: &[u8], cut: usize) -> Vec<Step> {
    let mut judge = Judge::new(detector);
    let mut splitter = LineSplitter::new(MAX_LINE_BYTES);
    let cut = cut.min(data.len());
    let (head, tail) = data.split_at(cut);
    let mut steps = Vec::new();
    for part in [head, tail] {
        for piece in splitter.push(part) {
            steps.push(judge.piece(&piece));
        }
    }
    if let Some(piece) = splitter.finish() {
        steps.push(judge.piece(&piece));
    }
    steps
}

#[cfg(test)]
#[path = "contain_tests.rs"]
mod contain_tests;
