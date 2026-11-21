// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tests of the pure part of finding the runtimes (B3, task 13.1).

use proptest::prelude::*;

use super::*;

#[test]
fn path_keeps_only_absolute_entries() {
    let dirs = split_path("~/.dotnet/tools:/opt/homebrew/bin::.:bin:/usr/bin/../sbin:/usr/bin:/opt/homebrew/bin", Os::Unix);
    assert_eq!(dirs, ["/opt/homebrew/bin", "/usr/bin"]);
}

#[test]
fn path_on_windows_takes_drives_and_drops_quotes() {
    let dirs = split_path(r#"C:\Tools;"D:\Program Files\Node";relative\bin;%USERPROFILE%\bin;\\server\share\bin"#, Os::Windows);
    assert_eq!(dirs, [r"C:\Tools", r"D:\Program Files\Node", r"\\server\share\bin"]);
}

#[test]
fn path_entries_with_control_characters_or_too_long_are_dropped() {
    let long = format!("/{}", "a".repeat(MAX_PATH_ENTRY));
    let text = format!("/ok\n/bin:/fine:{long}");
    assert_eq!(split_path(&text, Os::Unix), ["/fine"]);
}

#[test]
fn path_is_capped() {
    let text: Vec<String> = (0..1000).map(|i| format!("/d{i}")).collect();
    assert_eq!(split_path(&text.join(":"), Os::Unix).len(), MAX_PATH_ENTRIES);
}

#[test]
fn an_override_is_the_only_candidate() {
    let path = vec!["/usr/bin".to_string()];
    let places = Places { override_path: Some("/opt/example/claude"), path: &path, home: Some("/home/owner"), npm_bin: None, system: true };
    assert_eq!(candidates(Program::Claude, &places, Os::Unix), [Candidate { path: "/opt/example/claude".into(), source: Source::Override }]);
}

#[test]
fn claude_walks_path_then_well_known_places() {
    let path = vec!["/usr/bin".to_string(), "/usr/local/bin".to_string()];
    let places = Places { override_path: None, path: &path, home: Some("/home/owner"), npm_bin: None, system: true };
    let found: Vec<(String, Source)> = candidates(Program::Claude, &places, Os::Unix).into_iter().map(|c| (c.path, c.source)).collect();
    assert_eq!(
        found,
        [
            ("/usr/bin/claude".to_string(), Source::Path),
            ("/usr/local/bin/claude".to_string(), Source::Path),
            ("/home/owner/.local/bin/claude".to_string(), Source::WellKnown),
            ("/opt/homebrew/bin/claude".to_string(), Source::WellKnown),
        ]
    );
}

#[test]
fn codex_ends_with_npm_global_bin() {
    let places = Places { override_path: None, path: &[], home: Some("/home/owner"), npm_bin: Some("/home/owner/.npm-global/bin"), system: true };
    let found: Vec<String> = candidates(Program::Codex, &places, Os::Unix).into_iter().map(|c| c.path).collect();
    assert_eq!(found, ["/opt/homebrew/bin/codex", "/usr/local/bin/codex", "/home/owner/.local/bin/codex", "/home/owner/.npm-global/bin/codex"]);
}

#[test]
fn uar_is_only_in_its_own_place() {
    let path = vec!["/usr/bin".to_string()];
    let places = Places { override_path: None, path: &path, home: Some("/home/owner"), npm_bin: None, system: true };
    let found = candidates(Program::Uar, &places, Os::Unix);
    assert_eq!(found, [Candidate { path: "/home/owner/.uar/bin/universal-agent-runtime".into(), source: Source::WellKnown }]);
}

#[test]
fn a_relative_home_gives_no_home_candidates() {
    let places = Places { override_path: None, path: &[], home: Some("~"), npm_bin: Some("npm/bin"), system: true };
    assert!(candidates(Program::Uar, &places, Os::Unix).is_empty());
    assert!(candidates(Program::Codex, &places, Os::Unix).iter().all(|c| c.path.starts_with('/')));
}

#[test]
fn without_system_folders_only_the_owners_are_looked_in() {
    let places = Places { override_path: None, path: &[], home: Some("/home/owner"), npm_bin: None, system: false };
    let found: Vec<String> = candidates(Program::Claude, &places, Os::Unix).into_iter().map(|c| c.path).collect();
    assert_eq!(found, ["/home/owner/.local/bin/claude"]);
}

#[test]
fn windows_candidates_end_in_exe() {
    let path = vec![r"C:\Tools".to_string()];
    let places = Places { override_path: None, path: &path, home: Some(r"C:\Users\owner"), npm_bin: None, system: true };
    let found: Vec<String> = candidates(Program::Claude, &places, Os::Windows).into_iter().map(|c| c.path).collect();
    assert_eq!(found, [r"C:\Tools\claude.exe", r"C:\Users\owner\.local\bin\claude.exe"]);
}

#[test]
fn versions_are_found_in_usual_outputs() {
    assert_eq!(parse_version(b"2.1.326 (Claude Code)\n").semver.as_deref(), Some("2.1.326"));
    assert_eq!(parse_version(b"claude 2.1.326").semver.as_deref(), Some("2.1.326"));
    assert_eq!(parse_version(b"codex-cli 0.46.0\n").semver.as_deref(), Some("0.46.0"));
    assert_eq!(parse_version(b"universal-agent-runtime v1.4.0-beta.2+abc").semver.as_deref(), Some("1.4.0-beta.2+abc"));
    assert_eq!(parse_version(b"version 3.0.1.").semver.as_deref(), Some("3.0.1"));
}

#[test]
fn text_without_a_version_has_none() {
    for text in [&b"no version here"[..], b"1.2", b"1.2.3.4", b"v.1.2", b"1.2.x", b"1.2.3-", b"", b"\xff\xfe"] {
        assert_eq!(parse_version(text).semver, None, "{:?}", String::from_utf8_lossy(text));
    }
}

#[test]
fn raw_text_is_capped_and_clean() {
    let version = parse_version(b"claude\x1b[31m 2.1.326\x07\r\n\tmore text that goes on and on and on and on and on and on and on");
    assert!(version.raw.chars().count() <= MAX_RAW);
    assert!(!version.raw.chars().any(char::is_control));
    assert_eq!(version.semver.as_deref(), Some("2.1.326"));
}

#[test]
fn source_names_are_stable() {
    assert_eq!([Source::Override.name(), Source::Path.name(), Source::WellKnown.name()], ["override", "path", "wellknown"]);
    assert_eq!(Program::ALL.map(Program::kind_name), ["claude_code", "codex", "uar"]);
}

proptest! {
    #[test]
    fn any_bytes_parse_within_bounds(bytes in proptest::collection::vec(any::<u8>(), 0..4096)) {
        let version = parse_version(&bytes);
        prop_assert!(version.raw.chars().count() <= MAX_RAW);
        prop_assert!(!version.raw.chars().any(char::is_control));
        if let Some(v) = version.semver {
            prop_assert!(v.chars().count() <= 64 && !v.chars().any(char::is_control));
        }
    }

    #[test]
    fn any_path_text_splits_into_absolute_entries(text in ".{0,512}") {
        for os in [Os::Unix, Os::Windows] {
            let dirs = split_path(&text, os);
            prop_assert!(dirs.len() <= MAX_PATH_ENTRIES);
            prop_assert!(dirs.iter().all(|d| is_absolute(d, os) && !d.chars().any(char::is_control)));
        }
    }
}
