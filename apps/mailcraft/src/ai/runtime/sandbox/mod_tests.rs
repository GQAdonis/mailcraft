// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The policy and the wrapping of a run in its sandbox (B4, 14.6–14.9).

use super::*;

#[test]
fn policy_reads_system_install_and_recipe_paths_and_writes_the_run_folder() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    for d in ["install/bin", "cfg", "run"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    let program = root.join("install/bin/cli");
    let p = Policy::for_run(&[PathBuf::from("/usr")], &program, &program, &[root.join("cfg")], &root.join("run"), &[], &[]).unwrap();
    assert!(p.read.contains(&std::fs::canonicalize("/usr").unwrap()));
    assert!(p.read.contains(&root.join("install/bin")));
    assert!(p.read.contains(&root.join("cfg")));
    assert_eq!(p.write, vec![root.join("run")]);
    assert_eq!(p.read.iter().filter(|r| **r == root.join("install/bin")).count(), 1, "no duplicates");
}

#[test]
fn policy_refuses_relative_paths() {
    let r = Policy::for_run(&[], Path::new("/bin/sh"), Path::new("/bin/sh"), &[PathBuf::from("relative/cfg")], Path::new("/tmp"), &[], &[]);
    assert_eq!(r, Err(SandboxError::BadPath));
}

#[test]
fn path_text_refuses_control_characters() {
    assert_eq!(path_text(Path::new("/a\nb")), Err(SandboxError::BadPath));
    assert_eq!(path_text(Path::new("rel")), Err(SandboxError::BadPath));
    assert_eq!(path_text(Path::new("/ok/path")), Ok("/ok/path"));
}

#[test]
fn wrap_without_a_sandbox_runs_the_program_as_is() {
    let (p, a) = wrap(&Sandbox::None, &Policy::default(), Path::new("/tmp"), Path::new("/tmp/w"), Path::new("/bin/cli"), vec!["-p".into()]).unwrap();
    assert_eq!((p, a), (PathBuf::from("/bin/cli"), vec!["-p".to_owned()]));
}

#[test]
fn wrap_under_seatbelt_runs_sandbox_exec_with_a_0600_profile_in_the_run_folder() {
    let dir = tempfile::tempdir().unwrap();
    let run = std::fs::canonicalize(dir.path()).unwrap();
    let policy = Policy { read: vec![PathBuf::from("/usr")], write: vec![run.clone()], deny_write: vec![] };
    let (p, a) = wrap(&Sandbox::Seatbelt, &policy, &run, &run.join("work"), Path::new("/bin/cli"), vec!["-p".into()]).unwrap();
    assert_eq!(p, PathBuf::from(macos::SANDBOX_EXEC));
    let profile = run.join(macos::PROFILE_FILE);
    assert_eq!(a, vec!["-f".to_owned(), profile.to_string_lossy().into_owned(), "/bin/cli".into(), "-p".into()]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(std::fs::metadata(&profile).unwrap().permissions().mode() & 0o777, 0o600);
    }
}

#[test]
fn wrap_under_landlock_and_bwrap_puts_the_program_after_the_double_dash() {
    let policy = Policy { read: vec![PathBuf::from("/usr")], write: vec![PathBuf::from("/run/x")], deny_write: vec![] };
    let launcher = PathBuf::from("/opt/mc/mailcraft-launcher");
    let (p, a) = wrap(
        &Sandbox::Landlock { launcher: launcher.clone() },
        &policy,
        Path::new("/run/x"),
        Path::new("/run/x/work"),
        Path::new("/bin/cli"),
        vec!["-p".into()],
    )
    .unwrap();
    assert_eq!(p, launcher);
    assert_eq!(a, ["--read", "/usr", "--write", "/run/x", "--", "/bin/cli", "-p"]);
    let (p, a) = wrap(
        &Sandbox::Bwrap { bwrap: PathBuf::from("/usr/bin/bwrap") },
        &policy,
        Path::new("/run/x"),
        Path::new("/run/x/work"),
        Path::new("/bin/cli"),
        vec![],
    )
    .unwrap();
    assert_eq!(p, PathBuf::from("/usr/bin/bwrap"));
    assert_eq!(a.iter().rev().take(2).collect::<Vec<_>>(), ["/bin/cli", "--"]);
}

#[test]
fn sandbox_kinds_are_named() {
    assert_eq!(Sandbox::Seatbelt.kind(), "seatbelt");
    assert_eq!(Sandbox::None.kind(), "none");
    assert_eq!(SandboxError::BadPath.kind(), "sandbox_bad_path");
}

#[test]
fn guards_are_made_real_and_kept_once() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::create_dir_all(root.join("home/.claude/hooks")).unwrap();
    let program = PathBuf::from("/bin/sh");
    let guards = [
        Guard::Tree(root.join("home/.claude/hooks")),
        Guard::Tree(root.join("home/.claude/hooks")),
        Guard::Prefix(root.join("home/.claude/settings")),
        Guard::File(root.join("home/.claude/CLAUDE.md")),
    ];
    let p = Policy::for_run(&[], &program, &program, &[], &root.join("run"), &[root.join("home/.claude")], &guards).unwrap();
    assert_eq!(p.deny_write.len(), 3, "duplicates dropped");
    assert!(p.deny_write.contains(&Guard::Prefix(root.join("home/.claude/settings"))));
    let bad = Policy::for_run(&[], &program, &program, &[], &root.join("run"), &[], &[Guard::File(PathBuf::from("relative"))]);
    assert_eq!(bad, Err(SandboxError::BadPath));
}
