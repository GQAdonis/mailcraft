// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use mailcraft_model::ai::{ErrorKind, RequestState, Requests};
use serde_json::{Value, json};

use super::requests::{apply, cancel, request};
use crate::CmdError;
use crate::agent::{Access, access};

fn ask() -> Value {
    json!({
        "kind": "answer",
        "messages": [{"role": "system", "content": "Answer from the messages given."}, {"role": "user", "content": "What deposit did Harbor Hall ask for?"}],
        "sources": [3, 4],
        "stream": true,
        "temperature": 0.2,
    })
}

#[test]
fn a_request_is_recorded_and_answered_numbered_for_the_host() {
    let mut reqs = Requests::default();
    let v = request(&mut reqs, &ask()).unwrap();
    let r = &v["request"];
    assert_eq!((r["id"].as_u64(), r["kind"].as_str(), r["stream"].as_bool()), (Some(1), Some("answer"), Some(true)));
    assert_eq!(r["sources"], json!([3, 4]));
    assert_eq!(r["messages"][1]["role"], "user");
    assert_eq!(reqs.get(1).unwrap().state, RequestState::Pending);
    let v = request(&mut reqs, &json!({"kind": "narrow", "messages": [{"role": "user", "content": "mail from ada last week"}]})).unwrap();
    assert_eq!(v["request"]["id"], 2);
}

#[test]
fn what_comes_back_is_applied_piece_by_piece_then_ends() {
    let mut reqs = Requests::default();
    request(&mut reqs, &ask()).unwrap();
    assert_eq!(apply(&mut reqs, &json!({"requestId": 1, "event": {"type": "delta", "text": "The deposit "}})).unwrap()["state"], "streaming");
    assert_eq!(apply(&mut reqs, &json!({"requestId": 1, "event": {"type": "delta", "text": "is 500 euros."}})).unwrap()["state"], "streaming");
    assert_eq!(reqs.get(1).unwrap().text, "The deposit is 500 euros.");
    assert_eq!(apply(&mut reqs, &json!({"requestId": 1, "event": {"type": "done"}})).unwrap()["state"], "done");
    // Over: refused, and the answer stays.
    assert!(matches!(apply(&mut reqs, &json!({"requestId": 1, "event": {"type": "delta", "text": "x"}})), Err(CmdError::Params(_))));
    assert_eq!(reqs.get(1).unwrap().text, "The deposit is 500 euros.");
}

#[test]
fn an_error_from_the_host_fails_the_request() {
    let mut reqs = Requests::default();
    request(&mut reqs, &ask()).unwrap();
    let event = json!({"type": "error", "kind": "unreachable", "message": "The model server at 127.0.0.1:11437 didn't answer."});
    let v = apply(&mut reqs, &json!({"requestId": 1, "event": event})).unwrap();
    assert_eq!(v["state"], "failed");
    let t = reqs.get(1).unwrap();
    assert!(t.error.contains("didn't answer"));
    assert_eq!(t.error_kind, Some(ErrorKind::Unreachable));
}

#[test]
fn an_error_without_a_kind_or_with_a_status_that_is_none_is_refused() {
    let mut reqs = Requests::default();
    request(&mut reqs, &ask()).unwrap();
    for event in [
        json!({"type": "error", "message": "no kind"}),
        json!({"type": "error", "kind": "on_fire", "message": "m"}),
        json!({"type": "error", "kind": {"refused": {"status": 0}}, "message": "m"}),
        json!({"type": "error", "kind": {"refused": {"status": 600}}, "message": "m"}),
        json!({"type": "error", "kind": {"refused": {"status": 65536}}, "message": "m"}),
        json!({"type": "error", "kind": "unreachable", "message": "m", "providerText": [1]}),
    ] {
        assert!(matches!(apply(&mut reqs, &json!({"requestId": 1, "event": event})), Err(CmdError::Params(_))), "{event}");
    }
    assert_eq!(reqs.get(1).unwrap().state, RequestState::Pending, "a refused event changes nothing");
    let event = json!({"type": "error", "kind": {"refused": {"status": 400}}, "message": "m", "providerText": "\u{0}bad\r\nschema"});
    assert_eq!(apply(&mut reqs, &json!({"requestId": 1, "event": event})).unwrap()["state"], "failed");
    let t = reqs.get(1).unwrap();
    assert_eq!((t.error_kind, t.provider_text.as_str()), (Some(ErrorKind::Refused { status: 400 }), "bad  schema"));
}

