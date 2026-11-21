// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Model providers on the desktop (#ai): the providers the owner set up (`ai.json` in the
//! settings folder, never with a key), their keys in the credential store, and the thread each
//! model request runs on. The calls are `mailcraft-ai`'s; the engine only describes a request
//! and takes in its answer, which the window hands it piece by piece (`ai.apply`). When the
//! owner's primary can't answer, the on-device model the owner confirmed may ([`device`]).

pub(crate) mod device;
pub(crate) mod embed;
pub(crate) mod keys;
pub(crate) mod route;
pub(crate) mod run;
pub(crate) mod runtime;
pub(crate) mod runtimes_host;
pub(crate) mod service;
pub(crate) mod settings;
pub(crate) mod settings_migrate;
pub(crate) mod settings_v3;

#[cfg(test)]
mod chain_tests;
#[cfg(test)]
mod embed_tests;
#[cfg(test)]
mod keys_tests;
#[cfg(test)]
mod run_tests;
#[cfg(test)]
mod runtimes_host_tests;
#[cfg(test)]
mod service_tests;
#[cfg(test)]
mod settings_chain_tests;
#[cfg(test)]
mod settings_migrate_tests;
#[cfg(test)]
mod settings_tests;
#[cfg(test)]
mod settings_v3_tests;
