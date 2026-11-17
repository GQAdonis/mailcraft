// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Streamed requests (#ai) against servers on this machine: the answer arrives in pieces, the
//! caller's deadlines and flag stop it, redirects are not followed, header values lose their
//! control characters, and plain HTTP goes to loopback addresses alone (SCN-LOOPBACK-001).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::*;
use crate::http;

/// A server on 127.0.0.1 that runs `script` on each connection it takes (at most `connections`),
/// handing it what the request was. Counts the connections it took.
struct Script {
    port: u16,
    taken: Arc<AtomicUsize>,
    seen: Arc<Mutex<Vec<String>>>,
}

impl Script {
    fn start(connections: usize, script: impl Fn(&mut TcpStream, &str) + Send + Sync + 'static) -> Script {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (taken, seen) = (Arc::new(AtomicUsize::new(0)), Arc::new(Mutex::new(Vec::new())));
        let (count, kept) = (taken.clone(), seen.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming().take(connections) {
                let Ok(mut stream) = stream else { continue };
                count.fetch_add(1, Ordering::SeqCst);
                let request = read_request(&mut stream);
                kept.lock().unwrap().push(request.clone());
                script(&mut stream, &request);
            }
        });
        Script { port, taken, seen }
    }

    fn url(&self, host: &str, path: &str) -> String {
        format!("http://{host}:{}{path}", self.port)
    }

    fn taken(&self) -> usize {
        self.taken.load(Ordering::SeqCst)
    }

    fn request(&self, n: usize) -> String {
        self.seen.lock().unwrap()[n].clone()
    }
}

/// The request line, the headers and the body (by its length), as they came.
fn read_request(stream: &mut TcpStream) -> String {
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let (mut request, mut length) = (String::new(), 0usize);
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = value.trim().parse().unwrap();
        }
        request.push_str(&line);
        if line == "\r\n" {
            break;
        }
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).unwrap();
    request.push_str(&String::from_utf8_lossy(&body));
    request
}

fn quick() -> Deadlines {
    Deadlines { connect: Duration::from_secs(2), first_byte: Duration::from_secs(2), idle: Duration::from_secs(2), total: Duration::from_secs(10) }
}

fn ask<'a>(url: &'a str, fields: &'a [(&'a str, &'a str)], deadlines: Deadlines, cancel: Option<&'a AtomicBool>) -> StreamRequest<'a> {
    StreamRequest {
        method: "POST",
        url,
        fields,
        body: b"{\"stream\":true}",
        trusted: "",
        deadlines,
        max_response: 64 * 1024,
        max_error_response: 1024,
        cancel,
    }
}

/// The pieces of an answer: the status, and each body piece with when it came.
#[derive(Default)]
struct Got {
    status: Option<u16>,
    pieces: Vec<(Vec<u8>, Instant)>,
}

impl Got {
    fn body(&self) -> String {
        self.pieces.iter().map(|(bytes, _)| String::from_utf8_lossy(bytes).into_owned()).collect()
    }
}

fn collect(request: &StreamRequest<'_>) -> (Result<StreamOutcome, NetError>, Got) {
    let mut got = Got::default();
    let result = stream(request, &mut |piece: Piece<'_>| {
        match piece {
            Piece::Head(head) => got.status = Some(head.status),
            Piece::Body(bytes) => got.pieces.push((bytes.to_vec(), Instant::now())),
        }
        Flow::Go
    });
    (result, got)
}

/// A chunked event stream written in three parts with pauses between them.
fn drip(stream: &mut TcpStream, _: &str) {
    let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n");
    for part in ["data: one\n\n", "data: two\n\n", "data: [DONE]\n\n"] {
        let _ = stream.write_all(format!("{:x}\r\n{part}\r\n", part.len()).as_bytes());
        let _ = stream.flush();
        std::thread::sleep(Duration::from_millis(150));
    }
    let _ = stream.write_all(b"0\r\n\r\n");
}

