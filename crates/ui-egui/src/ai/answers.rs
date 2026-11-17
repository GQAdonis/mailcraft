// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Answers pane (#ai): at the right of Mail, beside the reading pane, so that a source opens
//! in the reading pane while its answer stays in view. A question goes to `ai.ask`, which finds
//! the messages that answer it and asks the model; the answer grows as it is written, can be
//! stopped at once (`ai.cancel`), and names at most three sources, which open the real messages
//! (`mail.select`; Ctrl+Enter or a double click, `ui.openWindow`). When no message answers the
//! question, nothing is sent to any model.
//!
//! The answer is model output: it is drawn as display blocks ([`super::answer_text`]), never as
//! HTML, and a note under it says every time that a model wrote it and that it can be wrong.
//! There is no conversation: each question is asked afresh, and the pane starts clean each time
//! it is opened.

use std::sync::mpsc::{Receiver, TryRecvError};

use egui::{Align, Align2, Layout, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_engine::Module;
use mailcraft_html::Block;
use mailcraft_model::ai::RequestState;
use serde_json::{Value, json};

use super::errors::Trouble;
use super::{Providers, flag, text};
use crate::MailApp;
use crate::theme::{Tokens, paint_clipped, regular, semibold};

/// The pane's width as it opens, the least and the most it may be given.
const DEFAULT_W: f32 = 360.0;
const MIN_W: f32 = 280.0;
const MAX_W: f32 = 560.0;
/// What the message list and the reading pane keep beside it at the least.
const BESIDE: f32 = 240.0 + 260.0;
/// Most characters of a question.
const MAX_QUESTION: usize = 2_000;
/// How long the question's vector is waited for before the question is asked without it, in ms.
const EMBED_WAIT_MS: f64 = 1_500.0;
/// Most characters of the question shown in the pane (all of it in its tip).
const SHOWN_QUESTION: usize = 280;

/// Where an answer is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    /// Nothing asked yet.
    Idle,
    /// The question's vector is being made, or the mail looked through, or the model hasn't
    /// written anything yet.
    Looking,
    /// The model is writing.
    Writing,
    Done,
    /// Stopped by the owner: what came stays, marked incomplete.
    Stopped,
    /// No message answers the question: nothing was sent.
    NothingFound,
    /// It failed; what came before stays.
    Trouble(Trouble),
}

/// A source shown under an answer: its number in the answer, and what the mailbox says of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Cited {
    pub(crate) number: usize,
    pub(crate) id: u64,
    pub(crate) sender: String,
    pub(crate) subject: String,
    pub(crate) date: String,
}

/// Where the keyboard goes at the next frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    None,
    Stop,
    FirstSource,
    Question,
}

/// The pane while it is open.
pub(crate) struct Pane {
    /// The question asked.
    pub(crate) question: String,
    /// The field at the pane's foot.
    pub(crate) field: String,
    pub(crate) stage: Stage,
    /// The model request, once there is one.
    pub(crate) request: Option<u64>,
    /// The question's vector on its way, and since when it is waited for.
    embedding: Option<(Receiver<Option<(Vec<f32>, String)>>, f64)>,
    /// How many messages went to the model with the question.
    pub(crate) sent: usize,
    /// The answer as far as it came, and its blocks.
    pub(crate) text: String,
    blocks: Vec<Block>,
    blocks_of: String,
    pub(crate) cited: Vec<Cited>,
    /// What the provider itself said of a failure, for the notice's details: data, never read.
    pub(crate) details: String,
    /// Who answered, as the host said (provider, model): data, cut and stripped.
    pub(crate) answered_by: Option<(String, String)>,
    /// The primary in use when the question was asked (its id, and its name as shown).
    pub(crate) primary: Option<(String, String)>,
    /// The answer names no message, and isn't the sentence that says the messages don't answer.
    pub(crate) uncited: bool,
    /// The answer's text was shown but its JSON couldn't be read: its sources are unknown.
    pub(crate) sources_unreadable: bool,
    focus: Focus,
    /// The mailbox it was opened on.
    openings: u64,
}

impl Pane {
    fn new(app: &MailApp) -> Pane {
        Pane {
            question: String::new(),
            field: String::new(),
            stage: Stage::Idle,
            request: None,
            embedding: None,
            sent: 0,
            text: String::new(),
            blocks: Vec::new(),
            blocks_of: String::new(),
            cited: Vec::new(),
            details: String::new(),
            answered_by: None,
            primary: None,
            uncited: false,
            sources_unreadable: false,
            focus: Focus::Question,
            openings: app.session.openings(),
        }
    }

