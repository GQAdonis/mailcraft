// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `ai.json` of version 1 or 2 to version 3 (spec `AI-SETTINGS-SPEC.md` §3.1), once.
//!
//! | version 2 | version 3 |
//! |---|---|
//! | `primary` set | `compat:<primary>` first, on |
//! | and `backup` `"apple-on-device"` | `on_device` second, on (never moved first: that is the owner's one-time question, `on_device_first`) |
//! | and `backup` naming another provider | `compat:<backup>` second, on |
//! | and no backup (absent, null, `""`, unknown) | no backup entry |
//! | `primary` the owner's explicit None, **and** `backup` naming a provider | `compat:<backup>` first, on; every other provider's entry off (this row wins over the next) |
//! | `primary` the owner's explicit None, no provider backup | every provider's entry, off; `never_configured: false` |
//! | `never_configured: true` | `never_configured: true`; `on_device` on when it is the backup (as with no file), and nothing else |
//! | a provider's `consented_accounts` | `consents[account] += "compat:<id>"` (an address that isn't one is dropped) |
//! | `keys_kept_for: [<id>]` | `ai-key:<id>`: the key is still under its version 2 name until [`rename_keys`] moves it |
//!
//! Reading a file never writes anything; [`super::settings::Shared::open`] keeps the original as
//! `ai.v2.json` first and only then writes version 3 in its place. A file that can't be read is no
//! settings at all, as before.
//!
//! **Keys.** Each key under `ai-key:<id>` is copied to `ai.compat:<id>`, read back, and only then
//! taken away from its old name ([`super::keys::move_legacy`]); `keys_kept_for` names both until
//! the old one is gone, and a key whose copy didn't read back stays under its old name and is still
//! used ([`super::keys::load`] falls back to it). This needs the credential store, which may wait
//! for the owner, so it runs on a thread of its own ([`spawn_rename`]), never the window's.

use std::sync::Arc;

use mailcraft_ai::ProviderConfig;
use mailcraft_host::SecretStore;
use mailcraft_model::ai::{Chain, ChainEntry, MAX_CHAIN, RuntimeId};
use serde_json::Value;

use super::keys::{Moved, move_legacy};
use super::settings::{APPLE_ON_DEVICE, AiSettings, Backup, NOTED, Shared};

/// The settings a file of version 1 or 2 said, as version 2 read them: the primary, the backup,
/// never configured, and the providers with their consents. Its noted keys are under their old
/// names.
fn read_v2(value: &Value) -> AiSettings {
    let mut settings = AiSettings::empty();
    // A file of version 1 says nothing of the backup or of never having been configured.
    let first_version = value.get("never_configured").is_none();
    settings.read_common(value, first_version);
    let primary = value.get("primary").or_else(|| value.get("active")).and_then(Value::as_str).unwrap_or("");
    if settings.provider(primary).is_some() {
        settings.active = primary.to_string();
    }
    settings.never_configured = match value.get("never_configured").and_then(Value::as_bool) {
        Some(never) => never && settings.active.is_empty(),
        None => settings.active.is_empty() && settings.providers.is_empty(),
    };
    settings.backup = match value.get("backup").and_then(Value::as_str) {
        // A backup this version doesn't know is none: nothing is sent to it.
        Some(name) => Backup::parse(name).unwrap_or(Backup::None),
        None if settings.never_configured => Backup::AppleOnDevice,
        None => Backup::None,
    };
    settings.keep_backup_rule();
    for id in value.get(NOTED).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
        settings.note_legacy_key(id);
    }
    settings
}

/// The provider a version 2 `backup` names, when it names one that is set up (not the on-device
/// model, and not none).
fn provider_backup<'a>(value: &'a Value, settings: &AiSettings) -> Option<&'a str> {
    let name = value.get("backup").and_then(Value::as_str)?.trim();
    (name != APPLE_ON_DEVICE && Backup::parse(name).is_none() && settings.provider(name).is_some()).then_some(name)
}

/// An entry for provider `id`, on or off.
fn compat_entry(id: &str, enabled: bool) -> Option<ChainEntry> {
    AiSettings::compat(id).map(|r| ChainEntry::new(r).with_enabled(enabled))
}

