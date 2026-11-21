// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::AiError;
use super::narrow::{check_query, settle};
use super::narrow_cues::*;
use mailcraft_time::Date;

/// A Saturday.
fn today() -> Date {
    Date::new(2026, 11, 21).unwrap()
}

fn terms(description: &str) -> Vec<String> {
    Cues::read(description, today()).terms()
}

fn answer(query: &str) -> String {
    serde_json::json!({ "query": query }).to_string()
}

// --- senders and recipients ---

#[test]
fn a_sender_in_capitals_after_from_is_read_whole() {
    assert_eq!(terms("mail from Rosa Lind"), ["from:\"Rosa Lind\""]);
    assert_eq!(terms("messages from Tomas since 3 May"), ["from:Tomas", "received:>=2026-05-03"]);
    assert_eq!(terms("from Rosa Lind, about the budget"), ["from:\"Rosa Lind\""]);
    assert_eq!(terms("mail from bob@example.org"), ["from:bob@example.org"]);
}

#[test]
fn a_name_before_sent_is_the_sender() {
    assert_eq!(terms("anything Nora sent me with an attachment"), ["from:Nora", "has:attachment"]);
    assert_eq!(terms("what Nora Vance sent us"), ["from:\"Nora Vance\""]);
    assert_eq!(terms("letters sent by Nora"), ["from:Nora"]);
}

#[test]
fn a_name_whose_mail_is_asked_for_is_the_sender() {
    assert_eq!(terms("Dmitri's emails since June 21st"), ["from:Dmitri", "received:>=2026-06-21"]);
    assert_eq!(terms("Rosa Lind's messages"), ["from:\"Rosa Lind\""]);
    // Not a sender: a possessive before anything but mail, or in small letters.
    assert!(terms("Ada's budget report").is_empty());
    assert!(terms("the team's mail").is_empty());
}

#[test]
fn files_at_the_head_of_a_request_ask_for_attachments() {
    assert_eq!(terms("attachments Soren sent"), ["from:Soren", "has:attachment"]);
    assert_eq!(terms("files from Rosa"), ["from:Rosa", "has:attachment"]);
    assert_eq!(
        Cues::read("Oskar sent something with an attachment", today()).refine("from:oskar has:attachment something"),
        "from:Oskar has:attachment"
    );
    // Further in, the words are what the mail is about.
    assert!(terms("notes about attachments").is_empty());
    assert!(terms("budget files").is_empty());
}

#[test]
fn not_from_excludes_the_sender_and_sent_to_names_the_recipient() {
    assert_eq!(terms("messages about the offsite, not from Tom"), ["-from:Tom"]);
    assert_eq!(terms("mail I sent to Rosa"), ["to:Rosa"]);
    assert_eq!(terms("emails to bob@example.org"), ["to:bob@example.org"]);
}

#[test]
fn neg_what_isnt_plainly_a_name_is_no_sender() {
    // Lower-case words, months, days of the week, words of the asking: not names.
    for d in ["mail from home", "messages from June", "mail from Monday", "anything from the team", "from last week", "sent to me"] {
        assert!(Cues::read(d, today()).terms().iter().all(|t| !t.contains("from:") && !t.contains("to:")), "{d}: {:?}", terms(d));
    }
    // Two different senders read as none: the model's reading stands.
    assert!(terms("mail from Ada or from Bob").iter().all(|t| !t.starts_with("from:")));
}

// --- dates ---

#[test]
fn a_day_without_a_year_is_the_latest_not_after_today() {
    assert_eq!(terms("since 21 June"), ["received:>=2026-06-21"]);
    assert_eq!(terms("since June 21st"), ["received:>=2026-06-21"]);
    assert_eq!(terms("since 29th of November"), ["received:>=2025-11-29"]);
    assert_eq!(terms("since 21 June 2024"), ["received:>=2024-06-21"]);
    assert_eq!(terms("since 2026-03-01"), ["received:>=2026-03-01"]);
}

