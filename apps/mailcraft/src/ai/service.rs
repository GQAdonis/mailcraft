// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the window is given for models (#ai): the thread each request runs on
//! ([`model_requests`]) and the providers' settings, keys and consents ([`Providers`]).
//!
//! A request runs on a thread of its own, as Send/Receive does, not on the host's job threads: it
//! streams for as long as the model writes, and its pieces go back through a short channel that a
//! busy window holds back. The thread ends when the answer is over, when the window lets go of
//! the channel, or when the job's flag is set (within a second, at the next read).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use mailcraft_ai::{ApiKey, ProviderConfig, ProviderKind, Route, default_context_window, fingerprint, route_of};
use mailcraft_host::SecretStore;
use mailcraft_model::ai::{ErrorKind, ModelEvent, clean_provider_text};
use mailcraft_ui_egui::{AiProviders, EmbedJob, EmbedTarget, ModelJob};
use serde_json::{Value, json};

use super::device;
use super::keys::{self, Broker};
use super::run::{DeskWatch, Failure, STORE_SILENT, desk, failure_of, shown_address};
use super::settings::{AiSettings, Backup, GatewayQuestion, Shared, read_ports, route_address, same_address};

/// How many pieces of an answer may wait for the window before the request waits for it.
const WAITING_PIECES: usize = 64;
/// What is said when the system has no credential store.
const NO_STORE: &str = "this system has no credential store to keep the key in";

/// `Services::model_request`: run each job on a thread of its own down the owner's chain. What the
/// walk tells and asks the owner (a tool-use stop, the `ask` policy's question, the provenance)
/// goes to the window's desk ([`super::run::desk`]), and the window is woken for it.
pub(crate) fn model_requests(
    settings: Shared,
    store: Option<Arc<dyn SecretStore>>,
    ctx: egui::Context,
) -> Box<dyn Fn(ModelJob) -> Receiver<ModelEvent>> {
    Box::new(move |job: ModelJob| {
        let (tx, rx) = std::sync::mpsc::sync_channel(WAITING_PIECES);
        let (settings, store, wake) = (settings.clone(), store.clone(), ctx.clone());
        let spawned = std::thread::Builder::new().name("model request".into()).spawn(move || {
            let cancel = job.cancel.clone();
            let mut watch = DeskWatch::new(desk(), Some(wake.clone()));
            super::run::run_with(&job, &settings, Broker::global(), store.as_ref(), &mut watch, &mut |event: ModelEvent| {
                let sent = tx.send(event).is_ok();
                wake.request_repaint();
                // Nobody is listening any more: the request stops at its next read.
                if !sent {
                    cancel.store(true, Ordering::Relaxed);
                }
                sent
            });
        });
        // A dropped sender reads as a request that stopped before it was finished.
        if let Err(e) = spawned {
            log::error!("couldn't start a model request: {e}");
        }
        rx
    })
}

/// `Services::embed_target`: where the passages are embedded now, read from the settings alone.
pub(crate) fn embed_target(settings: Shared) -> Box<dyn Fn() -> Option<EmbedTarget>> {
    Box::new(move || settings.read(super::embed::target))
}

/// `Services::embed_batch`: each batch on a thread of its own (the window hands out one at a time).
pub(crate) fn embed_batches(
    settings: Shared,
    store: Option<Arc<dyn SecretStore>>,
    ctx: egui::Context,
) -> Box<dyn Fn(EmbedJob) -> Receiver<Result<Vec<Vec<f32>>, String>>> {
    Box::new(move |job: EmbedJob| {
        let (settings, store) = (settings.clone(), store.clone());
        crate::background(&ctx, "embeddings", move || super::embed::run_batch(&job, &settings, store.as_ref()))
    })
}

/// `Services::embed_query`: the vector of the owner's question, on a thread of its own.
pub(crate) fn embed_query(
    settings: Shared,
    store: Option<Arc<dyn SecretStore>>,
    ctx: egui::Context,
) -> Box<dyn Fn(String) -> Receiver<Option<(Vec<f32>, String)>>> {
    Box::new(move |text: String| {
        let (settings, store) = (settings.clone(), store.clone());
        crate::background(&ctx, "question vector", move || super::embed::query(&text, &settings, store.as_ref()))
    })
}

/// `Services::ai_providers`: the settings in `ai.json`, the keys in the credential store.
pub(crate) struct Providers {
    settings: Shared,
    store: Option<Arc<dyn SecretStore>>,
    ctx: egui::Context,
}

impl Providers {
    pub(crate) fn new(settings: Shared, store: Option<Arc<dyn SecretStore>>, ctx: egui::Context) -> Providers {
        Providers { settings, store, ctx }
    }

