// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tests and fixtures for the W7 adapters (uar_fixtures): the AG-UI stream of SCN-RT-CONTAIN-001
//! (B9, 19.7), one line per tool class and an unknown non-text event, in the SSE framing an AG-UI
//! server sends (`data:` lines and blank lines). The integrator adds [`FIXTURES`] to
//! `contain_tests::registry()`.

#![cfg(unix)]

use mailcraft_ai::runtime::contain::{Gate, StreamDetector, ToolClass};
use mailcraft_ai::runtime::uar::AgUiDetector;

fn detector(_gate: Gate) -> Box<dyn StreamDetector> {
    // AG-UI has no init event to gate on: the prompt goes in the run's request.
    Box::new(AgUiDetector::new())
}

pub(crate) const FIXTURES: super::contain_tests::FormatFixtures = super::contain_tests::FormatFixtures {
    format: "ag_ui",
    detector,
    text_only: &[
        ": keep-alive",
        r#"data: {"type":"RUN_STARTED","threadId":"t-1","runId":"r-1"}"#,
        "",
        r#"data: {"type":"TEXT_MESSAGE_START","messageId":"m-1","role":"assistant"}"#,
        "",
        r#"data: {"type":"TEXT_MESSAGE_CONTENT","messageId":"m-1","delta":"The meeting "}"#,
        "",
        r#"data: {"type":"TEXT_MESSAGE_CONTENT","messageId":"m-1","delta":"is on Friday."}"#,
        "",
        r#"data: {"type":"TEXT_MESSAGE_END","messageId":"m-1"}"#,
        "",
        r#"data: {"type":"RUN_FINISHED","threadId":"t-1","runId":"r-1"}"#,
    ],
    answer: "The meeting is on Friday.",
    text_line: r#"data: {"type":"TEXT_MESSAGE_CONTENT","messageId":"m-1","delta":"PARTIAL-ANSWER-MARKER about the meeting"}"#,
    text_marker: "PARTIAL-ANSWER-MARKER",
    classes: &[
        (ToolClass::ToolUse, r#"data: {"type":"TOOL_CALL_START","toolCallId":"c-1","toolCallName":"file_read","parentMessageId":"m-1"}"#),
        (ToolClass::Command, r#"data: {"type":"TOOL_CALL_START","toolCallId":"c-2","toolCallName":"terminal_exec"}"#),
        (ToolClass::FileChange, r#"data: {"type":"TOOL_CALL_START","toolCallId":"c-3","toolCallName":"file_write"}"#),
        (ToolClass::McpCall, r#"data: {"type":"TOOL_CALL_START","toolCallId":"c-4","toolCallName":"mcp__mail__send"}"#),
        (ToolClass::WebSearch, r#"data: {"type":"TOOL_CALL_START","toolCallId":"c-5","toolCallName":"web_fetch"}"#),
        (ToolClass::UnknownNonText, r#"data: {"type":"STATE_DELTA","delta":[{"op":"add","path":"/plan","value":"read ~/.ssh"}]}"#),
    ],
};
