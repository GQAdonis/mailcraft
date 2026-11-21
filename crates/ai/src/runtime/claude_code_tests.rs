// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Claude Code adapter's pure parts (B5, 15.2–15.4): argv, auth status, stream detector.

use proptest::prelude::*;

use super::*;
use crate::runtime::contain::{Step, drive};

// --- 15.2 argv ------------------------------------------------------------------------------

fn lits(args: &[ClaudeArg]) -> Vec<String> {
    args.iter()
        .map(|a| match a {
            ClaudeArg::Lit(s) => (*s).to_owned(),
            ClaudeArg::InstructionsFile => "<instructions>".to_owned(),
            ClaudeArg::Model(m) => format!("<model:{m}>"),
        })
        .collect()
}

#[test]
fn run_args_are_the_plan_flag_list_and_the_instructions_file() {
    let args = run_args(None).unwrap();
    assert_eq!(
        lits(&args),
        [
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
            "--system-prompt-file",
            "<instructions>",
        ]
    );
}

#[test]
fn run_args_add_a_checked_model_only_when_chosen() {
    let args = run_args(Some("claude-sonnet-4-6")).unwrap();
    let tail: Vec<String> = lits(&args).into_iter().rev().take(2).collect();
    assert_eq!(tail, ["<model:claude-sonnet-4-6>", "--model"]);
}

#[test]
fn run_args_refuse_a_model_that_isnt_a_name() {
    for bad in ["", "-p", "--tools=Bash", "sonnet; rm -rf ~", "a b", "x\n", &"m".repeat(65)] {
        assert_eq!(run_args(Some(bad)), Err(ArgError::BadModel), "{bad:?}");
    }
    assert!(valid_model("opus"));
    assert!(valid_model("anthropic/claude-opus-5-5:latest"));
}

#[test]
fn no_run_arg_is_an_api_key_or_an_input_format() {
    let all = lits(&run_args(Some("opus")).unwrap()).join(" ");
    for absent in ["api-key", "--input-format", "--include-partial-messages", "--dangerously", "--mcp-config", "--settings"] {
        assert!(!all.contains(absent), "{absent}");
    }
}

// --- 15.3 auth status -----------------------------------------------------------------------

fn status(json: &str) -> AuthStatus {
    parse_auth_status(json.as_bytes())
}

#[test]
fn logged_in_with_a_claude_ai_plan_is_usable_and_uses_the_plan() {
    for (sub, tier) in [("max", "max_20x"), ("pro", "pro"), ("team", "team"), ("enterprise", "enterprise")] {
        let s = status(&format!(
            r#"{{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","email":"owner@example.org","orgName":"Example","subscriptionType":"{sub}","planTier":"{tier}"}}"#
        ));
        assert_eq!(s, AuthStatus::Usable { method: AuthMethod::ClaudeAi, uses_plan: true }, "{sub}");
    }
}

