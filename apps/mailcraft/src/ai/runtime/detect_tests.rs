// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Finding the runtimes (B3): fake `claude`, `codex` and `universal-agent-runtime` scripts the
//! test writes into its own temporary folders, run for real (SCN-RT-DETECT-001, -002). No real
//! runtime, no owner login, no network but loopback fakes.
#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mailcraft_ai::runtime::detect::{Program, Source};

use crate::ai::runtime::detect::{
    Context, Detection, Rate, Report, Status, UAR_CAP, check_executable, choose_shell, detect, expand_percent, path_from_shell_output, probe_uar,
    write_cache,
};
use crate::ai::settings::AiSettings;

/// A folder of the test's own: a home, a PATH folder and a folder for the markers that show a
/// fake ran.
pub(super) struct Bench {
    pub(super) root: tempfile::TempDir,
}

impl Bench {
    pub(super) fn new() -> Bench {
        let root = tempfile::Builder::new().prefix("mailcraft-detect-test-").tempdir().unwrap();
        for dir in ["home", "bin", "marks"] {
            std::fs::create_dir(root.path().join(dir)).unwrap();
        }
        Bench { root }
    }

    pub(super) fn dir(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }

    /// A context with this bench's PATH folder and home, no override and no UAR probe.
    pub(super) fn context(&self) -> Context {
        Context {
            path: vec![self.dir("bin").to_string_lossy().into_owned()],
            home: Some(self.dir("home").to_string_lossy().into_owned()),
            ..Context::default()
        }
    }

    /// The marker a fake called `name` writes its arguments to when it runs.
    pub(super) fn mark(&self, name: &str) -> PathBuf {
        self.dir("marks").join(name)
    }

    /// Writes a fake at `path` (mode `mode`) that notes its arguments in its marker, then runs
    /// `body` (shell, PATH-free).
    pub(super) fn fake(&self, path: &Path, name: &str, body: &str, mode: u32) -> PathBuf {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let script = format!("#!/bin/sh\necho \"$@\" >> '{}'\n{body}\n", self.mark(name).display());
        std::fs::write(path, script).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
        path.to_path_buf()
    }

    /// Whether the fake called `name` ran, and with what.
    pub(super) fn ran(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.mark(name)).ok()
    }
}

fn status(report: &Report, program: Program) -> Status {
    report.status(program).cloned().unwrap()
}

fn detected(status: &Status) -> (String, Source, PathBuf) {
    match status {
        Status::Detected(found) => (found.version.clone(), found.source, found.path.clone()),
        other => panic!("not detected: {other:?}"),
    }
}

fn broken(status: &Status) -> (String, Option<String>) {
    match status {
        Status::Broken(b) => (b.reason.clone(), b.raw.clone()),
        other => panic!("not broken: {other:?}"),
    }
}

#[test]
fn fakes_on_a_temp_path_are_found_with_versions_and_a_group_writable_one_is_refused() {
    let bench = Bench::new();
    let claude = bench.fake(&bench.dir("bin").join("claude"), "claude", "echo '2.1.326 (Claude Code)'", 0o755);
    let codex = bench.fake(&bench.dir("bin").join("codex"), "codex", "echo 'codex-cli 0.46.0'", 0o755);
    let uar_path = bench.dir("home").join(".uar/bin/universal-agent-runtime");
    bench.fake(&uar_path, "uar", "echo 'universal-agent-runtime 1.0.0'", 0o775);

    let report = detect(&bench.context());

    let (version, source, path) = detected(&status(&report, Program::Claude));
    assert_eq!((version.as_str(), source, path.clone()), ("2.1.326", Source::Path, std::fs::canonicalize(&claude).unwrap()));
    let (version, source, _) = detected(&status(&report, Program::Codex));
    assert_eq!((version.as_str(), source), ("0.46.0", Source::Path));
    let (reason, _) = broken(&status(&report, Program::Uar));
    assert_eq!(reason, "other users can change it");
    assert_eq!(bench.ran("uar"), None, "the group-writable fake was run");
    // Only --version is ever run, once each.
    assert_eq!(bench.ran("claude").as_deref(), Some("--version\n"));
    assert_eq!(bench.ran("codex").as_deref(), Some("--version\n"));
    let spawned = [std::fs::canonicalize(&claude).unwrap(), std::fs::canonicalize(&codex).unwrap()];
    assert_eq!(report.spawned, spawned);
}

#[test]
fn a_group_writable_runtime_on_the_path_is_broken_and_never_run() {
    let bench = Bench::new();
    bench.fake(&bench.dir("bin").join("codex"), "codex", "echo 'codex-cli 0.46.0'", 0o775);
    let report = detect(&bench.context());
    assert_eq!(broken(&status(&report, Program::Codex)).0, "other users can change it");
    assert_eq!(bench.ran("codex"), None);
    assert!(report.spawned.is_empty());
}

