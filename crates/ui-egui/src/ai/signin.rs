// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The runtime chain page's dialogs (B11, 21.3): Sign In with ChatGPT; Claude Code's sign-in in
//! the owner's own terminal, watched for up to ten minutes; the question before using a Universal
//! Agent Runtime MailCraft didn't start; the question before signing out of ChatGPT. The consent
//! dialog is [`super::runtime_consent`]'s.
//!
//! A dialog only collects the owner's intent and then runs one of the runtimes' methods
//! ([`super::settings_runtimes::run`]); a method that comes from the control channel or MCP never
//! opens one. MailCraft never signs in to a CLI for the owner: Claude Code's sign-in is the fixed
//! command `claude auth login` in a terminal the host opens (or the owner opens, when it can't),
//! and Codex's `codex login` is only ever shown ([`super::runtime_rows::CODEX_LOGIN`]). Escape
//! always cancels; a question whose go-ahead sends something has the "no" as its default, the one
//! Enter reaches.

use egui::{Context, Ui, vec2};
use mailcraft_model::ai::RuntimeId;
use serde_json::json;

use super::runtime_rows::{CLAUDE_LOGIN, Facts, Status};
use super::runtime_widgets as w;
use super::settings_runtimes::run;
use crate::MailApp;
use crate::theme::{Tokens, semibold};

/// How long a sign-in is waited for, and how often the host is asked meanwhile.
pub(crate) const WAIT_MS: f64 = 10.0 * 60.0 * 1000.0;
pub(crate) const POLL: std::time::Duration = std::time::Duration::from_secs(2);
/// A dialog's width.
const WIDTH: f32 = 440.0;

/// Where Sign In with ChatGPT is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum GptStep {
    Start,
    Waiting { until_ms: f64 },
    Done,
    TimedOut,
    Failed,
}

/// Where Claude Code's sign-in is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ClaudeStep {
    /// The host opened a terminal with `claude auth login`.
    Terminal {
        until_ms: f64,
    },
    /// It couldn't: the owner opens one and runs the command.
    NoTerminal {
        until_ms: f64,
    },
    Done,
    TimedOut,
}

/// A dialog of the runtime chain page.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum RtDialog {
    Consent { runtime: RuntimeId, account: String },
    ChatGpt(GptStep),
    Claude(ClaudeStep),
    UarOptIn,
    ChatGptSignOut,
}

impl RtDialog {
    /// The runtime a sign-in of this dialog is waiting for.
    pub(crate) fn waiting_for(&self) -> Option<RuntimeId> {
        match self {
            RtDialog::ChatGpt(GptStep::Waiting { .. }) => Some(RuntimeId::ChatGpt),
            RtDialog::Claude(ClaudeStep::Terminal { .. } | ClaudeStep::NoTerminal { .. }) => Some(RuntimeId::ClaudeCode),
            _ => None,
        }
    }

    /// Whether it asks the owner before something is sent or changed: no input from the control
    /// channel reaches the window while it is open ([`super::follow`]).
    pub(crate) fn asks(&self) -> bool {
        matches!(self, RtDialog::Consent { .. } | RtDialog::UarOptIn | RtDialog::ChatGptSignOut | RtDialog::ChatGpt(GptStep::Start))
    }

    /// Its name, for the tests (and `ui.inspect`, once the integrator lists it there).
    #[cfg(test)]
    pub(crate) fn name(&self) -> &'static str {
        match self {
            RtDialog::Consent { .. } => "aiRuntimeConsent",
            RtDialog::ChatGpt(_) => "aiSignInChatGpt",
            RtDialog::Claude(_) => "aiSignInClaudeCode",
            RtDialog::UarOptIn => "aiUseUar",
            RtDialog::ChatGptSignOut => "aiSignOutChatGpt",
        }
    }
}

