// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Hostile path overrides (B3, SCN-RT-PATH-001): what `ai.json` names as a runtime's program is
//! checked before detection runs it and again by the check every spawn makes; a path that isn't
//! safe is `Broken` with its reason and nothing runs.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use mailcraft_ai::runtime::detect::{Program, Source};

use super::detect_tests::Bench;
use crate::ai::runtime::detect::{Refusal, Status, check_executable, detect};

/// Detects Claude Code with `override_path` as the owner's path, a safe fake `claude` on the PATH
/// beside it (which must not be used instead); what was found and what ran.
fn with_override(bench: &Bench, override_path: &str) -> (Status, Vec<PathBuf>) {
    bench.fake(&bench.dir("bin").join("claude"), "on-path", "echo 'claude 1.0.0'", 0o755);
    let mut context = bench.context();
    context.overrides.insert(Program::Claude, override_path.to_string());
    let report = detect(&context);
    (report.status(Program::Claude).cloned().unwrap_or(Status::Missing), report.spawned)
}

fn assert_refused(bench: &Bench, path: &Path, override_text: &str, refusal: Refusal, fake: &str) {
    let (status, spawned) = with_override(bench, override_text);
    match &status {
        Status::Broken(broken) => {
            assert_eq!(broken.reason, refusal.reason(), "{override_text}");
            assert_eq!(broken.source, Source::Override);
        }
        other => panic!("{override_text}: not broken: {other:?}"),
    }
    assert!(spawned.is_empty(), "{override_text}: spawned {spawned:?}");
    assert_eq!(bench.ran(fake), None, "{override_text}: the fake ran");
    assert_eq!(bench.ran("on-path"), None, "{override_text}: the PATH was used instead");
    // The spawn's own re-check refuses it the same way.
    assert_eq!(check_executable(path), Err(refusal), "{override_text}");
}

#[test]
fn a_relative_override_is_broken_and_nothing_runs() {
    let bench = Bench::new();
    assert_refused(&bench, Path::new("bin/claude"), "bin/claude", Refusal::NotAbsolute, "never");
}

#[test]
fn a_symlink_to_a_world_writable_file_is_broken() {
    let bench = Bench::new();
    let target = bench.fake(&bench.dir("home").join("claude-real"), "target", "echo 'claude 2.1.326'", 0o777);
    let link = bench.dir("home").join("claude");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    assert_refused(&bench, &link, &link.to_string_lossy(), Refusal::Writable, "target");
}

#[test]
fn a_file_in_a_world_writable_folder_is_broken() {
    let bench = Bench::new();
    let open = bench.dir("open");
    std::fs::create_dir(&open).unwrap();
    let file = bench.fake(&open.join("claude"), "in-open", "echo 'claude 2.1.326'", 0o755);
    std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o777)).unwrap();
    assert_refused(&bench, &file, &file.to_string_lossy(), Refusal::ParentWritable, "in-open");
    std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn a_fifo_is_broken() {
    let bench = Bench::new();
    let fifo = bench.dir("home").join("claude");
    let made = std::process::Command::new("/usr/bin/mkfifo").arg(&fifo).status().unwrap();
    assert!(made.success());
    assert_refused(&bench, &fifo, &fifo.to_string_lossy(), Refusal::NotRegular, "never");
}

#[test]
fn a_folder_is_broken() {
    let bench = Bench::new();
    let folder = bench.dir("home");
    assert_refused(&bench, &folder, &folder.to_string_lossy(), Refusal::NotRegular, "never");
}

#[test]
fn a_missing_override_is_broken_not_passed_over() {
    let bench = Bench::new();
    let missing = bench.dir("home").join("no-such-claude");
    assert_refused(&bench, &missing, &missing.to_string_lossy(), Refusal::Missing, "never");
}

#[test]
fn a_symlink_in_a_world_writable_folder_is_broken() {
    let bench = Bench::new();
    let target = bench.fake(&bench.dir("home").join("claude-real"), "target", "echo 'claude 2.1.326'", 0o755);
    let open = bench.dir("open");
    std::fs::create_dir(&open).unwrap();
    let link = open.join("claude");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o777)).unwrap();
    assert_refused(&bench, &link, &link.to_string_lossy(), Refusal::ParentWritable, "target");
    std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn neg_a_well_formed_override_is_used_ahead_of_the_path() {
    let bench = Bench::new();
    let own = bench.fake(&bench.dir("home").join("tools/claude"), "override", "echo 'claude 2.1.326'", 0o755);
    let (status, spawned) = with_override(&bench, &own.to_string_lossy());
    match status {
        Status::Detected(found) => {
            assert_eq!((found.version.as_str(), found.source), ("2.1.326", Source::Override));
            assert_eq!(found.path, std::fs::canonicalize(&own).unwrap());
        }
        other => panic!("not detected: {other:?}"),
    }
    assert_eq!(spawned, [std::fs::canonicalize(&own).unwrap()]);
    assert_eq!(bench.ran("override").as_deref(), Some("--version\n"));
    assert_eq!(bench.ran("on-path"), None);
    assert!(check_executable(&own).is_ok());
}
