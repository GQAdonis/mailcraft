// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Universal Agent Runtime: AG-UI wire types and capped stream parser (B9).
//!
//! Everything here is pure and builds everywhere (wasm included); the host
//! (`apps/mailcraft/src/ai/runtime/uar_sidecar.rs`) does the I/O.
//!
//! - **The entry check** ([`ManagedSupport`]): a managed instance is offered only when the
//!   installed program's `--help` shows all four of a config path, a port, a launch token read on
//!   stdin and a readiness line. `universal-agent-runtime` 1.4.0-rc.5 shows the first two only, so
//!   for it UAR is external-only. No argv token, fixed shared port or polled readiness is taken in
//!   their place.
//! - **The managed config** ([`ManagedConfig`]): native file, web and terminal tools off, session
//!   search off, skills, skill evolution, ACP, failover and shared MCP transports off, and exactly
//!   one model endpoint, the one MailCraft chose. It never holds a key (a key never goes in a file).
//!   It is the only source of the managed-config fact that `Where` is derived from
//!   ([`ManagedConfig::endpoints`]); nothing UAR reports can make an instance `local`, and a
//!   model it reports beyond the chosen one makes it `leaves` ([`models_beyond`]).
//! - **The handshake**: a 32-byte launch token on stdin ([`TOKEN_BYTES`]), then `READY:<port>` as
//!   the first line within 20 s ([`parse_ready`], [`READY_TIMEOUT`]), then a capability check
//!   ([`capabilities_ok`]).
//! - **AG-UI** ([`AgUiDetector`]): only `TEXT_MESSAGE_CONTENT` (and a chunk's delta) and
//!   `RUN_FINISHED` are used. A tool-call event of any kind, and any event that isn't plain text or
//!   one of the few inert lifecycle events, kills the run through B4's judge. Only the event's
//!   kind is ever logged: kinds are `&'static str`.
//! - **Discovery** ([`parse_card`]): an external instance's agent card gives its name and version
//!   as untrusted text, capped and stripped of control characters. Nothing in it says where text
//!   goes.

use std::time::Duration;

use serde_json::{Map, Value, json};

use super::contain::{Gate, MAX_ANSWER_BYTES, StreamDetector, Verdict};

/// How long a managed instance has to print its readiness line.
pub const READY_TIMEOUT: Duration = Duration::from_secs(20);
/// The launch token's length in random bytes (written on stdin as 64 hexadecimal digits).
pub const TOKEN_BYTES: usize = 32;
/// The longest readiness line taken (`READY:65535` is 11 bytes).
pub const MAX_READY_LINE: usize = 32;
/// The AG-UI profile a managed instance must offer.
pub const AGUI_PROFILE: &str = "uar.agui/1";
/// Where runs are posted.
pub const RUNS_PATH: &str = "/api/uar/runs";
/// Where the capabilities are read.
pub const CAPABILITIES_PATH: &str = "/api/uar/capabilities";
/// Where the models an instance offers are listed.
pub const MODELS_PATH: &str = "/v1/models";
/// The port an external instance listens on.
pub const EXTERNAL_PORT: u16 = 1906;
/// The agent card's path.
pub const CARD_PATH: &str = "/.well-known/agent-card.json";
/// The most characters of an agent card's name or version shown.
pub const MAX_CARD_TEXT: usize = 64;
/// The longest model endpoint address taken.
pub const MAX_URL: usize = 2048;
/// The longest model name taken.
pub const MAX_MODEL: usize = 128;
/// The longest tool name looked at when a tool event is classed.
const MAX_TOOL_NAME: usize = 256;

/// What Settings says of an external instance (B11 shows it through `tl!`).
pub const EXTERNAL_CAVEAT: &str = "MailCraft didn't start this Universal Agent Runtime and can't turn off its tools. \
Whatever it is sent leaves this computer, as far as MailCraft can tell, and it is used only for the accounts you allowed. \
MailCraft stops any answer in which it tries to use a tool.";

/// An instance's status when it answers 401 or 403 (B11 shows it through `tl!`).
pub const REFUSED_STATUS: &str = "UAR refused MailCraft";

/// Why the managed config couldn't be written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UarError {
    /// The provider's id isn't one.
    BadProvider,
    /// The model's name is empty, too long, or has a character that isn't allowed.
    BadModel,
    /// The address isn't `https://`, or `http://` to this computer, or can't be carried.
    BadUrl,
    /// The request couldn't be written.
    Encode,
}

impl UarError {
    /// The error's kind, the only thing of it that is logged.
    pub fn kind(self) -> &'static str {
        match self {
            UarError::BadProvider => "uar_bad_provider",
            UarError::BadModel => "uar_bad_model",
            UarError::BadUrl => "uar_bad_url",
            UarError::Encode => "uar_encode",
        }
    }
}

