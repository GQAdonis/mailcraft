// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Opening the owner's terminal for a CLI sign-in (B5, task 15.7).
//!
//! MailCraft offers no claude.ai login of its own and never handles the login: to sign in, the
//! owner runs the CLI's own `claude auth login` in a terminal. MailCraft opens that terminal with
//! a **fixed argv** ([`launches`]): no string from settings, from mail or from a runtime ever
//! reaches it, and no shell is used to start it (the terminal's own shell runs the constant
//! command). The terminal gets MailCraft's environment without any variable that carries a key,
//! a token or a vendor's settings. When no terminal can be opened, the owner is shown the command
//! to copy ([`SignInStart::ShowCommand`]).
//!
//! Then the status is asked every 2 s for at most 10 minutes ([`poll_sign_in`]), until it is
//! usable or the owner cancels.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mailcraft_ai::runtime::claude_code::AuthStatus;
use mailcraft_ai::runtime::contain::is_forbidden_env_name;

/// The command the owner runs, as shown to copy.
pub(crate) const SIGN_IN_COMMAND: &str = "claude auth login";
/// The command's argv, as a terminal is given it.
pub(crate) const SIGN_IN_ARGV: [&str; 3] = ["claude", "auth", "login"];
/// How often the status is asked while the owner signs in.
pub(crate) const POLL_EVERY: Duration = Duration::from_secs(2);
/// How long the status is asked for.
pub(crate) const POLL_LIMIT: Duration = Duration::from_secs(10 * 60);
/// How long a launcher (macOS `osascript`) is waited for before it counts as started.
const LAUNCH_WAIT: Duration = Duration::from_secs(10);

/// One way to open a terminal running [`SIGN_IN_ARGV`]: a program and its fixed arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Launch {
    /// The program's name (looked up in absolute PATH folders) or absolute path.
    pub(crate) program: &'static str,
    pub(crate) args: Vec<&'static str>,
    /// Whether its exit status says whether the terminal opened (`osascript`), or it is the
    /// terminal itself and runs on (`xterm`).
    pub(crate) waits: bool,
}

/// The ways this platform opens a terminal, in order. All constants.
pub(crate) fn launches() -> Vec<Launch> {
    if cfg!(target_os = "macos") {
        vec![Launch {
            program: "/usr/bin/osascript",
            args: vec!["-e", "tell application \"Terminal\" to activate", "-e", "tell application \"Terminal\" to do script \"claude auth login\""],
            waits: true,
        }]
    } else if cfg!(windows) {
        vec![Launch { program: "cmd.exe", args: vec!["/c", "start", "", "cmd.exe", "/k", "claude", "auth", "login"], waits: true }]
    } else {
        let after_e = |program: &'static str, flag: &'static str| {
            let mut args = vec![flag];
            args.extend(SIGN_IN_ARGV);
            Launch { program, args, waits: false }
        };
        vec![
            after_e("x-terminal-emulator", "-e"),
            after_e("gnome-terminal", "--"),
            after_e("konsole", "-e"),
            after_e("xfce4-terminal", "-x"),
            after_e("kitty", "--"),
            after_e("alacritty", "-e"),
            after_e("foot", "--"),
            after_e("xterm", "-e"),
        ]
    }
}

/// What the owner sees after asking to sign in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SignInStart {
    /// A terminal was opened with the command.
    Opened,
    /// No terminal could be opened: show [`SIGN_IN_COMMAND`] to copy.
    ShowCommand(&'static str),
}

/// `program` as an absolute path: itself when absolute, else the first absolute PATH folder
/// holding a file of that name.
fn resolve(program: &str, path: &[String]) -> Option<PathBuf> {
    let p = Path::new(program);
    if p.is_absolute() {
        return p.is_file().then(|| p.to_path_buf());
    }
    path.iter().map(Path::new).filter(|d| d.is_absolute()).map(|d| d.join(program)).find(|c| c.is_file())
}

