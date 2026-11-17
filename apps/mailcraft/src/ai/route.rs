// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The route check of a server set up as on this computer (#ai, SCN-RELAY-001, SCN-RELAY-002):
//! before its first request in a session, its model list (`GET /v1/models`: no mail, no key) is
//! read, and what it says decides what may be sent (`mailcraft_ai::route_of`):
//!
//! - a hosted service's route (`alibaba-token/…`, `anthropic/…`): nothing is sent, the request
//!   fails with the kind `settings`, and the owner is told to set the server up as a gateway;
//! - several publishers: the owner is asked once whether the server sends mail to the internet
//!   ([`super::settings::GatewayQuestion`], which the window reads and answers). The request waits
//!   for the answer, at most [`QUESTION_WAIT`]: "no" is kept for that address and that list and
//!   the request goes on; "yes" makes the provider a gateway, and the request is taken again as
//!   one (so an account's mail goes only with its consent); the question closed without an
//!   answer, or no answer in time, sends nothing. A question not answered in time stays waiting
//!   (it isn't taken away): a late answer is kept and holds for the next request. A job that
//!   can't wait for the owner (embeddings, [`Asking::Never`]) puts no question and sends nothing;
//!   the next chat request asks;
//! - otherwise the server is what the owner set it up as.
//!
//! A list that can't be had because the server can't be reached ends the request then and there;
//! a list that isn't one (a server without `/v1/models`) leaves the owner's choice as it is, and is
//! asked for again at the next request. A gateway or a provider elsewhere isn't checked: the owner
//! already said its mail leaves this computer.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mailcraft_ai::{AiError, ProviderConfig, ProviderKind, Route, fingerprint, route_of};
use mailcraft_model::ai::{ErrorKind, clean_provider_text};

use super::run::{Failure, failure_of, shown_address};
use super::settings::{GatewayQuestion, Shared, route_address};

/// The longest a request waits for the owner's answer to the gateway question.
pub(crate) const QUESTION_WAIT: Duration = Duration::from_secs(120);
/// How often the answer is looked for.
const POLL: Duration = Duration::from_millis(100);

/// Whether a request may put the gateway question to the owner and wait for the answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Asking {
    /// Put the question (unless another is waiting) and wait for the answer, at most this long.
    Wait(Duration),
    /// A background job the owner didn't start (embeddings): no question is put, and a list the
    /// owner hasn't answered for sends nothing.
    Never,
}

/// What the check let through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Checked {
    /// The request may go to the provider as it is set up.
    Passed,
    /// The owner's answer changed the provider (into a gateway): take the request again.
    Reclassified,
}

/// The route check of `config` (see the module's notes), waiting for the owner's answer at most
/// [`QUESTION_WAIT`].
pub(crate) fn check(config: &ProviderConfig, settings: &Shared, cancel: &AtomicBool) -> Result<Checked, Failure> {
    check_as(config, settings, cancel, Asking::Wait(QUESTION_WAIT))
}

/// The route check of `config`, asking the owner as `asking` says.
pub(crate) fn check_as(config: &ProviderConfig, settings: &Shared, cancel: &AtomicBool, asking: Asking) -> Result<Checked, Failure> {
    if config.kind != ProviderKind::Local {
        return Ok(Checked::Passed);
    }
    let address = route_address(config);
    if settings.route_checked(&config.id, &address) {
        return Ok(Checked::Passed);
    }
    let shown = shown_address(&config.base_url);
    let started = Instant::now();
    let list = match mailcraft_ai::models(config, cancel) {
        Ok(list) => list,
        Err(
            error @ (AiError::Unreachable(_)
            | AiError::TimedOut(_)
            | AiError::Cancelled
            | AiError::Certificate(_)
            | AiError::Refused(_)
            | AiError::Config(_)
            | AiError::NotConfigured
            | AiError::NoConsent),
        ) => return Err(failure_of(&error, &shown, started.elapsed(), false)),
        // No list to check: the owner's choice stands, and the list is asked for next time.
        //
        // Residual risk (S4, docs/threat-model.md): a gateway set up as on this computer that
        // answers `/v1/models` with an error, or with something that isn't a list, passes here
        // and is sent what a server on this computer is sent. The owner's choice of kind stays
        // the rule; this check is a second guard that such a server steps around.
        Err(AiError::Status { .. } | AiError::Provider(_) | AiError::Malformed(_) | AiError::TooLarge) => return Ok(Checked::Passed),
    };
    // What the list says of the model's context window is kept for the session (#ai 5.4).
    if let Some(window) = list.find(&config.model).and_then(|m| m.context_window) {
        settings.keep_window(config, window);
    }
    match route_of(&list) {
        Route::Local => {
            settings.route_passed(&config.id, &address);
            Ok(Checked::Passed)
        }
        Route::Hosted { listed } => Err(Failure {
            kind: ErrorKind::Settings,
            message: format!(
                "Nothing was sent: the model server at {shown} is set up as one on this computer, but it lists models of a service on the internet. If it is a gateway, set it up as one."
            ),
            provider_text: Some(clean_provider_text(&format!("It lists {listed}"))),
        }),
        Route::Ask { publishers } => {
            let models = fingerprint(&list);
            if settings.read(|s| s.is_local_list(&address, &models)) {
                settings.route_passed(&config.id, &address);
                return Ok(Checked::Passed);
            }
            let wait = match asking {
                Asking::Wait(wait) => wait,
                Asking::Never => {
                    return Err(Failure::new(
                        ErrorKind::Settings,
                        format!("Nothing was sent: MailCraft needs your answer about the model server at {shown} first."),
                    ));
                }
            };
            let question = GatewayQuestion {
                provider: config.id.clone(),
                address: shown,
                models,
                publishers: publishers.iter().map(|p| clean_provider_text(p)).collect(),
                route_address: address,
            };
            wait_for_answer(settings, question, cancel, wait)
        }
    }
}

/// Puts `question` to the owner (unless another is waiting) and waits for the answer, at most
/// `wait`. When the time is up the request fails, and the question stays until it is answered or
/// closed.
fn wait_for_answer(settings: &Shared, question: GatewayQuestion, cancel: &AtomicBool, wait: Duration) -> Result<Checked, Failure> {
    let unanswered = Failure::new(
        ErrorKind::Settings,
        format!("Nothing was sent: MailCraft is waiting for your answer about the model server at {}.", question.address),
    );
    if !settings.ask_gateway(question.clone()) {
        return Err(Failure::new(ErrorKind::Settings, "Nothing was sent: another question about a model server is waiting for your answer."));
    }
    let ours = |waiting: &GatewayQuestion| waiting.provider == question.provider && waiting.models == question.models;
    let started = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(failure_of(&AiError::Cancelled, &question.address, started.elapsed(), false));
        }
        let (kept_local, changed) = settings.read(|s| {
            let changed = s.provider(&question.provider).is_none_or(|p| p.kind != ProviderKind::Local || route_address(p) != question.route_address);
            (s.is_local_list(&question.route_address, &question.models), changed)
        });
        if kept_local {
            settings.route_passed(&question.provider, &question.route_address);
            return Ok(Checked::Passed);
        }
        if changed {
            return Ok(Checked::Reclassified);
        }
        if !settings.gateway_question().is_some_and(|waiting| ours(&waiting)) {
            // Closed without an answer.
            return Err(unanswered);
        }
        if started.elapsed() >= wait {
            // The question stays: a late answer is kept, and holds for the next request.
            return Err(unanswered);
        }
        std::thread::sleep(POLL);
    }
}