#[test]
fn a_world_writable_runtime_on_the_path_is_broken_and_a_later_safe_one_is_used() {
    let bench = Bench::new();
    let later = bench.dir("later");
    std::fs::create_dir(&later).unwrap();
    bench.fake(&bench.dir("bin").join("claude"), "bad", "echo 'claude 9.9.9'", 0o757);
    bench.fake(&later.join("claude"), "good", "echo 'claude 2.1.326'", 0o700);
    let mut context = bench.context();
    context.path.push(later.to_string_lossy().into_owned());
    let report = detect(&context);
    assert_eq!(detected(&status(&report, Program::Claude)).0, "2.1.326");
    assert_eq!(bench.ran("bad"), None);
}

#[test]
fn neg_nothing_installed_reports_each_missing_and_spawns_nothing() {
    let bench = Bench::new();
    let report = detect(&bench.context());
    for program in Program::ALL {
        assert_eq!(status(&report, program), Status::Missing, "{program:?}");
    }
    assert!(report.spawned.is_empty());
    assert_eq!(report.detections.iter().find(|d| d.program == Program::Uar).and_then(|d| d.listening), Some(false));
}

#[test]
fn login_path_skips_non_absolute_entries_and_a_greeting() {
    assert_eq!(path_from_shell_output(b"Welcome back!\n~/.dotnet/tools:/opt/homebrew/bin:bin:"), ["/opt/homebrew/bin"]);
    assert_eq!(path_from_shell_output(b"/usr/bin:/bin\n"), ["/usr/bin", "/bin"]);
    assert!(path_from_shell_output(b"").is_empty());
}

#[test]
fn the_login_shell_must_be_absolute_and_listed() {
    let shells = "# list\n/bin/sh\n/bin/zsh\n/opt/homebrew/bin/fish\n";
    let yes = |_: &Path| true;
    assert_eq!(choose_shell(Some("/bin/zsh"), shells, yes), PathBuf::from("/bin/zsh"));
    assert_eq!(choose_shell(Some("/tmp/evil"), shells, yes), PathBuf::from("/bin/sh"));
    assert_eq!(choose_shell(Some("zsh"), shells, yes), PathBuf::from("/bin/sh"));
    assert_eq!(choose_shell(Some("# list"), shells, yes), PathBuf::from("/bin/sh"));
    assert_eq!(choose_shell(Some("/bin/zsh"), shells, |_| false), PathBuf::from("/bin/sh"));
    assert_eq!(choose_shell(None, shells, yes), PathBuf::from("/bin/sh"));
}

#[test]
fn registry_paths_expand_known_variables_only() {
    let lookup = |name: &str| (name == "USERPROFILE").then(|| r"C:\Users\owner".to_string());
    assert_eq!(expand_percent(r"%USERPROFILE%\bin;%NOPE%\x;50%", lookup), r"C:\Users\owner\bin;%NOPE%\x;50%");
    assert_eq!(expand_percent("%%", lookup), "%%");
}

/// A server on 127.0.0.1 that answers one request with `answer`; its address.
fn serve_once(answer: Vec<u8>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut request = [0u8; 2048];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(&answer);
        }
    });
    format!("http://127.0.0.1:{port}/.well-known/agent-card.json")
}

fn http(status: &str, extra: &str, body: &[u8]) -> Vec<u8> {
    let mut out =
        format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n", body.len())
            .into_bytes();
    out.extend_from_slice(body);
    out
}

