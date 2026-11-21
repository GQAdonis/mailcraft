// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File › Options › AI assistance, drawn (#ai): the status lines, then the Primary group (Kind,
//! Address, Key, Test Connection, Model, Context window, Embeddings model, the accounts), then the
//! Backup group ([`super::settings_device`]).
//!
//! Every control and every text item registers its rect in the hit map under the kind the
//! surface brief names, so that `ui.inspect` finds each state and can check that nothing is cut
//! off. Words are `text` or `text_dim`; a state's hue is carried by an icon beside them (the only
//! hue-coloured words are the red problem line and the green Allowed). Nothing is cut off at the
//! smallest window: a row's control takes what room there is (at most 340 pt), and its label goes
//! above it when that would leave less than 200 pt.

use egui::{RichText, Sense, Ui, vec2};
use serde_json::json;

use super::settings::{self, Kind, MAX_ADDRESS, MAX_KEY, MAX_MODEL};
use super::settings_state::{self as st, Primary, StatusInput, WindowInput, WindowSource};
use super::{Provider, Providers};
use crate::MailApp;
use crate::theme::{Tokens, mono, regular, semibold};

/// The widest a row's control is, the narrowest beside its label, the least label column.
const CONTROL_W: f32 = 340.0;
const CONTROL_MIN: f32 = 200.0;
const LABEL_W: f32 = 210.0;
/// The context window field's width.
const WINDOW_W: f32 = 120.0;

/// A labelled row: the label in a column of its own (at least 210 pt, as wide as it needs), the
/// control in what room is left (at most 340 pt); the label above the control when that room is
/// under 200 pt. `add` gets the control's width.
pub(crate) fn row<R>(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui, f32) -> R) -> R {
    let t = Tokens::get(ui.ctx());
    let label_w = ui.fonts_mut(|f| f.layout_no_wrap(label.to_string(), regular(12.5), t.text).size().x + 12.0).max(LABEL_W);
    let room = ui.available_width();
    let control = (room - label_w - 8.0).min(CONTROL_W);
    if control < CONTROL_MIN {
        ui.add(egui::Label::new(RichText::new(label).font(regular(12.5)).color(t.text)).wrap());
        return add(ui, room.clamp(0.0, CONTROL_W));
    }
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(label_w, 24.0), Sense::hover());
        ui.painter().text(egui::pos2(r.min.x, r.center().y), egui::Align2::LEFT_CENTER, label, regular(12.5), t.text);
        add(ui, control.max(0.0))
    })
    .inner
}

/// A line with a 16 pt icon in a state's hue and wrapped words: the words' rect.
pub(crate) fn marked(ui: &mut Ui, icon: &str, hue: egui::Color32, words: &str, color: egui::Color32) -> egui::Response {
    ui.horizontal_wrapped(|ui| {
        let t = Tokens::get(ui.ctx());
        let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
        crate::icons::paint(ui.painter(), r.shrink(1.0), icon, t.text, hue);
        ui.add(egui::Label::new(RichText::new(words).font(regular(12.5)).color(color)).wrap())
    })
    .inner
}

/// A paragraph in `text_dim`.
pub(crate) fn note(ui: &mut Ui, words: &str, t: &Tokens) -> egui::Response {
    ui.add(egui::Label::new(RichText::new(words).font(regular(11.5)).color(t.text_dim)).wrap())
}

/// A one-line field `width` wide, named for screen readers by its row's label.
fn field(ui: &mut Ui, value: &mut String, id: &str, label: &str, hint: &str, max: usize, width: f32) -> egui::Response {
    let edit = egui::TextEdit::singleline(value).id_salt(id).hint_text(hint).char_limit(max).desired_width(width);
    let resp = crate::text_menu::field(ui.add(edit));
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, label));
    resp
}

/// A group's head (Primary, Backup).
pub(crate) fn group_head(app: &mut MailApp, ui: &mut Ui, words: &str, n: u64, t: &Tokens) {
    ui.add_space(10.0);
    let head = ui.add(egui::Label::new(RichText::new(words).font(semibold(12.5)).color(t.text)).wrap());
    app.hit.add("aiGroupHead", n, head.rect);
}