#[test]
fn the_answer_arrives_in_pieces_as_the_server_sends_them() {
    let server = Script::start(1, drip);
    let url = server.url("127.0.0.1", "/v1/chat/completions");
    let (result, got) = collect(&ask(&url, &[("Accept", "text/event-stream")], quick(), None));
    let outcome = result.unwrap();
    assert_eq!((outcome.head.status, outcome.stopped, outcome.truncated), (200, false, false));
    assert_eq!(outcome.head.content_type.as_deref(), Some("text/event-stream"));
    assert_eq!(got.status, Some(200));
    assert_eq!(got.body(), "data: one\n\ndata: two\n\ndata: [DONE]\n\n");
    // Not one piece at the end: the first came well before the last.
    assert!(got.pieces.len() >= 3, "{} pieces", got.pieces.len());
    let (first, last) = (got.pieces.first().unwrap().1, got.pieces.last().unwrap().1);
    assert!(last.duration_since(first) >= Duration::from_millis(200));
    assert_eq!(outcome.received, u64::try_from(got.body().len()).unwrap());
}

#[test]
fn plain_http_goes_to_loopback_addresses_however_they_are_written() {
    for host in ["127.0.0.1", "localhost", "[::ffff:127.0.0.1]"] {
        let server = Script::start(1, |stream, _| {
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
        });
        let url = server.url(host, "/v1/x");
        let (result, got) = collect(&ask(&url, &[], quick(), None));
        assert_eq!(result.unwrap().head.status, 200, "{host}");
        assert_eq!(got.body(), "ok", "{host}");
    }
}

#[test]
fn plain_http_elsewhere_is_refused_before_anything_is_sent() {
    for url in [
        "http://192.0.2.1/v1/chat/completions",
        "http://192.0.2.1:11434/v1/chat/completions",
        "http://[::ffff:192.0.2.1]:11434/v1",
        "http://[2001:db8::1]:11434/v1",
        "http://models.example.org/v1",
        "http://localhost.example.org/v1",
        "ftp://127.0.0.1/v1",
    ] {
        let started = Instant::now();
        let (result, got) = collect(&ask(url, &[], quick(), None));
        assert!(matches!(result, Err(NetError::Config(_))), "{url}: {:?}", result.map(|o| o.head.status));
        assert!(got.status.is_none() && got.pieces.is_empty(), "{url}");
        assert!(started.elapsed() < Duration::from_millis(500), "{url}: refused at once, nothing dialled");
    }
}

#[test]
fn only_loopback_addresses_pass_and_mapped_ones_are_written_as_ipv4() {
    let at = |ip: IpAddr| SocketAddr::new(ip, 80);
    let v4 = |a, b, c, d| IpAddr::V4(Ipv4Addr::new(a, b, c, d));
    let mapped = |a, b, c, d| IpAddr::V6(Ipv4Addr::new(a, b, c, d).to_ipv6_mapped());
    assert_eq!(http::loopback_ip(v4(127, 0, 0, 1)), Some(v4(127, 0, 0, 1)));
    assert_eq!(http::loopback_ip(v4(127, 255, 255, 254)), Some(v4(127, 255, 255, 254)));
    assert_eq!(http::loopback_ip(IpAddr::V6(Ipv6Addr::LOCALHOST)), Some(IpAddr::V6(Ipv6Addr::LOCALHOST)));
    assert_eq!(http::loopback_ip(mapped(127, 0, 0, 1)), Some(v4(127, 0, 0, 1)));
    for refused in [
        v4(192, 0, 2, 1),
        v4(10, 0, 0, 1),
        v4(0, 0, 0, 0),
        v4(169, 254, 0, 1),
        mapped(192, 0, 2, 1),
        IpAddr::V6(Ipv6Addr::UNSPECIFIED),
        IpAddr::V6("2001:db8::1".parse().unwrap()),
        // IPv4-compatible (not mapped) is not loopback.
        IpAddr::V6("::127.0.0.1".parse().unwrap()),
    ] {
        assert_eq!(http::loopback_ip(refused), None, "{refused}");
    }
    // A name stands for this machine only when every address it has is this machine's: one
    // address elsewhere refuses them all.
    assert_eq!(http::loopback_peers(&[at(v4(127, 0, 0, 1)), at(IpAddr::V6(Ipv6Addr::LOCALHOST))]).map(|p| p.len()), Some(2));
    assert_eq!(http::loopback_peers(&[at(v4(127, 0, 0, 1)), at(v4(192, 0, 2, 1))]), None);
    assert_eq!(http::loopback_peers(&[at(mapped(192, 0, 2, 1))]), None);
    assert_eq!(http::loopback_peers(&[]), None);
}

