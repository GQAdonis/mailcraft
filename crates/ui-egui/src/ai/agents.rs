// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Agents button in the title bar and the Agents panel at the right of the window
//! (agent-console, part c): what agents did in this session, for the owner, and one switch that
//! stops all agent access until the owner turns it back on.
//!
//! The panel only reads the activity log the gate keeps ([`crate::agent_gate::activity`]): ids,
//! counts and who decided, never a message's text, a subject, an address or a search. It has no
//! Waiting section (a question for the owner is the modal over the window, which blocks it) and no
//! Disconnect per agent: an agent's name is its own word, so "disconnect this agent" couldn't be
//! made true. **Stop Agent Access** (`ui.agentAccess`) refuses every request with an `agent`
//! field and answers the questions waiting no, for this session only: it is never saved, and the
//! owner's own scripts (no `agent` field) are not affected.
//!
//! Window commands, neither of them for agents (`mailcraft_engine::agent`): `ui.agents` and
//! `ui.agentAccess`.

use egui::{Align, Align2, Layout, RichText, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_engine::agent::Class;
use serde_json::{Value, json};

use super::flag;
use crate::MailApp;
use crate::agent_gate::activity::{Decided, Entry, Log};
use crate::control::AgentClient;
use crate::theme::{Radius, Space, Tokens, TypeRung, semibold};

/// How long after an agent's last request the button keeps its dot.
pub(crate) const RECENT_MS: f64 = 5.0 * 60_000.0;

/// The panel's width, and the least room it leaves the rest of the window.
const PANEL_W: f32 = 340.0;
const PANEL_MIN_W: f32 = 260.0;
const PANEL_MAX_W: f32 = 460.0;
const LEAVES: f32 = 300.0;

/// The Agents panel and the switch, as they stand: in memory only.
#[derive(Debug, Default)]
pub(crate) struct Console {
    /// The panel is open.
    pub(crate) open: bool,
    /// The owner stopped agent access for this session.
    stopped: bool,
}

/// Whether the owner stopped agent access for this session.
pub(crate) fn stopped(app: &MailApp) -> bool {
    app.ai.agents.stopped
}

/// `ui.agents` and `ui.agentAccess`.
pub(crate) fn command(app: &mut MailApp, id: &str, p: &Value) -> Result<Value, String> {
    match id {
        "ui.agents" => {
            let open = flag(p, "open")?.unwrap_or(!app.ai.agents.open);
            app.ai.agents.open = open;
            Ok(json!({"open": open, "access": !stopped(app)}))
        }
        _ => {
            let on = flag(p, "value")?.unwrap_or(stopped(app));
            set_access(app, on);
            Ok(json!({"value": on}))
        }
    }
}

/// Agent access on, or stopped: stopping it answers the questions waiting no.
fn set_access(app: &mut MailApp, on: bool) {
    let changed = app.ai.agents.stopped == on;
    app.ai.agents.stopped = !on;
    if !on {
        crate::agent_gate::refuse_waiting(app);
    }
    if changed {
        app.status(if on { tl!("Agents can ask again.") } else { tl!("Agent access is stopped until you turn it back on or MailCraft closes.") });
    }
}

/// Escape closes the panel, when nothing else in front of it takes the key: a dialog, the agent's
/// question or a field being typed in.
pub(crate) fn escape(app: &mut MailApp, ctx: &egui::Context) {
    if !app.ai.agents.open || app.dialog.is_some() || !app.asking.is_empty() || ctx.egui_wants_keyboard_input() {
        return;
    }
    if crate::keys::take_escape_any(ctx, crate::keys::main_layer()) {
        app.ai.agents.open = false;
    }
}

// ---- What the panel shows, worked out from the log -------------------------------------------

/// An agent as the panel lists it under Recently active: by the name it reported (cleaned), with
/// when it was last seen, how many requests it made, how many went ahead, and the kinds of thing
/// the owner let it do without asking (seen in the log, not asked of a connection).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Seen {
    pub(crate) client: Option<AgentClient>,
    pub(crate) last_ms: f64,
    pub(crate) requests: u64,
    pub(crate) ran: u64,
    pub(crate) allowed: Vec<Class>,
}

