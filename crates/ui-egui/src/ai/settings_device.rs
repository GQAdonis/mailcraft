// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File › Options › AI assistance, the Backup group (#ai): the Backup choice and the on-device
//! model card.
//!
//! The backup is Apple's on-device model, served on this computer by apfel, at one address the
//! owner confirmed with Use It; or None. A server that was found but not confirmed is listed and
//! sent nothing; a server that lists other models (Ollama on the same port, a gateway) is no
//! candidate and isn't listed. Nothing is looked for while the backup is None. The model's name
//! appears as words only: no mark or logo of any vendor is drawn (the card's icons are
//! MailCraft's own, `icons.rs`).

use egui::{RichText, Sense, Ui, vec2};
use serde_json::json;

use super::Providers;
use super::settings;
use super::settings_page::{note, row};
use super::settings_state::{self as st, CardId};
use crate::MailApp;
use crate::theme::{Tokens, regular, semibold};

/// The card's widest, its inset and its icon column.
const CARD_W: f32 = 560.0;
const INSET: i8 = 12;
const ICON_COL: f32 = 24.0;

/// What the owner chose on the card this frame.
enum Act {
    Use(String),
    Stop,
    Look,
}

/// The Backup group: the choice, its note and the card.
pub(crate) fn backup(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    let providers = Providers::of(app);
    let off = providers.explicit_none();
    let on = providers.backup_on_device();
    // The on-device model is offered where the host can use one (or it is already chosen).
    let offered = on || providers.device.state != st::DeviceState::Unavailable;
    let on_name = tl!("Apple's on-device model");
    let shown = if on { on_name } else { tl!("None") };
    let mut chosen: Option<bool> = None;
    row(ui, tl!("Backup"), |ui, w| {
        let resp = ui
            .add_enabled_ui(!off, |ui| {
                egui::ComboBox::from_id_salt("opt_ai_backup").width(w).truncate().selected_text(shown).show_ui(ui, |ui| {
                    if offered && ui.selectable_label(on, on_name).clicked() && !on {
                        chosen = Some(true);
                    }
                    if ui.selectable_label(!on, tl!("None")).clicked() && on {
                        chosen = Some(false);
                    }
                })
            })
            .inner;
        resp.response.widget_info(|| {
            let mut info = egui::WidgetInfo::labeled(egui::Role::ComboBox, !off, tl!("Backup"));
            info.current_text_value = Some(shown.to_string());
            info
        });
        app.hit.add("aiBackupKind", 0, resp.response.rect);
    });
    let words = if off {
        tl!("AI assistance is turned off.")
    } else if on {
        tl!(
            "Used only when the primary can't be reached or refuses, before any of its answer arrives. It runs on this computer, so your mail stays here and no account needs your permission."
        )
    } else {
        tl!("If the primary can't answer, you'll see why. Nothing else is tried.")
    };
    let said = note(ui, words, t);
    app.hit.add("aiBackupNote", 0, said.rect);
    if let Some(on) = chosen
        && let Err(e) = app.run("ui.aiSettings", json!({"backup": if on { "onDevice" } else { "none" }}))
    {
        app.status(e);
    }
    ui.add_space(4.0);
    card(app, ui, t);
    if on && !off {
        ports(app, ui, t);
    }
}

/// "Look on ports": the ports besides 11434 the on-device model is looked for on.
fn ports(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    let mut left = false;
    row(ui, tl!("Look on ports"), |ui, w| {
        let edit = egui::TextEdit::singleline(&mut app.ai.settings.ports).id_salt("opt_ai_ports").hint_text("11437").char_limit(200).desired_width(w);
        let resp = crate::text_menu::field(ui.add(edit));
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, tl!("Look on ports")));
        app.hit.add("aiPorts", 0, resp.rect);
        left = resp.lost_focus();
    });
    if left && let Err(e) = settings::set_ports(app) {
        app.status(e);
    }
    if let Some(problem) = app.ai.settings.ports_problem.clone() {
        let r = super::settings_page::marked(ui, "highImportance", t.red, &problem, t.red);
        app.hit.add("aiPortsProblem", 0, r.rect);
    }
    let said = note(
        ui,
        tl!("MailCraft looks on port 11434 and these, only when this page opens or you ask your first question. Looking sends no mail and no key."),
        t,
    );
    app.hit.add("aiPortsNote", 0, said.rect);
}