/// What the status lines are made from: the providers as the host has them, and the page.
fn status_input(app: &MailApp, providers: &Providers, has_host: bool) -> StatusInput {
    let d = &app.ai.settings;
    let primary = match providers.active() {
        None if providers.explicit_none() && d.kind == Kind::None => Primary::Off,
        None => Primary::Never,
        Some(p) if p.remote() => Primary::Remote { model: p.model_name(), host: p.host() },
        Some(p) if p.gateway() => Primary::Gateway { model: p.model_name() },
        Some(p) => Primary::Local { model: p.model_name() },
    };
    let local = providers.active().is_some_and(|p| Kind::of(p) == Kind::Local);
    let changed = (providers.backup_on_device() && providers.device.state == st::DeviceState::Changed)
        .then(|| providers.device.confirmed.as_deref().map(super::shown_address).unwrap_or_default());
    StatusInput {
        has_host,
        primary,
        backup_confirmed: providers.backup_confirmed().is_some(),
        refused: d.refused.clone().filter(|_| local),
        asking: providers.gateway.as_ref().map(|g| g.address.clone()).or_else(|| d.gateway_waiting.clone()),
        changed,
    }
}

/// The section of File › Options.
pub(crate) fn options(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    if !app.ai.settings.loaded {
        let providers = Providers::of(app);
        app.ai.settings.load(&providers, None);
    }
    if std::mem::take(&mut app.ai.settings.scroll) {
        ui.scroll_to_cursor(Some(egui::Align::TOP));
    }
    settings::visit(app, Some(ui.ctx().cumulative_pass_nr()));
    settings::poll(app);
    // With a runtime chain, the chain page (B11) in place of Primary and Backup.
    if app.services.ai_runtimes.is_some() {
        super::settings_runtimes::page(app, ui, t);
        return;
    }
    let providers = Providers::of(app);
    let has_host = app.services.ai_providers.is_some() && app.services.can_ask_models();
    for (i, line) in st::status_lines(&status_input(app, &providers, has_host)).iter().enumerate() {
        let words = marked(ui, line.icon, line.hue.color(t), &line.words, t.text);
        app.hit.add("aiStatus", u64::try_from(i).unwrap_or(0), words.rect);
        // Which line it is, for `ui.inspect`.
        app.hit.add(line.id.hit(), 0, words.rect);
    }
    if !has_host {
        note(ui, tl!("Nothing is sent. Choose a model to describe searches in words and ask questions about your mail."), t);
        return;
    }
    group_head(app, ui, tl!("Primary"), 0, t);
    primary(app, ui, t, &providers);
    group_head(app, ui, tl!("Backup"), 1, t);
    super::settings_device::backup(app, ui, t);
}

/// The Primary group (and, on the chain page, Add OpenAI-Compatible Provider).
pub(crate) fn primary(app: &mut MailApp, ui: &mut Ui, t: &Tokens, providers: &Providers) {
    kind_row(app, ui, t, providers);
    let kind = app.ai.settings.kind;
    if kind == Kind::None {
        return;
    }
    let mut used = false;
    let hint = if kind == Kind::Remote { "https://…" } else { "http://127.0.0.1:11434/v1" };
    row(ui, tl!("Address"), |ui, w| {
        let resp = field(ui, &mut app.ai.settings.address, "opt_ai_address", tl!("Address"), hint, MAX_ADDRESS, w);
        app.hit.add("aiAddress", 0, resp.rect);
        used |= resp.lost_focus();
    });
    if let Some(problem) = app.ai.settings.problem {
        let r = marked(ui, "highImportance", t.red, problem, t.red);
        app.hit.add("aiProblem", 0, r.rect);
    }
    if kind.needs_consent() {
        key_rows(app, ui, t, providers);
    }
    test_row(app, ui, t);
    used |= model_row(app, ui);
    used |= window_rows(app, ui, t);
    row(ui, tl!("Embeddings model"), |ui, w| {
        let resp = field(ui, &mut app.ai.settings.embedding, "opt_ai_embeddings", tl!("Embeddings model"), tl!("Optional"), MAX_MODEL, w);
        app.hit.add("aiEmbeddings", 0, resp.rect);
        used |= resp.lost_focus();
    });
    if used && let Err(e) = settings::apply(app) {
        app.status(e);
    }
    if kind.needs_consent() {
        let providers = Providers::of(app);
        accounts(app, ui, t, &providers);
    } else {
        let said = note(
            ui,
            tl!(
                "Find by Description sends only what you type. Questions send what you ask and the messages that seem to answer it, to the model on this computer and nothing else."
            ),
            t,
        );
        app.hit.add("aiSendNote", 0, said.rect);
    }
}