/// The agents in the log, the one seen last first. Agents are told apart by what they reported
/// (name and version), never trusted for anything else.
pub(crate) fn recently_active(log: &Log) -> Vec<Seen> {
    let mut seen: Vec<Seen> = Vec::new();
    for e in log.entries() {
        let at = match seen.iter().position(|s| s.client == e.client) {
            Some(i) => i,
            None => {
                seen.push(Seen { client: e.client.clone(), last_ms: e.at_ms, requests: 0, ran: 0, allowed: Vec::new() });
                seen.len() - 1
            }
        };
        let Some(s) = seen.get_mut(at) else { continue };
        s.last_ms = s.last_ms.max(e.at_ms);
        s.requests = s.requests.saturating_add(1);
        if e.decided.went_ahead() {
            s.ran = s.ran.saturating_add(1);
        }
        if e.decided == Decided::AllowedInAdvance
            && let Some(c) = e.class
            && !s.allowed.contains(&c)
        {
            s.allowed.push(c);
        }
    }
    seen.sort_by(|a, b| b.last_ms.total_cmp(&a.last_ms));
    seen
}

/// Whether an agent made a request in the last [`RECENT_MS`] (the button's dot).
pub(crate) fn recent(log: &Log, now_ms: f64) -> bool {
    log.last().is_some_and(|e| now_ms - e.at_ms <= RECENT_MS)
}

/// The button's name, which says what its dot and its icon show.
pub(crate) fn button_name(app: &MailApp) -> String {
    if stopped(app) {
        tl!("Agents: access stopped").to_string()
    } else if recent(&app.asking.activity, crate::now_ms()) {
        tl!("Agents: active in the last 5 minutes").to_string()
    } else {
        tl!("Agents").to_string()
    }
}

/// A kind of thing an agent may be let do, in the owner's words.
pub(crate) fn class_words(c: Class) -> &'static str {
    match c {
        Class::Send => tl!("send"),
        Class::Delete => tl!("delete"),
        Class::Leave => tl!("leave the message you are writing"),
        Class::Accounts => tl!("change accounts"),
        Class::Mailbox => tl!("open or save the mailbox"),
        Class::Settings => tl!("change settings"),
        Class::Input => tl!("click and type in the window"),
    }
}

/// How long ago `at_ms` was, in a few words.
pub(crate) fn ago(now_ms: f64, at_ms: f64) -> String {
    let minutes = ((now_ms - at_ms).max(0.0) / 60_000.0).floor();
    if !minutes.is_finite() || minutes < 1.0 {
        return tl!("just now").to_string();
    }
    let minutes = minutes.min(1e9) as u64;
    if minutes < 60 { tln!(minutes, "{n} minute ago", "{n} minutes ago") } else { tln!(minutes / 60, "{n} hour ago", "{n} hours ago") }
}

/// The time of day `at_ms` was, as the window writes times.
fn clock(app: &MailApp, at_ms: f64) -> String {
    let secs = if at_ms.is_finite() { (at_ms / 1000.0).clamp(0.0, 1e12) as i64 } else { 0 };
    let t = mailcraft_time::DateTime::from_timestamp(secs, app.session.offset_at(secs));
    crate::dates::clock(t, app.session.prefs.twelve_hour)
}

/// One line of Activity: the command and what came of it, never what it was about.
pub(crate) fn outcome_words(e: &Entry) -> String {
    let mut words = e.decided.label().to_string();
    if e.failed && e.decided.went_ahead() {
        words = crate::i18n::fmt(tl!("{verdict}, with an error"), &[("verdict", &words)]);
    } else if let Some(n) = e.count.filter(|_| e.decided.went_ahead()) {
        words = crate::i18n::fmt(tl!("{verdict} · {count}"), &[("verdict", &words), ("count", &tln!(n, "{n} result", "{n} results"))]);
    }
    words
}

// ---- Drawn --------------------------------------------------------------------------------

/// Below this window width (points) the Agents button is its icon alone, so that the search box
/// keeps its room in the title bar (at 200 % a 1440-pixel window is 720 points wide).
pub(crate) const COMPACT_BELOW: f32 = 900.0;

