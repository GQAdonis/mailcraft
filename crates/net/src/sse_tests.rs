// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The event-stream reader: the standard's framing, pieces cut anywhere, the caps, and hostile
//! bytes that never panic.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use proptest::prelude::*;

use super::*;

/// The events of `stream` read in one piece, the stream then ended.
fn events(stream: &[u8]) -> Result<Vec<Event>, SseError> {
    events_within(stream, Limits::default())
}

fn events_within(stream: &[u8], limits: Limits) -> Result<Vec<Event>, SseError> {
    let mut parser = Parser::new(limits);
    let mut out = Vec::new();
    parser.feed(stream, &mut out)?;
    parser.finish(&mut out)?;
    Ok(out)
}

/// The events of `stream` fed one byte at a time.
fn events_bytewise(stream: &[u8]) -> Result<Vec<Event>, SseError> {
    let mut parser = Parser::default();
    let mut out = Vec::new();
    for byte in stream {
        parser.feed(std::slice::from_ref(byte), &mut out)?;
    }
    parser.finish(&mut out)?;
    Ok(out)
}

fn data(events: &[Event]) -> Vec<&str> {
    events.iter().map(|e| e.data.as_str()).collect()
}

#[test]
fn an_openai_style_stream_reads_as_its_chunks_and_done() {
    let stream = b"data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n\
data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n\
data: [DONE]\n\n";
    let got = events(stream).unwrap();
    assert_eq!(got.len(), 3);
    assert!(got[0].data.contains("Hel") && got[1].data.contains("lo"));
    assert!(!got[0].is_done() && got[2].is_done());
    assert_eq!(got[0].kind, None);
}

#[test]
fn every_line_end_is_one_and_a_cr_lf_cut_in_two_is_still_one() {
    for stream in [&b"data: a\r\n\r\ndata: b\r\n\r\n"[..], b"data: a\n\ndata: b\n\n", b"data: a\r\rdata: b\r\r", b"data: a\r\n\ndata: b\n\r\n"] {
        assert_eq!(data(&events(stream).unwrap()), ["a", "b"], "{stream:?}");
        assert_eq!(data(&events_bytewise(stream).unwrap()), ["a", "b"], "{stream:?}");
    }
    // A CR at the end of one piece and the LF at the start of the next.
    let mut parser = Parser::default();
    let mut out = Vec::new();
    parser.feed(b"data: a\r", &mut out).unwrap();
    parser.feed(b"\n\r", &mut out).unwrap();
    parser.feed(b"\n", &mut out).unwrap();
    assert_eq!(data(&out), ["a"]);
}

#[test]
fn comments_names_ids_and_unknown_fields() {
    let stream = b": keep-alive\n\n:another\nevent: delta\nid: 7\nretry: 10\nunknown: x\ndata\ndata:  two spaces\ndata:x\n\n";
    let got = events(stream).unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].kind.as_deref(), Some("delta"));
    assert_eq!(got[0].id.as_deref(), Some("7"));
    // `data` alone is an empty line of data; one space after the colon goes, a second stays.
    assert_eq!(got[0].data, "\n two spaces\nx");
    // The name is the event's own, the id the stream's.
    let got = events(b"event: a\nid: 1\ndata: x\n\ndata: y\n\nid: 2\0\ndata: z\n\n").unwrap();
    assert_eq!(
        got.iter().map(|e| (e.kind.as_deref(), e.id.as_deref())).collect::<Vec<_>>(),
        [(Some("a"), Some("1")), (None, Some("1")), (None, Some("1"))]
    );
}

#[test]
fn an_event_without_data_is_none_and_its_name_is_forgotten() {
    let got = events(b"event: ping\n\n\n\ndata: a\n\n").unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].kind, None);
    assert_eq!(got[0].data, "a");
    // `data:` with nothing after it is an event of empty data.
    assert_eq!(data(&events(b"data:\n\n").unwrap()), [""]);
}

