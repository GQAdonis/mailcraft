// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! SCN-RT-CONTAIN-002 (B4, 14.13): a spawned runtime's environment is the allow-list and its
//! recipe's variables, nothing else. The fake runtime dumps its environment with `/usr/bin/env`;
//! the variables a POSIX shell adds by itself (`PWD`, `SHLVL`, `_`) are the shell's, not MailCraft's,
//! and are set aside. The negative control runs in a child process of this test binary whose own
//! environment holds `OPENAI_API_KEY`, `ANTHROPIC_API_KEY` and `MAILCRAFT_TEST_SECRET`.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mailcraft_ai::runtime::contain::Gate;

use crate::ai::runtime::sandbox::Sandbox;
use crate::ai::runtime::spawn::test_support::{
    SHELL_ADDED, TEST_PATH, TestDetector, host, is_inner, limits, recipe, rerun_with_env, run_fake, script, sq,
};
use crate::ai::runtime::spawn::{Home, HostEnv, Recipe, RunEnd};

const SECRET_VALUE: &str = "planted-secret-6d1f0b2a-9e8c-4b7d-a3f5-0c9e8d7b6a5f";
const OPENAI_VALUE: &str = "sk-test-openai-00000000000000000000000000000000";
const ANTHROPIC_VALUE: &str = "sk-ant-test-0000000000000000000000000000000000";

struct Dump {
    vars: BTreeMap<String, String>,
    raw: String,
}

/// Runs a fake that dumps its environment, and reads the dump.
fn dump_env(recipe: &Recipe, host: &HostEnv, root: &Path) -> Dump {
    let file = root.join("env.txt");
    let body = format!("/usr/bin/env > {}\necho '{{\"type\":\"final\",\"text\":\"ok\"}}'", sq(&file.to_string_lossy()));
    let program = script(root, "fake-runtime", &body);
    let mut events = Vec::new();
    let end = run_fake(&program, recipe, "prompt", &[], host, &Sandbox::None, limits(10), TestDetector(Gate::Now), &mut events).end;
    assert_eq!(end, RunEnd::Answer("ok".into()));
    let raw = std::fs::read_to_string(&file).unwrap();
    let vars =
        raw.lines().filter_map(|l| l.split_once('=')).filter(|(k, _)| !SHELL_ADDED.contains(k)).map(|(k, v)| (k.to_owned(), v.to_owned())).collect();
    Dump { vars, raw }
}

fn names(d: &Dump) -> Vec<&str> {
    d.vars.keys().map(String::as_str).collect()
}

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::create_dir(root.join("home")).unwrap();
    (dir, root)
}

#[test]
fn env_of_a_real_home_runtime_is_only_the_allow_list() {
    let (_d, root) = fixture();
    let r = recipe("claude_code_like", true, Home::Real, vec![]);
    let d = dump_env(&r, &host(&root.join("home")), &root);
    assert_eq!(names(&d), ["HOME", "LANG", "PATH", "TMPDIR", "USER"]);
    assert_eq!(d.vars["PATH"], TEST_PATH);
    assert_eq!(d.vars["HOME"], root.join("home").to_string_lossy());
    assert!(d.vars["TMPDIR"].contains("mailcraft-run-"), "the temporary folder is inside the run folder");
}

#[test]
fn env_of_a_per_run_home_runtime_adds_only_its_recipe_vars() {
    let (_d, root) = fixture();
    let mut r = recipe("codex_like", true, Home::PerRun, vec![]);
    let codex_home = root.join("home").join(".codex");
    r.env = vec![("CODEX_HOME".into(), codex_home.to_string_lossy().into_owned())];
    let d = dump_env(&r, &host(&root.join("home")), &root);
    assert_eq!(names(&d), ["CODEX_HOME", "HOME", "LANG", "PATH", "TMPDIR", "USER"]);
    assert_eq!(d.vars["CODEX_HOME"], codex_home.to_string_lossy());
    assert!(d.vars["HOME"].contains("mailcraft-run-") && d.vars["HOME"].ends_with("/home"), "the run's own home");
}

#[test]
fn env_a_recipe_naming_a_key_variable_is_refused() {
    let (_d, root) = fixture();
    let mut r = recipe("bad", true, Home::Real, vec![]);
    r.env = vec![("OPENAI_API_KEY".into(), "x".into())];
    let program = script(&root, "fake-runtime", &format!("touch {}", sq(&root.join("ran").to_string_lossy())));
    let mut events = Vec::new();
    let end = run_fake(&program, &r, "p", &[], &host(&root.join("home")), &Sandbox::None, limits(5), TestDetector(Gate::Now), &mut events).end;
    assert_eq!(end, RunEnd::Refused { kind: "forbidden_variable" });
    assert!(!root.join("ran").exists());
}

#[test]
fn neg_planted_secrets_in_mailcrafts_env_are_absent_from_the_child() {
    if !is_inner() {
        let (_d, root) = fixture();
        let home = root.join("home");
        assert!(rerun_with_env(
            module_path!(),
            "neg_planted_secrets_in_mailcrafts_env_are_absent_from_the_child",
            &[
                ("OPENAI_API_KEY", OPENAI_VALUE),
                ("ANTHROPIC_API_KEY", ANTHROPIC_VALUE),
                ("MAILCRAFT_TEST_SECRET", SECRET_VALUE),
                ("HTTPS_PROXY", "http://proxy.example:3128"),
                ("NODE_OPTIONS", "--require /tmp/x.js"),
                // A fixture home, never the owner's.
                ("HOME", &home.to_string_lossy()),
            ],
        ));
        return;
    }
    // The inner run: MailCraft's own environment holds the planted variables.
    assert_eq!(std::env::var("MAILCRAFT_TEST_SECRET").as_deref(), Ok(SECRET_VALUE));
    let (_d, root) = fixture();
    let host = HostEnv::from_process(Some(TEST_PATH.into()));
    for (home, name) in [(Home::Real, "claude_code_like"), (Home::PerRun, "codex_like")] {
        let r = recipe(name, true, home, vec![]);
        let d = dump_env(&r, &host, &root);
        for planted in ["OPENAI_API_KEY", "ANTHROPIC_API_KEY", "MAILCRAFT_TEST_SECRET", "HTTPS_PROXY", "NODE_OPTIONS"] {
            assert!(!d.vars.contains_key(planted), "{name}: {planted} reached the child");
        }
        for value in [SECRET_VALUE, OPENAI_VALUE, ANTHROPIC_VALUE, "proxy.example"] {
            assert!(!d.raw.contains(value), "{name}: a planted value reached the child");
        }
        // LANG and USER come only when MailCraft has them.
        for n in names(&d) {
            assert!(["HOME", "LANG", "PATH", "TMPDIR", "USER"].contains(&n), "{name}: {n} isn't on the allow-list");
        }
        for n in ["HOME", "PATH", "TMPDIR"] {
            assert!(d.vars.contains_key(n), "{name}: {n} is missing");
        }
        std::fs::remove_file(root.join("env.txt")).unwrap();
        std::fs::remove_file(root.join("fake-runtime")).unwrap();
    }
}
