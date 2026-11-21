// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! SCN-RT-SANDBOX-001 (B4, 14.15): under the OS sandbox a fake runtime can't read a sentinel in a
//! fixture home or write outside its run folder, while reading the config path its recipe allows
//! succeeds (`neg_`). Where there is no sandbox, a recipe that needs one (Codex) is not started
//! and the run ends `no_sandbox`.
//!
//! macOS runs the real `sandbox-exec`. On Linux the Landlock launcher is used when
//! `MAILCRAFT_TEST_LAUNCHER` names a built `mailcraft-launcher` and the kernel has Landlock, else
//! bubblewrap when installed; with neither, only the refusal is checked there.

#![cfg(unix)]

use std::path::{Path, PathBuf};

use mailcraft_ai::runtime::contain::Gate;
use mailcraft_model::ai::ErrorKind;

use crate::ai::runtime::sandbox::{self, Sandbox};
use crate::ai::runtime::spawn::test_support::{TestDetector, host, limits, recipe, run_fake, script, sq};
use crate::ai::runtime::spawn::{Home, RunEnd};

const SENTINEL: &str = "SENTINEL-6a5b4c3d-the-owners-private-file";
const CONFIG: &str = "config-of-the-cli";

struct Rig {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Rig {
    fn new() -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        for d in ["bin", "home", "home/.cli", "outside"] {
            std::fs::create_dir(root.join(d)).unwrap();
        }
        std::fs::write(root.join("home/sentinel.txt"), SENTINEL).unwrap();
        std::fs::write(root.join("home/.cli/config.json"), CONFIG).unwrap();
        Rig { _dir: dir, root }
    }
    fn p(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }
    /// A fake that tries each access and answers with what happened.
    fn fake(&self) -> PathBuf {
        let q = |rel: &str| sq(&self.p(rel).to_string_lossy());
        let body = format!(
            "if cat {sentinel} >/dev/null 2>&1; then r=read_ok; else r=read_denied; fi\n\
             if : > {stray} 2>/dev/null; then w=write_ok; else w=write_denied; fi\n\
             if [ \"$(cat {config} 2>/dev/null)\" = {config_text} ]; then c=conf_ok; else c=conf_denied; fi\n\
             if : > \"$TMPDIR/scratch\" 2>/dev/null; then t=tmp_ok; else t=tmp_denied; fi\n\
             printf '{{\"type\":\"final\",\"text\":\"%s %s %s %s\"}}\\n' $r $w $c $t",
            sentinel = q("home/sentinel.txt"),
            stray = q("outside/stray.txt"),
            config = q("home/.cli/config.json"),
            config_text = sq(CONFIG),
        );
        script(&self.p("bin"), "fake-runtime", &body)
    }
    fn run(&self, sandbox: &Sandbox) -> RunEnd {
        let mut r = recipe("cli_like", false, Home::Real, vec![]);
        r.read_paths = vec![self.p("home/.cli")];
        let mut events = Vec::new();
        run_fake(&self.fake(), &r, "prompt", &[], &host(&self.p("home")), sandbox, limits(10), TestDetector(Gate::Now), &mut events).end
    }
}

