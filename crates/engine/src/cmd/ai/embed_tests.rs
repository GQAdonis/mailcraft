// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use serde_json::{Value, json};

use crate::agent::{Access, access};
use crate::retrieval::corpus_tests::corpus;
use crate::{CmdError, Session};

const MODEL: &str = "fake-embedder";
const DIMS: usize = 16;

/// A stand-in embeddings model: each word adds to the dimension its letters fall in.
fn embed(text: &str) -> Vec<f32> {
    let mut v = vec![0.01f32; DIMS];
    for w in mailcraft_model::words::words(text) {
        let slot = w.bytes().fold(0usize, |a, b| a.wrapping_mul(31).wrapping_add(usize::from(b))) % DIMS;
        v[slot] += 1.0;
    }
    v
}

/// One round of the host's work: ask for a batch, embed it, hand the vectors back. The batch.
fn round(s: &mut Session) -> Value {
    let batch = s.run("ai.embedRequest", &json!({"model": MODEL, "max": 64})).unwrap();
    if let Some(id) = batch["batch"].as_u64() {
        let vectors: Vec<Vec<f32>> = batch["items"].as_array().unwrap().iter().map(|i| embed(i["text"].as_str().unwrap())).collect();
        let v = s.run("ai.embedApply", &json!({"batch": id, "model": MODEL, "vectors": vectors})).unwrap();
        assert_eq!(v["stored"], batch["items"].as_array().unwrap().len());
    }
    batch
}

#[test]
fn batches_carry_passages_their_messages_and_accounts_then_run_out() {
    let (mut s, _) = corpus();
    let first = round(&mut s);
    let items = first["items"].as_array().unwrap();
    assert!(!items.is_empty());
    assert!(items.iter().all(|i| i["message"].as_u64().is_some() && i["text"].as_str().is_some()));
    assert_eq!(first["accounts"], json!(["robin@example.org"]));
    assert_eq!(first["model"], MODEL);
    // All embedded: nothing more is asked for.
    let next = round(&mut s);
    assert_eq!((next["batch"].clone(), next["remaining"].clone()), (Value::Null, json!(0)));
    assert!(!s.mailbox.messages.is_empty());
}

#[test]
fn only_the_accounts_the_host_names_are_carried() {
    let (mut s, _) = corpus();
    let v = s.run("ai.embedRequest", &json!({"model": MODEL, "accounts": ["other@example.org"]})).unwrap();
    assert_eq!((v["batch"].clone(), v["items"].clone(), v["remaining"].clone()), (Value::Null, json!([]), json!(0)));
    let v = s.run("ai.embedRequest", &json!({"model": MODEL, "accounts": ["ROBIN@example.org"]})).unwrap();
    assert!(v["batch"].as_u64().is_some());
}

#[test]
fn what_the_sender_hid_and_attachments_are_never_carried() {
    let (mut s, ids) = corpus();
    let mut m = s.mailbox.message(ids.harbor).unwrap().clone();
    m.hidden_text = "SECRET-PREHEADER".into();
    s.mailbox.messages.insert(ids.harbor, std::sync::Arc::new(m));
    let v = s.run("ai.embedRequest", &json!({"model": MODEL, "max": 64})).unwrap();
    let sent = v["items"].to_string();
    assert!(!sent.contains("SECRET-PREHEADER") && !sent.contains("floorplan.pdf"));
}

#[test]
fn retrieval_is_lexical_without_a_question_vector_and_says_so() {
    let (mut s, _) = corpus();
    round(&mut s);
    let lexical = s.run("ai.retrieve", &json!({"query": "What deposit did Harbor Hall ask for?"})).unwrap();
    assert_eq!(lexical["semantic"], false);
    assert!(lexical["notice"].as_str().unwrap().contains("Semantic matching is off"));
    // Another model's question: still lexical, and the same hits.
    let other = s
        .run("ai.retrieve", &json!({"query": "What deposit did Harbor Hall ask for?", "queryVector": embed("deposit"), "embedder": "another"}))
        .unwrap();
    assert_eq!((other["semantic"].clone(), other["hits"].clone()), (json!(false), lexical["hits"].clone()));
    // The same model's: fused.
    let q = "What deposit did Harbor Hall ask for?";
    let hybrid = s.run("ai.retrieve", &json!({"query": q, "queryVector": embed(q), "embedder": MODEL})).unwrap();
    assert_eq!(hybrid["semantic"], true);
    assert!(hybrid.get("notice").is_none());
    let lexical_ids: Vec<u64> = lexical["hits"].as_array().unwrap().iter().map(|h| h["id"].as_u64().unwrap()).collect();
    let hybrid_ids: Vec<u64> = hybrid["hits"].as_array().unwrap().iter().map(|h| h["id"].as_u64().unwrap()).collect();
    assert!(lexical_ids.iter().all(|id| hybrid_ids.contains(id)), "{lexical_ids:?} {hybrid_ids:?}");
}