/// An open dialog: which, whether its default focus was given, and the control that opened it
/// (the keyboard goes back there when it closes).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Open {
    pub(crate) dialog: RtDialog,
    pub(crate) focused: bool,
    pub(crate) opener: Option<egui::Id>,
    /// Whether the owner ticked that they understand what an external Universal Agent Runtime
    /// may do ([`super::runtime_consent::uar_understood`]): unticked whenever a dialog opens.
    pub(crate) understood: bool,
}

impl Open {
    pub(crate) fn new(dialog: RtDialog, opener: Option<egui::Id>) -> Open {
        Open { dialog, focused: false, opener, understood: false }
    }
}

/// What the owner did in a dialog this frame.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Said {
    /// Keep it open as it is.
    Stay,
    /// Go on to another dialog (or step), with its default focus given again.
    To(RtDialog),
    /// Close it.
    Close,
}

/// The frame of every dialog of the page: a modal window over everything, 440 pt wide with the
/// `border_strong` edge, a title, and a body that scrolls inside a short window. `focus_first`
/// is false once the default focus was given. Escape is `escape`.
pub(crate) fn modal<R>(ctx: &Context, salt: &str, title: &str, body: impl FnOnce(&mut Ui) -> R) -> (R, bool) {
    let t = Tokens::get(ctx);
    let layer = crate::keys::modal_layer(egui::Id::unique(("ai-runtime-dialog", salt)));
    let escape = crate::keys::take_escape_any(ctx, layer);
    let frame =
        egui::Frame::window(&ctx.global_style()).fill(t.surface).stroke(egui::Stroke::new(1.0, t.border_strong)).inner_margin(egui::Margin::same(16));
    let width = (ctx.content_rect().width() - 32.0).clamp(160.0, WIDTH);
    let most_h = (ctx.content_rect().height() - 64.0).max(120.0);
    let shown = egui::Modal::new(layer.id).frame(frame).backdrop_color(t.scrim).show(ctx, |ui| {
        ui.set_width(width);
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        ui.add(egui::Label::new(egui::RichText::new(title).font(semibold(14.0)).color(t.text)).wrap());
        ui.data_mut(|d| d.insert_temp(egui::Id::unique("ai-runtime-dialog-most-h"), most_h));
        body(ui)
    });
    (shown.inner, escape)
}

/// A dialog's words, which scroll inside a short window; its buttons stay under them, outside.
pub(crate) fn scrolled<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let most_h = ui.data(|d| d.get_temp::<f32>(egui::Id::unique("ai-runtime-dialog-most-h"))).unwrap_or(400.0);
    egui::ScrollArea::vertical().max_height((most_h - 96.0).max(80.0)).auto_shrink([false, true]).show(ui, add).inner
}

/// The modal layer of the dialog with `salt`, for the keys that are asked before it is drawn.
pub(crate) fn layer(salt: &str) -> egui::LayerId {
    crate::keys::modal_layer(egui::Id::unique(("ai-runtime-dialog", salt)))
}

/// The minutes left of a wait that ends at `until_ms`, said as a whole number (at least one).
pub(crate) fn minutes_left(until_ms: f64, now_ms: f64) -> u64 {
    let left = ((until_ms - now_ms) / 60_000.0).floor();
    if left.is_finite() && left >= 1.0 { left.min(10.0) as u64 } else { 1 }
}

/// Starts Claude Code's sign-in: the host opens the owner's terminal with `claude auth login`
/// and watches `claude auth status`. When it can't, the command is shown to run by hand.
pub(crate) fn start_claude(app: &mut MailApp) -> RtDialog {
    let until_ms = crate::now_ms() + WAIT_MS;
    match run(app, "ai.signIn", json!({"runtime": "claude_code"})) {
        Ok(_) => RtDialog::Claude(ClaudeStep::Terminal { until_ms }),
        Err(e) => {
            log::debug!("a terminal couldn't be opened for Claude Code's sign-in: {e}");
            RtDialog::Claude(ClaudeStep::NoTerminal { until_ms })
        }
    }
}

