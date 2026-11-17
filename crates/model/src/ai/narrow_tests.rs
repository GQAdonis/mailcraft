// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::AiError;
use super::call::Role;
use super::narrow::*;

#[test]
fn only_the_owners_words_and_the_language_are_sent() {
    let m = narrow_messages("  invoices from Ada last month with a PDF ").unwrap();
    assert_eq!(m.len(), 2);
    assert_eq!(m[0].role, Role::System);
    assert!(m[0].content.contains("query language") && m[0].content.contains("has:attachment"));
    assert_eq!((m[1].role, m[1].content.as_str()), (Role::User, "invoices from Ada last month with a PDF"));
    assert!(narrow_messages("   ").is_err());
    assert!(narrow_messages(&"x".repeat(MAX_NARROW_CHARS + 1)).is_err());
    // Control characters in the description are dropped.
    assert_eq!(narrow_messages("a\u{0}b\u{202e}").unwrap()[1].content, "ab");
}

#[test]
fn a_retry_says_what_was_wrong_in_the_engines_words() {
    let why = AiError::Invalid("`sender:` isn't a keyword of the search".into());
    let m = narrow_retry_messages("mail from ada", &why).unwrap();
    assert_eq!(m.len(), 3);
    assert!(m[2].content.contains("sender:") && m[2].content.contains("JSON only"));
}

#[test]
fn the_query_is_taken_from_the_answers_json() {
    assert_eq!(proposed_query(r#"{"query": "from:ada has:attachment"}"#).unwrap(), "from:ada has:attachment");
    assert_eq!(proposed_query("```json\n{\"query\": \" budget \"}\n```").unwrap(), "budget");
    for bad in ["", "from:ada", "{", "}{", r#"{"query": 5}"#, r#"{"q": "x"}"#, r#"["query"]"#, "{\"query\": \"x\"", "{not json}"] {
        assert!(proposed_query(bad).is_err(), "{bad}");
    }
}

#[test]
fn a_search_in_the_language_is_taken() {
    for ok in [
        "budget",
        "from:ada has:attachment",
        "invoice from:ada received:\"last month\" attachment:pdf",
        "-is:read after:2026-10-01 before:2026-11-01",
        "subject:\"budget review\" importance:high",
        "received:>=2026-10-01 meeting at 10:30",
        "from:ada@example.org",
    ] {
        assert_eq!(check_query(ok), Ok(()), "{ok}");
    }
}

#[test]
fn a_search_outside_the_language_is_refused() {
    for bad in ["", "   ", "sender:ada", "is:urgent", "before:yesterday", "has:pictures", "budget\nfrom:ada", "-", "@@ --"] {
        assert!(matches!(check_query(bad), Err(AiError::Invalid(_))), "{bad}");
    }
    assert!(check_query(&"a ".repeat(MAX_PROPOSAL_CHARS)).is_err());
}

proptest::proptest! {
    #[test]
    fn any_answer_is_read_without_panicking(answer in "\\PC{0,400}") {
        if let Ok(q) = proposed_query(&answer) {
            let _ = check_query(&q);
        }
        let _ = check_query(&answer);
    }
}
