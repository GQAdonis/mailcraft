// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use mailcraft_model::ai::{AiKind, MAX_PASSAGES, NARROW_SCHEMA, PRODUCT_SCHEMAS, RequestState, answer_schema, portable_problems};
use serde_json::{Value, json};

use crate::retrieval::corpus_tests::corpus;
use crate::{CmdError, Session};

fn narrow(s: &mut Session, text: &str) -> u64 {
    s.run("ai.narrow", &json!({"text": text})).unwrap()["request"]["id"].as_u64().unwrap()
}

fn answer(s: &mut Session, id: u64, text: &str) {
    s.run("ai.apply", &json!({"requestId": id, "event": {"type": "done", "text": text}})).unwrap();
}

fn result(s: &mut Session, id: u64) -> Value {
    s.run("ai.narrowResult", &json!({"requestId": id})).unwrap()
}

#[test]
fn only_the_owners_words_go_never_any_mail() {
    let (mut s, _) = corpus();
    let v = s.run("ai.narrow", &json!({"text": "the invoice from the hall with a PDF"})).unwrap();
    let r = &v["request"];
    assert_eq!((r["kind"].as_str(), r["temperature"].as_f64(), r["stream"].as_bool()), (Some("narrow"), Some(0.0), Some(false)));
    assert_eq!(r["schema"], NARROW_SCHEMA);
    assert_eq!(r["sources"], json!([]));
    let sent: String = r["messages"].as_array().unwrap().iter().map(|m| m["content"].as_str().unwrap().to_string()).collect();
    assert!(sent.contains("the invoice from the hall with a PDF"));
    // Nothing of any message of the mailbox is in it.
    for m in s.mailbox.messages.values() {
        for text in [&m.subject, &m.body.text, &m.from.email] {
            assert!(text.is_empty() || !sent.contains(text.as_str()), "{text}");
        }
    }
}

