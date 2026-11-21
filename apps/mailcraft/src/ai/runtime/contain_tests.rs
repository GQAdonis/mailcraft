// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! SCN-RT-CONTAIN-001 (B4, 14.12, with the spawner of 14.4–14.5): a tool event in any stream
//! format kills the whole process group, grandchildren included, drops the answer, ends the run
//! `tool_use_stopped`, and tells the owner before any fallback starts. A text-only stream is not
//! killed (`neg_`), and an init event that lists a tool kills before the prompt is written.
//!
//! The harness is shared: each format registers its fixtures in [`registry`], one per tool class
//! and one unknown non-text event; a format missing a class fails. B5 (Claude stream-json), B6
//! (Codex JSONL) and B9 (AG-UI) add theirs from `crates/ai/src/runtime/*_fixtures`; until they
//! land the registry holds the made-up `test_jsonl` format, which exercises every class.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use mailcraft_ai::runtime::contain::{Gate, Limits, StreamDetector, ToolClass};
use mailcraft_model::ai::ErrorKind;

use crate::ai::runtime::sandbox::Sandbox;
use crate::ai::runtime::spawn::test_support::{TestDetector, heartbeat_stops, host, limits, recipe, run_fake, script, sq, wait_for_file};
use crate::ai::runtime::spawn::{Home, Notice, RunEnd, RunSpec, run};

/// The formats CONTAIN-001 must cover (OpenCode removed by the plan).
const REQUIRED_FORMATS: [&str; 3] = ["claude_stream_json", "codex_jsonl", "ag_ui"];

/// One stream format's fixtures.
pub(crate) struct FormatFixtures {
    pub(crate) format: &'static str,
    pub(crate) detector: fn(Gate) -> Box<dyn StreamDetector>,
    /// A text-only stream, its last line the final answer.
    pub(crate) text_only: &'static [&'static str],
    /// The answer the text-only stream gives.
    pub(crate) answer: &'static str,
    /// A line of answer text sent before each tool event (it must be dropped).
    pub(crate) text_line: &'static str,
    /// The words of `text_line`, looked for in what the run hands back.
    pub(crate) text_marker: &'static str,
    /// One fixture line per class.
    pub(crate) classes: &'static [(ToolClass, &'static str)],
}

fn test_detector(gate: Gate) -> Box<dyn StreamDetector> {
    Box::new(TestDetector(gate))
}

