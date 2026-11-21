// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The AI runtimes of the chain: what is pure and builds everywhere (wasm included). Integrator-owned
//! registry (plan: crates/ai/src/runtime/mod.rs); each adapter lives in a file of its own.

pub mod claude_code;
pub mod codex;
pub mod contain;
pub mod detect;
pub mod responses;
pub mod siwc;
pub mod uar;