    fn working(&self) -> bool {
        matches!(self.stage, Stage::Looking | Stage::Writing)
    }

    /// The answer's blocks, made again only when its text changed.
    fn blocks(&mut self) -> &[Block] {
        if self.blocks_of != self.text {
            self.blocks = mailcraft_html::markdown_blocks(&self.text);
            self.blocks_of = self.text.clone();
        }
        &self.blocks
    }
}

/// Opens the pane (clean), or closes it, calling off what it asked.
pub(crate) fn set_open(app: &mut MailApp, open: bool) {
    if open {
        if app.ai.answers.is_none() {
            app.ai.answers = Some(Pane::new(app));
        }
    } else {
        close(app);
    }
}

fn close(app: &mut MailApp) {
    if let Some(Pane { request: Some(request), .. }) = app.ai.answers.take() {
        super::cancel(app, request);
    }
}

/// Asks `question` in the pane, opening it: what was asked before is called off and let go of.
pub(crate) fn ask(app: &mut MailApp, question: &str) -> Result<(), String> {
    let question = question.trim().to_string();
    if question.is_empty() {
        return Err(tl!("Type a question first").to_string());
    }
    if question.chars().nth(MAX_QUESTION).is_some() {
        return Err(tl!("A question is at most 2,000 characters").to_string());
    }
    if app.session.view.module != Module::Mail {
        app.execute("view.mail", json!({}))?;
    }
    close(app);
    // The first question of the session looks for the on-device backup, when there is one.
    super::settings::look_before_first_question(app);
    let mut pane = Pane::new(app);
    pane.question = question.clone();
    pane.stage = Stage::Looking;
    pane.focus = Focus::Stop;
    // The question's vector first, when the host makes them: waited for a moment at most, never
    // on the window's thread.
    if let Some(embed) = app.services.embed_query.as_ref() {
        pane.embedding = Some((embed(question), crate::now_ms()));
        app.ai.answers = Some(pane);
        return Ok(());
    }
    app.ai.answers = Some(pane);
    send(app, None)
}

/// `ai.ask` with the pane's question (and its vector, when there is one).
fn send(app: &mut MailApp, vector: Option<(Vec<f32>, String)>) -> Result<(), String> {
    let Some(question) = app.ai.answers.as_ref().map(|p| p.question.clone()) else { return Ok(()) };
    let mut params = json!({"question": question, "nonce": super::nonce(app)});
    // As much as the model that is to answer takes (the primary's, else the confirmed backup's),
    // when its settings say (256 to 1,048,576 tokens, as `ai.ask` takes).
    let providers = Providers::of(app);
    if let (Some(window), Some(map)) = (providers.answering_window(), params.as_object_mut()) {
        map.insert("contextWindow".into(), json!(window));
    }
    let primary = providers.active().map(|p| (p.id.clone(), primary_name(p)));
    if let Some(pane) = app.ai.answers.as_mut() {
        pane.primary = primary;
    }
    if let (Some((vector, embedder)), Some(map)) = (vector, params.as_object_mut()) {
        map.insert("queryVector".into(), json!(vector));
        map.insert("embedder".into(), json!(embedder));
    }
    let asked = match app.execute("ai.ask", params) {
        Ok(asked) => asked,
        Err(e) => {
            if let Some(pane) = app.ai.answers.as_mut() {
                pane.stage = Stage::Trouble(Trouble::Other);
            }
            return Err(e);
        }
    };
    let Some(pane) = app.ai.answers.as_mut() else { return Ok(()) };
    if asked.get("nothingFound").and_then(Value::as_bool) == Some(true) {
        pane.stage = Stage::NothingFound;
        pane.focus = Focus::Question;
        app.status(tl!("Nothing in your mail seems to answer this. Nothing was sent."));
        return Ok(());
    }
    pane.request = asked.get("request").and_then(|r| r.get("id")).and_then(Value::as_u64);
    pane.sent = asked.get("sources").and_then(Value::as_array).map_or(0, Vec::len);
    if pane.request.is_none() {
        pane.stage = Stage::Trouble(Trouble::Other);
    }
    Ok(())
}

