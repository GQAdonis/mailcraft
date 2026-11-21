// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Linux: the Landlock launcher when the kernel has Landlock (library SB-a), else `bwrap` by argv
//! (library BW-a), else none (B4, task 14.8).
//!
//! `mailcraft-launcher --read <p>… --write <p>… -- <exe> <args…>` restricts itself with the safe
//! `landlock` crate and then executes the runtime (`apps/mailcraft-launcher`). Bubblewrap is LGPL
//! and is only ever run as a program, never linked. The argv builders and the choice are pure and
//! built on every platform, so their tests run anywhere; [`detect`] looks at this computer.

#![cfg_attr(not(target_os = "linux"), allow(dead_code))] // Built everywhere so its tests run anywhere.

use std::path::{Path, PathBuf};

use super::{Guard, Policy, Sandbox, SandboxError, path_text};

/// The launcher's file name, next to the app's own program.
pub(crate) const LAUNCHER_NAME: &str = "mailcraft-launcher";

/// The launcher's answer to `--probe` when Landlock is enforced on this kernel.
pub(crate) const PROBE_ARG: &str = "--probe";

/// Where bubblewrap is looked for (never the process's PATH, which anybody's environment sets).
pub(crate) const BWRAP_PATHS: &[&str] = &["/usr/bin/bwrap", "/bin/bwrap", "/usr/local/bin/bwrap"];

/// What a runtime may read of the system to start. Of `/proc` only the runtime's own process
/// (`/proc/self`, `/proc/thread-self`, opened by the launcher, whose process the runtime becomes):
/// never the other processes' command lines, environments or memory maps.
pub(crate) const SYSTEM_READ: &[&str] = &[
    "/usr",
    "/bin",
    "/sbin",
    "/lib",
    "/lib32",
    "/lib64",
    "/libx32",
    "/etc",
    "/opt",
    "/proc/self",
    "/proc/thread-self",
    "/sys",
    "/dev",
    "/run/systemd/resolve",
    "/nix/store",
    "/snap",
];

/// The sandbox to use, from what this computer has.
pub(crate) fn choose(landlock: bool, launcher: Option<PathBuf>, bwrap: Option<PathBuf>) -> Sandbox {
    match (landlock, launcher, bwrap) {
        (true, Some(launcher), _) => Sandbox::Landlock { launcher },
        (_, _, Some(bwrap)) => Sandbox::Bwrap { bwrap },
        _ => Sandbox::None,
    }
}

/// The launcher's arguments for `program args` under `policy`.
///
/// What remains (a known limit, not an oversight): Landlock only ever grants, so a write it
/// allows beneath a folder can't be taken back for a file inside it, and `policy.deny_write`
/// can't be carried here. Granting only the subfolders the CLIs write instead of their whole
/// folder isn't possible either without breaking the owner's sign-in: Claude Code keeps it in
/// `~/.claude/.credentials.json` and Codex in `CODEX_HOME/auth.json`, and a refreshed (rotated)
/// sign-in must be written there, which may go through a temporary file beside it and a rename,
/// so the folder itself must be writable; the names of what else they keep there (Codex's state
/// databases and their journals) aren't fixed. So under Landlock `~/.claude` and the Codex home
/// stay writable as a whole. The runs it protects have their tools removed (Claude Code's
/// `--tools ""`; Codex's shell tool off under its own read-only sandbox), so nothing in a run
/// writes files of its own accord; bubblewrap
/// and Seatbelt also deny the configuration ([`bwrap_args`], `macos::profile`).
pub(crate) fn launcher_args(policy: &Policy, program: &str, args: Vec<String>) -> Result<Vec<String>, SandboxError> {
    let mut out = Vec::with_capacity(policy.read.len() * 2 + policy.write.len() * 2 + 2 + args.len());
    for r in &policy.read {
        out.push("--read".to_owned());
        out.push(path_text(r)?.to_owned());
    }
    for w in &policy.write {
        out.push("--write".to_owned());
        out.push(path_text(w)?.to_owned());
    }
    out.push("--".to_owned());
    out.push(program.to_owned());
    out.extend(args);
    Ok(out)
}

fn depth(p: &Path) -> usize {
    p.components().count()
}

/// Bubblewrap's arguments: an empty root with the read paths bound read-only and the write paths
/// read-write (parents before children), a fresh `/proc`, `/dev` and `/tmp`, new namespaces but
/// the network's, its own session, and death with MailCraft.
pub(crate) fn bwrap_args(policy: &Policy, run_dir: &Path, work: &Path, program: &str, args: Vec<String>) -> Result<Vec<String>, SandboxError> {
    let mut out: Vec<String> = [
        "--die-with-parent",
        "--new-session",
        "--unshare-user-try",
        "--unshare-ipc",
        "--unshare-pid",
        "--unshare-uts",
        "--unshare-cgroup-try",
        "--proc",
        "/proc",
        "--dev",
        "/dev",
        "--tmpfs",
        "/tmp",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    let mut binds: Vec<(&Path, bool)> = policy
        .read
        .iter()
        .filter(|p| !p.starts_with("/proc") && !matches!(p.to_str(), Some("/dev")))
        .map(|p| (p.as_path(), false))
        .chain(policy.write.iter().filter(|p| !p.starts_with("/dev")).map(|p| (p.as_path(), true)))
        .collect();
    binds.sort_by_key(|(p, _)| depth(p));
    for (p, writable) in binds {
        let s = path_text(p)?;
        out.push(if writable { "--bind-try" } else { "--ro-bind-try" }.to_owned());
        out.push(s.to_owned());
        out.push(s.to_owned());
    }
    // Then, over the writable trees, the CLI's executable configuration bound read-only where it
    // exists (a later bind covers an earlier one). A prefix can't be bound (its names aren't
    // known without reading the folder, which MailCraft never does): the recipe lists its files.
    for g in &policy.deny_write {
        if let Guard::Tree(p) | Guard::File(p) = g {
            let s = path_text(p)?;
            out.push("--ro-bind-try".to_owned());
            out.push(s.to_owned());
            out.push(s.to_owned());
        }
    }
    let _ = path_text(run_dir)?;
    out.push("--chdir".to_owned());
    out.push(path_text(work)?.to_owned());
    out.push("--".to_owned());
    out.push(program.to_owned());
    out.extend(args);
    Ok(out)
}

/// The launcher next to the app's own program, when it is there.
pub(crate) fn launcher_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let p = exe.parent()?.join(LAUNCHER_NAME);
    p.is_file().then_some(p)
}

/// The first bubblewrap found.
pub(crate) fn bwrap_path() -> Option<PathBuf> {
    BWRAP_PATHS.iter().map(PathBuf::from).find(|p| p.is_file())
}

/// Whether the launcher says Landlock is enforced on this kernel (`--probe`, exit 0), within two
/// seconds. Its output is not read.
#[cfg(target_os = "linux")]
pub(crate) fn landlock_probe(launcher: &Path) -> bool {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let Ok(mut child) = Command::new(launcher).arg(PROBE_ARG).env_clear().stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
    else {
        return false;
    };
    let deadline = Instant::now().checked_add(Duration::from_secs(2));
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if deadline.is_some_and(|d| Instant::now() < d) => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// The sandbox this Linux offers.
#[cfg(target_os = "linux")]
pub(crate) fn detect() -> Sandbox {
    let launcher = launcher_path();
    let landlock = launcher.as_deref().is_some_and(landlock_probe);
    choose(landlock, launcher, bwrap_path())
}

#[cfg(test)]
#[path = "linux_tests.rs"]
mod linux_tests;
