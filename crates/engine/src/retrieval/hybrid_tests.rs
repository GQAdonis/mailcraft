// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use mailcraft_model::Id;
use mailcraft_model::ai::{AiError, RetrievalRequest, RetrievalResult};

use super::corpus_tests::{Ids, corpus};
use super::*;
use crate::Session;

/// A store that finds what it was told to, nearest first, whatever the question.
struct Fake(Vec<(Id, u16)>);

impl VectorStore for Fake {
    fn dims(&self) -> usize {
        2
    }

    fn nearest(&self, _query: &[f32], k: usize) -> Result<Vec<VectorHit>, AiError> {
        Ok(self
            .0
            .iter()
            .take(k)
            .enumerate()
            .map(|(i, (message, chunk))| VectorHit { message: *message, chunk: *chunk, similarity: 1.0 - i as f32 / 100.0 })
            .collect())
    }
}

fn hybrid(s: &Session, store: &Fake, request: &RetrievalRequest) -> RetrievalResult {
    let threads = s.threads();
    let graph = MailGraph::build(s.mailbox.messages.values().map(|m| m.as_ref()), &threads);
    let scan = SessionScan::all(s);
    let texts = SessionTexts { session: s };
    let lexical = LexicalRetriever::new(&scan, &graph, &texts);
    HybridRetriever { lexical, vectors: store, texts: &texts, query: &[1.0, 0.0] }.retrieve(request).unwrap()
}

fn ask(query: &str) -> RetrievalRequest {
    RetrievalRequest::new(query, None, None, None, None).unwrap()
}

fn setup() -> (Session, Ids) {
    corpus()
}

#[test]
fn a_message_no_word_of_the_question_finds_is_found_by_meaning() {
    let (s, ids) = setup();
    // "lodging" is in no message; the store says the cabin is nearest.
    let r = hybrid(&s, &Fake(vec![(ids.cabin, 0)]), &ask("lodging refund"));
    assert_eq!(r.hits.first().map(|h| h.message), Some(ids.cabin));
    assert!(r.hits[0].chunk.text.contains("cabin deposit"));
}

#[test]
fn both_rankings_are_fused_and_what_both_find_comes_first() {
    let (s, ids) = setup();
    let r = hybrid(&s, &Fake(vec![(ids.lunch, 0), (ids.harbor, 0)]), &ask("What deposit did Harbor Hall ask for?"));
    let found: Vec<Id> = r.hits.iter().map(|h| h.message).collect();
    assert_eq!(found.first(), Some(&ids.harbor), "{found:?}");
    assert!(found.contains(&ids.lunch));
    for w in r.hits.windows(2) {
        assert!(w[0].score >= w[1].score);
    }
}

#[test]
fn filters_hold_for_the_nearest_too_and_gone_messages_are_left_out() {
    let (s, ids) = setup();
    let request = RetrievalRequest::new("budget", None, Some("ada"), None, None).unwrap();
    let r = hybrid(&s, &Fake(vec![(ids.lunch, 0), (u64::MAX, 0), (ids.cabin, 0)]), &request);
    let found: Vec<Id> = r.hits.iter().map(|h| h.message).collect();
    assert!(!found.contains(&ids.lunch) && !found.contains(&u64::MAX), "{found:?}");
    assert!(found.contains(&ids.cabin) && found.contains(&ids.budget));
    // Nobody of that name: nothing, whatever the store says.
    let nobody = RetrievalRequest::new("budget", None, Some("nobody@example.org"), None, None).unwrap();
    assert!(hybrid(&s, &Fake(vec![(ids.cabin, 0)]), &nobody).hits.is_empty());
}

#[test]
fn hits_are_at_most_the_limit() {
    let (s, ids) = setup();
    let all = vec![(ids.harbor, 0), (ids.lunch, 0), (ids.budget, 0), (ids.reply, 0), (ids.cabin, 0), (ids.news, 0)];
    let request = RetrievalRequest::new("budget", Some(2), None, None, None).unwrap();
    assert_eq!(hybrid(&s, &Fake(all), &request).hits.len(), 2);
}
