// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use mailcraft_model::words::{ATTACHMENT, BODY, CC, FROM, SUBJECT, TO};

use super::TermHits;
use super::scorer::*;

#[test]
fn parts_weigh_subject_then_people_then_text_then_attachment_names() {
    let w = Weights::default();
    assert!(w.of(SUBJECT) > w.of(FROM) && w.of(FROM) > w.of(BODY) && w.of(BODY) > w.of(ATTACHMENT) && w.of(ATTACHMENT) > 0.0);
    assert_eq!(w.of(TO), w.of(FROM));
    assert_eq!(w.of(CC), w.of(FROM));
    // The best part counts, not their sum.
    assert_eq!(w.of(SUBJECT | BODY | ATTACHMENT), w.of(SUBJECT));
    assert_eq!(w.of(0), 0.0);
}

#[test]
fn a_rare_word_weighs_more_than_a_common_one_and_never_nothing() {
    assert!(idf(1_000, 1) > idf(1_000, 500));
    assert!(idf(1_000, 1_000) > 0.0);
    // More holders than messages (a caller's mistake) stays finite and above 0.
    assert!(idf(1, 50).is_finite() && idf(1, 50) > 0.0);
    assert!(idf(0, 0).is_finite());
}

#[test]
fn each_word_counts_once_a_message_in_its_best_part() {
    // Word 0 in message 1's subject and (again, as another entry) its text; word 1 in message 2.
    let hits = TermHits { docs: 10, per_term: vec![vec![(1, BODY), (1, SUBJECT)], vec![(2, BODY)]] };
    let s = score(&hits, &Weights::default());
    let w0 = idf(10, 1) * 3.0;
    let w1 = idf(10, 1) * 1.0;
    assert!((s[&1] - w0).abs() < 1e-9, "{s:?}");
    assert!((s[&2] - w1).abs() < 1e-9, "{s:?}");
}

#[test]
fn a_message_holding_more_of_the_words_scores_higher() {
    let hits = TermHits { docs: 100, per_term: vec![vec![(1, BODY), (2, BODY)], vec![(1, BODY)], vec![(1, BODY), (3, BODY)]] };
    let s = score(&hits, &Weights::default());
    assert!(s[&1] > s[&2] && s[&1] > s[&3]);
    assert!(score(&TermHits::default(), &Weights::default()).is_empty());
}