const TEST_JSONL: FormatFixtures = FormatFixtures {
    format: "test_jsonl",
    detector: test_detector,
    text_only: &[
        r#"{"type":"note","text":"warming up"}"#,
        r#"{"type":"text","text":"The meeting "}"#,
        r#"{"type":"text","text":"is on Friday."}"#,
        r#"{"type":"final","text":"The meeting is on Friday."}"#,
    ],
    answer: "The meeting is on Friday.",
    text_line: r#"{"type":"text","text":"PARTIAL-ANSWER-MARKER about the meeting"}"#,
    text_marker: "PARTIAL-ANSWER-MARKER",
    classes: &[
        (ToolClass::ToolUse, r#"{"type":"tool_use","name":"Read","input":{"file_path":"/etc/hosts"}}"#),
        (ToolClass::Command, r#"{"type":"command","command":"ls ~"}"#),
        (ToolClass::FileChange, r#"{"type":"file_change","path":"notes.txt"}"#),
        (ToolClass::McpCall, r#"{"type":"mcp_call","server":"mail","tool":"send"}"#),
        (ToolClass::WebSearch, r#"{"type":"web_search","query":"example.org"}"#),
        (ToolClass::UnknownNonText, r#"{"type":"reasoning_trace_v9","data":"…"}"#),
    ],
};

/// Every registered format. B5, B6 and B9 add theirs here.
fn registry() -> Vec<&'static FormatFixtures> {
    vec![&TEST_JSONL, &super::claude_fixtures::FIXTURES, &super::codex_fixtures::FIXTURES, &super::uar_fixtures::FIXTURES]
}

/// The classes a format lacks.
fn missing_classes(f: &FormatFixtures) -> Vec<&'static str> {
    ToolClass::ALL.iter().filter(|c| !f.classes.iter().any(|(k, _)| k == *c)).map(|c| c.name()).collect()
}

struct Rig {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Rig {
    fn new() -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir(root.join("bin")).unwrap();
        std::fs::create_dir(root.join("rec")).unwrap();
        std::fs::create_dir(root.join("home")).unwrap();
        Rig { _dir: dir, root }
    }
    fn rec(&self, name: &str) -> PathBuf {
        self.root.join("rec").join(name)
    }
    fn home(&self) -> PathBuf {
        self.root.join("home")
    }
    /// A fake runtime that records its working folder, starts a grandchild writing a heartbeat
    /// every 50 ms, prints `lines`, then runs `tail`.
    fn fake(&self, lines: &[&str], tail: &str) -> PathBuf {
        let print =
            if lines.is_empty() { String::new() } else { format!("cat <<'MAILCRAFT_FIXTURE_END'\n{}\nMAILCRAFT_FIXTURE_END\n", lines.join("\n")) };
        let body = format!(
            "[ \"$1\" = warm ] && exit 0\npwd > {pwd}\n( while :; do printf x >> {hb}; sleep 0.05; done ) &\nsleep 0.2\n{print}{tail}",
            pwd = sq(&self.rec("pwd").to_string_lossy()),
            hb = sq(&self.rec("hb").to_string_lossy()),
        );
        script(&self.root.join("bin"), "fake-runtime", &body)
    }
    /// The run folder the fake ran in no longer exists.
    fn run_folder_gone(&self) -> bool {
        let work = std::fs::read_to_string(self.rec("pwd")).unwrap();
        let work = Path::new(work.trim());
        assert!(work.ends_with("work"), "the fake ran in its empty working folder");
        let run_dir = work.parent().unwrap();
        !work.exists() && !run_dir.exists()
    }
}

const PROMPT: &str = "Question: when is the meeting?\n\nMessage from alice@example.org: Friday at ten.";

fn run_lines(rig: &Rig, lines: &[&str], tail: &str, detector: Box<dyn StreamDetector>, events: &mut Vec<&'static str>) -> RunEnd {
    let program = rig.fake(lines, tail);
    let r = recipe("fake", true, Home::Real, vec![]);
    let host = host(&rig.home());
    run_fake(&program, &r, PROMPT, &[], &host, &Sandbox::None, limits(10), detector, events).end
}

/// One tool fixture: the group dies (heartbeat stops within 1 s), no text, notice first.
fn assert_tool_fixture_stops(format: &FormatFixtures, class: ToolClass, line: &str) {
    let rig = Rig::new();
    let mut events = Vec::new();
    let end = run_lines(&rig, &[format.text_line, line], "sleep 30", (format.detector)(Gate::Now), &mut events);
    // The chain would start its fallback only now.
    events.push("fallback_start");
    let what = format!("{} / {}", format.format, class.name());
    assert!(matches!(end, RunEnd::Stopped { .. }), "{what}: {end:?}");
    assert_eq!(end.error_kind(), Some(ErrorKind::ToolUseStopped), "{what}");
    assert!(!format!("{end:?}").contains(format.text_marker), "{what}: no answer text is handed back");
    assert_eq!(events, ["notice", "fallback_start"], "{what}: the notice comes before any fallback");
    assert!(heartbeat_stops(&rig.rec("hb"), Duration::from_secs(1)), "{what}: the grandchild's heartbeat stops within 1 s");
    assert!(rig.run_folder_gone(), "{what}: the run folder is removed");
}

#[test]
fn contain_001_every_tool_class_in_every_registered_format_kills_the_group() {
    for format in registry() {
        let missing = missing_classes(format);
        assert!(missing.is_empty(), "format {} has no fixture for {missing:?}", format.format);
        std::thread::scope(|s| {
            for (class, line) in format.classes {
                s.spawn(move || assert_tool_fixture_stops(format, *class, line));
            }
        });
    }
}

#[test]
fn contain_001_registry_covers_every_required_format() {
    let have: Vec<&str> = registry().iter().map(|f| f.format).collect();
    let missing: Vec<&str> = REQUIRED_FORMATS.iter().copied().filter(|f| !have.contains(f)).collect();
    assert!(missing.is_empty(), "CONTAIN-001 fails: no fixtures for {missing:?}");
    for f in registry() {
        assert!(missing_classes(f).is_empty(), "CONTAIN-001 fails: {} lacks {:?}", f.format, missing_classes(f));
    }
}

#[test]
fn contain_001_a_format_missing_a_class_is_caught() {
    const PARTIAL: FormatFixtures = FormatFixtures { classes: &[(ToolClass::ToolUse, r#"{"type":"tool_use"}"#)], ..TEST_JSONL };
    assert_eq!(missing_classes(&PARTIAL), ["command", "file_change", "mcp_call", "web_search", "unknown_non_text"]);
    assert!(missing_classes(&TEST_JSONL).is_empty());
}

#[test]
fn neg_text_only_stream_completes_with_its_answer() {
    for format in registry() {
        let rig = Rig::new();
        let mut events = Vec::new();
        let end = run_lines(&rig, format.text_only, "exit 0", (format.detector)(Gate::Now), &mut events);
        assert_eq!(end, RunEnd::Answer(format.answer.into()), "{}", format.format);
        assert!(events.is_empty(), "{}: no notice for plain text", format.format);
        // The run's end kills what the runtime left behind.
        assert!(heartbeat_stops(&rig.rec("hb"), Duration::from_secs(1)), "{}", format.format);
        assert!(rig.run_folder_gone(), "{}", format.format);
    }
}

#[test]
fn contain_001_init_listing_a_tool_kills_before_the_prompt_is_written() {
    let rig = Rig::new();
    let stdin = rig.rec("stdin");
    let tail = format!("sleep 0.5\nc=$(head -c 1)\n[ -n \"$c\" ] && echo read > {}\nsleep 30", sq(&stdin.to_string_lossy()));
    let mut events = Vec::new();
    let init = r#"{"type":"init","tools":["Bash","Read"],"mcp_servers":[]}"#;
    let end = run_lines(&rig, &[init], &tail, Box::new(TestDetector(Gate::AfterInit)), &mut events);
    assert_eq!(end, RunEnd::Stopped { kind: "init_lists_tool" });
    assert_eq!(events, ["notice"]);
    std::thread::sleep(Duration::from_millis(700));
    assert!(!stdin.exists(), "nothing was read from stdin");
    assert!(heartbeat_stops(&rig.rec("hb"), Duration::from_secs(1)));
    assert!(rig.run_folder_gone());
}

#[test]
fn contain_001_init_listing_an_mcp_server_kills_too() {
    let rig = Rig::new();
    let mut events = Vec::new();
    let init = r#"{"type":"init","tools":[],"mcp_servers":["mail"]}"#;
    let end = run_lines(&rig, &[init], "sleep 30", Box::new(TestDetector(Gate::AfterInit)), &mut events);
    assert_eq!(end, RunEnd::Stopped { kind: "init_lists_tool" });
}

#[test]
fn neg_clean_init_releases_the_prompt_on_stdin() {
    let rig = Rig::new();
    let stdin = rig.rec("stdin");
    let tail = format!("cat > {}\necho '{{\"type\":\"final\",\"text\":\"ok\"}}'", sq(&stdin.to_string_lossy()));
    let mut events = Vec::new();
    let init = r#"{"type":"init","tools":[],"mcp_servers":[]}"#;
    let end = run_lines(&rig, &[init], &tail, Box::new(TestDetector(Gate::AfterInit)), &mut events);
    assert_eq!(end, RunEnd::Answer("ok".into()));
    assert_eq!(std::fs::read_to_string(&stdin).unwrap(), PROMPT);
}

#[test]
fn contain_001_unparsable_and_over_cap_lines_kill() {
    let rig = Rig::new();
    let mut events = Vec::new();
    let end = run_lines(&rig, &["not json at all"], "sleep 30", Box::new(TestDetector(Gate::Now)), &mut events);
    assert_eq!(end, RunEnd::Stopped { kind: "unparsable" });
    assert!(heartbeat_stops(&rig.rec("hb"), Duration::from_secs(1)));

    let rig = Rig::new();
    let program = rig.fake(&[], "head -c 4096 /dev/zero | tr '\\0' 'a'\necho\nsleep 30");
    let r = recipe("fake", true, Home::Real, vec![]);
    let small = Limits { line_bytes: 1024, ..limits(10) };
    let mut events = Vec::new();
    let end = run_fake(&program, &r, PROMPT, &[], &host(&rig.home()), &Sandbox::None, small, TestDetector(Gate::Now), &mut events).end;
    assert_eq!(end, RunEnd::Stopped { kind: "line_too_long" });
    assert_eq!(events, ["notice"]);
}

#[test]
fn contain_001_stdout_over_cap_is_too_large() {
    let rig = Rig::new();
    let program = rig.fake(&[], "while :; do echo '{\"type\":\"note\"}'; done");
    let r = recipe("fake", true, Home::Real, vec![]);
    let small = Limits { stdout_bytes: 64 * 1024, ..limits(10) };
    let mut events = Vec::new();
    let end = run_fake(&program, &r, PROMPT, &[], &host(&rig.home()), &Sandbox::None, small, TestDetector(Gate::Now), &mut events).end;
    assert_eq!(end, RunEnd::TooLarge);
    assert!(heartbeat_stops(&rig.rec("hb"), Duration::from_secs(1)));
    assert!(rig.run_folder_gone());
}

#[test]
fn contain_001_silence_times_out_and_kills_the_group() {
    let rig = Rig::new();
    // One note once the grandchild is running, then silence: the idle clock starts there.
    let program = rig.fake(&[r#"{"type":"note"}"#], "sleep 30");
    let r = recipe("fake", true, Home::Real, vec![]);
    let quiet = Limits { idle: Duration::from_millis(800), ..limits(10) };
    let mut events = Vec::new();
    // macOS looks a new program over on its first run (seconds when tests run side by side):
    // run it once first, so the idle clock measures the runtime's silence and not that.
    let _ = std::process::Command::new(&program).arg("warm").status();
    let end = run_fake(&program, &r, PROMPT, &[], &host(&rig.home()), &Sandbox::None, quiet, TestDetector(Gate::Now), &mut events).end;
    assert_eq!(end, RunEnd::TimedOut);
    assert_eq!(end.error_kind(), Some(ErrorKind::TimedOut { before_first_byte: true }));
    assert!(heartbeat_stops(&rig.rec("hb"), Duration::from_secs(1)));
    assert!(rig.run_folder_gone());
}

#[test]
fn contain_001_cancel_kills_the_grandchild_within_a_second() {
    let rig = Rig::new();
    let program = rig.fake(&[r#"{"type":"text","text":"thinking"}"#], "sleep 30");
    let r = recipe("fake", true, Home::Real, vec![]);
    let host = host(&rig.home());
    let spec =
        RunSpec { program, recipe: &r, instructions: "i", prompt: PROMPT, secrets: &[], host: &host, sandbox: &Sandbox::None, limits: limits(10) };
    let cancel = AtomicBool::new(false);
    let end = std::thread::scope(|s| {
        s.spawn(|| {
            // Called off once the fake and its grandchild are running.
            assert!(wait_for_file(&rig.rec("hb")));
            std::thread::sleep(Duration::from_millis(200));
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        run(&spec, TestDetector(Gate::Now), &cancel, &mut |_: Notice| {}).end
    });
    assert_eq!(end, RunEnd::Cancelled);
    assert!(heartbeat_stops(&rig.rec("hb"), Duration::from_secs(1)));
    assert!(rig.run_folder_gone());
}

#[test]
fn contain_001_a_runtime_ending_without_a_final_answers_with_its_text() {
    let rig = Rig::new();
    let mut events = Vec::new();
    let end = run_lines(&rig, &[r#"{"type":"text","text":"Friday."}"#], "exit 0", Box::new(TestDetector(Gate::Now)), &mut events);
    assert_eq!(end, RunEnd::Answer("Friday.".into()));
    let rig = Rig::new();
    let end = run_lines(&rig, &[], "exit 3", Box::new(TestDetector(Gate::Now)), &mut events);
    assert_eq!(end, RunEnd::Broken { kind: "exit_status" });
}

#[test]
fn contain_001_a_refused_program_spawns_nothing() {
    let rig = Rig::new();
    let program = rig.fake(&[], "exit 0");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o777)).unwrap();
    }
    let r = recipe("fake", true, Home::Real, vec![]);
    let mut events = Vec::new();
    let end = run_fake(&program, &r, PROMPT, &[], &host(&rig.home()), &Sandbox::None, limits(5), TestDetector(Gate::Now), &mut events).end;
    assert_eq!(end, RunEnd::Broken { kind: "program_refused" });
    assert!(!rig.rec("pwd").exists(), "a world-writable program is never run");
}

#[test]
fn contain_001_stderr_is_kept_capped_and_apart_from_the_answer() {
    let rig = Rig::new();
    let program = rig.fake(&[], "head -c 200000 /dev/zero | tr '\\0' 'e' >&2\necho '{\"type\":\"final\",\"text\":\"ok\"}'");
    let r = recipe("fake", true, Home::Real, vec![]);
    let small = Limits { stderr_bytes: 1000, ..limits(10) };
    let mut events = Vec::new();
    let report = run_fake(&program, &r, PROMPT, &[], &host(&rig.home()), &Sandbox::None, small, TestDetector(Gate::Now), &mut events);
    assert_eq!(report.end, RunEnd::Answer("ok".into()));
    assert_eq!(report.stderr.len(), 1000, "stderr is read to the end and kept to its cap");
}
