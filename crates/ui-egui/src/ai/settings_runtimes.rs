// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File › Options › AI assistance as the runtime chain (B11, 21.2): the summary line, the chain
//! list (order arrows, icon, name, status and where tags, one action, the on/off switch, Details),
//! the inline details panel, Add OpenAI-Compatible Provider, what happens when a runtime can't
//! answer, and the accounts matrix ([`super::runtime_consent`]).
//!
//! The page is thin: it reads the host's settings and statuses ([`facts_of`]) and every control
//! runs one of the runtimes' methods ([`run`]: `ai.setRuntimeOrder`, `ai.setRuntimeEnabled`,
//! `ai.setRuntimeOption`, `ai.rescan`, `ai.signIn`, `ai.signOut`, `ai.setRuntimeKey`,
//! `ai.setFallback`, `ai.setConsent`), the same the control channel runs. Turning a runtime on
//! allows no account anything. Every control is reachable with Tab in reading order, draws the
//! focus ring, and is named for screen readers; the order is changed with the keyboard.

use std::collections::BTreeMap;

use egui::{Align, Layout, Ui, vec2};
use mailcraft_model::ai::{Fallback, ProviderId, RuntimeId, RuntimeKind, Where};
use serde_json::{Value, json};

use super::runtime_rows::{self as rows, Action, CLAUDE_LOGOUT, CODEX_LOGIN, CODEX_LOGOUT, Extra, Facts, Row, RowFacts, Status, Tone};
use super::runtime_widgets as w;
use super::signin::{self, Open, RtDialog, Said};
use crate::MailApp;
use crate::control::Step;
use crate::theme::{Radius, Tokens, medium, mono, regular, semibold};

/// Below this width a row's controls go to a line of their own.
const NARROW: f32 = 520.0;
/// The order column's width, a row's least height and the details panel's key column.
const ORDER_W: f32 = 24.0;
const ROW_H: f32 = 56.0;
const KEY_W: f32 = 110.0;
const NO_HOST: &str = "the runtimes aren't available in this build";

/// What the page keeps between frames: memory only.
#[derive(Clone, Debug, Default)]
pub(crate) struct PageState {
    /// The runtime whose details are open (one at a time).
    pub(crate) open: Option<RuntimeId>,
    /// The dialog over the page.
    pub(crate) dialog: Option<Open>,
    /// After a move: the runtime moved and which of its arrows keeps the keyboard.
    pub(crate) moved: Option<(RuntimeId, bool)>,
    /// What the host says beyond its status (B11 C2), by runtime id.
    pub(crate) extra: BTreeMap<String, Extra>,
    /// The Program field of the details panel.
    pub(crate) path: String,
    /// Add OpenAI-Compatible Provider is open under the list.
    pub(crate) adding: bool,
    /// Each row's Details button, for the keyboard to go back to.
    details_ids: BTreeMap<String, egui::Id>,
    /// The pass the page was last drawn in (its Escape is taken only while it is on screen).
    pub(crate) drawn: u64,
    /// The control the keyboard goes back to once a dialog has gone, and from which pass (egui
    /// keeps a modal layer a pass after it closes, and takes the keyboard from under it).
    refocus: Option<(egui::Id, u64)>,
}

/// Everything the page reads, from the host and the window, once.
pub(crate) fn facts_of(app: &MailApp) -> Facts {
    let Some(host) = app.services.ai_runtimes.as_deref() else { return Facts::default() };
    let settings = host.settings();
    let providers = if app.services.ai_providers.is_some() { super::Providers::of(app) } else { super::Providers::default() };
    let rows = settings
        .chain
        .entries()
        .iter()
        .map(|e| {
            let id = e.runtime().clone();
            let compat = id
                .provider()
                .and_then(|p| providers.get(p.as_str()))
                .map(|(p, _)| rows::CompatFacts { kind: p.kind.clone(), base_url: p.base_url.clone() });
            RowFacts {
                status: host.status(&id),
                place: host.where_of(&id),
                extra: app.ai.runtimes.extra.get(&id.to_string()).cloned().unwrap_or_default(),
                compat,
                model: e.model().map(|m| m.as_str().to_string()),
                enabled: e.enabled(),
                id,
            }
        })
        .collect();
    let signing_in = app.ai.runtimes.dialog.as_ref().and_then(|o| o.dialog.waiting_for());
    Facts { settings, rows, platform: None, signing_in }
}

/// Runs runtime method `method` with `p`, as the control channel would: the window's own
/// settings change, and the key field is File › Options at AI assistance.
pub(crate) fn run(app: &mut MailApp, method: &str, p: Value) -> Result<Value, String> {
    let step = match app.services.ai_runtimes.as_deref() {
        Some(host) => crate::control::run_runtime(host, method, &p)?,
        None => return Err(NO_HOST.into()),
    };
    match step {
        Step::Done(v) => Ok(v),
        Step::OpenKeyField(provider) => open_key_field(app, &provider),
    }
}

