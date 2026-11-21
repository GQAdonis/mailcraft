// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Finding the runtimes: PATH splitting, candidates and version parsing (B3, task 13.1).
//!
//! Pure and the same everywhere (wasm included): nothing here reads a file, runs a program or
//! opens a socket. The host (`apps/mailcraft/src/ai/runtime/detect.rs`) reads the login shell's
//! PATH, walks these candidates, checks each file and runs `--version`; what comes back is
//! hostile, and is read here within bounds.
//!
//! - **PATH** ([`split_path`]): only absolute entries are kept. A literal `~/.dotnet/tools`, an
//!   empty entry or `.` would make a lookup depend on a directory nobody chose, and is skipped.
//! - **Candidates** ([`candidates`]): the owner's override alone when there is one; else the PATH
//!   walked here (never `sh -c`), then the well-known places of each program.
//! - **Versions** ([`parse_version`]): the first token that looks like a semantic version; the
//!   raw text kept to [`MAX_RAW`] characters with control characters removed.

/// The most characters of a program's `--version` text kept.
pub const MAX_RAW: usize = 64;
/// The most PATH entries taken.
pub const MAX_PATH_ENTRIES: usize = 256;
/// The longest PATH entry taken, in bytes.
pub const MAX_PATH_ENTRY: usize = 4096;
/// The longest version token taken, in characters.
const MAX_VERSION: usize = 64;

/// A runtime that is a program on this computer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Program {
    /// Claude Code: `claude`.
    Claude,
    /// Codex: `codex`.
    Codex,
    /// The Universal Agent Runtime: `~/.uar/bin/universal-agent-runtime`.
    Uar,
}

impl Program {
    /// Every program, in the order the spec names them.
    pub const ALL: [Program; 3] = [Program::Claude, Program::Codex, Program::Uar];

    /// The runtime's kind as `ai.json` names it (`claude_code`, `codex`, `uar`).
    pub fn kind_name(self) -> &'static str {
        match self {
            Program::Claude => "claude_code",
            Program::Codex => "codex",
            Program::Uar => "uar",
        }
    }

    /// The program's file name, without an extension.
    pub fn file_name(self) -> &'static str {
        match self {
            Program::Claude => "claude",
            Program::Codex => "codex",
            Program::Uar => "universal-agent-runtime",
        }
    }

    /// Whether the program is looked for on the PATH (UAR is only in its own place).
    pub fn on_path(self) -> bool {
        !matches!(self, Program::Uar)
    }
}

/// Where a candidate came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    /// The owner's path in the chain entry.
    Override,
    /// The login shell's PATH.
    Path,
    /// A well-known place.
    WellKnown,
}

impl Source {
    /// Its name: `override`, `path` or `wellknown`.
    pub fn name(self) -> &'static str {
        match self {
            Source::Override => "override",
            Source::Path => "path",
            Source::WellKnown => "wellknown",
        }
    }
}

/// A file to look at, and where its name came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// An absolute path.
    pub path: String,
    pub source: Source,
}

/// The system the candidates are for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    /// macOS, Linux, the BSDs: `/` paths, `:` between PATH entries.
    Unix,
    /// Windows: `C:\` paths, `;` between PATH entries, programs end in `.exe`.
    Windows,
}

impl Os {
    /// The system this was built for.
    pub fn this() -> Os {
        if cfg!(windows) { Os::Windows } else { Os::Unix }
    }

    fn separator(self) -> char {
        match self {
            Os::Unix => ':',
            Os::Windows => ';',
        }
    }

    fn slash(self) -> char {
        match self {
            Os::Unix => '/',
            Os::Windows => '\\',
        }
    }
}

/// What the host knows of the owner's computer for [`candidates`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Places<'a> {
    /// The owner's path to the program (`path` in the chain entry), when set.
    pub override_path: Option<&'a str>,
    /// The login shell's PATH, already split ([`split_path`]).
    pub path: &'a [String],
    /// The owner's home (`$HOME`, `%USERPROFILE%`), when absolute.
    pub home: Option<&'a str>,
    /// npm's global `bin`, when known.
    pub npm_bin: Option<&'a str>,
    /// Whether the system's own folders (`/opt/homebrew/bin`, `/usr/local/bin`) are looked in:
    /// always on the owner's computer; off where a test keeps to folders of its own.
    pub system: bool,
}

/// Whether `entry` is an absolute path on `os`: `/…` everywhere, and `C:\…` or `C:/…` (or
/// `\\server\…`) on Windows.
pub fn is_absolute(entry: &str, os: Os) -> bool {
    let bytes = entry.as_bytes();
    match os {
        Os::Unix => bytes.first() == Some(&b'/'),
        Os::Windows => {
            let drive = matches!((bytes.first(), bytes.get(1), bytes.get(2)), (Some(l), Some(b':'), Some(b'\\' | b'/')) if l.is_ascii_alphabetic());
            drive || entry.starts_with("\\\\")
        }
    }
}

