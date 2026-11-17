// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Apple's on-device model, through apfel on this computer (#ai, ai-provider-chain 5.2): found,
//! confirmed once by the owner, re-checked, and only then used as the backup.
//!
//! - **Looking** ([`discover_on`]) asks each port to look on (11434, apfel's default, and those
//!   the owner added) two questions: `GET /v1/models`, then, of a server that lists exactly
//!   `apple-foundationmodel`, `GET /health`. Loopback alone, no body, no key, no mail. It runs only
//!   when Options opens or on a first question (the window asks for it), never in the
//!   background, and never while the backup is None (owner decision 2).
//! - **Candidates** are listed for the owner and sent nothing more. A server that lists other
//!   models, or whose `/health` isn't apfel's, is no candidate and isn't listed.
//! - **Confirming** ([`confirm`]) one specific address the look found binds that address to the
//!   identity its answers give now ([`super::settings::OnDeviceBinding`]). Only a confirmed
//!   address is ever sent a request.
//! - **Re-checking** ([`ready_backup`]): before its first use in each run, and after any error,
//!   the confirmed address is asked the same two questions again. A different answer stops its
//!   use (the card says "changed") until the owner confirms it again; a server that can't be
//!   reached just isn't used this time.

use std::sync::atomic::AtomicBool;

use mailcraft_ai::on_device::{APPLE_MODEL, context_window_of, identity, lists_only_apple};
use mailcraft_ai::{AiError, ProviderConfig, ProviderKind, Timeouts, default_context_window};
use serde_json::{Value, json};

use super::settings::{Backup, Candidate, OnDeviceBinding, Shared, device_address, device_port};

/// The provider id the backup answers under ("Answered by …").
pub(crate) const PROVIDER_ID: &str = "apple-on-device";
/// Whether this build can have Apple's on-device model (apfel runs on macOS alone).
pub(crate) const PLATFORM: bool = cfg!(target_os = "macos");
/// The waits of a look: short, since the server is on this computer.
const LOOK_WAITS: Timeouts = Timeouts { connect_secs: 2, first_byte_secs: 3, idle_secs: 3, total_secs: 5 };

/// The provider the on-device model at `address` is spoken to as: on this computer, its one
/// model, no key, its context window `window`.
pub(crate) fn provider_at(address: &str, window: u32, timeouts: Timeouts) -> ProviderConfig {
    ProviderConfig {
        id: PROVIDER_ID.into(),
        kind: ProviderKind::Local,
        base_url: address.to_string(),
        model: APPLE_MODEL.into(),
        timeouts,
        context_window: window,
        ..ProviderConfig::default()
    }
}

/// What one look at `address` found: an apfel server (a candidate), something else (none), or
/// no answer at all (an error: unreachable, too slow, stopped).
pub(crate) fn probe(address: &str, cancel: &AtomicBool) -> Result<Option<Candidate>, AiError> {
    if device_port(address).is_none() {
        return Ok(None);
    }
    let asked = provider_at(address, default_context_window(ProviderKind::Local), LOOK_WAITS);
    let list = match mailcraft_ai::models(&asked, cancel) {
        Ok(list) => list,
        // It answered, but not with a list: not apfel.
        Err(AiError::Status { .. } | AiError::Provider(_) | AiError::Malformed(_) | AiError::TooLarge) => return Ok(None),
        Err(error) => return Err(error),
    };
    if !lists_only_apple(&list) {
        return Ok(None);
    }
    let health = match mailcraft_ai::health(&asked, cancel) {
        Ok(health) => health,
        Err(AiError::Status { .. } | AiError::Provider(_) | AiError::Malformed(_) | AiError::TooLarge) => return Ok(None),
        Err(error) => return Err(error),
    };
    Ok(identity(&list, &health).map(|identity| Candidate {
        address: address.to_string(),
        model: APPLE_MODEL.into(),
        context_window: context_window_of(&list, &health),
        identity,
    }))
}

/// Look on `ports` of this computer (see the module's notes), and what the window shows now.
/// Nothing is asked while the backup is None, or when the primary is the owner's None.
pub(crate) fn discover_on(settings: &Shared, ports: &[u16], cancel: &AtomicBool) -> Value {
    let (backup, binding) = settings.read(|s| (if s.explicitly_none() { Backup::None } else { s.backup }, s.on_device.clone()));
    if backup == Backup::None {
        return state(settings, true);
    }
    settings.device_edit(|d| d.looking = true);
    let mut candidates = Vec::new();
    let mut confirmed_answer: Option<Result<Option<Candidate>, ()>> = None;
    for port in ports.iter().copied().filter(|p| *p > 0) {
        let address = device_address(port);
        let found = probe(&address, cancel);
        if binding.as_ref().is_some_and(|b| b.address == address) {
            confirmed_answer = Some(found.clone().map_err(|_| ()));
        }
        if let Ok(Some(candidate)) = found {
            candidates.push(candidate);
        }
    }
    settings.device_edit(|d| {
        d.looking = false;
        d.looked = true;
        d.candidates = candidates;
        if let (Some(binding), Some(answer)) = (&binding, confirmed_answer) {
            match answer {
                Ok(Some(found)) if found.identity == binding.identity => {
                    d.verified = Some(binding.address.clone());
                    d.changed = None;
                }
                Ok(_) => {
                    d.verified = None;
                    d.changed = Some(binding.address.clone());
                }
                // No answer: it is checked again before it is used.
                Err(()) => d.verified = None,
            }
        }
    });
    state(settings, true)
}

