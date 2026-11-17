// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use std::collections::HashSet;

use mailcraft_model::Id;
use mailcraft_model::ai::{RetrievalRequest, RetrievalResult, ScoredHit, searchable_text};

use super::corpus_tests::{Ids, corpus};
use super::lexical::ALPHA;
use super::*;
use crate::Session;

fn retrieve(s: &Session, request: &RetrievalRequest) -> RetrievalResult {
    let threads = s.threads();
    let graph = MailGraph::build(s.mailbox.messages.values().map(|m| m.as_ref()), &threads);
    let scan = SessionScan::all(s);
    let texts = SessionTexts { session: s };
    LexicalRetriever::new(&scan, &graph, &texts).retrieve(request).unwrap()
}

fn ask(s: &Session, query: &str) -> Vec<Id> {
    found(&retrieve(s, &RetrievalRequest::new(query, None, None, None, None).unwrap()).hits)
}

fn found(hits: &[ScoredHit]) -> Vec<Id> {
    hits.iter().map(|h| h.message).collect()
}

fn setup() -> (Session, Ids) {
    corpus()
}

#[test]
fn a_question_in_plain_words_finds_the_message_that_answers_it_first() {
    let (s, ids) = setup();
    let hits = ask(&s, "What deposit did Harbor Hall ask for?");
    assert_eq!(hits.first(), Some(&ids.harbor), "{hits:?}");
    // The distractors that hold one of its words come after it, if at all.
    for other in [ids.cabin, ids.lunch] {
        if let Some(at) = hits.iter().position(|h| *h == other) {
            assert!(at > 0);
        }
    }
}

#[test]
fn the_hit_carries_the_passage_that_holds_the_words() {
    let (s, ids) = setup();
    let r = retrieve(&s, &RetrievalRequest::new("deposit harbor hall", Some(1), None, None, None).unwrap());
    let hit = &r.hits[0];
    assert_eq!((hit.message, hit.chunk.message), (ids.harbor, ids.harbor));
    assert!(hit.chunk.text.contains("deposit of 500 euros"), "{:?}", hit.chunk);
    let m = s.mailbox.message(ids.harbor).unwrap();
    let text = searchable_text(&m.subject, &m.body.text);
    assert_eq!(&text[hit.chunk.start..hit.chunk.end], hit.chunk.text);
    assert_eq!(r.terms, ["deposit", "harbor", "hall"]);
}

#[test]
fn a_query_no_message_answers_has_no_hits_and_no_error() {
    let (s, _) = setup();
    assert!(ask(&s, "zzqx flurbington quixotical").is_empty());
    assert!(ask(&s, "").is_empty());
    assert!(ask(&s, "what is the?").is_empty());
}

#[test]
fn a_person_filter_keeps_only_their_messages() {
    let (s, ids) = setup();
    for from in ["ada@example.org", "Ada Byron", "ada"] {
        let r = retrieve(&s, &RetrievalRequest::new("budget deposit", None, Some(from), None, None).unwrap());
        assert!(!r.hits.is_empty(), "{from}");
        for h in &r.hits {
            assert_eq!(s.mailbox.message(h.message).unwrap().from.email, "ada@example.org", "{from}");
        }
        assert!(!found(&r.hits).contains(&ids.reply));
    }
    // Written in the query.
    let hits = ask(&s, "budget from:kit@example.org");
    assert_eq!(hits, [ids.reply]);
    // Nobody of that name: nothing.
    let r = retrieve(&s, &RetrievalRequest::new("budget", None, Some("nobody@example.org"), None, None).unwrap());
    assert!(r.hits.is_empty());
}

#[test]
fn an_attachment_name_finds_its_message() {
    let (s, ids) = setup();
    assert!(ask(&s, "floorplan").contains(&ids.harbor));
    assert!(ask(&s, "harbor-hall-floorplan.pdf").first() == Some(&ids.harbor));
    assert!(ask(&s, "q4-budget.xlsx").contains(&ids.budget));
}