#[test]
fn a_mapped_address_in_brackets_is_rewritten_and_nothing_else_is() {
    assert_eq!(http::unmapped("http://[::ffff:127.0.0.1]:11434/v1"), "http://127.0.0.1:11434/v1");
    assert_eq!(http::unmapped(" http://[::ffff:7f00:1]/v1 "), "http://127.0.0.1/v1");
    assert_eq!(http::unmapped("http://[::ffff:192.0.2.1]/v1"), "http://192.0.2.1/v1");
    for same in ["http://[::1]:8080/v1", "https://example.org/v1", "http://[nonsense]/v1", "http://[::ffff:127.0.0.1/v1", "no scheme"] {
        assert_eq!(http::unmapped(same), same.trim());
    }
}

#[test]
fn a_redirect_is_given_back_and_not_followed() {
    let server = Script::start(2, |stream, _| {
        let _ = stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: http://192.0.2.1/elsewhere\r\nContent-Length: 0\r\n\r\n");
    });
    let url = server.url("127.0.0.1", "/v1/chat/completions");
    let (result, _) = collect(&ask(&url, &[], quick(), None));
    assert_eq!(result.unwrap().head.status, 302);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(server.taken(), 1);
}

#[test]
fn header_values_lose_their_control_characters() {
    let server = Script::start(1, |stream, _| {
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
    });
    let url = server.url("127.0.0.1", "/v1/x");
    let fields = [("Authorization", "Bearer sk\r\nX-Injected: yes"), ("X-Model", "a\0b\rc\nd"), ("Bad Name", "x"), ("Connection", "keep-alive")];
    let (result, _) = collect(&ask(&url, &fields, quick(), None));
    assert!(result.is_ok());
    let sent = server.request(0);
    assert!(sent.contains("Authorization: Bearer skX-Injected: yes\r\n"), "{sent}");
    assert!(sent.contains("X-Model: abcd\r\n"), "{sent}");
    assert!(!sent.contains("\r\nX-Injected") && !sent.contains("Bad Name") && !sent.contains('\0'), "{sent}");
    assert!(sent.contains("Connection: close\r\n") && !sent.contains("keep-alive"), "{sent}");
    assert!(sent.ends_with("\r\n\r\n{\"stream\":true}"), "{sent}");
}

#[test]
fn the_callers_flag_stops_the_stream_within_a_second() {
    let server = Script::start(1, |stream, _| {
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\ndata: one\n\n");
        let _ = stream.flush();
        // Then nothing, for longer than the test waits.
        std::thread::sleep(Duration::from_secs(8));
    });
    let url = server.url("127.0.0.1", "/v1/chat/completions");
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let deadlines = Deadlines { idle: Duration::from_secs(30), first_byte: Duration::from_secs(30), ..quick() };
    let request = ask(&url, &[], deadlines, Some(&cancel));
    let mut set_at = None;
    let result = stream(&request, &mut |piece: Piece<'_>| {
        if let Piece::Body(_) = piece {
            let flag = flag.clone();
            set_at = Some(Instant::now());
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(100));
                flag.store(true, Ordering::SeqCst);
            });
        }
        Flow::Go
    });
    let error = result.unwrap_err();
    assert!(is_stopped(&error), "{error:?}");
    assert!(set_at.unwrap().elapsed() < Duration::from_secs(1));
}

#[test]
fn a_flag_already_set_sends_nothing() {
    let server = Script::start(1, |_, _| {});
    let url = server.url("127.0.0.1", "/v1/x");
    let cancel = AtomicBool::new(true);
    let (result, _) = collect(&ask(&url, &[], quick(), Some(&cancel)));
    assert!(is_stopped(&result.unwrap_err()));
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(server.taken(), 0);
}

#[test]
fn the_caller_may_stop_at_any_piece() {
    let server = Script::start(1, drip);
    let url = server.url("127.0.0.1", "/v1/chat/completions");
    let mut pieces = 0;
    let outcome = stream(&ask(&url, &[], quick(), None), &mut |piece: Piece<'_>| match piece {
        Piece::Head(_) => Flow::Go,
        Piece::Body(_) => {
            pieces += 1;
            Flow::Stop
        }
    })
    .unwrap();
    assert!(outcome.stopped);
    assert_eq!(pieces, 1);
}