/// Add Key: File › Options at AI assistance with provider `provider` in the form, so its Key
/// field is the one on the page. Nothing of a key goes through here; the owner types it.
pub(crate) fn open_key_field(app: &mut MailApp, provider: &ProviderId) -> Result<Value, String> {
    let providers = super::Providers::of(app);
    // A provider the window's list doesn't have yet (just added, the list not read again) still
    // gets Options opened, with the form as it was.
    if !app.ai.settings.load_provider(&providers, provider.as_str()) {
        log::debug!("the key field was opened for a provider the window's list doesn't hold");
    }
    let runtime = RuntimeId::Compat(provider.clone());
    app.ai.settings.focus_key = true;
    app.ai.runtimes.adding = true;
    app.ai.runtimes.open = Some(runtime.clone());
    app.execute("ui.aiSettings", json!({}))?;
    Ok(json!({"opened": "aiSettings", "runtime": runtime.to_string()}))
}

/// Opens a dialog of the page from the control `opener`.
fn open_dialog(app: &mut MailApp, dialog: RtDialog, opener: Option<egui::Id>) {
    app.ai.runtimes.dialog = Some(Open::new(dialog, opener));
}

/// Says what happened in the status bar, or why it didn't.
fn said(app: &mut MailApp, done: Result<Value, String>, words: String) {
    match done {
        Ok(_) => app.status(words),
        Err(e) => app.status(e),
    }
}

/// The page.
pub(crate) fn page(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    let pass = ui.ctx().cumulative_pass_nr();
    app.ai.runtimes.drawn = pass;
    if let Some((id, from)) = app.ai.runtimes.refocus
        && pass >= from
    {
        ui.ctx().memory_mut(|m| m.request_focus(id));
        app.ai.runtimes.refocus = None;
    }
    let facts = facts_of(app);
    w::body(
        ui,
        tl!(
            "MailCraft asks a model only when you ask: a question in Ask Mail, a search you describe in words, a draft. It tries the runtimes below in order and uses the first one that can answer. A runtime that sends text off this computer gets an account's mail only after you allow that account."
        ),
        t.text_dim,
    );
    summary(ui, &facts, t);
    chain_head(app, ui, t);
    chain(app, ui, &facts, t);
    ui.add_space(4.0);
    let add = w::icon_text(ui, "plus", tl!("Add OpenAI-Compatible Provider…"), tl!("Add OpenAI-Compatible Provider…"));
    app.hit.add("aiRtAddProvider", 0, add.rect);
    if add.clicked() {
        app.ai.runtimes.adding = !app.ai.runtimes.adding;
    }
    if app.ai.runtimes.adding {
        let providers = super::Providers::of(app);
        super::settings_page::primary(app, ui, t, &providers);
    }
    if cfg!(target_os = "windows") {
        w::caption(ui, tl!("Codex isn't offered on Windows: it runs only inside a system sandbox, and Windows doesn't provide one."), t);
    }
    section(ui, tl!("When a runtime can't answer"), t);
    fallback(app, ui, facts.settings.fallback, t);
    section(ui, tl!("Accounts and what may leave this computer"), t);
    let asked = super::runtime_consent::matrix(app, ui, &facts, t);
    if let Some((runtime, account, opener)) = asked.consent {
        open_dialog(app, RtDialog::Consent { runtime, account }, Some(opener));
    }
    dialogs(app, ui.ctx(), &facts);
}

fn section(ui: &mut Ui, words: &str, t: &Tokens) {
    ui.add_space(12.0);
    ui.add(egui::Label::new(egui::RichText::new(words).font(semibold(14.0)).color(t.accent_text)).wrap());
}

/// The summary line: what answers first, how many are ready, and where text may go. Text, not
/// controls; the last tag reads the chip's own state.
fn summary(ui: &mut Ui, facts: &Facts, t: &Tokens) {
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
        match facts.first_usable() {
            Some(f) => {
                let (_, wash, edge) = w::tone_colors(Tone::Green, t);
                w::tag(
                    ui,
                    &crate::i18n::fmt(tl!("First to answer: {runtime}"), &[("runtime", &rows::name(&f.id))]),
                    Some("check"),
                    t.text,
                    wash,
                    edge,
                    t.green,
                );
            }
            None => {
                w::tone_tag(ui, tl!("Nothing can answer yet"), Some("info"), Tone::Quiet, t);
            }
        }
        let n = facts.ready_count();
        w::tone_tag(ui, &tln!(n, "{n} ready and on", "{n} ready and on"), None, Tone::Quiet, t);
        let (words, local, tip) = where_summary(facts);
        let (ink, fill, icon) = if local { (t.ai_local, t.ai_local_soft, "shield") } else { (t.ai_remote, t.ai_remote_soft, "cloud") };
        let shown = w::tag(ui, &words, Some(icon), ink, fill, None, ink);
        if let Some(tip) = tip {
            shown.on_hover_text(tip);
        }
    });
}