/// A waiting step moves on by itself: to Done when the runtime is Ready, to Timed out at its end.
pub(crate) fn follow(dialog: &RtDialog, facts: &Facts, now_ms: f64) -> Option<RtDialog> {
    let ready = |id: &RuntimeId| facts.rows.iter().find(|f| &f.id == id).is_some_and(|f| matches!(Facts::found(f), Status::Ready));
    match dialog {
        RtDialog::ChatGpt(GptStep::Waiting { .. }) if ready(&RuntimeId::ChatGpt) => Some(RtDialog::ChatGpt(GptStep::Done)),
        RtDialog::ChatGpt(GptStep::Waiting { until_ms }) if now_ms >= *until_ms => Some(RtDialog::ChatGpt(GptStep::TimedOut)),
        RtDialog::Claude(ClaudeStep::Terminal { .. } | ClaudeStep::NoTerminal { .. }) if ready(&RuntimeId::ClaudeCode) => {
            Some(RtDialog::Claude(ClaudeStep::Done))
        }
        RtDialog::Claude(ClaudeStep::Terminal { until_ms } | ClaudeStep::NoTerminal { until_ms }) if now_ms >= *until_ms => {
            Some(RtDialog::Claude(ClaudeStep::TimedOut))
        }
        _ => None,
    }
}

/// Draws a sign-in dialog (or the UAR or sign-out question): what the owner said.
pub(crate) fn show(app: &mut MailApp, ctx: &Context, open: &Open, facts: &Facts, understood: &mut bool) -> Said {
    let first = !open.focused;
    match &open.dialog {
        RtDialog::ChatGpt(step) => chatgpt(app, ctx, *step, facts, first),
        RtDialog::Claude(step) => claude(app, ctx, *step, facts, first),
        RtDialog::UarOptIn => uar(app, ctx, first, understood),
        RtDialog::ChatGptSignOut => sign_out(app, ctx, first),
        RtDialog::Consent { .. } => Said::Stay,
    }
}

/// A row of buttons: the go-ahead rightmost. `default` (an index into `labels`) gets the focus on
/// the first frame. The index of the one pressed.
fn buttons(app: &mut MailApp, ui: &mut Ui, labels: &[&str], default: usize, first: bool) -> Option<usize> {
    buttons_go(app, ui, labels, default, first, true)
}

/// [`buttons`], with the go-ahead (the last) greyed out and never pressed unless `go`.
fn buttons_go(app: &mut MailApp, ui: &mut Ui, labels: &[&str], default: usize, first: bool, go: bool) -> Option<usize> {
    let mut pressed = None;
    crate::dialogs::button_row(ui, |ui| {
        for (i, label) in labels.iter().enumerate().rev() {
            let enabled = go || i + 1 != labels.len() || labels.len() == 1;
            let b = w::button(ui, label, label, enabled);
            let kind = match i {
                0 if labels.len() > 1 => "aiSignInCancel",
                _ if i + 1 == labels.len() => "aiSignInGo",
                _ => "aiSignInOther",
            };
            app.hit.add(kind, 0, b.rect);
            if first && i == default {
                b.request_focus();
            }
            if b.clicked() && enabled {
                pressed = Some(i);
            }
        }
    });
    pressed
}

fn lock_note(ui: &mut Ui, words: &str, t: &Tokens) {
    w::notice(ui, t.blue, "lock", None, words, t);
}

