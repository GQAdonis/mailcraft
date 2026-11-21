// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Claude Code adapter, pure parts: argv, auth status, stream-json detector (B5, tasks 15.2–15.4).
//!
//! MailCraft runs the owner's **unmodified** `claude` on the owner's own login, as a text model
//! with every tool removed. It never reads, stores or passes that login: only the CLI, in its own
//! process, reads `~/.claude`, `~/.claude.json` or the Keychain. Everything here is pure and
//! builds everywhere (wasm included); the host (`apps/mailcraft/src/ai/runtime/claude_code.rs`)
//! spawns the CLI through B4's spawner and asks these functions what to pass and what came back.
//!
//! - **Argv** ([`run_args`]): the fixed flag list, the instructions file, and `--model` only when
//!   the owner chose one (checked). No prompt, no mail, no key ever goes on argv.
//! - **Status** ([`parse_auth_status`]): `claude auth status --json`, read within
//!   [`MAX_STATUS_BYTES`]. Only `loggedIn`, `authMethod`, `subscriptionType` and `planTier` are
//!   read; the account's e-mail and organisation are never kept.
//! - **Stream** ([`ClaudeStreamDetector`]): every line is hostile. Text is answer text; any tool
//!   block, tool result, or kind it doesn't know kills the run. An error `result` is noted, never
//!   shown, and its kind ([`RunOutcome`]) is kept for the host (a "not logged in" is
//!   [`RunOutcome::SignedOut`]). Only kinds are ever logged.

use std::cell::Cell;
use std::rc::Rc;

use serde::Deserialize;
use serde_json::Value;

use super::contain::{Gate, StreamDetector, Verdict};

/// The program's name, as the owner's terminal runs it for the sign-in.
pub const PROGRAM: &str = "claude";

/// The flags of every run, in order (plan.md §7.2, plus `--safe-mode` from the W7 contamination
/// entry check: it keeps the owner's CLAUDE.md, skills, plugins, hooks, MCP servers, custom agents
/// and auto-memory out of the run, and leaves the CLI's own login working).
pub const RUN_FLAGS: [&str; 11] = [
    "-p",
    "--tools",
    "",
    "--strict-mcp-config",
    "--setting-sources",
    "",
    "--no-session-persistence",
    "--output-format",
    "stream-json",
    "--verbose",
    "--safe-mode",
];

/// The flag naming the run's 0600 instructions file.
pub const SYSTEM_PROMPT_FILE_FLAG: &str = "--system-prompt-file";
/// The flag naming the owner's model.
pub const MODEL_FLAG: &str = "--model";

/// The status probe's argv after the program.
pub const STATUS_ARGS: [&str; 3] = ["auth", "status", "--json"];
/// The sign-in's argv after the program: run by the owner in their own terminal, never by
/// MailCraft.
pub const SIGN_IN_ARGS: [&str; 2] = ["auth", "login"];

/// The most bytes of `claude auth status` read; more is [`AuthStatus::Broken`].
pub const MAX_STATUS_BYTES: usize = 16 * 1024;
/// The longest model name taken (B2's `mailcraft_model::ai::MAX_MODEL` for runtimes).
pub const MAX_MODEL: usize = 64;

/// One argument of a Claude Code run, for the host to turn into its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClaudeArg {
    /// A constant of the recipe.
    Lit(&'static str),
    /// The path of the run's 0600 instructions file.
    InstructionsFile,
    /// The owner's model, checked by [`valid_model`].
    Model(String),
}

/// Why a run's argv couldn't be built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgError {
    /// The owner's model isn't a model's name.
    BadModel,
}

impl ArgError {
    /// The error's kind, the only thing of it that is logged.
    pub fn kind(self) -> &'static str {
        match self {
            ArgError::BadModel => "bad_model",
        }
    }
}

/// Whether `name` may be passed as `--model`: B2's pattern for a runtime's model (1 to
/// [`MAX_MODEL`] bytes of `A-Z a-z 0-9 . _ : / -`), and not starting with `-`, so it can never be
/// read as a flag.
pub fn valid_model(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_MODEL
        && !name.starts_with('-')
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'/' | b'-'))
}