#[test]
fn a_valid_proposal_is_offered_and_the_list_doesnt_change() {
    let (mut s, _) = corpus();
    let rows = s.rows().iter().map(|r| r.id).collect::<Vec<_>>();
    let search = s.view.search.clone();
    let id = narrow(&mut s, "pdfs from ada");
    answer(&mut s, id, r#"{"query": "from:ada has:attachment"}"#);
    let v = result(&mut s, id);
    assert_eq!(v, json!({"query": "from:ada has:attachment", "valid": true}));
    assert_eq!(s.view.search, search);
    assert_eq!(s.rows().iter().map(|r| r.id).collect::<Vec<_>>(), rows);
    // Accepting is the search box's own command.
    s.run("view.search", &json!({"query": v["query"]})).unwrap();
    assert_eq!(s.view.search, "from:ada has:attachment");
}

#[test]
fn an_invalid_proposal_is_asked_for_again_once_then_the_owners_words_or_refused() {
    let (mut s, _) = corpus();
    let id = narrow(&mut s, "urgent mail from ada");
    answer(&mut s, id, r#"{"query": "sender:ada is:urgent"}"#);
    let v = result(&mut s, id);
    assert_eq!(v["valid"], false);
    assert!(v["query"].is_null() && v["error"].as_str().unwrap().contains("sender:"));
    let retry = v["retry"]["id"].as_u64().unwrap();
    assert_ne!(retry, id);
    let t = s.ai.get(retry).unwrap();
    assert_eq!((t.retries, t.request.kind, t.state), (1, AiKind::Narrow, RequestState::Pending));
    let last = t.request.messages.last().unwrap().content.clone();
    assert!(last.contains("sender:") && !last.contains("is:urgent\""), "{last}");
    assert_eq!(t.request.messages[1].content, "urgent mail from ada");
    // The retry's answer is no better: the owner's own words are the search, no more retries.
    answer(&mut s, retry, "I can't do that.");
    let v = result(&mut s, retry);
    assert_eq!((v["valid"].as_bool(), v["query"].as_str()), (Some(true), Some("urgent ada")));
    assert!(v.get("retry").is_none());
    // With nothing of the owner's to search for either: refused.
    let id = narrow(&mut s, "find me all the mail");
    answer(&mut s, id, "no");
    let retry = result(&mut s, id)["retry"]["id"].as_u64().unwrap();
    answer(&mut s, retry, "I can't do that.");
    let v = result(&mut s, retry);
    assert_eq!(v["valid"], false);
    assert!(v["query"].is_null() && v.get("retry").is_none());
}

#[test]
fn a_result_needs_a_finished_search_request() {
    let (mut s, _) = corpus();
    let id = narrow(&mut s, "budget");
    // Still under way.
    assert!(matches!(s.run("ai.narrowResult", &json!({"requestId": id})), Err(CmdError::Params(_))));
    // Not a search.
    let other =
        s.run("ai.request", &json!({"kind": "answer", "messages": [{"role": "user", "content": "x"}]})).unwrap()["request"]["id"].as_u64().unwrap();
    s.run("ai.apply", &json!({"requestId": other, "event": {"type": "done", "text": "{\"query\": \"x\"}"}})).unwrap();
    assert!(matches!(s.run("ai.narrowResult", &json!({"requestId": other})), Err(CmdError::Params(_))));
    // Failed: said with its kind; called off and unknown: refused.
    s.run("ai.apply", &json!({"requestId": id, "event": {"type": "error", "kind": "unreachable", "message": "closed"}})).unwrap();
    assert_eq!(result(&mut s, id), json!({"state": "failed", "kind": "unreachable", "message": "closed"}));
    let gone = narrow(&mut s, "budget");
    s.run("ai.cancel", &json!({"requestId": gone})).unwrap();
    assert!(s.run("ai.narrowResult", &json!({"requestId": gone})).is_err());
    assert!(s.run("ai.narrowResult", &json!({"requestId": 999})).is_err());
}

#[test]
fn every_schema_the_engine_asks_for_keeps_to_the_portable_subset() {
    let (mut s, _) = corpus();
    let narrowing = s.run("ai.narrow", &json!({"text": "invoices from ada"})).unwrap();
    let answer = s.run("ai.ask", &json!({"question": "What deposit did Harbor Hall ask for?", "nonce": 1})).unwrap();
    let mut seen = 0;
    for v in [&narrowing, &answer] {
        if let Some(schema) = v["request"]["schema"].as_str() {
            assert_eq!(portable_problems(schema), Vec::<String>::new(), "{schema}");
            // An answer's schema is made for its number of messages (`answer_schema`); the one
            // listed is it for the most there can be.
            let listed = PRODUCT_SCHEMAS.iter().any(|(_, known)| *known == schema);
            let an_answers = (1..=MAX_PASSAGES).any(|n| answer_schema(n) == schema);
            assert!(listed || an_answers, "a schema sent is listed in PRODUCT_SCHEMAS or is an answer's: {schema}");
            seen += 1;
        }
    }
    assert!(seen >= 1);
}

#[test]
fn a_refused_schema_is_answered_with_its_kind_and_status_not_words() {
    let (mut s, _) = corpus();
    let id = narrow(&mut s, "mail from ada about the budget last week");
    let event = json!({
        "type": "error",
        "kind": {"refused": {"status": 400}},
        "message": "The model provider refused the request.",
        "providerText": "\"maxLength\" at #/properties/query cannot be enforced",
    });
    s.run("ai.apply", &json!({"requestId": id, "event": event})).unwrap();
    let v = result(&mut s, id);
    assert_eq!(v, json!({"state": "failed", "kind": "refused", "status": 400, "message": "The model provider refused the request."}));
    // The provider's words stay with the request, as data, for the owner's details.
    assert!(s.ai.get(id).unwrap().provider_text.contains("maxLength"));
    // A time-out says whether anything had come.
    let id = narrow(&mut s, "budget");
    let event = json!({"type": "error", "kind": {"timed_out": {"before_first_byte": true}}, "message": "slow"});
    s.run("ai.apply", &json!({"requestId": id, "event": event})).unwrap();
    assert_eq!(result(&mut s, id), json!({"state": "failed", "kind": "timed_out", "beforeFirstByte": true, "message": "slow"}));
}

#[test]
fn bad_parameters_are_refused() {
    let (mut s, _) = corpus();
    for params in
        [json!(null), json!({}), json!({"text": ""}), json!({"text": 5}), json!({"text": "x".repeat(1_001)}), json!({"text": "x", "send": true})]
    {
        assert!(matches!(s.run("ai.narrow", &params), Err(CmdError::Params(_))), "{params}");
    }
    for params in [json!(null), json!({}), json!({"requestId": 0}), json!({"requestId": "1"}), json!({"requestId": 1, "x": 1})] {
        assert!(matches!(s.run("ai.narrowResult", &params), Err(CmdError::Params(_))), "{params}");
    }
    assert!(s.ai.all().is_empty());
}
