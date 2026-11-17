// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The model list is read as hostile input, and the route check tells a gateway that forwards to
//! the internet from a server running models here (SCN-RELAY-001, SCN-RELAY-002).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use proptest::prelude::*;
use serde_json::{Value, json};

use crate::error::AiError;
use crate::models::{MAX_MODEL_ID, MAX_MODELS, ModelInfo, ModelList, Route, fingerprint, is_fingerprint, parse_models, route_of};

fn list_of(ids: &[&str]) -> ModelList {
    ModelList { models: ids.iter().map(|id| ModelInfo { id: id.to_string(), ..ModelInfo::default() }).collect(), truncated: false }
}

fn body(entries: Value) -> Vec<u8> {
    json!({"object": "list", "data": entries}).to_string().into_bytes()
}

#[test]
fn a_list_is_read_with_its_owners_and_windows() {
    let read = parse_models(&body(json!([
        {"id": "llama3.2:latest", "object": "model", "owned_by": "library", "context_length": 131072},
        {"id": "qwen", "context_window": 32768},
        {"id": "small", "max_model_len": 10},
        {"id": "huge", "context_length": 9_999_999_999u64},
    ])))
    .unwrap();
    assert_eq!(read.models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["llama3.2:latest", "qwen", "small", "huge"]);
    assert_eq!(read.models[0].owned_by.as_deref(), Some("library"));
    assert_eq!(read.models.iter().map(|m| m.context_window).collect::<Vec<_>>(), [Some(131_072), Some(32_768), Some(256), Some(1_048_576)]);
    assert!(read.find(" qwen ").is_some());
    // `{"models": […]}` too.
    assert_eq!(parse_models(br#"{"models":[{"id":"a"}]}"#).unwrap().models.len(), 1);
}

#[test]
fn what_isnt_a_model_is_skipped_and_what_isnt_a_list_is_an_error() {
    let read = parse_models(&body(json!([
        {"id": ""}, {"id": 7}, {"name": "x"}, "string", null, [1],
        {"id": "bad\u{0007}id"}, {"id": "x".repeat(MAX_MODEL_ID + 1)},
        {"id": "kept"}, {"id": "kept"},
        {"id": "owner", "owned_by": {"nested": true}},
    ])))
    .unwrap();
    assert_eq!(read.models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["kept", "owner"]);
    assert_eq!(read.models[1].owned_by, None);
    for bad in [&b"not json"[..], b"[]", b"{}", br#"{"data": "x"}"#, b""] {
        assert!(matches!(parse_models(bad), Err(AiError::Malformed(_))), "{}", String::from_utf8_lossy(bad));
    }
    assert!(matches!(parse_models(br#"{"error": {"message": "no"}}"#), Err(AiError::Provider(_))));
    assert_eq!(parse_models(&vec![b' '; 1024 * 1024 + 1]), Err(AiError::TooLarge));
}

#[test]
fn a_long_list_is_cut_at_its_cap() {
    let entries: Vec<Value> = (0..MAX_MODELS + 5).map(|n| json!({"id": format!("m{n}")})).collect();
    let read = parse_models(&body(Value::Array(entries))).unwrap();
    assert_eq!(read.models.len(), MAX_MODELS);
    assert!(read.truncated);
}

#[test]
fn relay_001_hosted_routes_are_refused_as_local() {
    // The ids of the owner's gateway in front of Alibaba Cloud, and the other hosted routes.
    for ids in [
        &["openai/gpt-5", "alibaba-token/qwen4-max"][..],
        &["alibaba-token/qwen4-max"],
        &["alibaba-token/qwen3.8-max", "llama3.2:latest"],
        &["anthropic/claude"],
        &["azure/gpt"],
        &["bedrock/titan"],
        &["vertex_ai/gemini-pro"],
        &["gemini/gemini-2"],
        &["groq/llama"],
        &["openrouter/anthropic/claude"],
        &["dashscope/qwen-max"],
        &["Anthropic/Claude"],
        &["some-token/model"],
    ] {
        assert!(matches!(route_of(&list_of(ids)), Route::Hosted { .. }), "{ids:?}");
    }
    match route_of(&list_of(&["openai/gpt-5", "alibaba-token/qwen4-max"])) {
        Route::Hosted { listed } => assert_eq!(listed, "alibaba-token/qwen4-max"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn relay_001_an_owner_naming_a_hosted_service_is_refused_too() {
    for owner in ["anthropic", "DashScope", "openrouter", "alibaba-token"] {
        let list = ModelList { models: vec![ModelInfo { id: "chat".into(), owned_by: Some(owner.into()), context_window: None }], truncated: false };
        assert!(matches!(route_of(&list), Route::Hosted { .. }), "{owner}");
    }
}

#[test]
fn relay_002_local_publisher_namespaces_are_asked_about_once_not_refused() {
    let list =
        list_of(&["llama3.2:latest", "hf.co/bartowski/Llama-3.2-3B-Instruct-GGUF:Q4_K_M", "lmstudio-community/qwen3-8b", "openai/gpt-oss-20b"]);
    match route_of(&list) {
        Route::Ask { publishers } => assert_eq!(publishers, ["hf.co", "lmstudio-community", "openai"]),
        other => panic!("{other:?}"),
    }
    for ids in [&["openai/gpt-oss-20b"][..], &["hf.co/x/y"], &["mistral/m"], &["deepseek/r1"]] {
        assert!(matches!(route_of(&list_of(ids)), Route::Ask { .. }), "{ids:?}");
    }
}

#[test]
fn several_owners_are_asked_about_and_one_is_not() {
    let owned = |owners: &[&str]| ModelList {
        models: owners
            .iter()
            .enumerate()
            .map(|(n, o)| ModelInfo { id: format!("m{n}"), owned_by: Some(o.to_string()), context_window: None })
            .collect(),
        truncated: false,
    };
    assert_eq!(route_of(&owned(&["library", "library"])), Route::Local);
    assert_eq!(route_of(&owned(&["organization_owner"])), Route::Local);
    assert!(matches!(route_of(&owned(&["openai", "meta"])), Route::Ask { .. }));
}

#[test]
fn a_plain_local_server_is_local() {
    for ids in [&["llama3.2:latest", "qwen3:8b"][..], &["apple-foundationmodel"], &[], &["/x"], &["x/"]] {
        assert_eq!(route_of(&list_of(ids)), Route::Local, "{ids:?}");
    }
}

#[test]
fn the_fingerprint_names_the_list_whatever_its_order() {
    let one = fingerprint(&list_of(&["b", "A", "c"]));
    assert_eq!(one, fingerprint(&list_of(&["c", "a", "b", "b"])));
    assert_ne!(one, fingerprint(&list_of(&["a", "b"])));
    assert!(is_fingerprint(&one));
    assert!(!is_fingerprint("xyz") && !is_fingerprint(&"g".repeat(16)));
}

proptest! {
    #[test]
    fn any_bytes_are_a_list_or_an_error_never_a_panic(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        if let Ok(list) = parse_models(&bytes) {
            let _ = route_of(&list);
            prop_assert!(is_fingerprint(&fingerprint(&list)));
        }
    }

    #[test]
    fn any_ids_are_classified_without_a_panic(ids in proptest::collection::vec(".{0,40}", 0..20)) {
        let entries: Vec<Value> = ids.iter().map(|id| json!({"id": id, "owned_by": id})).collect();
        if let Ok(list) = parse_models(&body(Value::Array(entries))) {
            let _ = route_of(&list);
        }
    }
}