/// The summary's where tag: its words, whether nothing leaves, and the whole list under the
/// pointer when it was cut.
pub(crate) fn where_summary(facts: &Facts) -> (String, bool, Option<String>) {
    use super::status::ChainChip;
    match ChainChip::of(facts) {
        ChainChip::Off | ChainChip::Local => (tl!("Nothing leaves this computer").to_string(), true, None),
        ChainChip::Typed { runtimes } => (
            crate::i18n::fmt(
                tl!("Your typed searches may go to {runtime}"),
                &[("runtime", runtimes.first().map(String::as_str).unwrap_or_default())],
            ),
            false,
            None,
        ),
        ChainChip::Remote { .. } => {
            let pairs: Vec<String> = rows::leaving_pairs(facts)
                .iter()
                .flat_map(|(account, ids)| {
                    ids.iter().map(move |id| crate::i18n::fmt(tl!("{account} → {runtime}"), &[("account", account), ("runtime", &rows::name(id))]))
                })
                .collect();
            let mut shown = pairs.iter().take(2).cloned().collect::<Vec<_>>().join(", ");
            let more = pairs.len().saturating_sub(2);
            if more > 0 {
                shown = format!("{shown} {}", tln!(more, "and {n} more", "and {n} more"));
            }
            let words = crate::i18n::fmt(tl!("Mail may leave: {pairs}"), &[("pairs", &shown)]);
            (words, false, (more > 0).then(|| pairs.join(", ")))
        }
    }
}

/// "Runtimes, in the order they're tried", its caption and Look Again at its right end.
fn chain_head(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    section(ui, tl!("Runtimes, in the order they're tried"), t);
    ui.horizontal(|ui| {
        let look_w = crate::widgets::icon_text_width(ui, tl!("Look Again"));
        ui.allocate_ui_with_layout(vec2((ui.available_width() - look_w - 8.0).max(0.0), 24.0), Layout::left_to_right(Align::Center), |ui| {
            w::caption(ui, tl!("Use the arrows to change the order. Runtimes that are off are skipped."), t);
        });
        let look = w::icon_text(ui, "sync", tl!("Look Again"), tl!("Look Again")).on_hover_text(tl!(
            "Look for the runtimes' programs on this computer again. Looking runs each program's version check and sends nothing."
        ));
        app.hit.add("aiRtLook", 0, look.rect);
        if look.clicked() {
            look_again(app);
        }
    });
}

/// `ai.rescan`, and what it found in the status bar.
fn look_again(app: &mut MailApp) {
    match run(app, "ai.rescan", json!({})) {
        Ok(v) => {
            let n = v
                .get("runtimes")
                .and_then(Value::as_array)
                .map_or(0, |l| l.iter().filter(|r| r.get("status").and_then(Value::as_str) != Some("notFound")).count());
            app.status(tln!(n, "Looked again: {n} runtime found.", "Looked again: {n} runtimes found."));
        }
        Err(e) => app.status(e),
    }
}

/// The chain list: one bordered group, a rule between rows, a details panel under its row.
fn chain(app: &mut MailApp, ui: &mut Ui, facts: &Facts, t: &Tokens) {
    let shown = facts.rows();
    if shown.is_empty() {
        w::caption(ui, tl!("No runtime is set up yet. Choose Look Again to look for the runtimes' programs on this computer."), t);
        return;
    }
    let total = shown.len();
    egui::Frame::new().fill(t.surface).stroke(egui::Stroke::new(1.0, t.border)).corner_radius(Radius::CARD).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing = vec2(8.0, 4.0);
        for (i, row) in shown.iter().enumerate() {
            if i > 0 {
                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), egui::Sense::hover());
                ui.painter().hline(r.x_range(), r.center().y, egui::Stroke::new(1.0, t.rule));
            }
            draw_row(app, ui, facts, row, i, total, t);
            if app.ai.runtimes.open.as_ref() == Some(&row.id)
                && let Some(f) = facts.rows.iter().find(|f| f.id == row.id)
            {
                details(app, ui, facts, f, row, t);
            }
        }
    });
}

/// How wide a row's controls are together.
fn controls_width(ui: &Ui, row: &Row) -> f32 {
    let action = row.action.map_or(0.0, |a| w::button_width(ui, a.label()) + 8.0);
    action + w::switch_width(ui) + 8.0 + crate::widgets::icon_text_width(ui, tl!("Details"))
}

#[allow(clippy::too_many_arguments)]
fn draw_row(app: &mut MailApp, ui: &mut Ui, facts: &Facts, row: &Row, i: usize, total: usize, t: &Tokens) {
    let index = u64::try_from(i).unwrap_or(0);
    let room = ui.available_width();
    let narrow = room < NARROW;
    let controls = controls_width(ui, row);
    // The row's inset (8 a side), the order column, the icon and the gaps between them.
    let fixed = 16.0 + ORDER_W + 20.0 + 2.0 * 8.0 + 4.0;
    let text_w = if narrow { room - fixed } else { room - fixed - controls - 8.0 }.max(80.0);
    egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
        ui.set_min_height(ROW_H - 12.0);
        ui.horizontal_top(|ui| {
            order(app, ui, row, i, total, facts);
            let (icon_r, _) = ui.allocate_exact_size(vec2(20.0, 20.0), egui::Sense::hover());
            let ink = if row.enabled { t.icon } else { t.text_dim };
            crate::icons::paint(ui.painter(), icon_r, row.icon, ink, if row.enabled { t.accent } else { ink });
            ui.allocate_ui_with_layout(vec2(text_w, 0.0), Layout::top_down(Align::Min), |ui| {
                ui.set_min_width(text_w);
                ui.set_max_width(text_w);
                text_block(app, ui, row, i, t);
            });
            if !narrow {
                ui.allocate_ui_with_layout(vec2(controls, 26.0), Layout::left_to_right(Align::Center), |ui| row_controls(app, ui, facts, row, index));
            }
        });
        if narrow {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.allocate_ui_with_layout(vec2(controls.min(ui.available_width()), 26.0), Layout::left_to_right(Align::Center), |ui| {
                    row_controls(app, ui, facts, row, index);
                });
            });
        }
    });
}

