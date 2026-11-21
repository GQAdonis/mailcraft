// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! SCN-RT-ARGV-001 (B4, 14.14): no prompt, mail, key or token on any runtime's argv. The fake
//! records its argv, its stdin and the instructions file's mode; the prompt quotes a message
//! carrying marker strings made by the test. A spawn whose argv would hold one is refused and
//! nothing runs. The negative control plants a secret in MailCraft's own environment.

#![cfg(unix)]

use std::path::{Path, PathBuf};

use mailcraft_ai::runtime::contain::Gate;

use crate::ai::runtime::sandbox::Sandbox;
use crate::ai::runtime::spawn::test_support::{TEST_PATH, TestDetector, host, is_inner, limits, recipe, rerun_with_env, run_fake, script, sq};
use crate::ai::runtime::spawn::{Arg, Home, HostEnv, INSTRUCTIONS_FILE, Recipe, RunEnd};

const MAIL_MARKER: &str = "MAILMARK-3b9f2c71-e4a8-4d0a-9c55-1f2e3d4c5b6a";
const SUBJECT_MARKER: &str = "SUBJMARK-a1b2c3d4-quarterly-figures";
const KEY: &str = "sk-test-KEYMARK-7c6b5a4f3e2d1c0b9a8f7e6d5c4b3a29";
const TOKEN: &str = "tok-TOKENMARK-0f1e2d3c4b5a69788796a5b4c3d2e1f0";
const PLANTED: &str = "planted-argv-secret-5e4d3c2b1a09";

fn mail_text() -> String {
    format!("Subject: {SUBJECT_MARKER}\nFrom: alice@example.org\n\n{MAIL_MARKER} The figures stay private until Friday.")
}

fn prompt() -> String {
    format!("Summarise this message.\n\n<message>\n{}\n</message>", mail_text())
}

fn secrets() -> Vec<String> {
    vec![mail_text(), KEY.into(), TOKEN.into()]
}

fn claude_like_args() -> Vec<Arg> {
    ["-p", "--output-format", "stream-json", "--verbose", "--append-system-prompt-file"]
        .iter()
        .map(|s| Arg::Lit((*s).into()))
        .chain([Arg::InstructionsFile, Arg::Lit("--model".into()), Arg::Lit("test-model".into())])
        .collect()
}

struct Recorded {
    argv: Vec<String>,
    stdin: String,
    mode: String,
    env: String,
}

fn recording_fake(root: &Path) -> PathBuf {
    let r = |n: &str| sq(&root.join(n).to_string_lossy());
    let body = format!(
        "printf '%s\\n' \"$0\" \"$@\" > {argv}\n\
         cat > {stdin}\n\
         f=''; prev=''; for a in \"$@\"; do [ \"$prev\" = --append-system-prompt-file ] && f=\"$a\"; prev=\"$a\"; done\n\
         if [ \"$(uname)\" = Darwin ]; then stat -f %Lp \"$f\"; else stat -c %a \"$f\"; fi > {mode}\n\
         /usr/bin/env > {env}\n\
         echo '{{\"type\":\"final\",\"text\":\"done\"}}'",
        argv = r("argv"),
        stdin = r("stdin"),
        mode = r("mode"),
        env = r("env"),
    );
    script(root, "fake-runtime", &body)
}

fn run_recorded(root: &Path, recipe: &Recipe, host: &HostEnv) -> (RunEnd, Option<Recorded>) {
    let program = recording_fake(root);
    let mut events = Vec::new();
    let end = run_fake(&program, recipe, &prompt(), &secrets(), host, &Sandbox::None, limits(10), TestDetector(Gate::Now), &mut events).end;
    let read = |n: &str| std::fs::read_to_string(root.join(n)).ok();
    let rec = read("argv").map(|argv| Recorded {
        argv: argv.lines().map(str::to_owned).collect(),
        stdin: read("stdin").unwrap_or_default(),
        mode: read("mode").unwrap_or_default().trim().to_owned(),
        env: read("env").unwrap_or_default(),
    });
    (end, rec)
}

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::create_dir(root.join("home")).unwrap();
    (dir, root)
}

/// Pieces of the prompt and the mail an argv must not hold.
fn forbidden_pieces() -> Vec<String> {
    let p = prompt();
    let mut v: Vec<String> = vec![MAIL_MARKER.into(), SUBJECT_MARKER.into(), KEY.into(), TOKEN.into(), "alice@example.org".into()];
    v.extend(p.lines().filter(|l| l.len() >= 16).map(str::to_owned));
    v
}

