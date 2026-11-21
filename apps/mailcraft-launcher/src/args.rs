// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The launcher's arguments. Every path is absolute; the program comes after `--`.

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

/// The most paths the launcher takes.
pub(crate) const MAX_PATHS: usize = 512;
/// The most arguments it passes on.
pub(crate) const MAX_ARGS: usize = 256;

/// What to do.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Command {
    /// Say whether Landlock is enforced here.
    Probe,
    /// Restrict, then run.
    Run(Plan),
}

/// A program and the paths it may use.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Plan {
    pub(crate) read: Vec<PathBuf>,
    pub(crate) write: Vec<PathBuf>,
    pub(crate) program: PathBuf,
    pub(crate) args: Vec<OsString>,
}

/// Why the arguments were refused (no path is ever echoed).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArgsError {
    NoProgram,
    MissingPath,
    RelativePath,
    UnknownOption,
    TooMany,
}

impl std::fmt::Display for ArgsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ArgsError::NoProgram => "no program after --",
            ArgsError::MissingPath => "--read or --write without a path",
            ArgsError::RelativePath => "every path must be absolute",
            ArgsError::UnknownOption => "unknown option (usage: --read <path>… --write <path>… -- <program> <args…>, or --probe)",
            ArgsError::TooMany => "too many paths or arguments",
        })
    }
}

fn absolute(p: &OsStr) -> Result<PathBuf, ArgsError> {
    let p = PathBuf::from(p);
    if p.is_absolute() { Ok(p) } else { Err(ArgsError::RelativePath) }
}

/// Reads the arguments (without the launcher's own name).
pub(crate) fn parse(raw: &[OsString]) -> Result<Command, ArgsError> {
    if raw.len() == 1 && raw.first().is_some_and(|a| a == "--probe") {
        return Ok(Command::Probe);
    }
    let mut plan = Plan::default();
    let mut it = raw.iter();
    loop {
        let Some(a) = it.next() else {
            return Err(ArgsError::NoProgram);
        };
        let list = if a == "--read" {
            &mut plan.read
        } else if a == "--write" {
            &mut plan.write
        } else if a == "--" {
            break;
        } else {
            return Err(ArgsError::UnknownOption);
        };
        let path = it.next().ok_or(ArgsError::MissingPath)?;
        list.push(absolute(path)?);
        if plan.read.len().saturating_add(plan.write.len()) > MAX_PATHS {
            return Err(ArgsError::TooMany);
        }
    }
    plan.program = absolute(it.next().ok_or(ArgsError::NoProgram)?)?;
    plan.args = it.cloned().collect();
    if plan.args.len() > MAX_ARGS {
        return Err(ArgsError::TooMany);
    }
    Ok(Command::Run(plan))
}

#[cfg(test)]
#[path = "args_tests.rs"]
mod args_tests;
