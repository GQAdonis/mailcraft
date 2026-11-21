// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An answer's Sources list and its provenance line (#ai, A6). The list is the answer's
//! citations and nothing else: one row for each (its number, the sender, the subject and the
//! date, all read from the mailbox, never from the model), and a row shows its message in the
//! reading pane as a `[n]` in the text does. A cited message that has gone since is said so and
//! opens nothing. A stopped answer keeps only the sources its text named before it stopped.
//!
//! The provenance line says which model answered, where it ran (on this computer, or the host of
//! the gateway or the service, never its path, a key or a query), how many passages of the mail
//! it read, and that the mail's text was treated as data. Subjects and senders are shown to the
//! owner only; nothing here is sent anywhere.

use egui::{Align2, Rect, Sense, Stroke, Ui, pos2, vec2};

use super::{Act, Cited};
use crate::MailApp;
use crate::ai::{Provider, Providers, cut};
use crate::theme::{Radius, Tokens, paint_clipped, regular, semibold};

/// The provider id the on-device backup answers under: on this computer.
const ON_DEVICE: &str = "apple-on-device";
/// Most characters of a model's name in the provenance line.
const SHOWN_MODEL: usize = 40;
/// A source row's height.
const ROW_H: f32 = 40.0;

/// Where a model ran, as the provenance line says it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Place {
    /// On this computer: a local server or the on-device model.
    Here,
    /// A gateway, at this host.
    Gateway(String),
    /// A service on the internet, at this host.
    Remote(String),
    /// A provider the window doesn't know of.
    Unknown,
}

/// The host of `base` alone: no scheme, user, port, path, query or fragment.
pub(crate) fn hostname(base: &str) -> String {
    let shown = crate::ai::shown_host(base);
    // `[::1]:8080` keeps its brackets; `host:443` loses its port.
    let host = match shown.strip_prefix('[').and_then(|rest| rest.split_once(']')) {
        Some((inside, _)) => format!("[{inside}]"),
        None => match shown.rsplit_once(':') {
            Some((host, port)) if !host.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => host.to_string(),
            _ => shown,
        },
    };
    cut(&host, 80)
}

/// Where provider `id` ran: what the host said of it, else nothing.
pub(crate) fn place_of(id: &str, provider: Option<&Provider>) -> Place {
    if id == ON_DEVICE {
        return Place::Here;
    }
    match provider {
        Some(p) if p.remote() => Place::Remote(hostname(&p.base_url)),
        Some(p) if p.gateway() => Place::Gateway(hostname(&p.base_url)),
        Some(p) if p.kind == "local" => Place::Here,
        _ => Place::Unknown,
    }
}

/// The provenance line: who answered (as the host said, else the provider in use), where it ran,
/// how many passages it read, and that message text was data.
pub(crate) fn provenance(by: Option<&(String, String)>, providers: &Providers, passages: usize) -> String {
    let (model, place) = match by {
        Some((id, model)) => {
            let provider = providers.get(id).map(|(p, _)| p);
            let model = if model.trim().is_empty() { provider.map(Provider::model_name).unwrap_or_default() } else { cut(model.trim(), SHOWN_MODEL) };
            (model, place_of(id, provider))
        }
        None => match providers.active() {
            Some(p) => (p.model_name(), place_of(&p.id, Some(p))),
            None => (String::new(), Place::Unknown),
        },
    };
    let model = if model.is_empty() { tl!("a model").to_string() } else { model };
    let (said, host) = match place {
        Place::Here => (
            tln!(
                passages,
                "Answered by {model} on this computer, from {n} passage of your mail. Message text was treated as data, never as instructions.",
                "Answered by {model} on this computer, from {n} passages of your mail. Message text was treated as data, never as instructions."
            ),
            String::new(),
        ),
        Place::Gateway(host) => (
            tln!(
                passages,
                "Answered by {model} through the gateway at {host}, from {n} passage of your mail. Message text was treated as data, never as instructions.",
                "Answered by {model} through the gateway at {host}, from {n} passages of your mail. Message text was treated as data, never as instructions."
            ),
            host,
        ),
        Place::Remote(host) => (
            tln!(
                passages,
                "Answered by {model} at {host}, from {n} passage of your mail. Message text was treated as data, never as instructions.",
                "Answered by {model} at {host}, from {n} passages of your mail. Message text was treated as data, never as instructions."
            ),
            host,
        ),
        Place::Unknown => (
            tln!(
                passages,
                "Answered by {model}, from {n} passage of your mail. Message text was treated as data, never as instructions.",
                "Answered by {model}, from {n} passages of your mail. Message text was treated as data, never as instructions."
            ),
            String::new(),
        ),
    };
    crate::i18n::fmt(&said, &[("model", &model), ("host", &host)])
}

/// The numbers `text` names as `[n]` among `cited`, in the order they first come.
fn named(text: &str, cited: &[usize]) -> Vec<usize> {
    let mut seen = Vec::new();
    for piece in crate::ai::answer_text::pieces(text, cited) {
        if let crate::ai::answer_text::Piece::Source(n) = piece
            && !seen.contains(&n)
        {
            seen.push(n);
        }
    }
    seen
}

