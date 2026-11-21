// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::AiError;
use super::call::Role;
use super::narrow::*;
use mailcraft_time::Date;

fn today() -> Date {
    Date::new(2026, 11, 21).unwrap()
}

#[test]
fn only_the_owners_words_and_the_language_are_sent() {
    let m = narrow_messages("  invoices from Ada last month with a PDF ", today()).unwrap();
    assert_eq!(m.len(), 2);
    assert_eq!(m[0].role, Role::System);
    assert!(m[0].content.contains("query language") && m[0].content.contains("has:attachment"));
    assert_eq!((m[1].role, m[1].content.as_str()), (Role::User, "invoices from Ada last month with a PDF"));
    assert!(narrow_messages("   ", today()).is_err());
    assert!(narrow_messages(&"x".repeat(MAX_NARROW_CHARS + 1), today()).is_err());
    // Control characters in the description are dropped.
    assert_eq!(narrow_messages("a\u{0}b\u{202e}", today()).unwrap()[1].content, "ab");
}

/// SCN-PRIOR-SEARCH-001 diagnostic: "since 29 September" was read as 2025 because the model
/// wasn't told the date. The date goes in the instructions; the owner's words stay alone in theirs.
#[test]
fn todays_date_is_in_the_instructions_and_not_in_the_owners_words() {
    let m = narrow_messages("mail from ada since 29 September", today()).unwrap();
    assert!(m[0].content.contains("Today is 2026-11-21."), "{}", m[0].content);
    assert!(m[0].content.contains("without a year"));
    assert_eq!(m[1].content, "mail from ada since 29 September");
    let why = AiError::Invalid("x".into());
    let r = narrow_retry_messages("mail from ada", today(), &why).unwrap();
    assert!(r[0].content.contains("Today is 2026-11-21."));
    assert_eq!(r[1].content, "mail from ada");
}

/// A negative control: another day gives another date, and never a fixed one.
#[test]
fn neg_date_follows_the_day_given() {
    let m = narrow_messages("mail from ada", Date::new(2027, 1, 2).unwrap()).unwrap();
    assert!(m[0].content.contains("Today is 2027-01-02.") && !m[0].content.contains("2026-11-21"));
}

#[test]
fn a_retry_says_what_was_wrong_in_the_engines_words() {
    let why = AiError::Invalid("`sender:` isn't a keyword of the search".into());
    let m = narrow_retry_messages("mail from ada", today(), &why).unwrap();
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
