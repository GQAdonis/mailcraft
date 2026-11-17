// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Find by Description (#ai): the panel under the search box, behind the button left of
//! Advanced Search's. The owner describes what they are looking for; `ai.narrow` asks the model
//! for a search in MailCraft's query language (only the words typed go, never mail), and
//! `ai.narrowResult` reads what came back. The proposal is shown with the search box's keyword
//! pills, editable, and **the list doesn't change until the owner chooses Use This Search**,
//! which runs `view.search` with it in the scope in view, in place of the search there was.
//!
//! The panel is built like Advanced Search's: part of the main window, drawn just over it, closed
//! by Escape (which first stops a request under way), a click outside it, Mail leaving the
//! front, File or a dialog opening (but not the gateway question its own request is waiting on),
//! another mailbox, or Advanced Search opening. Closing it calls off a request under way. When a proposal arrives the keyboard goes to the proposal, never to
//! Use This Search, so a second Enter can't accept a search the owner hasn't seen.

use egui::{Area, Frame, Id, Key, LayerId, Order, Rect, Stroke, Ui, pos2, vec2};
use mailcraft_engine::{Module, Scope};
use mailcraft_model::ai::RequestState;
use serde_json::{Value, json};

use super::errors::Trouble;
use super::{Providers, flag, text};
use crate::MailApp;
use crate::theme::{Tokens, regular, semibold};

/// The panel's width as it would have it.
const PANEL_W: f32 = 480.0;
/// Most characters of a description.
pub(crate) const MAX_TEXT: usize = 500;
/// Most characters of a proposal kept in the field.
const MAX_QUERY: usize = 1_000;

/// Where the panel is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    /// Waiting for a description.
    Ready,
    /// Request `request` is with the model.
    Asking { request: u64 },
    /// A search to look at: [`Panel::query`] (empty after Write It Myself).
    Proposal,
    /// Nothing usable came back.
    Trouble(Trouble),
}

/// Where the keyboard goes at the next frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    None,
    Request,
    Proposal,
}

/// The panel while it is open.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Panel {
    /// The owner's description.
    pub(crate) text: String,
    /// The search proposed, as the owner may have edited it.
    pub(crate) query: String,
    pub(crate) stage: Stage,
    /// What the provider itself said of a failure, for the notice's details: data, never read.
    pub(crate) details: String,
    focus: Focus,
    /// The mailbox it was opened on ([`mailcraft_engine::Session::openings`]).
    openings: u64,
}

impl Panel {
    fn new(app: &MailApp) -> Panel {
        Panel {
            text: String::new(),
            query: String::new(),
            stage: Stage::Ready,
            details: String::new(),
            focus: Focus::Request,
            openings: app.session.openings(),
        }
    }
}

fn panel_id() -> Id {
    Id::unique("find_by_description")
}

fn request_field() -> Id {
    panel_id().with("request")
}

fn proposal_field() -> Id {
    panel_id().with("proposal")
}

/// Whether the panel may open now: in Mail, with the search box drawn, and nothing over the
/// window.
fn can_open(app: &MailApp) -> Result<(), &'static str> {
    if app.session.view.module != Module::Mail || !crate::keys::takes_commands(app) {
        return Err("Find by Description is Mail's: it opens with Mail in front and nothing over the window");
    }
    if !app.search_box_shown {
        return Err("the window is too narrow for the search box");
    }
    Ok(())
}

/// Opens the panel or closes it (the button in the search box). Advanced Search and the list of
/// suggestions close when it opens; a request under way is called off when it closes.
pub(crate) fn set_open(app: &mut MailApp, open: bool) -> Result<(), &'static str> {
    if open && app.ai.narrow.is_none() {
        can_open(app)?;
        app.advanced_search = None;
        app.ai.narrow = Some(Panel::new(app));
        if let Some(ctx) = &app.ctx {
            crate::search_suggest::Suggesting::close(ctx);
        }
    } else if !open {
        close(app);
    }
    Ok(())
}

/// Closes the panel, calling off what it asked.
fn close(app: &mut MailApp) {
    if let Some(Panel { stage: Stage::Asking { request }, .. }) = app.ai.narrow.take() {
        super::cancel(app, request);
    }
}