#[test]
fn a_conversation_filter_keeps_its_messages() {
    let (s, ids) = setup();
    let r = retrieve(&s, &RetrievalRequest::new("budget travel", None, None, Some(ids.reply), None).unwrap());
    let hits: HashSet<Id> = found(&r.hits).into_iter().collect();
    assert!(!hits.is_empty() && hits.is_subset(&[ids.budget, ids.reply].into_iter().collect()), "{hits:?}");
    // A conversation that isn't there: nothing.
    let r = retrieve(&s, &RetrievalRequest::new("budget", None, None, Some(u64::MAX), None).unwrap());
    assert!(r.hits.is_empty());
}

#[test]
fn a_folder_filter_keeps_its_messages() {
    let (s, ids) = setup();
    let r = retrieve(&s, &RetrievalRequest::new("budget", None, None, None, Some(ids.archive)).unwrap());
    assert_eq!(found(&r.hits), [ids.archived]);
    let r = retrieve(&s, &RetrievalRequest::new("budget", None, Some("ada"), None, Some(ids.inbox)).unwrap());
    assert_eq!(found(&r.hits), [ids.budget]);
}

#[test]
fn hits_are_ranked_and_at_most_the_limit() {
    let (s, _) = setup();
    let r = retrieve(&s, &RetrievalRequest::new("budget quarter travel deposit harbor friday", Some(2), None, None, None).unwrap());
    assert!(r.hits.len() <= 2);
    for w in r.hits.windows(2) {
        assert!(w[0].score >= w[1].score);
    }
}

#[test]
fn the_subject_outweighs_the_text() {
    let (s, ids) = setup();
    // "budget" is in the subject of the review and its reply, and only in the text of the notes.
    let hits = ask(&s, "budget");
    let notes = hits.iter().position(|h| *h == ids.archived);
    let review = hits.iter().position(|h| *h == ids.budget);
    assert!(review.is_some());
    if let (Some(n), Some(r)) = (notes, review) {
        assert!(r < n, "{hits:?}");
    }
}

/// A mailbox of `strong` messages holding every word of "kestrel deposit invoice" in their
/// subject, and `weak` holding only "invoice" in their text.
fn strong_and_weak(strong: usize, weak: usize) -> (Session, Vec<Id>) {
    use mailcraft_model::{Address, Body, FolderKind, Mailbox, Message};

    let mut mb = Mailbox::with_local_account("Robin Owner", "robin@example.org");
    let account = mb.accounts[0].id;
    let inbox = mb.folder_of_kind(account, FolderKind::Inbox).or_else(|| mb.folders.first().map(|f| f.id)).unwrap();
    let mut strong_ids = Vec::new();
    for i in 0..strong.saturating_add(weak) {
        let (subject, body) = if i < strong {
            ("Kestrel deposit invoice".to_string(), "The kestrel deposit invoice is attached.".to_string())
        } else {
            (format!("Note {i}"), "Here is the invoice for the month.".to_string())
        };
        let m = Message {
            folder: inbox,
            from: Address::new("Sender", format!("sender{i}@example.org")),
            subject,
            date: 1_000 + i as i64,
            body: Body { text: body, html: None },
            ..Default::default()
        };
        let id = mb.add_message(m).unwrap();
        if i < strong {
            strong_ids.push(id);
        }
    }
    (Session::new(mb), strong_ids)
}

fn with_alpha(s: &Session, query: &str, limit: Option<usize>, alpha: f64) -> Vec<Id> {
    let threads = s.threads();
    let graph = MailGraph::build(s.mailbox.messages.values().map(|m| m.as_ref()), &threads);
    let scan = SessionScan::all(s);
    let texts = SessionTexts { session: s };
    let mut retriever = LexicalRetriever::new(&scan, &graph, &texts);
    retriever.alpha = alpha;
    let limit = limit.map(|n| u64::try_from(n).unwrap());
    found(&retriever.retrieve(&RetrievalRequest::new(query, limit, None, None, None).unwrap()).unwrap().hits)
}

#[test]
fn two_strong_hits_and_eight_weak_give_only_the_strong() {
    let (s, strong) = strong_and_weak(2, 8);
    let hits = ask(&s, "kestrel deposit invoice");
    let mut sorted = hits.clone();
    sorted.sort_unstable();
    let mut want = strong;
    want.sort_unstable();
    assert_eq!(sorted, want, "{hits:?}");
    // Across the whole range the calibration sweeps, the weak ones stay out.
    for step in 0..=12u32 {
        let alpha = (30.0 + 5.0 * f64::from(step)) / 100.0;
        assert_eq!(with_alpha(&s, "kestrel deposit invoice", None, alpha).len(), 2, "alpha {alpha}");
    }
}

