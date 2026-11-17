// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::answer::MAX_ANSWER_JSON_BYTES;
use super::call::MAX_RESPONSE_BYTES;
use super::extract::*;

/// `s` as a JSON string with every character outside ASCII written as `\uXXXX` (a surrogate pair
/// for those past U+FFFF), and every ASCII one as serde_json writes it.
fn escaped(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        if c.is_ascii() {
            let one = serde_json::to_string(&c.to_string()).unwrap();
            out.push_str(&one[1..one.len() - 1]);
        } else {
            for u in c.encode_utf16(&mut [0u16; 2]) {
                out.push_str(&format!("\\u{u:04x}"));
            }
        }
    }
    out.push('"');
    out
}

/// A document with a decoy before the answer and its citations after.
fn document(answer_json: &str) -> String {
    format!(
        r#"{{"before": {{"answer": "decoy", "x": [1, "]}}\"", {{"answer": "no"}}]}}, "note": "answer", "n": -1.5e3, "answer": {answer_json}, "citations": [1]}}"#
    )
}

/// `doc` given in pieces cut at the characters `cuts` name.
fn in_pieces(doc: &str, cuts: &[usize]) -> (String, AnswerExtractor) {
    let chars: Vec<char> = doc.chars().collect();
    let mut points: Vec<usize> = cuts.iter().map(|c| c % (chars.len() + 1)).collect();
    points.sort_unstable();
    let mut x = AnswerExtractor::new();
    let mut out = String::new();
    let mut from = 0;
    for p in points.into_iter().chain([chars.len()]) {
        let piece: String = chars[from..p.max(from)].iter().collect();
        out.push_str(&x.push(&piece));
        from = p.max(from);
    }
    (out, x)
}

#[test]
fn the_answer_is_read_past_decoys_with_its_escapes() {
    let doc = document(r#""Line one\nTab\there \"quoted\" back\\slash \/ é 😀 end""#);
    let mut x = AnswerExtractor::new();
    assert_eq!(x.push(&doc), "Line one\nTab\there \"quoted\" back\\slash / é 😀 end");
    assert!(x.complete() && !x.cut());
    // Its name escaped is still its name; one inside another value isn't it.
    assert_eq!(extract_answer(r#"{"answer": "yes"}"#), "yes");
    assert_eq!(extract_answer(r#"{"answers": "no", "answer": "yes"}"#), "yes");
    assert_eq!(extract_answer(r#"{"a": {"answer": "no"}}"#), "");
    assert_eq!(extract_answer(r#"{"answer": 5, "x": "no"}"#), "");
    // Not an object: nothing.
    for not in ["", "[\"answer\", \"x\"]", "\"answer\"", "answer: x", "{answer: \"x\"}"] {
        assert_eq!(extract_answer(not), "", "{not}");
    }
}

#[test]
fn lone_surrogates_and_broken_escapes_become_replacement_characters() {
    assert_eq!(extract_answer(r#"{"answer": "a\ud83db""#), "a\u{FFFD}b");
    assert_eq!(extract_answer(r#"{"answer": "a\ude00b""#), "a\u{FFFD}b");
    assert_eq!(extract_answer(r#"{"answer": "a\ud83d\nb""#), "a\u{FFFD}\nb");
    assert_eq!(extract_answer(r#"{"answer": "a\ud83dAb""#), "a\u{FFFD}Ab");
    assert_eq!(extract_answer(r#"{"answer": "a\ud83d😀""#), "a\u{FFFD}😀");
    assert_eq!(extract_answer(r#"{"answer": "a\u12zb""#), "a\u{FFFD}zb");
    assert_eq!(extract_answer(r#"{"answer": "a\u12"b""#), "a\u{FFFD}");
    assert_eq!(extract_answer(r#"{"answer": "a\qb""#), "aqb");
}

#[test]
fn an_escape_cut_between_pieces_is_finished_by_the_next() {
    let doc = r#"{"answer": "x😀\n\"y"}"#;
    for cut in 0..=doc.len() {
        let (out, x) = in_pieces(doc, &[cut]);
        assert_eq!(out, "x😀\n\"y", "cut at {cut}");
        assert!(x.complete());
    }
}

#[test]
fn the_bounds_hold() {
    let huge = format!(r#"{{"answer": "{}"}}"#, "é".repeat(MAX_RESPONSE_BYTES));
    let mut x = AnswerExtractor::new();
    let out = x.push(&huge);
    assert!(out.len() <= MAX_RESPONSE_BYTES && x.cut() && !x.complete());
    assert_eq!(x.push("more"), "");
    let deep = format!(r#"{{"x": {}, "answer": "y"}}"#, "[".repeat(10_000));
    let mut x = AnswerExtractor::new();
    assert_eq!(x.push(&deep), "");
    assert!(x.cut());
    let mut x = AnswerExtractor::new();
    x.push(&format!(r#"{{"x": "{}"#, " ".repeat(MAX_ANSWER_JSON_BYTES)));
    assert!(x.cut());
    assert_eq!(x.push(r#"", "answer": "y"}"#), "");
}

proptest::proptest! {
    #[test]
    fn any_answer_is_read_back_whole_however_it_is_cut(answer in proptest::prelude::any::<String>(), cuts in proptest::collection::vec(0usize..2_000, 0..6)) {
        let answer: String = answer.chars().take(400).collect();
        for doc in [document(&serde_json::to_string(&answer).unwrap()), document(&escaped(&answer))] {
            let (out, x) = in_pieces(&doc, &cuts);
            proptest::prop_assert_eq!(&out, &answer);
            proptest::prop_assert!(x.complete());
            // What a document's start gives is a start of the answer.
            let chars: Vec<char> = doc.chars().collect();
            let at = cuts.first().copied().unwrap_or(0) % (chars.len() + 1);
            let start: String = chars[..at].iter().collect();
            proptest::prop_assert!(answer.starts_with(&extract_answer(&start)));
        }
    }

    #[test]
    fn any_text_at_all_is_read_without_a_panic_within_the_bounds(text in proptest::prelude::any::<String>(), cuts in proptest::collection::vec(0usize..2_000, 0..6)) {
        let (whole, _) = in_pieces(&text, &[]);
        let (pieced, _) = in_pieces(&text, &cuts);
        proptest::prop_assert_eq!(&whole, &pieced, "where the text was cut changed what was read");
        proptest::prop_assert!(whole.len() <= MAX_RESPONSE_BYTES);
    }

    #[test]
    fn json_like_text_is_read_without_a_panic(text in "[{}\\[\\]\":,\\\\ua-f0-9 answer\\n]{0,300}") {
        let doc = format!("{{{text}");
        let (whole, _) = in_pieces(&doc, &[]);
        let (pieced, _) = in_pieces(&doc, &[doc.len() / 2, doc.len() / 3]);
        proptest::prop_assert_eq!(whole, pieced);
    }
}