/// The order column: up, the position, down. A move keeps the keyboard on the moved row's arrow,
/// or its other arrow where that one is now disabled.
fn order(app: &mut MailApp, ui: &mut Ui, row: &Row, i: usize, total: usize, facts: &Facts) {
    let t = Tokens::get(ui.ctx());
    let index = u64::try_from(i).unwrap_or(0);
    ui.allocate_ui_with_layout(vec2(ORDER_W, ROW_H - 12.0), Layout::top_down(Align::Center), |ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        let (can_up, can_down) = (i > 0, i + 1 < total);
        let up = w::icon_button(ui, "moveUp", &crate::i18n::fmt(tl!("Move {runtime} up"), &[("runtime", &row.name)]), can_up);
        let (r, _) = ui.allocate_exact_size(vec2(ORDER_W, 16.0), egui::Sense::hover());
        ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, (i + 1).to_string(), regular(11.0), t.text_dim);
        let down = w::icon_button(ui, "moveDown", &crate::i18n::fmt(tl!("Move {runtime} down"), &[("runtime", &row.name)]), can_down);
        app.hit.add("aiRtUp", index, up.rect);
        app.hit.add("aiRtDown", index, down.rect);
        if let Some((id, was_down)) = app.ai.runtimes.moved.clone()
            && id == row.id
        {
            let target = match (was_down, can_up, can_down) {
                (true, _, true) | (false, false, true) => &down,
                _ => &up,
            };
            target.request_focus();
            app.ai.runtimes.moved = None;
        }
        for (pressed, is_down) in [(up.clicked(), false), (down.clicked(), true)] {
            if pressed && let Some(order) = facts.moved(i, is_down) {
                let list: Vec<String> = order.iter().map(RuntimeId::to_string).collect();
                let done = run(app, "ai.setRuntimeOrder", json!({"order": list}));
                let n = if is_down { i + 2 } else { i };
                let words = crate::i18n::fmt(
                    tl!("{runtime} is now number {n} of {total}."),
                    &[("runtime", &row.name), ("n", &n.to_string()), ("total", &total.to_string())],
                );
                said(app, done, words);
                app.ai.runtimes.moved = Some((row.id.clone(), is_down));
            }
        }
    });
}

/// The name, line 2 and the tags (and Codex's `codex login`, when it is signed out).
fn text_block(app: &mut MailApp, ui: &mut Ui, row: &Row, i: usize, t: &Tokens) {
    ui.spacing_mut().item_spacing = vec2(6.0, 3.0);
    let ink = if row.enabled { t.text } else { t.text_dim };
    let name = ui.add(egui::Label::new(egui::RichText::new(&row.name).font(medium(12.5)).color(ink)).truncate());
    let announced = format!("{}. {}, {}, {}", i + 1, row.name, row.status.words(), row.place.words);
    name.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &announced));
    let line = ui.add(egui::Label::new(egui::RichText::new(&row.line2).font(regular(11.0)).color(t.text_dim)).truncate());
    line.on_hover_text(&row.line2);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
        let (tone, icon) = row.status.look();
        w::tone_tag(ui, row.status.words(), icon, tone, t);
        w::where_tag(ui, &row.place.words, row.place.local, t);
    });
    if row.id == RuntimeId::Codex && row.status == Status::SignedOut {
        let (_, copy) = w::command_box(ui, CODEX_LOGIN, false);
        app.hit.add("aiRtCopy", u64::try_from(i).unwrap_or(0), copy.rect);
        // The row's second line already says MailCraft never runs it (S8): this says what next.
        w::caption(ui, tl!("Copy it, run it in a terminal, then choose Look Again."), t);
    }
}

/// The action, the switch and Details, left to right (their Tab order).
fn row_controls(app: &mut MailApp, ui: &mut Ui, facts: &Facts, row: &Row, index: u64) {
    ui.spacing_mut().item_spacing = vec2(8.0, 0.0);
    if let Some(action) = row.action {
        let name = crate::i18n::fmt(tl!("{label}: {runtime}"), &[("label", action.label()), ("runtime", &row.name)]);
        let b = w::button(ui, action.label(), &name, true);
        app.hit.add("aiRtAction", index, b.rect);
        if b.clicked() {
            act(app, row, action, b.id);
        }
    }
    let switchable = row.status.switchable();
    let switch = w::switch(ui, row.enabled, switchable, &crate::i18n::fmt(tl!("Use {runtime}"), &[("runtime", &row.name)]));
    app.hit.add("aiRtSwitch", index, switch.rect);
    if switchable && switch.clicked() {
        turn(app, facts, row);
    }
    let open = app.ai.runtimes.open.as_ref() == Some(&row.id);
    let details = w::icon_text(
        ui,
        if open { "expanded" } else { "collapsed" },
        tl!("Details"),
        &crate::i18n::fmt(tl!("Details: {runtime}"), &[("runtime", &row.name)]),
    );
    app.hit.add("aiRtDetails", index, details.rect);
    app.ai.runtimes.details_ids.insert(row.id.to_string(), details.id);
    if details.clicked() {
        app.ai.runtimes.open = if open { None } else { Some(row.id.clone()) };
        app.ai.runtimes.path.clear();
    }
}