    /// `work` with the credential store, on a background thread.
    fn with_store<T: Send + 'static>(
        &self,
        name: &str,
        work: impl FnOnce(&dyn SecretStore) -> Result<T, String> + Send + 'static,
    ) -> Receiver<Result<T, String>> {
        let store = self.store.clone();
        crate::background(&self.ctx, name, move || match store {
            Some(store) => work(store.as_ref()),
            None => Err(NO_STORE.to_string()),
        })
    }
}

/// A failure as Test Connection and the card answer it: `{"kind", "message"}`, with
/// `providerText` (the provider's own words, cleaned) when there are some, as text.
fn refusal(failure: &Failure) -> String {
    refusal_naming(failure, None)
}

/// [`refusal`], with `refusedModel`: the hosted model id that made a server set up as on this
/// computer refused (level 1 of the route check), when that was why.
fn refusal_naming(failure: &Failure, refused_model: Option<&str>) -> String {
    let mut said = json!({"kind": failure.kind, "message": failure.message});
    if let Some(fields) = said.as_object_mut() {
        if let Some(text) = &failure.provider_text {
            fields.insert("providerText".into(), json!(text));
        }
        if let Some(model) = refused_model {
            fields.insert("refusedModel".into(), json!(model));
        }
    }
    said.to_string()
}

/// Whether the owner's answer about `provider` and `models` is an answer to `question`: the same
/// provider, the same list (its fingerprint), and the provider still set up at the address the
/// question was about, on this computer. An answer never applies to a list the owner didn't see.
pub(crate) fn answer_matches(question: &GatewayQuestion, settings: &AiSettings, provider: &str, models: &str) -> bool {
    question.provider == provider
        && question.models.eq_ignore_ascii_case(models.trim())
        && settings.provider(provider).is_some_and(|p| p.kind == ProviderKind::Local && route_address(p) == question.route_address)
}

/// Test Connection for `incoming` (as typed in Options): its model list (`GET /v1/models`, no
/// mail), with the key kept for it only when it is the provider as kept (same id, address and
/// kind: a key is never sent to an address it wasn't given for). `{"models": [{"id",
/// "contextWindow"?}], "contextWindow"?}` (the window of its model, as the server says it), or
/// the failure ([`refusal`]). A server set up as on this computer gets the route check: a hosted
/// service's route is refused, several publishers put the gateway question to the owner (without
/// waiting for the answer).
pub(crate) fn test_connection(
    incoming: ProviderConfig,
    settings: &Shared,
    store: Option<&Arc<dyn SecretStore>>,
    broker: &Broker,
) -> Result<Value, String> {
    let incoming = ProviderConfig { api_key: None, ..incoming };
    let address = shown_address(&incoming.base_url);
    incoming.check().map_err(|e| refusal(&failure_of(&e, &address, Duration::ZERO, false)))?;
    let snapshot = settings.snapshot();
    let kept_as_is =
        snapshot.provider(&incoming.id).is_some_and(|kept| same_address(&kept.base_url, &incoming.base_url) && kept.kind == incoming.kind);
    let config = if kept_as_is {
        let kept = snapshot.key_kept(&incoming.id);
        keys::with_key(incoming, kept, broker, store).map_err(|_| refusal(&Failure::new(ErrorKind::CredentialStore, STORE_SILENT)))?
    } else {
        incoming
    };
    let started = Instant::now();
    let list = mailcraft_ai::models(&config, &AtomicBool::new(false)).map_err(|e| refusal(&failure_of(&e, &address, started.elapsed(), false)))?;
    if config.kind == ProviderKind::Local {
        match route_of(&list) {
            Route::Local => {}
            Route::Hosted { listed } => {
                // The model whose id (or owner) said so: its id, as the server wrote it, cleaned.
                let model =
                    list.models.iter().find(|m| m.id == listed || m.owned_by.as_deref() == Some(listed.as_str())).map(|m| clean_provider_text(&m.id));
                let failure = Failure {
                    kind: ErrorKind::Settings,
                    message: format!("{address} lists models of services on the internet, so it can't be used as a model on this computer."),
                    provider_text: Some(clean_provider_text(&listed)),
                };
                return Err(refusal_naming(&failure, model.as_deref()));
            }
            Route::Ask { publishers } => {
                let models = fingerprint(&list);
                let route = route_address(&config);
                if kept_as_is && !settings.read(|s| s.is_local_list(&route, &models)) {
                    let _ = settings.ask_gateway(GatewayQuestion {
                        provider: config.id.clone(),
                        address: address.clone(),
                        models,
                        publishers: publishers.iter().map(|p| clean_provider_text(p)).collect(),
                        route_address: route,
                    });
                }
            }
        }
    }
    let window = list.find(&config.model).and_then(|m| m.context_window);
    if let (true, Some(window)) = (kept_as_is, window) {
        settings.keep_window(&config, window);
    }
    let models: Vec<Value> = list
        .models
        .iter()
        .map(|m| match m.context_window {
            Some(window) => json!({"id": m.id, "contextWindow": window}),
            None => json!({"id": m.id}),
        })
        .collect();
    let mut answer = json!({"models": models});
    if let (Some(window), Some(fields)) = (window, answer.as_object_mut()) {
        fields.insert("contextWindow".into(), json!(window));
    }
    Ok(answer)
}

