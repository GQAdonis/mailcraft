// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Sandboxes per platform (B4, tasks 14.6–14.9).
//!
//! Every runtime runs under the operating system's sandbox when there is one, applied by argv
//! and never through `pre_exec`:
//! - macOS: `/usr/bin/sandbox-exec -f <profile>` with a Seatbelt profile written into the run's
//!   folder ([`macos`]);
//! - Linux: `mailcraft-launcher`, which restricts itself with Landlock and then executes the
//!   runtime, when the kernel has Landlock; else `bwrap` by argv; else none ([`linux`]);
//! - Windows and the BSDs: none (so a runtime whose recipe needs a sandbox, Codex, is not run).
//!
//! Reads are allowed only beneath the system's own paths, the runtime's install folder, the run
//! folder and the config paths its recipe names; writes only beneath the run folder and the
//! paths the recipe names. Network is allowed (the runtime talks to its vendor).

pub(crate) mod linux;
pub(crate) mod macos;

use std::path::{Path, PathBuf};

/// The sandbox a run goes under.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Sandbox {
    /// macOS `sandbox-exec` with a generated profile.
    Seatbelt,
    /// Linux: the Landlock launcher next to the app.
    Landlock { launcher: PathBuf },
    /// Linux without Landlock: bubblewrap, run by argv (never linked).
    Bwrap { bwrap: PathBuf },
    /// No sandbox on this computer.
    None,
}

impl Sandbox {
    /// The sandbox's kind, as logged and shown in diagnostics.
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Sandbox::Seatbelt => "seatbelt",
            Sandbox::Landlock { .. } => "landlock",
            Sandbox::Bwrap { .. } => "bwrap",
            Sandbox::None => "none",
        }
    }
}

/// Why a run couldn't be put in its sandbox.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SandboxError {
    /// A path isn't absolute, isn't text, or holds a character a profile or argv can't carry.
    BadPath,
    /// The profile couldn't be written.
    Io,
}

impl SandboxError {
    pub(crate) fn kind(self) -> &'static str {
        match self {
            SandboxError::BadPath => "sandbox_bad_path",
            SandboxError::Io => "sandbox_io",
        }
    }
}

/// A place a run may never write, even beneath a write path: a CLI's own executable
/// configuration (its settings, hooks, plugins, agents, skills and commands), which a run that
/// could write would turn into code the CLI runs next time, outside any sandbox.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Guard {
    /// The folder and everything beneath it.
    Tree(PathBuf),
    /// The one file.
    File(PathBuf),
    /// Every path that starts with this text (`~/.claude/settings` for `settings*.json`, and the
    /// temporary files a rewrite of them goes through).
    Prefix(PathBuf),
}

impl Guard {
    /// The guarded path (for a prefix, the path it starts with).
    pub(crate) fn path(&self) -> &Path {
        match self {
            Guard::Tree(p) | Guard::File(p) | Guard::Prefix(p) => p,
        }
    }

    /// The guard with its path made real (links followed where they exist): for a prefix, its
    /// folder is made real and the start of the name kept.
    fn real(&self) -> Result<Guard, SandboxError> {
        Ok(match self {
            Guard::Tree(p) => Guard::Tree(real(p)?),
            Guard::File(p) => Guard::File(real(p)?),
            Guard::Prefix(p) => {
                let (Some(dir), Some(name)) = (p.parent(), p.file_name()) else { return Err(SandboxError::BadPath) };
                Guard::Prefix(real(dir)?.join(name))
            }
        })
    }
}

/// What a run may read and write.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Policy {
    /// Beneath these, reads (and execution) are allowed.
    pub(crate) read: Vec<PathBuf>,
    /// Beneath these, reads and writes are allowed.
    pub(crate) write: Vec<PathBuf>,
    /// Never written, even beneath a write path (Seatbelt denies them after the allows; bubblewrap
    /// binds those that exist read-only over the writable tree; Landlock can't take a write back
    /// beneath a tree it granted: see [`linux::launcher_args`]).
    pub(crate) deny_write: Vec<Guard>,
}

