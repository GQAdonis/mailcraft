// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The launcher's arguments.

use super::*;

fn os(v: &[&str]) -> Vec<OsString> {
    v.iter().map(OsString::from).collect()
}

#[test]
fn reads_paths_then_the_program_and_its_args() {
    let c = parse(&os(&["--read", "/usr", "--write", "/tmp/run", "--read", "/etc", "--", "/bin/cat", "--", "-x"])).unwrap();
    assert_eq!(
        c,
        Command::Run(Plan {
            read: vec!["/usr".into(), "/etc".into()],
            write: vec!["/tmp/run".into()],
            program: "/bin/cat".into(),
            args: os(&["--", "-x"]),
        })
    );
}

#[test]
fn probe_alone() {
    assert_eq!(parse(&os(&["--probe"])), Ok(Command::Probe));
    assert_eq!(parse(&os(&["--probe", "--", "/bin/true"])), Err(ArgsError::UnknownOption));
}

#[test]
fn refuses_what_it_cant_trust() {
    assert_eq!(parse(&os(&[])), Err(ArgsError::NoProgram));
    assert_eq!(parse(&os(&["--read", "/usr"])), Err(ArgsError::NoProgram));
    assert_eq!(parse(&os(&["--read", "/usr", "--"])), Err(ArgsError::NoProgram));
    assert_eq!(parse(&os(&["--read"])), Err(ArgsError::MissingPath));
    assert_eq!(parse(&os(&["--read", "usr", "--", "/bin/cat"])), Err(ArgsError::RelativePath));
    assert_eq!(parse(&os(&["--", "cat"])), Err(ArgsError::RelativePath));
    assert_eq!(parse(&os(&["/bin/cat"])), Err(ArgsError::UnknownOption));
    assert_eq!(parse(&os(&["--exec", "/x", "--", "/bin/cat"])), Err(ArgsError::UnknownOption));
}

#[test]
fn caps_paths_and_args() {
    let mut many = Vec::new();
    for _ in 0..=MAX_PATHS {
        many.extend(["--read", "/usr"]);
    }
    many.extend(["--", "/bin/true"]);
    assert_eq!(parse(&os(&many)), Err(ArgsError::TooMany));
    let mut args = vec!["--", "/bin/true"];
    args.extend(std::iter::repeat_n("x", MAX_ARGS + 1));
    assert_eq!(parse(&os(&args)), Err(ArgsError::TooMany));
}

#[test]
fn errors_never_echo_a_path() {
    for e in [ArgsError::NoProgram, ArgsError::MissingPath, ArgsError::RelativePath, ArgsError::UnknownOption, ArgsError::TooMany] {
        assert!(!e.to_string().contains("/home"));
    }
}