#[test]
fn each_word_before_a_date_bounds_it_its_own_way() {
    assert_eq!(terms("after 3 May"), ["after:2026-05-03"]);
    assert_eq!(terms("before 3 May"), ["before:2026-05-03"]);
    assert_eq!(terms("on 3 May"), ["on:2026-05-03"]);
    assert_eq!(terms("until 3 May"), ["received:<=2026-05-03"]);
    assert_eq!(terms("in March"), ["received:2026-03-01..2026-03-31"]);
    assert_eq!(terms("in December"), ["received:2025-12-01..2025-12-31"]);
    assert_eq!(terms("since March"), ["received:>=2026-03-01"]);
    assert_eq!(terms("between 1 May and 3 June"), ["received:2026-05-01..2026-06-03"]);
    assert_eq!(terms("from 1 May to 3 June"), ["received:2026-05-01..2026-06-03"]);
    assert_eq!(terms("in 2025"), ["received:2025-01-01..2025-12-31"]);
}

#[test]
fn days_named_in_words_are_read() {
    assert_eq!(terms("last Tuesday"), ["on:2026-11-17"]);
    // Today is a Saturday: last Saturday is a week ago, Saturday is today.
    assert_eq!(terms("last Saturday"), ["on:2026-11-14"]);
    assert_eq!(terms("on Saturday"), ["on:2026-11-21"]);
    assert_eq!(terms("since yesterday"), ["received:>=2026-11-20"]);
    assert_eq!(terms("mail from today"), ["received:>=2026-11-21"]);
    assert_eq!(terms("last week"), ["received:\"last week\""]);
    assert_eq!(terms("this month"), ["received:\"this month\""]);
    assert_eq!(terms("yesterday"), ["received:yesterday"]);
    assert_eq!(terms("last year"), ["received:2025-01-01..2025-12-31"]);
    assert_eq!(terms("in the last 7 days"), ["received:>=2026-11-14"]);
    assert_eq!(terms("past two weeks"), ["received:>=2026-11-07"]);
}

#[test]
fn neg_a_date_that_may_be_the_subject_is_left_alone() {
    for d in ["the 21 June meeting notes", "notes for May", "since then", "on the budget", "between us", "in the summer", "since 31 February"] {
        assert!(terms(d).iter().all(|t| !t.contains(':') || t.starts_with("from:")), "{d}: {:?}", terms(d));
    }
    // An end before the start is no range.
    assert!(terms("between 3 June and 1 May").is_empty());
}

// --- attachments and state ---

#[test]
fn attachments_are_read_when_the_mail_comes_with_them() {
    assert_eq!(terms("with an attachment"), ["has:attachment"]);
    assert_eq!(terms("messages that have attachments"), ["has:attachment"]);
    assert_eq!(terms("with a file attached"), ["has:attachment"]);
    assert_eq!(terms("with a PDF"), ["attachment:pdf"]);
    assert_eq!(terms("without attachments"), ["-has:attachment"]);
    assert_eq!(terms("with no attachment"), ["-has:attachment"]);
    assert_eq!(terms("unread flagged mail"), ["is:unread", "is:flagged"]);
}

#[test]
fn neg_attachments_mentioned_otherwise_are_words() {
    for d in ["about attachments", "the attachment policy", "with Ada", "with the budget", "read mail"] {
        assert!(terms(d).is_empty(), "{d}: {:?}", terms(d));
    }
    // Asked for both ways: neither.
    assert!(terms("with an attachment without attachments").is_empty());
}

// --- refining the model's search ---

