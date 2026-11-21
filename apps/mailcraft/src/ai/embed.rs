// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Embeddings on the desktop (#ai): where the mailbox's passages are embedded ([`target`]), one
//! batch of them embedded on a host thread ([`run_batch`]), and the vector of the owner's question
//! ([`query`]). The engine hands out the passages and keeps the vectors (`ai.embedRequest`,
//! `ai.embedApply`); nothing here keeps any.
//!
//! Only a runtime of the owner's chain that serves `/embeddings` is asked (ai-runtime-chain 20.4):
//! the first enabled `compat` provider with an embeddings model the owner named
//! ([`embedding_provider`]). The on-device model serves no `/embeddings`, and a CLI, the ChatGPT
//! plan or UAR is never asked for one: a chain of those alone embeds nothing, and a batch says no
//! embedding runtime is available. A
//! provider elsewhere is asked only for the mail of the accounts the owner allowed it (the engine
//! hands out nothing else, and `mailcraft_ai` refuses it all the same), and the question carries
//! the owner's words alone. A server set up as on this computer is route-checked first
//! ([`route_allows`]): embeddings can't wait for the owner, so a list that would need the gateway
//! question, or that names a hosted service, sends nothing.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use mailcraft_ai::{Carries, ProviderConfig};
use mailcraft_host::SecretStore;
use mailcraft_model::ai::{ErrorKind, RuntimeId};
use mailcraft_ui_egui::{EmbedJob, EmbedTarget};

use super::keys::{self, Broker};
use super::route::{self, Asking, Checked};
use super::run::{Failure, said, shown_address};
use super::settings::{AiSettings, Shared};

/// The route check of `config` before any embedding request (F1, SCN-BASE-002), never asking the
/// owner: a provider that changed under the check (`Reclassified`) has its batch dropped, and a
/// refusal or a question still to be answered sends nothing.
pub(crate) fn route_allows(config: &ProviderConfig, settings: &Shared, cancel: &AtomicBool) -> Result<(), Failure> {
    match route::check_as(config, settings, cancel, Asking::Never)? {
        Checked::Passed => Ok(()),
        Checked::Reclassified => Err(Failure::new(ErrorKind::Settings, "Nothing was sent: the model provider changed.")),
    }
}

/// What a batch says when no runtime of the chain serves embeddings.
pub(crate) const NO_EMBEDDING_RUNTIME: &str = "no embedding runtime is available";

/// The provider that embeds: the first enabled entry of the owner's chain that serves
/// `/embeddings`, a `compat` provider that is set up with an embeddings model the owner named.
/// Every other kind is passed over: the on-device model has no `/embeddings`, and a CLI, the
/// ChatGPT plan or UAR is never sent passages.
pub(crate) fn embedding_provider(settings: &AiSettings) -> Option<&ProviderConfig> {
    let serves = |p: &&ProviderConfig| !p.embedding_model.trim().is_empty();
    let on = |p: &&ProviderConfig| AiSettings::compat(&p.id).and_then(|r| settings.chain.get(&r).map(|e| e.enabled())).unwrap_or(false);
    // The primary is the first enabled `compat` entry of the chain: when it is on and serves
    // embeddings it is the one; else the first enabled `compat` entry that does.
    if let Some(primary) = settings.active().filter(on).filter(serves) {
        return Some(primary);
    }
    settings.chain.entries().iter().filter(|e| e.enabled()).find_map(|e| match e.runtime() {
        RuntimeId::Compat(id) => settings.provider(id.as_str()).filter(serves),
        RuntimeId::OnDevice | RuntimeId::ChatGpt | RuntimeId::ClaudeCode | RuntimeId::Codex | RuntimeId::Uar => None,
    })
}

/// Where the passages are embedded now: the embedding provider's model ([`embedding_provider`]), and the accounts
/// it may be sent mail of (`None`: every account, a provider on this computer). None when there is
/// no embedding provider, or it is elsewhere and allowed no account.
pub(crate) fn target(settings: &AiSettings) -> Option<EmbedTarget> {
    let provider = embedding_provider(settings)?;
    let model = provider.embedding_model.trim();
    // A gateway hands what it is sent to the internet: like a provider elsewhere, only the mail of
    // the accounts the owner allowed it.
    let accounts = if provider.kind.mail_leaves_this_computer() {
        let allowed: Vec<String> = provider.consented_accounts.iter().map(|a| a.trim().to_lowercase()).filter(|a| !a.is_empty()).collect();
        if allowed.is_empty() {
            return None;
        }
        Some(allowed)
    } else {
        None
    };
    Some(EmbedTarget { provider: provider.id.clone(), model: model.to_string(), accounts })
}

/// Embeds `job`'s texts with the provider it was asked for (which must still be the embedding
/// provider, with the same embeddings model), its key from `store`: one vector a text, in order,
/// or why not in our words. With no embedding provider at all, nothing is sent and the batch says
/// so ([`NO_EMBEDDING_RUNTIME`]).
pub(crate) fn run_batch(job: &EmbedJob, settings: &Shared, store: Option<&Arc<dyn SecretStore>>) -> Result<Vec<Vec<f32>>, String> {
    let snapshot = settings.snapshot();
    let provider = embedding_provider(&snapshot).ok_or_else(|| NO_EMBEDDING_RUNTIME.to_string())?;
    let config = Some(provider)
        .filter(|p| p.id == job.provider && p.embedding_model.trim() == job.model)
        .cloned()
        .ok_or_else(|| "the model provider changed".to_string())?;
    let address = shown_address(&config.base_url);
    // Before anything is sent, and before the key is read: a "local" server that is a gateway gets
    // no mail.
    route_allows(&config, settings, &job.cancel).map_err(|failure| failure.message)?;
    // Only a key the settings note as kept for this address is read, and never for long.
    let kept = snapshot.kept_key(&config.id);
    let config =
        keys::with_key(config, kept, Broker::global(), store).map_err(|_| format!("the key of the model provider at {address} couldn't be read"))?;
    let accounts: Vec<&str> = job.accounts.iter().map(String::as_str).collect();
    let started = Instant::now();
    let got = mailcraft_ai::embed(&config, Carries::MailOf(&accounts), &job.texts, &job.cancel).map_err(|e| said(&e, &address, started.elapsed()))?;
    Ok(got.vectors)
}

/// The vector of the owner's question with the embedding provider's model, and the name of
/// that model; none when there is none, or it failed (the question is then answered without it).
pub(crate) fn query(text: &str, settings: &Shared, store: Option<&Arc<dyn SecretStore>>) -> Option<(Vec<f32>, String)> {
    let snapshot = settings.snapshot();
    // Only where the passages may be embedded: an embeddings model the owner named, and for a
    // provider elsewhere an account it was allowed.
    let model = target(&snapshot)?.model;
    let config = embedding_provider(&snapshot).cloned()?;
    // (The window doesn't wait for long; the provider's own waits end it.)
    let never = AtomicBool::new(false);
    // The owner's words go nowhere the route check refuses either.
    route_allows(&config, settings, &never).ok()?;
    let kept = snapshot.kept_key(&config.id);
    let config = keys::with_key(config, kept, Broker::global(), store).ok()?;
    let mut got = mailcraft_ai::embed(&config, Carries::OwnerTextOnly, &[text.to_string()], &never).ok()?;
    let vector = got.vectors.pop()?;
    Some((vector, model))
}
