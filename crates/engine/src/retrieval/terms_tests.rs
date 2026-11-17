// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::terms::*;

fn words_of(query: &str) -> Vec<(String, bool)> {
    parse(query).terms.into_iter().map(|t| (t.word, t.prefix)).collect()
}

#[test]
fn a_question_keeps_only_the_words_that_say_what_is_looked_for() {
    let t = words_of("What deposit did Harbor Hall ask for?");
    assert_eq!(t, [("deposit".to_string(), true), ("harbor".to_string(), true), ("hall".to_string(), true)]);
    assert!(words_of("what is the of to for").is_empty());
}

#[test]
fn endings_are_taken_off_so_other_forms_are_found() {
    assert_eq!(words_of("deposits"), [("deposit".to_string(), true)]);
    assert_eq!(words_of("booking"), [("book".to_string(), true)]);
    assert_eq!(words_of("booked"), [("book".to_string(), true)]);
    assert_eq!(words_of("policies"), [("polic".to_string(), true)]);
    // Not where the ending is the word's own.
    assert_eq!(words_of("address"), [("address".to_string(), true)]);
    assert_eq!(words_of("status"), [("status".to_string(), true)]);
}

#[test]
fn short_words_and_numbers_find_only_themselves_and_letters_alone_nothing() {
    assert_eq!(words_of("Q4 500 x"), [("q4".to_string(), false), ("500".to_string(), false)]);
    // A Chinese character alone is a word.
    assert_eq!(words_of("会"), [("会".to_string(), false)]);
}

#[test]
fn each_word_once_and_at_most_the_cap() {
    assert_eq!(words_of("budget Budget BUDGETS").len(), 1);
    let many: String = (0..100).map(|i| format!("word{i} ")).collect();
    assert_eq!(parse(&many).terms.len(), MAX_TERMS);
}

#[test]
fn a_from_in_the_query_is_read_and_left_out_of_its_words() {
    let p = parse("budget from:Ada@Example.org travel");
    assert_eq!(p.from.as_deref(), Some("ada@example.org"));
    assert_eq!(p.terms.iter().map(|t| t.word.as_str()).collect::<Vec<_>>(), ["budget", "travel"]);
    assert_eq!(parse("from: budget").from, None);
    assert_eq!(parse("FROM:\"kit\"?").from.as_deref(), Some("kit"));
}

#[test]
fn a_term_finds_its_words() {
    let t = Term { word: "book".into(), prefix: true };
    assert!(t.finds("booking") && t.finds("book") && !t.finds("ebook"));
    let exact = Term { word: "q4".into(), prefix: false };
    assert!(exact.finds("q4") && !exact.finds("q45"));
}

proptest::proptest! {
    #[test]
    fn any_query_is_read_without_panicking(query in "\\PC{0,500}") {
        let p = parse(&query);
        proptest::prop_assert!(p.terms.len() <= MAX_TERMS);
        for t in &p.terms {
            proptest::prop_assert!(!t.word.is_empty() && !is_stop_word(&t.word));
        }
    }
}
