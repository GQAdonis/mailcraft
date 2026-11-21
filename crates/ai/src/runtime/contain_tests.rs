// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The pure parts of containment: the env allow-list, the argv check, caps, the splitter and the
//! judge (B4, 14.2–14.3).

use super::*;
use proptest::prelude::*;

fn base() -> BaseEnv {
    BaseEnv {
        path: Some("/usr/bin:/bin".into()),
        home: Some("/home/owner".into()),
        user: Some("owner".into()),
        lang: Some("en_GB.UTF-8".into()),
        tmpdir: Some("/tmp/run/tmp".into()),
    }
}

fn names(env: &[(String, String)]) -> Vec<&str> {
    env.iter().map(|(n, _)| n.as_str()).collect()
}

#[test]
fn env_is_the_base_alone_without_recipe_vars() {
    let env = build_env(&base(), &[]).unwrap();
    let tmp = if cfg!(windows) { "TEMP" } else { "TMPDIR" };
    assert_eq!(names(&env), vec!["PATH", "HOME", "USER", "LANG", tmp]);
}

#[test]
fn env_leaves_out_unknown_base_values() {
    let env = build_env(&BaseEnv { path: Some("/bin".into()), ..BaseEnv::default() }, &[]).unwrap();
    assert_eq!(names(&env), vec!["PATH"]);
}

#[test]
fn env_takes_codex_home_from_a_recipe() {
    let env = build_env(&base(), &[("CODEX_HOME".into(), "/home/owner/.codex".into())]).unwrap();
    assert!(env.iter().any(|(n, v)| n == "CODEX_HOME" && v == "/home/owner/.codex"));
}

#[test]
fn env_refuses_every_forbidden_name_a_recipe_could_name() {
    for name in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_BASE_URL",
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
        "CODEX_API_KEY",
        "CLAUDE_CONFIG_DIR",
        "CLAUDE_CODE_USE_BEDROCK",
        "UAR_TOKEN",
        "LLM_KEY",
        "GEMINI_API_KEY",
        "NODE_OPTIONS",
        "HTTP_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "NO_PROXY",
        "GITHUB_TOKEN",
        "MAILCRAFT_TEST_SECRET",
        "LD_PRELOAD",
        "DYLD_INSERT_LIBRARIES",
    ] {
        assert!(is_forbidden_env_name(name), "{name}");
        assert_eq!(build_env(&base(), &[(name.into(), "x".into())]), Err(ContainError::ForbiddenVariable), "{name}");
    }
}

#[test]
fn env_allows_harmless_names() {
    for name in ["CODEX_HOME", "codex_home", "TERM", "NO_COLOR"] {
        assert!(!is_forbidden_env_name(name), "{name}");
    }
}

#[test]
fn env_refuses_malformed_or_duplicate_vars() {
    for name in ["", "A=B", "1ABC", "A B", "PATH", "home"] {
        assert_eq!(build_env(&base(), &[(name.into(), "x".into())]), Err(ContainError::MalformedVariable), "{name:?}");
    }
    assert_eq!(build_env(&base(), &[("TERM".into(), "a\0b".into())]), Err(ContainError::MalformedVariable));
    let many: Vec<(String, String)> = (0..=MAX_RECIPE_VARS).map(|i| (format!("V{i}"), "x".into())).collect();
    assert_eq!(build_env(&base(), &many), Err(ContainError::MalformedVariable));
    let twice = [("TERM".into(), "a".into()), ("TERM".into(), "b".into())];
    assert_eq!(build_env(&base(), &twice), Err(ContainError::MalformedVariable));
    let long = BaseEnv { path: Some("x".repeat(MAX_ENV_VALUE_BYTES + 1)), ..BaseEnv::default() };
    assert_eq!(build_env(&long, &[]), Err(ContainError::MalformedVariable));
}

const PROMPT: &str = "Summarise this message from alice@example.org: The quarterly figures are attached, \
please keep them to yourself until Friday's meeting at the Example Hall.";
const KEY: &str = "sk-test-0123456789abcdefABCDEF0123456789";

#[test]
fn argv_of_constants_and_paths_passes() {
    let argv = ["/usr/local/bin/claude", "-p", "--output-format", "stream-json", "--append-system-prompt-file", "/tmp/run/instructions.txt"];
    assert_eq!(check_argv(&argv, &[PROMPT, KEY]), Ok(()));
}