/// The switch: `ai.setRuntimeEnabled`, and nothing else (no consent, no dialog).
fn turn(app: &mut MailApp, facts: &Facts, row: &Row) {
    let on = !row.enabled;
    let done = run(app, "ai.setRuntimeEnabled", json!({"runtime": row.id.to_string(), "value": on}));
    let args = [("runtime", row.name.as_str())];
    let words = match (on, &row.status) {
        (false, _) => crate::i18n::fmt(tl!("{runtime} is off."), &args),
        (true, Status::SignedOut) => crate::i18n::fmt(tl!("{runtime} is on. It's skipped until you sign in."), &args),
        (true, Status::NeedsKey) => crate::i18n::fmt(tl!("{runtime} is on. It's skipped until you add its key."), &args),
        (true, _) if row.leaves => {
            crate::i18n::fmt(tl!("{runtime} is on. No account's mail goes to it until you allow that account under Accounts."), &args)
        }
        (true, _) => crate::i18n::fmt(tl!("{runtime} is on."), &args),
    };
    let _ = facts;
    said(app, done, words);
}

/// A row's action.
fn act(app: &mut MailApp, row: &Row, action: Action, opener: egui::Id) {
    match action {
        Action::SignIn if row.id == RuntimeId::ClaudeCode => {
            let dialog = signin::start_claude(app);
            open_dialog(app, dialog, Some(opener));
        }
        Action::SignIn => open_dialog(app, RtDialog::ChatGpt(signin::GptStep::Start), Some(opener)),
        Action::CancelSignIn => {
            app.ai.runtimes.dialog = None;
            app.status(tl!("Sign-in cancelled. Nothing was changed."));
        }
        Action::LookAgain => look_again(app),
        Action::AddKey => {
            let done = run(app, "ai.setRuntimeKey", json!({"runtime": row.id.to_string()}));
            if let Err(e) = done {
                app.status(e);
            }
        }
        Action::HowToInstall => app.ai.runtimes.open = Some(row.id.clone()),
        Action::UseUar => open_dialog(app, RtDialog::UarOptIn, Some(opener)),
        Action::UseDevice => {
            // The click is the one-time yes: the on-device model is turned on in the chain.
            let done = run(app, "ai.setRuntimeEnabled", json!({"runtime": row.id.to_string(), "value": true}));
            said(app, done, crate::i18n::fmt(tl!("{runtime} is on."), &[("runtime", &row.name)]));
        }
    }
}

/// The inline details panel: Found, Install, Sign-in, Model, What it may do, Usage, Notes.
fn details(app: &mut MailApp, ui: &mut Ui, facts: &Facts, f: &RowFacts, row: &Row, t: &Tokens) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().hline(r.x_range(), r.center().y, egui::Stroke::new(1.0, t.rule));
    egui::Frame::new().fill(t.panel).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
        found(app, ui, f, row, t);
        if row.status == Status::NotInstalled {
            w::sub_head(ui, tl!("Install"), t);
            w::body(ui, &crate::i18n::fmt(tl!("Install {runtime} with its own installer, then choose Look Again. MailCraft never installs or updates it."), &[("runtime", &row.name)]), t.text);
        }
        sign_in_section(app, ui, f, row, t);
        if let Some(model) = &f.model {
            w::sub_head(ui, tl!("Model"), t);
            w::fact(ui, tl!("Model"), KEY_W, |ui| {
                w::body(ui, model, t.text);
            });
        }
        if row.leaves || f.id.kind() == RuntimeKind::Uar {
            w::sub_head(ui, tl!("What it may do"), t);
            if f.id.kind() == RuntimeKind::Uar && !(f.place == Where::Local) {
                w::notice(ui, t.orange, "warning", Some(tl!("MailCraft can't turn off this runtime's tools.")), super::runtime_consent::uar_tools(), t);
            } else {
                w::notice(
                    ui,
                    t.blue,
                    "lock",
                    Some(tl!("Text only, no tools.")),
                    tl!("It can't run commands, read or write files, browse the web or use other tools, and MailCraft stops it if it tries. Mail is data, never instructions: a message can't talk it into anything. This can't be turned off."),
                    t,
                );
            }
        }
        usage(ui, f, t);
        notes(ui, f, t);
        let _ = facts;
    });
}