/// Kind: where the model runs (None turns everything off at once; "Not set up" until chosen).
fn kind_row(app: &mut MailApp, ui: &mut Ui, t: &Tokens, providers: &Providers) {
    let kind = app.ai.settings.kind;
    let never = kind == Kind::None && providers.never_configured;
    let shown = if never { tl!("Not set up") } else { kind.name() };
    let mut chosen: Option<Kind> = None;
    row(ui, tl!("Kind"), |ui, w| {
        let resp = egui::ComboBox::from_id_salt("opt_ai_model").width(w).truncate().selected_text(shown).show_ui(ui, |ui| {
            for k in [Kind::Local, Kind::Gateway, Kind::Remote, Kind::None] {
                if ui.selectable_label(k == kind && !never, k.name()).clicked() && (k != kind || never) {
                    chosen = Some(k);
                }
            }
        });
        resp.response.widget_info(|| {
            let mut info = egui::WidgetInfo::labeled(egui::Role::ComboBox, true, tl!("Kind"));
            info.current_text_value = Some(shown.to_string());
            info
        });
        app.hit.add("aiPrimaryKind", 0, resp.response.rect);
        // Kept for the scripts written before the Kind control had its own name.
        app.hit.add("aiModel", 0, resp.response.rect);
    });
    let words = if never { tl!("Choose where the model runs. Until you do, only a backup you confirmed below is used.") } else { kind.note() };
    let said = note(ui, words, t);
    app.hit.add("aiKindNote", 0, said.rect);
    if let Some(k) = chosen
        && let Err(e) = settings::set_kind(app, k, false)
    {
        app.status(e);
    }
}

/// The key, for a gateway or a service on the internet: never shown back.
fn key_rows(app: &mut MailApp, ui: &mut Ui, t: &Tokens, providers: &Providers) {
    let kind = app.ai.settings.kind;
    let kept = providers.get(&app.ai.settings.id).is_some();
    // Whether a key is kept is asked once, of the credential store, on its own thread.
    if kept
        && app.ai.settings.key_kept.is_none()
        && app.ai.settings.key_asked.is_none()
        && let Some(host) = app.services.ai_providers.as_ref()
    {
        app.ai.settings.key_asked = Some(host.has_key(&app.ai.settings.id));
    }
    let watch = app.secret_watch();
    let store_trouble = app.ai.settings.store_trouble;
    let hint = if kind == Kind::Gateway { tl!("The key the gateway asks for, if any") } else { tl!("Some services need none") };
    let mut save = false;
    row(ui, tl!("Key"), |ui, w| {
        ui.vertical(|ui| {
            ui.set_max_width(w);
            let room = egui::Margin::from(ui.spacing().button_padding);
            let d = &mut app.ai.settings;
            let resp = crate::secret_field::field(ui, &mut d.key, &mut d.key_shown, watch, room, |edit| {
                edit.id_salt("opt_ai_key").char_limit(MAX_KEY).hint_text(hint).interactive(!store_trouble)
            });
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::PasswordInput, !store_trouble, tl!("Key")));
            if std::mem::take(&mut app.ai.settings.focus_key) {
                resp.scroll_to_me(Some(egui::Align::Center));
                if !store_trouble {
                    resp.request_focus();
                }
            }
            app.hit.add("aiKey", 0, resp.rect);
            save = resp.lost_focus();
        });
    });
    if save {
        settings::save_key(app);
    }
    let said = if store_trouble {
        marked(ui, "warning", t.orange, tl!("The key can't be kept: the system's credential store isn't available."), t.text)
    } else {
        note(ui, tl!("Kept in the system's credential store, never in MailCraft's files."), t)
    };
    app.hit.add("aiKeyNote", 0, said.rect);
    if app.ai.settings.key_kept == Some(true) {
        ui.horizontal_wrapped(|ui| {
            note(ui, tl!("A key is kept."), t);
            let remove = ui.link(tl!("Remove Key"));
            app.hit.add("aiRemoveKey", 0, remove.rect);
            if remove.clicked()
                && app.ai.settings.key_forgetting.is_none()
                && let Some(host) = app.services.ai_providers.as_ref()
            {
                app.ai.settings.key_forgetting = Some(host.forget_key(&app.ai.settings.id));
            }
        });
    }
}