/// Stops the answer being written: what came stays.
pub(crate) fn stop(app: &mut MailApp) {
    let Some(pane) = app.ai.answers.as_mut() else { return };
    if !pane.working() {
        return;
    }
    pane.embedding = None;
    pane.stage = Stage::Stopped;
    pane.focus = Focus::Question;
    if let Some(request) = pane.request {
        super::cancel(app, request);
    }
}

/// `ui.answers`: `{"open"?: bool, "question"?: string, "stop"?: bool}`.
pub(crate) fn command(app: &mut MailApp, p: &Value) -> Result<Value, String> {
    let open = flag(p, "open")?;
    let question = text(p, "question", MAX_QUESTION)?;
    let halt = flag(p, "stop")?;
    match (open, question, halt) {
        (Some(false), None, None | Some(false)) => set_open(app, false),
        (Some(false), ..) => return Err("`open`: false goes with nothing else".into()),
        (_, Some(question), _) => ask(app, &question)?,
        (_, None, Some(true)) => stop(app),
        (Some(true), None, _) => {
            if app.session.view.module != Module::Mail {
                app.execute("view.mail", json!({}))?;
            }
            set_open(app, true);
        }
        (None, None, _) => {
            let now = app.ai.answers.is_none();
            if now && app.session.view.module != Module::Mail {
                app.execute("view.mail", json!({}))?;
            }
            set_open(app, now);
        }
    }
    Ok(answer(app))
}

/// What the commands answer: where the pane is.
fn answer(app: &MailApp) -> Value {
    let Some(pane) = &app.ai.answers else { return json!({"open": false}) };
    let stage = match pane.stage {
        Stage::Idle => "idle",
        Stage::Looking => "looking",
        Stage::Writing => "writing",
        Stage::Done => "done",
        Stage::Stopped => "stopped",
        Stage::NothingFound => "nothingFound",
        Stage::Trouble(_) => "trouble",
    };
    let sources: Vec<Value> = pane.cited.iter().map(|c| json!({"number": c.number, "id": c.id})).collect();
    json!({"open": true, "stage": stage, "request": pane.request, "question": pane.question, "sources": sources})
}

/// Once a frame: the pane closes with the mailbox it was opened on, and takes in what came.
pub(crate) fn follow(app: &mut MailApp) {
    let Some(pane) = app.ai.answers.as_mut() else { return };
    if pane.openings != app.session.openings() {
        close(app);
        return;
    }
    // The question's vector: what came, or nothing after a moment.
    if let Some((rx, since)) = &pane.embedding {
        let came = match rx.try_recv() {
            Ok(vector) => Some(vector),
            Err(TryRecvError::Disconnected) => Some(None),
            Err(TryRecvError::Empty) if crate::now_ms() - since > EMBED_WAIT_MS => Some(None),
            Err(TryRecvError::Empty) => None,
        };
        match came {
            Some(vector) => {
                pane.embedding = None;
                if let Err(e) = send(app, vector) {
                    app.say_refused("ui.answers", &e);
                }
            }
            None => {
                if let Some(ctx) = &app.ctx {
                    crate::software::repaint_after(ctx, std::time::Duration::from_millis(100));
                }
            }
        }
        return;
    }
    let Some(request) = pane.request.filter(|_| pane.working()) else { return };
    let Some(tracked) = app.session.ai.get(request) else {
        // Let go of by the session: what came stays, and it is said that it broke off.
        pane.stage = Stage::Trouble(Trouble::Other);
        return;
    };
    let (state, came, kind, said) = (tracked.state, tracked.text.clone(), tracked.error_kind, tracked.provider_text.clone());
    pane.text = came;
    match state {
        RequestState::Pending => pane.stage = Stage::Looking,
        RequestState::Streaming => pane.stage = Stage::Writing,
        RequestState::Failed if kind == Some(mailcraft_model::ai::ErrorKind::Malformed) && !pane.text.is_empty() => {
            // The answer came, but which messages it came from couldn't be read: it stays, said so.
            unreadable(app, request);
        }
        RequestState::Failed => {
            // The kind the host named, never the words: what the provider said goes to the details.
            let trouble = Trouble::of_kind(kind);
            pane.stage = Stage::Trouble(trouble);
            pane.details = said;
            pane.focus = Focus::Question;
            let address = Providers::of(app).active().map(|p| p.address()).unwrap_or_default();
            app.status(trouble.words(&address, false).0);
        }
        RequestState::Done | RequestState::Cancelled => finished(app, request, state == RequestState::Done),
    }
}

