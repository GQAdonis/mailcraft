// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Linux's sandbox choice and argv (B4, 14.8), tested on every platform.

use std::path::PathBuf;

use super::*;

#[test]
fn choose_prefers_the_launcher_with_landlock_then_bwrap_then_none() {
    let l = Some(PathBuf::from("/app/mailcraft-launcher"));
    let b = Some(PathBuf::from("/usr/bin/bwrap"));
    assert_eq!(choose(true, l.clone(), b.clone()), Sandbox::Landlock { launcher: PathBuf::from("/app/mailcraft-launcher") });
    assert_eq!(choose(false, l.clone(), b.clone()), Sandbox::Bwrap { bwrap: PathBuf::from("/usr/bin/bwrap") });
    assert_eq!(choose(true, None, b.clone()), Sandbox::Bwrap { bwrap: PathBuf::from("/usr/bin/bwrap") });
    assert_eq!(choose(false, l, None), Sandbox::None);
    assert_eq!(choose(true, None, None), Sandbox::None);
}

#[test]
fn launcher_args_list_reads_then_writes_then_the_program() {
    let policy = Policy { read: vec![PathBuf::from("/usr"), PathBuf::from("/etc")], write: vec![PathBuf::from("/tmp/run")], deny_write: vec![] };
    let a = launcher_args(&policy, "/bin/cli", vec!["-p".into()]).unwrap();
    assert_eq!(a, ["--read", "/usr", "--read", "/etc", "--write", "/tmp/run", "--", "/bin/cli", "-p"]);
}

#[test]
fn bwrap_args_bind_parents_first_and_keep_the_network() {
    let policy = Policy {
        read: vec![PathBuf::from("/home/o/.cli/sub"), PathBuf::from("/usr"), PathBuf::from("/proc")],
        write: vec![PathBuf::from("/tmp/run"), PathBuf::from("/dev/null")],
        deny_write: vec![],
    };
    let a = bwrap_args(&policy, std::path::Path::new("/tmp/run"), std::path::Path::new("/tmp/run/work"), "/bin/cli", vec![]).unwrap();
    assert!(!a.iter().any(|s| s == "--unshare-net" || s == "--unshare-all"), "the runtime talks to its vendor");
    assert!(a.contains(&"--die-with-parent".to_owned()));
    let pos = |s: &str| a.iter().position(|x| x == s).unwrap();
    assert!(pos("/usr") < pos("/home/o/.cli/sub"), "parents are bound before children");
    assert_eq!(a.get(pos("/tmp/run") - 1).map(String::as_str), Some("--bind-try"));
    assert_eq!(a.get(pos("/usr") - 1).map(String::as_str), Some("--ro-bind-try"));
    assert_eq!(a.iter().filter(|s| *s == "/proc").count(), 1, "/proc is the fresh one only");
    assert_eq!(&a[a.len() - 4..], ["--chdir", "/tmp/run/work", "--", "/bin/cli"]);
}

#[test]
fn bwrap_is_never_looked_for_on_path() {
    assert!(BWRAP_PATHS.iter().all(|p| p.starts_with('/')));
}

#[test]
fn system_read_paths_hold_no_home() {
    assert!(!SYSTEM_READ.iter().any(|p| p.starts_with("/home") || p.starts_with("/root") || *p == "/tmp"));
}

#[test]
fn proc_is_read_only_for_the_runtimes_own_process() {
    assert!(!SYSTEM_READ.contains(&"/proc"), "never every process's command line and environment");
    assert!(SYSTEM_READ.contains(&"/proc/self") && SYSTEM_READ.contains(&"/proc/thread-self"));
    // Made real, `/proc/self` would name MailCraft's process instead of the runtime's.
    assert_eq!(super::super::real(std::path::Path::new("/proc/self")).unwrap(), PathBuf::from("/proc/self"));
    let policy = Policy { read: SYSTEM_READ.iter().map(PathBuf::from).collect(), write: vec![PathBuf::from("/tmp/run")], deny_write: vec![] };
    let a = launcher_args(&policy, "/bin/cli", vec![]).unwrap();
    assert!(!a.iter().any(|s| s == "/proc"));
    assert!(a.windows(2).any(|w| w[0] == "--read" && w[1] == "/proc/self"));
    let b = bwrap_args(&policy, std::path::Path::new("/tmp/run"), std::path::Path::new("/tmp/run/work"), "/bin/cli", vec![]).unwrap();
    assert!(!b.iter().any(|s| s.starts_with("/proc/")), "bubblewrap mounts a fresh /proc instead");
}

#[test]
fn bwrap_binds_the_guarded_configuration_read_only_over_its_writable_folder() {
    let policy = Policy {
        read: vec![PathBuf::from("/usr")],
        write: vec![PathBuf::from("/home/o/.claude")],
        deny_write: vec![
            Guard::Prefix(PathBuf::from("/home/o/.claude/settings")),
            Guard::File(PathBuf::from("/home/o/.claude/settings.json")),
            Guard::Tree(PathBuf::from("/home/o/.claude/hooks")),
        ],
    };
    let a = bwrap_args(&policy, std::path::Path::new("/tmp/run"), std::path::Path::new("/tmp/run/work"), "/bin/cli", vec![]).unwrap();
    let pos = |s: &str| a.iter().position(|x| x == s).unwrap();
    assert_eq!(a[pos("/home/o/.claude") - 1], "--bind-try");
    for guarded in ["/home/o/.claude/settings.json", "/home/o/.claude/hooks"] {
        assert_eq!(a[pos(guarded) - 1], "--ro-bind-try", "{guarded}");
        assert!(pos(guarded) > pos("/home/o/.claude"), "{guarded} is bound after (over) its writable folder");
    }
    assert!(!a.iter().any(|s| s == "/home/o/.claude/settings"), "a prefix isn't a path to bind");
}
