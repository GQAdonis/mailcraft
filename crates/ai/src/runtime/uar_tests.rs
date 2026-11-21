// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The pure parts of the UAR adapter (B9, 19.1–19.3, 19.5): the entry check, the readiness line,
//! the managed config, the capability and model checks, the AG-UI detector and the agent card.

use super::*;
use crate::runtime::contain::{Step, drive};
use proptest::prelude::*;

/// The option lines of `universal-agent-runtime --help` 1.4.0-rc.5 that matter here, as the
/// entry check read them (shortened).
const INSTALLED_HELP: &str = "Usage: universal-agent-runtime [OPTIONS] [COMMAND]\n\
Options:\n      --env-file <ENV_FILE>\n  -c, --config <CONFIG>\n          Config file path [env: CONFIG_FILE=]\n\
      --port <PORT>\n          Port to listen on [env: PORT=]\n      --jwt-required <JWT_REQUIRED>\n\
      --native-file-tools <NATIVE_FILE_TOOLS>\n      --native-web-fetch <NATIVE_WEB_FETCH>\n\
      --native-terminal-exec <NATIVE_TERMINAL_EXEC>\n  -h, --help\n  -V, --version\n";

/// What a program that can be managed says (the fake sidecar of the host's tests says this).
const MANAGED_HELP: &str = "Options:\n  --config <PATH>  config file\n  --port <PORT>  0 for any\n\
  --launch-token-stdin  read the launch token from stdin\n  --ready-line  print READY:<port> when listening\n";

#[test]
fn entry_check_the_installed_binary_is_external_only() {
    let s = ManagedSupport::from_help(INSTALLED_HELP);
    assert!(s.config_path && s.port);
    assert!(!s.stdin_token && !s.readiness);
    assert!(!s.offered());
}

#[test]
fn entry_check_a_program_with_all_four_is_managed() {
    assert!(ManagedSupport::from_help(MANAGED_HELP).offered());
}

#[test]
fn neg_entry_check_words_outside_option_lines_dont_count() {
    let help = "This program can read a token from stdin and print READY:1 one day.\n --config x\n --port y\n";
    assert!(!ManagedSupport::from_help(help).offered());
    assert_eq!(ManagedSupport::from_help(""), ManagedSupport::default());
}

#[test]
fn ready_line_is_exact() {
    assert_eq!(parse_ready(b"READY:4321"), Some(4321));
    assert_eq!(parse_ready(b"READY:4321\r"), Some(4321));
    assert_eq!(parse_ready(b"READY:65535"), Some(65535));
}

#[test]
fn neg_ready_line_anything_else_is_refused() {
    for bad in [
        &b"READY:0"[..],
        b"READY:65536",
        b"READY:",
        b"READY: 80",
        b"READY:80 ",
        b"ready:80",
        b"READY:+80",
        b"READY:-1",
        b"READY:000080",
        b"LISTENING 80",
        b"",
        b"READY:\xff",
        b"READY:99999999999999999999999999999999",
    ] {
        assert_eq!(parse_ready(bad), None, "{:?}", String::from_utf8_lossy(bad));
    }
}

