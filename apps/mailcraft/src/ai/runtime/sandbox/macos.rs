// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! macOS: `sandbox-exec` with a generated Seatbelt profile (library SB-b; B4, task 14.6).
//!
//! The profile allows everything but the file system, then denies every file read and write and
//! allows back reads beneath the policy's read paths and reads and writes beneath its write paths.
//! A file's metadata (`stat`) stays readable everywhere, which programs need to find their way;
//! its contents don't. Network, Mach services (the keychain's among them) and processes are left
//! as they are: the runtime talks to its vendor and signs in with what the CLI keeps itself.
//!
//! `sandbox-exec` is deprecated by Apple and may be removed from a future macOS (a recorded risk);
//! when it is missing there is no sandbox. Pure apart from [`write_profile`] and [`detect`], and
//! built on every platform so its tests run anywhere.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use super::{Guard, Policy, SandboxError, path_text};

/// The program that applies a profile.
pub(crate) const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// The profile's file in the run folder.
pub(crate) const PROFILE_FILE: &str = "sandbox.sb";

/// What a runtime may read of the system to start: the system, its libraries and frameworks, the
/// shells' `/private/var/select` links, the time zones, resolver settings, devices, and the two
/// places packages of programs are installed (Homebrew, MacPorts). None of it is the owner's.
pub(crate) const SYSTEM_READ: &[&str] = &[
    "/System",
    "/Library",
    "/usr",
    "/bin",
    "/sbin",
    "/opt/homebrew",
    "/opt/local",
    "/private/etc",
    "/private/var/select",
    "/private/var/db/timezone",
    "/private/var/db/dyld",
    "/private/var/run/resolv.conf",
    "/dev",
];

/// `s` as a Seatbelt string literal.
fn quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// The Seatbelt profile of `policy`.
pub(crate) fn profile(policy: &Policy) -> Result<String, SandboxError> {
    let mut p = String::from(
        "(version 1)\n\
         ; MailCraft: a runtime may read only the system, its install folder, its run folder and\n\
         ; its recipe's config paths, and write only its run folder and its recipe's paths.\n\
         (allow default)\n\
         (deny file-read* file-write*)\n\
         (allow file-read-metadata)\n\
         (allow file-read* (literal \"/\"))\n",
    );
    if !policy.read.is_empty() {
        p.push_str("(allow file-read*");
        for r in &policy.read {
            p.push_str(&format!(" (subpath {})", quoted(path_text(r)?)));
        }
        p.push_str(")\n");
    }
    if !policy.write.is_empty() {
        p.push_str("(allow file-read* file-write*");
        for w in &policy.write {
            p.push_str(&format!(" (subpath {})", quoted(path_text(w)?)));
        }
        p.push_str(")\n");
    }
    // A program writes to the null device and its own descriptors.
    p.push_str("(allow file-read* file-write* (literal \"/dev/null\") (literal \"/dev/tty\") (regex #\"^/dev/fd/\"))\n");
    // Last, since later rules win: the CLI's executable configuration is never written, even
    // where its folder is writable (it stays readable).
    if !policy.deny_write.is_empty() {
        p.push_str("(deny file-write*");
        for g in &policy.deny_write {
            let filter = match g {
                Guard::Tree(_) => "subpath",
                Guard::File(_) => "literal",
                Guard::Prefix(_) => "prefix",
            };
            p.push_str(&format!(" ({filter} {})", quoted(path_text(g.path())?)));
        }
        p.push_str(")\n");
    }
    Ok(p)
}

/// Writes the profile into the run folder, 0600, and returns its path.
pub(crate) fn write_profile(run_dir: &Path, profile: &str) -> Result<PathBuf, SandboxError> {
    let path = run_dir.join(PROFILE_FILE);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(&path).map_err(|_| SandboxError::Io)?;
    file.write_all(profile.as_bytes()).map_err(|_| SandboxError::Io)?;
    Ok(path)
}

/// Seatbelt when `sandbox-exec` is there.
#[cfg(target_os = "macos")]
pub(crate) fn detect() -> super::Sandbox {
    if Path::new(SANDBOX_EXEC).is_file() { super::Sandbox::Seatbelt } else { super::Sandbox::None }
}

#[cfg(test)]
#[path = "macos_tests.rs"]
mod macos_tests;
