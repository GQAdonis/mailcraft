// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Claude Code stream-json fixtures for SCN-RT-CONTAIN-001 (B5, task 15.9): one line per tool
//! class and an unknown kind, a text-only stream, an init that lists a tool, and the
//! not-logged-in end. The shapes are those Claude Code 2.1.326 wrote in the G-RUNS evidence;
//! the text is synthetic. The integrator adds [`FIXTURES`] to `contain_tests::registry()`.

#![cfg(unix)]
#![allow(dead_code)] // Read by contain_tests once registered, and by claude_auth_tests.

use mailcraft_ai::runtime::claude_code::ClaudeStreamDetector;
use mailcraft_ai::runtime::contain::{Gate, StreamDetector, ToolClass};

use super::contain_tests::FormatFixtures;

fn detector(gate: Gate) -> Box<dyn StreamDetector> {
    Box::new(ClaudeStreamDetector::new(gate))
}

/// An init with no tool and no MCP server, naming skills and agents (which don't kill).
pub(crate) const INIT: &str = r#"{"type":"system","subtype":"init","cwd":"/tmp/work","session_id":"00000000-0000-0000-0000-000000000000","tools":[],"mcp_servers":[],"model":"claude-opus-5-5","permissionMode":"default","slash_commands":["deep-research","design"],"apiKeySource":"none","claude_code_version":"2.1.326","agents":["claude","Explore","general-purpose","Plan"],"skills":["deep-research","design"],"plugins":[{"name":"cc-plugin-privacy","path":"builtin"}],"memory_paths":{}}"#;

/// An init that lists a tool: killed before the prompt (under the gate).
pub(crate) const INIT_WITH_TOOL: &str = r#"{"type":"system","subtype":"init","tools":["Bash"],"mcp_servers":[],"apiKeySource":"none","claude_code_version":"2.1.326","skills":[],"agents":[]}"#;

/// The answer of the text-only stream.
pub(crate) const ANSWER: &str = "The meeting is on Friday.";

/// A text-only stream: init, two assistant messages, a rate-limit note, the result.
pub(crate) const TEXT_ONLY: &[&str] = &[
    INIT,
    r#"{"type":"assistant","message":{"model":"claude-opus-5-5","id":"msg_1","type":"message","role":"assistant","content":[{"type":"thinking","thinking":"The message says Friday."},{"type":"text","text":"The meeting "}]},"parent_tool_use_id":null}"#,
    r#"{"type":"assistant","message":{"model":"claude-opus-5-5","id":"msg_1","type":"message","role":"assistant","content":[{"type":"text","text":"is on Friday."}]},"parent_tool_use_id":null}"#,
    r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","rateLimitType":"seven_day"}}"#,
    r#"{"type":"result","subtype":"success","is_error":false,"num_turns":1,"result":"The meeting is on Friday.","stop_reason":"end_turn"}"#,
];

/// The not-logged-in end of a run (the CLI's own words, never shown).
pub(crate) const NOT_LOGGED_IN: &[&str] = &[
    INIT,
    r#"{"type":"assistant","message":{"id":"eb813539","model":"<synthetic>","role":"assistant","type":"message","content":[{"type":"text","text":"Not logged in · Please run /login"}]},"parent_tool_use_id":null,"error":"authentication_failed","is_api_error_message":true}"#,
    r#"{"type":"result","subtype":"success","is_error":true,"num_turns":1,"result":"Not logged in · Please run /login","terminal_reason":"api_error"}"#,
];

/// The registry entry.
pub(crate) const FIXTURES: FormatFixtures = FormatFixtures {
    format: "claude_stream_json",
    detector,
    text_only: TEXT_ONLY,
    answer: ANSWER,
    text_line: r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"PARTIAL-ANSWER-MARKER about the meeting"}]}}"#,
    text_marker: "PARTIAL-ANSWER-MARKER",
    classes: &[
        (
            ToolClass::ToolUse,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"Read","input":{"file_path":"/etc/hosts"}}]}}"#,
        ),
        (
            ToolClass::Command,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_2","name":"Bash","input":{"command":"ls ~"}}]}}"#,
        ),
        (
            ToolClass::FileChange,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_3","name":"Edit","input":{"file_path":"notes.txt","old_string":"a","new_string":"b"}}]}}"#,
        ),
        (
            ToolClass::McpCall,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_4","name":"mcp__mail__send","input":{"to":"bob@example.org"}}]}}"#,
        ),
        (
            ToolClass::WebSearch,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"server_tool_use","id":"srvtoolu_5","name":"web_search","input":{"query":"example.org"}}]}}"#,
        ),
        (ToolClass::UnknownNonText, r#"{"type":"telemetry_snapshot_v9","data":{"k":1}}"#),
    ],
};
