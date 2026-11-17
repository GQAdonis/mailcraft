// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::AiError;
use super::citation::*;

fn c(message: u64, start: usize, end: usize) -> Citation {
    Citation { message, start, end }
}

#[test]
fn citations_of_messages_given_are_kept_in_order_once() {
    let cited = [c(12, 0, 40), Citation::message(7), c(12, 0, 40), c(12, 50, 90)];
    assert_eq!(validate_citations(&cited, &[7, 12, 30]), Ok(vec![c(12, 0, 40), c(7, 0, 0), c(12, 50, 90)]));
    assert_eq!(validate_citations(&[], &[7]), Ok(vec![]));
}

#[test]
fn a_message_that_was_not_given_is_refused() {
    assert_eq!(validate_citations(&[Citation::message(13)], &[12]), Err(AiError::UnknownCitation(13)));
    // Among good ones, it still refuses the whole answer's citations.
    assert_eq!(validate_citations(&[Citation::message(12), Citation::message(u64::MAX)], &[12]), Err(AiError::UnknownCitation(u64::MAX)));
    // No message is id 0, even if a caller "gave" it.
    assert_eq!(validate_citations(&[Citation::message(0)], &[0]), Err(AiError::UnknownCitation(0)));
    // Nothing given: nothing may be cited.
    assert_eq!(validate_citations(&[Citation::message(1)], &[]), Err(AiError::UnknownCitation(1)));
}

#[test]
fn a_span_that_runs_backwards_or_past_any_text_is_refused() {
    assert!(matches!(validate_citations(&[c(1, 10, 5)], &[1]), Err(AiError::Invalid(_))));
    assert!(matches!(validate_citations(&[c(1, 0, usize::MAX)], &[1]), Err(AiError::Invalid(_))));
}

#[test]
fn more_than_the_cap_is_refused() {
    let many: Vec<Citation> = (0..=MAX_CITATIONS).map(|i| c(1, i, i + 1)).collect();
    assert_eq!(validate_citations(&many, &[1]), Err(AiError::TooMany { what: "citations", max: MAX_CITATIONS }));
    assert_eq!(validate_citations(&many[..MAX_CITATIONS], &[1]).map(|v| v.len()), Ok(MAX_CITATIONS));
}

proptest::proptest! {
    #[test]
    fn whatever_is_cited_only_given_messages_come_back(
        cited in proptest::collection::vec((proptest::prelude::any::<u64>(), 0usize..2000, 0usize..2000), 0..40),
        given in proptest::collection::vec(proptest::prelude::any::<u64>(), 0..20),
    ) {
        let cited: Vec<Citation> = cited.into_iter().map(|(m, a, b)| c(m, a.min(b), a.max(b))).collect();
        if let Ok(kept) = validate_citations(&cited, &given) {
            proptest::prop_assert!(kept.len() <= MAX_CITATIONS);
            for k in &kept {
                proptest::prop_assert!(given.contains(&k.message) && k.message != 0);
            }
        } else {
            // Refused only when one names a message not given, or there are too many.
            let unknown = cited.iter().any(|k| k.message == 0 || !given.contains(&k.message));
            let distinct: std::collections::HashSet<&Citation> = cited.iter().collect();
            proptest::prop_assert!(unknown || distinct.len() > MAX_CITATIONS);
        }
    }
}
