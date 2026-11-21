// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Seatbelt profile (B4, 14.6).

use std::path::PathBuf;

use super::*;

#[test]
fn profile_denies_files_then_allows_the_policys_paths() {
    let policy = Policy {
        read: vec![PathBuf::from("/usr"), PathBuf::from("/Users/o/.cli")],
        write: vec![PathBuf::from("/private/var/run-1")],
        deny_write: vec![],
    };
    let p = profile(&policy).unwrap();
    let deny = p.find("(deny file-read* file-write*)").unwrap();
    let read = p.find("(allow file-read* (subpath \"/usr\") (subpath \"/Users/o/.cli\"))").unwrap();
    let write = p.find("(allow file-read* file-write* (subpath \"/private/var/run-1\"))").unwrap();
    assert!(deny < read && read < write, "later rules win, so the allows come after the deny");
    assert!(p.starts_with("(version 1)"));
    assert!(!p.contains("(subpath \"/Users/o\")"), "the home itself is never allowed");
}

#[test]
fn profile_quotes_paths() {
    let policy = Policy { read: vec![PathBuf::from("/a \"b\" \\c")], write: vec![], deny_write: vec![] };
    let p = profile(&policy).unwrap();
    assert!(p.contains(r#"(subpath "/a \"b\" \\c")"#), "{p}");
}

#[test]
fn profile_refuses_a_path_with_a_newline() {
    let policy = Policy { read: vec![PathBuf::from("/a\n(allow default)")], write: vec![], deny_write: vec![] };
    assert_eq!(profile(&policy), Err(SandboxError::BadPath));
}

#[test]
fn system_read_paths_hold_nothing_of_the_owners() {
    assert!(!SYSTEM_READ.iter().any(|p| p.starts_with("/Users") || *p == "/private/var" || *p == "/private/var/folders"));
}

#[test]
fn profile_denies_writes_to_guarded_configuration_after_the_allows() {
    let policy = Policy {
        read: vec![PathBuf::from("/Users/o/.claude")],
        write: vec![PathBuf::from("/Users/o/.claude")],
        deny_write: vec![
            Guard::Prefix(PathBuf::from("/Users/o/.claude/settings")),
            Guard::File(PathBuf::from("/Users/o/.claude/CLAUDE.md")),
            Guard::Tree(PathBuf::from("/Users/o/.claude/hooks")),
        ],
    };
    let p = profile(&policy).unwrap();
    let write = p.find("(allow file-read* file-write* (subpath \"/Users/o/.claude\"))").unwrap();
    let devices = p.find("(literal \"/dev/null\")").unwrap();
    let deny = p
        .find(
            "(deny file-write* (prefix \"/Users/o/.claude/settings\") (literal \"/Users/o/.claude/CLAUDE.md\") (subpath \"/Users/o/.claude/hooks\"))",
        )
        .unwrap();
    assert!(write < deny && devices < deny, "later rules win, so the deny comes after every allow");
    assert!(!p[deny..].contains("file-read*"), "the configuration stays readable");
}

#[test]
fn profile_refuses_a_guard_with_a_newline() {
    let policy = Policy { read: vec![], write: vec![], deny_write: vec![Guard::File(PathBuf::from("/a\n(allow default)"))] };
    assert_eq!(profile(&policy), Err(SandboxError::BadPath));
}
