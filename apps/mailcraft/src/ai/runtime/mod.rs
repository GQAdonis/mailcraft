// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The host side of the AI runtimes: finding them, spawning them contained, sandboxing them.
//! Integrator-owned module list.

pub(crate) mod claude_code;
pub(crate) mod codex;
pub(crate) mod detect;
pub(crate) mod sandbox;
pub(crate) mod siwc;
pub(crate) mod spawn;
pub(crate) mod terminal;
pub(crate) mod uar_sidecar;

#[cfg(test)]
mod argv_tests;
#[cfg(test)]
mod chain_tests;
#[cfg(test)]
mod claude_auth_tests;
#[cfg(test)]
mod codex_tests;
#[cfg(test)]
mod contain_tests;
#[cfg(test)]
mod detect_tests;
#[cfg(test)]
mod env_tests;
#[cfg(test)]
mod path_tests;
#[cfg(test)]
mod sandbox_tests;
#[cfg(test)]
mod siwc_refresh_tests;
#[cfg(test)]
pub(crate) mod siwc_tests;
#[cfg(test)]
mod uar_tests;
// Each adapter's stream fixtures for SCN-RT-CONTAIN-001; the integrator adds them to contain_tests::registry().
#[cfg(test)]
pub(crate) mod claude_fixtures;
#[cfg(test)]
pub(crate) mod codex_fixtures;
#[cfg(test)]
pub(crate) mod uar_fixtures;
