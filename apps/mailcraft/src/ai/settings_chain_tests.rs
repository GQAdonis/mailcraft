// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Primary, backup and never configured in `ai.json` (#ai, ai-provider-chain 5.1, 5.4): the
//! owner's None kept apart from never having chosen, a primary of None turning the backup off,
//! version 1 files read as they meant, the confirmed on-device address and the owner's ports read
//! as hostile input, and the context window by kind. Synthetic settings only.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use mailcraft_ai::{ProviderConfig, ProviderKind};
use serde_json::{Value, json};

use super::settings::{AiSettings, Backup, MAX_PORTS, OnDeviceBinding, Shared, device_address, device_port};

const IDENTITY: &str = "apfel:apple-foundationmodel:0123456789abcdef";

fn local(id: &str) -> ProviderConfig {
    ProviderConfig { id: id.into(), base_url: "http://127.0.0.1:11437".into(), model: "apple-foundationmodel".into(), ..ProviderConfig::default() }
}

fn read(value: &Value) -> AiSettings {
    AiSettings::from_json(value.to_string().as_bytes())
}

#[test]
fn no_file_is_never_configured_with_the_on_device_backup_unconfirmed() {
    let settings = AiSettings::default();
    assert!(settings.never_configured && settings.active.is_empty());
    assert_eq!(settings.backup, Backup::AppleOnDevice);
    assert_eq!(settings.backup_binding(), None, "nothing is used until the owner confirms an address");
    assert!(!settings.explicitly_none());
    assert_eq!(read(&settings.to_json()), settings);
}

#[test]
fn the_owners_none_is_kept_apart_from_never_configured_and_turns_the_backup_off() {
    let mut settings = AiSettings::default();
    settings.set_provider(local("apfel")).unwrap();
    settings.on_device = Some(OnDeviceBinding { address: device_address(11437), identity: IDENTITY.into(), context_window: 4_096 });
    settings.set_active("").unwrap();
    assert!(settings.explicitly_none() && !settings.never_configured);
    assert_eq!((settings.backup, settings.on_device.clone()), (Backup::None, None));
    assert!(settings.set_backup(Backup::AppleOnDevice).is_err());
    // Kept so: written and read back, still None and not "never".
    let back = read(&settings.to_json());
    assert!(back.explicitly_none());
    assert_eq!(back.backup, Backup::None);
    // A file that says None with a backup is read with none (the rule holds whatever it says).
    let planted = read(&json!({"version": 2, "primary": "", "never_configured": false, "backup": "apple-on-device",
        "on_device": {"address": "http://127.0.0.1:11437", "identity": IDENTITY}}));
    assert_eq!((planted.backup, planted.on_device.clone(), planted.backup_binding()), (Backup::None, None, None));
    // Choosing a primary again lets the backup be chosen.
    settings.set_active("apfel").unwrap();
    settings.set_backup(Backup::AppleOnDevice).unwrap();
    assert_eq!(settings.backup, Backup::AppleOnDevice);
    // Removing the primary is a primary of None.
    assert!(settings.remove("apfel"));
    assert_eq!(settings.backup, Backup::None);
}

#[test]
fn version_one_files_read_as_they_meant() {
    // A configured file: no backup, not never configured.
    let configured = read(&json!({"version": 1, "active": "apfel", "providers": [serde_json::to_value(local("apfel")).unwrap()]}));
    assert_eq!((configured.active.as_str(), configured.never_configured, configured.backup), ("apfel", false, Backup::None));
    // Providers but no active one: the owner's None.
    let none = read(&json!({"version": 1, "active": "", "providers": [serde_json::to_value(local("apfel")).unwrap()]}));
    assert!(none.explicitly_none());
    // Nothing at all: never configured, the on-device backup.
    let empty = read(&json!({"version": 1, "active": "", "providers": []}));
    assert_eq!((empty.never_configured, empty.backup), (true, Backup::AppleOnDevice));
    // A gateway's or remote provider's 4,096 (version 1's default) is 32,768; a local one's stays.
    let gateway = ProviderConfig { kind: ProviderKind::Gateway, ..local("gw") };
    let remote = ProviderConfig { kind: ProviderKind::Remote, base_url: "https://api.example.org/v1".into(), ..local("r") };
    let old = read(
        &json!({"version": 1, "providers": [serde_json::to_value(&gateway).unwrap(), serde_json::to_value(&remote).unwrap(), serde_json::to_value(local("l")).unwrap()]}),
    );
    let windows: Vec<u32> = old.providers.iter().map(|p| p.context_window).collect();
    assert_eq!(windows, [32_768, 32_768, 4_096]);
    // Version 2 keeps the owner's number as written.
    let mut kept = AiSettings::default();
    kept.set_provider(ProviderConfig { context_window: 4_096, ..remote }).unwrap();
    assert_eq!(read(&kept.to_json()).providers[0].context_window, 4_096);
}

