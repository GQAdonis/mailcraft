// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The status bar's AI chip (ai-status-chip, SCN-CHIP-001): each state has its own words and
//! icon, a service elsewhere that no account allowed says the owner's typed searches go to its
//! host (and to nothing else of its address), drawing asks
//! the host for nothing but what its settings are, a click or Enter opens AI assistance, the
//! words are its accessible name, they keep to the type floor and end in an ellipsis when there
//! is no room, and a window without a provider service has no chip.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::Receiver;

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use mailcraft_engine::Session;
use serde_json::{Value, json};

use super::Providers;
use super::status::{self, Chip};
use crate::chrome::type_floor_tests::{assert_type_floor, text_count};
use crate::people_view::testing::{self, Window};
use crate::theme::Tokens;
use crate::{AiProviders, MailApp, Services};

/// What the fake host says of its settings, and every request made of it besides reading them.
#[derive(Default)]
struct Kept {
    settings: Value,
    device: Value,
    requests: Vec<&'static str>,
}

/// A host in memory that records each request.
#[derive(Clone, Default)]
struct Host(Rc<RefCell<Kept>>);

impl Host {
    fn with(settings: Value) -> Host {
        let host = Host::default();
        host.0.borrow_mut().settings = settings;
        host
    }
    fn asked(&self, what: &'static str) {
        self.0.borrow_mut().requests.push(what);
    }
}

fn never<T>() -> Receiver<T> {
    std::sync::mpsc::channel().1
}

impl AiProviders for Host {
    fn providers(&self) -> Value {
        self.0.borrow().settings.clone()
    }
    fn on_device(&self) -> Value {
        self.0.borrow().device.clone()
    }
    fn set_provider(&self, _: &Value) -> Result<(), String> {
        self.asked("set_provider");
        Ok(())
    }
    fn remove_provider(&self, _: &str) -> Result<(), String> {
        self.asked("remove_provider");
        Ok(())
    }
    fn set_active(&self, _: &str) -> Result<(), String> {
        self.asked("set_active");
        Ok(())
    }
    fn set_consent(&self, _: &str, _: &str, _: bool) -> Result<(), String> {
        self.asked("set_consent");
        Ok(())
    }
    fn save_key(&self, _: &str, _: String) -> Receiver<Result<(), String>> {
        self.asked("save_key");
        never()
    }
    fn forget_key(&self, _: &str) -> Receiver<Result<(), String>> {
        self.asked("forget_key");
        never()
    }
    fn has_key(&self, _: &str) -> Receiver<Result<bool, String>> {
        self.asked("has_key");
        never()
    }
    fn answer_gateway(&self, _: &str, _: &str, _: bool) -> Result<(), String> {
        self.asked("answer_gateway");
        Ok(())
    }
    fn dismiss_gateway_question(&self) {
        self.asked("dismiss_gateway_question");
    }
    fn set_backup(&self, _: &str) -> Result<(), String> {
        self.asked("set_backup");
        Ok(())
    }
    fn test_provider(&self, _: &Value) -> Receiver<Result<Value, String>> {
        self.asked("test_provider");
        never()
    }
    fn discover_on_device(&self) -> Receiver<Result<Value, String>> {
        self.asked("discover_on_device");
        never()
    }
    fn confirm_on_device(&self, _: &str) -> Receiver<Result<(), String>> {
        self.asked("confirm_on_device");
        never()
    }
    fn set_device_ports(&self, _: &str) -> Result<(), String> {
        self.asked("set_device_ports");
        Ok(())
    }
    fn stop_using_on_device(&self) -> Result<(), String> {
        self.asked("stop_using_on_device");
        Ok(())
    }
}

// ---- Settings as the host gives them ------------------------------------------------------

fn none() -> Value {
    json!({"active": "", "providers": [], "neverConfigured": false})
}

fn local() -> Value {
    json!({"active": "here", "providers": [{"id": "here", "kind": "local", "base_url": "http://127.0.0.1:11434/v1", "model": "m"}]})
}