/// Test Connection, its result on the same line (under it when there is no room), and what
/// testing sends.
fn test_row(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    let d = &app.ai.settings;
    let address = super::shown_address(&d.address);
    let host = super::shown_host(&d.address);
    let blank = d.address.trim().is_empty();
    let why_not = if blank {
        Some(tl!("Write the address first"))
    } else if d.problem.is_some() {
        Some(tl!("The address can't be used yet"))
    } else if d.test == st::TestState::Testing {
        Some(tl!("Testing…"))
    } else {
        None
    };
    let line = st::test_line(&d.test, &host, &address);
    let said = match &d.test {
        st::TestState::Failed { said, .. } if !said.is_empty() => Some(said.clone()),
        _ => None,
    };
    let mut clicked = false;
    row(ui, "", |ui, _| {
        ui.horizontal_wrapped(|ui| {
            let button = ui.add_enabled_ui(why_not.is_none(), |ui| crate::widgets::plain_button(ui, tl!("Test Connection"))).inner;
            let button = match why_not {
                Some(why) => button.on_disabled_hover_text(why),
                None => button,
            };
            app.hit.add("aiTest", 0, button.rect);
            clicked = button.clicked();
            if let Some((icon, hue, words)) = &line {
                let t = Tokens::get(ui.ctx());
                let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                crate::icons::paint(ui.painter(), r.shrink(1.0), icon, t.text, hue.color(&t));
                let result = ui.add(egui::Label::new(RichText::new(words.as_str()).font(regular(12.5)).color(t.text)).wrap());
                // What the server said is data, shown only in the tip and never read.
                let result = match &said {
                    Some(said) => result.on_hover_text(format!("{} {said}", tl!("The server said:"))),
                    None => result,
                };
                app.hit.add("aiTestResult", 0, result.rect);
            }
        });
    });
    let said = note(
        ui,
        &crate::i18n::fmt(
            tl!("Testing asks {address} for its list of models, with the key if there is one. No mail is sent."),
            &[("address", &address)],
        ),
        t,
    );
    app.hit.add("aiTestNote", 0, said.rect);
    if clicked && let Err(e) = app.run("ui.testProvider", json!({})) {
        app.status(e);
    }
}