/// The button in the search box (`r`, the size of Advanced Search's, just left of it): opens the
/// panel or closes it.
pub(crate) fn button(app: &mut MailApp, ui: &mut Ui, r: Rect) {
    let t = Tokens::get(ui.ctx());
    let resp = ui.interact(r, ui.scope_id().with("find_by_description_button"), egui::Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tl!("Find by Description")));
    let resp = resp.on_hover_text(tl!("Find by Description"));
    app.hit.add("findByDescriptionButton", 0, r);
    let open = app.ai.narrow.is_some();
    if resp.hovered() || open {
        ui.painter().rect_filled(r, 4.0, if open { t.pressed } else { t.hover });
    }
    if resp.has_focus() {
        ui.painter().rect_stroke(r, 4.0, Stroke::new(2.0, t.focus_ring), egui::StrokeKind::Inside);
    }
    crate::icons::paint(ui.painter(), Rect::from_center_size(r.center(), vec2(15.0, 15.0)), "describe", t.text_dim, t.accent);
    if resp.clicked()
        && let Err(e) = set_open(app, !open)
    {
        app.say_refused("ui.findByDescription", e);
    }
}

/// Asks the model for a search for what the panel's field says.
pub(crate) fn suggest(app: &mut MailApp) -> Result<(), String> {
    let panel = app.ai.narrow.as_ref().ok_or("Find by Description isn't open")?;
    let words = panel.text.trim().to_string();
    if words.is_empty() {
        return Err(tl!("Describe what you're looking for first").to_string());
    }
    if let Stage::Asking { request } = panel.stage {
        super::cancel(app, request);
    }
    let made = app.execute("ai.narrow", json!({"text": words}))?;
    let request = made.get("request").and_then(|r| r.get("id")).and_then(Value::as_u64).ok_or("the model request wasn't made")?;
    if let Some(panel) = app.ai.narrow.as_mut() {
        panel.stage = Stage::Asking { request };
        panel.query.clear();
    }
    Ok(())
}

/// Runs the proposal as the search, in the scope in view, in place of the search there was, and
/// closes the panel.
pub(crate) fn accept(app: &mut MailApp) -> Result<(), String> {
    let panel = app.ai.narrow.as_ref().ok_or("Find by Description isn't open")?;
    if panel.stage != Stage::Proposal {
        return Err(tl!("There is no suggested search to use").to_string());
    }
    let query = panel.query.trim().to_string();
    if query.is_empty() {
        return Err(tl!("The suggested search is empty").to_string());
    }
    let scope = app.session.view.scope;
    let code = crate::chrome::scopes().into_iter().find(|(s, ..)| *s == scope).map_or("folder", |(_, _, code)| code);
    app.execute("view.search", json!({"query": query, "scope": code}))?;
    let taken = app.session.view.search.clone();
    let _ = app.execute("view.recentSearches", json!({"remember": taken}));
    app.ai.narrow = None;
    app.search_text = taken;
    if let Some(ctx) = &app.ctx {
        crate::chrome::caret_at_end(ctx, &app.search_text);
    }
    app.search_focus = true;
    app.status(tl!("Search applied."));
    Ok(())
}

/// The scope a search runs in, by name.
fn scope_name(scope: Scope) -> &'static str {
    crate::chrome::scopes().into_iter().find(|(s, ..)| *s == scope).map_or(tl!("Current Folder"), |(_, name, _)| name)
}

/// `ui.findByDescription`: `{"open"?, "text"?, "suggest"?, "query"?, "accept"?}`.
pub(crate) fn command(app: &mut MailApp, p: &Value) -> Result<Value, String> {
    let open = flag(p, "open")?;
    let words = text(p, "text", MAX_TEXT)?;
    let query = text(p, "query", MAX_QUERY)?;
    let (go, take) = (flag(p, "suggest")?, flag(p, "accept")?);
    if open == Some(false) {
        if words.is_some() || query.is_some() || go == Some(true) || take == Some(true) {
            return Err("`open`: false goes with nothing else".into());
        }
        set_open(app, false)?;
        return Ok(answer(app));
    }
    let wants = open == Some(true) || words.is_some() || query.is_some() || go == Some(true) || take == Some(true);
    match (open, wants) {
        (None, false) => {
            let now = app.ai.narrow.is_none();
            set_open(app, now)?;
        }
        _ => set_open(app, true)?,
    }
    if let Some(panel) = app.ai.narrow.as_mut() {
        if let Some(words) = words {
            panel.text = words;
        }
        if let Some(query) = query {
            panel.query = query;
            panel.stage = Stage::Proposal;
            panel.focus = Focus::Proposal;
        }
    }
    if go == Some(true) {
        suggest(app)?;
    }
    if take == Some(true) {
        accept(app)?;
    }
    Ok(answer(app))
}