impl AiProviders for Providers {
    /// The settings (version 3, as `ai.json` holds them), with what the window reads as it did
    /// before the chain: `primary` and `active` (the first enabled provider of the chain),
    /// `backup`, and each provider's `consented_accounts` (from the consents); for each provider
    /// the context window used for it (`contextWindow`, with `contextWindowSource`: `owner`,
    /// `server` or `default`); `neverConfigured`; and (as `gatewayQuestion`, never saved) the
    /// gateway question waiting for the owner, when there is one. Never the detection cache or the
    /// ChatGPT plan's host id.
    fn providers(&self) -> Value {
        let snapshot = self.settings.snapshot();
        let mut shown = snapshot.to_json();
        if let Some(listed) = shown.get_mut("providers").and_then(Value::as_array_mut) {
            for entry in listed.iter_mut() {
                let id = entry.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
                let (Some(config), Some(fields)) = (snapshot.provider(&id), entry.as_object_mut()) else { continue };
                let (window, source) = self.settings.window_of(config);
                fields.insert("consented_accounts".into(), json!(config.consented_accounts));
                fields.insert("contextWindow".into(), json!(window));
                fields.insert("contextWindowSource".into(), json!(source));
            }
        }
        if let Some(fields) = shown.as_object_mut() {
            // Not for the window or an agent on the control channel: where a runtime's program
            // was found (a path under the owner's home) and the ChatGPT plan's host id.
            fields.remove("detected");
            fields.remove("chatgpt");
            fields.insert("primary".into(), json!(snapshot.active));
            fields.insert("active".into(), json!(snapshot.active));
            fields.insert("neverConfigured".into(), json!(snapshot.never_configured));
            fields.insert("backup".into(), json!(snapshot.backup.name()));
            if let Some(question) = self.gateway_question() {
                fields.insert("gatewayQuestion".into(), question);
            }
        }
        shown
    }

    fn set_provider(&self, provider: &Value) -> Result<(), String> {
        let mut incoming: ProviderConfig =
            serde_json::from_value(provider.clone()).map_err(|_| "that isn't a model provider's settings".to_string())?;
        // No window said: the default of its kind (4,096 on this computer, 32,768 elsewhere).
        if provider.get("context_window").is_none() {
            incoming.context_window = default_context_window(incoming.kind);
        }
        let id = incoming.id.clone();
        let moved = self.settings.change(|settings| settings.set_provider(incoming))?;
        if moved {
            // The key was for the old address: it is not used from now on (withdrawn above), and
            // goes out of the store; the owner is asked for one again.
            Broker::global().forget_late(&id);
            let settings = self.settings.clone();
            let _ = self.with_store("model provider key", move |store| {
                keys::forget(store, &id).map_err(|e| e.to_string())?;
                settings.change(|s| {
                    s.unnote_key(&id);
                    Ok(())
                })
            });
        }
        Ok(())
    }

    fn remove_provider(&self, id: &str) -> Result<(), String> {
        let removed = self.settings.change(|settings| Ok(settings.remove(id)))?;
        if !removed {
            return Err("no model provider of that name is set up".into());
        }
        // Its key goes with it (on a thread of its own: the store may take a while); what came
        // of that isn't waited for.
        Broker::global().forget_late(id);
        let (id, settings) = (id.to_string(), self.settings.clone());
        let _ = self.with_store("model provider key", move |store| {
            keys::forget(store, &id).map_err(|e| e.to_string())?;
            settings.change(|s| {
                s.unnote_key(&id);
                Ok(())
            })
        });
        Ok(())
    }

    fn set_active(&self, id: &str) -> Result<(), String> {
        self.settings.change(|settings| settings.set_active(id))
    }

    fn set_consent(&self, id: &str, account: &str, allowed: bool) -> Result<(), String> {
        self.settings.change(|settings| settings.set_consent(id, account, allowed))
    }

