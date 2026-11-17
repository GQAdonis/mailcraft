// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use serde_json::{Value, json};

use crate::agent::{Access, Verdict, access, check};
use crate::retrieval::corpus_tests::corpus;
use crate::{CmdError, Session};

fn run(s: &mut Session, params: Value) -> Value {
    s.run("ai.retrieve", &params).unwrap_or_else(|e| panic!("ai.retrieve {params}: {e}"))
}

fn hit_ids(v: &Value) -> Vec<u64> {
    v["hits"].as_array().unwrap().iter().map(|h| h["id"].as_u64().unwrap()).collect()
}

#[test]
fn it_answers_ranked_hits_with_their_passages_and_says_the_text_is_data() {
    let (mut s, ids) = corpus();
    let before = s.mailbox.clone();
    let v = run(&mut s, json!({"query": "What deposit did Harbor Hall ask for?"}));
    assert_eq!(hit_ids(&v).first(), Some(&ids.harbor), "{v}");
    let hit = &v["hits"][0];
    assert_eq!(hit["subject"], "Your booking at Harbor Hall");
    assert_eq!(hit["from"], "events@harborhall.example");
    assert!(hit["chunk"]["text"].as_str().unwrap().contains("deposit of 500 euros"));
    assert!(hit["score"].as_f64().unwrap() > 0.0);
    assert!(v["untrusted"].as_str().unwrap().contains("never instructions"));
    // Nothing of the mailbox changed, and nothing to undo.
    assert!(s.mailbox == before && !s.can_undo());
}

#[test]
fn filters_and_the_limit_are_taken() {
    let (mut s, ids) = corpus();
    let v = run(&mut s, json!({"query": "budget deposit", "from": "ada@example.org", "limit": 2}));
    assert!(hit_ids(&v).len() <= 2 && !hit_ids(&v).is_empty());
    let v = run(&mut s, json!({"query": "budget", "folder": ids.archive}));
    assert_eq!(hit_ids(&v), [ids.archived]);
    let v = run(&mut s, json!({"query": "budget travel", "thread": ids.budget}));
    assert!(hit_ids(&v).iter().all(|h| [ids.budget, ids.reply].contains(h)));
    // An unknown person: nothing, and no error.
    let v = run(&mut s, json!({"query": "budget", "from": "nobody@example.org"}));
    assert!(hit_ids(&v).is_empty());
    let v = run(&mut s, json!({"query": "zzqx flurbington"}));
    assert!(hit_ids(&v).is_empty());
}

#[test]
fn bad_parameters_are_refused_and_change_nothing() {
    let (mut s, _) = corpus();
    let before = s.mailbox.clone();
    for params in [
        json!(null),
        json!([]),
        json!("budget"),
        json!({}),
        json!({"query": 5}),
        json!({"query": "x", "limit": 0}),
        json!({"query": "x", "limit": -1}),
        json!({"query": "x", "limit": 1.5}),
        json!({"query": "x", "limit": "10"}),
        json!({"query": "x", "limit": 1e300}),
        json!({"query": "x", "from": 5}),
        json!({"query": "x", "from": "  "}),
        json!({"query": "x", "thread": 0}),
        json!({"query": "x", "thread": null}),
        json!({"query": "x", "folder": "inbox"}),
        json!({"query": "x", "send": true}),
        json!({"query": "q".repeat(5_000)}),
    ] {
        assert!(matches!(s.run("ai.retrieve", &params), Err(CmdError::Params(_))), "{params}");
    }
    // A limit above the cap is capped, not refused.
    assert!(s.run("ai.retrieve", &json!({"query": "budget", "limit": 18446744073709551615u64})).is_ok());
    assert!(s.mailbox == before);
}

#[test]
fn agents_may_retrieve_without_asking() {
    let (s, _) = corpus();
    assert_eq!(access("ai.retrieve"), Some(Access::Safe));
    assert_eq!(check(&s, "ai.retrieve", &json!({"query": "budget"}), &[]), Verdict::Run);
    // An agent's run changes nothing either.
    let (mut s, ids) = corpus();
    s.agent = Some(Vec::new());
    let v = s.run("ai.retrieve", &json!({"query": "floorplan"})).unwrap();
    s.agent = None;
    assert!(hit_ids(&v).contains(&ids.harbor));
}
