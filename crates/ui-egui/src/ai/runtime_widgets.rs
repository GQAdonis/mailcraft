// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The small controls of the runtime chain page (B11): a tag, the on/off switch, an order arrow,
//! a plain button, a command box with Copy. Each is reachable with Tab, draws the focus ring
//! when it has the keyboard, and has an accessible name that holds its visible words; colours
//! come from [`Tokens`] alone.

use egui::{Align2, Color32, Rect, Response, Sense, Stroke, Ui, pos2, vec2};

use super::runtime_rows::Tone;
use crate::theme::{Radius, Tokens, TypeRung, medium, mono, regular};

/// A tag's height, its icon and its inset.
const TAG_H: f32 = 18.0;
const TAG_ICON: f32 = 12.0;
const TAG_PAD: f32 = 4.0;
/// The switch's track and knob.
const TRACK: (f32, f32) = (34.0, 20.0);
const KNOB: f32 = 16.0;

/// The ink and the fill of a tag of `tone`.
pub(crate) fn tone_colors(tone: Tone, t: &Tokens) -> (Color32, Color32, Option<Color32>) {
    let notice = |hue: Color32| {
        let (wash, edge) = t.notice(hue);
        (t.text, wash, Some(edge))
    };
    match tone {
        Tone::Green => notice(t.green),
        Tone::Orange => notice(t.orange),
        Tone::Red => notice(t.red),
        Tone::Quiet => (t.text_dim, t.pill_quiet, None),
    }
}

/// The hue of a tag's icon.
fn tone_hue(tone: Tone, t: &Tokens) -> Color32 {
    match tone {
        Tone::Green => t.green,
        Tone::Orange => t.orange,
        Tone::Red => t.red,
        Tone::Quiet => t.text_dim,
    }
}

/// How wide a tag with `words` is.
pub(crate) fn tag_width(ui: &Ui, words: &str, icon: bool) -> f32 {
    let w = ui.ctx().fonts_mut(|f| f.layout_no_wrap(words.to_string(), TypeRung::Note.regular(), Color32::WHITE).size().x);
    TAG_PAD * 2.0 + w + if icon { TAG_ICON + TAG_PAD } else { 0.0 }
}

/// A tag: text, not a control. Its words are its accessible name.
pub(crate) fn tag(ui: &mut Ui, words: &str, icon: Option<&str>, ink: Color32, fill: Color32, edge: Option<Color32>, hue: Color32) -> Response {
    let (r, resp) = ui.allocate_exact_size(vec2(tag_width(ui, words, icon.is_some()), TAG_H), Sense::hover());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, words));
    if ui.is_rect_visible(r) {
        let p = ui.painter();
        p.rect_filled(r, Radius::CONTROL, fill);
        if let Some(edge) = edge {
            p.rect_stroke(r, Radius::CONTROL, Stroke::new(1.0, edge), egui::StrokeKind::Inside);
        }
        let mut x = r.min.x + TAG_PAD;
        if let Some(icon) = icon {
            let ir = Rect::from_min_size(pos2(x, r.center().y - TAG_ICON / 2.0), vec2(TAG_ICON, TAG_ICON));
            crate::icons::paint(p, ir, icon, hue, hue);
            x = ir.max.x + TAG_PAD;
        }
        p.text(pos2(x, r.center().y), Align2::LEFT_CENTER, words, TypeRung::Note.regular(), ink);
    }
    resp
}

/// A status tag in its tone.
pub(crate) fn tone_tag(ui: &mut Ui, words: &str, icon: Option<&str>, tone: Tone, t: &Tokens) -> Response {
    let (ink, fill, edge) = tone_colors(tone, t);
    tag(ui, words, icon, ink, fill, edge, tone_hue(tone, t))
}

/// A where tag: `ai_local` on its soft fill, or `ai_remote` on its.
pub(crate) fn where_tag(ui: &mut Ui, words: &str, local: bool, t: &Tokens) -> Response {
    let (ink, fill, icon) = if local { (t.ai_local, t.ai_local_soft, "aiLocal") } else { (t.ai_remote, t.ai_remote_soft, "aiRemote") };
    tag(ui, words, Some(icon), ink, fill, None, ink)
}

/// How wide [`button`] is with `label`.
pub(crate) fn button_width(ui: &Ui, label: &str) -> f32 {
    ui.ctx().fonts_mut(|f| f.layout_no_wrap(label.to_string(), medium(12.5), Color32::WHITE).size().x).max(44.0) + 26.0
}