fn found(app: &mut MailApp, ui: &mut Ui, f: &RowFacts, row: &Row, t: &Tokens) {
    let kind = f.id.kind();
    let path = f.status.path.as_ref().map(|p| p.as_str().to_string());
    let version = f.status.version.clone();
    let how = match kind {
        RuntimeKind::ClaudeCode => Some(tl!("claude -p, text only, no tools or MCP servers, nothing saved")),
        RuntimeKind::Codex => Some(tl!("codex exec, inside the system sandbox, text only, nothing saved")),
        RuntimeKind::ChatGpt => Some(tl!("MailCraft talks to OpenAI directly")),
        RuntimeKind::Uar => Some(tl!("runs on this computer; MailCraft didn't start it")),
        RuntimeKind::OnDevice | RuntimeKind::Compat => None,
    };
    w::sub_head(ui, tl!("Found"), t);
    if kind == RuntimeKind::Compat
        && let Some(c) = &f.compat
    {
        w::fact(ui, tl!("Address"), KEY_W, |ui| {
            ui.add(egui::Label::new(egui::RichText::new(super::shown_address(&c.base_url)).font(mono(11.0)).color(t.text)).selectable(true));
        });
    }
    if kind.takes_path() {
        w::fact(ui, tl!("Program"), KEY_W, |ui| {
            // A path in the path font; words about where it is looked for in the window's own (S13).
            let (shown, font) = match &path {
                Some(p) => (p.clone(), mono(11.0)),
                None if row.status == Status::NotInstalled => (tl!("Not found").to_string(), regular(11.0)),
                None => (tl!("In the usual places").to_string(), regular(11.0)),
            };
            ui.add(egui::Label::new(egui::RichText::new(shown).font(font).color(t.text)).selectable(true));
            let field = ui.add(
                egui::TextEdit::singleline(&mut app.ai.runtimes.path)
                    .hint_text(tl!("Another path"))
                    .char_limit(mailcraft_model::ai::runtime::MAX_PATH)
                    .desired_width(ui.available_width().min(320.0)),
            );
            field.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, tl!("Program")));
            app.hit.add("aiRtPath", 0, field.rect);
            ui.horizontal_wrapped(|ui| {
                let typed = app.ai.runtimes.path.trim().to_string();
                if w::button(ui, tl!("Use This Path"), tl!("Use This Path"), !typed.is_empty()).clicked() {
                    let done = run(app, "ai.setRuntimeOption", json!({"runtime": f.id.to_string(), "path": typed}));
                    said(app, done, crate::i18n::fmt(tl!("{runtime} will be looked for there."), &[("runtime", &row.name)]));
                }
                if w::button(ui, tl!("Look in the Usual Places"), tl!("Look in the Usual Places"), true).clicked() {
                    app.ai.runtimes.path.clear();
                    let done = run(app, "ai.setRuntimeOption", json!({"runtime": f.id.to_string(), "path": Value::Null}));
                    said(app, done, crate::i18n::fmt(tl!("{runtime} will be looked for in the usual places."), &[("runtime", &row.name)]));
                }
            });
        });
    }
    if let Some(v) = version {
        w::fact(ui, tl!("Version"), KEY_W, |ui| {
            w::body(ui, &v, t.text);
        });
    }
    if let Some(how) = how {
        w::fact(ui, tl!("How it runs"), KEY_W, |ui| {
            w::body(ui, how, t.text);
        });
    }
    if let Status::Broken(reason) = &row.status {
        w::notice(
            ui,
            t.red,
            "highImportance",
            Some(&crate::i18n::fmt(tl!("Can't use it: {reason}."), &[("reason", &reason.words())])),
            &reason.remedy(),
            t,
        );
    }
    if row.status == Status::NotInstalled {
        w::caption(ui, &crate::i18n::fmt(tl!("Looked in: your PATH and the usual places for {runtime}."), &[("runtime", &row.name)]), t);
    }
}