    fn save_key(&self, id: &str, key: String) -> Receiver<Result<(), String>> {
        // A key that came late for an earlier request is not the one the owner gives now.
        Broker::global().forget_late(id);
        let (id, settings) = (id.to_string(), self.settings.clone());
        self.with_store("model provider key", move |store| {
            // (Neither error repeats the key.)
            let key = ApiKey::new(&key).map_err(|e| e.to_string())?;
            // Noted first (the name alone), so that Remove MailCraft's data finds it even if
            // what follows stops half way.
            settings.change(|s| {
                s.note_key(&id);
                Ok(())
            })?;
            keys::save(store, &id, &key).map_err(|e| e.to_string())?;
            // A key given now is for the provider's address now, and is the one used: a key still
            // under its version 2 name is no longer moved over it (it is taken away with this one).
            settings.change(|s| {
                s.restore_key(&id);
                s.drop_legacy_key(&id);
                Ok(())
            })
        })
    }

    fn forget_key(&self, id: &str) -> Receiver<Result<(), String>> {
        Broker::global().forget_late(id);
        let (id, settings) = (id.to_string(), self.settings.clone());
        self.with_store("model provider key", move |store| {
            keys::forget(store, &id).map_err(|e| e.to_string())?;
            settings.change(|s| {
                s.unnote_key(&id);
                Ok(())
            })
        })
    }

    fn has_key(&self, id: &str) -> Receiver<Result<bool, String>> {
        // Through the one credential thread, like a request's read: never a thread of its own
        // waiting on a store that doesn't answer.
        // A key that comes after this look gave up is kept for a request only when the settings
        // note a usable key for the provider (and only while its generation stands).
        let (id, store, settings) = (id.to_string(), self.store.clone(), self.settings.clone());
        crate::background(&self.ctx, "model provider key", move || match store {
            Some(store) => {
                let kept = settings.read(|s| s.kept_key(&id));
                let (keep_late, legacy) = (kept != super::keys::Kept::No, kept == super::keys::Kept::Either);
                Broker::global().read_kept(&store, &id, keep_late, legacy).map(|key| key.is_some()).map_err(|e| e.to_string())
            }
            None => Err(NO_STORE.to_string()),
        })
    }

    /// The gateway question waiting for the owner's answer, as the window shows it:
    /// `{"provider", "address", "models", "publishers"}`, or none.
    fn gateway_question(&self) -> Option<Value> {
        self.settings.gateway_question().and_then(|question| serde_json::to_value(question).ok())
    }

    /// The owner's answer to the question waiting about `provider` and the model list `models`
    /// (the fingerprint the window was shown, `gateway_question()["models"]`): whether that
    /// server sends mail to the internet. "Yes" makes it a gateway (no account's mail goes until
    /// allowed); "no" is kept for that address and that list ([`answer_matches`]).
    fn answer_gateway(&self, provider: &str, models: &str, sends_to_internet: bool) -> Result<(), String> {
        let snapshot = self.settings.snapshot();
        let question = self.settings.gateway_question();
        if !question.as_ref().is_some_and(|q| answer_matches(q, &snapshot, provider, models)) {
            return Err("that question isn't being asked".into());
        }
        self.settings.answer_gateway(provider, models, sends_to_internet)
    }

    /// The owner closed the question without an answer: nothing is sent, the next request asks again.
    fn dismiss_gateway_question(&self) {
        self.settings.dismiss_gateway_question();
    }

    /// The backup from now on: "" (none) or "apple-on-device". None forgets the confirmed address.
    fn set_backup(&self, backup: &str) -> Result<(), String> {
        let backup = Backup::parse(backup).ok_or_else(|| "that isn't a backup MailCraft knows".to_string())?;
        self.settings.change(|s| s.set_backup(backup))?;
        if backup == Backup::None {
            self.settings.device_edit(|d| {
                d.verified = None;
                d.changed = None;
                d.candidates.clear();
            });
        }
        Ok(())
    }

    /// Test Connection ([`test_connection`]), on a thread of its own.
    fn test_provider(&self, provider: &Value) -> Receiver<Result<Value, String>> {
        let parsed = serde_json::from_value::<ProviderConfig>(provider.clone()).ok();
        let (settings, store) = (self.settings.clone(), self.store.clone());
        crate::background(&self.ctx, "test connection", move || {
            let Some(config) = parsed else { return Err(refusal(&Failure::new(ErrorKind::Settings, "That isn't a model provider's settings."))) };
            test_connection(config, &settings, store.as_ref(), Broker::global())
        })
    }

    /// The on-device model card's state ([`device::state`]).
    fn on_device(&self) -> Value {
        device::state(&self.settings, device::PLATFORM)
    }