impl std::fmt::Display for UarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.kind())
    }
}

impl std::error::Error for UarError {}

// ---------------------------------------------------------------------------------------------
// The entry check (19.1)
// ---------------------------------------------------------------------------------------------

/// What an installed program's `--help` says it can do for a managed instance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ManagedSupport {
    /// It takes a config file's path (`--config`).
    pub config_path: bool,
    /// It takes the port to listen on (`--port`; MailCraft passes 0).
    pub port: bool,
    /// It reads a launch token on stdin.
    pub stdin_token: bool,
    /// It prints `READY:<port>` once it listens.
    pub readiness: bool,
}

impl ManagedSupport {
    /// What `help` (the program's `--help`, capped by the host) shows. An option counts only on
    /// a line that names it as an option.
    pub fn from_help(help: &str) -> ManagedSupport {
        let mut s = ManagedSupport::default();
        for line in help.lines().take(2000) {
            let l = line.trim_start();
            if !l.starts_with('-') {
                continue;
            }
            let lower = l.to_ascii_lowercase();
            s.config_path |= lower.contains("--config");
            s.port |= lower.contains("--port");
            s.stdin_token |= lower.contains("stdin") && lower.contains("token");
            s.readiness |= l.contains("READY:");
        }
        s
    }

    /// Whether a managed instance may be offered: all four, or none of it.
    pub fn offered(&self) -> bool {
        self.config_path && self.port && self.stdin_token && self.readiness
    }
}

/// The port of a readiness line: exactly `READY:<port>`, the port 1 to 65535 in decimal digits.
/// Anything else is `None`.
pub fn parse_ready(line: &[u8]) -> Option<u16> {
    if line.len() > MAX_READY_LINE {
        return None;
    }
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let digits = line.strip_prefix(b"READY:")?;
    if digits.is_empty() || digits.len() > 5 || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let port: u32 = std::str::from_utf8(digits).ok()?.parse().ok()?;
    u16::try_from(port).ok().filter(|p| *p != 0)
}

/// The launch token as it is written on stdin and sent as the bearer: the bytes in lowercase
/// hexadecimal.
pub fn token_text(bytes: &[u8; TOKEN_BYTES]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(TOKEN_BYTES * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

// ---------------------------------------------------------------------------------------------
// The managed config (19.3)
// ---------------------------------------------------------------------------------------------

/// The one model endpoint a managed instance is configured with: a MailCraft `compat` provider.
#[derive(Clone, PartialEq, Eq)]
pub struct ManagedConfig {
    /// The `compat` provider's id.
    pub provider: String,
    /// The model's name on that provider.
    pub model: String,
    /// The provider's OpenAI-compatible address.
    pub base_url: String,
}

impl std::fmt::Debug for ManagedConfig {
    /// The provider's id alone: no address, no model.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ManagedConfig").field("provider", &self.provider).finish_non_exhaustive()
    }
}

fn valid_provider(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

fn valid_model(name: &str) -> bool {
    !name.is_empty() && name.len() <= MAX_MODEL && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'/' | b'-'))
}

/// Whether `url` may be a model endpoint: `https://`, or `http://` to a loopback host, printable
/// ASCII without a user name, query or fragment.
fn valid_url(url: &str) -> bool {
    if url.is_empty() || url.len() > MAX_URL || !url.bytes().all(|b| b.is_ascii_graphic()) || url.contains(['?', '#', '@', '"', '\\']) {
        return false;
    }
    let Some((scheme, rest)) = url.split_once("://") else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or("");
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split_once(']').map_or("", |(h, _)| h),
        None => authority.split_once(':').map_or(authority, |(h, _)| h),
    };
    if host.is_empty() {
        return false;
    }
    if scheme.eq_ignore_ascii_case("https") {
        true
    } else if scheme.eq_ignore_ascii_case("http") {
        crate::config::is_loopback_host(host)
    } else {
        false
    }
}

impl ManagedConfig {
    /// The config, checked.
    ///
    /// # Errors
    /// [`UarError`] for an id, a model or an address that can't be used.
    pub fn new(provider: &str, model: &str, base_url: &str) -> Result<ManagedConfig, UarError> {
        if !valid_provider(provider) {
            return Err(UarError::BadProvider);
        }
        if !valid_model(model) {
            return Err(UarError::BadModel);
        }
        let base_url = base_url.trim().trim_end_matches('/');
        if !valid_url(base_url) {
            return Err(UarError::BadUrl);
        }
        Ok(ManagedConfig { provider: provider.to_owned(), model: model.to_owned(), base_url: base_url.to_owned() })
    }