#[test]
fn any_other_logged_in_method_is_usable() {
    assert_eq!(status(r#"{"loggedIn":true,"authMethod":"console"}"#), AuthStatus::Usable { method: AuthMethod::Console, uses_plan: false });
    assert_eq!(status(r#"{"loggedIn":true,"authMethod":"api_key"}"#), AuthStatus::Usable { method: AuthMethod::ApiKey, uses_plan: false });
    assert_eq!(status(r#"{"loggedIn":true,"authMethod":"bedrock"}"#), AuthStatus::Usable { method: AuthMethod::Other, uses_plan: false });
    assert_eq!(status(r#"{"loggedIn":true}"#), AuthStatus::Usable { method: AuthMethod::Other, uses_plan: false });
}

#[test]
fn neg_logged_out_needs_sign_in() {
    assert_eq!(status(r#"{"loggedIn":false,"authMethod":"none","apiProvider":"firstParty"}"#), AuthStatus::NeedsSignIn);
}

#[test]
fn status_that_cant_be_read_is_broken() {
    assert_eq!(status("Not logged in"), AuthStatus::Broken(StatusProblem::Malformed));
    assert_eq!(status("[true]"), AuthStatus::Broken(StatusProblem::Malformed));
    assert_eq!(status(r#"{"authMethod":"claude.ai"}"#), AuthStatus::Broken(StatusProblem::MissingLoggedIn));
    assert_eq!(status(r#"{"loggedIn":"yes"}"#), AuthStatus::Broken(StatusProblem::Malformed));
    assert_eq!(status(r#"{"loggedIn":true,"authMethod":7}"#), AuthStatus::Broken(StatusProblem::Malformed));
    let big = format!(r#"{{"loggedIn":true,"pad":"{}"}}"#, "x".repeat(MAX_STATUS_BYTES));
    assert_eq!(status(&big), AuthStatus::Broken(StatusProblem::TooLarge));
    let deep = format!("{{\"loggedIn\":{}{}}}", "[".repeat(4_000), "]".repeat(4_000));
    assert_eq!(status(&deep), AuthStatus::Broken(StatusProblem::Malformed));
}

#[test]
fn a_plan_named_none_or_empty_isnt_a_plan() {
    let s = status(r#"{"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"none","planTier":""}"#);
    assert_eq!(s, AuthStatus::Usable { method: AuthMethod::ClaudeAi, uses_plan: false });
}

// --- 15.4 stream detector -------------------------------------------------------------------

const INIT: &str = r#"{"type":"system","subtype":"init","tools":[],"mcp_servers":[],"skills":["deep-research","design"],"agents":["claude","Explore"],"slash_commands":["design"],"apiKeySource":"none"}"#;
const TEXT: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"The meeting is on Friday."}]}}"#;
const RESULT: &str = r#"{"type":"result","subtype":"success","is_error":false,"result":"The meeting is on Friday."}"#;

fn verdict(line: &str) -> Verdict {
    ClaudeStreamDetector::new(Gate::AfterInit).line(line.as_bytes())
}

#[test]
fn init_with_no_tools_is_ok_whatever_skills_and_agents_it_names() {
    assert_eq!(verdict(INIT), Verdict::InitOk);
}

#[test]
fn init_listing_a_tool_or_an_mcp_server_kills() {
    assert_eq!(verdict(r#"{"type":"system","subtype":"init","tools":["Bash"],"mcp_servers":[]}"#), Verdict::Kill("init_lists_tool"));
    assert_eq!(verdict(r#"{"type":"system","subtype":"init","tools":["Skill"],"mcp_servers":[]}"#), Verdict::Kill("init_lists_tool"));
    assert_eq!(verdict(r#"{"type":"system","subtype":"init","tools":[],"mcp_servers":[{"name":"mail"}]}"#), Verdict::Kill("init_lists_mcp"));
    assert_eq!(verdict(r#"{"type":"system","subtype":"init","mcp_servers":[]}"#), Verdict::Kill("init_malformed"));
    assert_eq!(verdict(r#"{"type":"system","subtype":"init","tools":"none","mcp_servers":[]}"#), Verdict::Kill("init_malformed"));
}

#[test]
fn text_thinking_rate_limits_and_results_are_read() {
    assert_eq!(verdict(TEXT), Verdict::Text("The meeting is on Friday.".into()));
    assert_eq!(verdict(r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"hmm"}]}}"#), Verdict::Note("thinking"));
    assert_eq!(verdict(r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed"}}"#), Verdict::Note("rate_limit_event"));
    assert_eq!(verdict(r#"{"type":"system","subtype":"api_retry","attempt":1}"#), Verdict::Note("api_retry"));
    assert_eq!(verdict(RESULT), Verdict::Final("The meeting is on Friday.".into()));
}

#[test]
fn every_tool_block_and_unknown_kind_kills() {
    let tool = |block: &str| format!(r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"x"}},{block}]}}}}"#);
    let cases = [
        (tool(r#"{"type":"tool_use","name":"Read","input":{}}"#), "tool_use"),
        (tool(r#"{"type":"tool_use","name":"Bash","input":{"command":"ls"}}"#), "command"),
        (tool(r#"{"type":"tool_use","name":"Edit","input":{}}"#), "file_change"),
        (tool(r#"{"type":"tool_use","name":"mcp__mail__send","input":{}}"#), "mcp_call"),
        (tool(r#"{"type":"mcp_tool_use","name":"send"}"#), "mcp_call"),
        (tool(r#"{"type":"server_tool_use","name":"web_search","input":{}}"#), "web_search"),
        (tool(r#"{"type":"image","source":{}}"#), "unknown_block"),
        (tool(r#"{"text":"no type"}"#), "unknown_block"),
        (r#"{"type":"user","message":{"content":[{"type":"tool_result"}]}}"#.to_owned(), "tool_result"),
        (r#"{"type":"stream_event","event":{}}"#.to_owned(), "unknown_event"),
        (r#"{"type":"system","subtype":"compact_boundary"}"#.to_owned(), "unknown_system"),
        (r#"{"subtype":"init"}"#.to_owned(), "unknown_event"),
        ("not json".to_owned(), "unparsable"),
        ("42".to_owned(), "unparsable"),
        (r#"{"type":"assistant","message":{}}"#.to_owned(), "malformed"),
    ];
    for (line, kind) in cases {
        assert_eq!(verdict(&line), Verdict::Kill(kind), "{line}");
    }
}

#[test]
fn not_logged_in_is_noted_never_shown_and_reported_as_signed_out() {
    let mut d = ClaudeStreamDetector::new(Gate::AfterInit);
    let outcome = d.outcome();
    let msg = r#"{"type":"assistant","message":{"model":"<synthetic>","content":[{"type":"text","text":"Not logged in · Please run /login"}]},"error":"authentication_failed","is_api_error_message":true}"#;
    assert_eq!(d.line(msg.as_bytes()), Verdict::Note("api_error"));
    let res = r#"{"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login"}"#;
    assert_eq!(d.line(res.as_bytes()), Verdict::Note("result_error"));
    assert_eq!(outcome.get(), RunOutcome::SignedOut);
}

#[test]
fn error_results_are_classified_from_the_result_alone() {
    let run = |line: &str| {
        let mut d = ClaudeStreamDetector::new(Gate::Now);
        let o = d.outcome();
        assert_eq!(d.line(line.as_bytes()), Verdict::Note("result_error"));
        o.get()
    };
    assert_eq!(run(r#"{"type":"result","subtype":"success","is_error":true,"result":"Not logged in"}"#), RunOutcome::SignedOut);
    assert_eq!(run(r#"{"type":"result","is_error":true,"api_error_status":401,"result":"?"}"#), RunOutcome::SignedOut);
    assert_eq!(run(r#"{"type":"result","is_error":true,"api_error_status":429,"result":"slow down"}"#), RunOutcome::Refused { status: 429 });
    assert_eq!(run(r#"{"type":"result","is_error":true,"result":"Connection error."}"#), RunOutcome::Unreachable);
    assert_eq!(run(r#"{"type":"result","subtype":"error_max_turns"}"#), RunOutcome::Refused { status: 0 });
}

#[test]
fn the_gate_holds_answers_until_init_and_releases_the_prompt_on_init() {
    let stream = [INIT, TEXT, RESULT].join("\n");
    let steps = drive(ClaudeStreamDetector::new(Gate::AfterInit), stream.as_bytes(), 7);
    assert_eq!(steps, [Step::WritePrompt, Step::Continue, Step::Done("The meeting is on Friday.".into())]);
    // Text before any init, under the gate, kills.
    let steps = drive(ClaudeStreamDetector::new(Gate::AfterInit), TEXT.as_bytes(), 0);
    assert_eq!(steps, [Step::Kill("text_before_init")]);
}

#[test]
fn a_tool_after_text_kills_and_drops_the_text() {
    let tool = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{}}]}}"#;
    let stream = [INIT, TEXT, tool, RESULT].join("\n");
    let steps = drive(ClaudeStreamDetector::new(Gate::Now), stream.as_bytes(), 0);
    assert_eq!(steps, [Step::Continue, Step::Continue, Step::Kill("command"), Step::Continue]);
}

proptest! {
    #[test]
    fn any_bytes_never_panic_and_the_cut_changes_nothing(data in proptest::collection::vec(any::<u8>(), 0..2048), cut in 0usize..2048) {
        for gate in [Gate::Now, Gate::AfterInit] {
            let whole = drive(ClaudeStreamDetector::new(gate), &data, 0);
            let split = drive(ClaudeStreamDetector::new(gate), &data, cut);
            prop_assert_eq!(whole, split);
        }
        let _ = parse_auth_status(&data);
    }

    #[test]
    fn json_shaped_lines_never_panic(kind in "[a-z_]{0,12}", sub in "[a-z_]{0,8}", body in ".{0,64}") {
        let line = format!(r#"{{"type":"{kind}","subtype":"{sub}","message":{{"content":[{{"type":"{sub}","text":{body:?}}}]}},"result":{body:?},"is_error":false}}"#);
        let _ = ClaudeStreamDetector::new(Gate::Now).line(line.as_bytes());
    }
}