#[test]
fn a_cancelled_request_lets_go_of_what_comes_later() {
    let mut reqs = Requests::default();
    request(&mut reqs, &ask()).unwrap();
    assert_eq!(cancel(&mut reqs, &json!({"requestId": 1})).unwrap()["cancelled"], true);
    assert_eq!(apply(&mut reqs, &json!({"requestId": 1, "event": {"type": "delta", "text": "late"}})).unwrap()["ignored"], true);
    assert_eq!(reqs.get(1).unwrap().state, RequestState::Cancelled);
    assert!(reqs.get(1).unwrap().text.is_empty());
    assert_eq!(cancel(&mut reqs, &json!({"requestId": 1})).unwrap()["cancelled"], false);
}

#[test]
fn unknown_ids_and_bad_parameters_are_refused_and_change_nothing() {
    let mut reqs = Requests::default();
    request(&mut reqs, &ask()).unwrap();
    let kept = reqs.clone();
    for params in [
        json!(null),
        json!({}),
        json!({"requestId": 2, "event": {"type": "done"}}),
        json!({"requestId": 0, "event": {"type": "done"}}),
        json!({"requestId": -1, "event": {"type": "done"}}),
        json!({"requestId": "1", "event": {"type": "done"}}),
        json!({"requestId": 18446744073709551615u64, "event": {"type": "done"}}),
        json!({"requestId": 1}),
        json!({"requestId": 1, "event": null}),
        json!({"requestId": 1, "event": {"type": "run", "command": "mail.send"}}),
        json!({"requestId": 1, "event": {"type": "delta"}}),
        json!({"requestId": 1, "event": {"type": "delta", "text": 5}}),
        json!({"requestId": 1, "event": {"type": "done"}, "send": true}),
    ] {
        assert!(matches!(apply(&mut reqs, &params), Err(CmdError::Params(_))), "{params}");
    }
    for params in [json!({}), json!({"requestId": 9}), json!({"requestId": 1.5}), json!({"requestId": 1, "x": 1})] {
        assert!(matches!(cancel(&mut reqs, &params), Err(CmdError::Params(_))), "{params}");
    }
    for params in [
        json!(null),
        json!({}),
        json!({"kind": "ask", "messages": [{"role": "user", "content": "x"}]}),
        json!({"kind": "answer"}),
        json!({"kind": "answer", "messages": []}),
        json!({"kind": "answer", "messages": [{"role": "tool", "content": "x"}]}),
        json!({"kind": "answer", "messages": [{"role": "user"}]}),
        json!({"kind": "answer", "messages": [{"role": "user", "content": "x"}], "sources": [0]}),
        json!({"kind": "answer", "messages": [{"role": "user", "content": "x"}], "sources": "3"}),
        json!({"kind": "answer", "messages": [{"role": "user", "content": "x"}], "sources": (1..=65).collect::<Vec<u64>>()}),
        json!({"kind": "answer", "messages": [{"role": "user", "content": "x"}], "stream": "yes"}),
        json!({"kind": "answer", "messages": [{"role": "user", "content": "x"}], "maxTokens": -5}),
        json!({"kind": "answer", "messages": [{"role": "user", "content": "x"}], "temperature": "hot"}),
        json!({"kind": "answer", "messages": [{"role": "user", "content": "x"}], "schema": {"type": "object"}}),
        json!({"kind": "answer", "messages": [{"role": "user", "content": "x".repeat(600 * 1024)}]}),
        json!({"kind": "answer", "messages": (0..33).map(|_| json!({"role": "user", "content": "x"})).collect::<Vec<_>>()}),
        json!({"kind": "answer", "messages": [{"role": "user", "content": "x"}], "send": true}),
    ] {
        assert!(matches!(request(&mut reqs, &params), Err(CmdError::Params(_))), "{params}");
    }
    assert_eq!(reqs, kept);
}

