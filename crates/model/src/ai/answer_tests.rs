// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::answer::*;
use super::call::MAX_RESPONSE_BYTES;
use super::prompt::MAX_PASSAGES;
use super::schema::{PRODUCT_SCHEMAS, portable_problems};

#[test]
fn the_answer_schema_keeps_to_the_subset_for_every_number_of_messages_and_requires_no_citation() {
    assert_eq!(MAX_PASSAGES, 20, "ANSWER_SCHEMA is written for 20 messages");
    assert_eq!(answer_schema(MAX_PASSAGES), ANSWER_SCHEMA);
    assert!(PRODUCT_SCHEMAS.iter().any(|(what, s)| *what == "answer" && *s == ANSWER_SCHEMA));
    for n in 0..=MAX_PASSAGES + 5 {
        let schema = answer_schema(n);
        assert_eq!(portable_problems(&schema), Vec::<String>::new(), "{n}");
        assert!(!schema.contains("minItems"), "no citation is required");
        let v: serde_json::Value = serde_json::from_str(&schema).unwrap();
        let items = &v["properties"]["citations"]["items"];
        assert_eq!((items["type"].as_str(), items["minimum"].as_u64()), (Some("integer"), Some(1)));
        assert_eq!(items["maximum"].as_u64(), Some(n.clamp(1, MAX_PASSAGES) as u64));
        assert_eq!(v["properties"]["citations"]["maxItems"].as_u64(), Some(MAX_ANSWER_CITATIONS as u64));
        assert_eq!(v["required"], serde_json::json!(["answer", "citations"]));
        assert_eq!(v["additionalProperties"], false);
    }
}

#[test]
fn an_answer_is_read_checked_and_capped() {
    let a = read_answer_json(r#" {"answer": "The deposit is 500 euros [2].", "citations": [2]} "#, 3).unwrap();
    assert_eq!((a.answer.as_str(), a.citations.as_slice(), a.dropped, a.truncated), ("The deposit is 500 euros [2].", &[2usize][..], 0, false));
    // Not found: no citation, none invented.
    let a = read_answer_json(r#"{"answer": "The messages don't say.", "citations": []}"#, 3).unwrap();
    assert!(a.citations.is_empty() && a.dropped == 0 && is_not_found(&a.answer));
    // Numbers that aren't messages given, repeats, more than three, floats, strings.
    let a = read_answer_json(r#"{"answer": "x", "citations": [4, 0, -1, 1.0, "1", 1, 1, 2, 3, 2, 18446744073709551615]}"#, 3).unwrap();
    assert_eq!((a.citations, a.dropped), (vec![1, 2, 3], 6));
    // A missing or null list names no message; a huge one is looked at only so far.
    assert!(read_answer_json(r#"{"answer": "x"}"#, 3).unwrap().citations.is_empty());
    assert!(read_answer_json(r#"{"answer": "x", "citations": null}"#, 3).unwrap().citations.is_empty());
    let many = format!(r#"{{"answer": "x", "citations": [{}]}}"#, vec!["1"; 1_000].join(","));
    assert_eq!(read_answer_json(&many, 3).unwrap().citations, [1]);
    // Control characters and direction marks dropped, length capped.
    let long = format!(r#"{{"answer": "a\u0000\u202eb{}", "citations": []}}"#, "x".repeat(MAX_RESPONSE_BYTES));
    let a = read_answer_json(&long, 3).unwrap();
    assert!(a.answer.starts_with("ab") && a.answer.len() <= MAX_RESPONSE_BYTES && a.truncated);
    // What isn't such an answer is refused.
    for bad in [
        "",
        "{",
        "[]",
        "\"x\"",
        r#"{"answer": 5}"#,
        r#"{"citations": [1]}"#,
        r#"{"answer": "x", "citations": 1}"#,
        r#"{"answer": "x", "citations": {"1": 1}}"#,
    ] {
        assert!(read_answer_json(bad, 3).is_err(), "{bad}");
    }
    assert!(read_answer_json(&" ".repeat(MAX_ANSWER_JSON_BYTES + 1), 3).is_err());
    let deep = format!("{}{}", "[".repeat(10_000), "]".repeat(10_000));
    assert!(read_answer_json(&deep, 3).is_err());
}

#[test]
fn the_not_found_sentence_is_known_as_models_write_it() {
    assert!(is_not_found(NOT_FOUND));
    for yes in ["the messages don't say", "  The messages don\u{2019}t say.", "THE MESSAGES DO NOT SAY.", "The messages don't say who signed it."] {
        assert!(is_not_found(yes), "{yes}");
    }
    for no in ["", "The deposit is 500 euros.", "The messages say 500 euros.", "Not found."] {
        assert!(!is_not_found(no), "{no}");
    }
}

#[test]
fn debug_of_an_answer_shows_lengths_never_its_text() {
    let a = read_answer_json(r#"{"answer": "SECRET-ANSWER", "citations": [1]}"#, 1).unwrap();
    let shown = format!("{a:?}");
    assert!(!shown.contains("SECRET") && shown.contains("answer_bytes"), "{shown}");
}

proptest::proptest! {
    #[test]
    fn any_text_is_read_or_refused_within_the_bounds(json in "\\PC{0,600}", sources in 0usize..30) {
        if let Ok(a) = read_answer_json(&json, sources) {
            proptest::prop_assert!(a.citations.len() <= MAX_ANSWER_CITATIONS);
            proptest::prop_assert!(a.citations.iter().all(|n| (1..=sources).contains(n)));
            proptest::prop_assert!(a.answer.len() <= MAX_RESPONSE_BYTES);
        }
    }

    #[test]
    fn any_answer_written_as_json_is_read_back(answer in "\\PC{0,300}", cited in proptest::collection::vec(1usize..=5, 0..3)) {
        let json = serde_json::json!({"answer": answer, "citations": cited}).to_string();
        let a = read_answer_json(&json, 5).unwrap();
        proptest::prop_assert_eq!(&a.answer, &super::call::clean_text(&answer, MAX_RESPONSE_BYTES).0);
        let mut want: Vec<usize> = Vec::new();
        for n in cited {
            if !want.contains(&n) {
                want.push(n);
            }
        }
        proptest::prop_assert_eq!(a.citations, want);
    }
}