#[test]
fn argv_holding_the_prompt_or_a_part_of_it_is_refused() {
    assert_eq!(check_argv(&["claude", PROMPT], &[PROMPT]), Err(ContainError::ArgvCarriesSecret));
    let part = PROMPT.get(40..90).unwrap();
    assert_eq!(check_argv(&["claude", "-p", part], &[PROMPT]), Err(ContainError::ArgvCarriesSecret));
    let embedded = format!("--message={}", PROMPT.get(10..60).unwrap());
    assert_eq!(check_argv(&["claude", &embedded], &[PROMPT]), Err(ContainError::ArgvCarriesSecret));
}

#[test]
fn argv_holding_a_key_or_token_is_refused() {
    let flag = format!("--api-key={KEY}");
    assert_eq!(check_argv(&["codex", &flag], &[KEY]), Err(ContainError::ArgvCarriesSecret));
    // A short token, held whole.
    assert_eq!(check_argv(&["codex", "--t=tok12345"], &["tok12345"]), Err(ContainError::ArgvCarriesSecret));
}

#[test]
fn argv_ignores_needles_too_short_to_tell() {
    assert_eq!(check_argv(&["codex", "exec"], &["exec"]), Ok(()));
}

#[test]
fn argv_malformed_is_refused() {
    let none: [&str; 0] = [];
    assert_eq!(check_argv(&none, &[PROMPT]), Err(ContainError::ArgvMalformed));
    assert_eq!(check_argv(&["a\0b"], &[PROMPT]), Err(ContainError::ArgvMalformed));
    let many = vec!["x"; MAX_ARGV + 1];
    assert_eq!(check_argv(&many, &[PROMPT]), Err(ContainError::ArgvMalformed));
    let big = "y".repeat(MAX_ARGV_BYTES + 1);
    assert_eq!(check_argv(&[big.as_str()], &[PROMPT]), Err(ContainError::ArgvMalformed));
}

proptest! {
    #[test]
    fn argv_never_passes_a_window_of_the_needle(needle in "[a-zA-Z0-9 .,@]{32,200}", start in 0usize..200, pre in "[a-z-]{0,10}") {
        let start = start.min(needle.len() - ARGV_WINDOW);
        let piece = format!("{pre}{}", &needle[start..start + ARGV_WINDOW]);
        prop_assert_eq!(check_argv(&["/bin/x", piece.as_str()], &[needle.as_str()]), Err(ContainError::ArgvCarriesSecret));
    }

    #[test]
    fn argv_check_never_panics(argv in proptest::collection::vec(".{0,80}", 0..8), needles in proptest::collection::vec(".{0,80}", 0..4)) {
        let _ = check_argv(&argv, &needles);
    }
}

#[test]
fn limits_default_to_the_spec_and_clamp() {
    let d = Limits::default();
    assert_eq!((d.stdout_bytes, d.stderr_bytes, d.line_bytes), (4 << 20, 64 << 10, 1 << 20));
    assert_eq!((d.wall_clock, d.idle), (Duration::from_secs(120), Duration::from_secs(30)));
    assert_eq!(Limits::with_wall_clock_secs(0).wall_clock, MIN_TIME);
    assert_eq!(Limits::with_wall_clock_secs(u64::MAX).wall_clock, MAX_WALL_CLOCK);
    let huge = Limits { stdout_bytes: usize::MAX, stderr_bytes: usize::MAX, line_bytes: usize::MAX, ..d }.clamped();
    assert_eq!((huge.stdout_bytes, huge.stderr_bytes, huge.line_bytes), (MAX_STDOUT_BYTES, MAX_STDERR_BYTES, MAX_LINE_BYTES));
}

#[test]
fn splitter_joins_pieces_and_drops_cr() {
    let mut s = LineSplitter::new(64);
    assert_eq!(s.push(b"ab"), vec![]);
    assert_eq!(s.push(b"c\r\nde\nf"), vec![Piece::Line(b"abc".to_vec()), Piece::Line(b"de".to_vec())]);
    assert_eq!(s.finish(), Some(Piece::Line(b"f".to_vec())));
    assert_eq!(s.finish(), None);
}

#[test]
fn splitter_reports_an_over_cap_line_once() {
    let mut s = LineSplitter::new(4);
    assert_eq!(s.push(b"ok\n12345"), vec![Piece::Line(b"ok".to_vec()), Piece::TooLong]);
    assert_eq!(s.push(b"\nmore\n"), vec![]);
    assert_eq!(s.finish(), None);
    let mut s = LineSplitter::new(4);
    assert_eq!(s.push(b"123456\n"), vec![Piece::TooLong]);
}

/// A detector of a made-up line format: `t:<text>`, `f:<text>`, `i` (init ok), `n`, anything else kills.
struct Scripted(Gate);