#[test]
fn the_words_of_the_asking_are_dropped_from_the_models_search() {
    let c = Cues::read("notes that mention marzipan", today());
    assert_eq!(c.refine("notes that mention marzipan"), "marzipan");
    let c = Cues::read("mail from Rosa Lind", today());
    assert_eq!(c.refine("mail from:rosa lind"), "from:\"Rosa Lind\"");
    let c = Cues::read("messages from Tomas since 3 May", today());
    assert_eq!(c.refine("messages from:tomas received:\"3 May\""), "from:Tomas received:>=2026-05-03");
    // An unknown keyword of a kind a rule read gives way: `since:` was the model's date.
    assert_eq!(c.refine("from:tomas since:3-may"), "from:Tomas received:>=2026-05-03");
}

#[test]
fn neg_what_the_rules_didnt_read_stays_in_the_models_search() {
    let c = Cues::read("invoices from Rosa last month with a PDF", today());
    assert_eq!(c.refine("invoice from:rosa received:\"last month\" attachment:pdf"), "from:Rosa attachment:pdf received:\"last month\" invoice");
    let c = Cues::read("budget review", today());
    assert_eq!(c.refine("subject:\"budget review\" importance:high"), "subject:\"budget review\" importance:high");
    // A more precise attachment of the model's stays beside a plain one of the rules'.
    let c = Cues::read("from Rosa with an attachment", today());
    assert_eq!(c.refine("from:rosa attachment:pdf"), "from:Rosa has:attachment attachment:pdf");
}

#[test]
fn the_owners_words_are_the_search_when_nothing_of_the_models_is_left() {
    let c = Cues::read("notes that mention marzipan and quince", today());
    assert_eq!(c.fallback(), "marzipan quince");
    assert_eq!(Cues::read("anything Nora sent me", today()).fallback(), "from:Nora");
    assert_eq!(Cues::read("mail", today()).fallback(), "");
    // A word that bounds the mail with nothing after it to say how: not searched for.
    assert_eq!(Cues::read("urgent mail from ada", today()).fallback(), "urgent ada");
}

// --- settle: what is offered ---

#[test]
fn settle_offers_the_refined_search() {
    assert_eq!(
        settle(&answer("Nora sent me with an attachment"), "anything Nora sent me with an attachment", today(), false),
        Ok("from:Nora has:attachment".into())
    );
    assert_eq!(settle(&answer("notes that mention marzipan"), "notes that mention marzipan", today(), false), Ok("marzipan".into()));
    // A search that is all asking: the owner's own words.
    assert_eq!(settle(&answer("mail about"), "mail about the quince harvest", today(), false), Ok("quince harvest".into()));
}

#[test]
fn neg_settle_never_offers_an_unchecked_search() {
    // An unknown keyword no rule explains: refused (the engine asks again), then on the last try
    // the owner's own words.
    let r = settle(&answer("sender:ada"), "budget review", today(), false);
    assert!(matches!(r, Err(AiError::Invalid(_))), "{r:?}");
    assert_eq!(settle(&answer("sender:ada"), "budget review", today(), true), Ok("budget review".into()));
    // Nothing readable and nothing of the owner's to fall back on: refused, on every try.
    assert!(settle("no json here", "mail", today(), true).is_err());
    assert!(settle("no json here", "budget review", today(), false).is_err());
    assert_eq!(settle("no json here", "budget review", today(), true), Ok("budget review".into()));
    // Whatever is offered, the query language takes.
    for d in ["mail from Rosa Lind since 3 May", "with a PDF", "messages about \"x\" not from Tom", "a:b c:d"] {
        if let Ok(q) = settle(&answer("x"), d, today(), true) {
            assert_eq!(check_query(&q), Ok(()), "{d} -> {q}");
        }
    }
}

proptest::proptest! {
    #[test]
    fn any_description_and_answer_are_read_without_panicking(description in "\\PC{0,300}", query in "\\PC{0,200}") {
        let c = Cues::read(&description, today());
        let _ = c.refine(&query);
        let _ = c.fallback();
        if let Ok(q) = settle(&answer(&query), &description, today(), true) {
            proptest::prop_assert!(check_query(&q).is_ok());
        }
    }
}
