// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What an answer says of the runtimes' chain (ai-runtime-chain 20.5).

use serde_json::{Value, json};

use super::chain::{passed_over, question_words, reason_words, runtime_name};
use crate::ai::Providers;

fn providers() -> Providers {
    Providers::read(
        &json!({"active": "lab", "providers": [{"id": "lab", "kind": "local", "base_url": "http://127.0.0.1:11434/v1", "model": "small-model"}]}),
    )
}

#[test]
fn every_runtime_passed_over_or_failed_is_said_with_its_reason() {
    let provenance = json!({
        "request": 4,
        "answeredBy": "compat:lab",
        "passedOver": [
            {"runtime": "chatgpt", "skipped": "no_consent"},
            {"runtime": "claude_code", "skipped": "signed_out"},
            {"runtime": "codex", "failed": "tool_use_stopped"},
        ],
    });
    let lines = passed_over(Some(&provenance), &[], &providers());
    assert_eq!(
        lines,
        [
            "ChatGPT plan was passed over: this account's mail isn't allowed to go to it.",
            "Claude Code was passed over: it's signed out.",
            "Codex didn't answer: it tried to use a tool, so it was stopped.",
        ]
    );
}

#[test]
fn a_tool_use_stop_not_already_said_gets_its_own_line() {
    let provenance = json!({"request": 1, "answeredBy": "on_device", "passedOver": [{"runtime": "codex", "failed": "tool_use_stopped"}]});
    let notices = ["codex".to_string(), "uar".to_string()];
    let lines = passed_over(Some(&provenance), &notices, &providers());
    assert_eq!(lines.len(), 2, "codex is said once");
    assert_eq!(lines[1], "Universal Agent Runtime was stopped: it tried to use a tool.");
}

#[test]
fn a_compatible_provider_is_named_by_its_model_and_an_unknown_one_by_its_id() {
    assert_eq!(runtime_name("compat:lab", &providers()), "small-model");
    assert_eq!(runtime_name("compat:elsewhere", &providers()), "elsewhere");
    assert_eq!(runtime_name("on_device", &providers()), "the model on this computer");
}

#[test]
fn an_unknown_reason_reads_as_couldnt_answer() {
    assert_eq!(reason_words("something_new"), "it couldn't answer");
    assert_eq!(reason_words(""), "it couldn't answer");
}

#[test]
fn nothing_is_said_without_a_walk() {
    assert!(passed_over(None, &[], &providers()).is_empty());
    assert!(passed_over(Some(&json!({"request": 1, "answeredBy": "uar", "passedOver": []})), &[], &providers()).is_empty());
}

#[test]
fn the_fallback_question_names_both_runtimes_and_is_only_for_its_request() {
    let question = json!({"request": 9, "failed": "claude_code", "reason": "signed_out", "next": "compat:lab"});
    assert_eq!(question_words(&question, 9, &providers()).as_deref(), Some("Claude Code couldn't answer (it's signed out). Try small-model?"));
    assert_eq!(question_words(&question, 8, &providers()), None, "another request's question isn't shown here");
}

#[test]
fn hostile_json_from_the_host_never_panics_and_stays_short() {
    let long = "x".repeat(100_000);
    let cases: Vec<Value> = vec![
        Value::Null,
        json!([1, 2, 3]),
        json!({"passedOver": "not a list"}),
        json!({"passedOver": [null, 5, {"runtime": 7}, {"runtime": "codex"}, {"runtime": "codex", "skipped": 3}]}),
        json!({"passedOver": [{"runtime": format!("compat:{long}"), "skipped": long}]}),
        json!({"passedOver": (0..1000).map(|_| json!({"runtime": "uar", "failed": "timed_out"})).collect::<Vec<_>>()}),
    ];
    for case in &cases {
        let lines = passed_over(Some(case), std::slice::from_ref(&long), &providers());
        assert!(lines.len() <= 16, "the walk said is capped");
        assert!(lines.iter().all(|l| l.chars().count() < 200), "every line stays short");
        let _ = question_words(case, 0, &providers());
    }
    let question = json!({"request": 1, "failed": long, "reason": 5, "next": "\u{0}\r\n"});
    if let Some(words) = question_words(&question, 1, &providers()) {
        assert!(words.chars().count() < 200);
        assert!(!words.contains('\r') && !words.contains('\u{0}'));
    }
}