#[test]
fn a_message_changed_since_is_embedded_again_and_one_gone_goes() {
    let (mut s, ids) = corpus();
    round(&mut s);
    let mut m = s.mailbox.message(ids.cabin).unwrap().clone();
    m.subject = "The cabin, again".into();
    s.mailbox.messages.insert(ids.cabin, std::sync::Arc::new(m));
    s.mailbox.messages.remove(&ids.news);
    let v = s.run("ai.embedRequest", &json!({"model": MODEL})).unwrap();
    let asked: Vec<u64> = v["items"].as_array().unwrap().iter().map(|i| i["message"].as_u64().unwrap()).collect();
    assert!(!asked.is_empty() && asked.iter().all(|id| *id == ids.cabin), "{asked:?}");
}

#[test]
fn vectors_that_dont_fit_the_batch_are_refused_and_change_nothing() {
    let (mut s, _) = corpus();
    let batch = s.run("ai.embedRequest", &json!({"model": MODEL, "max": 2})).unwrap();
    let id = batch["batch"].as_u64().unwrap();
    let n = batch["items"].as_array().unwrap().len();
    let good: Vec<Vec<f32>> = (0..n).map(|_| embed("x")).collect();
    for params in [
        json!(null),
        json!({}),
        json!({"batch": id, "model": MODEL}),
        json!({"batch": id, "model": "another", "vectors": good}),
        json!({"batch": id + 1, "model": MODEL, "vectors": good}),
        json!({"batch": id, "model": MODEL, "vectors": vec![vec![1.0f32]; n + 1]}),
        json!({"batch": id, "model": MODEL, "vectors": (0..n).map(|_| json!(["x"])).collect::<Vec<_>>()}),
        json!({"batch": id, "model": MODEL, "vectors": (0..n).map(|i| vec![1.0; i + 1]).collect::<Vec<_>>()}),
        json!({"batch": id, "model": MODEL, "vectors": (0..n).map(|_| vec![0.0; 5_000]).collect::<Vec<_>>()}),
        json!({"batch": id, "model": MODEL, "vectors": good, "send": true}),
    ] {
        assert!(matches!(s.run("ai.embedApply", &params), Err(CmdError::Params(_))), "{params}");
    }
    // Still waiting, and taken once.
    assert!(s.run("ai.embedApply", &json!({"batch": id, "model": MODEL, "vectors": good})).is_ok());
    assert!(s.run("ai.embedApply", &json!({"batch": id, "model": MODEL, "vectors": good})).is_err());
}

#[test]
fn bad_requests_are_refused() {
    let (mut s, _) = corpus();
    for params in [
        json!(null),
        json!({}),
        json!({"model": ""}),
        json!({"model": "x".repeat(300)}),
        json!({"model": MODEL, "max": 0}),
        json!({"model": MODEL, "max": 65}),
        json!({"model": MODEL, "accounts": "robin@example.org"}),
        json!({"model": MODEL, "accounts": [5]}),
        json!({"model": MODEL, "send": true}),
    ] {
        assert!(matches!(s.run("ai.embedRequest", &params), Err(CmdError::Params(_))), "{params}");
    }
    for params in [
        json!({"query": "x", "queryVector": [1.0]}),
        json!({"query": "x", "embedder": MODEL}),
        json!({"query": "x", "queryVector": [], "embedder": MODEL}),
        json!({"query": "x", "queryVector": ["a"], "embedder": MODEL}),
        json!({"query": "x", "queryVector": [1.0], "embedder": 5}),
        json!({"query": "x", "queryVector": vec![1.0; 5_000], "embedder": MODEL}),
    ] {
        assert!(matches!(s.run("ai.retrieve", &params), Err(CmdError::Params(_))), "{params}");
    }
}

#[test]
fn embedding_is_never_an_agents() {
    for id in ["ai.embedRequest", "ai.embedApply"] {
        assert_eq!(access(id), Some(Access::Never), "{id}");
    }
}
