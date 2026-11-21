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

use egui::{Align, Layout, RichText, Sense, Stroke, Ui, vec2};
use mailcraft_engine::Module;
use mailcraft_html::Block;
use mailcraft_model::ai::RequestState;
use serde_json::{Value, json};

use super::errors::Trouble;
use super::{Providers, flag, text};
use crate::MailApp;
use crate::theme::{Tokens, regular, semibold};

/// The runtimes passed over before the answer, and the fallback question.
#[path = "answers_chain.rs"]
pub(crate) mod chain;
#[cfg(test)]
#[path = "answers_chain_tests.rs"]
mod chain_tests;
#[cfg(test)]
#[path = "answers_layout_tests.rs"]
mod layout_tests;
/// The Sources list and the provenance line under an answer.
#[path = "answers_sources.rs"]
pub(crate) mod sources;
#[cfg(test)]
#[path = "answers_sources_tests.rs"]
mod sources_tests;

/// The pane's width as it opens, the least and the most it may be given.
const DEFAULT_W: f32 = 360.0;
const MIN_W: f32 = 280.0;
const MAX_W: f32 = 560.0;
/// What the message list and the reading pane beside it keep at the least, and the list alone
/// when the reading pane is under it or off.
const BESIDE: f32 = 240.0 + 300.0;
const LIST_ALONE: f32 = 300.0;
/// The folder pane's strip while the pane stands aside (`nav::STRIP_W`).
const STRIP: f32 = 30.0;
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
    /// How many passages of them the model was given to read.
    pub(crate) passages: usize,
    /// The answer as far as it came, and its blocks.
    pub(crate) text: String,
    blocks: Vec<Block>,
    blocks_of: String,
    pub(crate) cited: Vec<Cited>,
    /// What the provider itself said of a failure, for the notice's details: data, never read.
    pub(crate) details: String,
    /// Who answered, as the host said (provider, model): data, cut and stripped.
    pub(crate) answered_by: Option<(String, String)>,
    /// The runtimes passed over or failed before the answer, each with its reason, as said.
    pub(crate) passed_over: Vec<String>,
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
            passages: 0,
            text: String::new(),
            blocks: Vec::new(),
            blocks_of: String::new(),
            cited: Vec::new(),
            details: String::new(),
            answered_by: None,
            passed_over: Vec::new(),
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
    // The passages the request carries for the model; a host that doesn't say: one a message.
    pane.passages = asked.pointer("/request/answer/passages").and_then(Value::as_array).map_or(pane.sent, Vec::len);
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
    let Some(request) = pane.request else { return };
    super::cancel(app, request);
    // What came stays, with the sources it named so far; the keyboard stays at the question.
    finished(app, request, false);
    if let Some(pane) = app.ai.answers.as_mut() {
        pane.focus = Focus::Question;
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

/// Whether the pane is asking the owner the fallback question (the `ask` policy) for the answer
/// it is waiting on: only the owner's own click answers it, never an agent's input.
pub(crate) fn fallback_asked(app: &MailApp) -> bool {
    let Some(request) = app.ai.answers.as_ref().filter(|p| p.working()).and_then(|p| p.request) else { return false };
    let Some(question) = app.services.ai_providers.as_deref().and_then(|h| h.fallback_question()) else { return false };
    chain::question_words(&question, request, &Providers::of(app)).is_some()
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

/// What the host says of the chain's walk for `request`: the runtimes passed over or failed
/// before the answer, and those stopped for trying a tool (nothing from a host without a chain).
fn chain_said(app: &MailApp, request: u64) -> Vec<String> {
    let Some(host) = app.services.ai_providers.as_deref() else { return Vec::new() };
    let providers = Providers::of(app);
    chain::passed_over(host.provenance(request).as_ref(), &host.tool_use_notices(request), &providers)
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
        let number = usize::try_from(number).unwrap_or(0);
        // What the row says comes from the mailbox, never from the model. A message gone since
        // keeps its row, which says so and opens nothing.
        let Some(m) = app.session.mailbox.message(id) else {
            cited.push(Cited { number, id, sender: String::new(), subject: String::new(), date: String::new() });
            continue;
        };
        let sender = if m.from.name.trim().is_empty() { m.from.email.clone() } else { m.from.name.clone() };
        let day = mailcraft_time::DateTime::from_timestamp(m.date, app.session.offset_at(m.date)).date;
        cited.push(Cited {
            number,
            id,
            sender: super::cut(sender.trim(), 120),
            subject: super::cut(m.subject.trim(), 200),
            date: crate::dates::medium(day),
        });
    }
    let said = chain_said(app, request);
    let Some(pane) = app.ai.answers.as_mut() else { return };
    if let Some(text) = read.get("text").and_then(Value::as_str) {
        pane.text = text.to_string();
    }
    // Stopped: only the sources the text named before it stopped.
    let cited = if done { cited } else { sources::cited_so_far(cited, &pane.text) };
    let n = cited.len();
    pane.focus = if n > 0 { Focus::FirstSource } else { Focus::Question };
    // A host that doesn't say: an answer with no source shown is taken as naming none.
    pane.uncited = read.get("uncited").and_then(Value::as_bool).unwrap_or(n == 0);
    pane.answered_by = answered_by(&read);
    pane.passed_over = said;
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
    /// The owner's answer to the fallback question of a request: try the next runtime or not.
    Fallback(u64, bool),
}

/// A caption in `text_dim`.
fn caption(ui: &mut Ui, words: &str, t: &Tokens) -> egui::Response {
    ui.add(egui::Label::new(RichText::new(words).font(regular(11.5)).color(t.text_dim)).wrap())
}

/// What Mail keeps beside the pane: the folder pane (its strip when `yield_folders`), then the
/// list and a reading pane at its right, or the list alone.
fn beside(app: &MailApp, yield_folders: bool) -> f32 {
    let folders = if app.session.view.folder_pane && !yield_folders {
        if app.ui.folder_pane_width.is_finite() { app.ui.folder_pane_width.clamp(160.0, 420.0) } else { 240.0 }
    } else {
        STRIP
    };
    let rest = if app.session.view.reading_pane == mailcraft_engine::Pane::Right { BESIDE } else { LIST_ALONE };
    folders + rest
}

/// How the pane fits beside Mail in `room` points: `Some(false)` with the folder pane as it is,
/// `Some(true)` with the folder pane standing aside as its strip, `None` when it doesn't fit.
pub(crate) fn fits(app: &MailApp, room: f32) -> Option<bool> {
    [false, true].into_iter().find(|&yield_folders| room >= MIN_W + beside(app, yield_folders))
}

/// The pane at the right of Mail, while it is open: drawn before the folder pane, the list and
/// the reading pane take the rest. When the folder pane would leave the reading pane too
/// narrow, it stands aside as its strip while the pane is open; not drawn when even that leaves
/// no room.
pub(crate) fn pane(app: &mut MailApp, ui: &mut Ui) {
    app.ai.folders_yield = false;
    if app.ai.answers.is_none() {
        return;
    }
    let Some(yield_folders) = fits(app, ui.available_width()) else { return };
    app.ai.folders_yield = yield_folders;
    let keep = beside(app, yield_folders);
    let Some(mut pane) = app.ai.answers.take() else { return };
    let t = Tokens::get(ui.ctx());
    let providers = Providers::of(app);
    let ready = super::model_ready(app);
    let address = providers.active().map(|p| p.address()).unwrap_or_default();
    let max = (ui.available_width() - keep).clamp(MIN_W, MAX_W);
    let focus = std::mem::replace(&mut pane.focus, Focus::None);
    let mut act: Option<Act> = None;
    // The `ask` policy's question, while this answer waits on it (only the window answers it).
    let asked = pane.request.filter(|_| pane.working()).and_then(|request| {
        let question = app.services.ai_providers.as_deref()?.fallback_question()?;
        chain::question_words(&question, request, &providers).map(|words| (request, words))
    });
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
                    && app.session.mailbox.message(c.id).is_some()
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
                    _ => {}
                }
                if let Some((request, words)) = &asked {
                    let said = sourced(ui, "info", t.blue, words, &t);
                    app.hit.add("answerFallbackQuestion", 0, said.rect);
                    ui.horizontal_wrapped(|ui| {
                        let yes = crate::widgets::primary_button(ui, tl!("Try It"), true);
                        app.hit.add("answerFallbackYes", 0, yes.rect);
                        if yes.clicked() {
                            act = Some(Act::Fallback(*request, true));
                        }
                        let no = crate::widgets::plain_button(ui, tl!("Don't"));
                        app.hit.add("answerFallbackNo", 0, no.rect);
                        if no.clicked() {
                            act = Some(Act::Fallback(*request, false));
                        }
                    });
                }
                match pane.stage {
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
                } else if !pane.cited.is_empty()
                    && let Some(chosen) = sources::list(app, ui, &pane.cited, focus == Focus::FirstSource, &t)
                {
                    act = Some(chosen);
                }
                ui.add_space(6.0);
                // Which model, where it ran, what it read: after a fallback, the one that answered.
                let line = sources::provenance(pane.answered_by.as_ref(), &providers, pane.passages);
                let by = caption(ui, &line, &t);
                app.hit.add("answerBy", 0, by.rect);
                // The runtimes passed over or failed before this answer, each with its reason.
                for (i, words) in pane.passed_over.iter().enumerate() {
                    let said = caption(ui, words, &t);
                    app.hit.add("answerPassedOver", u64::try_from(i).unwrap_or(u64::MAX), said.rect);
                }
                caption(ui, tl!("A model wrote this answer, and it can be wrong: check the sources."), &t);
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
        Act::Fallback(request, yes) => match app.services.ai_providers.as_deref() {
            Some(host) => host.answer_fallback(request, yes),
            None => Ok(()),
        },
    };
    if let Err(e) = done {
        app.say_refused("ui.answers", &e);
    }
}