    /// The model endpoints this config names, by provider id: always exactly one. This is the
    /// managed-config fact `Where` is derived from (the host adds whether it verified the peer).
    pub fn endpoints(&self) -> Vec<&str> {
        vec![self.provider.as_str()]
    }

    /// The config as the instance reads it (JSON, which is YAML too). No key is ever written.
    pub fn to_json(&self) -> Value {
        json!({
            "server": {"host": "127.0.0.1", "port": 0, "mcp_shared_transports_enabled": false},
            "native_tools": {
                "file_tools_enabled": false,
                "file_allowed_paths": [],
                "web_fetch_enabled": false,
                "terminal_exec_enabled": false,
                "session_search_enabled": false
            },
            "skill_source_roots": [],
            "skill_evolution": {"enabled": false},
            "acp": {"enabled": false},
            "failover": {"enabled": false},
            "llm": {
                "model": format!("openai/{}", self.model),
                "base_url": self.base_url,
                "api_key": null,
                "api_key_env": null
            }
        })
    }

    /// [`Self::to_json`]'s bytes.
    ///
    /// # Errors
    /// [`UarError::Encode`] if it couldn't be written.
    pub fn to_bytes(&self) -> Result<Vec<u8>, UarError> {
        serde_json::to_vec_pretty(&self.to_json()).map_err(|_| UarError::Encode)
    }
}

/// Whether the managed instance's capabilities (`GET /api/uar/capabilities`) are a JSON object
/// offering [`AGUI_PROFILE`]. Anything else refuses the instance.
pub fn capabilities_ok(body: &[u8]) -> bool {
    let Ok(v) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    v.get("agui").and_then(|a| a.get("profile")).and_then(Value::as_str) == Some(AGUI_PROFILE)
}

/// Whether the instance's model list (`GET /v1/models`, OpenAI's shape) names any model but
/// `model` (with or without the `openai/` it was configured with). A list that can't be read
/// counts as naming others: the instance then leaves.
pub fn models_beyond(body: &[u8], model: &str) -> bool {
    let Ok(v) = serde_json::from_slice::<Value>(body) else {
        return true;
    };
    let Some(data) = v.get("data").and_then(Value::as_array) else {
        return true;
    };
    let configured = format!("openai/{model}");
    data.iter().take(10_000).any(|m| match m.get("id").and_then(Value::as_str) {
        Some(id) => id != model && id != configured,
        None => true,
    })
}

// ---------------------------------------------------------------------------------------------
// Runs
// ---------------------------------------------------------------------------------------------

/// The body of `POST /api/uar/runs`: AG-UI's run input, with no tools, no context and no state.
/// The instructions and the prompt are messages, never anything that runs.
///
/// # Errors
/// [`UarError::Encode`] if it couldn't be written.
pub fn run_body(run_id: &str, instructions: &str, prompt: &str) -> Result<Vec<u8>, UarError> {
    let body = json!({
        "threadId": run_id,
        "runId": run_id,
        "messages": [
            {"id": "mailcraft-instructions", "role": "system", "content": instructions},
            {"id": "mailcraft-prompt", "role": "user", "content": prompt}
        ],
        "tools": [],
        "context": [],
        "state": {},
        "forwardedProps": {}
    });
    serde_json::to_vec(&body).map_err(|_| UarError::Encode)
}

// ---------------------------------------------------------------------------------------------
// AG-UI (19.2)
// ---------------------------------------------------------------------------------------------

/// The kind a tool event is logged as, from the tool's name: an MCP call, a command, a file
/// change, a web search, or another tool use. Only the returned kind is ever logged.
pub fn tool_kind(name: &str) -> &'static str {
    let cut = name.char_indices().nth(MAX_TOOL_NAME).map_or(name, |(i, _)| name.get(..i).unwrap_or(name));
    let n = cut.to_ascii_lowercase();
    let any = |words: &[&str]| words.iter().any(|w| n.contains(w));
    if n.starts_with("mcp") || n.contains("__") {
        "mcp_call"
    } else if any(&["terminal", "exec", "shell", "bash", "command"]) {
        "command"
    } else if any(&["write", "patch", "edit", "delete", "create_file", "move_file"]) {
        "file_change"
    } else if any(&["web", "search", "fetch", "browse", "http"]) {
        "web_search"
    } else {
        "tool_use"
    }
}

/// Reads an AG-UI stream, event by event or (from a pipe) line by line, for B4's judge. Every
/// event is hostile.
#[derive(Debug, Default)]
pub struct AgUiDetector {
    /// The answer so far, kept so that `RUN_FINISHED` hands it back whole (capped).
    text: String,
}