/// The primary's name as the fallback notice says it: its model's name, else its address.
fn primary_name(p: &super::Provider) -> String {
    let model = p.model_name();
    if model.is_empty() { p.address() } else { model }
}

/// Who answered, as `ai.answerResult` says it: `{"provider", "model"}`, each one line, cut.
fn answered_by(read: &Value) -> Option<(String, String)> {
    let by = read.get("answeredBy")?;
    let part = |key: &str| super::settings_state::clean(by.get(key).and_then(Value::as_str).unwrap_or_default(), 80);
    let (provider, model) = (part("provider"), part("model"));
    (!provider.is_empty() || !model.is_empty()).then_some((provider, model))
}

/// Request `request` failed after its text was shown, because the answer's JSON couldn't be
/// read: the text stays, marked as having no sources MailCraft could read.
fn unreadable(app: &mut MailApp, request: u64) {
    let read = app.execute("ai.answerResult", json!({"requestId": request})).unwrap_or(Value::Null);
    let Some(pane) = app.ai.answers.as_mut() else { return };
    if read.get("sourcesUnreadable").and_then(Value::as_bool) != Some(true) {
        pane.stage = Stage::Trouble(Trouble::Malformed);
        pane.focus = Focus::Question;
        return;
    }
    if let Some(text) = read.get("text").and_then(Value::as_str) {
        pane.text = text.to_string();
    }
    pane.answered_by = answered_by(&read);
    pane.cited.clear();
    pane.sources_unreadable = true;
    pane.stage = Stage::Done;
    pane.focus = Focus::Question;
    app.status(tl!("MailCraft couldn't read which messages this came from"));
}

/// Request `request` is over: its citations read, and said.
fn finished(app: &mut MailApp, request: u64, done: bool) {
    let read = app.execute("ai.answerResult", json!({"requestId": request})).unwrap_or(Value::Null);
    let mut cited = Vec::new();
    for c in read.get("citations").and_then(Value::as_array).into_iter().flatten().take(3) {
        let (Some(number), Some(id)) = (c.get("number").and_then(Value::as_u64), c.get("id").and_then(Value::as_u64)) else { continue };
        // What the chip says comes from the mailbox, never from the model.
        let Some(m) = app.session.mailbox.message(id) else { continue };
        let sender = if m.from.name.trim().is_empty() { m.from.email.clone() } else { m.from.name.clone() };
        let day = mailcraft_time::DateTime::from_timestamp(m.date, app.session.offset_at(m.date)).date;
        cited.push(Cited {
            number: usize::try_from(number).unwrap_or(0),
            id,
            sender: super::cut(sender.trim(), 120),
            subject: super::cut(m.subject.trim(), 200),
            date: crate::dates::medium(day),
        });
    }
    let n = cited.len();
    let Some(pane) = app.ai.answers.as_mut() else { return };
    if let Some(text) = read.get("text").and_then(Value::as_str) {
        pane.text = text.to_string();
    }
    pane.focus = if n > 0 { Focus::FirstSource } else { Focus::Question };
    // A host that doesn't say: an answer with no source shown is taken as naming none.
    pane.uncited = read.get("uncited").and_then(Value::as_bool).unwrap_or(n == 0);
    pane.answered_by = answered_by(&read);
    pane.cited = cited;
    if done {
        pane.stage = Stage::Done;
        app.status(tln!(n, "Answer ready, with {n} source", "Answer ready, with {n} sources"));
    } else {
        pane.stage = Stage::Stopped;
    }
}

/// A line under the answer about its sources: a 16 pt icon in a hue, the words in `text_dim`.
fn sourced(ui: &mut Ui, icon: &str, hue: egui::Color32, words: &str, t: &Tokens) -> egui::Response {
    ui.horizontal_wrapped(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
        crate::icons::paint(ui.painter(), r.shrink(1.0), icon, t.text_dim, hue);
        caption(ui, words, t)
    })
    .inner
}