/// What the commands answer: where the panel is.
fn answer(app: &MailApp) -> Value {
    let Some(panel) = &app.ai.narrow else { return json!({"open": false, "query": app.session.view.search}) };
    let (stage, request) = match panel.stage {
        Stage::Ready => ("ready", None),
        Stage::Asking { request } => ("asking", Some(request)),
        Stage::Proposal => ("proposal", None),
        Stage::Trouble(_) => ("trouble", None),
    };
    json!({"open": true, "stage": stage, "request": request, "text": panel.text, "proposal": panel.query})
}

/// Once a frame, before anything is drawn: closes the panel when what it was opened for has gone,
/// and takes in what came for its request.
pub(crate) fn follow(app: &mut MailApp) {
    let Some(panel) = &app.ai.narrow else { return };
    let gone = !app.search_box_shown
        || panel.openings != app.session.openings()
        || app.session.view.module != Module::Mail
        || app.ui.backstage
        // The gateway question is part of the request under way (the route check waits for its
        // answer): it neither closes the panel nor calls the request off (M1, SCN-BASE-003).
        || (app.dialog.is_some() && !super::settings_ask::gateway_dialog_open(app))
        || app.advanced_search.is_some();
    if gone {
        close(app);
        return;
    }
    let Stage::Asking { request } = panel.stage else { return };
    match super::state_of(app, request) {
        Some(RequestState::Pending | RequestState::Streaming) => {}
        // Gone, or called off: back to the description, as it was.
        None | Some(RequestState::Cancelled) => set_stage(app, Stage::Ready, Focus::Request),
        Some(RequestState::Failed) => {
            // The kind the host named, never the words: what the provider said goes to the details.
            let kind = app.session.ai.get(request).and_then(|t| t.error_kind);
            failed(app, request, Trouble::of_kind(kind));
        }
        Some(RequestState::Done) => took(app, request),
    }
}

fn set_stage(app: &mut MailApp, stage: Stage, focus: Focus) {
    if let Some(panel) = app.ai.narrow.as_mut() {
        if !matches!(stage, Stage::Trouble(_)) {
            panel.details.clear();
        }
        panel.stage = stage;
        if focus != Focus::None {
            panel.focus = focus;
        }
    }
}

/// Request `request` failed with `trouble`: said in the status bar and shown in the panel, with
/// what the provider itself said kept for the notice's details.
fn failed(app: &mut MailApp, request: u64, trouble: Trouble) {
    let said = app.session.ai.get(request).map(|t| t.provider_text.clone()).unwrap_or_default();
    let address = Providers::of(app).active().map(|p| p.address()).unwrap_or_default();
    app.status(trouble.words(&address, true).0);
    set_stage(app, Stage::Trouble(trouble), Focus::None);
    if let Some(panel) = app.ai.narrow.as_mut() {
        panel.details = said;
    }
}

/// The model answered request `request`: a search to show, the question asked again once, or
/// nothing usable.
fn took(app: &mut MailApp, request: u64) {
    let read = match app.execute("ai.narrowResult", json!({"requestId": request})) {
        Ok(read) => read,
        Err(e) => {
            log::debug!("the suggested search couldn't be read: {e}");
            set_stage(app, Stage::Trouble(Trouble::Malformed), Focus::None);
            return;
        }
    };
    // A failed request is answered, not refused: `{"state": "failed", "kind", …}`.
    if let Some(trouble) = Trouble::of_result(&read) {
        failed(app, request, trouble);
        return;
    }
    let query = read.get("query").and_then(Value::as_str).filter(|_| read.get("valid").and_then(Value::as_bool) == Some(true));
    if let Some(query) = query {
        let query: String = query.chars().take(MAX_QUERY).collect();
        if let Some(panel) = app.ai.narrow.as_mut() {
            panel.query = query;
        }
        set_stage(app, Stage::Proposal, Focus::Proposal);
        app.status(tl!("Suggested a search."));
        return;
    }
    // Asked again once, by the engine, with what was wrong: the host sends it as any other.
    match read.get("retry").and_then(|r| r.get("id")).and_then(Value::as_u64) {
        Some(again) => set_stage(app, Stage::Asking { request: again }, Focus::None),
        None => {
            set_stage(app, Stage::Trouble(Trouble::Malformed), Focus::None);
            app.status(tl!("No search came back."));
        }
    }
}