/// Starts `launch` with MailCraft's environment minus every forbidden variable.
fn start(program: &Path, launch: &Launch) -> bool {
    let mut command = Command::new(program);
    command.args(&launch.args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    for (name, _) in std::env::vars_os() {
        if name.to_str().is_none_or(is_forbidden_env_name) {
            command.env_remove(&name);
        }
    }
    let Ok(mut child) = command.spawn() else {
        return false;
    };
    if !launch.waits {
        // The terminal itself: it runs on, and is never waited for; it ends when the owner
        // closes it.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        return true;
    }
    let deadline = deadline_after(Instant::now(), LAUNCH_WAIT);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if deadline.is_some_and(|d| Instant::now() < d) => std::thread::sleep(Duration::from_millis(50)),
            // Still waiting (macOS may be asking the owner to let MailCraft drive Terminal): the
            // owner sees that question, and the poll goes on.
            Ok(None) => {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                return true;
            }
            Err(_) => return false,
        }
    }
}

/// `wait` after `now`, or `None` when the clock can't hold it (then nothing is waited for).
pub(crate) fn deadline_after(now: Instant, wait: Duration) -> Option<Instant> {
    now.checked_add(wait)
}

/// Whether one of `launches` is on this computer (`path`: the login shell's PATH): a cheap look
/// at files, so the window can say "run it yourself" at once instead of after a thread tried.
pub(crate) fn can_open(launches: &[Launch], path: &[String]) -> bool {
    launches.iter().any(|l| resolve(l.program, path).is_some())
}

/// Opens a terminal running the sign-in, or says to show the command.
pub(crate) fn open_sign_in(path: &[String]) -> SignInStart {
    open_sign_in_with(&launches(), path, &mut |program, launch| start(program, launch))
}

/// [`open_sign_in`] with its launcher (tests never open a terminal).
pub(crate) fn open_sign_in_with(launches: &[Launch], path: &[String], launcher: &mut dyn FnMut(&Path, &Launch) -> bool) -> SignInStart {
    for launch in launches {
        if let Some(program) = resolve(launch.program, path)
            && launcher(&program, launch)
        {
            log::info!("ai runtime claude_code: a terminal was opened for the sign-in");
            return SignInStart::Opened;
        }
    }
    log::info!("ai runtime claude_code: no terminal could be opened, showing the command");
    SignInStart::ShowCommand(SIGN_IN_COMMAND)
}

/// The time, for the poll (a fake one in tests).
pub(crate) trait Clock {
    /// Time since some fixed start.
    fn now(&self) -> Duration;
    /// Waits `d`.
    fn sleep(&self, d: Duration);
}

/// The real clock.
pub(crate) struct RealClock(Instant);

impl RealClock {
    pub(crate) fn new() -> RealClock {
        RealClock(Instant::now())
    }
}

impl Clock for RealClock {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }
}

/// How a sign-in poll ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PollEnd {
    /// The status became usable.
    SignedIn(AuthStatus),
    /// The owner cancelled.
    Cancelled,
    /// [`POLL_LIMIT`] passed.
    GaveUp,
}

/// The longest one sleep of the poll, so a cancel is seen quickly.
const CANCEL_SLICE: Duration = Duration::from_millis(100);

/// Asks `probe` every `every` until it is usable, `cancel` is set, or `limit` has passed.
pub(crate) fn poll_sign_in(
    probe: &mut dyn FnMut() -> AuthStatus,
    cancel: &AtomicBool,
    clock: &dyn Clock,
    every: Duration,
    limit: Duration,
) -> PollEnd {
    let start = clock.now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return PollEnd::Cancelled;
        }
        let status = probe();
        if status.is_usable() {
            return PollEnd::SignedIn(status);
        }
        let next = clock.now().saturating_add(every);
        while clock.now() < next {
            if cancel.load(Ordering::Relaxed) {
                return PollEnd::Cancelled;
            }
            if clock.now().saturating_sub(start) >= limit {
                return PollEnd::GaveUp;
            }
            clock.sleep(next.saturating_sub(clock.now()).min(CANCEL_SLICE));
        }
        if clock.now().saturating_sub(start) >= limit {
            return PollEnd::GaveUp;
        }
    }
}

/// [`poll_sign_in`] with the real clock, every [`POLL_EVERY`] for [`POLL_LIMIT`].
pub(crate) fn poll_sign_in_default(probe: &mut dyn FnMut() -> AuthStatus, cancel: &AtomicBool) -> PollEnd {
    poll_sign_in(probe, cancel, &RealClock::new(), POLL_EVERY, POLL_LIMIT)
}