impl AgUiDetector {
    pub fn new() -> AgUiDetector {
        AgUiDetector::default()
    }

    fn push(&mut self, delta: &str) {
        if self.text.len().saturating_add(delta.len()) <= MAX_ANSWER_BYTES {
            self.text.push_str(delta);
        }
    }

    /// What one AG-UI event (its JSON, an SSE event's data) is.
    pub fn event(&mut self, data: &[u8]) -> Verdict {
        let Ok(value) = serde_json::from_slice::<Value>(data) else {
            return Verdict::Kill("unparsable");
        };
        let Some(obj) = value.as_object() else {
            return Verdict::Kill("unparsable");
        };
        let Some(kind) = obj.get("type").and_then(Value::as_str) else {
            return Verdict::Kill("unparsable");
        };
        self.classify(kind, obj)
    }

    fn classify(&mut self, kind: &str, obj: &Map<String, Value>) -> Verdict {
        let delta = obj.get("delta");
        match kind {
            "TEXT_MESSAGE_CONTENT" => match delta.and_then(Value::as_str) {
                Some(d) => {
                    self.push(d);
                    Verdict::Text(d.to_owned())
                }
                None => Verdict::Kill("unparsable"),
            },
            "TEXT_MESSAGE_CHUNK" => match delta {
                None | Some(Value::Null) => Verdict::Note("text_message_chunk"),
                Some(Value::String(d)) => {
                    self.push(d);
                    Verdict::Text(d.clone())
                }
                Some(_) => Verdict::Kill("unparsable"),
            },
            "RUN_FINISHED" => {
                if self.text.is_empty() {
                    Verdict::Note("run_finished_empty")
                } else {
                    Verdict::Final(std::mem::take(&mut self.text))
                }
            }
            "RUN_STARTED" => Verdict::Note("run_started"),
            "TEXT_MESSAGE_START" => Verdict::Note("text_message_start"),
            "TEXT_MESSAGE_END" => Verdict::Note("text_message_end"),
            "STEP_STARTED" => Verdict::Note("step_started"),
            "STEP_FINISHED" => Verdict::Note("step_finished"),
            // Thinking is text: not shown, not used, not a tool.
            "THINKING_START" | "THINKING_END" | "THINKING_TEXT_MESSAGE_START" | "THINKING_TEXT_MESSAGE_CONTENT" | "THINKING_TEXT_MESSAGE_END" => {
                Verdict::Note("thinking")
            }
            "RUN_ERROR" => Verdict::Kill("run_error"),
            "TOOL_CALL_START" | "TOOL_CALL_CHUNK" => Verdict::Kill(obj.get("toolCallName").and_then(Value::as_str).map_or("tool_use", tool_kind)),
            "TOOL_CALL_ARGS" | "TOOL_CALL_END" | "TOOL_CALL_RESULT" => Verdict::Kill("tool_use"),
            // State, snapshots, raw and custom events, activity, and anything this version doesn't
            // know: none of it is plain text.
            _ => Verdict::Kill("unknown_non_text"),
        }
    }
}

impl StreamDetector for AgUiDetector {
    fn gate(&self) -> Gate {
        Gate::Now
    }

    /// One line of an AG-UI stream read from a pipe: SSE framing (`data:` lines, comments and
    /// other fields) or a bare JSON event.
    fn line(&mut self, bytes: &[u8]) -> Verdict {
        if bytes.first() == Some(&b':') {
            return Verdict::Note("sse_comment");
        }
        if let Some(rest) = bytes.strip_prefix(b"data:") {
            return self.event(rest.strip_prefix(b" ").unwrap_or(rest));
        }
        if [&b"event:"[..], b"id:", b"retry:"].iter().any(|p| bytes.starts_with(p)) {
            return Verdict::Note("sse_field");
        }
        self.event(bytes)
    }
}

// ---------------------------------------------------------------------------------------------
// Discovery (19.5)
// ---------------------------------------------------------------------------------------------

/// An external instance's agent card, as shown: untrusted text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    pub name: String,
    pub version: String,
}

/// `text` without control characters, at most [`MAX_CARD_TEXT`] characters.
pub fn untrusted_text(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() && !matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
        .take(MAX_CARD_TEXT)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// The agent card's name and version, when `body` is a JSON object; `None` for anything else.
pub fn parse_card(body: &[u8]) -> Option<Card> {
    let v = serde_json::from_slice::<Value>(body).ok()?;
    let obj = v.as_object()?;
    let field = |k: &str| obj.get(k).and_then(Value::as_str).map(untrusted_text).unwrap_or_default();
    Some(Card { name: field("name"), version: field("version") })
}

#[cfg(test)]
#[path = "uar_tests.rs"]
mod uar_tests;
