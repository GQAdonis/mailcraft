// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What an answer says of the runtimes' chain (#ai, ai-runtime-chain): which runtimes were passed
//! over or failed before the one that answered, each with its reason; which were stopped for
//! trying to use a tool; and the words of the fallback question the owner is asked under the
//! `ask` policy. Everything here is read from the host's JSON, which names runtimes by id and
//! reasons by kind; nothing here is the model's text, and nothing is sent anywhere.

use serde_json::Value;

use crate::ai::Providers;

/// Most runtimes said under one answer: the chain is short, and a host that says more is cut.
const MOST_SAID: usize = 8;
/// Most characters of a provider's name or id, as said.
const SHOWN_NAME: usize = 40;

/// A runtime's name as the owner knows it, from its id: `compat:<id>` is the provider's model (or
/// its id, when the window doesn't know it).
pub(crate) fn runtime_name(id: &str, providers: &Providers) -> String {
    match id {
        "on_device" => tl!("the model on this computer").to_string(),
        "chatgpt" => tl!("ChatGPT plan").to_string(),
        "claude_code" => tl!("Claude Code").to_string(),
        "codex" => tl!("Codex").to_string(),
        "uar" => tl!("Universal Agent Runtime").to_string(),
        other => {
            let provider = other.strip_prefix("compat:").unwrap_or(other);
            let name = providers.get(provider).map(|(p, _)| p.model_name()).filter(|n| !n.trim().is_empty()).unwrap_or_else(|| provider.to_string());
            crate::ai::cut(crate::ai::settings_state::clean(&name, SHOWN_NAME * 4).trim(), SHOWN_NAME)
        }
    }
}

/// Why a runtime was passed over (a skip) or didn't answer (an error kind), in the owner's words.
pub(crate) fn reason_words(reason: &str) -> &'static str {
    match reason {
        "disabled" => tl!("it's turned off"),
        "no_consent" => tl!("this account's mail isn't allowed to go to it"),
        "not_set_up" | "not_configured" => tl!("it isn't set up"),
        "signed_out" => tl!("it's signed out"),
        "not_installed" => tl!("it isn't installed"),
        "broken" | "broken_runtime" => tl!("it isn't working"),
        "not_running" | "unreachable" => tl!("it couldn't be reached"),
        "not_offered" | "no_sandbox" => tl!("it isn't offered on this computer"),
        "no_room" | "too_large" => tl!("the question is too long for it"),
        "tool_use_stopped" => tl!("it tried to use a tool, so it was stopped"),
        "timed_out" => tl!("it took too long"),
        "refused" => tl!("it refused the request"),
        "cancelled" => tl!("it was called off"),
        _ => tl!("it couldn't answer"),
    }
}

/// One line for each runtime passed over or failed before the answer (at most [`MOST_SAID`]),
/// then one for each stopped for trying a tool that isn't already said.
pub(crate) fn passed_over(provenance: Option<&Value>, notices: &[String], providers: &Providers) -> Vec<String> {
    let mut lines = Vec::new();
    let mut said: Vec<String> = Vec::new();
    let notes = provenance.and_then(|p| p.get("passedOver")).and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    for note in notes.iter().take(MOST_SAID) {
        let Some(runtime) = note.get("runtime").and_then(Value::as_str) else { continue };
        let name = runtime_name(runtime, providers);
        let line = if let Some(skip) = note.get("skipped").and_then(Value::as_str) {
            crate::i18n::fmt(tl!("{runtime} was passed over: {reason}."), &[("runtime", &name), ("reason", reason_words(skip))])
        } else if let Some(kind) = note.get("failed").and_then(Value::as_str) {
            crate::i18n::fmt(tl!("{runtime} didn't answer: {reason}."), &[("runtime", &name), ("reason", reason_words(kind))])
        } else {
            continue;
        };
        said.push(runtime.to_string());
        lines.push(line);
    }
    for runtime in notices.iter().take(MOST_SAID) {
        if said.iter().any(|s| s == runtime) {
            continue;
        }
        let name = runtime_name(runtime, providers);
        lines.push(crate::i18n::fmt(tl!("{runtime} was stopped: it tried to use a tool."), &[("runtime", &name)]));
        said.push(runtime.clone());
    }
    lines
}

/// The fallback question for `request`, as words, when the host is asking it for that request.
pub(crate) fn question_words(question: &Value, request: u64, providers: &Providers) -> Option<String> {
    if question.get("request").and_then(Value::as_u64) != Some(request) {
        return None;
    }
    let failed = runtime_name(question.get("failed").and_then(Value::as_str)?, providers);
    let next = runtime_name(question.get("next").and_then(Value::as_str)?, providers);
    let reason = reason_words(question.get("reason").and_then(Value::as_str).unwrap_or_default());
    Some(crate::i18n::fmt(tl!("{failed} couldn't answer ({reason}). Try {next}?"), &[("failed", &failed), ("reason", reason), ("next", &next)]))
}