/// A run's argv after the program: [`RUN_FLAGS`], `--system-prompt-file <file>`, and
/// `--model <m>` when the owner chose one. Nothing else.
pub fn run_args(model: Option<&str>) -> Result<Vec<ClaudeArg>, ArgError> {
    let mut args: Vec<ClaudeArg> = RUN_FLAGS.iter().map(|f| ClaudeArg::Lit(f)).collect();
    args.push(ClaudeArg::Lit(SYSTEM_PROMPT_FILE_FLAG));
    args.push(ClaudeArg::InstructionsFile);
    if let Some(m) = model {
        if !valid_model(m) {
            return Err(ArgError::BadModel);
        }
        args.push(ClaudeArg::Lit(MODEL_FLAG));
        args.push(ClaudeArg::Model(m.to_owned()));
    }
    Ok(args)
}

// ---------------------------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------------------------

/// How the CLI is signed in, as `authMethod` says. Nothing else of the account is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthMethod {
    /// A claude.ai subscription (Pro, Max, Team, Enterprise).
    ClaudeAi,
    /// An Anthropic Console account.
    Console,
    /// An API key the CLI itself holds.
    ApiKey,
    /// Any other method the CLI names (still usable: the CLI is signed in).
    Other,
}

impl AuthMethod {
    fn from_text(text: &str) -> AuthMethod {
        match text {
            "claude.ai" | "claude_ai" | "claudeai" => AuthMethod::ClaudeAi,
            "console" | "console_oauth" => AuthMethod::Console,
            "api_key" | "apiKey" | "api-key" => AuthMethod::ApiKey,
            _ => AuthMethod::Other,
        }
    }
}

/// Why a status couldn't be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusProblem {
    /// More than [`MAX_STATUS_BYTES`].
    TooLarge,
    /// Not JSON, not an object, or a field of the wrong type.
    Malformed,
    /// No `loggedIn`.
    MissingLoggedIn,
    /// The probe didn't answer in time (set by the host).
    Late,
    /// The probe couldn't run (set by the host).
    NotRun,
}

impl StatusProblem {
    /// The problem's kind, the only thing of it that is logged.
    pub fn kind(self) -> &'static str {
        match self {
            StatusProblem::TooLarge => "status_too_large",
            StatusProblem::Malformed => "status_malformed",
            StatusProblem::MissingLoggedIn => "status_missing_logged_in",
            StatusProblem::Late => "status_late",
            StatusProblem::NotRun => "status_not_run",
        }
    }
}

/// What `claude auth status` says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthStatus {
    /// Signed in with any method. `uses_plan`: a subscription type or plan tier is named, so the
    /// owner is told answers use their Claude plan.
    Usable { method: AuthMethod, uses_plan: bool },
    /// Not signed in: the owner signs in with `claude auth login` in a terminal.
    NeedsSignIn,
    /// The status couldn't be read.
    Broken(StatusProblem),
}

impl AuthStatus {
    /// Whether runs may use it.
    pub fn is_usable(self) -> bool {
        matches!(self, AuthStatus::Usable { .. })
    }
}

/// What the owner is told for a status that uses their plan (a `tl!` key in the window).
pub const USES_YOUR_PLAN: &str = "uses your Claude plan";

/// The only fields read; every other one (the e-mail, the organisation, paths) is skipped by
/// serde without being kept.
#[derive(Deserialize)]
struct StatusFields {
    #[serde(rename = "loggedIn")]
    logged_in: Option<Value>,
    #[serde(rename = "authMethod")]
    auth_method: Option<Value>,
    #[serde(rename = "subscriptionType")]
    subscription_type: Option<Value>,
    #[serde(rename = "planTier")]
    plan_tier: Option<Value>,
}

/// Whether a plan field names a plan (a non-empty string other than `none`).
fn names_plan(v: &Option<Value>) -> bool {
    v.as_ref().and_then(Value::as_str).is_some_and(|s| !s.trim().is_empty() && !s.eq_ignore_ascii_case("none"))
}