/// The Agents button, in the title bar's right-hand group: its icon (struck through while access
/// is stopped), its words (none when `compact`), and a dot when an agent asked for something in
/// the last few minutes. Its accessible name and tip are the same either way.
pub(crate) fn title_button(app: &mut MailApp, ui: &mut Ui, h: f32, compact: bool) {
    let t = Tokens::get(ui.ctx());
    let off = stopped(app);
    let dot = !off && recent(&app.asking.activity, crate::now_ms());
    let label = if off { tl!("Agents: off") } else { tl!("Agents") };
    let font = TypeRung::Control.medium();
    let w = ui.ctx().fonts_mut(|f| f.layout_no_wrap(label.to_string(), font.clone(), t.text).size().x);
    let width = if compact { h } else { w + 34.0 };
    let (r, resp) = ui.allocate_exact_size(vec2(width, h), Sense::click());
    let fill = if resp.is_pointer_button_down_on() {
        t.pressed
    } else if resp.hovered() {
        t.hover
    } else if app.ai.agents.open {
        t.checked
    } else {
        t.title_bar
    };
    ui.painter().rect(r, h / 2.0, fill, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    let icon_at = if compact { r.center() } else { pos2(r.min.x + 15.0, r.center().y) };
    let icon = egui::Rect::from_center_size(icon_at, vec2(16.0, 16.0));
    crate::icons::paint(ui.painter(), icon, if off { "agentsOff" } else { "agents" }, t.icon, t.accent);
    if dot {
        // Ringed in the bar's colour, so it reads apart from the icon at any size.
        let at = pos2(icon.max.x - 1.0, icon.min.y + 2.0);
        ui.painter().circle(at, 3.5, t.accent, Stroke::new(1.5, fill));
    }
    if !compact {
        ui.painter().text(pos2(r.min.x + 26.0, r.center().y), Align2::LEFT_CENTER, label, font, t.text);
    }
    let name = button_name(app);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &name));
    crate::chrome::focus_ring(ui, &resp, h / 2.0);
    app.hit.add("agentsButton", 0, r);
    let tip = if off {
        tl!("Agent access is stopped: every agent request is refused. Open to let agents in again.")
    } else {
        tl!("What agents did in this session, and how to stop them.")
    };
    if resp.on_hover_text(tip).clicked() {
        let _ = app.run("ui.agents", json!({}));
    }
}

/// What the owner chose in the panel this frame.
enum Act {
    Close,
    Access(bool),
}

/// The Agents panel at the right of the window, while it is open and there is room for it.
pub(crate) fn panel(app: &mut MailApp, ui: &mut Ui) {
    if !app.ai.agents.open || ui.available_width() < PANEL_MIN_W + LEAVES {
        return;
    }
    let t = Tokens::get(ui.ctx());
    let max = (ui.available_width() - LEAVES).clamp(PANEL_MIN_W, PANEL_MAX_W);
    let mut act = None;
    let edge = if t.pane_rule { t.border } else { egui::Color32::TRANSPARENT };
    let frame = egui::Frame::NONE.fill(t.panel).stroke(Stroke::new(1.0, edge));
    let shown = egui::Panel::right("agents_panel").default_size(PANEL_W.min(max)).size_range(PANEL_MIN_W..=max).resizable(true).frame(frame).show(
        ui,
        |ui| {
            head(app, ui, &t, &mut act);
            egui::ScrollArea::vertical().id_salt("agents_body").auto_shrink([false, false]).show(ui, |ui| {
                egui::Frame::NONE.inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
                    ui.spacing_mut().item_spacing = vec2(Space::GAP, Space::TIGHT);
                    access(app, ui, &t, &mut act);
                    ui.add_space(Space::GAP * 2.0);
                    recently(app, ui, &t);
                    ui.add_space(Space::GAP * 2.0);
                    activity(app, ui, &t);
                    ui.add_space(Space::GAP * 2.0);
                    note(ui, &t, tl!("Kept only while MailCraft is open, and never shown to an agent. No message text, subject, address or search is kept here."));
                });
            });
        },
    );
    app.hit.add("agentsPanel", 0, shown.response.rect);
    match act {
        Some(Act::Close) => {
            let _ = app.run("ui.agents", json!({"open": false}));
        }
        Some(Act::Access(on)) => {
            let _ = app.run("ui.agentAccess", json!({"value": on}));
        }
        None => {}
    }
}

fn head(app: &mut MailApp, ui: &mut Ui, t: &Tokens, act: &mut Option<Act>) {
    egui::Panel::top("agents_head")
        .exact_size(36.0)
        .resizable(false)
        .show_separator_line(false)
        .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(12, 0)))
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.label(RichText::new(tl!("Agents")).font(semibold(13.0)).color(t.text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let x = crate::widgets::icon_button(ui, "close", tl!("Close Agents"), 14.0, true);
                    x.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tl!("Close Agents")));
                    crate::chrome::focus_ring(ui, &x, Radius::CONTROL);
                    app.hit.add("closeAgents", 0, x.rect);
                    if x.clicked() {
                        *act = Some(Act::Close);
                    }
                });
            });
        });
}

/// A section's heading.
fn heading(ui: &mut Ui, t: &Tokens, text: &str) {
    ui.label(RichText::new(text).font(semibold(12.5)).color(t.text));
}

/// Small words, at the type floor.
fn note(ui: &mut Ui, t: &Tokens, text: &str) {
    ui.label(RichText::new(text).font(TypeRung::Note.regular()).color(t.text_dim));
}