/// The fallback notice, when the answer came from another provider than the primary asked.
pub(crate) fn fallback_words(pane: &Pane) -> Option<String> {
    let (by, model) = pane.answered_by.as_ref()?;
    let (primary, name) = pane.primary.as_ref()?;
    if by.is_empty() || by == primary {
        return None;
    }
    let model = if model.is_empty() { by.as_str() } else { model.as_str() };
    Some(crate::i18n::fmt(tl!("{provider} didn't answer; this answer comes from {model}"), &[("provider", name), ("model", model)]))
}

/// What the owner chose in the pane this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    Close,
    Stop,
    AskAgain,
    AskNew,
    Show(u64),
    OpenWindow(u64),
    Options,
}

/// A caption in `text_dim`.
fn caption(ui: &mut Ui, words: &str, t: &Tokens) -> egui::Response {
    ui.add(egui::Label::new(RichText::new(words).font(regular(11.5)).color(t.text_dim)).wrap())
}

/// The note under every answer: who wrote it, from how many messages, and that it can be wrong.
fn model_note(providers: &Providers, sent: usize) -> String {
    let Some(p) = providers.active() else {
        return tln!(
            sent,
            "Written by a model, from {n} message in your mailbox. It can be wrong: check the sources.",
            "Written by a model, from {n} messages in your mailbox. It can be wrong: check the sources."
        );
    };
    if p.remote() {
        let said = tln!(
            sent,
            "Written by {model} at {host}, from {n} message in your mailbox. It can be wrong: check the sources.",
            "Written by {model} at {host}, from {n} messages in your mailbox. It can be wrong: check the sources."
        );
        crate::i18n::fmt(&said, &[("model", &p.model_name()), ("host", &p.host())])
    } else {
        let said = tln!(
            sent,
            "Written by {model} on this computer, from {n} message in your mailbox. It can be wrong: check the sources.",
            "Written by {model} on this computer, from {n} messages in your mailbox. It can be wrong: check the sources."
        );
        crate::i18n::fmt(&said, &[("model", &p.model_name())])
    }
}