#[test]
fn requests_under_way_are_capped() {
    let mut reqs = Requests::default();
    for _ in 0..mailcraft_model::ai::MAX_LIVE {
        request(&mut reqs, &ask()).unwrap();
    }
    assert!(matches!(request(&mut reqs, &ask()), Err(CmdError::Params(_))));
}

#[test]
fn model_calls_are_never_an_agents() {
    for id in ["ai.request", "ai.apply", "ai.cancel", "ai.ask", "ai.narrow", "ai.answerResult", "ai.narrowResult"] {
        assert_eq!(access(id), Some(Access::Never), "{id}");
    }
}

#[test]
fn junk_never_panics() {
    let mut reqs = Requests::default();
    request(&mut reqs, &ask()).unwrap();
    let junk = [
        json!({"requestId": 1, "event": {"type": "delta", "text": "\u{0}\u{202e}\r\n".repeat(10_000)}}),
        json!({"requestId": 1, "event": {"type": "done", "text": "x".repeat(1 << 20)}}),
        json!({"requestId": 1e300, "event": {"type": "error", "message": [[[]]]}}),
        json!({"requestId": 1, "event": {"type": "error", "kind": {"refused": {"status": 4_294_967_295u32}}, "message": "m"}}),
        json!({"requestId": 1, "event": {"type": "error", "kind": {"timed_out": {"before_first_byte": [[]]}}, "message": "\u{0}"}}),
        json!({"requestId": 1, "event": {"type": "error", "kind": "malformed", "message": "\u{202e}".repeat(10_000), "providerText": "\u{0}".repeat(100_000)}}),
        json!({"kind": "narrow", "messages": [{"role": "user", "content": "\u{0}"}], "maxTokens": 1e300, "temperature": -1e300, "sources": [18446744073709551615u64]}),
        json!({"requestId": {"deep": [[[]]]}, "event": "done"}),
    ];
    for j in &junk {
        let _ = request(&mut reqs, j);
        let _ = apply(&mut reqs, j);
        let _ = cancel(&mut reqs, j);
    }
    for t in reqs.all() {
        assert!(t.text.len() <= mailcraft_model::ai::MAX_RESPONSE_BYTES);
        assert!(t.provider_text.len() <= mailcraft_model::ai::MAX_PROVIDER_TEXT_BYTES);
        assert!(!t.text.contains('\u{0}') && !t.text.contains('\u{202e}'));
    }
}

#[test]
fn the_commands_keep_the_requests_in_the_session_and_change_no_mailbox() {
    let mut s = crate::Session::new(crate::sample::mailbox(1_791_993_600, 0));
    let before = s.mailbox.clone();
    let id = s.run("ai.request", &ask()).unwrap()["request"]["id"].as_u64().unwrap();
    assert_eq!(s.run("ai.apply", &json!({"requestId": id, "event": {"type": "delta", "text": "hi"}})).unwrap()["state"], "streaming");
    assert_eq!(s.ai.get(id).unwrap().text, "hi");
    assert_eq!(s.run("ai.cancel", &json!({"requestId": id})).unwrap()["cancelled"], true);
    assert!(s.mailbox == before && !s.can_undo());
}

#[test]
fn another_mailbox_forgets_the_requests() {
    let mut s = crate::Session::new(crate::sample::mailbox(1_791_993_600, 0));
    let id = s.run("ai.request", &ask()).unwrap()["request"]["id"].as_u64().unwrap();
    s.run("file.new", &json!({})).unwrap();
    assert!(s.ai.all().is_empty());
    assert!(matches!(s.run("ai.apply", &json!({"requestId": id, "event": {"type": "done"}})), Err(CmdError::Params(_))));
    // New requests get new numbers: an old one's never names a new one.
    let next = s.run("ai.request", &ask()).unwrap()["request"]["id"].as_u64().unwrap();
    assert!(next > id);
}
