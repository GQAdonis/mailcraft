// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The owner's own control methods for the model providers (`ai.providers`, `ai.setProvider`,
//! `ai.setActive`, `ai.setConsent`, `ai.removeProvider`).
//!
//! They are control methods, not engine commands: no command id names them, so `engine.execute`,
//! the MCP tools and the agent gate never reach them, and an agent's request (one with an `agent`
//! field) is refused here too. No method takes or returns a key; the host's key functions are not
//! exposed at all. Every param is hostile: a wrong shape is an error, never a panic.
//!
//! What the AI surfaces show is found through `ui.inspect`'s `items` (the hit map), by geometry
//! only: an answer's text is the `answerText` item, its rect and nothing of its words; the
//! settings' states are the `ai*` items `ai::settings_page` and `ai::settings_device` register.
//! No method here hands out what a model wrote.

use serde_json::{Value, json};

use super::{Outcome, err, ok};
use crate::MailApp;

/// The control methods of this module.
pub(super) fn is_ai_method(method: &str) -> bool {
    matches!(method, "ai.providers" | "ai.setProvider" | "ai.setActive" | "ai.setConsent" | "ai.removeProvider")
}

/// The longest text taken for an id or an account address.
const MOST_TEXT: usize = 512;
/// The most members of a provider object, and its encoded size.
const MOST_MEMBERS: usize = 64;
const MOST_BYTES: usize = 16 * 1024;
/// Names in a provider that would carry a secret: refused whatever the host would do with them.
const KEY_NAMES: [&str; 6] = ["key", "apikey", "api_key", "apiKey", "secret", "token"];

pub(super) fn handle(app: &mut MailApp, agent: bool, method: &str, p: &Value) -> Outcome {
    if agent {
        return err(format!("`{method}` is the owner's own: it isn't available to agents"));
    }
    let Some(host) = app.services.ai_providers.as_deref() else {
        return err("model providers aren't available in this build");
    };
    let done = |r: Result<(), String>| match r {
        Ok(()) => ok(host.providers()),
        Err(e) => err(e),
    };
    match method {
        "ai.providers" => ok(host.providers()),
        "ai.setProvider" => match provider(p) {
            Ok(v) => done(host.set_provider(&v)),
            Err(e) => err(e),
        },
        "ai.setActive" => match text(p, "id", true) {
            Ok(id) => done(host.set_active(id)),
            Err(e) => err(e),
        },
        "ai.removeProvider" => match text(p, "id", false) {
            Ok(id) => done(host.remove_provider(id)),
            Err(e) => err(e),
        },
        "ai.setConsent" => {
            let (id, account) = match (text(p, "id", false), text(p, "account", false)) {
                (Ok(i), Ok(a)) => (i, a),
                (Err(e), _) | (_, Err(e)) => return err(e),
            };
            let Some(allowed) = p.get("allowed").and_then(Value::as_bool) else { return err("`allowed` must be true or false") };
            done(host.set_consent(id, account, allowed))
        }
        other => err(format!("unknown method `{other}`")),
    }
}

/// A short text param; `blank`: empty is allowed (for no provider at all).
fn text<'a>(p: &'a Value, name: &str, blank: bool) -> Result<&'a str, String> {
    let Some(t) = p.get(name).and_then(Value::as_str) else { return Err(format!("missing `{name}` (text)")) };
    if (!blank && t.trim().is_empty()) || t.chars().count() > MOST_TEXT || t.chars().any(char::is_control) {
        return Err(format!("`{name}` is empty, too long or has control characters"));
    }
    Ok(t)
}

/// The `provider` object, bounded, without any member that could hold a key. The consents are
/// not taken from here either ([`crate::AiProviders::set_provider`] ignores them; this drops them).
fn provider(p: &Value) -> Result<Value, String> {
    let Some(obj) = p.get("provider").and_then(Value::as_object) else { return Err("missing `provider` (an object)".into()) };
    if obj.len() > MOST_MEMBERS {
        return Err("`provider` has too many members".into());
    }
    if let Some(name) = obj.keys().find(|k| KEY_NAMES.iter().any(|n| n.eq_ignore_ascii_case(k))) {
        return Err(format!("`{name}`: keys are never accepted here; the window keeps them in the credential store"));
    }
    let mut clean = obj.clone();
    clean.remove("consented_accounts");
    let v = Value::Object(clean);
    if v.to_string().len() > MOST_BYTES {
        return Err("`provider` is too large".into());
    }
    if v.get("id").and_then(Value::as_str).is_none_or(|i| i.trim().is_empty() || i.chars().count() > MOST_TEXT) {
        return Err("`provider.id` is missing, empty or too long".into());
    }
    Ok(json!(v))
}
