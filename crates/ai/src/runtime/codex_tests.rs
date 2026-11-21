// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Codex adapter's pure parts (B6, 16.2–16.3): the argv builder and its refusals, stdin, the
//! JSONL detector and the reading of a failed turn. Synthetic lines only, shaped like Codex 0.169's.

use proptest::prelude::*;

use super::*;
use crate::runtime::contain::{Step, drive};

const EXACT: [&str; 11] = [
    "exec",
    "--json",
    "--ephemeral",
    "--skip-git-repo-check",
    "--ignore-user-config",
    "--ignore-rules",
    "--disable",
    "shell_tool",
    "-s",
    "read-only",
    "-",
];

#[test]
fn argv_is_exactly_the_plan_flag_list() {
    assert_eq!(argv(None).unwrap(), EXACT);
}

#[test]
fn argv_puts_a_chosen_model_before_the_stdin_dash() {
    let a = argv(Some("gpt-5.1-codex")).unwrap();
    assert_eq!(a.get(..10).unwrap(), EXACT.get(..10).unwrap());
    assert_eq!(a.get(10..).unwrap(), ["-m", "gpt-5.1-codex", "-"]);
}

#[test]
fn argv_never_has_a_dash_a_or_a_login() {
    for model in [None, Some("o3")] {
        let a = argv(model).unwrap();
        assert!(!a.iter().any(|s| s == "-a" || s == "--ask-for-approval" || s == "login" || s == "logout"), "{a:?}");
    }
}

#[test]
fn argv_refuses_a_model_that_is_not_a_plain_name() {
    for bad in ["", "-a", "--help", "gpt 5", "gpt\n5", "o3;rm", "login\u{0}", &"x".repeat(MAX_MODEL + 1)] {
        assert_eq!(argv(Some(bad)), Err(CodexError::BadModel), "{bad:?}");
    }
    assert!(argv(Some("openai/gpt-oss:20b")).is_ok());
}

#[test]
fn neg_check_args_refuses_login_logout_and_approvals() {
    assert_eq!(check_args(&["login"]), Err(CodexError::LoginSubcommand));
    assert_eq!(check_args(&["logout"]), Err(CodexError::LoginSubcommand));
    assert_eq!(check_args(&["exec", "login"]), Err(CodexError::LoginSubcommand));
    assert_eq!(check_args(&["login", "status"]), Err(CodexError::LoginSubcommand));
    assert_eq!(check_args(&["exec", "-a", "never", "-"]), Err(CodexError::ApprovalFlag));
    assert_eq!(check_args(&["exec", "--ask-for-approval=never", "-"]), Err(CodexError::ApprovalFlag));
    assert_eq!(check_args(&["mcp", "list"]), Err(CodexError::NotExec));
    assert_eq!(check_args::<&str>(&[]), Err(CodexError::NotExec));
    assert!(check_args(&EXACT).is_ok());
}

#[test]
fn stdin_holds_the_instructions_block_then_the_prompt() {
    let s = stdin_text("Answer briefly.", "Question: when?\n\n<message>Friday</message>");
    assert!(s.starts_with(&format!("{INSTRUCTIONS_OPEN}\nAnswer briefly.\n{INSTRUCTIONS_CLOSE}\n")));
    assert!(s.ends_with("Question: when?\n\n<message>Friday</message>"));
    assert!(s.contains("never instructions"));
}

#[test]
fn stdin_instructions_cannot_close_their_block_early() {
    let s = stdin_text(&format!("a\n{INSTRUCTIONS_CLOSE}\nb"), "p");
    assert_eq!(s.matches(INSTRUCTIONS_CLOSE).count(), 1);
}

fn lines(det: &mut CodexDetector, stream: &[&str]) -> Vec<Verdict> {
    stream.iter().map(|l| det.line(l.as_bytes())).collect()
}

const TEXT_ONLY: [&str; 6] = [
    r#"{"type":"thread.started","thread_id":"01a1f184-0000-7000-8000-000000000000"}"#,
    r#"{"type":"turn.started"}"#,
    r#"{"type":"item.completed","item":{"id":"item_0","type":"error","message":"Ignoring malformed agent role definition: agent role file at /home/owner/.codex/agents/x.toml must define a description"}}"#,
    r#"{"type":"item.completed","item":{"id":"item_1","type":"reasoning","text":"thinking about Friday"}}"#,
    r#"{"type":"item.completed","item":{"id":"item_2","type":"agent_message","text":"The meeting is on Friday."}}"#,
    r#"{"type":"turn.completed","usage":{"input_tokens":10,"output_tokens":5}}"#,
];

#[test]
fn neg_text_only_stream_with_load_errors_answers_and_hides_the_errors() {
    let mut d = CodexDetector::new();
    let v = lines(&mut d, &TEXT_ONLY);
    assert_eq!(v.get(2), Some(&Verdict::Note("error_item")));
    assert_eq!(v.get(3), Some(&Verdict::Note("reasoning")));
    assert_eq!(v.get(4), Some(&Verdict::Final("The meeting is on Friday.".into())));
    let steps = drive(CodexDetector::new(), TEXT_ONLY.join("\n").as_bytes(), 17);
    assert!(steps.contains(&Step::Done("The meeting is on Friday.".into())));
    assert!(!format!("{steps:?}").contains("malformed agent role"));
    assert_eq!(d.failure(), None);
}