#[test]
fn argv_holds_no_prompt_mail_key_or_token_and_the_prompt_arrives_on_stdin() {
    let (_d, root) = fixture();
    let r = recipe("claude_code_like", true, Home::Real, claude_like_args());
    let (end, rec) = run_recorded(&root, &r, &host(&root.join("home")));
    assert_eq!(end, RunEnd::Answer("done".into()));
    let rec = rec.unwrap();
    for piece in forbidden_pieces() {
        assert!(!rec.argv.iter().any(|a| a.contains(&piece)), "argv holds {piece:?}");
    }
    assert_eq!(rec.stdin, prompt(), "the prompt arrives on stdin");
    let instructions = rec.argv.iter().find(|a| a.ends_with(INSTRUCTIONS_FILE)).unwrap();
    assert!(instructions.contains("mailcraft-run-"), "the instructions file is in the run folder");
    assert_eq!(rec.mode, "600", "the instructions file is 0600");
}

#[test]
fn argv_with_a_per_run_home_holds_nothing_either() {
    let (_d, root) = fixture();
    let mut r = recipe(
        "codex_like",
        false,
        Home::PerRun,
        vec![Arg::Lit("exec".into()), Arg::Lit("--json".into()), Arg::Lit("--cd".into()), Arg::WorkDir, Arg::Lit("-".into())],
    );
    r.may_run_unsandboxed = true;
    let (end, rec) = run_recorded(&root, &r, &host(&root.join("home")));
    // This fake has no instructions flag, so it records no mode: only argv and stdin matter here.
    assert_eq!(end, RunEnd::Answer("done".into()));
    let rec = rec.unwrap();
    for piece in forbidden_pieces() {
        assert!(!rec.argv.iter().any(|a| a.contains(&piece)), "argv holds {piece:?}");
    }
    assert_eq!(rec.stdin, prompt());
    let work = rec.argv.iter().find(|a| a.ends_with("/work")).unwrap();
    assert!(work.contains("mailcraft-run-"), "the working folder is the run's");
}

#[test]
fn argv_that_would_hold_mail_text_is_refused_and_nothing_runs() {
    let (_d, root) = fixture();
    let mut args = claude_like_args();
    args.push(Arg::Lit(format!("--message={}", mail_text())));
    let r = recipe("bad", true, Home::Real, args);
    let (end, rec) = run_recorded(&root, &r, &host(&root.join("home")));
    assert_eq!(end, RunEnd::Refused { kind: "argv_carries_secret" });
    assert!(rec.is_none(), "the fake never ran");
}

#[test]
fn argv_that_would_hold_a_key_or_token_is_refused_and_nothing_runs() {
    for secret in [KEY, TOKEN] {
        let (_d, root) = fixture();
        let r = recipe("bad", true, Home::Real, vec![Arg::Lit("--api-key".into()), Arg::Lit(secret.into())]);
        let (end, rec) = run_recorded(&root, &r, &host(&root.join("home")));
        assert_eq!(end, RunEnd::Refused { kind: "argv_carries_secret" });
        assert!(rec.is_none());
    }
}

#[test]
fn argv_that_would_hold_the_prompt_is_refused() {
    let (_d, root) = fixture();
    let r = recipe("bad", true, Home::Real, vec![Arg::Lit("-p".into()), Arg::Lit(prompt())]);
    let (end, rec) = run_recorded(&root, &r, &host(&root.join("home")));
    assert_eq!(end, RunEnd::Refused { kind: "argv_carries_secret" });
    assert!(rec.is_none());
}

#[test]
fn neg_planted_secret_is_absent_from_the_childs_argv_and_env() {
    if !is_inner() {
        let (_d, root) = fixture();
        assert!(rerun_with_env(
            module_path!(),
            "neg_planted_secret_is_absent_from_the_childs_argv_and_env",
            &[("MAILCRAFT_TEST_SECRET", PLANTED), ("HOME", &root.join("home").to_string_lossy())],
        ));
        return;
    }
    assert_eq!(std::env::var("MAILCRAFT_TEST_SECRET").as_deref(), Ok(PLANTED));
    let (_d, root) = fixture();
    let r = recipe("claude_code_like", true, Home::Real, claude_like_args());
    let (end, rec) = run_recorded(&root, &r, &HostEnv::from_process(Some(TEST_PATH.into())));
    assert_eq!(end, RunEnd::Answer("done".into()));
    let rec = rec.unwrap();
    assert!(!rec.argv.iter().any(|a| a.contains(PLANTED)), "the planted secret is on argv");
    assert!(!rec.env.contains(PLANTED) && !rec.env.contains("MAILCRAFT_TEST_SECRET"), "the planted secret is in the env");
}
