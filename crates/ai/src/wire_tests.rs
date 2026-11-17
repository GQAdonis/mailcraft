// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What is written to a provider and what is read back: the shapes, the limits, and hostile
//! answers that are refused without a panic.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use proptest::prelude::*;
use serde_json::{Value, json};

use crate::error::AiError;
use crate::wire::*;

fn ask(text: &str) -> ChatRequest {
    ChatRequest { messages: vec![Message::system("Answer from the excerpts."), Message::user(text)], max_tokens: Some(512), ..ChatRequest::default() }
}

fn body_of(request: &ChatRequest, stream: bool) -> Value {
    serde_json::from_slice(&chat_body("llama3.2", request, stream).unwrap()).unwrap()
}

#[test]
fn a_chat_request_is_written_as_openai_compatible_servers_read_it() {
    let body = body_of(&ask("When is the meeting?"), true);
    assert_eq!(body["model"], "llama3.2");
    assert_eq!(body["stream"], true);
    assert_eq!(body["max_tokens"], 512);
    assert_eq!(
        body["messages"],
        json!([{"role": "system", "content": "Answer from the excerpts."}, {"role": "user", "content": "When is the meeting?"}])
    );
    // What isn't set isn't written.
    for absent in ["temperature", "seed", "response_format"] {
        assert!(body.get(absent).is_none(), "{absent}");
    }
    assert_eq!(body_of(&ask("x"), false)["stream"], false);
}

#[test]
fn structured_output_is_asked_for_by_json_schema() {
    let schema = json!({"type": "object", "properties": {"query": {"type": "string", "maxLength": 512}}, "required": ["query"], "additionalProperties": false});
    let request = ChatRequest {
        response_format: Some(ResponseFormat::JsonSchema { name: "mail_query".into(), schema: schema.clone(), strict: true }),
        temperature: Some(0.0),
        seed: Some(20261116),
        ..ask("unread mail from Ana last week")
    };
    let body = body_of(&request, false);
    assert_eq!(body["response_format"], json!({"type": "json_schema", "json_schema": {"name": "mail_query", "strict": true, "schema": schema}}));
    assert_eq!(body["temperature"], 0.0);
    assert_eq!(body["seed"], 20261116);
    let object = ChatRequest { response_format: Some(ResponseFormat::JsonObject), ..ask("x") };
    assert_eq!(body_of(&object, false)["response_format"], json!({"type": "json_object"}));
}

#[test]
fn requests_out_of_bounds_are_refused_before_writing() {
    let refused = |request: ChatRequest| matches!(chat_body("m", &request, false), Err(AiError::Config(_)));
    assert!(refused(ChatRequest::default()));
    assert!(refused(ChatRequest { messages: vec![Message::user("x"); MAX_MESSAGES + 1], ..ChatRequest::default() }));
    assert!(refused(ask(&"x".repeat(MAX_MESSAGE_BYTES + 1))));
    assert!(refused(ChatRequest { max_tokens: Some(0), ..ask("x") }));
    assert!(refused(ChatRequest { max_tokens: Some(1_000_000), ..ask("x") }));
    for temperature in [f32::NAN, f32::INFINITY, -0.5, 2.5] {
        assert!(refused(ChatRequest { temperature: Some(temperature), ..ask("x") }), "{temperature}");
    }
    let schema = |name: &str, schema: Value| ChatRequest {
        response_format: Some(ResponseFormat::JsonSchema { name: name.into(), schema, strict: true }),
        ..ask("x")
    };
    assert!(refused(schema("", json!({}))));
    assert!(refused(schema("has space", json!({}))));
    assert!(refused(schema("ok", json!("not an object"))));
    assert!(refused(schema("ok", json!({"description": "x".repeat(MAX_SCHEMA_BYTES)}))));
    // Five messages of 1 MB each fit one by one and not together.
    let big = ChatRequest { messages: vec![Message::user("y".repeat(MAX_MESSAGE_BYTES)); 5], ..ChatRequest::default() };
    assert!(refused(big));
}