fn chatgpt(app: &mut MailApp, ctx: &Context, step: GptStep, facts: &Facts, first: bool) -> Said {
    let t = Tokens::get(ctx);
    let title = match step {
        GptStep::Start => tl!("Sign In with ChatGPT"),
        GptStep::Waiting { .. } => tl!("Finish Signing In in Your Browser"),
        GptStep::Done => tl!("Signed In to ChatGPT"),
        GptStep::TimedOut => tl!("The Sign-in Took Too Long"),
        GptStep::Failed => tl!("The Sign-in Didn't Finish"),
    };
    let still = ctx.global_style().animation_time <= 0.0;
    let row = facts.rows.iter().find(|f| f.id == RuntimeId::ChatGpt);
    let (said, escape) = modal(ctx, "chatgpt", title, |ui| match step {
        GptStep::Start => {
            scrolled(ui, |ui| {
                w::body(
                    ui,
                    tl!(
                        "OpenAI's sign-in page opens in your web browser. Sign in with your ChatGPT account, then choose Allow. Answers then use your ChatGPT plan and its usage limits."
                    ),
                    t.text,
                );
                lock_note(
                    ui,
                    tl!(
                        "MailCraft keeps this sign-in in the system's credential store, never in its files, and uses it only for your own questions."
                    ),
                    &t,
                );
            });
            match buttons(app, ui, &[tl!("Cancel"), tl!("Open Sign-in Page")], 1, first) {
                Some(1) => match run(app, "ai.signIn", json!({"runtime": "chatgpt"})) {
                    Ok(_) => Said::To(RtDialog::ChatGpt(GptStep::Waiting { until_ms: crate::now_ms() + WAIT_MS })),
                    Err(e) => {
                        log::debug!("the ChatGPT sign-in couldn't be started: {e}");
                        Said::To(RtDialog::ChatGpt(GptStep::Failed))
                    }
                },
                Some(_) => Said::Close,
                None => Said::Stay,
            }
        }
        GptStep::Waiting { until_ms } => {
            ui.ctx().request_repaint_after(POLL);
            w::waiting_bar(ui, &crate::i18n::fmt(tl!("Waiting for {vendor}"), &[("vendor", tl!("OpenAI"))]), still);
            w::body(ui, tl!("Waiting for OpenAI. MailCraft waits up to 10 minutes."), t.text);
            let m = minutes_left(until_ms, crate::now_ms());
            w::caption(ui, &crate::i18n::fmt(&tln!(m, "About {m} minute left.", "About {m} minutes left."), &[("m", &m.to_string())]), &t);
            // Cancel calls the host's sign-in off (its wait for the browser ends; nothing is kept).
            match buttons(app, ui, &[tl!("Cancel")], 0, first) {
                Some(_) => {
                    cancel_chatgpt_sign_in(app);
                    app.status(tl!("Sign-in cancelled. Nothing was changed."));
                    Said::Close
                }
                None => Said::Stay,
            }
        }
        GptStep::Done => {
            let who = match row.map(|f| (&f.extra.email, &f.extra.plan)) {
                Some((Some(email), Some(plan))) => crate::i18n::fmt(tl!("As {email} ({plan})."), &[("email", email), ("plan", plan)]),
                Some((Some(email), None)) => crate::i18n::fmt(tl!("As {email}."), &[("email", email)]),
                _ => tl!("You're signed in.").to_string(),
            };
            scrolled(ui, |ui| {
                w::body(ui, &who, t.text);
                w::body(ui, tl!("Answers use your ChatGPT plan and its usage limits."), t.text);
                if row.is_some_and(|f| !f.enabled) {
                    w::body(ui, tl!("Turn ChatGPT plan on to use it."), t.text);
                }
                w::body(ui, tl!("Next, choose which accounts may use it, under Accounts."), t.text_dim);
            });
            if buttons(app, ui, &[tl!("Done")], 0, first).is_some() { Said::Close } else { Said::Stay }
        }
        GptStep::TimedOut | GptStep::Failed => {
            let words = if step == GptStep::TimedOut {
                tl!("MailCraft stopped waiting after 10 minutes. Nothing was changed.")
            } else {
                tl!("The sign-in couldn't be started. Nothing was changed.")
            };
            w::body(ui, words, t.text);
            match buttons(app, ui, &[tl!("Close"), tl!("Try Again")], 1, first) {
                Some(1) => Said::To(RtDialog::ChatGpt(GptStep::Start)),
                Some(_) => Said::Close,
                None => Said::Stay,
            }
        }
    });
    if escape {
        if matches!(step, GptStep::Waiting { .. }) {
            cancel_chatgpt_sign_in(app);
            app.status(tl!("Sign-in cancelled. Nothing was changed."));
        }
        return Said::Close;
    }
    said
}

