// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The host's side of Options › AI assistance (#ai, ai-settings-ux 7.4), against fake servers in
//! this process on 127.0.0.1: Test Connection reads the model list with the kept key only for
//! the provider as kept, fills the window, refuses a hosted route and puts the gateway question;
//! what the window is told of the providers (backup, never configured, each window). No real
//! provider, no real key.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mailcraft_ai::{ProviderConfig, ProviderKind, Timeouts};
use mailcraft_host::{SecretError, SecretStore};
use mailcraft_ui_egui::AiProviders;
use serde_json::{Value, json};

use super::keys::Broker;
use super::service::{Providers, test_connection};
use super::settings::Shared;

/// Not anybody's key.
const KEY: &str = "sk-test-not-a-real-key-0123456789";

/// A server answering `GET /v1/models` with `models`, keeping each request's head.
fn lister(models: &'static str) -> (u16, Arc<Mutex<Vec<String>>>, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (seen, chats) = (Arc::new(Mutex::new(Vec::new())), Arc::new(AtomicUsize::new(0)));
    let (kept, counted) = (seen.clone(), chats.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let mut bytes = Vec::new();
            let mut buf = [0u8; 4096];
            while !bytes.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => bytes.extend_from_slice(&buf[..n]),
                }
            }
            let head = String::from_utf8_lossy(&bytes).into_owned();
            if !head.starts_with("GET /v1/models ") {
                counted.fetch_add(1, Ordering::SeqCst);
            }
            kept.lock().unwrap().push(head);
            let _ = stream.write_all(
                format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{models}", models.len())
                    .as_bytes(),
            );
        }
    });
    (port, seen, chats)
}

struct Store(Option<String>);

impl SecretStore for Store {
    fn get(&self, _key: &str) -> Option<String> {
        self.0.clone()
    }
    fn set(&self, _key: &str, _secret: &str) -> Result<(), SecretError> {
        Ok(())
    }
    fn delete(&self, _key: &str) {}
}

fn provider(port: u16, kind: ProviderKind) -> ProviderConfig {
    ProviderConfig {
        id: "test-provider".into(),
        kind,
        base_url: format!("http://127.0.0.1:{port}"),
        model: "qwen".into(),
        timeouts: Timeouts { connect_secs: 2, first_byte_secs: 5, idle_secs: 5, total_secs: 20 },
        // No window said by the owner: the default of its kind (`ProviderConfig::default`'s is
        // the one on this computer, which for a gateway would be a number the owner wrote).
        context_window: mailcraft_ai::default_context_window(kind),
        ..ProviderConfig::default()
    }
}

fn failure_kind(error: &str) -> Value {
    serde_json::from_str::<Value>(error).unwrap()["kind"].clone()
}