/// The chain of a migrated file, per the table.
fn chain_of(settings: &mut AiSettings, backup: Option<&str>) -> Vec<ChainEntry> {
    if settings.never_configured {
        return if settings.backup == Backup::AppleOnDevice { vec![ChainEntry::new(RuntimeId::OnDevice).with_enabled(true)] } else { Vec::new() };
    }
    let ids: Vec<String> = settings.providers.iter().map(|p| p.id.clone()).collect();
    if settings.active.is_empty() {
        // The owner's None: a provider backup is the one entry on (and so the primary), first.
        let first = backup.map(str::to_string);
        if let Some(id) = &first {
            settings.active.clone_from(id);
        }
        let on = first.iter().filter_map(|id| compat_entry(id, true));
        let off = ids.iter().filter(|id| Some(*id) != first.as_ref()).filter_map(|id| compat_entry(id, false));
        return on.chain(off).collect();
    }
    let mut entries: Vec<ChainEntry> = compat_entry(&settings.active, true).into_iter().collect();
    match backup {
        Some(id) if id != settings.active => entries.extend(compat_entry(id, true)),
        Some(_) => {}
        None if settings.backup == Backup::AppleOnDevice => entries.push(ChainEntry::new(RuntimeId::OnDevice).with_enabled(true)),
        None => {}
    }
    entries
}

/// The version 3 settings a file of version 1 or 2 (`value`, an object) says: every row of the
/// table. Nothing is written.
pub(crate) fn migrate(value: &Value) -> AiSettings {
    let mut settings = read_v2(value);
    let backup = provider_backup(value, &settings).map(str::to_string);
    let entries = chain_of(&mut settings, backup.as_deref());
    settings.chain = Chain::new(entries.into_iter().take(MAX_CHAIN)).unwrap_or_default();
    // Each provider's consents become its runtime's; an address that isn't one is dropped.
    let providers: Vec<ProviderConfig> = settings.providers.clone();
    for provider in providers.iter().filter(|p| p.kind.mail_leaves_this_computer()) {
        let Some(runtime) = AiSettings::compat(&provider.id) else { continue };
        for account in &provider.consented_accounts {
            // One that doesn't check (or past the caps) is dropped, as a version 3 file's would be.
            if let Ok(consents) = settings.consents.with(account, &runtime, true) {
                settings.consents = consents;
            }
        }
    }
    settings.mirror_consents();
    settings.read_chain();
    settings
}

/// Move every key still under its version 2 name to its version 3 one, in `store`, and note what
/// came of it in `shared`: a key moved and read back is noted under its new name alone; one whose
/// copy didn't read back (or couldn't be made) stays noted under both, and is tried again at the
/// next start; a name with nothing under it is no longer noted. Never a key in a log or an error.
pub(crate) fn rename_keys(shared: &Shared, store: &dyn SecretStore) {
    for id in shared.read(|s| s.legacy_keys.clone()) {
        let outcome = move_legacy(store, &id);
        let kept = shared.change(|s| {
            match &outcome {
                Ok(Moved::Done) => {
                    s.note_key(&id);
                    s.drop_legacy_key(&id);
                }
                Ok(Moved::Nothing) => s.drop_legacy_key(&id),
                // Both names are noted until the old one is gone.
                Ok(Moved::NotReadBack | Moved::OldKept) | Err(_) => s.note_key(&id),
            }
            Ok(())
        });
        match (outcome, kept) {
            (Ok(Moved::NotReadBack), _) => log::warn!("a model provider's key couldn't be read back under its new name; the old one is kept"),
            (Ok(Moved::OldKept), _) => log::warn!("a model provider's key was moved but its old name couldn't be taken away yet"),
            (Err(e), _) => log::warn!("a model provider's key couldn't be moved to its new name: {e}"),
            (Ok(_), Err(e)) => log::warn!("a model provider's moved key couldn't be noted: {e}"),
            (Ok(_), Ok(())) => {}
        }
    }
}

/// [`rename_keys`] on a thread of its own, when there is a store and a key to move.
pub(crate) fn spawn_rename(shared: Shared, store: Option<Arc<dyn SecretStore>>) {
    let Some(store) = store else { return };
    if shared.read(|s| s.legacy_keys.is_empty()) {
        return;
    }
    let spawned = std::thread::Builder::new().name("model keys".into()).spawn(move || rename_keys(&shared, store.as_ref()));
    if spawned.is_err() {
        log::warn!("the model providers' keys couldn't be moved to their new names this time");
    }
}