fn remote(consents: &[&str]) -> Value {
    json!({"active": "far", "providers": [
        {"id": "here", "kind": "local", "base_url": "http://127.0.0.1:11434/v1"},
        {"id": "far", "kind": "remote", "base_url": "https://api.example.org/v1", "consented_accounts": consents},
    ]})
}

fn gateway(consents: &[&str]) -> Value {
    json!({"active": "gw", "providers": [{"id": "gw", "kind": "gateway", "base_url": "http://127.0.0.1:4000/v1", "consented_accounts": consents}]})
}

fn chip(settings: Value) -> Chip {
    Chip::of(&Providers::read(&settings))
}

// ---- The states, worked out ---------------------------------------------------------------

#[test]
fn scn_chip_001_each_state_has_its_words_and_its_icon() {
    let off = chip(none());
    let here = chip(local());
    let far = chip(remote(&["ada@example.org", "ben@example.org"]));
    assert_eq!((off.text().as_str(), off.icon()), ("AI off", "aiOff"));
    assert_eq!((here.text().as_str(), here.icon()), ("AI: on this computer", "aiLocal"));
    assert_eq!((far.text().as_str(), far.icon()), ("AI: remote allowed for 2 accounts", "aiRemote"));
    // Not colour alone: three icons and three sets of words, each one drawn by the icon set.
    let states = [&off, &here, &far];
    for (i, a) in states.iter().enumerate() {
        assert!(crate::icons::NAMES.contains(&a.icon()), "{} has a drawing", a.icon());
        for b in states.iter().skip(i + 1) {
            assert_ne!(a.icon(), b.icon());
            assert_ne!(a.text(), b.text());
        }
    }
}

#[test]
fn neg_chip_001_a_service_elsewhere_no_account_allowed_gets_typed_searches() {
    let far = chip(remote(&[]));
    assert_eq!(far, Chip::Typed { host: Some("api.example.org".into()), gateway: false });
    assert_eq!(far.text(), "AI: your typed searches go to api.example.org");
    assert_ne!(far.text(), "AI: on this computer", "never says nothing leaves when typed words do");
    assert_eq!(far.icon(), "aiRemote");
    // Consents that are only white space or control characters allow nobody.
    assert_eq!(chip(remote(&["  ", "\u{0}\r\n"])), far);
    let gw = chip(gateway(&[]));
    assert_eq!(gw, Chip::Typed { host: Some("127.0.0.1".into()), gateway: true });
    assert_eq!(gw.text(), "AI: your typed searches go to 127.0.0.1");
}

/// SCN-RT-CHIP-001, negative control: a runtime that leaves this computer is on in the chain, and
/// no account allows it (nor any other runtime that is on): the chip never says mail may leave,
/// never says AI is off, and, in the signed state, still says "AI: on this computer".
#[test]
fn neg_runtime_a_leaving_runtime_no_account_allowed_never_says_mail_may_leave() {
    use mailcraft_model::ai::{Chain, ChainEntry, Consents, Fallback, RuntimeId, Where};

    use super::runtime_rows::{Extra, Facts, Platform, RowFacts};
    use super::status::{ChainChip, TYPED_STATE};
    use crate::control::{RuntimeSettings, RuntimeState, RuntimeStatus};

    fn row(id: &str, enabled: bool, place: Where) -> RowFacts {
        RowFacts {
            id: RuntimeId::parse(id).unwrap(),
            enabled,
            status: RuntimeStatus { state: RuntimeState::Ready, ..Default::default() },
            place,
            extra: Extra::default(),
            compat: None,
            model: None,
        }
    }
    fn facts(rows: Vec<RowFacts>, consents: &[(&str, &str)]) -> Facts {
        let chain = Chain::new(rows.iter().map(|r| ChainEntry::new(r.id.clone()).with_enabled(r.enabled))).unwrap();
        let mut kept = Consents::new();
        for (account, runtime) in consents {
            kept = kept.with(account, &RuntimeId::parse(runtime).unwrap(), true).unwrap();
        }
        Facts {
            settings: RuntimeSettings { chain, consents: kept, fallback: Fallback::Next },
            rows,
            platform: Some(Platform { mac: true, windows: false }),
            signing_in: None,
        }
    }

    // No consent at all; and a consent only for a runtime that is off, which allows nothing.
    let bare = facts(vec![row("on_device", true, Where::Local), row("codex", true, Where::Leaves)], &[]);
    let off_only = facts(
        vec![row("on_device", true, Where::Local), row("codex", true, Where::Leaves), row("chatgpt", false, Where::Leaves)],
        &[("ada@example.org", "chatgpt")],
    );
    for f in [bare, off_only] {
        let chip = ChainChip::of(&f);
        assert!(!matches!(chip, ChainChip::Remote { .. }), "no account allowed: {chip:?}");
        assert!(!chip.text().contains("may leave for"), "{}", chip.text());
        assert!(!chip.text().contains("ada@example.org"), "{}", chip.text());
        assert_ne!(chip, ChainChip::Off);
        if !TYPED_STATE {
            assert_eq!(chip, ChainChip::Local);
            assert_eq!(chip.text(), "AI: on this computer");
            assert_eq!(chip.icon(), "aiLocal");
        }
    }
}