fn sign_in_section(app: &mut MailApp, ui: &mut Ui, f: &RowFacts, row: &Row, t: &Tokens) {
    let strong_then = |ui: &mut Ui, strong: &str, rest_before: &str, rest_after: &str| {
        let mut job = egui::text::LayoutJob::default();
        let plain = egui::TextFormat::simple(regular(12.5), t.text);
        job.append(rest_before, 0.0, plain.clone());
        job.append(strong, 0.0, egui::TextFormat::simple(semibold(12.5), t.text));
        job.append(rest_after, 0.0, plain);
        ui.add(egui::Label::new(job).wrap());
    };
    match f.id.kind() {
        RuntimeKind::Compat => {}
        RuntimeKind::OnDevice => {
            w::sub_head(ui, tl!("Sign-in"), t);
            w::body(ui, tl!("No sign-in. It runs on this Mac and never sends text anywhere."), t.text);
        }
        RuntimeKind::ChatGpt => {
            w::sub_head(ui, tl!("Sign-in"), t);
            w::body(
                ui,
                tl!(
                    "MailCraft signs in with OpenAI's Sign in with ChatGPT. It keeps the long-lived part of the sign-in in the system's credential store and the short-lived part only in memory. They renew on their own; if OpenAI ends the sign-in, ChatGPT plan turns off and says so."
                ),
                t.text,
            );
            if row.status.usable() {
                let b = w::button(ui, tl!("Sign Out of ChatGPT…"), tl!("Sign Out of ChatGPT…"), true);
                app.hit.add("aiRtSignOut", 0, b.rect);
                if b.clicked() {
                    open_dialog(app, RtDialog::ChatGptSignOut, Some(b.id));
                }
            }
        }
        RuntimeKind::ClaudeCode => {
            w::sub_head(ui, tl!("Sign-in"), t);
            strong_then(
                ui,
                tl!("MailCraft never reads, keeps or passes on your sign-in:"),
                &format!("{} ", tl!("Claude Code uses your own sign-in, which Claude Code keeps.")),
                &format!(
                    " {}",
                    tl!(
                        "it only asks Claude Code whether you're signed in. To sign in, run claude auth login; to sign out, run claude auth logout, in a terminal."
                    )
                ),
            );
            let (_, copy) = w::command_box(ui, CLAUDE_LOGOUT, false);
            app.hit.add("aiRtCopy", 100, copy.rect);
        }
        RuntimeKind::Codex => {
            w::sub_head(ui, tl!("Sign-in"), t);
            strong_then(
                ui,
                tl!("MailCraft never reads, keeps or passes on your sign-in, and never runs codex login."),
                &format!("{} ", tl!("Codex uses your own sign-in, kept in your Codex folder.")),
                &format!(
                    " {}",
                    tl!(
                        "It doesn't ask Codex whether you're signed in either: it finds out when Codex answers. To sign in, run codex login; to sign out, run codex logout, in a terminal."
                    )
                ),
            );
            let (_, copy) = w::command_box(ui, CODEX_LOGIN, false);
            app.hit.add("aiRtCopy", 101, copy.rect);
            w::body(ui, tl!("If Codex isn't signed in yet, run codex login in a terminal yourself, with your ChatGPT account."), t.text);
            w::body(ui, &crate::i18n::fmt(tl!("To sign out, run {command} yourself."), &[("command", CODEX_LOGOUT)]), t.text);
        }
        RuntimeKind::Uar => {
            w::sub_head(ui, tl!("Sign-in"), t);
            w::body(ui, tl!("The runtime uses the model sign-ins you gave it. MailCraft never sees them."), t.text);
        }
    }
}

/// The plan-quota line for the runtimes that use the owner's plan.
fn usage(ui: &mut Ui, f: &RowFacts, t: &Tokens) {
    let line = match f.id.kind() {
        RuntimeKind::ChatGpt => tl!("Answers count against your ChatGPT plan's usage limits."),
        RuntimeKind::ClaudeCode if f.extra.uses_plan == Some(false) => return,
        RuntimeKind::ClaudeCode => tl!("Answers count against your Claude plan's usage limits, the same as using Claude Code yourself."),
        RuntimeKind::Codex => tl!("Answers count against your ChatGPT plan's usage limits, the same as using Codex yourself."),
        _ => return,
    };
    w::sub_head(ui, tl!("Usage"), t);
    w::body(ui, line, t.text_dim);
    w::body(ui, tl!("MailCraft adds no API key, so it never pays by the token for you."), t.text_dim);
}

fn notes(ui: &mut Ui, f: &RowFacts, t: &Tokens) {
    let codex = f.id.kind() == RuntimeKind::Codex;
    let no_plan = f.id.kind() == RuntimeKind::ClaudeCode && f.extra.uses_plan == Some(false);
    if !(codex || no_plan || f.extra.tool_stopped) {
        return;
    }
    w::sub_head(ui, tl!("Notes"), t);
    if codex {
        w::notice(
            ui,
            t.orange,
            "warning",
            None,
            tl!(
                "Codex can't be made fully tool-free on its own. MailCraft runs it inside the system sandbox, in an empty folder, with nothing saved, and stops it the moment it tries a tool. The skills and agent instructions in your Codex folder still reach what it's sent."
            ),
            t,
        );
    }
    if no_plan {
        w::notice(
            ui,
            t.orange,
            "warning",
            None,
            tl!("Claude Code is signed in without a Claude plan. Anthropic may charge that sign-in for each answer."),
            t,
        );
    }
    if f.extra.tool_stopped {
        let words = crate::i18n::fmt(
            tl!("{runtime} tried to use a tool, so MailCraft stopped it. Nothing it did was kept."),
            &[("runtime", &rows::name(&f.id))],
        );
        w::notice(ui, t.orange, "warning", Some(tl!("Last time:")), &words, t);
    }
}

/// The three choices of what happens when a runtime can't answer: `ai.setFallback`.
fn fallback(app: &mut MailApp, ui: &mut Ui, current: Fallback, t: &Tokens) {
    let choices = [
        (
            Fallback::Next,
            tl!("Try the next one"),
            tl!("Only runtimes already allowed for that account are tried. The answer says which one replied, and which couldn't."),
        ),
        (Fallback::Ask, tl!("Ask me first"), tl!("MailCraft says which runtime couldn't answer and which it would try next, and waits for you.")),
        (Fallback::Stop, tl!("Stop and tell me"), tl!("Nothing else is tried.")),
    ];
    for (i, (value, title, help)) in choices.into_iter().enumerate() {
        let r = ui
            .scope(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let r = ui.radio(current == value, egui::RichText::new(title).font(regular(12.5)).color(t.text));
                ui.horizontal(|ui| {
                    ui.add_space(24.0);
                    w::caption(ui, help, t);
                });
                r
            })
            .inner;
        crate::chrome::focus_ring(ui, &r, Radius::CONTROL);
        app.hit.add("aiFallback", u64::try_from(i).unwrap_or(0), r.rect);
        if r.clicked() && current != value {
            let done = run(app, "ai.setFallback", json!({"value": value.name()}));
            said(app, done, crate::i18n::fmt(tl!("When a runtime can't answer: {choice}."), &[("choice", title)]));
        }
    }
    w::caption(ui, tl!("If a runtime tries to use a tool, MailCraft stops it and always tells you, whichever you choose."), t);
}

