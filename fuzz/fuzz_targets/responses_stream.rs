// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The ChatGPT plan's Responses stream parser (B7, 17.4): any bytes, as an event stream, whole
//! and cut in two, through the same caps, detector and judge a run uses. Never a panic; where the
//! stream is cut changes nothing; at most one step ends the run and nothing comes after it; a
//! kill's kind is never empty, and an answer is never larger than a run keeps.
#![no_main]

use mailcraft_ai::runtime::contain::{MAX_ANSWER_BYTES, Step};
use mailcraft_ai::runtime::responses::drive_stream;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let cut = data.first().map_or(0, |b| usize::from(*b) * 7);
    let whole = drive_stream(data, 0);
    let split = drive_stream(data, cut);
    assert_eq!(whole, split, "where the stream was cut changed what was judged");
    assert!(whole.len() <= 1, "something came of a run after its end");
    for step in &whole {
        match step {
            Step::Kill(kind) => assert!(!kind.is_empty()),
            Step::Done(text) => assert!(text.len() <= MAX_ANSWER_BYTES),
            Step::TooLarge | Step::Continue | Step::WritePrompt => {}
        }
    }
});
