// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! apfel's identity read from its two answers (#ai, ai-provider-chain 5.2, 5.4): exactly one model,
//! `apple-foundationmodel`, and a `/health` of apfel's shape; look-alikes (Ollama on the same
//! port, a gateway, a server that names another model) refused; windows clamped; hostile bytes
//! never a panic. Synthetic answers only.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use proptest::prelude::*;
use serde_json::json;

use crate::config::{MAX_CONTEXT, MIN_CONTEXT, ProviderKind};
use crate::config::{ProviderConfig, clamp_context_window, default_context_window};
use crate::error::AiError;
use crate::models::{ModelInfo, ModelList, parse_models};
use crate::on_device::{APPLE_MODEL, Health, MAX_HEALTH_BYTES, context_window_of, identity, is_identity, lists_only_apple, parse_health};

fn list(ids: &[&str]) -> ModelList {
    ModelList { models: ids.iter().map(|id| ModelInfo { id: (*id).into(), ..ModelInfo::default() }).collect(), truncated: false }
}

fn apfel_health() -> Vec<u8> {
    json!({"status": "ok", "model": APPLE_MODEL, "model_available": true, "context_window": 4096}).to_string().into_bytes()
}

#[test]
fn apfel_answers_are_its_identity() {
    let health = parse_health(&apfel_health()).unwrap();
    assert_eq!((health.available, health.context_window), (Some(true), Some(4_096)));
    let apple = list(&[APPLE_MODEL]);
    let id = identity(&apple, &health).unwrap();
    assert!(is_identity(&id), "{id}");
    // The same answers are the same identity; another list is another.
    assert_eq!(identity(&apple, &health), Some(id.clone()));
    assert_eq!(context_window_of(&apple, &health), Some(4_096));
}

#[test]
fn look_alikes_are_not_apfel() {
    let health = parse_health(&apfel_health()).unwrap();
    // Ollama on 11434, a gateway, an apfel-named model beside others: not candidates.
    let lookalikes: Vec<Vec<&str>> = vec![
        vec!["llama3.2:latest"],
        vec!["openai/gpt-5", "alibaba-token/qwen"],
        vec![APPLE_MODEL, "llama3.2:latest"],
        vec![],
        vec!["Apple-FoundationModel"],
    ];
    for ids in &lookalikes {
        assert!(!lists_only_apple(&list(ids)), "{ids:?}");
        assert_eq!(identity(&list(ids), &health), None, "{ids:?}");
    }
    let mut cut = list(&[APPLE_MODEL]);
    cut.truncated = true;
    assert_eq!(identity(&cut, &health), None, "a list that was cut short isn't one model");
}

#[test]
fn a_health_answer_not_of_apfels_shape_is_refused() {
    for body in [
        json!({"status": "ok"}),
        json!({"model": APPLE_MODEL}),
        json!({"status": "ok", "model": "llama3.2"}),
        json!({"status": "ok", "model": 7, "model_available": true}),
        json!({"status": 1, "model_available": true}),
        json!(["status", "ok"]),
        json!("ok"),
    ] {
        assert!(matches!(parse_health(body.to_string().as_bytes()), Err(AiError::Malformed(_))), "{body}");
    }
    assert!(matches!(parse_health(b"OK"), Err(AiError::Malformed(_))));
    assert!(matches!(parse_health(&vec![b' '; MAX_HEALTH_BYTES + 1]), Err(AiError::TooLarge)));
    // A shape with only some of apfel's facts is still apfel's.
    assert_eq!(parse_health(br#"{"status":"ok","available":false}"#).unwrap(), Health { model: None, available: Some(false), context_window: None });
}

#[test]
fn windows_from_outside_are_clamped() {
    for (said, kept) in [(0u64, MIN_CONTEXT), (10, MIN_CONTEXT), (4_096, 4_096), (9_999_999_999, MAX_CONTEXT), (u64::MAX, MAX_CONTEXT)] {
        assert_eq!(clamp_context_window(said), kept, "{said}");
        let body = json!({"status": "ok", "context_window": said}).to_string();
        assert_eq!(parse_health(body.as_bytes()).unwrap().context_window, Some(kept), "{said}");
    }
    // /health first, else the model list's entry.
    let listed = parse_models(json!({"data": [{"id": APPLE_MODEL, "context_length": 8192}]}).to_string().as_bytes()).unwrap();
    assert_eq!(context_window_of(&listed, &Health { available: Some(true), ..Health::default() }), Some(8_192));
    assert_eq!(context_window_of(&listed, &Health { context_window: Some(2_048), ..Health::default() }), Some(2_048));
}

#[test]
fn defaults_by_kind_and_the_root_address() {
    assert_eq!(default_context_window(ProviderKind::Local), 4_096);
    assert_eq!(default_context_window(ProviderKind::Gateway), 32_768);
    assert_eq!(default_context_window(ProviderKind::Remote), 32_768);
    let at = |base: &str| ProviderConfig { base_url: base.into(), ..ProviderConfig::default() }.root_endpoint("health");
    assert_eq!(at("http://127.0.0.1:11434"), "http://127.0.0.1:11434/health");
    assert_eq!(at("http://127.0.0.1:11434/v1/"), "http://127.0.0.1:11434/health");
    assert_eq!(at("http://127.0.0.1:11434/V1"), "http://127.0.0.1:11434/health");
}

proptest! {
    #[test]
    fn hostile_health_answers_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = parse_health(&bytes);
    }

    #[test]
    fn hostile_identity_text_is_checked_without_panic(text in ".{0,300}") {
        let _ = is_identity(&text);
    }
}