/// A source: its number, the envelope, the sender and the date, the subject. Enter, Space or a
/// click shows the message; Ctrl+Enter (⌘ on a Mac) or a double click opens it in a window.
fn chip(ui: &mut Ui, c: &Cited, focus: bool, t: &Tokens) -> (egui::Response, Option<Act>) {
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
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
    p.rect(r, 6.0, fill, Stroke::new(1.0, t.input_border), egui::StrokeKind::Inside);
    if resp.has_focus() {
        p.rect_stroke(r.shrink(1.0), 5.0, Stroke::new(2.0, t.focus_ring), egui::StrokeKind::Inside);
    }
    let disc = pos2(r.min.x + 17.0, r.center().y);
    p.circle_filled(disc, 9.0, t.accent_soft);
    p.text(disc, Align2::CENTER_CENTER, c.number.to_string(), semibold(11.0), t.accent_soft_text);
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

/// The pane at the right of Mail, while it is open: drawn before the folder pane, the list and
/// the reading pane take the rest. Not drawn when there isn't room for it beside them.
pub(crate) fn pane(app: &mut MailApp, ui: &mut Ui) {
    if app.ai.answers.is_none() || ui.available_width() < MIN_W + BESIDE {
        return;
    }
    let Some(mut pane) = app.ai.answers.take() else { return };
    let t = Tokens::get(ui.ctx());
    let providers = Providers::of(app);
    let ready = super::model_ready(app);
    let address = providers.active().map(|p| p.address()).unwrap_or_default();
    let max = (ui.available_width() - BESIDE).clamp(MIN_W, MAX_W);
    let focus = std::mem::replace(&mut pane.focus, Focus::None);
    let mut act: Option<Act> = None;
    let edge = if t.pane_rule { t.border } else { egui::Color32::TRANSPARENT };
    let frame = egui::Frame::NONE.fill(t.panel).stroke(Stroke::new(1.0, edge));
    let shown = egui::Panel::right("answers").default_size(DEFAULT_W.min(max)).size_range(MIN_W..=max).resizable(true).frame(frame).show(ui, |ui| {
        // The header: the pane's name and its close button.
        egui::Panel::top("answers_head")
            .exact_size(36.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(12, 0)))
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(RichText::new(tl!("Answer")).font(semibold(13.0)).color(t.text));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let x = crate::widgets::icon_button(ui, "close", tl!("Close Answer"), 14.0, true);
                        x.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tl!("Close Answer")));
                        app.hit.add("closeAnswers", 0, x.rect);
                        if x.clicked() {
                            act = Some(Act::Close);
                        }
                    });
                });
            });
        // The foot: what can be done, and the question field.
        egui::Panel::bottom("answers_foot")
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(12, 10)))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
                if pane.working() {
                    let halt = crate::widgets::plain_button(ui, tl!("Stop"));
                    app.hit.add("stopAnswer", 0, halt.rect);
                    if focus == Focus::Stop {
                        halt.request_focus();
                    }
                    if halt.clicked() {
                        act = Some(Act::Stop);
                    }
                } else {
                    ui.horizontal_wrapped(|ui| {
                        if !pane.question.is_empty() && ready && crate::widgets::plain_button(ui, tl!("Ask Again")).clicked() {
                            act = Some(Act::AskAgain);
                        }
                        let needs_options = !ready || matches!(pane.stage, Stage::Trouble(trouble) if trouble.options_help());
                        if needs_options && crate::widgets::plain_button(ui, tl!("Open Options")).clicked() {
                            act = Some(Act::Options);
                        }
                    });
                    if ready {
                        let field = egui::TextEdit::singleline(&mut pane.field)
                            .id(egui::Id::unique("answers_question"))
                            .hint_text(tl!("Ask about your mail"))
                            .char_limit(MAX_QUESTION)
                            .desired_width(f32::INFINITY);
                        let resp = crate::text_menu::field(ui.add(field));
                        app.hit.add("answersQuestion", 0, resp.rect);
                        if focus == Focus::Question {
                            resp.request_focus();
                        }
                        if resp.lost_focus() {
                            if crate::keys::escape(ui.ctx(), ui.layer_id()) {
                                act = Some(Act::Close);
                            } else if crate::keys::enter(ui.ctx(), ui.layer_id()) && !pane.field.trim().is_empty() {
                                act = Some(Act::AskNew);
                            }
                        }
                    }
                }
            });
        // The question and the answer, on the reading pane's sheet.
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.surface).inner_margin(egui::Margin::symmetric(14, 12))).show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(pane.working()).show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
                if !ready && pane.stage == Stage::Idle {
                    let (lead, rest) = Trouble::NoModel.words(&address, false);
                    super::narrow::notice(ui, Trouble::NoModel, &lead, &rest, &t);
                    return;
                }
                if pane.question.is_empty() {
                    caption(ui, tl!("Ask a question about your mail in the field below. The answer names the messages it comes from."), &t);
                    return;
                }
                caption(ui, tl!("You asked"), &t);
                ui.add(egui::Label::new(RichText::new(super::cut(&pane.question, SHOWN_QUESTION)).font(regular(12.5)).color(t.text)).wrap())
                    .on_hover_text(pane.question.as_str());
                ui.add_space(6.0);
                match pane.stage {
                    Stage::NothingFound => {
                        super::narrow::notice(
                            ui,
                            Trouble::NoModel,
                            tl!("Nothing in your mail seems to answer this."),
                            tl!("Nothing was sent to the model. Try other words."),
                            &t,
                        );
                    }
                    Stage::Trouble(trouble) => {
                        let (lead, rest) = trouble.words(&address, false);
                        super::narrow::notice(ui, trouble, &lead, &rest, &t);
                        super::errors::details(ui, "answers", &pane.details, &t);
                        ui.add_space(4.0);
                    }
                    _ => {}
                }
                let numbers: Vec<usize> = pane.cited.iter().map(|c| c.number).collect();
                let has_text = !pane.text.is_empty();
                let drawn = ui.scope(|ui| super::answer_text::show(ui, pane.blocks(), &numbers, &t));
                // Where the answer is, for `ui.inspect`: its rect only, never its words.
                if has_text {
                    app.hit.add("answerText", 0, drawn.response.rect);
                }
                if let Some(n) = drawn.inner
                    && let Some(c) = pane.cited.iter().find(|c| c.number == n)
                {
                    act = Some(Act::Show(c.id));
                }
                match pane.stage {
                    Stage::Looking => {
                        ui.horizontal(|ui| {
                            crate::software::spinner(ui, 14.0, Some(t.text_dim));
                            caption(ui, tl!("Looking through your mail…"), &t);
                        });
                    }
                    Stage::Writing => {
                        ui.horizontal(|ui| {
                            crate::software::spinner(ui, 14.0, Some(t.text_dim));
                            caption(ui, tl!("Writing…"), &t);
                        });
                    }
                    Stage::Stopped => {
                        caption(ui, tl!("Stopped. The answer above is incomplete."), &t);
                    }
                    Stage::Trouble(trouble) if trouble.broke_off() && !pane.text.is_empty() => {
                        caption(ui, tl!("The answer above is incomplete."), &t);
                    }
                    _ => {}
                }
                if !matches!(pane.stage, Stage::Done | Stage::Stopped) {
                    return;
                }
                ui.add_space(8.0);
                // Another model answered than the one asked first: said every time.
                let fallback = fallback_words(&pane);
                if let Some(words) = &fallback {
                    let said = sourced(ui, "info", t.blue, words, &t);
                    app.hit.add("answerFallback", 0, said.rect);
                }
                if pane.sources_unreadable {
                    let said = sourced(ui, "warning", t.orange, tl!("MailCraft couldn't read which messages this came from"), &t);
                    app.hit.add("answerSourcesUnread", 0, said.rect);
                } else if pane.cited.is_empty() && pane.uncited {
                    let said = sourced(ui, "info", t.blue, tl!("No message was named as the source"), &t);
                    app.hit.add("answerUncited", 0, said.rect);
                } else if !pane.cited.is_empty() {
                    ui.label(RichText::new(tl!("Sources")).font(semibold(11.5)).color(t.text_dim));
                    let mut chips: Vec<egui::Response> = Vec::new();
                    for (i, c) in pane.cited.iter().enumerate() {
                        let (resp, chosen) = chip(ui, c, i == 0 && focus == Focus::FirstSource, &t);
                        app.hit.add("answerSource", u64::try_from(c.number).unwrap_or(0), resp.rect);
                        if chosen.is_some() {
                            act = chosen;
                        }
                        chips.push(resp);
                    }
                    // Up and Down move between the sources.
                    if let Some(at) = chips.iter().position(egui::Response::has_focus) {
                        let (up, down) = ui.input(|i| (i.key_pressed(egui::Key::ArrowUp), i.key_pressed(egui::Key::ArrowDown)));
                        let to = if up {
                            at.checked_sub(1)
                        } else if down {
                            Some(at.saturating_add(1))
                        } else {
                            None
                        };
                        if let Some(next) = to.and_then(|i| chips.get(i)) {
                            next.request_focus();
                        }
                    }
                }
                ui.add_space(6.0);
                if let Some((_, model)) = pane.answered_by.as_ref().filter(|(_, model)| !model.is_empty()) {
                    let by = caption(ui, &crate::i18n::fmt(tl!("Answered by {model}"), &[("model", model)]), &t);
                    app.hit.add("answerBy", 0, by.rect);
                }
                // After a fallback the model in use didn't write it: the note names none.
                let noted = if fallback.is_some() { Providers::default() } else { providers.clone() };
                caption(ui, &model_note(&noted, pane.sent), &t);
            });
        });
    });
    app.hit.add("answers", 0, shown.response.rect);
    // Escape while the answer is being written stops it (the field's own Escape closes the pane).
    if pane.working() && shown.response.contains_pointer() && crate::keys::take_escape(ui.ctx(), ui.layer_id()) {
        act = Some(Act::Stop);
    }
    app.ai.answers = Some(pane);
    let Some(act) = act else { return };
    let done = match act {
        Act::Close => {
            close(app);
            app.search_focus = true;
            Ok(())
        }
        Act::Stop => {
            stop(app);
            Ok(())
        }
        Act::AskAgain => {
            let question = app.ai.answers.as_ref().map(|p| p.question.clone()).unwrap_or_default();
            ask(app, &question)
        }
        Act::AskNew => {
            let question = app.ai.answers.as_ref().map(|p| p.field.trim().to_string()).unwrap_or_default();
            ask(app, &question)
        }
        Act::Show(id) => {
            app.open_in_mail(id);
            Ok(())
        }
        Act::OpenWindow(id) => app.run("ui.openWindow", json!({"message": id})).map(|_| ()),
        Act::Options => app.run("ui.aiSettings", json!({})).map(|_| ()),
    };
    if let Err(e) = done {
        app.say_refused("ui.answers", &e);
    }
}