#[test]
fn test_connection_lists_the_models_with_their_windows_and_uses_the_kept_key_only_where_it_was_given() {
    let (port, seen, chats) = lister(r#"{"data":[{"id":"qwen","context_window":131072},{"id":"embed"}]}"#);
    let settings = Shared::open(None);
    let kept = provider(port, ProviderKind::Gateway);
    settings.change(|s| s.set_provider(kept.clone())).unwrap();
    settings
        .change(|s| {
            s.note_key("test-provider");
            Ok(())
        })
        .unwrap();
    let store: Arc<dyn SecretStore> = Arc::new(Store(Some(KEY.into())));
    let broker = Broker::new(Duration::from_secs(5));
    let answer = test_connection(kept.clone(), &settings, Some(&store), &broker).unwrap();
    assert_eq!(answer, json!({"models": [{"id": "qwen", "contextWindow": 131072}, {"id": "embed"}], "contextWindow": 131072}));
    assert!(seen.lock().unwrap()[0].contains(&format!("Bearer {KEY}")), "the kept key, for the provider as kept");
    assert_eq!(settings.window_of(&settings.snapshot().providers[0]), (131_072, "server"));
    // The same id typed with another address: no key goes there.
    let (other, other_seen, _) = lister(r#"{"data":[{"id":"qwen"}]}"#);
    test_connection(provider(other, ProviderKind::Gateway), &settings, Some(&store), &broker).unwrap();
    assert!(!other_seen.lock().unwrap()[0].contains(KEY));
    assert_eq!(chats.load(Ordering::SeqCst), 0, "nothing but the list is asked");
}

#[test]
fn test_connection_refuses_a_hosted_route_and_asks_about_several_publishers() {
    let (hosted, _, _) = lister(r#"{"data":[{"id":"alibaba-token/qwen4-max"}]}"#);
    let settings = Shared::open(None);
    let error = test_connection(provider(hosted, ProviderKind::Local), &settings, None, &Broker::new(Duration::from_secs(5))).unwrap_err();
    assert_eq!(failure_kind(&error), json!("settings"));
    // The card is told which model made it refuse, and the server's words as data.
    let said: Value = serde_json::from_str(&error).unwrap();
    assert_eq!(said["refusedModel"], json!("alibaba-token/qwen4-max"));
    assert_eq!(said["providerText"], json!("alibaba-token/qwen4-max"));
    let (mixed, _, _) = lister(r#"{"data":[{"id":"hf.co/a/b"},{"id":"lmstudio-community/c"}]}"#);
    settings.change(|s| s.set_provider(provider(mixed, ProviderKind::Local))).unwrap();
    test_connection(provider(mixed, ProviderKind::Local), &settings, None, &Broker::new(Duration::from_secs(5))).unwrap();
    let question = settings.gateway_question().unwrap();
    assert_eq!(question.provider, "test-provider");
}

#[test]
fn test_connection_says_the_kind_of_a_closed_port_and_of_settings_that_cant_be_used() {
    let closed = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let settings = Shared::open(None);
    let error = test_connection(provider(closed, ProviderKind::Local), &settings, None, &Broker::new(Duration::from_secs(5))).unwrap_err();
    assert_eq!(failure_kind(&error), json!("unreachable"));
    let message = serde_json::from_str::<Value>(&error).unwrap()["message"].as_str().unwrap().to_string();
    assert!(message.contains(&format!("127.0.0.1:{closed}")));
    let elsewhere_plain = ProviderConfig { kind: ProviderKind::Remote, ..provider(closed, ProviderKind::Local) };
    let error = test_connection(elsewhere_plain, &settings, None, &Broker::new(Duration::from_secs(5))).unwrap_err();
    assert_eq!(failure_kind(&error), json!("settings"));
    assert!(!error.contains(KEY));
}

#[test]
fn the_window_is_told_the_backup_never_configured_and_each_window() {
    let settings = Shared::open(None);
    let providers = Providers::new(settings.clone(), None, egui::Context::default());
    let shown = providers.providers();
    assert_eq!((shown["backup"].clone(), shown["neverConfigured"].clone()), (json!("apple-on-device"), json!(true)));
    providers.set_provider(&serde_json::to_value(provider(1, ProviderKind::Gateway)).unwrap()).unwrap();
    providers.set_active("test-provider").unwrap();
    let shown = providers.providers();
    assert_eq!(shown["neverConfigured"], json!(false));
    assert_eq!(shown["primary"], json!("test-provider"));
    assert_eq!(shown["providers"][0]["kind"], json!("gateway"));
    assert_eq!(
        (shown["providers"][0]["contextWindow"].clone(), shown["providers"][0]["contextWindowSource"].clone()),
        (json!(32_768), json!("default"))
    );
    assert!(!shown.to_string().contains("api_key"));
    // Backups: none, the on-device model, nothing else; a primary of None turns it off.
    providers.set_backup("").unwrap();
    assert_eq!(providers.providers()["backup"], json!(""));
    assert!(providers.set_backup("somebody-elses-cloud").is_err());
    providers.set_backup("apple-on-device").unwrap();
    providers.set_active("").unwrap();
    assert_eq!(providers.providers()["backup"], json!(""));
    assert!(providers.set_backup("apple-on-device").is_err());
    assert_eq!(providers.on_device()["state"], if super::device::PLATFORM { json!("off") } else { json!("unavailable") });
}

#[test]
fn the_gateway_question_is_answered_for_the_provider_it_asks_about() {
    let (mixed, _, _) = lister(r#"{"data":[{"id":"hf.co/a/b"},{"id":"lmstudio-community/c"}]}"#);
    let settings = Shared::open(None);
    let providers = Providers::new(settings.clone(), None, egui::Context::default());
    assert!(providers.answer_gateway("test-provider", "0123456789abcdef", true).is_err(), "nothing was asked");
    settings.change(|s| s.set_provider(provider(mixed, ProviderKind::Local))).unwrap();
    test_connection(provider(mixed, ProviderKind::Local), &settings, None, &Broker::new(Duration::from_secs(5))).unwrap();
    let asked = providers.gateway_question().unwrap();
    assert_eq!(asked["provider"], json!("test-provider"));
    let models = asked["models"].as_str().unwrap().to_string();
    assert!(providers.answer_gateway("someone-else", &models, true).is_err());
    // An answer about another list than the one shown is refused, and nothing changes.
    assert!(providers.answer_gateway("test-provider", "0123456789abcdef", true).is_err());
    assert_eq!(settings.snapshot().providers[0].kind, ProviderKind::Local);
    providers.answer_gateway("test-provider", &models, true).unwrap();
    assert_eq!(settings.snapshot().providers[0].kind, ProviderKind::Gateway);
    assert!(providers.gateway_question().is_none());
}

#[test]
fn ports_typed_by_the_owner_are_checked_before_they_are_kept() {
    let settings = Shared::open(None);
    let providers = Providers::new(settings.clone(), None, egui::Context::default());
    providers.set_device_ports("11437, 11500").unwrap();
    assert_eq!(settings.snapshot().device_ports, [11437, 11500]);
    for bad in ["0", "70000", "11437; 1", "-1", "1,2,3,4,5,6,7,8,9", "１２"] {
        assert!(providers.set_device_ports(bad).is_err(), "{bad}");
        assert_eq!(settings.snapshot().device_ports, [11437, 11500], "{bad}");
    }
    providers.set_device_ports("").unwrap();
    assert!(settings.snapshot().device_ports.is_empty());
    providers.stop_using_on_device().unwrap();
}

#[test]
fn an_answer_about_a_provider_moved_since_the_question_is_refused() {
    let (mixed, _, _) = lister(r#"{"data":[{"id":"hf.co/a/b"},{"id":"lmstudio-community/c"}]}"#);
    let settings = Shared::open(None);
    let providers = Providers::new(settings.clone(), None, egui::Context::default());
    settings.change(|s| s.set_provider(provider(mixed, ProviderKind::Local))).unwrap();
    test_connection(provider(mixed, ProviderKind::Local), &settings, None, &Broker::new(Duration::from_secs(5))).unwrap();
    let models = providers.gateway_question().unwrap()["models"].as_str().unwrap().to_string();
    // The owner points the provider at another server: the question was about the old one.
    let (other, _, _) = lister(r#"{"data":[{"id":"hf.co/a/b"},{"id":"lmstudio-community/c"}]}"#);
    settings.change(|s| s.set_provider(provider(other, ProviderKind::Local))).unwrap();
    assert!(providers.answer_gateway("test-provider", &models, false).is_err());
    assert!(providers.answer_gateway("test-provider", &models, true).is_err());
    assert_eq!(settings.snapshot().providers[0].kind, ProviderKind::Local);
}

#[test]
fn the_window_is_never_shown_the_detection_cache_or_the_host_id() {
    let settings = Shared::open(None);
    settings
        .change(|s| {
            s.set_chatgpt_host_id("Zm9yLXRlc3RzLW9ubHktbm90LWEtcmVhbC1ob3N0LTA")?;
            s.detected.insert("claude_code".into(), super::settings_v3::Detected::default());
            Ok(())
        })
        .unwrap();
    let providers = Providers::new(settings.clone(), None, egui::Context::default());
    let shown = providers.providers();
    assert!(shown.get("detected").is_none() && shown.get("chatgpt").is_none(), "{shown}");
    assert!(!shown.to_string().contains("Zm9yLXRlc3Rz"));
    // What the window reads as before the chain is still there.
    assert_eq!((shown["primary"].clone(), shown["active"].clone(), shown["version"].clone()), (json!(""), json!(""), json!(3)));
}