#[test]
fn a_byte_order_mark_goes_and_what_isnt_utf8_is_replaced() {
    assert_eq!(data(&events(b"\xEF\xBB\xBFdata: a\n\n").unwrap()), ["a"]);
    // Only at the start.
    assert_eq!(events(b"data: a\n\n\xEF\xBB\xBFdata: b\n\n").unwrap().len(), 1);
    assert_eq!(data(&events(b"data: \xFF\xFEok\n\n").unwrap()), ["\u{FFFD}\u{FFFD}ok"]);
}

#[test]
fn a_stream_cut_off_still_gives_its_last_event() {
    assert_eq!(data(&events(b"data: a\n\ndata: b").unwrap()), ["a", "b"]);
    assert_eq!(data(&events(b"data: a\n\ndata: b\n").unwrap()), ["a", "b"]);
    assert!(events(b"").unwrap().is_empty());
    assert!(events(b": only a comment").unwrap().is_empty());
}

#[test]
fn the_caps_hold_and_stay_held() {
    let small = Limits { max_line: 16, max_event: 32, max_total: 1024, max_events: 3 };
    assert_eq!(events_within(&[b'x'; 17], small), Err(SseError::LineTooLong));
    assert!(events_within(&[b'x'; 16], small).is_ok());
    assert_eq!(events_within(b"data: 0123456789\ndata: 0123456789\ndata: 0123456789\n\n", small), Err(SseError::EventTooLarge));
    assert_eq!(events_within(b"data: a\n\ndata: b\n\ndata: c\n\ndata: d\n\n", small), Err(SseError::TooManyEvents));
    assert_eq!(events_within(&b"\n".repeat(1025), small), Err(SseError::StreamTooLarge));
    // Once given up on, always given up on, and nothing more comes out.
    let mut parser = Parser::new(small);
    let mut out = Vec::new();
    assert_eq!(parser.feed(&[b'x'; 40], &mut out), Err(SseError::LineTooLong));
    assert_eq!(parser.feed(b"\ndata: a\n\n", &mut out), Err(SseError::LineTooLong));
    assert_eq!(parser.finish(&mut out), Err(SseError::LineTooLong));
    assert!(out.is_empty());
}

#[test]
fn the_debug_of_an_event_shows_no_text() {
    let got = events(b"data: the secret meeting is at noon\n\n").unwrap();
    let shown = format!("{:?}", got[0]);
    assert!(!shown.contains("secret") && shown.contains("data_len"), "{shown}");
}

proptest! {
    #[test]
    fn arbitrary_bytes_never_panic_and_stay_within_the_caps(bytes in proptest::collection::vec(any::<u8>(), 0..2_000)) {
        let limits = Limits { max_line: 64, max_event: 128, max_total: 1_500, max_events: 20 };
        if let Ok(got) = events_within(&bytes, limits) {
            prop_assert!(got.len() <= 20);
            prop_assert!(got.iter().all(|e| e.data.len() <= 128));
        }
    }

    #[test]
    fn where_the_pieces_are_cut_changes_nothing(
        lines in proptest::collection::vec(prop_oneof![
            Just("data: [DONE]".to_string()),
            Just(String::new()),
            "(data|event|id|retry|:)?:? ?[ -~]{0,12}",
        ], 0..20),
        ends in proptest::collection::vec(prop_oneof![Just("\n"), Just("\r"), Just("\r\n")], 20),
        cuts in proptest::collection::vec(0usize..400, 0..8),
    ) {
        let stream: String = lines.iter().zip(ends.iter().cycle()).map(|(line, end)| format!("{line}{end}")).collect();
        let bytes = stream.as_bytes();
        let whole = events(bytes);
        let mut cuts: Vec<usize> = cuts.into_iter().map(|c| c.min(bytes.len())).collect();
        cuts.sort_unstable();
        let mut parser = Parser::default();
        let mut out = Vec::new();
        let mut from = 0;
        let mut pieced = Ok(());
        for cut in cuts.into_iter().chain(std::iter::once(bytes.len())) {
            pieced = pieced.and_then(|()| parser.feed(&bytes[from..cut], &mut out));
            from = cut;
        }
        let pieced = pieced.and_then(|()| parser.finish(&mut out)).map(|()| out);
        prop_assert_eq!(whole, pieced);
        prop_assert_eq!(events_bytewise(bytes), events(bytes));
    }
}