fn claude(app: &mut MailApp, ctx: &Context, step: ClaudeStep, facts: &Facts, first: bool) -> Said {
    let t = Tokens::get(ctx);
    let title = match step {
        ClaudeStep::Terminal { .. } | ClaudeStep::NoTerminal { .. } => tl!("Sign In to Claude Code"),
        ClaudeStep::Done => tl!("Signed In to Claude Code"),
        ClaudeStep::TimedOut => tl!("MailCraft Stopped Waiting"),
    };
    let still = ctx.global_style().animation_time <= 0.0;
    let row = facts.rows.iter().find(|f| f.id == RuntimeId::ClaudeCode);
    let stop_waiting = |app: &mut MailApp| app.status(tl!("MailCraft stopped waiting. If you sign in later, choose Look Again."));
    let (said, escape) = modal(ctx, "claude", title, |ui| match step {
        ClaudeStep::Terminal { .. } | ClaudeStep::NoTerminal { .. } => {
            ui.ctx().request_repaint_after(POLL);
            let terminal = matches!(step, ClaudeStep::Terminal { .. });
            scrolled(ui, |ui| {
                if terminal {
                    w::body(ui, tl!("A Terminal window opened with the command below. Sign in there with your Claude account."), t.text);
                } else {
                    w::body(ui, tl!("MailCraft couldn't open a terminal. Open one yourself and run:"), t.text);
                }
                let (_, copy) = w::command_box(ui, CLAUDE_LOGIN, first && !terminal);
                app.hit.add("aiSignInCopy", 0, copy.rect);
                w::body(
                    ui,
                    tl!("MailCraft never sees your sign-in. It asks Claude Code every few seconds whether you're signed in, for up to 10 minutes."),
                    t.text,
                );
                w::waiting_bar(ui, &crate::i18n::fmt(tl!("Waiting for {vendor}"), &[("vendor", tl!("Claude Code"))]), still);
                w::caption(ui, tl!("Waiting for Claude Code…"), &t);
            });
            match buttons(app, ui, &[tl!("Cancel")], 0, first && terminal) {
                Some(_) => {
                    stop_waiting(app);
                    Said::Close
                }
                None => Said::Stay,
            }
        }
        ClaudeStep::Done => {
            scrolled(ui, |ui| {
                if row.and_then(|f| f.extra.uses_plan) == Some(false) {
                    w::body(ui, tl!("Claude Code says you're signed in."), t.text);
                    w::notice(
                        ui,
                        t.orange,
                        "warning",
                        None,
                        tl!("Claude Code is signed in without a Claude plan. Anthropic may charge that sign-in for each answer."),
                        &t,
                    );
                } else {
                    w::body(
                        ui,
                        tl!(
                            "Claude Code says you're signed in. Answers use your Claude plan and its usage limits, as if you used Claude Code yourself."
                        ),
                        t.text,
                    );
                }
                w::body(ui, tl!("Next, choose which accounts may use it, under Accounts."), t.text_dim);
            });
            if buttons(app, ui, &[tl!("Done")], 0, first).is_some() { Said::Close } else { Said::Stay }
        }
        ClaudeStep::TimedOut => {
            w::body(ui, tl!("Claude Code didn't say you're signed in within 10 minutes. If you finished signing in, choose Look Again."), t.text);
            match buttons(app, ui, &[tl!("Close"), tl!("Look Again")], 1, first) {
                Some(1) => {
                    if let Err(e) = run(app, "ai.rescan", json!({})) {
                        app.status(e);
                    }
                    Said::Close
                }
                Some(_) => Said::Close,
                None => Said::Stay,
            }
        }
    });
    if escape {
        if matches!(step, ClaudeStep::Terminal { .. } | ClaudeStep::NoTerminal { .. }) {
            stop_waiting(app);
        }
        return Said::Close;
    }
    said
}