/// Model: a field until a test lists the server's models, then a list of them. Whether the
/// settings are to be used (a model chosen, or the field left).
fn model_row(app: &mut MailApp, ui: &mut Ui) -> bool {
    let mut used = false;
    if !app.ai.settings.picking() {
        row(ui, tl!("Model"), |ui, w| {
            let resp = field(
                ui,
                &mut app.ai.settings.model,
                "opt_ai_model_name",
                tl!("Model"),
                tl!("Test the connection to choose from its list"),
                MAX_MODEL,
                w,
            );
            app.hit.add("aiModelName", 0, resp.rect);
            used = resp.lost_focus();
        });
        return used;
    }
    let current = app.ai.settings.model.trim().to_string();
    let ids: Vec<String> = app.ai.settings.test.models().iter().map(|m| m.id.clone()).collect();
    let offered = ids.contains(&current);
    let mut picked: Option<Option<String>> = None;
    let mut entries: Vec<egui::Rect> = Vec::new();
    row(ui, tl!("Model"), |ui, w| {
        let shown = if current.is_empty() { tl!("Choose a model").to_string() } else { current.clone() };
        let popup_w = ui.ctx().content_rect().width().clamp(0.0, 480.0);
        let resp = egui::ComboBox::from_id_salt("opt_ai_model_pick").width(w).truncate().selected_text(shown.as_str()).show_ui(ui, |ui| {
            ui.set_max_width(popup_w);
            // A long id is cut with an ellipsis inside the list; all of it is in its tip.
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            let t = Tokens::get(ui.ctx());
            if !offered && !current.is_empty() {
                ui.horizontal(|ui| {
                    let r = ui.selectable_label(true, egui::RichText::new(current.as_str())).on_hover_text(current.as_str());
                    entries.push(r.rect);
                    ui.label(RichText::new(tl!("(not offered by this server)")).font(regular(11.5)).color(t.text_dim));
                });
            }
            for id in &ids {
                let r = ui.selectable_label(*id == current, id.as_str()).on_hover_text(id.as_str());
                entries.push(r.rect);
                if r.clicked() {
                    picked = Some(Some(id.clone()));
                }
            }
            if ui.selectable_label(false, tl!("Type a name…")).clicked() {
                picked = Some(None);
            }
        });
        resp.response.clone().on_hover_text(shown.as_str()).widget_info(|| {
            let mut info = egui::WidgetInfo::labeled(egui::Role::ComboBox, true, tl!("Model"));
            info.current_text_value = Some(shown.clone());
            info
        });
        app.hit.add("aiModelPicker", 0, resp.response.rect);
    });
    for (i, r) in entries.into_iter().enumerate() {
        app.hit.add("aiModelChoice", u64::try_from(i).unwrap_or(0), r);
    }
    match picked {
        Some(Some(id)) => {
            app.ai.settings.model = id;
            used = true;
        }
        Some(None) => app.ai.settings.type_name = true,
        None => {}
    }
    used
}

/// Context window: a number of tokens, and where the number came from. Whether the field was
/// left (the settings are to be used).
fn window_rows(app: &mut MailApp, ui: &mut Ui, t: &Tokens) -> bool {
    let mut used = false;
    row(ui, tl!("Context window"), |ui, _| {
        ui.horizontal(|ui| {
            let d = &mut app.ai.settings;
            let before = d.window.clone();
            let resp = field(ui, &mut d.window, "opt_ai_window", tl!("Context window"), "4096", st::MAX_WINDOW_DIGITS + 2, WINDOW_W);
            if d.window != before {
                d.window_source = WindowSource::Owner;
            }
            app.hit.add("aiContextWindow", 0, resp.rect);
            used = resp.lost_focus();
            ui.label(RichText::new(tl!("tokens")).font(regular(12.5)).color(t.text_dim));
        });
    });
    let d = &app.ai.settings;
    let n = st::grouped(d.window_tokens());
    let host = super::shown_host(&d.address);
    let said = if d.window_bad {
        marked(ui, "highImportance", t.red, tl!("Write a number of tokens from 256 to 1,048,576."), t.red)
    } else if let Some(at) = d.window_clamped.filter(|_| st::parse_window(&d.window) != WindowInput::Empty) {
        note(ui, &crate::i18n::fmt(tl!("Set to {n}: MailCraft uses from 256 to 1,048,576 tokens."), &[("n", &st::grouped(at))]), t)
    } else {
        let words = match d.window_source {
            WindowSource::Server => crate::i18n::fmt(tl!("{host} says {n} tokens."), &[("host", &host), ("n", &n)]),
            WindowSource::Default => crate::i18n::fmt(
                tl!("{host} doesn't say, so MailCraft uses {n} tokens. Change it if you know the model's size."),
                &[("host", &host), ("n", &n)],
            ),
            WindowSource::Owner => crate::i18n::fmt(tl!("You set {n} tokens."), &[("n", &n)]),
        };
        note(ui, &words, t)
    };
    app.hit.add("aiWindowNote", 0, said.rect);
    used
}