/// The on-device model card, in the state the backup and the host say.
fn card(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    let providers = Providers::of(app);
    let looking = app.ai.settings.looking.is_some();
    let id = st::card_id(providers.backup_on_device() && !providers.explicit_none(), looking, &providers.device);
    let (icon, hue, title, detail, caption) = st::card_text(id, &providers.device, &providers.device_ports);
    let focus_stop = std::mem::take(&mut app.ai.settings.focus_stop);
    let mut act: Option<Act> = None;
    let width = ui.available_width().min(CARD_W);
    let frame = egui::Frame::NONE.fill(t.panel).stroke(egui::Stroke::new(1.0, t.border)).corner_radius(6.0).inner_margin(egui::Margin::same(INSET));
    let shown = frame.show(ui, |ui| {
        ui.set_width((width - 2.0 * f32::from(INSET)).max(0.0));
        ui.horizontal_top(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(ICON_COL, ICON_COL), Sense::hover());
            let ink = if matches!(id, CardId::Found) { t.icon } else { t.text };
            crate::icons::paint(ui.painter(), r.shrink(4.0), icon, ink, hue.color(t));
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 4.0);
                let r0 = ui.add(egui::Label::new(RichText::new(title.as_str()).font(semibold(12.5)).color(t.text)).wrap());
                app.hit.add("aiDeviceText", 0, r0.rect);
                if id == CardId::Found {
                    candidates(app, ui, t, &providers, &mut act);
                } else if !detail.is_empty() {
                    let r1 = ui.add(egui::Label::new(RichText::new(detail.as_str()).font(regular(11.5)).color(t.text_dim)).wrap());
                    app.hit.add("aiDeviceText", 1, r1.rect);
                }
                if let Some(caption) = &caption {
                    let r2 = ui.add(egui::Label::new(RichText::new(caption.as_str()).font(regular(11.5)).color(t.text_dim)).wrap());
                    app.hit.add("aiDeviceText", 2, r2.rect);
                }
                actions(app, ui, id, &providers, focus_stop, &mut act);
            });
        });
    });
    app.hit.add("aiDeviceCard", 0, shown.response.rect);
    let done = match act {
        None => return,
        Some(Act::Use(address)) => app.run("ui.confirmOnDevice", json!({"address": address})).map(|_| ()),
        Some(Act::Stop) => settings::stop_using(app),
        Some(Act::Look) => {
            settings::look(app);
            Ok(())
        }
    };
    if let Err(e) = done {
        app.status(e);
    }
}

/// The candidates found, each on its own line with its own Use It.
fn candidates(app: &mut MailApp, ui: &mut Ui, t: &Tokens, providers: &Providers, act: &mut Option<Act>) {
    for (i, c) in providers.device.candidates.iter().enumerate() {
        let line = st::candidate_line(c);
        ui.horizontal_wrapped(|ui| {
            let words = ui.add(egui::Label::new(RichText::new(line.as_str()).font(regular(11.5)).color(t.text_dim)).wrap());
            if i == 0 {
                app.hit.add("aiDeviceText", 1, words.rect);
            }
            let button = crate::widgets::plain_button(ui, tl!("Use It"));
            let name = crate::i18n::fmt(tl!("Use It: {what}"), &[("what", &line)]);
            button.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &name));
            app.hit.add("aiDeviceUse", u64::try_from(i).unwrap_or(0), button.rect);
            if button.clicked() {
                *act = Some(Act::Use(c.address.clone()));
            }
        });
    }
}

/// The card's links and buttons for its state.
fn actions(app: &mut MailApp, ui: &mut Ui, id: CardId, providers: &Providers, focus_stop: bool, act: &mut Option<Act>) {
    match id {
        CardId::Using => {
            let stop = ui.link(tl!("Stop Using It"));
            if focus_stop {
                stop.request_focus();
            }
            app.hit.add("aiDeviceStop", 0, stop.rect);
            if stop.clicked() {
                *act = Some(Act::Stop);
            }
        }
        CardId::Changed => {
            ui.horizontal_wrapped(|ui| {
                if let Some(address) = providers.device.confirmed.clone() {
                    let again = crate::widgets::plain_button(ui, tl!("Use It"));
                    app.hit.add("aiDeviceUse", 0, again.rect);
                    if again.clicked() {
                        *act = Some(Act::Use(address));
                    }
                }
                let look = ui.link(tl!("Look Again"));
                app.hit.add("aiDeviceLook", 0, look.rect);
                if look.clicked() {
                    *act = Some(Act::Look);
                }
            });
        }
        CardId::NotFound => {
            let look = ui.link(tl!("Look Again"));
            app.hit.add("aiDeviceLook", 0, look.rect);
            if look.clicked() {
                *act = Some(Act::Look);
            }
        }
        CardId::Looking | CardId::Found | CardId::Off | CardId::Unavailable => {}
    }
}
