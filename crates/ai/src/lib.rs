// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Talking to a model provider the owner set up (#ai): OpenAI-compatible chat, streamed or not,
//! embeddings, and structured output (`response_format: json_schema`). Transport only: what to
//! ask and what to do with the answer are the engine's (`ai.request`, `ai.apply`), and the host
//! runs these calls on a thread of its own and maps between their types and the engine's.
//!
//! The rules:
//! - **Nothing is sent until the owner has configured a provider**, and nothing here runs by
//!   itself. A [`ProviderKind::Local`] provider must be on this computer; a
//!   [`ProviderKind::Remote`] one is spoken to over TLS alone; a [`ProviderKind::Gateway`] is on
//!   this computer but hands what it is sent to the internet. Remote providers and gateways are
//!   sent mail only of the accounts the owner consented for ([`ProviderConfig::may_send`],
//!   [`Carries`]). A server set up as on this computer whose model list names a hosted service's
//!   route is sent nothing ([`models::route_of`]).
//! - **Encrypt or refuse.** Plain HTTP goes only to a loopback address, checked on the address
//!   actually connected to; a remote provider's certificate may be pinned by its SHA-256
//!   fingerprint. Redirects are never followed. (`mailcraft_net::https::stream`.)
//! - **Keys** ([`ApiKey`]) live in memory and in the system's credential store, never in a file,
//!   a log, a command's params, an address or an error's text: `#[serde(skip)]`, redacted from
//!   `Debug`, scrubbed from what a server writes back.
//! - **Every answer is hostile.** Sizes are capped, lengths are never trusted, nothing panics.
//!   A model's answer is text to show (or JSON to check), never instructions or a command.
//! - Calls block, with deadlines (connecting, the first byte, a silence, the whole) and a stop
//!   flag looked at between reads, so a stop closes the connection within a second. There is no
//!   async runtime and no automatic retry: the host decides whether to ask again.
//!
//! On the web the configuration and wire types are here, and the calls are not: a browser can't
//! open sockets.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod config;
pub mod error;
pub mod key;
pub mod models;
pub mod on_device;
pub mod runtime;
pub mod structured;
pub mod wire;

#[cfg(not(target_arch = "wasm32"))]
mod client;

pub use config::{
    Carries, MAX_CONTEXT, MIN_CONTEXT, ProviderConfig, ProviderKind, Timeouts, clamp_context_window, default_context_window, is_loopback_host,
    valid_id,
};
pub use error::AiError;
pub use key::ApiKey;
pub use models::{ModelInfo, ModelList, Route, fingerprint, is_fingerprint, parse_models, route_of};
pub use on_device::{APPLE_MODEL, Health, parse_health};
pub use structured::{Ladders, Level};
pub use wire::{ChatAnswer, ChatRequest, Chunk, Embeddings, Message, ResponseFormat, Role, Usage, json_answer};

#[cfg(not(target_arch = "wasm32"))]
pub use client::{chat, chat_json, chat_stream, chat_stream_json, deadlines, embed, health, models};

/// Whether this build can talk to a model provider.
pub const AVAILABLE: bool = cfg!(not(target_arch = "wasm32"));

#[cfg(all(test, not(target_arch = "wasm32")))]
mod client_tests;
#[cfg(test)]
mod config_tests;
#[cfg(test)]
mod key_tests;
#[cfg(test)]
mod models_tests;
#[cfg(test)]
mod on_device_tests;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod stream_json_tests;
#[cfg(test)]
mod structured_tests;
#[cfg(test)]
mod wire_tests;