/// The pinned certificate and the accounts, for a gateway or a service on the internet.
fn accounts(app: &mut MailApp, ui: &mut Ui, t: &Tokens, providers: &Providers) {
    let gateway = app.ai.settings.kind == Kind::Gateway;
    let provider = providers.get(&app.ai.settings.id).map(|(p, _)| p.clone()).filter(Provider::needs_consent);
    let Some(provider) = provider.filter(|p| !p.base_url.trim().is_empty()) else {
        let said = note(
            ui,
            tl!(
                "Find by Description sends only what you type, never mail. Questions send what you ask, and messages only from the accounts you allow, to the service at the address above."
            ),
            t,
        );
        app.hit.add("aiSendNote", 0, said.rect);
        return;
    };
    let host = provider.host();
    // The certificate the owner chose to trust for this service, when there is one.
    let pinned = provider.pinned_sha256.trim();
    if !pinned.is_empty() {
        ui.add_space(4.0);
        note(ui, tl!("Pinned certificate (SHA-256 fingerprint):"), t);
        ui.add(egui::Label::new(RichText::new(crate::account_setup::grouped(pinned)).font(mono(11.0)).color(t.text)).wrap());
        let forget = ui.link(tl!("Forget"));
        if forget.clicked() {
            let mut value = providers.get(&provider.id).map(|(_, v)| v.clone()).unwrap_or_else(|| json!({}));
            if let Some(map) = value.as_object_mut() {
                map.insert("pinned_sha256".into(), json!(""));
            }
            let forgot = app.services.ai_providers.as_ref().map(|h| h.set_provider(&value));
            super::providers_changed(app);
            if !matches!(forgot, Some(Ok(()))) {
                app.status(tl!("The pinned certificate couldn't be forgotten."));
            }
        }
    }
    ui.add_space(6.0);
    let head = if gateway {
        crate::i18n::fmt(tl!("Mail that may go through the gateway at {address}"), &[("address", &provider.address())])
    } else {
        crate::i18n::fmt(tl!("Mail that may go to {host}"), &[("host", &host)])
    };
    ui.add(egui::Label::new(RichText::new(head).font(semibold(12.5)).color(t.text)).wrap());
    let list: Vec<(u64, String)> =
        app.session.mailbox.accounts.iter().map(|a| (a.id, a.email.trim().to_string())).filter(|(_, e)| !e.is_empty()).collect();
    if list.is_empty() {
        note(ui, tl!("There are no accounts yet."), t);
    }
    let mut asked: Option<(String, bool)> = None;
    for (id, address) in &list {
        let allowed = provider.allows(address);
        ui.horizontal_wrapped(|ui| {
            ui.add(egui::Label::new(RichText::new(address.as_str()).font(regular(12.5)).color(t.text)).wrap());
            ui.add_space(8.0);
            if allowed {
                let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                crate::icons::paint(ui.painter(), r, "tick", t.green, t.green);
                ui.label(RichText::new(tl!("Allowed")).font(regular(11.5)).color(t.green));
            } else {
                ui.label(RichText::new(tl!("Not allowed")).font(regular(11.5)).color(t.text_dim));
            }
            ui.add_space(8.0);
            let button = crate::widgets::plain_button(ui, if allowed { tl!("Stop") } else { tl!("Allow…") });
            app.hit.add("aiConsent", *id, button.rect);
            if button.clicked() {
                asked = Some((address.clone(), !allowed));
            }
        });
    }
    if let Some((account, allow)) = asked {
        let params = if allow { json!({"account": account}) } else { json!({"account": account, "allowed": false}) };
        if let Err(e) = app.run("ui.aiConsent", params) {
            app.status(e);
        }
    }
    if providers.backup_confirmed().is_some() {
        let said = note(ui, tl!("Questions about mail from an account you haven't allowed are answered by the backup on this computer instead."), t);
        app.hit.add("aiSendNote", 1, said.rect);
    }
    let words = if gateway {
        tl!(
            "Find by Description sends only what you type, never mail. Questions send what you ask, and messages only from the accounts you allow above, to the gateway, which sends them to the internet."
        )
        .to_string()
    } else {
        crate::i18n::fmt(
            tl!(
                "Find by Description sends only what you type, never mail. Questions send what you ask, and messages only from the accounts you allow above, to {host}."
            ),
            &[("host", &host)],
        )
    };
    let said = note(ui, &words, t);
    app.hit.add("aiSendNote", 0, said.rect);
}