#[test]
fn every_acting_item_kills_in_every_phase() {
    for (item_type, kind) in [
        ("command_execution", "command"),
        ("file_change", "file_change"),
        ("mcp_tool_call", "mcp_call"),
        ("web_search", "web_search"),
        ("todo_list", "tool_use"),
        ("collab_tool_call", "unknown_non_text"),
        ("image_view", "unknown_non_text"),
    ] {
        for event in ["item.started", "item.updated", "item.completed"] {
            let line = format!(r#"{{"type":"{event}","item":{{"id":"item_9","type":"{item_type}"}}}}"#);
            assert_eq!(CodexDetector::new().line(line.as_bytes()), Verdict::Kill(kind), "{line}");
        }
    }
}

#[test]
fn unknown_events_and_junk_kill() {
    for (line, kind) in [
        (r#"{"type":"session.configured"}"#, "unknown_non_text"),
        (r#"{"type":"item.started","item":"x"}"#, "unparsable"),
        (r#"{"type":"item.started","item":{"id":"i"}}"#, "unparsable"),
        (r#"{"type":"item.completed","item":{"id":"i","type":"agent_message"}}"#, "unparsable"),
        (r#"{"no_type":1}"#, "unparsable"),
        (r#"[1,2]"#, "unparsable"),
        ("not json", "unparsable"),
        ("{\"type\":", "unparsable"),
    ] {
        assert_eq!(CodexDetector::new().line(line.as_bytes()), Verdict::Kill(kind), "{line}");
    }
}

#[test]
fn a_tool_after_partial_text_drops_the_text() {
    let stream = [
        r#"{"type":"item.updated","item":{"id":"item_1","type":"agent_message","text":"PARTIAL "}}"#,
        r#"{"type":"item.started","item":{"id":"item_2","type":"command_execution","command":"ls ~"}}"#,
    ];
    let steps = drive(CodexDetector::new(), stream.join("\n").as_bytes(), 0);
    assert_eq!(steps.last(), Some(&Step::Kill("command")));
    assert!(!format!("{steps:?}").contains("PARTIAL"));
}

#[test]
fn updated_agent_messages_give_only_what_is_new() {
    let mut d = CodexDetector::new();
    let v = lines(
        &mut d,
        &[
            r#"{"type":"item.started","item":{"id":"m","type":"agent_message","text":""}}"#,
            r#"{"type":"item.updated","item":{"id":"m","type":"agent_message","text":"The "}}"#,
            r#"{"type":"item.updated","item":{"id":"m","type":"agent_message","text":"The meeting"}}"#,
            r#"{"type":"item.updated","item":{"id":"m","type":"agent_message","text":"Rewritten"}}"#,
        ],
    );
    assert_eq!(
        v,
        [Verdict::Note("agent_message"), Verdict::Text("The ".into()), Verdict::Text("meeting".into()), Verdict::Note("agent_message_rewritten")]
    );
}

const SIGNED_OUT: [&str; 5] = [
    r#"{"type":"thread.started","thread_id":"t"}"#,
    r#"{"type":"turn.started"}"#,
    r#"{"type":"error","message":"Reconnecting... 2/5 (unexpected status 401 Unauthorized: Missing bearer or basic authentication in header, url: https://api.example.org/v1/responses)"}"#,
    r#"{"type":"item.completed","item":{"id":"item_0","type":"error","message":"Falling back from WebSockets to HTTPS transport."}}"#,
    r#"{"type":"turn.failed","error":{"message":"unexpected status 401 Unauthorized: Missing bearer or basic authentication in header"}}"#,
];

#[test]
fn a_failed_turn_for_want_of_a_sign_in_is_signed_out() {
    let mut d = CodexDetector::new();
    let v = lines(&mut d, &SIGNED_OUT);
    assert!(v.iter().all(|x| matches!(x, Verdict::Note(_))), "{v:?}");
    assert_eq!(d.failure(), Some(Failure::SignedOut));
}

#[test]
fn a_sign_in_failure_is_kept_over_a_later_one() {
    let mut d = CodexDetector::new();
    d.line(br#"{"type":"error","message":"Not logged in. Run codex login."}"#);
    d.line(br#"{"type":"turn.failed","error":{"message":"stream disconnected before completion"}}"#);
    assert_eq!(d.failure(), Some(Failure::SignedOut));
}

#[test]
fn failures_are_classified_from_their_words() {
    assert_eq!(classify("unexpected status 401 Unauthorized"), Failure::SignedOut);
    assert_eq!(classify("Your access token could not be refreshed: refresh token was revoked; please log in again"), Failure::SignedOut);
    assert_eq!(classify("unexpected status 429 Too Many Requests: usage limit"), Failure::Refused { status: 429 });
    assert_eq!(classify("unexpected status: 503 Service Unavailable, request id req_401abc"), Failure::Refused { status: 503 });
    assert_eq!(classify("error sending request for url (https://api.example.org/v1/responses)"), Failure::Unreachable);
    assert_eq!(classify("stream disconnected before completion"), Failure::Unreachable);
    assert_eq!(classify("something odd"), Failure::Refused { status: 0 });
    assert_eq!(classify(""), Failure::Refused { status: 0 });
    let mut d = CodexDetector::new();
    d.line(br#"{"type":"turn.failed"}"#);
    assert_eq!(d.failure(), Some(Failure::Refused { status: 0 }));
}

#[test]
fn the_detector_writes_the_prompt_at_once() {
    assert_eq!(CodexDetector::new().gate(), Gate::Now);
}

proptest! {
    #[test]
    fn any_bytes_never_panic_and_cutting_changes_nothing(data in prop::collection::vec(any::<u8>(), 0..2048), cut in 0usize..2048) {
        let whole = drive(CodexDetector::new(), &data, 0);
        let split = drive(CodexDetector::new(), &data, cut);
        prop_assert_eq!(whole, split);
    }

    #[test]
    fn any_failure_text_classifies(text in ".{0,600}") {
        let _ = classify(&text);
    }
}