fn remote_at(kind: &str, base_url: &str) -> Chip {
    chip(json!({"active": "p", "providers": [{"id": "p", "kind": kind, "base_url": base_url}]}))
}

#[test]
fn typed_searches_name_the_host_alone_never_a_password_port_path_or_key() {
    let c = remote_at("remote", "https://user:s3cret@API.Example.org:8443/v1/chat?key=sk-abc123#frag");
    assert_eq!(c.text(), "AI: your typed searches go to api.example.org");
    let tip = c.tip();
    for leak in ["user", "s3cret", "8443", "/v1", "key=", "sk-abc123", "frag"] {
        assert!(!c.text().contains(leak) && !tip.contains(leak), "{leak} leaked: {} / {tip}", c.text());
    }
    assert!(tip.contains("api.example.org"), "{tip}");
    // A bracketed IPv6 address keeps its brackets and loses its port.
    assert_eq!(remote_at("remote", "https://[2001:DB8::1]:9000/v1").text(), "AI: your typed searches go to [2001:db8::1]");
    assert_eq!(status::host_only("http://a.example:80/x"), Some("a.example".into()));
}

#[test]
fn an_address_that_cant_be_read_names_the_kind_of_place() {
    for junk in ["", "not a url", "https://", "https://bad host/v1", "https://[zz]/", "https://a..b/", "://x"] {
        assert_eq!(status::host_only(junk), None, "{junk:?}");
        let far = remote_at("remote", junk);
        assert_eq!(far, Chip::Typed { host: None, gateway: false });
        assert_eq!(far.text(), "AI: your typed searches go to a remote service");
        let gw = remote_at("gateway", junk);
        assert_eq!(gw.text(), "AI: your typed searches go to the gateway");
        assert!(gw.tip().contains("gateway") && !gw.tip().contains("{host}"), "{}", gw.tip());
        assert!(!far.tip().contains("{host}"), "{}", far.tip());
    }
}

#[test]
fn a_gateway_and_a_remote_service_are_told_apart_under_the_pointer() {
    let far = remote_at("remote", "https://api.example.org/v1");
    let gw = remote_at("gateway", "http://gw.example:4000/v1");
    assert_eq!(gw.text(), "AI: your typed searches go to gw.example");
    assert!(gw.tip().contains("through the gateway at gw.example") && gw.tip().contains("internet"), "{}", gw.tip());
    assert!(!far.tip().contains("gateway"), "{}", far.tip());
    for c in [&far, &gw] {
        let tip = c.tip();
        assert!(tip.contains("Find by Description") && tip.contains("Ask Mail"), "what is sent: {tip}");
        assert!(tip.contains("No mail is sent there until you allow an account"), "no mail: {tip}");
        assert!(tip.contains("AI assistance"), "how to stop: {tip}");
        assert_eq!(c.icon(), "aiRemote");
    }
}

