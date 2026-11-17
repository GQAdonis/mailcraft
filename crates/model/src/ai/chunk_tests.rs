// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::chunk::*;

#[test]
fn a_short_message_is_one_passage_with_its_subject_first() {
    let text = searchable_text("  Harbor Hall booking ", "The deposit is 500 euros.");
    assert_eq!(text, "Harbor Hall booking\n\nThe deposit is 500 euros.");
    let cs = chunks(7, &text);
    assert_eq!(cs.len(), 1);
    assert_eq!((cs[0].message, cs[0].ordinal, cs[0].start, cs[0].end), (7, 0, 0, text.len()));
    assert_eq!(cs[0].text, text);
}

#[test]
fn passages_cover_the_text_in_order_at_paragraph_breaks() {
    let para = "word ".repeat(100);
    let body = (0..10).map(|_| para.trim()).collect::<Vec<_>>().join("\n\n");
    let text = searchable_text("Subject", &body);
    let cs = chunks(1, &text);
    assert!(cs.len() > 1);
    let mut at = 0;
    for (i, c) in cs.iter().enumerate() {
        assert_eq!(c.ordinal as usize, i);
        assert!(c.start >= at && c.end > c.start);
        assert_eq!(&text[c.start..c.end], c.text);
        assert!(c.text.chars().count() <= MAX_CHUNK_CHARS);
        at = c.end;
    }
    assert_eq!(at, text.len());
    // Cut after a paragraph break: every passage but the last ends with one.
    assert!(cs[..cs.len() - 1].iter().all(|c| c.text.ends_with("\n\n")), "{cs:?}");
}

#[test]
fn a_long_word_is_cut_at_the_limit_at_a_character_boundary() {
    let text = "ž".repeat(MAX_CHUNK_CHARS * 3 + 5);
    let cs = chunks(1, &text);
    assert_eq!(cs.len(), 4);
    assert_eq!(cs[0].text.chars().count(), MAX_CHUNK_CHARS);
    assert_eq!(cs.iter().map(|c| c.text.as_str()).collect::<String>(), text);
}

#[test]
fn white_space_makes_no_passage_and_nothing_is_endless() {
    assert!(chunks(1, "").is_empty());
    assert!(chunks(1, "   \n\n\t  ").is_empty());
    let huge = "a ".repeat(MAX_TEXT_BYTES);
    let cs = chunks(1, &huge);
    assert_eq!(cs.len(), MAX_CHUNKS);
}

#[test]
fn the_text_is_capped_at_a_character_boundary() {
    let body = "é".repeat(MAX_TEXT_BYTES);
    let text = searchable_text("s", &body);
    assert!(text.len() <= MAX_TEXT_BYTES);
    assert!(text.starts_with("s\n\n"));
}

proptest::proptest! {
    #[test]
    fn any_text_is_cut_without_panicking_and_spans_hold(text in "\\PC{0,3000}", bytes in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..4000)) {
        let lossy = String::from_utf8_lossy(&bytes);
        for t in [text.as_str(), &lossy] {
            let cs = chunks(3, t);
            proptest::prop_assert!(cs.len() <= MAX_CHUNKS);
            let mut last = 0;
            for c in &cs {
                proptest::prop_assert!(c.start >= last && c.end > c.start && c.end <= t.len());
                proptest::prop_assert_eq!(t.get(c.start..c.end), Some(c.text.as_str()));
                proptest::prop_assert!(c.text.chars().count() <= MAX_CHUNK_CHARS);
                last = c.end;
            }
        }
    }
}
