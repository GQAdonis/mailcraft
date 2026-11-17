// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! DNS rebinding is refused, proven at unit level (D-rebind, SCN-LOOPBACK-001): a plain model
//! server's name is looked up once, through the [`Resolve`] handed in, and the connection goes
//! to the very address that was checked. Every server here is an in-process listener on
//! 127.0.0.1; every other address is a documentation or reserved one, and none is connected to.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::*;
use crate::NetError;
use crate::http::{self, Deadlines, Flow, Piece, StreamCall, Url};

/// A resolver that answers from a script: the first lookup gets the first answer, the second the
/// second, and so on (the last again after that). It counts the lookups and keeps the names.
struct Scripted {
    answers: Vec<Vec<SocketAddr>>,
    asked: AtomicUsize,
    names: Mutex<Vec<(String, u16)>>,
}

impl Scripted {
    fn new(answers: Vec<Vec<SocketAddr>>) -> Scripted {
        Scripted { answers, asked: AtomicUsize::new(0), names: Mutex::new(Vec::new()) }
    }

    fn asked(&self) -> usize {
        self.asked.load(Ordering::SeqCst)
    }
}

impl Resolve for Scripted {
    fn resolve(&self, host: &str, port: u16) -> std::io::Result<Vec<SocketAddr>> {
        let n = self.asked.fetch_add(1, Ordering::SeqCst);
        self.names.lock().unwrap().push((host.to_string(), port));
        Ok(self.answers.get(n).or(self.answers.last()).cloned().unwrap_or_default())
    }
}

/// A resolver whose every lookup fails.
struct Failing(AtomicUsize);

impl Resolve for Failing {
    fn resolve(&self, _host: &str, _port: u16) -> std::io::Result<Vec<SocketAddr>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(std::io::Error::new(std::io::ErrorKind::NotFound, "no such name"))
    }
}

/// What the listener saw of the one connection it took: the address it was reached at, and the
/// request as it came.
struct Seen {
    local: SocketAddr,
    request: String,
}

/// A listener on 127.0.0.1 that waits up to `wait` for one connection, reads its request and
/// answers it with a short event stream. Its port, and what it saw (`None`: nobody came).
fn listen(wait: Duration) -> (u16, mpsc::Receiver<Option<Seen>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let (tell, told) = mpsc::channel();
    std::thread::spawn(move || {
        let until = Instant::now() + wait;
        let seen = loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).unwrap();
                    let local = stream.local_addr().unwrap();
                    let request = read_request(&mut stream);
                    let body = "data: hello\n\n";
                    let answer = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    stream.write_all(answer.as_bytes()).unwrap();
                    stream.flush().unwrap();
                    break Some(Seen { local, request });
                }
                Err(_) if Instant::now() < until => std::thread::sleep(Duration::from_millis(10)),
                Err(_) => break None,
            }
        };
        let _ = tell.send(seen);
    });
    (port, told)
}

/// The request line, the headers and the body (by its length), as they came.
fn read_request(stream: &mut std::net::TcpStream) -> String {
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

const BODY: &[u8] = br#"{"model":"local","stream":true}"#;

/// One streamed request to `url` through `resolver`; the result, and how many pieces of the
/// answer were handed over.
fn ask(url: &str, resolver: &dyn Resolve) -> (Result<http::StreamOutcome, NetError>, usize) {
    let url = Url::parse(url).unwrap();
    let call = StreamCall {
        method: "POST",
        headers: &[("Content-Type", "application/json")],
        body: BODY,
        deadlines: quick(),
        max_body: 64 * 1024,
        max_error_body: 4 * 1024,
        cancel: None,
    };
    let mut pieces = 0usize;
    let result = http::post_stream_via(&url, &call, None, resolver, &mut |_piece: Piece<'_>| {
        pieces += 1;
        Flow::Go
    });
    (result, pieces)
}

fn at(ip: IpAddr, port: u16) -> SocketAddr {
    SocketAddr::new(ip, port)
}

fn v4(a: u8, b: u8, c: u8, d: u8) -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(a, b, c, d))
}

fn mapped(a: u8, b: u8, c: u8, d: u8) -> IpAddr {
    IpAddr::V6(Ipv4Addr::new(a, b, c, d).to_ipv6_mapped())
}