/// Whether agents may ask, in plain words, and the one switch.
fn access(app: &mut MailApp, ui: &mut Ui, t: &Tokens, act: &mut Option<Act>) {
    let off = stopped(app);
    let card = egui::Frame::NONE.fill(t.surface).stroke(Stroke::new(1.0, t.border)).corner_radius(Radius::CARD).inner_margin(egui::Margin::same(10));
    card.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing = vec2(Space::GAP, Space::GAP);
        let (title, body) = if off {
            (
                tl!("Agent access is stopped"),
                tl!("Every agent request is refused until you allow agents again. This lasts until MailCraft closes. Your own scripts still work."),
            )
        } else {
            (
                tl!("Agent access is on"),
                tl!("Agents connected through MailCraft's MCP bridge can search and read your mail. Before one sends, deletes, or changes an account or a setting, you are asked, unless you allowed it when you started the bridge."),
            )
        };
        ui.label(RichText::new(title).font(semibold(12.5)).color(t.text));
        ui.label(RichText::new(body).font(TypeRung::Control.regular()).color(t.text_dim));
        let resp = if off {
            crate::widgets::primary_button(ui, tl!("Allow Agent Access"), true)
        } else {
            crate::widgets::plain_button(ui, tl!("Stop Agent Access"))
        };
        crate::chrome::focus_ring(ui, &resp, Radius::CARD);
        app.hit.add("agentAccess", 0, resp.rect);
        if resp.clicked() {
            *act = Some(Act::Access(off));
        }
    });
}

/// Recently active: each agent by its reported name, when last seen and what it did.
fn recently(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    heading(ui, t, tl!("Recently active"));
    let seen = recently_active(&app.asking.activity);
    if seen.is_empty() {
        note(ui, t, tl!("No agent has asked for anything in this session."));
        return;
    }
    let now = crate::now_ms();
    for s in &seen {
        ui.add_space(Space::GAP);
        ui.add(egui::Label::new(RichText::new(AgentClient::shown(s.client.as_ref())).font(TypeRung::Control.medium()).color(t.text)).truncate());
        let mut line = crate::i18n::fmt(
            tl!("Last seen {when} · {requests}, {ran}"),
            &[
                ("when", &ago(now, s.last_ms)),
                ("requests", &tln!(s.requests, "{n} request", "{n} requests")),
                ("ran", &tln!(s.ran, "{n} ran", "{n} ran")),
            ],
        );
        if let Some(version) = s.client.as_ref().and_then(|c| c.version.as_deref()) {
            line = crate::i18n::fmt(tl!("Version {version} · {rest}"), &[("version", version), ("rest", &line)]);
        }
        ui.add(egui::Label::new(RichText::new(line).font(TypeRung::Note.regular()).color(t.text_dim)).wrap());
        if !s.allowed.is_empty() {
            let kinds: Vec<&str> = s.allowed.iter().map(|c| class_words(*c)).collect();
            let words = crate::i18n::fmt(tl!("Allowed without asking: {kinds}"), &[("kinds", &kinds.join(", "))]);
            ui.add(egui::Label::new(RichText::new(words).font(TypeRung::Note.regular()).color(t.text_dim)).wrap());
        }
    }
    ui.add_space(Space::TIGHT);
    note(ui, t, tl!("Names are what each agent says it is; MailCraft can't check them."));
}

/// Activity, the newest first: when, who, which command, and who decided.
fn activity(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    heading(ui, t, tl!("Activity"));
    if app.asking.activity.is_empty() {
        note(ui, t, tl!("What agents ask for, and what was decided, shows here."));
        return;
    }
    let rows: Vec<(String, String, String, String, bool)> = app
        .asking
        .activity
        .entries()
        .rev()
        .map(|e| {
            let command = e.command.clone().unwrap_or_else(|| tl!("(a command MailCraft doesn't know)").to_string());
            (clock(app, e.at_ms), AgentClient::shown(e.client.as_ref()), command, outcome_words(e), e.decided.went_ahead())
        })
        .collect();
    for (when, who, command, outcome, ran) in rows {
        ui.add_space(Space::GAP);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = Space::GAP;
            ui.label(RichText::new(when).font(TypeRung::Note.regular()).color(t.text_dim));
            ui.add(egui::Label::new(RichText::new(who).font(TypeRung::Note.regular()).color(t.text_dim)).truncate());
        });
        ui.add(egui::Label::new(RichText::new(command).font(crate::theme::mono(11.5)).color(t.text)).truncate());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = Space::TIGHT;
            let (r, _) = ui.allocate_exact_size(vec2(8.0, 14.0), Sense::hover());
            let mark = if ran { t.green } else { t.overdue };
            ui.painter().circle_filled(r.center(), 3.0, mark);
            ui.add(egui::Label::new(RichText::new(outcome).font(TypeRung::Note.regular()).color(t.text)).wrap());
        });
    }
}