impl StreamDetector for Scripted {
    fn gate(&self) -> Gate {
        self.0
    }
    fn line(&mut self, bytes: &[u8]) -> Verdict {
        let s = String::from_utf8_lossy(bytes);
        match s.split_once(':') {
            Some(("t", rest)) => Verdict::Text(rest.into()),
            Some(("f", rest)) => Verdict::Final(rest.into()),
            _ if s == "i" => Verdict::InitOk,
            _ if s == "n" => Verdict::Note("note"),
            _ => Verdict::Kill("tool_use"),
        }
    }
}

#[test]
fn judge_passes_text_and_ends_on_final() {
    let mut j = Judge::new(Scripted(Gate::Now));
    assert!(j.prompt_at_start());
    assert_eq!(j.line(b"t:Hel"), Step::Continue);
    assert_eq!(j.line(b"n"), Step::Continue);
    assert_eq!(j.line(b"t:lo"), Step::Continue);
    assert_eq!(j.text(), "Hello");
    assert_eq!(j.line(b"f:Hello"), Step::Done("Hello".into()));
    assert!(j.ended());
    assert_eq!(j.line(b"tool"), Step::Continue, "nothing after the end");
}

#[test]
fn judge_skips_blank_lines() {
    let mut j = Judge::new(Scripted(Gate::AfterInit));
    assert_eq!(j.line(b""), Step::Continue);
    assert_eq!(j.line(b"  \t\r"), Step::Continue);
    assert!(!j.ended());
}

#[test]
fn judge_kill_drops_the_text() {
    let mut j = Judge::new(Scripted(Gate::Now));
    j.line(b"t:secret part of an answer");
    assert_eq!(j.line(b"{\"type\":\"tool_use\"}"), Step::Kill("tool_use"));
    assert_eq!(j.text(), "");
    assert_eq!(j.line(b"f:late"), Step::Continue);
}

#[test]
fn judge_holds_the_prompt_until_init() {
    let mut j = Judge::new(Scripted(Gate::AfterInit));
    assert!(!j.prompt_at_start());
    assert_eq!(j.line(b"n"), Step::Continue);
    assert_eq!(j.line(b"i"), Step::WritePrompt);
    assert_eq!(j.line(b"i"), Step::Continue, "a second init writes nothing");
    assert_eq!(j.line(b"f:ok"), Step::Done("ok".into()));
}

#[test]
fn judge_kills_text_before_init_and_an_init_with_a_tool() {
    let mut j = Judge::new(Scripted(Gate::AfterInit));
    assert_eq!(j.line(b"t:early"), Step::Kill("text_before_init"));
    let mut j = Judge::new(Scripted(Gate::AfterInit));
    assert_eq!(j.line(b"init-with-tools"), Step::Kill("tool_use"));
}

#[test]
fn judge_kills_over_cap_lines_and_answers() {
    let mut j = Judge::new(Scripted(Gate::Now));
    assert_eq!(j.piece(&Piece::TooLong), Step::Kill("line_too_long"));
    let mut j = Judge::with_cap(Scripted(Gate::Now), 4);
    assert_eq!(j.line(b"t:abc"), Step::Continue);
    assert_eq!(j.line(b"t:de"), Step::TooLarge);
    assert_eq!(j.text(), "");
    let mut j = Judge::with_cap(Scripted(Gate::Now), 4);
    assert_eq!(j.line(b"f:abcde"), Step::TooLarge);
}

#[test]
fn tool_classes_are_six_and_named() {
    let names: Vec<_> = ToolClass::ALL.iter().map(|c| c.name()).collect();
    assert_eq!(names, ["tool_use", "command", "file_change", "mcp_call", "web_search", "unknown_non_text"]);
}

#[test]
fn drive_runs_any_bytes_through_splitter_and_judge() {
    let steps = drive(Scripted(Gate::Now), b"t:a\nt:b\nf:ab", 3);
    assert_eq!(steps.last(), Some(&Step::Done("ab".into())));
    let steps = drive(Scripted(Gate::Now), b"t:a\nrm -rf\nf:x\n", 0);
    assert!(steps.contains(&Step::Kill("tool_use")));
    assert!(!steps.iter().any(|s| matches!(s, Step::Done(_))));
}

proptest! {
    #[test]
    fn drive_never_panics_and_where_it_is_cut_changes_nothing(data in proptest::collection::vec(any::<u8>(), 0..512), cut in 0usize..512) {
        let whole = drive(Scripted(Gate::AfterInit), &data, 0);
        let split = drive(Scripted(Gate::AfterInit), &data, cut);
        prop_assert_eq!(whole, split);
    }
}

#[test]
fn errors_name_their_kind_only() {
    assert_eq!(ContainError::ArgvCarriesSecret.to_string(), "argv_carries_secret");
    assert_eq!(ContainError::ForbiddenVariable.kind(), "forbidden_variable");
}