#[test]
fn a_server_that_never_answers_runs_out_of_its_first_byte_time() {
    let server = Script::start(1, |_, _| std::thread::sleep(Duration::from_secs(5)));
    let url = server.url("127.0.0.1", "/v1/x");
    let deadlines = Deadlines { first_byte: Duration::from_millis(600), ..quick() };
    let started = Instant::now();
    let (result, _) = collect(&ask(&url, &[], deadlines, None));
    let error = result.unwrap_err();
    assert!(is_timeout(&error), "{error:?}");
    let took = started.elapsed();
    assert!(took >= Duration::from_millis(550) && took < Duration::from_millis(1_600), "{took:?}");
}

#[test]
fn a_server_that_falls_silent_runs_out_of_its_idle_time() {
    let server = Script::start(1, |stream, _| {
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\ndata: one\n\n");
        let _ = stream.flush();
        std::thread::sleep(Duration::from_secs(5));
    });
    let url = server.url("127.0.0.1", "/v1/x");
    let deadlines = Deadlines { idle: Duration::from_millis(600), ..quick() };
    let (result, got) = collect(&ask(&url, &[], deadlines, None));
    assert!(is_timeout(&result.unwrap_err()));
    assert_eq!(got.body(), "data: one\n\n");
}

#[test]
fn a_closed_port_is_an_error_within_a_second() {
    let port = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    for host in ["127.0.0.1", "localhost"] {
        let url = format!("http://{host}:{port}/v1/chat/completions");
        let started = Instant::now();
        let (result, _) = collect(&ask(&url, &[], quick(), None));
        let error = result.unwrap_err();
        assert!(matches!(error, NetError::Connect { .. }), "{host}: {error:?}");
        assert!(started.elapsed() < Duration::from_secs(1), "{host}");
    }
}

#[test]
fn a_success_longer_than_asked_for_is_an_error_and_a_failure_is_cut_short() {
    let long = "x".repeat(70 * 1024);
    let answer = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{long}", long.len());
    let server = Script::start(1, move |stream, _| {
        let _ = stream.write_all(answer.as_bytes());
    });
    let url = server.url("127.0.0.1", "/v1/x");
    let (result, got) = collect(&ask(&url, &[], quick(), None));
    assert!(matches!(result, Err(NetError::Proto(_))));
    assert!(got.pieces.is_empty(), "refused before a byte of the body");

    let error_body = format!("{{\"error\":{{\"message\":\"{}\"}}}}", "y".repeat(5_000));
    for framing in ["length", "chunked", "to the end"] {
        let body = error_body.clone();
        let server = Script::start(1, move |stream, _| {
            let answer = match framing {
                "length" => format!("HTTP/1.1 500 Oops\r\nContent-Length: {}\r\n\r\n{body}", body.len()),
                "chunked" => format!("HTTP/1.1 500 Oops\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{body}\r\n0\r\n\r\n", body.len()),
                _ => format!("HTTP/1.1 500 Oops\r\nConnection: close\r\n\r\n{body}"),
            };
            let _ = stream.write_all(answer.as_bytes());
        });
        let url = server.url("127.0.0.1", "/v1/x");
        let (result, got) = collect(&ask(&url, &[], quick(), None));
        let outcome = result.unwrap();
        assert_eq!((outcome.head.status, outcome.truncated, outcome.received), (500, true, 1024), "{framing}");
        assert_eq!(got.body().len(), 1024, "{framing}");
    }
}

#[test]
fn the_debug_of_a_request_shows_no_address_and_no_credential() {
    let fields = [("Authorization", "Bearer sk-very-secret")];
    let request = ask("http://127.0.0.1:9/v1/private-path", &fields, quick(), None);
    let shown = format!("{request:?}");
    assert!(!shown.contains("secret") && !shown.contains("private-path") && !shown.contains("127.0.0.1"), "{shown}");
}

#[test]
fn the_deadlines_are_kept_within_the_ceilings() {
    let wild =
        Deadlines { connect: Duration::ZERO, first_byte: Duration::from_secs(9_999), idle: Duration::ZERO, total: Duration::from_secs(99_999) };
    let server = Script::start(1, |stream, _| {
        let _ = stream.write_all(b"HTTP/1.1 204 No Content\r\n\r\n");
    });
    let url = server.url("127.0.0.1", "/v1/x");
    // A zero connect time is a millisecond, never "for ever": this may fail to connect, but returns.
    let started = Instant::now();
    let _ = collect(&ask(&url, &[], wild, None));
    assert!(started.elapsed() < Duration::from_secs(5));
}
