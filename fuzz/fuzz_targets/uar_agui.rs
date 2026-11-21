// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! UAR's AG-UI stream of any bytes (B9, 19.2): read as a pipe's lines through B4's shared harness
//! (`contain::drive`, whole and cut in two) and as an SSE stream event by event, as a run reads it.
//! Never a panic; where the stream is cut changes nothing; once a run is killed nothing more comes
//! of it; an answer is never longer than the cap; the readiness line, the agent card, the
//! capabilities and the model list readers take the same bytes without a panic.
#![no_main]

use mailcraft_ai::runtime::contain::{Judge, MAX_ANSWER_BYTES, Step, drive};
use mailcraft_ai::runtime::uar::{AgUiDetector, ManagedSupport, capabilities_ok, models_beyond, parse_card, parse_ready};
use mailcraft_net::sse::{Limits, Parser};

fn check(steps: &[Step]) {
    let mut ended = false;
    for s in steps {
        if ended {
            assert_eq!(s, &Step::Continue, "something came of a run after its end");
        }
        match s {
            Step::Kill(kind) => {
                assert!(!kind.is_empty());
                ended = true;
            }
            Step::Done(text) => {
                assert!(text.len() <= MAX_ANSWER_BYTES);
                ended = true;
            }
            Step::TooLarge => ended = true,
            Step::Continue | Step::WritePrompt => {}
        }
    }
}

const LIMITS: Limits = Limits { max_line: 4096, max_event: 4096, max_total: 1 << 20, max_events: 1_000 };

fn as_sse(pieces: &[&[u8]]) -> Vec<Step> {
    let mut parser = Parser::new(LIMITS);
    let mut judge = Judge::new(AgUiDetector::new());
    let mut events = Vec::new();
    let mut steps = Vec::new();
    for piece in pieces {
        if parser.feed(piece, &mut events).is_err() {
            return steps;
        }
        steps.extend(events.drain(..).map(|e| judge.line(e.data.as_bytes())));
    }
    if parser.finish(&mut events).is_ok() {
        steps.extend(events.drain(..).map(|e| judge.line(e.data.as_bytes())));
    }
    steps
}

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let cut = data.first().map_or(0, |b| usize::from(*b));
    let whole = drive(AgUiDetector::new(), data, 0);
    let split = drive(AgUiDetector::new(), data, cut);
    assert_eq!(whole, split, "where the stream was cut changed what was judged");
    check(&whole);

    let cut = cut.min(data.len());
    let (head, tail) = data.split_at(cut);
    let one = as_sse(&[data]);
    let two = as_sse(&[head, tail]);
    check(&one);
    check(&two);

    let _ = parse_ready(data);
    let _ = parse_card(data);
    let _ = capabilities_ok(data);
    let _ = models_beyond(data, "model");
    let _ = ManagedSupport::from_help(&String::from_utf8_lossy(data));
});