/// The status the CLI printed, read as hostile: over [`MAX_STATUS_BYTES`], not a JSON object,
/// without a boolean `loggedIn`, or with a field of the wrong type is [`AuthStatus::Broken`].
pub fn parse_auth_status(bytes: &[u8]) -> AuthStatus {
    if bytes.len() > MAX_STATUS_BYTES {
        return AuthStatus::Broken(StatusProblem::TooLarge);
    }
    // serde would read a struct from an array too: only an object is a status.
    if bytes.iter().find(|b| !b.is_ascii_whitespace()) != Some(&b'{') {
        return AuthStatus::Broken(StatusProblem::Malformed);
    }
    let fields: StatusFields = match serde_json::from_slice(bytes) {
        Ok(f) => f,
        Err(_) => return AuthStatus::Broken(StatusProblem::Malformed),
    };
    let logged_in = match fields.logged_in {
        None | Some(Value::Null) => return AuthStatus::Broken(StatusProblem::MissingLoggedIn),
        Some(Value::Bool(b)) => b,
        Some(_) => return AuthStatus::Broken(StatusProblem::Malformed),
    };
    if !logged_in {
        return AuthStatus::NeedsSignIn;
    }
    let method = match &fields.auth_method {
        None | Some(Value::Null) => AuthMethod::Other,
        Some(Value::String(s)) => AuthMethod::from_text(s),
        Some(_) => return AuthStatus::Broken(StatusProblem::Malformed),
    };
    let uses_plan = names_plan(&fields.subscription_type) || names_plan(&fields.plan_tier);
    AuthStatus::Usable { method, uses_plan }
}

// ---------------------------------------------------------------------------------------------
// Stream
// ---------------------------------------------------------------------------------------------

/// What an error `result` (or an API error message) said, kept as a kind for the host; the
/// CLI's words are never kept or shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RunOutcome {
    /// No error was reported.
    #[default]
    Clean,
    /// The CLI isn't signed in (or its sign-in was refused): `signed_out`.
    SignedOut,
    /// The vendor refused (a rate limit, the plan's quota, a bad request); the HTTP status when
    /// the CLI gave one, else 0.
    Refused { status: u16 },
    /// The vendor couldn't be reached.
    Unreachable,
}

/// A shared handle on a run's [`RunOutcome`], read by the host after the run.
pub type OutcomeHandle = Rc<Cell<RunOutcome>>;

/// The detector of Claude Code's `--output-format stream-json`.
#[derive(Debug)]
pub struct ClaudeStreamDetector {
    gate: Gate,
    outcome: OutcomeHandle,
}

impl ClaudeStreamDetector {
    /// A detector with a gate of its own and an outcome nobody reads (fixtures, fuzzing).
    pub fn new(gate: Gate) -> ClaudeStreamDetector {
        ClaudeStreamDetector { gate, outcome: Rc::new(Cell::new(RunOutcome::Clean)) }
    }

    /// A detector that reports its run's outcome into `outcome`.
    pub fn with_outcome(gate: Gate, outcome: OutcomeHandle) -> ClaudeStreamDetector {
        ClaudeStreamDetector { gate, outcome }
    }

    /// The handle the outcome is reported into.
    pub fn outcome(&self) -> OutcomeHandle {
        Rc::clone(&self.outcome)
    }

    /// Keeps the first error reported (an API error message comes before its `result`, and says
    /// more).
    fn report(&self, outcome: RunOutcome) {
        if self.outcome.get() == RunOutcome::Clean {
            self.outcome.set(outcome);
        }
    }

    fn init(v: &Value) -> Verdict {
        let empty_list = |key: &str| v.get(key).and_then(Value::as_array).map(Vec::is_empty);
        match (empty_list("tools"), empty_list("mcp_servers")) {
            // The skills, agents and slash-command name lists are not tools and don't kill.
            (Some(true), Some(true)) => Verdict::InitOk,
            (Some(false), _) => Verdict::Kill("init_lists_tool"),
            (_, Some(false)) => Verdict::Kill("init_lists_mcp"),
            _ => Verdict::Kill("init_malformed"),
        }
    }

    fn assistant(&self, v: &Value) -> Verdict {
        let Some(blocks) = v.get("message").and_then(|m| m.get("content")).and_then(Value::as_array) else {
            return Verdict::Kill("malformed");
        };
        let mut text = String::new();
        for block in blocks {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => match block.get("text").and_then(Value::as_str) {
                    Some(t) => text.push_str(t),
                    None => return Verdict::Kill("malformed"),
                },
                Some("thinking" | "redacted_thinking") => {}
                Some(kind) => return Verdict::Kill(block_kill_kind(kind, block)),
                None => return Verdict::Kill("unknown_block"),
            }
        }
        let api_error = v.get("error").is_some_and(|e| !e.is_null()) || v.get("is_api_error_message").and_then(Value::as_bool) == Some(true);
        if api_error {
            // The CLI's own words ("Not logged in · Please run /login") are never answer text.
            self.report(outcome_of_api_error(v.get("error").and_then(Value::as_str)));
            return Verdict::Note("api_error");
        }
        if text.is_empty() { Verdict::Note("thinking") } else { Verdict::Text(text) }
    }

    fn result(&self, v: &Value) -> Verdict {
        let is_error = v.get("is_error").and_then(Value::as_bool).unwrap_or(true) || v.get("subtype").and_then(Value::as_str) != Some("success");
        if !is_error {
            return match v.get("result").and_then(Value::as_str) {
                Some(text) => Verdict::Final(text.to_owned()),
                None => Verdict::Note("result_without_text"),
            };
        }
        let status = v.get("api_error_status").and_then(Value::as_u64).and_then(|s| u16::try_from(s).ok());
        self.report(outcome_of_result(v.get("result").and_then(Value::as_str), status));
        Verdict::Note("result_error")
    }
}