fn refused_as_not_loopback(result: &Result<http::StreamOutcome, NetError>) -> bool {
    matches!(result, Err(NetError::Config(_)))
}

// (a) The first lookup answers loopback and a second would answer a public address: exactly one
// lookup is made, and the connection goes to the loopback address that was checked.
#[test]
fn a_name_is_looked_up_once_and_the_checked_address_is_connected_to() {
    let (port, told) = listen(Duration::from_secs(5));
    let resolver = Scripted::new(vec![vec![at(v4(127, 0, 0, 1), port)], vec![at(v4(203, 0, 113, 7), port)]]);

    let (result, pieces) = ask(&format!("http://localhost:{port}/v1/chat/completions"), &resolver);

    let outcome = result.unwrap();
    assert_eq!(outcome.head.status, 200);
    assert!(pieces >= 2, "the head and the body were handed over");
    assert_eq!(resolver.asked(), 1, "the name is looked up once");
    assert_eq!(resolver.names.lock().unwrap().as_slice(), &[("localhost".to_string(), port)]);
    let seen = told.recv_timeout(Duration::from_secs(10)).unwrap().expect("the loopback listener was connected to");
    assert_eq!(seen.local, at(v4(127, 0, 0, 1), port));
    assert!(seen.request.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"), "{}", seen.request);
    assert!(seen.request.ends_with(std::str::from_utf8(BODY).unwrap()));
}

// (a), the same over IPv6: an answer of `::1` alone is connected to as `::1`, once.
#[test]
fn an_ipv6_loopback_answer_is_used_as_given() {
    let Ok(listener) = TcpListener::bind("[::1]:0") else {
        // A machine without IPv6 loopback has nothing here to show.
        return;
    };
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let local = stream.local_addr().unwrap();
        let _ = read_request(&mut stream);
        stream.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n").unwrap();
        local
    });
    let resolver = Scripted::new(vec![vec![at(IpAddr::V6(Ipv6Addr::LOCALHOST), port)], vec![at(v4(203, 0, 113, 7), port)]]);

    let (result, _) = ask(&format!("http://localhost:{port}/v1/chat/completions"), &resolver);

    assert_eq!(result.unwrap().head.status, 204);
    assert_eq!(resolver.asked(), 1);
    assert_eq!(server.join().unwrap(), at(IpAddr::V6(Ipv6Addr::LOCALHOST), port));
}

// (b) The first lookup answers a public address for an `http://` address: refused before
// anything is sent, and nothing anywhere is connected to.
#[test]
fn a_public_answer_over_plain_http_is_refused_and_nothing_is_sent() {
    let (port, told) = listen(Duration::from_millis(500));
    let resolver = Scripted::new(vec![vec![at(v4(203, 0, 113, 7), port)], vec![at(v4(127, 0, 0, 1), port)]]);

    let (result, pieces) = ask(&format!("http://localhost:{port}/v1/chat/completions"), &resolver);

    assert!(refused_as_not_loopback(&result), "{:?}", result.map(|o| o.head.status));
    assert_eq!(pieces, 0, "nothing came back, as nothing went");
    assert_eq!(resolver.asked(), 1, "no second lookup is made to look for a better answer");
    assert!(told.recv_timeout(Duration::from_secs(5)).unwrap().is_none(), "nothing reached the loopback listener either");
}

// (b), mixed: one public address among loopback ones is enough to refuse the whole answer.
#[test]
fn one_public_address_among_loopback_ones_is_refused() {
    let (port, told) = listen(Duration::from_millis(500));
    let resolver = Scripted::new(vec![vec![at(v4(127, 0, 0, 1), port), at(v4(198, 51, 100, 9), port)]]);

    let (result, pieces) = ask(&format!("http://localhost:{port}/v1/models"), &resolver);

    assert!(refused_as_not_loopback(&result));
    assert_eq!((pieces, resolver.asked()), (0, 1));
    assert!(told.recv_timeout(Duration::from_secs(5)).unwrap().is_none());
}

