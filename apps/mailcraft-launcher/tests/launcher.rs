// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The launcher restricts itself before it executes the runtime (Linux, with Landlock). Skipped,
//! with a message, on a kernel that enforces none (the launcher then refuses to run anything,
//! which is checked instead).
#![cfg(target_os = "linux")]

use std::process::Command;

const LAUNCHER: &str = env!("CARGO_BIN_EXE_mailcraft-launcher");
const SYSTEM: [&str; 6] = ["/usr", "/bin", "/lib", "/lib64", "/etc", "/proc"];

fn landlock() -> bool {
    Command::new(LAUNCHER).arg("--probe").status().map(|s| s.success()).unwrap_or(false)
}

fn launch(read: &[&str], write: &[&str], program: &[&str]) -> std::process::Output {
    let mut c = Command::new(LAUNCHER);
    for r in read {
        c.args(["--read", r]);
    }
    for w in write {
        c.args(["--write", w]);
    }
    c.arg("--").args(program);
    c.output().unwrap()
}

#[test]
fn launcher_denies_a_read_outside_its_paths_and_passes_the_status_on() {
    let dir = tempfile_dir();
    let secret = format!("{dir}/b");
    std::fs::write(&secret, "secret").unwrap();
    let out = launch(&SYSTEM, &[], &["/bin/cat", &secret]);
    if !landlock() {
        assert_eq!(out.status.code(), Some(125), "without Landlock nothing is run");
        return;
    }
    assert!(!out.status.success(), "reading /b is denied");
    assert_eq!(out.status.code(), Some(1), "cat's own status comes back");
    assert!(!String::from_utf8_lossy(&out.stdout).contains("secret"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn launcher_allows_reads_beneath_a_read_path_and_writes_beneath_a_write_path() {
    if !landlock() {
        return;
    }
    let dir = tempfile_dir();
    std::fs::write(format!("{dir}/ok"), "fine").unwrap();
    let mut read: Vec<&str> = SYSTEM.to_vec();
    read.push(&dir);
    let out = launch(&read, &[], &["/bin/cat", &format!("{dir}/ok")]);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "fine");
    let target = format!("{dir}/w");
    let out = launch(&SYSTEM, &[&dir], &["/bin/sh", "-c", &format!("echo hi > {target}")]);
    assert!(out.status.success());
    let out = launch(&SYSTEM, &[], &["/bin/sh", "-c", &format!("echo hi > {dir}/denied")]);
    assert!(!out.status.success());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn launcher_refuses_bad_arguments() {
    let out = launch(&["relative"], &[], &["/bin/true"]);
    assert_eq!(out.status.code(), Some(2));
}

fn tempfile_dir() -> String {
    let base = std::env::temp_dir().join(format!("mailcraft-launcher-test-{}-{:?}", std::process::id(), std::thread::current().id()));
    std::fs::create_dir_all(&base).unwrap();
    std::fs::canonicalize(base).unwrap().to_string_lossy().into_owned()
}