/// The on-device model card's state, as the window reads it:
/// `{"state", "candidates": [{"address", "model", "contextWindow"?}], "confirmed": address|null}`.
/// `platform`: whether this build can have the on-device model at all.
pub(crate) fn state(settings: &Shared, platform: bool) -> Value {
    let (backup, binding) = settings.read(|s| (if s.explicitly_none() { Backup::None } else { s.backup }, s.on_device.clone()));
    let device = settings.device();
    let candidates: Vec<Value> = device
        .candidates
        .iter()
        .map(|c| {
            let mut shown = json!({"address": c.address, "model": c.model});
            if let (Some(window), Some(fields)) = (c.context_window, shown.as_object_mut()) {
                fields.insert("contextWindow".into(), json!(window));
            }
            shown
        })
        .collect();
    let confirmed = binding.as_ref().map(|b| b.address.clone());
    let named = if !platform {
        "unavailable"
    } else if backup == Backup::None {
        "off"
    } else if device.looking {
        "looking"
    } else if let Some(binding) = &binding {
        if device.changed.as_deref() == Some(binding.address.as_str()) { "changed" } else { "using" }
    } else if !device.candidates.is_empty() {
        "found"
    } else if device.looked {
        "notFound"
    } else {
        "looking"
    };
    let candidates = if named == "off" || named == "unavailable" { Vec::new() } else { candidates };
    json!({"state": named, "candidates": candidates, "confirmed": confirmed})
}

/// The owner's Use It for `address`: one the last look found (or the confirmed one that changed),
/// asked again now, and bound to what it answers. Refused when the backup isn't the on-device
/// model, when the address wasn't found, or when it no longer answers as apfel.
pub(crate) fn confirm(settings: &Shared, address: &str, cancel: &AtomicBool) -> Result<(), String> {
    let address = address.trim();
    let usable = settings.read(|s| !s.explicitly_none() && s.backup == Backup::AppleOnDevice);
    if !usable {
        return Err("the backup isn't Apple's on-device model".into());
    }
    let device = settings.device();
    let known = device.candidates.iter().any(|c| c.address == address)
        || device.changed.as_deref() == Some(address)
        || settings.read(|s| s.on_device.as_ref().is_some_and(|b| b.address == address));
    if !known || device_port(address).is_none() {
        return Err("MailCraft didn't find an on-device model at that address".into());
    }
    let found = match probe(address, cancel) {
        Ok(Some(found)) => found,
        Ok(None) => return Err("it no longer answers as Apple's on-device model".into()),
        Err(_) => return Err("nothing answered at that address".into()),
    };
    let binding = OnDeviceBinding {
        address: found.address.clone(),
        identity: found.identity.clone(),
        context_window: found.context_window.unwrap_or_else(|| default_context_window(ProviderKind::Local)),
    };
    settings.change(|s| {
        if s.explicitly_none() || s.backup != Backup::AppleOnDevice {
            return Err("the backup isn't Apple's on-device model".to_string());
        }
        s.on_device = Some(binding);
        Ok(())
    })?;
    settings.device_edit(|d| {
        d.verified = Some(found.address.clone());
        d.changed = None;
    });
    Ok(())
}

/// The owner's Stop Using It: the confirmation is taken back (the backup stays the on-device
/// model, which is looked for again and used only once confirmed again).
pub(crate) fn stop_using(settings: &Shared) -> Result<(), String> {
    settings.change(|s| {
        s.on_device = None;
        Ok(())
    })?;
    settings.device_edit(|d| {
        d.verified = None;
        d.changed = None;
    });
    Ok(())
}

/// The confirmed on-device model as the backup, ready to be asked, with its context window; none
/// when there is no backup, no confirmation, or its identity changed (see the module's notes).
pub(crate) fn ready_backup(settings: &Shared, timeouts: Timeouts, cancel: &AtomicBool) -> Option<(ProviderConfig, u32)> {
    let binding = settings.read(|s| s.backup_binding().cloned())?;
    let device = settings.device();
    if device.changed.as_deref() == Some(binding.address.as_str()) {
        return None;
    }
    let mut window = binding.context_window;
    if device.verified.as_deref() != Some(binding.address.as_str()) {
        match probe(&binding.address, cancel) {
            Ok(Some(found)) if found.identity == binding.identity => {
                window = found.context_window.unwrap_or(window);
                settings.device_edit(|d| d.verified = Some(binding.address.clone()));
            }
            Ok(_) => {
                log::warn!("the confirmed on-device model no longer answers as it did; it isn't used");
                settings.device_edit(|d| {
                    d.verified = None;
                    d.changed = Some(binding.address.clone());
                });
                return None;
            }
            // Not there now: checked again next time, and not changed.
            Err(_) => return None,
        }
    }
    Some((provider_at(&binding.address, window, timeouts), window))
}

/// The backup failed: its identity is checked again before its next use.
pub(crate) fn after_error(settings: &Shared) {
    settings.device_edit(|d| d.verified = None);
}