/// Escape for the open panel: while a request is with the model it stops it (the description
/// stays); otherwise it closes the panel, and the keyboard goes back to the search box.
pub(crate) fn escape(app: &mut MailApp, ctx: &egui::Context) {
    let Some(panel) = &app.ai.narrow else { return };
    // A dialog open over the panel (the gateway question) takes its own Escape.
    if app.dialog.is_some() {
        return;
    }
    if !crate::keys::take_escape_any(ctx, crate::keys::main_layer()) {
        return;
    }
    if let Stage::Asking { request } = panel.stage {
        super::cancel(app, request);
        set_stage(app, Stage::Ready, Focus::Request);
        return;
    }
    app.ai.narrow = None;
    app.search_focus = true;
}

/// What the owner chose in the panel this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    Suggest,
    AnswerInstead,
    Accept,
    WriteOwn,
    Stop,
    Options,
    Close,
}

/// An Enter pressed in the description, taken out of the input: `Some(true)` with Ctrl (⌘ on a
/// Mac), Answer Instead; `Some(false)` alone, Suggest Search. Shift+Enter stays a new line.
fn enter_in(i: &mut egui::InputState) -> Option<bool> {
    let mut got = None;
    i.events.retain(|e| match e {
        egui::Event::Key { key: Key::Enter, pressed: true, modifiers, .. } if !modifiers.shift => {
            got = Some(modifiers.command);
            false
        }
        _ => true,
    });
    got
}

/// A caption in `text_dim`.
fn caption(ui: &mut Ui, text: &str, t: &Tokens) -> egui::Response {
    ui.add(egui::Label::new(egui::RichText::new(text).font(regular(11.5)).color(t.text_dim)).wrap())
}

/// The privacy line under the description: what goes where.
fn privacy_line(providers: &Providers) -> Option<String> {
    let p = providers.active()?;
    Some(if p.remote() {
        crate::i18n::fmt(tl!("Only what you type here goes to {host}, encrypted. No mail is sent."), &[("host", &p.host())])
    } else {
        crate::i18n::fmt(tl!("Only what you type here goes to the model at {address}. No mail is sent."), &[("address", &p.address())])
    })
}

/// A notice: an icon, a few words in bold, the rest after them.
pub(crate) fn notice(ui: &mut Ui, trouble: Trouble, lead: &str, rest: &str, t: &Tokens) {
    let hue = trouble.hue(t);
    let icon = if hue == t.blue { "info" } else { "highImportance" };
    crate::reading::remote::notice(hue, t).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 2.0);
            let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), egui::Sense::hover());
            crate::icons::paint(ui.painter(), r.shrink(1.0), icon, t.text, hue);
            ui.add(egui::Label::new(egui::RichText::new(lead).font(semibold(11.5)).color(t.text)).wrap());
            ui.add(egui::Label::new(egui::RichText::new(rest).font(regular(11.5)).color(t.text_dim)).wrap());
        });
    });
}

/// The proposal's field, its keywords drawn as the search box draws them. Enter in it uses it.
fn proposal(ui: &mut Ui, query: &mut String, label: egui::Id, focus: bool, t: &Tokens) -> bool {
    let font = regular(13.0);
    let slot = ui.painter().add(egui::Shape::Noop);
    let mut layouter = |ui: &Ui, text: &dyn egui::TextBuffer, _wrap: f32| {
        let pills = crate::search_pills::pills(text.as_str(), None);
        ui.fonts_mut(|f| f.layout_job(crate::search_pills::layout(text.as_str(), &pills, font.clone(), t)))
    };
    let output = egui::TextEdit::singleline(query)
        .id(proposal_field())
        .font(font.clone())
        .char_limit(MAX_QUERY)
        .desired_width(ui.available_width())
        .layouter(&mut layouter)
        .show(ui);
    let pills = crate::search_pills::pills(query, None);
    let shapes = crate::search_pills::shapes(&output.galley, output.galley_pos, &pills, t);
    ui.painter().with_clip_rect(output.text_clip_rect).set(slot, egui::Shape::Vec(shapes));
    let resp = crate::text_menu::field(output.response.response).labelled_by(label);
    if focus {
        resp.request_focus();
    }
    resp.lost_focus() && crate::keys::enter(ui.ctx(), ui.layer_id())
}