// (b), the edges: an empty answer is refused, and a failed lookup is a connection error, each
// after one lookup and with nothing sent.
#[test]
fn an_empty_or_failed_lookup_sends_nothing() {
    let (port, told) = listen(Duration::from_millis(500));
    let url = format!("http://localhost:{port}/v1/models");

    let empty = Scripted::new(vec![Vec::new()]);
    let (result, pieces) = ask(&url, &empty);
    assert!(refused_as_not_loopback(&result));
    assert_eq!((pieces, empty.asked()), (0, 1));

    let failing = Failing(AtomicUsize::new(0));
    let (result, pieces) = ask(&url, &failing);
    assert!(matches!(result, Err(NetError::Connect { .. })), "{:?}", result.map(|o| o.head.status));
    assert_eq!((pieces, failing.0.load(Ordering::SeqCst)), (0, 1));

    assert!(told.recv_timeout(Duration::from_secs(5)).unwrap().is_none());
}

// (c) An IPv4 loopback address mapped into IPv6 is accepted, and connected to as the IPv4
// address it is.
#[test]
fn an_ipv4_mapped_loopback_answer_is_accepted() {
    let (port, told) = listen(Duration::from_secs(5));
    let resolver = Scripted::new(vec![vec![at(mapped(127, 0, 0, 1), port)], vec![at(v4(203, 0, 113, 7), port)]]);

    let (result, _) = ask(&format!("http://localhost:{port}/v1/chat/completions"), &resolver);

    assert_eq!(result.unwrap().head.status, 200);
    assert_eq!(resolver.asked(), 1);
    let seen = told.recv_timeout(Duration::from_secs(10)).unwrap().expect("the mapped loopback address was connected to");
    assert_eq!(seen.local, at(v4(127, 0, 0, 1), port));
}

// (c) `0.0.0.0`, `::`, link-local (169.254.0.0/16, fe80::/10) and their mapped forms are not this
// machine's loopback: each is refused, after one lookup, and nothing is connected to.
#[test]
fn unspecified_and_link_local_answers_are_refused() {
    let (port, told) = listen(Duration::from_millis(800));
    let url = format!("http://localhost:{port}/v1/chat/completions");
    let refused = [
        v4(0, 0, 0, 0),
        IpAddr::V6(Ipv6Addr::UNSPECIFIED),
        v4(169, 254, 1, 1),
        v4(169, 254, 169, 254),
        IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1)),
        mapped(0, 0, 0, 0),
        mapped(169, 254, 1, 1),
    ];
    for ip in refused {
        let resolver = Scripted::new(vec![vec![at(ip, port)], vec![at(v4(127, 0, 0, 1), port)]]);
        let (result, pieces) = ask(&url, &resolver);
        assert!(refused_as_not_loopback(&result), "{ip}: {:?}", result.map(|o| o.head.status));
        assert_eq!((pieces, resolver.asked()), (0, 1), "{ip}");
        assert_eq!(http::loopback_ip(ip), None, "{ip}");
    }
    assert!(told.recv_timeout(Duration::from_secs(5)).unwrap().is_none(), "none of them reached the listener");
}

// A hostile resolver's flood of addresses is cut to the cap and still checked whole: one
// public address past the loopback ones is not hidden by their number.
#[test]
fn a_flood_of_addresses_is_capped_and_still_checked() {
    let (port, told) = listen(Duration::from_millis(500));
    let mut answer = vec![at(v4(127, 0, 0, 1), port); MAX_RESOLVED - 1];
    answer.push(at(v4(203, 0, 113, 7), port));
    answer.extend(vec![at(v4(127, 0, 0, 1), port); 10_000]);
    let resolver = Scripted::new(vec![answer]);

    let (result, pieces) = ask(&format!("http://localhost:{port}/v1/models"), &resolver);

    assert!(refused_as_not_loopback(&result));
    assert_eq!((pieces, resolver.asked()), (0, 1));
    assert!(told.recv_timeout(Duration::from_secs(5)).unwrap().is_none());
}

// The system's resolver looks up an address written as one without asking anyone, and keeps no
// more than the cap.
#[test]
fn the_system_resolver_reads_a_literal_address() {
    let found = SystemResolver.resolve("127.0.0.1", 8080).unwrap();
    assert_eq!(found, vec![at(v4(127, 0, 0, 1), 8080)]);
    let found = SystemResolver.resolve("::1", 8080).unwrap();
    assert_eq!(found, vec![at(IpAddr::V6(Ipv6Addr::LOCALHOST), 8080)]);
    assert!(found.len() <= MAX_RESOLVED);
}
