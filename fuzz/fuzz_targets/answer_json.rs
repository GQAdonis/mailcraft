// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A model's structured answer of any bytes, its text read as it streams (whole and cut in two)
//! and the whole read at the end: never a panic, the caps hold, and where the stream is cut
//! changes nothing.
#![no_main]

use mailcraft_model::ai::{AnswerExtractor, MAX_ANSWER_CITATIONS, MAX_RESPONSE_BYTES, read_answer_json};

fn streamed(pieces: &[&str]) -> String {
    let mut x = AnswerExtractor::new();
    let mut out = String::new();
    for piece in pieces {
        out.push_str(&x.push(piece));
    }
    out
}

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let whole = streamed(&[&text]);
    assert!(whole.len() <= MAX_RESPONSE_BYTES);
    let mut cut = data.first().map_or(0, |b| usize::from(*b)).min(text.len());
    while !text.is_char_boundary(cut) {
        cut = cut.saturating_sub(1);
    }
    let (head, tail) = text.split_at(cut);
    assert_eq!(whole, streamed(&[head, tail]), "where the stream was cut changed what was read");
    let sources = data.len() % 24;
    if let Ok(answer) = read_answer_json(&text, sources) {
        assert!(answer.answer.len() <= MAX_RESPONSE_BYTES);
        assert!(answer.citations.len() <= MAX_ANSWER_CITATIONS);
        assert!(answer.citations.iter().all(|n| (1..=sources).contains(n)));
    }
});