#[test]
fn the_cutoff_keeps_at_least_the_best_and_at_most_the_limit() {
    let (s, strong) = strong_and_weak(1, 8);
    // Even at 1, the best is kept.
    assert_eq!(with_alpha(&s, "kestrel deposit invoice", None, 1.0), strong);
    // At 0 nothing is cut but the limit: 10 by default.
    let (s, _) = strong_and_weak(12, 0);
    assert_eq!(with_alpha(&s, "kestrel", None, 0.0).len(), 10);
    assert_eq!(ask(&s, "kestrel").len(), 10, "equal scores are all kept, up to 10");
    assert_eq!(with_alpha(&s, "kestrel", Some(3), 0.0).len(), 3);
}

#[test]
fn an_alpha_out_of_range_is_brought_within_it() {
    let (s, strong) = strong_and_weak(2, 8);
    let at = |alpha: f64| with_alpha(&s, "kestrel deposit invoice", None, alpha);
    assert_eq!(at(f64::NAN), at(ALPHA));
    assert_eq!(at(f64::INFINITY), at(ALPHA));
    assert_eq!(at(7.0).len(), strong.len(), "above 1 is 1: the strong ones tie");
    assert_eq!(at(-3.0).len(), 10, "below 0 is 0: nothing cut but the limit");
    assert!((0.30..=0.90).contains(&ALPHA));
}

/// Another index of the same messages gives the same hits (the boundary is replaceable).
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn the_word_index_and_reading_the_messages_find_the_same() {
    use crate::words::{Doc, Index};

    let (s, _) = setup();
    let mut index = Index::new(None);
    for m in s.mailbox.messages.values() {
        index.insert(Doc::of(m, &m.body.text), None);
    }
    let scan = SessionScan::all(&s);
    let parsed = terms::parse("What deposit did Harbor Hall ask for? budget floorplan from robin");
    let mut a = index.lookup(&parsed.terms);
    let mut b = scan.lookup(&parsed.terms);
    for list in a.per_term.iter_mut().chain(b.per_term.iter_mut()) {
        list.sort_unstable();
    }
    assert_eq!(a, b);

    let threads = s.threads();
    let graph = MailGraph::build(s.mailbox.messages.values().map(|m| m.as_ref()), &threads);
    let texts = SessionTexts { session: &s };
    let request = RetrievalRequest::new("What deposit did Harbor Hall ask for?", None, None, None, None).unwrap();
    let by_index = LexicalRetriever::new(&index, &graph, &texts).retrieve(&request).unwrap();
    let by_scan = LexicalRetriever::new(&scan, &graph, &texts).retrieve(&request).unwrap();
    assert_eq!(by_index, by_scan);

    // And split between the two (the index hasn't read some messages yet), the same again.
    let half: Vec<Id> = s.mailbox.messages.keys().copied().take(3).collect();
    let mut partial = Index::new(None);
    for m in s.mailbox.messages.values().filter(|m| !half.contains(&m.id)) {
        partial.insert(Doc::of(m, &m.body.text), None);
    }
    let rest = SessionScan::new(&s, half);
    let both = Both { first: &partial, then: &rest };
    assert_eq!(LexicalRetriever::new(&both, &graph, &texts).retrieve(&request).unwrap(), by_scan);
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(64))]
    #[test]
    fn any_query_and_filters_retrieve_without_panicking(query in "\\PC{0,300}", limit in 1u64..200, from in proptest::option::of("\\PC{1,40}"), thread in proptest::option::of(1u64..20), folder in proptest::option::of(1u64..20)) {
        let (s, _) = setup();
        if let Ok(request) = RetrievalRequest::new(&query, Some(limit), from.as_deref(), thread, folder) {
            let r = retrieve(&s, &request);
            proptest::prop_assert!(r.hits.len() <= request.limit);
            for h in &r.hits {
                proptest::prop_assert!(h.score.is_finite() && h.score > 0.0);
                proptest::prop_assert!(s.mailbox.message(h.message).is_some());
            }
        }
    }
}
