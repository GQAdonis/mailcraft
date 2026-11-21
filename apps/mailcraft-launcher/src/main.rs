// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `mailcraft-launcher`: the sandbox MailCraft puts an AI runtime in on Linux (B4, task 14.7;
//! library SB-a).
//!
//! ```text
//! mailcraft-launcher --read <path>… --write <path>… -- <program> <args…>
//! mailcraft-launcher --probe
//! ```
//!
//! It restricts itself with Landlock (the safe `landlock` crate): reads and execution only beneath
//! the `--read` paths, reads and writes only beneath the `--write` paths, nothing else of the file
//! system; then it replaces itself with `<program>` (`exec`, no shell, no `pre_exec`), which
//! inherits the restriction and the environment MailCraft built. Paths that don't exist are left
//! out. When the kernel enforces nothing it refuses to run the program (exit 125), so a runtime is
//! never run believed sandboxed and not. `--probe` says whether this kernel enforces Landlock
//! (exit 0) or not (exit 1). Its own errors exit 2 (usage) or 126 (the program couldn't be run);
//! otherwise the exit status is the program's.
//!
//! Built on every platform so the workspace builds everywhere; anywhere but Linux it refuses.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod args;
#[cfg(target_os = "linux")]
mod restrict;

use std::process::ExitCode;

use args::{Command, parse};

/// The kernel enforces nothing, or the restriction failed: the program is not run.
const EXIT_UNSANDBOXED: u8 = 125;
/// The program couldn't be run.
#[cfg(target_os = "linux")]
const EXIT_EXEC: u8 = 126;
/// The arguments are wrong.
const EXIT_USAGE: u8 = 2;

fn main() -> ExitCode {
    let raw: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    match parse(&raw) {
        Ok(Command::Probe) => probe(),
        Ok(Command::Run(plan)) => run(&plan),
        Err(e) => {
            eprintln!("mailcraft-launcher: {e}");
            ExitCode::from(EXIT_USAGE)
        }
    }
}

#[cfg(target_os = "linux")]
fn probe() -> ExitCode {
    match restrict::restrict(&[], &[]) {
        Ok(true) => ExitCode::SUCCESS,
        _ => ExitCode::from(1),
    }
}

#[cfg(not(target_os = "linux"))]
fn probe() -> ExitCode {
    ExitCode::from(1)
}

#[cfg(target_os = "linux")]
fn run(plan: &args::Plan) -> ExitCode {
    use std::os::unix::process::CommandExt as _;
    match restrict::restrict(&plan.read, &plan.write) {
        Ok(true) => {}
        Ok(false) => {
            eprintln!("mailcraft-launcher: this kernel enforces no Landlock; not running the program");
            return ExitCode::from(EXIT_UNSANDBOXED);
        }
        Err(e) => {
            eprintln!("mailcraft-launcher: the sandbox couldn't be set up: {e}");
            return ExitCode::from(EXIT_UNSANDBOXED);
        }
    }
    // Only returns when the program couldn't be run.
    let error = std::process::Command::new(&plan.program).args(&plan.args).exec();
    eprintln!("mailcraft-launcher: the program couldn't be run: {}", error.kind());
    ExitCode::from(EXIT_EXEC)
}

#[cfg(not(target_os = "linux"))]
fn run(_plan: &args::Plan) -> ExitCode {
    eprintln!("mailcraft-launcher: Landlock exists only on Linux; not running the program");
    ExitCode::from(EXIT_UNSANDBOXED)
}