#[test]
fn a_gateway_with_consent_is_remote_and_accounts_are_counted_once() {
    let gw = chip(gateway(&["ada@example.org", "ADA@example.org ", "ada@example.org"]));
    assert_eq!(gw.text(), "AI: remote allowed for 1 account");
    assert!(matches!(&gw, Chip::Remote { gateway: true, accounts, .. } if accounts.len() == 1));
    let tip = gw.tip();
    assert!(tip.contains("ada@example.org") && tip.contains("127.0.0.1:4000") && tip.contains("internet"), "{tip}");
    let far = chip(remote(&["a@example.org"]));
    assert!(far.tip().contains("api.example.org") && !far.tip().contains("/v1"), "{}", far.tip());
}

#[test]
fn many_accounts_are_named_five_at_most_under_the_pointer() {
    let many: Vec<String> = (0..9).map(|i| format!("p{i}@example.org")).collect();
    let refs: Vec<&str> = many.iter().map(String::as_str).collect();
    let far = chip(remote(&refs));
    assert_eq!(far.text(), "AI: remote allowed for 9 accounts");
    let tip = far.tip();
    assert!(tip.contains("p4@example.org") && !tip.contains("p5@example.org") && tip.contains("and 4 more"), "{tip}");
}

#[test]
fn the_on_device_backup_counts_as_this_computer() {
    let using =
        json!({"state": "using", "candidates": [{"address": "http://127.0.0.1:11437", "model": "apple"}], "confirmed": "http://127.0.0.1:11437"});
    let mut providers = Providers::read(&json!({"active": "", "providers": [], "backup": "apple-on-device", "neverConfigured": true}));
    providers.device = super::settings_state::Device::read(&using);
    assert_eq!(Chip::of(&providers), Chip::Local);
    // A backup beside a local primary is local too; beside a consented remote, remote.
    let mut beside = Providers::read(
        &json!({"active": "far", "providers": [{"id": "far", "kind": "remote", "consented_accounts": ["a@example.org"]}], "backup": "apple-on-device"}),
    );
    beside.device = super::settings_state::Device::read(&using);
    assert!(matches!(Chip::of(&beside), Chip::Remote { .. }));
    // An active id that names no provider is off.
    assert_eq!(chip(json!({"active": "gone", "providers": []})), Chip::Off);
}

#[test]
fn hostile_settings_never_panic_and_read_as_off() {
    for junk in [json!(null), json!([]), json!({"active": 5, "providers": "x"}), json!({"providers": [{"id": 3}, "x", null]})] {
        assert_eq!(chip(junk), Chip::Off);
    }
}

// ---- The chip in the window ---------------------------------------------------------------

fn no_model(_job: crate::ModelJob) -> Receiver<mailcraft_model::ai::ModelEvent> {
    never()
}

fn window_with(host: &Host) -> Window {
    let mut w = testing::window("view.mail");
    let _ = w.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    w.state_mut().services.ai_providers = Some(Box::new(host.clone()));
    w.state_mut().services.model_request = Some(Box::new(no_model));
    w.run_steps(3);
    w
}

fn chip_rect(w: &Window) -> egui::Rect {
    w.state().hit.rect("aiStatusChip", 0).expect("the chip is drawn")
}

#[test]
fn drawing_the_chip_asks_the_host_nothing() {
    for settings in [none(), local(), remote(&[]), gateway(&[]), remote(&["ada@example.org", "ben@example.org"]), gateway(&["ada@example.org"])] {
        let host = Host::with(settings);
        let mut w = window_with(&host);
        w.run_steps(60);
        let _ = chip_rect(&w);
        assert!(host.0.borrow().requests.is_empty(), "drawing asked {:?}", host.0.borrow().requests);
    }
}

