// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Codex JSONL fixtures for SCN-RT-CONTAIN-001 (B6, 16.9): one item per tool class, one item type
//! Codex doesn't have, a text-only stream with the error items a real owner's `CODEX_HOME` makes
//! Codex write (malformed agent roles, a transport fallback), and a run that fails for want of a
//! sign-in. Shaped like Codex 0.169's `exec --json`; every word in them is made up.

#![cfg(unix)]

use mailcraft_ai::runtime::codex::CodexDetector;
use mailcraft_ai::runtime::contain::{Gate, StreamDetector, ToolClass};

use super::contain_tests::FormatFixtures;

fn detector(_gate: Gate) -> Box<dyn StreamDetector> {
    // Codex's detector always writes the prompt at once; it has no init event to wait for.
    Box::new(CodexDetector::new())
}

/// The text-only stream's error items carry this; it must never reach an answer.
pub(crate) const ERROR_ITEM_MARKER: &str = "ERRORITEM-MARKER";

pub(crate) const FIXTURES: FormatFixtures = FormatFixtures {
    format: "codex_jsonl",
    detector,
    text_only: &[
        r#"{"type":"thread.started","thread_id":"01a1f184-0000-7000-8000-000000000001"}"#,
        r#"{"type":"turn.started"}"#,
        r#"{"type":"item.completed","item":{"id":"item_0","type":"error","message":"ERRORITEM-MARKER Ignoring malformed agent role definition: agent role file at /home/owner/.codex/agents/a.toml must define a description"}}"#,
        r#"{"type":"item.completed","item":{"id":"item_1","type":"error","message":"ERRORITEM-MARKER Ignoring malformed agent role definition: agent role file at /home/owner/.codex/agents/b.toml must define a description"}}"#,
        r#"{"type":"item.completed","item":{"id":"item_2","type":"reasoning","text":"The message says Friday."}}"#,
        r#"{"type":"item.started","item":{"id":"item_3","type":"agent_message","text":""}}"#,
        r#"{"type":"item.updated","item":{"id":"item_3","type":"agent_message","text":"The meeting "}}"#,
        r#"{"type":"item.completed","item":{"id":"item_3","type":"agent_message","text":"The meeting is on Friday."}}"#,
        r#"{"type":"turn.completed","usage":{"input_tokens":120,"cached_input_tokens":0,"output_tokens":7}}"#,
    ],
    answer: "The meeting is on Friday.",
    text_line: r#"{"type":"item.updated","item":{"id":"item_1","type":"agent_message","text":"PARTIAL-ANSWER-MARKER about the meeting"}}"#,
    text_marker: "PARTIAL-ANSWER-MARKER",
    classes: &[
        // Codex has no generic tool-use item; its plan tool is the one that acts on nothing else.
        (
            ToolClass::ToolUse,
            r#"{"type":"item.started","item":{"id":"item_4","type":"todo_list","items":[{"text":"read the inbox","completed":false}]}}"#,
        ),
        (
            ToolClass::Command,
            r#"{"type":"item.started","item":{"id":"item_4","type":"command_execution","command":"ls ~","aggregated_output":"","status":"in_progress"}}"#,
        ),
        (
            ToolClass::FileChange,
            r#"{"type":"item.completed","item":{"id":"item_4","type":"file_change","changes":[{"path":"notes.txt","kind":"add"}],"status":"completed"}}"#,
        ),
        (
            ToolClass::McpCall,
            r#"{"type":"item.started","item":{"id":"item_4","type":"mcp_tool_call","server":"mail","tool":"send","status":"in_progress"}}"#,
        ),
        (ToolClass::WebSearch, r#"{"type":"item.started","item":{"id":"item_4","type":"web_search","query":"example.org"}}"#),
        (ToolClass::UnknownNonText, r#"{"type":"item.started","item":{"id":"item_4","type":"collab_agent_spawn","prompt":"…"}}"#),
    ],
};

/// An event type Codex doesn't have, at the top level.
pub(crate) const UNKNOWN_EVENT: &str = r#"{"type":"session.configured","model":"x"}"#;

/// A run that fails because Codex isn't signed in (Codex exits 1 after it).
pub(crate) const SIGNED_OUT: &[&str] = &[
    r#"{"type":"thread.started","thread_id":"01a1f186-0000-7000-8000-000000000002"}"#,
    r#"{"type":"turn.started"}"#,
    r#"{"type":"error","message":"Reconnecting... 1/5 (unexpected status 401 Unauthorized: Missing bearer or basic authentication in header, url: https://api.example.org/v1/responses)"}"#,
    r#"{"type":"item.completed","item":{"id":"item_0","type":"error","message":"Falling back from WebSockets to HTTPS transport. unexpected status 401 Unauthorized"}}"#,
    r#"{"type":"turn.failed","error":{"message":"unexpected status 401 Unauthorized: Missing bearer or basic authentication in header, url: https://api.example.org/v1/responses"}}"#,
];