/// The kind a non-text content block kills with: a shell command, a file change, an MCP call, a
/// web search, any other tool, or a block nobody knows.
fn block_kill_kind(kind: &str, block: &Value) -> &'static str {
    let name = block.get("name").and_then(Value::as_str).unwrap_or("");
    match kind {
        "tool_use" => match name {
            "Bash" | "BashOutput" | "PowerShell" | "KillShell" | "KillBash" => "command",
            "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => "file_change",
            n if n.starts_with("mcp__") => "mcp_call",
            "WebSearch" | "WebFetch" => "web_search",
            _ => "tool_use",
        },
        "server_tool_use" | "web_search_tool_result" | "web_fetch_tool_result" => "web_search",
        "mcp_tool_use" | "mcp_tool_result" => "mcp_call",
        "tool_result" => "tool_result",
        _ => "unknown_block",
    }
}

/// An API error message's `error` code as an outcome.
fn outcome_of_api_error(code: Option<&str>) -> RunOutcome {
    match code {
        Some("authentication_failed" | "oauth_token_expired" | "unauthorized" | "permission_denied") => RunOutcome::SignedOut,
        Some("rate_limit" | "rate_limit_error") => RunOutcome::Refused { status: 429 },
        Some("billing_error") => RunOutcome::Refused { status: 402 },
        Some("server_error" | "overloaded" | "overloaded_error") => RunOutcome::Refused { status: 500 },
        Some("invalid_request") => RunOutcome::Refused { status: 400 },
        Some("network_error" | "connection_error") => RunOutcome::Unreachable,
        _ => RunOutcome::Refused { status: 0 },
    }
}

/// An error `result`'s words and HTTP status as an outcome. The words are looked at for a few
/// fixed phrases and then dropped.
fn outcome_of_result(words: Option<&str>, status: Option<u16>) -> RunOutcome {
    if matches!(status, Some(401 | 403)) {
        return RunOutcome::SignedOut;
    }
    let lower = words.map(|w| w.chars().take(512).collect::<String>().to_lowercase()).unwrap_or_default();
    let has = |needles: &[&str]| needles.iter().any(|n| lower.contains(n));
    if has(&["not logged in", "/login", "please log in", "authentication", "invalid api key", "oauth token", "unauthorized"]) {
        return RunOutcome::SignedOut;
    }
    if let Some(s) = status {
        return RunOutcome::Refused { status: s };
    }
    if has(&["connection error", "unable to connect", "econnrefused", "enotfound", "network", "getaddrinfo", "timed out"]) {
        return RunOutcome::Unreachable;
    }
    RunOutcome::Refused { status: 0 }
}

impl StreamDetector for ClaudeStreamDetector {
    fn gate(&self) -> Gate {
        self.gate
    }

    fn line(&mut self, bytes: &[u8]) -> Verdict {
        let Ok(v) = serde_json::from_slice::<Value>(bytes) else {
            return Verdict::Kill("unparsable");
        };
        if !v.is_object() {
            return Verdict::Kill("unparsable");
        }
        match (v.get("type").and_then(Value::as_str), v.get("subtype").and_then(Value::as_str)) {
            (Some("system"), Some("init")) => Self::init(&v),
            (Some("system"), Some("api_retry")) => Verdict::Note("api_retry"),
            (Some("system"), _) => Verdict::Kill("unknown_system"),
            (Some("assistant"), _) => self.assistant(&v),
            // A user event in this stream is a tool's result: a tool ran.
            (Some("user"), _) => Verdict::Kill("tool_result"),
            (Some("rate_limit_event"), _) => Verdict::Note("rate_limit_event"),
            (Some("result"), _) => self.result(&v),
            _ => Verdict::Kill("unknown_event"),
        }
    }
}

#[cfg(test)]
#[path = "claude_code_tests.rs"]
mod claude_code_tests;