#[test]
fn token_text_is_64_hex_digits() {
    let t = token_text(&[0xab; TOKEN_BYTES]);
    assert_eq!(t.len(), 64);
    assert!(t.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    assert!(t.starts_with("abab"));
}

#[test]
fn managed_config_turns_tools_off_and_names_one_endpoint() {
    let c = ManagedConfig::new("ollama", "llama3.2:3b", "http://127.0.0.1:11434/v1/").unwrap();
    let v = c.to_json();
    let tools = &v["native_tools"];
    assert_eq!(tools["file_tools_enabled"], false);
    assert_eq!(tools["web_fetch_enabled"], false);
    assert_eq!(tools["terminal_exec_enabled"], false);
    assert_eq!(tools["session_search_enabled"], false);
    assert_eq!(v["server"]["port"], 0);
    assert_eq!(v["server"]["host"], "127.0.0.1");
    assert_eq!(v["llm"]["model"], "openai/llama3.2:3b");
    assert_eq!(v["llm"]["base_url"], "http://127.0.0.1:11434/v1");
    assert!(v["llm"]["api_key"].is_null() && v["llm"]["api_key_env"].is_null(), "no key ever goes in the file");
    assert!(v.get("providers").is_none(), "no other model endpoint");
    assert_eq!(c.endpoints(), ["ollama"]);
    let bytes = c.to_bytes().unwrap();
    assert!(serde_json::from_slice::<Value>(&bytes).is_ok());
}

#[test]
fn managed_config_takes_https_anywhere() {
    assert!(ManagedConfig::new("remote", "gpt-4o", "https://models.example.org/v1").is_ok());
    assert!(ManagedConfig::new("local", "m", "http://localhost:8080").is_ok());
    assert!(ManagedConfig::new("local", "m", "http://[::1]:8080/v1").is_ok());
}

#[test]
fn neg_managed_config_refuses_what_cant_be_used() {
    assert_eq!(ManagedConfig::new("", "m", "https://a.example").unwrap_err(), UarError::BadProvider);
    assert_eq!(ManagedConfig::new("a b", "m", "https://a.example").unwrap_err(), UarError::BadProvider);
    assert_eq!(ManagedConfig::new("p", "", "https://a.example").unwrap_err(), UarError::BadModel);
    assert_eq!(ManagedConfig::new("p", "m\"}", "https://a.example").unwrap_err(), UarError::BadModel);
    for url in [
        "http://models.example.org/v1",
        "http://10.0.0.5:11434",
        "ftp://127.0.0.1",
        "https://user:pw@a.example",
        "https://a.example/v1?key=1",
        "https://",
        "https://a.example/ v1",
        "",
    ] {
        assert_eq!(ManagedConfig::new("p", "m", url).unwrap_err(), UarError::BadUrl, "{url}");
    }
    let long = format!("https://a.example/{}", "x".repeat(MAX_URL));
    assert_eq!(ManagedConfig::new("p", "m", &long).unwrap_err(), UarError::BadUrl);
}

#[test]
fn managed_config_debug_names_no_address() {
    let c = ManagedConfig::new("ollama", "secret-model-name", "http://127.0.0.1:11434").unwrap();
    let d = format!("{c:?}");
    assert!(!d.contains("11434") && !d.contains("secret-model-name"));
}

#[test]
fn capabilities_need_the_agui_profile() {
    assert!(capabilities_ok(br#"{"agui":{"profile":"uar.agui/1","profile_revision":1}}"#));
    assert!(!capabilities_ok(br#"{"agui":{"profile":"uar.agui/2"}}"#));
    assert!(!capabilities_ok(br#"{"ownership":"external"}"#));
    assert!(!capabilities_ok(b"not json"));
    assert!(!capabilities_ok(b"[]"));
}

#[test]
fn models_beyond_the_chosen_one_make_it_leave() {
    assert!(!models_beyond(br#"{"data":[{"id":"llama3"}]}"#, "llama3"));
    assert!(!models_beyond(br#"{"data":[{"id":"openai/llama3"}]}"#, "llama3"));
    assert!(!models_beyond(br#"{"data":[]}"#, "llama3"));
    assert!(models_beyond(br#"{"data":[{"id":"llama3"},{"id":"anthropic/claude"}]}"#, "llama3"));
    assert!(models_beyond(br#"{"data":[{"name":"x"}]}"#, "llama3"));
    assert!(models_beyond(b"junk", "llama3"));
    assert!(models_beyond(br#"{"models":[]}"#, "llama3"));
}

#[test]
fn run_body_has_no_tools_and_carries_the_prompt_as_a_message() {
    let b = run_body("run-1", "Answer from the messages.", "When is the meeting?").unwrap();
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(v["tools"], json!([]));
    assert_eq!(v["messages"][0]["role"], "system");
    assert_eq!(v["messages"][1]["content"], "When is the meeting?");
}

fn feed(lines: &[&str]) -> Vec<Step> {
    drive(AgUiDetector::new(), lines.join("\n").as_bytes(), 0)
}

#[test]
fn agui_text_only_stream_answers() {
    let steps = feed(&[
        r#"data: {"type":"RUN_STARTED","threadId":"t","runId":"r"}"#,
        "",
        r#"data: {"type":"TEXT_MESSAGE_START","messageId":"m","role":"assistant"}"#,
        "",
        r#"data: {"type":"TEXT_MESSAGE_CONTENT","messageId":"m","delta":"The meeting "}"#,
        "",
        r#"data: {"type":"TEXT_MESSAGE_CONTENT","messageId":"m","delta":"is on Friday."}"#,
        "",
        r#"data: {"type":"TEXT_MESSAGE_END","messageId":"m"}"#,
        r#"data: {"type":"RUN_FINISHED","threadId":"t","runId":"r"}"#,
    ]);
    assert_eq!(steps.last(), Some(&Step::Done("The meeting is on Friday.".into())));
}

#[test]
fn agui_bare_json_lines_and_sse_fields_are_read() {
    let steps = feed(&[
        ": keep-alive",
        "event: message",
        "id: 7",
        r#"{"type":"TEXT_MESSAGE_CHUNK","delta":"ok"}"#,
        r#"{"type":"TEXT_MESSAGE_CHUNK"}"#,
        r#"{"type":"THINKING_TEXT_MESSAGE_CONTENT","delta":"hmm"}"#,
        r#"{"type":"RUN_FINISHED"}"#,
    ]);
    assert_eq!(steps.last(), Some(&Step::Done("ok".into())));
}

#[test]
fn agui_every_tool_event_and_unknown_kind_kills() {
    let cases = [
        (r#"{"type":"TOOL_CALL_START","toolCallId":"1","toolCallName":"file_read"}"#, "tool_use"),
        (r#"{"type":"TOOL_CALL_START","toolCallId":"1","toolCallName":"terminal_exec"}"#, "command"),
        (r#"{"type":"TOOL_CALL_START","toolCallId":"1","toolCallName":"file_write"}"#, "file_change"),
        (r#"{"type":"TOOL_CALL_START","toolCallId":"1","toolCallName":"mcp__mail__send"}"#, "mcp_call"),
        (r#"{"type":"TOOL_CALL_START","toolCallId":"1","toolCallName":"web_fetch"}"#, "web_search"),
        (r#"{"type":"TOOL_CALL_START","toolCallId":"1"}"#, "tool_use"),
        (r#"{"type":"TOOL_CALL_CHUNK","toolCallName":"bash"}"#, "command"),
        (r#"{"type":"TOOL_CALL_ARGS","toolCallId":"1","delta":"{}"}"#, "tool_use"),
        (r#"{"type":"TOOL_CALL_END","toolCallId":"1"}"#, "tool_use"),
        (r#"{"type":"TOOL_CALL_RESULT","toolCallId":"1","content":"x"}"#, "tool_use"),
        (r#"{"type":"STATE_DELTA","delta":[{"op":"add","path":"/a","value":1}]}"#, "unknown_non_text"),
        (r#"{"type":"STATE_SNAPSHOT","snapshot":{}}"#, "unknown_non_text"),
        (r#"{"type":"MESSAGES_SNAPSHOT","messages":[]}"#, "unknown_non_text"),
        (r#"{"type":"RAW","event":{}}"#, "unknown_non_text"),
        (r#"{"type":"CUSTOM","name":"x","value":1}"#, "unknown_non_text"),
        (r#"{"type":"text_message_content","delta":"lower case is not the kind"}"#, "unknown_non_text"),
        (r#"{"type":"RUN_ERROR","message":"boom"}"#, "run_error"),
        (r#"{"type":"TEXT_MESSAGE_CONTENT"}"#, "unparsable"),
        (r#"{"type":"TEXT_MESSAGE_CONTENT","delta":5}"#, "unparsable"),
        (r#"{"type":7}"#, "unparsable"),
        ("[1,2]", "unparsable"),
        ("data: not json", "unparsable"),
    ];
    for (line, kind) in cases {
        let steps = feed(&[r#"data: {"type":"TEXT_MESSAGE_CONTENT","delta":"PARTIAL"}"#, line, r#"data: {"type":"RUN_FINISHED"}"#]);
        assert!(steps.contains(&Step::Kill(kind)), "{line}: {steps:?}");
        assert!(!steps.iter().any(|s| matches!(s, Step::Done(_))), "{line}: nothing is answered after a kill");
    }
}

#[test]
fn neg_agui_run_finished_without_text_is_no_answer() {
    let steps = feed(&[r#"{"type":"RUN_STARTED"}"#, r#"{"type":"RUN_FINISHED"}"#]);
    assert!(steps.iter().all(|s| *s == Step::Continue));
}

#[test]
fn tool_kind_reads_the_name_only() {
    assert_eq!(tool_kind("Shell"), "command");
    assert_eq!(tool_kind("file_patch"), "file_change");
    assert_eq!(tool_kind("mcp_mail_list"), "mcp_call");
    assert_eq!(tool_kind("search_web"), "web_search");
    assert_eq!(tool_kind("lookup"), "tool_use");
    assert_eq!(tool_kind(&"é".repeat(10_000)), "tool_use");
}

#[test]
fn agent_card_text_is_untrusted() {
    let c = parse_card("{\"name\":\"UAR\\u0007 Agent\\u202e\",\"version\":\"1.4.0-rc.5\\n\",\"locality\":\"local\"}".as_bytes()).unwrap();
    assert_eq!(c.name, "UAR Agent");
    assert_eq!(c.version, "1.4.0-rc.5");
    let long = format!(r#"{{"name":"{}"}}"#, "n".repeat(1000));
    assert_eq!(parse_card(long.as_bytes()).unwrap().name.chars().count(), MAX_CARD_TEXT);
    assert_eq!(parse_card(br#"{"name":5}"#).unwrap(), Card { name: String::new(), version: String::new() });
    assert_eq!(parse_card(b"[]"), None);
    assert_eq!(parse_card(b"\xff\xfe"), None);
}

#[test]
fn external_caveat_says_the_tools_cant_be_turned_off() {
    assert!(EXTERNAL_CAVEAT.contains("can't turn off its tools"));
    assert!(EXTERNAL_CAVEAT.contains("leaves this computer"));
}

proptest! {
    #[test]
    fn agui_hostile_bytes_never_panic(data in proptest::collection::vec(any::<u8>(), 0..4096), cut in 0usize..4096) {
        let whole = drive(AgUiDetector::new(), &data, 0);
        let split = drive(AgUiDetector::new(), &data, cut);
        prop_assert_eq!(whole, split);
        let _ = parse_ready(&data);
        let _ = parse_card(&data);
        let _ = capabilities_ok(&data);
        let _ = models_beyond(&data, "m");
        let _ = ManagedSupport::from_help(&String::from_utf8_lossy(&data));
    }

    #[test]
    fn agui_any_event_type_but_text_kills_or_is_inert(kind in "[A-Z_]{1,24}") {
        let line = format!(r#"{{"type":"{kind}","delta":"x"}}"#);
        let v = AgUiDetector::new().event(line.as_bytes());
        let text = matches!(kind.as_str(), "TEXT_MESSAGE_CONTENT" | "TEXT_MESSAGE_CHUNK");
        prop_assert!(text == matches!(v, Verdict::Text(_)), "{} -> {:?}", kind, v);
    }
}