/// The sandbox the tests use on this computer, when there is one.
fn test_sandbox() -> Option<Sandbox> {
    #[cfg(target_os = "macos")]
    {
        Some(sandbox::detect()).filter(|s| *s == Sandbox::Seatbelt)
    }
    #[cfg(target_os = "linux")]
    {
        let launcher = std::env::var_os("MAILCRAFT_TEST_LAUNCHER").map(PathBuf::from).filter(|p| p.is_file());
        let landlock = launcher.as_deref().is_some_and(sandbox::linux::landlock_probe);
        Some(sandbox::linux::choose(landlock, launcher, sandbox::linux::bwrap_path())).filter(|s| *s != Sandbox::None)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

fn answer_words(end: &RunEnd) -> Vec<String> {
    match end {
        RunEnd::Answer(t) => t.split(' ').map(str::to_owned).collect(),
        other => panic!("the fake didn't answer: {other:?}"),
    }
}

#[test]
fn sandbox_denies_the_home_sentinel_and_a_write_outside_the_run_folder() {
    let Some(sb) = test_sandbox() else {
        if cfg!(target_os = "macos") {
            panic!("macOS must offer sandbox-exec for SCN-RT-SANDBOX-001");
        }
        eprintln!("no sandbox to test on this computer; the refusal is tested below");
        return;
    };
    let rig = Rig::new();
    let words = answer_words(&rig.run(&sb));
    assert!(words.contains(&"read_denied".to_owned()), "{words:?}");
    assert!(words.contains(&"write_denied".to_owned()), "{words:?}");
    assert!(words.contains(&"tmp_ok".to_owned()), "writes inside the run folder work: {words:?}");
    assert!(!rig.p("outside/stray.txt").exists(), "nothing was written outside");
}

#[test]
fn neg_sandbox_lets_the_recipes_config_path_be_read() {
    let Some(sb) = test_sandbox() else {
        if cfg!(target_os = "macos") {
            panic!("macOS must offer sandbox-exec for SCN-RT-SANDBOX-001");
        }
        return;
    };
    let rig = Rig::new();
    let words = answer_words(&rig.run(&sb));
    assert!(words.contains(&"conf_ok".to_owned()), "{words:?}");
}

#[test]
fn sandbox_control_without_it_everything_is_reachable() {
    // The same fake with no sandbox reads and writes everything: the denials above are the sandbox's.
    let rig = Rig::new();
    let mut r = recipe("claude_code_like", true, Home::Real, vec![]);
    r.read_paths = vec![rig.p("home/.cli")];
    let mut events = Vec::new();
    let end = run_fake(&rig.fake(), &r, "p", &[], &host(&rig.p("home")), &Sandbox::None, limits(10), TestDetector(Gate::Now), &mut events).end;
    assert_eq!(answer_words(&end), ["read_ok", "write_ok", "conf_ok", "tmp_ok"]);
}

#[test]
fn sandbox_codex_without_a_sandbox_is_not_started() {
    let rig = Rig::new();
    let ran = rig.p("outside/ran");
    let program = script(&rig.p("bin"), "codex-like", &format!("touch {}", sq(&ran.to_string_lossy())));
    let r = recipe("codex_like", false, Home::PerRun, vec![]);
    let mut events = Vec::new();
    let end = run_fake(&program, &r, "p", &[], &host(&rig.p("home")), &Sandbox::None, limits(5), TestDetector(Gate::Now), &mut events).end;
    assert_eq!(end, RunEnd::NoSandbox);
    assert_eq!(end.error_kind(), Some(ErrorKind::NoSandbox));
    assert!(!ran.exists(), "Codex was not started");
}

#[test]
fn sandbox_this_platform_offers_what_it_should() {
    let found = sandbox::detect();
    if cfg!(target_os = "macos") {
        assert_eq!(found, Sandbox::Seatbelt);
    } else if !cfg!(target_os = "linux") {
        assert_eq!(found, Sandbox::None, "no sandbox here, so Codex is not offered");
    }
}

#[test]
fn sandbox_profile_paths_are_real_paths() {
    let rig = Rig::new();
    let policy = sandbox::Policy::for_run(&[], Path::new("/bin/sh"), Path::new("/bin/sh"), &[rig.p("home/.cli")], &rig.p("bin"), &[], &[]).unwrap();
    assert!(policy.read.contains(&rig.p("home/.cli")));
    assert!(!policy.read.iter().any(|p| p == &rig.p("home")), "the home itself is never readable");
}

#[test]
fn sandbox_denies_writes_to_the_clis_configuration_inside_its_writable_folder() {
    // Real enforcement where the sandbox carries guards (Seatbelt; bubblewrap for files that
    // exist). Landlock can't take a write back beneath a writable folder (`linux::launcher_args`).
    let Some(sb) = test_sandbox().filter(|s| !matches!(s, Sandbox::Landlock { .. })) else {
        if cfg!(target_os = "macos") {
            panic!("macOS must offer sandbox-exec");
        }
        return;
    };
    let rig = Rig::new();
    let claude = rig.p("home/.claude");
    std::fs::create_dir_all(claude.join("hooks")).unwrap();
    std::fs::create_dir_all(claude.join("projects")).unwrap();
    std::fs::write(claude.join("settings.json"), "{}").unwrap();
    std::fs::write(claude.join("CLAUDE.md"), "owner's").unwrap();
    let q = |p: PathBuf| sq(&p.to_string_lossy());
    let body = format!(
        "if echo x >> {settings} 2>/dev/null; then a=settings_ok; else a=settings_denied; fi\n\
         if echo x >> {md} 2>/dev/null; then b=md_ok; else b=md_denied; fi\n\
         if : > {hook} 2>/dev/null; then c=hook_ok; else c=hook_denied; fi\n\
         if : > {session} 2>/dev/null; then d=session_ok; else d=session_denied; fi\n\
         if cat {settings} >/dev/null 2>&1; then e=read_ok; else e=read_denied; fi\n\
         printf '{{\"type\":\"final\",\"text\":\"%s %s %s %s %s\"}}\\n' $a $b $c $d $e",
        settings = q(claude.join("settings.json")),
        md = q(claude.join("CLAUDE.md")),
        hook = q(claude.join("hooks/pre.sh")),
        session = q(claude.join("projects/session.jsonl")),
    );
    let program = script(&rig.p("bin"), "claude-like", &body);
    let mut r = recipe("claude_code_like", false, Home::Real, vec![]);
    r.read_paths = vec![claude.clone()];
    r.write_paths = vec![claude.clone()];
    r.deny_write = crate::ai::runtime::claude_code::guarded(&rig.p("home"));
    let mut events = Vec::new();
    let end = run_fake(&program, &r, "p", &[], &host(&rig.p("home")), &sb, limits(10), TestDetector(Gate::Now), &mut events).end;
    assert_eq!(answer_words(&end), ["settings_denied", "md_denied", "hook_denied", "session_ok", "read_ok"]);
    assert_eq!(std::fs::read_to_string(claude.join("settings.json")).unwrap(), "{}");
    assert!(!claude.join("hooks/pre.sh").exists());
}