/// After Stop: only the sources the text named before it stopped, in the host's order.
pub(crate) fn cited_so_far(cited: Vec<Cited>, text: &str) -> Vec<Cited> {
    let numbers: Vec<usize> = cited.iter().map(|c| c.number).collect();
    let so_far = named(text, &numbers);
    cited.into_iter().filter(|c| so_far.contains(&c.number)).collect()
}

/// The number in its disc, at the row's left.
fn disc(ui: &Ui, r: Rect, number: usize, fill: egui::Color32, ink: egui::Color32) {
    let at = pos2(r.min.x + 17.0, r.center().y);
    ui.painter().circle_filled(at, 9.0, fill);
    ui.painter().text(at, Align2::CENTER_CENTER, number.to_string(), semibold(11.0), ink);
}

/// A source: its number, the envelope, the sender and the date, the subject. Enter, Space or a
/// click shows the message; Ctrl+Enter (⌘ on a Mac) or a double click opens it in a window.
fn row(ui: &mut Ui, c: &Cited, focus: bool, t: &Tokens) -> (egui::Response, Option<Act>) {
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
    let name = crate::i18n::fmt(
        tl!("Source {n}: {sender}, {subject}, {date}"),
        &[("n", &c.number.to_string()), ("sender", &c.sender), ("subject", &c.subject), ("date", &c.date)],
    );
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &name));
    if focus {
        resp.request_focus();
    }
    let fill = if resp.is_pointer_button_down_on() {
        t.pressed
    } else if resp.hovered() {
        t.hover
    } else {
        t.input
    };
    let p = ui.painter();
    p.rect(r, Radius::CARD, fill, Stroke::new(1.0, t.input_border), egui::StrokeKind::Inside);
    crate::chrome::focus_ring(ui, &resp, Radius::CARD - 1.0);
    disc(ui, r, c.number, t.accent_soft, t.accent_soft_text);
    let p = ui.painter();
    crate::icons::paint(p, Rect::from_center_size(pos2(r.min.x + 40.0, r.center().y), vec2(16.0, 16.0)), "mail", t.icon, t.accent);
    let left = r.min.x + 56.0;
    let date = p.layout_no_wrap(c.date.clone(), regular(11.0), t.text_dim);
    let date_x = r.max.x - 10.0 - date.size().x;
    p.galley(pos2(date_x, r.min.y + 13.0 - date.size().y / 2.0), date, t.text_dim);
    paint_clipped(p, pos2(left, r.min.y + 13.0), (date_x - left - 8.0).max(0.0), &c.sender, semibold(12.5), t.text);
    paint_clipped(p, pos2(left, r.min.y + 28.0), (r.max.x - left - 10.0).max(0.0), &c.subject, regular(11.0), t.text_dim);
    let resp = resp.on_hover_text(format!("{}\n{}\n\n{}", c.sender, c.subject, tl!("Show this message. Double-click to open it in its own window.")));
    let window = resp.double_clicked() || (resp.clicked() && ui.input(|i| i.modifiers.command));
    let act = if window {
        Some(Act::OpenWindow(c.id))
    } else if resp.clicked() {
        Some(Act::Show(c.id))
    } else {
        None
    };
    (resp, act)
}

/// A source whose message is gone: its number and the words, drawn quieter, opening nothing.
fn gone(ui: &mut Ui, c: &Cited, t: &Tokens) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    let name = crate::i18n::fmt(tl!("Source {n}: message no longer available"), &[("n", &c.number.to_string())]);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &name));
    ui.painter().rect(r, Radius::CARD, t.panel, Stroke::new(1.0, t.input_border), egui::StrokeKind::Inside);
    disc(ui, r, c.number, t.hover, t.text_dim);
    let left = r.min.x + 40.0;
    let words = tl!("Message no longer available");
    paint_clipped(ui.painter(), pos2(left, r.center().y), (r.max.x - left - 10.0).max(0.0), words, regular(12.0), t.text_dim);
    resp
}

/// The Sources list: a heading, then a row for each cited message, in citation order. Up and
/// Down move between the rows that open something (egui's own arrow-key focus moves: a row is
/// a focusable button). What the owner chose, if anything.
pub(super) fn list(app: &mut MailApp, ui: &mut Ui, cited: &[Cited], focus_first: bool, t: &Tokens) -> Option<Act> {
    ui.label(egui::RichText::new(tl!("Sources")).font(semibold(11.5)).color(t.text_dim));
    let mut act = None;
    let mut first = focus_first;
    for c in cited {
        let number = u64::try_from(c.number).unwrap_or(0);
        if app.session.mailbox.message(c.id).is_none() {
            let resp = gone(ui, c, t);
            app.hit.add("answerSourceGone", number, resp.rect);
            continue;
        }
        let (resp, chosen) = row(ui, c, std::mem::take(&mut first), t);
        app.hit.add("answerSource", number, resp.rect);
        if chosen.is_some() {
            act = chosen;
        }
    }
    act
}