#[test]
fn the_chip_is_found_by_its_words_and_a_click_opens_ai_assistance() {
    let host = Host::with(remote(&["ada@example.org", "ben@example.org"]));
    let mut w = window_with(&host);
    assert!(!w.state().ui.backstage);
    w.get_by_label("AI: remote allowed for 2 accounts").click();
    w.run_steps(3);
    assert!(w.state().ui.backstage, "File opened");
    assert_eq!(w.state().ui.backstage_page, "options");
    // The page itself looks whether a key is kept (its own business); nothing is asked of a model
    // or a server, and nothing is changed.
    let asked = host.0.borrow().requests.clone();
    assert!(asked.iter().all(|r| *r == "has_key"), "opening the page asked {asked:?}");
}

#[test]
fn enter_on_the_focused_chip_opens_ai_assistance() {
    let host = Host::with(local());
    let mut w = window_with(&host);
    w.get_by_label("AI: on this computer").focus();
    w.run_steps(2);
    testing::key(&mut w, egui::Key::Enter);
    w.run_steps(2);
    assert!(w.state().ui.backstage && w.state().ui.backstage_page == "options");
}

#[test]
fn each_state_is_drawn_with_its_words_as_its_name() {
    for (settings, words) in [
        (none(), "AI off"),
        (local(), "AI: on this computer"),
        (remote(&[]), "AI: your typed searches go to api.example.org"),
        (gateway(&[]), "AI: your typed searches go to 127.0.0.1"),
        (remote(&["a@example.org", "b@example.org"]), "AI: remote allowed for 2 accounts"),
    ] {
        let host = Host::with(settings);
        let w = window_with(&host);
        let r = chip_rect(&w);
        let node = w.get_by_label(words);
        assert_eq!(node.accesskit_node().label().as_deref(), Some(words));
        let status = w.state().hit.rect("statusBar", 0).expect("the status bar");
        assert!(status.contains_rect(r), "{r:?} sits in the status bar {status:?}");
    }
}

#[test]
fn the_chips_words_keep_to_the_type_floor() {
    let host = Host::with(remote(&["a@example.org"]));
    let w = window_with(&host);
    let r = chip_rect(&w);
    assert!(text_count(w.output(), r) > 0, "the region holds the chip's words");
    assert_type_floor(&w, r, 11.0);
}

#[test]
fn a_window_without_a_provider_service_has_no_chip() {
    let w = testing::window("view.mail");
    assert!(w.state().services.ai_providers.is_none());
    assert!(w.state().hit.rect("aiStatusChip", 0).is_none());
    assert!(w.query_by_label("AI off").is_none());
}

/// The characters each text drawn in `region` shows, row by row (an ellipsis a cut line ends in
/// is in its row, not in the text it was laid out from).
fn drawn_rows(output: &egui::FullOutput, region: egui::Rect) -> Vec<String> {
    let mut rows = Vec::new();
    for clipped in &output.shapes {
        if let egui::Shape::Text(text) = &clipped.shape
            && text.visual_bounding_rect().intersects(region)
        {
            for row in &text.galley.rows {
                rows.push(row.glyphs.iter().map(|g| g.chr).collect());
            }
        }
    }
    rows
}

/// The chip alone in a strip `width` wide, as the status bar's right-to-left block holds it.
fn strip(host: &Host, width: f32) -> Harness<'static, MailApp> {
    let services = Services { ai_providers: Some(Box::new(host.clone())), ..Default::default() };
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(testing::now(), 0), testing::now(), 0), services);
    app.autosave = false;
    let mut h = Harness::builder().with_size(egui::vec2(width + 40.0, 40.0)).allow_missing_glyphs().build_ui_state(
        move |ui, app: &mut MailApp| {
            app.hit.clear();
            let t = Tokens::get(ui.ctx());
            let (r, _) = ui.allocate_exact_size(egui::vec2(width, 30.0), egui::Sense::hover());
            ui.scope_builder(egui::UiBuilder::new().max_rect(r).layout(egui::Layout::right_to_left(egui::Align::Center)), |ui| {
                status::status_item(app, ui, &t);
            });
        },
        app,
    );
    h.run_steps(3);
    h
}