#[test]
fn uar_answers_with_its_agent_card_on_loopback() {
    let url = serve_once(http("200 OK", "", br#"{"name":"universal-agent-runtime","url":"http://127.0.0.1:1906"}"#));
    assert!(probe_uar(&url));
}

#[test]
fn uar_probe_refuses_a_redirect_junk_and_anything_not_loopback() {
    let redirect = serve_once(http("302 Found", "Location: http://127.0.0.1:1/\r\n", b"{}"));
    assert!(!probe_uar(&redirect));
    let junk = serve_once(http("200 OK", "", b"not json"));
    assert!(!probe_uar(&junk));
    let big = serve_once(http("200 OK", "", &vec![b' '; usize::try_from(UAR_CAP).unwrap() + 10]));
    assert!(!probe_uar(&big));
    let started = Instant::now();
    assert!(!probe_uar("http://192.0.2.1:1906/.well-known/agent-card.json"));
    assert!(!probe_uar("https://example.org/.well-known/agent-card.json"));
    assert!(started.elapsed() < Duration::from_secs(3));
    // Nobody listening: nothing found, quickly.
    let closed = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    assert!(!probe_uar(&format!("http://127.0.0.1:{closed}/.well-known/agent-card.json")));
}

#[test]
fn uar_listening_is_reported_with_the_look() {
    let bench = Bench::new();
    let mut context = bench.context();
    context.uar_card_url = Some(serve_once(http("200 OK", "", b"{\"name\":\"uar\"}")));
    let report = detect(&context);
    let uar: Option<&Detection> = report.detections.iter().find(|d| d.program == Program::Uar);
    assert_eq!(uar.map(|d| (&d.status, d.listening)), Some((&Status::Missing, Some(true))));
}

#[test]
fn looks_run_once_a_minute_unless_the_owner_asks_again() {
    let mut rate = Rate::default();
    let start = Instant::now();
    assert!(rate.allow(start, false));
    assert!(!rate.allow(start + Duration::from_secs(20), false));
    assert!(rate.allow(start + Duration::from_secs(20), true), "ai.rescan runs again");
    assert!(!rate.allow(start + Duration::from_secs(79), false));
    assert!(rate.allow(start + Duration::from_secs(80), false));
}

#[test]
fn the_cache_keeps_found_and_broken_paths_and_drops_missing() {
    let bench = Bench::new();
    bench.fake(&bench.dir("bin").join("claude"), "claude", "echo 'claude 2.1.326'", 0o755);
    bench.fake(&bench.dir("bin").join("codex"), "codex", "echo 'codex 1.0.0'", 0o777);
    let report = detect(&bench.context());
    let mut settings = AiSettings::from_json(br#"{"version": 3, "detected": {"uar": {"path": "/opt/example/uar", "version": "1.0.0"}}}"#);
    write_cache(&mut settings, &report, 1_795_000_000);
    let claude = settings.detected.get("claude_code").cloned().unwrap();
    assert_eq!(claude.version.as_deref(), Some("2.1.326"));
    assert!(claude.path.is_some());
    assert_eq!(claude.checked.as_deref(), Some("2026-11-18T11:06:40Z"));
    let codex = settings.detected.get("codex").cloned().unwrap();
    assert_eq!((codex.path.is_some(), codex.version), (true, None));
    assert!(!settings.detected.contains_key("uar"));
    // It round-trips through ai.json as a cache.
    let again = AiSettings::from_json(&serde_json::to_vec(&settings.to_json()).unwrap());
    assert_eq!(again.detected, settings.detected);
}

#[test]
fn check_executable_returns_the_file_a_safe_link_leads_to() {
    let bench = Bench::new();
    let real = bench.fake(&bench.dir("home").join("versions/claude-2"), "claude", "echo 'claude 2.1.326'", 0o755);
    let link = bench.dir("bin").join("claude");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert_eq!(check_executable(&link), Ok(std::fs::canonicalize(&real).unwrap()));
}

// --- SCN-RT-DETECT-002: hostile --version output -------------------------------------------

/// Detects a lone fake `claude` printing as `body` says, and how long that took.
fn claude_printing(body: &str) -> (Status, Duration, Bench) {
    let bench = Bench::new();
    bench.fake(&bench.dir("bin").join("claude"), "claude", body, 0o755);
    let started = Instant::now();
    let report = detect(&bench.context());
    (status(&report, Program::Claude), started.elapsed(), bench)
}

fn assert_clean(raw: &Option<String>) {
    if let Some(raw) = raw {
        assert!(raw.chars().count() <= 64, "{raw:?}");
        assert!(!raw.chars().any(char::is_control), "{raw:?}");
    }
}

#[test]
fn hostile_ten_megabytes_is_capped_and_broken() {
    let (status, took, _bench) = claude_printing("/usr/bin/yes 'claude 2.1.326' | /usr/bin/head -c 10485760");
    let (reason, raw) = broken(&status);
    assert_eq!(reason, "it printed too much for --version");
    assert_clean(&raw);
    assert!(took < Duration::from_secs(6), "{took:?}");
}

#[test]
fn hostile_control_characters_are_broken_and_removed() {
    let (status, _, _bench) = claude_printing(r"printf 'claude 2.1.326\033[2J\007\r\n\001\002 more\n'");
    let (reason, raw) = broken(&status);
    assert_eq!(reason, "--version printed control characters");
    assert_clean(&raw);
    assert_eq!(raw.as_deref(), Some("claude 2.1.326 [2J more"));
}

#[test]
fn hostile_output_without_a_version_is_broken() {
    let (status, _, _bench) = claude_printing("echo 'Claude Code (latest and greatest)'");
    let (reason, raw) = broken(&status);
    assert_eq!(reason, "--version printed no version");
    assert_clean(&raw);
}

#[test]
fn hostile_hang_is_stopped_within_the_timeout() {
    let (status, took, _bench) = claude_printing("exec /bin/sleep 30");
    assert_eq!(broken(&status).0, "it didn't answer --version in time");
    assert!(took < Duration::from_secs(6), "{took:?}");
}

#[test]
fn hostile_failing_exit_is_broken() {
    let (status, _, _bench) = claude_printing("echo 'claude 2.1.326'; exit 3");
    assert_eq!(broken(&status).0, "--version failed");
}

#[test]
fn hostile_long_line_is_kept_to_64_characters() {
    let long = "x".repeat(3000);
    let (status, _, _bench) = claude_printing(&format!("echo '{long}'"));
    assert_clean(&broken(&status).1);
}

#[test]
fn neg_hostile_well_formed_version_is_detected() {
    let (status, _, bench) = claude_printing("echo 'claude 2.1.326'");
    assert_eq!(detected(&status).0, "2.1.326");
    match status {
        Status::Detected(found) => assert_eq!(found.raw, "claude 2.1.326"),
        other => panic!("{other:?}"),
    }
    assert_eq!(bench.ran("claude").as_deref(), Some("--version\n"));
}
