// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A model server's event stream of any bytes, read whole and cut in two: never a panic, the
//! caps hold, and where the stream is cut changes nothing.
#![no_main]

use mailcraft_net::sse::{Event, Limits, Parser, SseError};

const LIMITS: Limits = Limits { max_line: 1024, max_event: 4096, max_total: 1 << 20, max_events: 1_000 };

fn read(pieces: &[&[u8]]) -> Result<Vec<Event>, SseError> {
    let mut parser = Parser::new(LIMITS);
    let mut out = Vec::new();
    for piece in pieces {
        parser.feed(piece, &mut out)?;
    }
    parser.finish(&mut out)?;
    Ok(out)
}

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let whole = read(&[data]);
    let cut = data.first().map_or(0, |b| usize::from(*b)).min(data.len());
    let (head, tail) = data.split_at(cut);
    let pieced = read(&[head, tail]);
    match (&whole, &pieced) {
        (Ok(one), Ok(other)) => assert_eq!(one, other),
        (Err(_), Err(_)) => {}
        _ => panic!("where the stream was cut changed whether it could be read"),
    }
    if let Ok(events) = whole {
        assert!(events.len() <= 1_000);
        assert!(events.iter().all(|event| event.data.len() <= 4096));
    }
});