/// The dialog over the page, when there is one: drawn, moved on, closed.
fn dialogs(app: &mut MailApp, ctx: &egui::Context, facts: &Facts) {
    let Some(open) = app.ai.runtimes.dialog.clone() else { return };
    if let Some(next) = signin::follow(&open.dialog, facts, crate::now_ms()) {
        app.ai.runtimes.dialog = Some(Open::new(next, open.opener));
        return;
    }
    let mut understood = open.understood;
    let said = match &open.dialog {
        RtDialog::Consent { runtime, account } => super::runtime_consent::dialog(app, ctx, runtime, account, facts, !open.focused, &mut understood),
        _ => signin::show(app, ctx, &open, facts, &mut understood),
    };
    match said {
        Said::Stay => {
            if let Some(o) = app.ai.runtimes.dialog.as_mut() {
                o.focused = true;
                o.understood = understood;
            }
        }
        Said::To(next) => app.ai.runtimes.dialog = Some(Open::new(next, open.opener)),
        Said::Close => {
            app.ai.runtimes.dialog = None;
            app.ai.runtimes.refocus = open.opener.map(|id| (id, ctx.cumulative_pass_nr().saturating_add(2)));
        }
    }
}

/// Escape for the page (before the window's own): it closes the open details panel and gives the
/// keyboard back to its Details button. A dialog takes its own Escape.
pub(crate) fn escape(app: &mut MailApp, ctx: &egui::Context) {
    let s = &app.ai.runtimes;
    if s.dialog.is_some() || s.open.is_none() || s.drawn.saturating_add(1) < ctx.cumulative_pass_nr() || app.dialog.is_some() {
        return;
    }
    if !crate::keys::take_escape_any(ctx, crate::keys::main_layer()) {
        return;
    }
    if let Some(id) = s.open.as_ref().and_then(|r| s.details_ids.get(&r.to_string())).copied() {
        ctx.memory_mut(|m| m.request_focus(id));
    }
    app.ai.runtimes.open = None;
}

/// The window's runtime dialog's name, for the tests.
#[cfg(test)]
pub(crate) fn dialog_name(app: &MailApp) -> Option<&'static str> {
    app.ai.runtimes.dialog.as_ref().map(|o| o.dialog.name())
}

impl MailApp {
    /// For screenshots and tests (ui_shot's stand-in host): what a widened host would say of
    /// `runtime` beyond its status (B11 C2): `{"status", "reason", "folder", "usesPlan", "email",
    /// "plan", "toolStopped", "optedIn"}`. Read as anything from outside.
    #[doc(hidden)]
    pub fn stand_in_runtime_extra(&mut self, runtime: &str, extra: &Value) {
        if RuntimeId::parse(runtime).is_ok() && self.ai.runtimes.extra.len() < 16 {
            self.ai.runtimes.extra.insert(runtime.to_string(), Extra::read(extra));
        }
    }

    /// For screenshots: opens the page's dialog `name` (`chatgpt`, `chatgptWaiting`, `claude`,
    /// `claudeNoTerminal`, `uar`, `signOut`, `consent:<runtime>:<account>`), as the owner's click
    /// would. Never reached from the control channel or an agent.
    #[doc(hidden)]
    pub fn stand_in_runtime_dialog(&mut self, name: &str) {
        let until_ms = crate::now_ms() + signin::WAIT_MS;
        let dialog = match name {
            "chatgpt" => RtDialog::ChatGpt(signin::GptStep::Start),
            "chatgptWaiting" => RtDialog::ChatGpt(signin::GptStep::Waiting { until_ms }),
            "claude" => RtDialog::Claude(signin::ClaudeStep::Terminal { until_ms }),
            "claudeNoTerminal" => RtDialog::Claude(signin::ClaudeStep::NoTerminal { until_ms }),
            "uar" => RtDialog::UarOptIn,
            "signOut" => RtDialog::ChatGptSignOut,
            other => match other.strip_prefix("consent:").and_then(|rest| rest.split_once(':')) {
                Some((runtime, account)) => match RuntimeId::parse(runtime) {
                    Ok(runtime) => RtDialog::Consent { runtime, account: account.to_string() },
                    Err(_) => return,
                },
                None => return,
            },
        };
        open_dialog(self, dialog, None);
    }

    /// For screenshots: opens the details panel of `runtime`.
    #[doc(hidden)]
    pub fn stand_in_runtime_details(&mut self, runtime: &str) {
        self.ai.runtimes.open = RuntimeId::parse(runtime).ok();
    }
}