#[test]
fn a_provider_without_a_window_gets_its_kinds_default_and_a_new_kind_brings_its_own() {
    let mut entry = serde_json::to_value(ProviderConfig { kind: ProviderKind::Gateway, ..local("gw") }).unwrap();
    entry.as_object_mut().unwrap().remove("context_window");
    let settings = read(&json!({"version": 2, "never_configured": false, "providers": [entry]}));
    assert_eq!(settings.providers[0].context_window, 32_768);
    let mut settings = AiSettings::default();
    settings.set_provider(local("x")).unwrap();
    settings.set_provider(ProviderConfig { kind: ProviderKind::Gateway, ..local("x") }).unwrap();
    assert_eq!(settings.providers[0].context_window, 32_768, "the default follows the kind");
    settings.set_provider(ProviderConfig { kind: ProviderKind::Local, context_window: 8_000, ..local("x") }).unwrap();
    assert_eq!(settings.providers[0].context_window, 8_000, "a number the owner wrote stays");
}

#[test]
fn the_window_used_is_the_owners_else_the_servers_else_the_default() {
    let shared = Shared::open(None);
    let gateway = ProviderConfig { kind: ProviderKind::Gateway, context_window: 32_768, ..local("gw") };
    assert_eq!(shared.window_of(&gateway), (32_768, "default"));
    shared.keep_window(&gateway, 131_072);
    assert_eq!(shared.window_of(&gateway), (131_072, "server"));
    assert_eq!(shared.window_of(&ProviderConfig { context_window: 16_000, ..gateway.clone() }), (16_000, "owner"));
    shared.keep_window(&gateway, u32::MAX);
    assert_eq!(shared.window_of(&gateway), (1_048_576, "server"), "clamped");
}

#[test]
fn the_confirmed_address_and_the_ports_are_read_as_hostile_input() {
    for bad in [
        json!({"address": "http://10.0.0.1:11434", "identity": IDENTITY}),
        json!({"address": "http://127.0.0.1:0", "identity": IDENTITY}),
        json!({"address": "https://127.0.0.1:11437", "identity": IDENTITY}),
        json!({"address": "http://127.0.0.1:11437/v1", "identity": IDENTITY}),
        json!({"address": "http://127.0.0.1:11437", "identity": "llama"}),
        json!({"address": "http://127.0.0.1:11437", "identity": "apfel:a\nb"}),
        json!({"address": 7, "identity": IDENTITY}),
        json!("http://127.0.0.1:11437"),
    ] {
        let settings = read(&json!({"backup": "apple-on-device", "on_device": bad}));
        assert_eq!(settings.on_device, None, "{bad}");
    }
    let good =
        read(&json!({"backup": "apple-on-device", "on_device": {"address": "http://127.0.0.1:11437", "identity": IDENTITY, "context_window": 99}}));
    assert_eq!(good.on_device.map(|b| (b.address, b.context_window)), Some(("http://127.0.0.1:11437".to_string(), 256)));
    // Ports: each 1 to 65535, at most eight, else none of them.
    assert_eq!(read(&json!({"on_device_ports": [11437, 11437, 11500]})).device_ports, [11437, 11500]);
    for bad in [json!([0]), json!([70_000]), json!(["11437"]), json!([1, 2, 3, 4, 5, 6, 7, 8, 9]), json!(11437)] {
        assert!(read(&json!({"on_device_ports": bad})).device_ports.is_empty(), "{bad}");
    }
    assert_eq!(MAX_PORTS, 8);
    let mut settings = AiSettings { device_ports: vec![11437, 11434], ..AiSettings::default() };
    assert_eq!(settings.ports_to_look_on(), [11434, 11437], "11434 first, each once");
    settings.device_ports.clear();
    assert_eq!(settings.ports_to_look_on(), [11434]);
    // A backup this version doesn't know is none.
    assert_eq!(read(&json!({"backup": "somebody-elses-cloud"})).backup, Backup::None);
    assert_eq!(
        (device_port("http://127.0.0.1:11437"), device_port("http://127.0.0.1:+1"), device_port("http://127.0.0.1:")),
        (Some(11437), None, None)
    );
}

#[test]
fn turning_the_backup_off_forgets_the_confirmed_address() {
    let mut settings = AiSettings {
        on_device: Some(OnDeviceBinding { address: device_address(11437), identity: IDENTITY.into(), context_window: 4_096 }),
        ..AiSettings::default()
    };
    assert!(settings.backup_binding().is_some());
    settings.set_backup(Backup::None).unwrap();
    assert_eq!((settings.on_device.clone(), settings.backup_binding()), (None, None));
    assert_eq!(Backup::parse("apple-on-device"), Some(Backup::AppleOnDevice));
    assert_eq!(Backup::parse("cloud"), None);
}