/// A plain button with an outline (never the filled accent one), named `name` for screen readers
/// (which holds `label`).
pub(crate) fn button(ui: &mut Ui, label: &str, name: &str, enabled: bool) -> Response {
    let t = Tokens::get(ui.ctx());
    let (r, resp) = ui.allocate_exact_size(vec2(button_width(ui, label), 26.0), if enabled { Sense::click() } else { Sense::hover() });
    let fill = if !enabled || resp.is_pointer_button_down_on() {
        t.pressed
    } else if resp.hovered() {
        t.hover
    } else {
        t.input
    };
    let p = ui.painter();
    p.rect(r, 6.0, fill, Stroke::new(1.0, if enabled { t.input_border } else { t.pressed }), egui::StrokeKind::Inside);
    p.text(r.center(), Align2::CENTER_CENTER, label, medium(12.5), if enabled { t.text } else { t.text_disabled });
    crate::chrome::focus_ring(ui, &resp, 6.0);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, name));
    resp
}

/// An icon and a label as a flat button, with the focus ring and the name `name`.
pub(crate) fn icon_text(ui: &mut Ui, icon: &str, label: &str, name: &str) -> Response {
    let resp = crate::widgets::icon_text_button(ui, icon, label, true);
    crate::chrome::focus_ring(ui, &resp, Radius::CONTROL);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name));
    resp
}

/// A 20 × 20 icon button (an order arrow, Copy); disabled ones take no Tab stop.
pub(crate) fn icon_button(ui: &mut Ui, icon: &str, name: &str, enabled: bool) -> Response {
    let t = Tokens::get(ui.ctx());
    let (r, resp) = ui.allocate_exact_size(vec2(20.0, 20.0), if enabled { Sense::click() } else { Sense::hover() });
    if enabled && resp.hovered() {
        ui.painter().rect_filled(r, Radius::CONTROL, if resp.is_pointer_button_down_on() { t.pressed } else { t.hover });
    }
    let c = if enabled { t.icon } else { t.text_disabled };
    crate::icons::paint(ui.painter(), r.shrink(2.0), icon, c, if enabled { t.accent } else { c });
    crate::chrome::focus_ring(ui, &resp, Radius::CONTROL);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, name));
    if enabled { resp.on_hover_text(name) } else { resp }
}

/// How wide [`switch`] is with its word.
pub(crate) fn switch_width(ui: &Ui) -> f32 {
    let w = |s: &str| ui.ctx().fonts_mut(|f| f.layout_no_wrap(s.to_string(), regular(11.0), Color32::WHITE).size().x);
    w(tl!("On")).max(w(tl!("Off"))) + 6.0 + TRACK.0
}

/// The on/off switch, with the word "On" or "Off" to its left so that its state is never carried
/// by colour or place alone. A toggle named `name`, with its checked state. `true` when turned.
pub(crate) fn switch(ui: &mut Ui, on: bool, enabled: bool, name: &str) -> Response {
    let t = Tokens::get(ui.ctx());
    let (r, resp) = ui.allocate_exact_size(vec2(switch_width(ui), TRACK.1), if enabled { Sense::click() } else { Sense::hover() });
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, enabled, on, name));
    let p = ui.painter();
    let word = if on { tl!("On") } else { tl!("Off") };
    p.text(pos2(r.min.x, r.center().y), Align2::LEFT_CENTER, word, regular(11.0), t.text_dim);
    let track = Rect::from_min_size(pos2(r.max.x - TRACK.0, r.min.y), vec2(TRACK.0, TRACK.1));
    let round = TRACK.1 / 2.0;
    let (fill, stroke, knob) = if !enabled {
        (t.pressed, Stroke::NONE, t.text_disabled)
    } else if on {
        (t.accent, Stroke::NONE, t.on_accent)
    } else {
        (t.input, Stroke::new(1.0, t.input_border), t.text_dim)
    };
    p.rect(track, round, fill, stroke, egui::StrokeKind::Inside);
    let inset = (TRACK.1 - KNOB) / 2.0;
    let x = if on { track.max.x - inset - KNOB / 2.0 } else { track.min.x + inset + KNOB / 2.0 };
    p.circle_filled(pos2(x, track.center().y), KNOB / 2.0, knob);
    if resp.has_focus() {
        p.rect_stroke(track.expand(2.0), round + 2.0, Stroke::new(2.0, t.focus_ring), egui::StrokeKind::Outside);
    }
    resp
}