impl Policy {
    /// The policy of one run: the system's paths, the program's install folder (its own folder and,
    /// when a link led to it, the link's), the recipe's read paths; the run folder and the recipe's
    /// write paths. Paths are made real (links followed) where they exist, since a sandbox matches
    /// real paths; duplicates are dropped.
    pub(crate) fn for_run(
        system: &[PathBuf],
        program: &Path,
        real_program: &Path,
        recipe_read: &[PathBuf],
        run_dir: &Path,
        recipe_write: &[PathBuf],
        recipe_deny_write: &[Guard],
    ) -> Result<Policy, SandboxError> {
        let mut read = Vec::new();
        for p in system.iter().chain(recipe_read) {
            push_unique(&mut read, real(p)?);
        }
        for p in [program, real_program] {
            if let Some(dir) = p.parent() {
                push_unique(&mut read, real(dir)?);
            }
        }
        let mut write = Vec::new();
        push_unique(&mut write, real(run_dir)?);
        for p in recipe_write {
            push_unique(&mut write, real(p)?);
        }
        let mut deny_write = Vec::new();
        for g in recipe_deny_write {
            let g = g.real()?;
            if !deny_write.contains(&g) {
                deny_write.push(g);
            }
        }
        Ok(Policy { read, write, deny_write })
    }
}

fn push_unique(list: &mut Vec<PathBuf>, p: PathBuf) {
    if !list.contains(&p) {
        list.push(p);
    }
}

/// `p` with its links followed when it exists; refused when it isn't absolute or isn't text.
/// A path under `/proc` is kept as it is: `/proc/self` must name the runtime's own process when
/// the sandbox opens it, not MailCraft's, which following the link here would.
pub(crate) fn real(p: &Path) -> Result<PathBuf, SandboxError> {
    if !p.is_absolute() || p.to_str().is_none() {
        return Err(SandboxError::BadPath);
    }
    if p.starts_with("/proc") {
        return Ok(p.to_path_buf());
    }
    let r = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    if r.to_str().is_none() {
        return Err(SandboxError::BadPath);
    }
    Ok(r)
}

/// A path as argv or a profile carries it: text without control characters.
pub(crate) fn path_text(p: &Path) -> Result<&str, SandboxError> {
    let s = p.to_str().ok_or(SandboxError::BadPath)?;
    if s.chars().any(char::is_control) || !p.is_absolute() {
        return Err(SandboxError::BadPath);
    }
    Ok(s)
}

/// The system paths a runtime may read on this platform.
pub(crate) fn system_read_paths() -> Vec<PathBuf> {
    if cfg!(target_os = "macos") {
        macos::SYSTEM_READ.iter().map(PathBuf::from).collect()
    } else if cfg!(target_os = "linux") {
        linux::SYSTEM_READ.iter().map(PathBuf::from).collect()
    } else {
        Vec::new()
    }
}

/// The system paths a runtime may write on this platform (devices such as `/dev/null`).
pub(crate) fn system_write_paths() -> Vec<PathBuf> {
    if cfg!(any(target_os = "macos", target_os = "linux")) { ["/dev/null", "/dev/tty"].iter().map(PathBuf::from).collect() } else { Vec::new() }
}

/// The sandbox this computer offers. Looked at for every run (cheap on macOS; a probe of the
/// launcher on Linux).
pub(crate) fn detect() -> Sandbox {
    #[cfg(target_os = "macos")]
    {
        macos::detect()
    }
    #[cfg(target_os = "linux")]
    {
        linux::detect()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        Sandbox::None
    }
}

/// The program and arguments that run `program args` under `sandbox`. `run_dir` is the run's
/// 0700 folder (the Seatbelt profile is written there, 0600); `work` its empty working folder.
pub(crate) fn wrap(
    sandbox: &Sandbox,
    policy: &Policy,
    run_dir: &Path,
    work: &Path,
    program: &Path,
    args: Vec<String>,
) -> Result<(PathBuf, Vec<String>), SandboxError> {
    let program_text = path_text(program)?.to_owned();
    match sandbox {
        Sandbox::None => Ok((program.to_path_buf(), args)),
        Sandbox::Seatbelt => {
            let profile = macos::profile(policy)?;
            let file = macos::write_profile(run_dir, &profile)?;
            let mut out = vec!["-f".to_owned(), path_text(&file)?.to_owned(), program_text];
            out.extend(args);
            Ok((PathBuf::from(macos::SANDBOX_EXEC), out))
        }
        Sandbox::Landlock { launcher } => {
            let out = linux::launcher_args(policy, &program_text, args)?;
            Ok((launcher.clone(), out))
        }
        Sandbox::Bwrap { bwrap } => {
            let out = linux::bwrap_args(policy, run_dir, work, &program_text, args)?;
            Ok((bwrap.clone(), out))
        }
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod mod_tests;