/// Using a Universal Agent Runtime MailCraft didn't start: Don't Use It is what Enter and Escape
/// reach, and Use It stays greyed out until the owner ticks that they understand what its tools
/// may do.
fn uar(app: &mut MailApp, ctx: &Context, first: bool, understood: &mut bool) -> Said {
    let t = Tokens::get(ctx);
    let no = crate::keys::take_enter_out(ctx, layer("uar"));
    let (said, escape) = modal(ctx, "uar", tl!("Use the Universal Agent Runtime That's Already Running?"), |ui| {
        scrolled(ui, |ui| {
            w::body(
                ui,
                tl!(
                    "MailCraft didn't start this Universal Agent Runtime and can't turn off its tools. Whatever it is sent leaves this computer, as far as MailCraft can tell, and it is used only for the accounts you allow."
                ),
                t.text,
            );
            w::notice(ui, t.orange, "warning", Some(tl!("Its tools stay on.")), super::runtime_consent::uar_tools(), &t);
            let key_w = 110.0;
            w::fact(ui, tl!("Started by"), key_w, |ui| {
                w::body(ui, tl!("not MailCraft"), t.text);
            });
        });
        super::runtime_consent::understood_box(app, ui, understood);
        match buttons_go(app, ui, &[tl!("Don't Use It"), tl!("Use It")], 0, first, *understood) {
            Some(1) => {
                // Set to the one already running (owner-only), then turned on: each account still
                // needs its own yes.
                let external = run(app, "ai.setRuntimeOption", json!({"runtime": "uar", "instance": "external"}));
                match external.and_then(|_| run(app, "ai.setRuntimeEnabled", json!({"runtime": "uar", "value": true}))) {
                    Ok(_) => {
                        app.status(tl!("Universal Agent Runtime is on. No account's mail goes to it until you allow that account under Accounts."))
                    }
                    Err(e) => app.status(crate::i18n::fmt(tl!("The Universal Agent Runtime wasn't changed: {reason}"), &[("reason", &e)])),
                }
                Said::Close
            }
            Some(_) => Said::Close,
            None => Said::Stay,
        }
    });
    if escape || no { Said::Close } else { said }
}

/// Signing out of ChatGPT: Cancel is the default.
fn sign_out(app: &mut MailApp, ctx: &Context, first: bool) -> Said {
    let t = Tokens::get(ctx);
    let (said, escape) = modal(ctx, "signout", tl!("Sign Out of ChatGPT?"), |ui| {
        w::body(
            ui,
            tl!(
                "MailCraft forgets this sign-in and stops sending any account's mail to ChatGPT. You'd allow each account again after signing back in."
            ),
            t.text,
        );
        match buttons(app, ui, &[tl!("Cancel"), tl!("Sign Out")], 0, first) {
            Some(1) => {
                match run(app, "ai.signOut", json!({"runtime": "chatgpt"})) {
                    Ok(_) => app.status(tl!("MailCraft has forgotten this ChatGPT sign-in on this computer. You can also remove MailCraft from the connected apps in your ChatGPT settings.")),
                    Err(e) => app.status(e),
                }
                Said::Close
            }
            Some(_) => Said::Close,
            None => Said::Stay,
        }
    });
    if escape { Said::Close } else { said }
}

/// The owner's Cancel of a ChatGPT sign-in that waits for the browser: the host stops waiting.
fn cancel_chatgpt_sign_in(app: &MailApp) {
    if let Some(host) = app.services.ai_runtimes.as_deref() {
        host.cancel_sign_in(crate::control::SignIn::ChatGpt);
    }
}