/// Draws the open panel under the search box (`anchor`); `button` is the button that opens and
/// closes it.
pub(crate) fn show(app: &mut MailApp, ctx: &egui::Context, anchor: Rect, button: Rect) {
    let Some(mut panel) = app.ai.narrow.take() else { return };
    let t = Tokens::get(ctx);
    let providers = Providers::of(app);
    let ready = super::model_ready(app);
    let address = providers.active().map(|p| p.address()).unwrap_or_default();
    let scope = scope_name(app.session.view.scope);
    let list_open = egui::Popup::is_any_open(ctx);
    let screen = ctx.content_rect();
    let width = PANEL_W.max(anchor.width()).min((screen.width() - 16.0).max(0.0));
    let x = anchor.min.x.min(screen.max.x - 8.0 - width).max(screen.min.x + 8.0);
    let top = anchor.max.y + 2.0;
    let layer = LayerId::new(Order::Background, panel_id());
    // Just over the main window, as a part of it: under its dialogs and lists.
    ctx.set_sublayer(LayerId::background(), layer);
    let mut act: Option<Act> = None;
    let focus = std::mem::replace(&mut panel.focus, Focus::None);
    let asking = matches!(panel.stage, Stage::Asking { .. });
    let area = Area::new(panel_id()).order(Order::Background).fixed_pos(pos2(x, top)).show(ctx, |ui| {
        Frame::popup(ui.style()).fill(t.menu).stroke(Stroke::new(1.0, t.border_strong)).inner_margin(12.0).show(ui, |ui| {
            ui.set_width(width - 24.0);
            egui::ScrollArea::vertical().max_height((screen.max.y - top - 80.0).max(120.0)).show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
                // No model, and nothing asked: nothing can be sent, and the panel says where to
                // set one up instead of offering a field.
                if !ready && panel.stage == Stage::Ready {
                    let (lead, rest) = Trouble::NoModel.words(&address, true);
                    notice(ui, Trouble::NoModel, &lead, &rest, &t);
                    ui.horizontal_wrapped(|ui| {
                        if crate::widgets::primary_button(ui, tl!("Open Options"), true).clicked() {
                            act = Some(Act::Options);
                        }
                        if crate::widgets::plain_button(ui, tl!("Cancel")).clicked() {
                            act = Some(Act::Close);
                        }
                    });
                    return;
                }
                let label = ui.label(egui::RichText::new(tl!("Describe what you're looking for")).font(regular(12.5)).color(t.text)).id;
                if ui.memory(|m| m.has_focus(request_field())) {
                    let pressed = ui.input_mut(|i| {
                        crate::keys::drop_repeats(i, &[Key::Enter]);
                        enter_in(i)
                    });
                    match pressed {
                        Some(true) if !panel.text.trim().is_empty() => act = Some(Act::AnswerInstead),
                        Some(false) if !panel.text.trim().is_empty() && !asking => act = Some(Act::Suggest),
                        _ => {}
                    }
                }
                let field = egui::TextEdit::multiline(&mut panel.text)
                    .id(request_field())
                    .hint_text(tl!("For example: invoices from Ada last month with a PDF"))
                    .desired_rows(2)
                    .desired_width(f32::INFINITY)
                    .char_limit(MAX_TEXT)
                    .interactive(!asking);
                let resp = crate::text_menu::field(ui.add(field)).labelled_by(label);
                if focus == Focus::Request {
                    resp.request_focus();
                }
                app.hit.add("findByDescriptionText", 0, resp.rect);
                if let Some(line) = privacy_line(&providers) {
                    caption(ui, &line, &t);
                }
                ui.horizontal_wrapped(|ui| {
                    let some = !panel.text.trim().is_empty();
                    let go = crate::widgets::primary_button(ui, tl!("Suggest Search"), some && !asking);
                    app.hit.add("suggestSearch", 0, go.rect);
                    if go.clicked() {
                        act = Some(Act::Suggest);
                    }
                    let instead = crate::widgets::plain_button(ui, tl!("Answer Instead"))
                        .on_hover_text(tl!("Ask a question about your mail with these words, in the Answers pane"));
                    app.hit.add("answerInstead", 0, instead.rect);
                    if instead.clicked() && some {
                        act = Some(Act::AnswerInstead);
                    }
                    if crate::widgets::plain_button(ui, tl!("Cancel")).clicked() {
                        act = Some(if asking { Act::Stop } else { Act::Close });
                    }
                });
                if panel.stage == Stage::Ready {
                    return;
                }
                ui.add_space(2.0);
                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), egui::Sense::hover());
                ui.painter().hline(r.x_range(), r.center().y, Stroke::new(1.0, t.rule));
                ui.add_space(2.0);
                match panel.stage {
                    Stage::Ready => {}
                    Stage::Asking { .. } => {
                        ui.horizontal(|ui| {
                            crate::software::spinner(ui, 14.0, Some(t.text_dim));
                            caption(ui, tl!("Asking the model…"), &t);
                        });
                    }
                    Stage::Proposal => {
                        let named = crate::i18n::fmt(tl!("Suggested search, in {scope}"), &[("scope", scope)]);
                        let label = caption(ui, &named, &t).id;
                        let enter = proposal(ui, &mut panel.query, label, focus == Focus::Proposal, &t);
                        let some = !panel.query.trim().is_empty();
                        if enter && some {
                            act = Some(Act::Accept);
                        }
                        ui.horizontal_wrapped(|ui| {
                            let use_it = crate::widgets::primary_button(ui, tl!("Use This Search"), some);
                            app.hit.add("useThisSearch", 0, use_it.rect);
                            if use_it.clicked() {
                                act = Some(Act::Accept);
                            }
                            if crate::widgets::plain_button(ui, tl!("Try Again")).clicked() {
                                act = Some(Act::Suggest);
                            }
                            if crate::widgets::plain_button(ui, tl!("Cancel")).clicked() {
                                act = Some(Act::Close);
                            }
                        });
                    }
                    Stage::Trouble(trouble) => {
                        let (lead, rest) = trouble.words(&address, true);
                        notice(ui, trouble, &lead, &rest, &t);
                        super::errors::details(ui, "narrow", &panel.details, &t);
                        ui.horizontal_wrapped(|ui| {
                            if trouble != Trouble::NoModel && crate::widgets::plain_button(ui, tl!("Try Again")).clicked() {
                                act = Some(Act::Suggest);
                            }
                            if matches!(trouble, Trouble::Malformed | Trouble::TooLarge)
                                && crate::widgets::plain_button(ui, tl!("Write It Myself")).clicked()
                            {
                                act = Some(Act::WriteOwn);
                            }
                            if trouble.options_help() && crate::widgets::plain_button(ui, tl!("Open Options")).clicked() {
                                act = Some(Act::Options);
                            }
                            if crate::widgets::plain_button(ui, tl!("Cancel")).clicked() {
                                act = Some(Act::Close);
                            }
                        });
                    }
                }
            });
        });
    });
    let rect = area.response.rect;
    app.hit.add("findByDescription", 0, rect);
    // A click outside the panel closes it (and calls off what it asked).
    let outside = ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !rect.contains(p) && !button.contains(p)));
    app.ai.narrow = Some(panel);
    if outside && !list_open && act.is_none() {
        close(app);
        return;
    }
    let Some(act) = act else { return };
    let done = match act {
        Act::Suggest => suggest(app),
        Act::Accept => accept(app),
        Act::AnswerInstead => {
            let words = app.ai.narrow.as_ref().map(|p| p.text.trim().to_string()).unwrap_or_default();
            close(app);
            super::answers::ask(app, &words)
        }
        Act::WriteOwn => {
            if let Some(panel) = app.ai.narrow.as_mut() {
                panel.query.clear();
                panel.stage = Stage::Proposal;
                panel.focus = Focus::Proposal;
            }
            Ok(())
        }
        Act::Stop => {
            if let Some(Stage::Asking { request }) = app.ai.narrow.as_ref().map(|p| p.stage) {
                super::cancel(app, request);
            }
            set_stage(app, Stage::Ready, Focus::Request);
            Ok(())
        }
        Act::Options => {
            close(app);
            app.run("ui.aiSettings", json!({})).map(|_| ())
        }
        Act::Close => {
            close(app);
            app.search_focus = true;
            Ok(())
        }
    };
    if let Err(e) = done {
        app.say_refused("ui.findByDescription", &e);
    }
}