#[test]
fn an_embeddings_request_and_its_limits() {
    let inputs = vec!["first".to_string(), "second".to_string()];
    let body: Value = serde_json::from_slice(&embeddings_body("nomic-embed-text", &inputs).unwrap()).unwrap();
    assert_eq!(body, json!({"model": "nomic-embed-text", "input": ["first", "second"]}));
    assert!(matches!(embeddings_body("m", &[]), Err(AiError::Config(_))));
    assert!(matches!(embeddings_body("m", &vec!["x".to_string(); MAX_EMBED_INPUTS + 1]), Err(AiError::Config(_))));
    assert!(matches!(embeddings_body("m", &["x".repeat(MAX_EMBED_INPUT_BYTES + 1)]), Err(AiError::Config(_))));
}

#[test]
fn a_whole_answer_is_read_leniently_and_within_limits() {
    let answer = parse_completion(
        br#"{"id":"x","model":"llama3.2","choices":[{"index":0,"message":{"role":"assistant","content":"At noon [1]."},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":4,"total_tokens":14},"system_fingerprint":"fp","prompt_eval_count":10}"#,
    )
    .unwrap();
    assert_eq!(answer.text, "At noon [1].");
    assert_eq!(answer.finish_reason.as_deref(), Some("stop"));
    assert_eq!(answer.model.as_deref(), Some("llama3.2"));
    assert_eq!(answer.usage, Some(Usage { prompt_tokens: 10, completion_tokens: 4, total_tokens: 14 }));
    // A null content is no text, not an error.
    assert_eq!(parse_completion(br#"{"choices":[{"message":{"content":null}}]}"#).unwrap().text, "");
    // An error in the answer is the provider's.
    assert_eq!(parse_completion(br#"{"error":{"message":"model\n not found","type":"x"}}"#), Err(AiError::Provider("model not found".into())));
    assert_eq!(parse_completion(br#"{"error":"busy"}"#), Err(AiError::Provider("busy".into())));
    for junk in [&b""[..], b"null", b"[]", b"{}", b"{\"choices\":[]}", b"{\"choices\":[{\"message\":{\"content\":7}}]}", b"<html>", b"{\"choices\":"]
    {
        assert!(matches!(parse_completion(junk), Err(AiError::Malformed(_))), "{:?}", String::from_utf8_lossy(junk));
    }
    let long = json!({"choices": [{"message": {"content": "z".repeat(MAX_ANSWER_BYTES + 1)}}]}).to_string();
    assert_eq!(parse_completion(long.as_bytes()), Err(AiError::TooLarge));
    // A model's name is cut to a line, and its debug shows no text.
    let odd = parse_completion(br#"{"model":"a\u0007b\nc","choices":[{"message":{"content":"private words"}}]}"#).unwrap();
    assert_eq!(odd.model.as_deref(), Some("a b c"));
    assert!(!format!("{odd:?}").contains("private"));
}

#[test]
fn a_streamed_chunk_is_its_delta() {
    let chunk = parse_chunk(r#"{"choices":[{"delta":{"role":"assistant","content":"Hel"},"finish_reason":null}],"model":"m"}"#).unwrap();
    assert_eq!((chunk.text.as_str(), chunk.finish_reason.as_deref(), chunk.model.as_deref()), ("Hel", None, Some("m")));
    let last = parse_chunk(r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#).unwrap();
    assert_eq!((last.text.as_str(), last.finish_reason.as_deref()), ("", Some("stop")));
    let usage = parse_chunk(r#"{"choices":[],"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}}"#).unwrap();
    assert_eq!(usage.usage.map(|u| u.total_tokens), Some(5));
    assert_eq!(parse_chunk(r#"{"error":{"message":"overloaded"}}"#), Err(AiError::Provider("overloaded".into())));
    for junk in ["", "{", "data: x", "{\"choices\":[{\"delta\":{\"content\":[1]}}]}"] {
        assert!(matches!(parse_chunk(junk), Err(AiError::Malformed(_))), "{junk}");
    }
}

#[test]
fn embeddings_are_placed_by_index_and_must_match_what_was_asked() {
    let got =
        parse_embeddings(br#"{"data":[{"index":1,"embedding":[0.5,1]},{"index":0,"embedding":[1,2.5]}],"model":"e","usage":{"prompt_tokens":4}}"#, 2)
            .unwrap();
    assert_eq!(got.vectors, vec![vec![1.0, 2.5], vec![0.5, 1.0]]);
    assert_eq!(got.model.as_deref(), Some("e"));
    assert_eq!(got.usage.map(|u| u.prompt_tokens), Some(4));
    // Without indexes, by place.
    assert_eq!(parse_embeddings(br#"{"data":[{"embedding":[1]},{"embedding":[2]}]}"#, 2).unwrap().vectors, vec![vec![1.0], vec![2.0]]);
    let junk: [(&[u8], usize); 9] = [
        (br#"{"data":[{"embedding":[1]}]}"#, 2),
        (br#"{"data":[{"index":5,"embedding":[1]}]}"#, 1),
        (br#"{"data":[{"index":0,"embedding":[1]},{"index":0,"embedding":[2]}]}"#, 2),
        (br#"{"data":[{"embedding":[1,2]},{"embedding":[3]}]}"#, 2),
        (br#"{"data":[{"embedding":[]}]}"#, 1),
        (br#"{"data":[{"embedding":[1e39]}]}"#, 1),
        (br#"{"data":[{"embedding":"AAAA"}]}"#, 1),
        (br#"{"data":7}"#, 1),
        (b"", 1),
    ];
    for (junk, expected) in junk {
        assert!(matches!(parse_embeddings(junk, expected), Err(AiError::Malformed(_))), "{}", String::from_utf8_lossy(junk));
    }
    let wide = json!({"data": [{"embedding": vec![0.0; MAX_DIMENSIONS + 1]}]}).to_string();
    assert!(matches!(parse_embeddings(wide.as_bytes(), 1), Err(AiError::Malformed(_))));
    assert_eq!(parse_embeddings(br#"{"error":{"message":"no such model"}}"#, 1), Err(AiError::Provider("no such model".into())));
}

#[test]
fn a_structured_answer_is_json_with_or_without_a_fence() {
    assert_eq!(json_answer(r#" {"query":"from:ana is:unread"} "#).unwrap(), json!({"query": "from:ana is:unread"}));
    assert_eq!(json_answer("```json\n{\"query\":\"a\"}\n```").unwrap(), json!({"query": "a"}));
    assert_eq!(json_answer("```\n[1]\n```\n").unwrap(), json!([1]));
    for junk in ["", "Sure! Here is the query: from:ana", "```json\n{\"query\":", "{\"a\":1} trailing"] {
        assert!(matches!(json_answer(junk), Err(AiError::Malformed(_))), "{junk}");
    }
    assert_eq!(json_answer(&format!("\"{}\"", "q".repeat(MAX_JSON_ANSWER_BYTES))), Err(AiError::TooLarge));
    // Nesting deeper than the JSON reader goes is an error, not a stack overflow.
    assert!(json_answer(&"[".repeat(100_000)).is_err());
}

#[test]
fn debug_shows_lengths_never_text() {
    let request = ask("the secret plan in Ana's message");
    let shown = format!("{request:?} {:?}", request.messages);
    assert!(!shown.contains("secret") && !shown.contains("excerpts"), "{shown}");
    let chunk = parse_chunk(r#"{"choices":[{"delta":{"content":"private words"}}]}"#).unwrap();
    assert!(!format!("{chunk:?}").contains("private"));
}

proptest! {
    #[test]
    fn hostile_answers_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..400), expected in 0usize..4) {
        let _ = parse_completion(&bytes);
        let _ = parse_chunk(&String::from_utf8_lossy(&bytes));
        let _ = parse_embeddings(&bytes, expected);
        let _ = json_answer(&String::from_utf8_lossy(&bytes));
    }

    #[test]
    fn answer_shaped_noise_never_panics(
        content in prop_oneof![Just(Value::Null), any::<String>().prop_map(Value::from), any::<i64>().prop_map(Value::from), Just(json!([]))],
        reason in prop_oneof![Just(Value::Null), "[ -~]{0,20}".prop_map(Value::from), Just(json!({}))],
        count in prop_oneof![Just(json!(-1)), Just(json!(1.5)), Just(json!("7")), any::<u64>().prop_map(Value::from)],
    ) {
        let whole = json!({"choices": [{"message": {"content": content.clone()}, "finish_reason": reason.clone()}], "usage": {"prompt_tokens": count.clone()}, "model": reason.clone()});
        if let Ok(answer) = parse_completion(whole.to_string().as_bytes()) {
            prop_assert!(answer.text.len() <= MAX_ANSWER_BYTES);
        }
        let piece = json!({"choices": [{"delta": {"content": content}, "finish_reason": reason}], "usage": {"total_tokens": count}});
        let _ = parse_chunk(&piece.to_string());
    }
}