    /// A look for the on-device model now, on 11434 and the owner's ports (nothing is asked while
    /// the backup is None, or on a system without one), on a thread of its own.
    fn discover_on_device(&self) -> Receiver<Result<Value, String>> {
        let settings = self.settings.clone();
        crate::background(&self.ctx, "on-device model", move || {
            if !device::PLATFORM {
                return Ok(device::state(&settings, false));
            }
            let ports = settings.read(|s| s.ports_to_look_on());
            Ok(device::discover_on(&settings, &ports, &AtomicBool::new(false)))
        })
    }

    /// The owner's Use It for one address the look found ([`device::confirm`]).
    fn confirm_on_device(&self, address: &str) -> Receiver<Result<(), String>> {
        let (settings, address) = (self.settings.clone(), address.to_string());
        crate::background(&self.ctx, "on-device model", move || device::confirm(&settings, &address, &AtomicBool::new(false)))
    }

    /// The owner's Look on ports field, as typed ([`Providers::set_device_ports`]).
    fn set_device_ports(&self, typed: &str) -> Result<(), String> {
        Providers::set_device_ports(self, typed)
    }

    /// The owner's Stop Using It ([`Providers::stop_using_on_device`]).
    fn stop_using_on_device(&self) -> Result<(), String> {
        Providers::stop_using_on_device(self)
    }

    /// The fallback question waiting for the owner ([`Providers::fallback_question`]).
    fn fallback_question(&self) -> Option<Value> {
        Providers::fallback_question(self)
    }

    /// The window's own Try It / Don't buttons ([`Providers::answer_fallback`]).
    fn answer_fallback(&self, request: u64, yes: bool) -> Result<(), String> {
        Providers::answer_fallback(self, request, yes)
    }

    /// The question closed unanswered ([`Providers::dismiss_fallback_question`]).
    fn dismiss_fallback_question(&self) {
        Providers::dismiss_fallback_question(self);
    }

    /// Who answered and who was passed over ([`Providers::provenance`]).
    fn provenance(&self, request: u64) -> Option<Value> {
        Providers::provenance(self, request)
    }

    /// The runtimes stopped for trying a tool ([`Providers::tool_use_notices`]).
    fn tool_use_notices(&self, request: u64) -> Vec<String> {
        Providers::tool_use_notices(self, request)
    }
}

// The host's side of Look on ports and Stop Using It; `AiProviders` forwards to these.
impl Providers {
    /// The fallback question waiting for the owner (the `ask` policy), as the window reads it.
    pub(crate) fn fallback_question(&self) -> Option<Value> {
        desk().fallback_question().map(|q| q.to_json())
    }

    /// The owner's answer to the fallback question of `request`. Only the window's own buttons
    /// call this: an agent can't answer the question (no command, control method or MCP tool).
    pub(crate) fn answer_fallback(&self, request: u64, yes: bool) -> Result<(), String> {
        desk().answer_fallback(request, yes)
    }

    /// The fallback question closed without an answer: nothing more is tried.
    pub(crate) fn dismiss_fallback_question(&self) {
        desk().dismiss_fallback();
    }

    /// The provenance of `request` once its walk is over: who answered and every runtime passed
    /// over or failed before it, with the reason ([`super::route::Provenance::to_json`]).
    pub(crate) fn provenance(&self, request: u64) -> Option<Value> {
        desk().provenance(request).map(|p| p.to_json())
    }

    /// The runtimes stopped for trying to use a tool while `request` ran, in order.
    pub(crate) fn tool_use_notices(&self, request: u64) -> Vec<String> {
        desk().tool_use_notices(request).iter().map(ToString::to_string).collect()
    }

    /// The owner's Stop Using It: the on-device model's confirmation is taken back.
    pub(crate) fn stop_using_on_device(&self) -> Result<(), String> {
        device::stop_using(&self.settings)
    }

    /// The ports the owner adds to look on, as typed ("11437, 11500"): at most eight, each from 1
    /// to 65535; refused (and nothing kept) otherwise.
    pub(crate) fn set_device_ports(&self, typed: &str) -> Result<(), String> {
        let bad = || "Write port numbers from 1 to 65535, separated by commas.".to_string();
        let listed: Vec<Value> = typed
            .split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(|p| {
                if p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()) { p.parse::<u64>().map(Value::from).map_err(|_| bad()) } else { Err(bad()) }
            })
            .collect::<Result<_, _>>()?;
        let ports = read_ports(Some(&Value::Array(listed))).ok_or_else(bad)?;
        self.settings.change(|s| {
            s.device_ports = ports;
            Ok(())
        })
    }
}