/// A command in a mono box, selectable, with a Copy button beside it. `true` when Copy was
/// pressed (the command is then on the clipboard). `focus` puts the keyboard on Copy.
pub(crate) fn command_box(ui: &mut Ui, command: &str, focus: bool) -> (Response, Response) {
    let t = Tokens::get(ui.ctx());
    ui.horizontal(|ui| {
        let frame = egui::Frame::new()
            .fill(t.input)
            .stroke(Stroke::new(1.0, t.input_border))
            .corner_radius(Radius::CONTROL)
            .inner_margin(egui::Margin::symmetric(8, 4));
        let shown = frame.show(ui, |ui| ui.add(egui::Label::new(egui::RichText::new(command).font(mono(11.0)).color(t.text)).selectable(true))).inner;
        let copy = icon_button(ui, "copy", &crate::i18n::fmt(tl!("Copy {command}"), &[("command", command)]), true);
        if focus {
            copy.request_focus();
        }
        if copy.clicked() {
            ui.ctx().copy_text(command.to_string());
        }
        (shown, copy)
    })
    .inner
}

/// A key and its value in the details panel's grid: the key in a column at least `key_w` wide.
pub(crate) fn fact(ui: &mut Ui, key: &str, key_w: f32, value: impl FnOnce(&mut Ui)) {
    let t = Tokens::get(ui.ctx());
    ui.horizontal_top(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(key_w, 18.0), Sense::hover());
        ui.painter().text(pos2(r.min.x, r.center().y), Align2::LEFT_CENTER, key, regular(11.0), t.text_dim);
        ui.vertical(value);
    });
}

/// A sub-heading of the details panel.
pub(crate) fn sub_head(ui: &mut Ui, words: &str, t: &Tokens) {
    ui.add_space(4.0);
    ui.add(egui::Label::new(egui::RichText::new(words).font(crate::theme::semibold(12.5)).color(t.text)).wrap());
}

/// Wrapped body text in `color`.
pub(crate) fn body(ui: &mut Ui, words: &str, color: Color32) -> Response {
    ui.add(egui::Label::new(egui::RichText::new(words).font(regular(12.5)).color(color)).wrap())
}

/// Wrapped caption text in `text_dim`.
pub(crate) fn caption(ui: &mut Ui, words: &str, t: &Tokens) -> Response {
    ui.add(egui::Label::new(egui::RichText::new(words).font(regular(11.0)).color(t.text_dim)).wrap())
}

/// A notice: a wash and an edge in `hue`, an icon, and wrapped words; `strong` leads in SemiBold.
pub(crate) fn notice(ui: &mut Ui, hue: Color32, icon: &str, strong: Option<&str>, words: &str, t: &Tokens) {
    let (wash, edge) = t.notice(hue);
    egui::Frame::new().fill(wash).stroke(Stroke::new(1.0, edge)).corner_radius(Radius::CONTROL).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_top(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
            crate::icons::paint(ui.painter(), r, icon, hue, hue);
            let mut job = egui::text::LayoutJob::default();
            if let Some(strong) = strong {
                job.append(strong, 0.0, egui::TextFormat::simple(crate::theme::semibold(12.5), t.text));
                job.append(" ", 0.0, egui::TextFormat::simple(regular(12.5), t.text));
            }
            job.append(words, 0.0, egui::TextFormat::simple(regular(12.5), t.text));
            ui.add(egui::Label::new(job).wrap());
        });
    });
}

/// The bar of a wait without an end: in `accent` on `pressed`, 4 pt high; still under reduced
/// motion (the words beside it then say it alone).
pub(crate) fn waiting_bar(ui: &mut Ui, name: &str, still: bool) -> Response {
    let t = Tokens::get(ui.ctx());
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 4.0), Sense::hover());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ProgressIndicator, true, name));
    let p = ui.painter();
    p.rect_filled(r, Radius::CONTROL, t.pressed);
    let w = r.width() * 0.3;
    let x = if still {
        r.min.x
    } else {
        let phase = (ui.input(|i| i.time) * 0.6).fract() as f32;
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
        r.min.x + (r.width() + w) * phase - w
    };
    let bar = Rect::from_min_max(pos2(x.max(r.min.x), r.min.y), pos2((x + w).min(r.max.x), r.max.y));
    if bar.width() > 0.0 {
        p.rect_filled(bar, Radius::CONTROL, t.accent);
    }
    resp
}