/// The entries of the PATH `text`, for `os`: only absolute ones, without control characters or a
/// `..` part, each at most [`MAX_PATH_ENTRY`] bytes, without repeats, at most
/// [`MAX_PATH_ENTRIES`] of them. On Windows an entry in quotes loses them.
pub fn split_path(text: &str, os: Os) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in text.split(os.separator()) {
        if out.len() >= MAX_PATH_ENTRIES {
            break;
        }
        let entry = match os {
            Os::Windows => raw.trim().trim_matches('"'),
            Os::Unix => raw,
        };
        let ok = !entry.is_empty()
            && entry.len() <= MAX_PATH_ENTRY
            && !entry.chars().any(char::is_control)
            && is_absolute(entry, os)
            && !entry.split(['/', '\\']).any(|part| part == "..");
        if ok && !out.iter().any(|seen| seen == entry) {
            out.push(entry.to_string());
        }
    }
    out
}

/// `dir` joined with `name` on `os`.
fn join(dir: &str, name: &str, os: Os) -> String {
    let dir = dir.trim_end_matches(['/', '\\']);
    format!("{dir}{}{name}", os.slash())
}

/// The program's file name on `os`.
fn exe(program: Program, os: Os) -> String {
    match os {
        Os::Unix => program.file_name().to_string(),
        Os::Windows => format!("{}.exe", program.file_name()),
    }
}

/// The well-known directories of `program`, in order, for the places known.
fn well_known_dirs(program: Program, places: &Places<'_>, os: Os) -> Vec<String> {
    let home = places.home.filter(|h| is_absolute(h, os));
    let in_home = |rest: &[&str]| home.map(|h| rest.iter().fold(h.to_string(), |dir, part| join(&dir, part, os)));
    let fixed = |dirs: &[&str]| -> Vec<Option<String>> {
        if os == Os::Unix && places.system { dirs.iter().map(|d| Some((*d).to_string())).collect() } else { Vec::new() }
    };
    let dirs: Vec<Option<String>> = match program {
        Program::Claude => {
            let mut all = vec![in_home(&[".local", "bin"])];
            all.extend(fixed(&["/opt/homebrew/bin", "/usr/local/bin"]));
            all
        }
        Program::Codex => {
            let mut all = fixed(&["/opt/homebrew/bin", "/usr/local/bin"]);
            all.push(in_home(&[".local", "bin"]));
            all.push(places.npm_bin.filter(|d| is_absolute(d, os)).map(str::to_string));
            all
        }
        Program::Uar => vec![in_home(&[".uar", "bin"])],
    };
    dirs.into_iter().flatten().collect()
}

/// The files to look at for `program`, in order: the owner's override alone when there is one
/// (a bad override is never passed over for another file); else each PATH entry (for Claude Code
/// and Codex), then the well-known places. Every candidate is an absolute path, and none is
/// named twice.
pub fn candidates(program: Program, places: &Places<'_>, os: Os) -> Vec<Candidate> {
    if let Some(path) = places.override_path {
        return vec![Candidate { path: path.to_string(), source: Source::Override }];
    }
    let name = exe(program, os);
    let mut out: Vec<Candidate> = Vec::new();
    let mut push = |path: String, source: Source| {
        if is_absolute(&path, os) && !out.iter().any(|c| c.path == path) {
            out.push(Candidate { path, source });
        }
    };
    if program.on_path() {
        for dir in places.path.iter().filter(|d| is_absolute(d, os)) {
            push(join(dir, &name, os), Source::Path);
        }
    }
    for dir in well_known_dirs(program, places, os) {
        push(join(&dir, &name, os), Source::WellKnown);
    }
    out
}

/// What a program's `--version` printed, read as hostile text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    /// The first semantic-version token (`2.1.326`, `0.46.0-alpha.3`), without a leading `v`.
    pub semver: Option<String>,
    /// The text, control characters made spaces, runs of spaces made one, trimmed, at most
    /// [`MAX_RAW`] characters.
    pub raw: String,
}

/// The `--version` output `bytes`, read within bounds: whatever the bytes are, the raw text is
/// at most [`MAX_RAW`] characters with no control character, and the version is the first token
/// that looks like `MAJOR.MINOR.PATCH` (optionally `-pre` and `+build`).
pub fn parse_version(bytes: &[u8]) -> Version {
    let text = String::from_utf8_lossy(bytes);
    let cleaned: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let raw: String = cleaned.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(MAX_RAW).collect();
    let semver = cleaned.split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | ',' | ';' | '[' | ']' | '"' | '\'')).find_map(semver_of);
    Version { semver, raw }
}

/// `token` as a semantic version, when it is one (a leading `v` or `V` dropped).
pub fn semver_of(token: &str) -> Option<String> {
    let token = token.trim_end_matches(['.', ':']);
    let token = token.strip_prefix(['v', 'V']).unwrap_or(token);
    if token.is_empty() || token.chars().count() > MAX_VERSION {
        return None;
    }
    let (core, rest) = match token.find(['-', '+']) {
        Some(at) => (token.get(..at)?, token.get(at..)?),
        None => (token, ""),
    };
    let parts: Vec<&str> = core.split('.').collect();
    let numeric = |p: &&str| !p.is_empty() && p.len() <= 9 && p.bytes().all(|b| b.is_ascii_digit());
    if parts.len() != 3 || !parts.iter().all(numeric) {
        return None;
    }
    let suffix_ok = rest.chars().skip(1).all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
        && (rest.is_empty() || rest.chars().count() > 1)
        && !rest.ends_with(['.', '-', '+']);
    suffix_ok.then(|| token.to_string())
}

#[cfg(test)]
#[path = "detect_tests.rs"]
mod detect_tests;