#[test]
fn at_a_narrow_width_the_words_end_in_an_ellipsis_and_the_name_stays_whole() {
    let host = Host::with(remote(&["ada@example.org", "ben@example.org", "cy@example.org"]));
    let wide = strip(&host, 400.0);
    let whole = wide.state().hit.rect("aiStatusChip", 0).expect("drawn");
    let narrow = strip(&host, 110.0);
    let r = narrow.state().hit.rect("aiStatusChip", 0).expect("drawn when narrow");
    assert!(r.width() < whole.width() && r.width() <= 110.0, "{r:?} within 110 of {whole:?}");
    let shown = drawn_rows(narrow.output(), r);
    assert!(shown.iter().any(|s| s.ends_with('…') && s.starts_with("AI")), "cut with an ellipsis: {shown:?}");
    assert!(!drawn_rows(wide.output(), whole).iter().any(|s| s.contains('…')), "whole when there is room");
    assert_type_floor(&narrow, r, 11.0);
    let node = narrow.get_by_label("AI: remote allowed for 3 accounts");
    assert_eq!(node.accesskit_node().label().as_deref(), Some("AI: remote allowed for 3 accounts"));
    // With hardly any room it is left out rather than squeezed to a sliver.
    let none = strip(&host, 20.0);
    assert!(none.state().hit.rect("aiStatusChip", 0).is_none());
    assert!(host.0.borrow().requests.is_empty());
}

/// The pictures for review, when `MAILCRAFT_SHOTS_DIR` names a folder: the whole window and the
/// foot of it, for each state, light, dark, high contrast and at 200 %.
#[test]
fn shots() {
    let Ok(dir) = std::env::var("MAILCRAFT_SHOTS_DIR") else { return };
    let dir = std::path::Path::new(&dir);
    let looks: [(&str, &str, bool, f64); 4] =
        [("light", "light", false, 100.0), ("dark", "dark", false, 100.0), ("hc", "light", true, 100.0), ("200", "light", false, 200.0)];
    let only = std::env::var("MAILCRAFT_SHOTS_ONLY").unwrap_or_default();
    let states = [
        ("off", none()),
        ("local", local()),
        ("remote", remote(&["ada@example.org", "ben@example.org"])),
        ("typed-remote", remote(&[])),
        ("typed-gateway", gateway(&[])),
    ];
    for (state, settings) in states.into_iter().filter(|(s, _)| only.is_empty() || s.starts_with(only.as_str())) {
        let host = Host::with(settings);
        let mut w = window_with(&host);
        for (look, theme, hc, size) in looks {
            w.state_mut().execute("ui.theme", json!({"value": theme})).unwrap();
            w.state_mut().execute("ui.highContrast", json!({"on": hc})).unwrap();
            w.state_mut().execute("ui.interfaceSize", json!({"value": size})).unwrap();
            w.run_steps(6);
            let shot = w.render().unwrap();
            shot.save(dir.join(format!("{state}-{look}-window.png"))).unwrap();
            let r = chip_rect(&w);
            let ppp = w.ctx.pixels_per_point();
            let (x0, y0) = (((r.min.x - 260.0) * ppp).max(0.0) as u32, ((r.min.y - 8.0) * ppp).max(0.0) as u32);
            let (cw, ch) =
                ((((r.width() + 520.0) * ppp) as u32).min(shot.width() - x0), (((r.height() + 16.0) * ppp) as u32).min(shot.height() - y0));
            image::imageops::crop_imm(&shot, x0, y0, cw, ch).to_image().save(dir.join(format!("{state}-{look}-status.png"))).unwrap();
        }
    }
    for (width, name) in [(400.0, "strip-wide"), (110.0, "strip-narrow")] {
        let host = Host::with(remote(&["ada@example.org", "ben@example.org", "cy@example.org"]));
        let mut h = strip(&host, width);
        h.run_steps(2);
        h.render().unwrap().save(dir.join(format!("{name}.png"))).unwrap();
    }
}
