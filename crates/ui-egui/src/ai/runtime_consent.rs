// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Accounts and what may leave this computer (B11, 21.4): the accounts × runtimes matrix, and the
//! dialog that asks before an account's mail may go to a runtime that sends text elsewhere.
//!
//! Turning a runtime on allows nothing: each account's yes is its own, given only by the go-ahead
//! of the dialog, which says what is sent, to whom, what the runtime may do and how to stop.
//! Don't Allow is its default focus and the only answer Enter and Escape reach, and no input from
//! the control channel reaches the window while it is open ([`super::follow`]). Stop takes the yes
//! back at once, with no question.

use egui::{Context, Ui, vec2};
use mailcraft_model::ai::{RuntimeId, RuntimeKind, Where};
use serde_json::json;

use super::runtime_rows::{self as rows, Facts, RowFacts, Status};
use super::runtime_widgets as w;
use super::settings_runtimes::run;
use super::signin::{self, Said};
use crate::MailApp;
use crate::theme::Tokens;

/// The account column's least width, and a runtime column's.
const ACCOUNT_W: f32 = 210.0;
const RUNTIME_W: f32 = 120.0;

/// A column of the matrix: "On this computer", or a runtime that leaves it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Column {
    /// `None`: the column of every runtime on this computer.
    pub(crate) runtime: Option<RuntimeId>,
    pub(crate) name: String,
    pub(crate) off: bool,
}

/// An account, by address, with the name it goes by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AccountRow {
    pub(crate) address: String,
    pub(crate) name: String,
}

/// The columns: one "On this computer" when any runtime here is in the chain, then each runtime
/// that leaves this computer, offered here and installed, in chain order.
pub(crate) fn columns(facts: &Facts) -> Vec<Column> {
    let mut out = Vec::new();
    if facts.shown().any(|f| f.place == Where::Local) {
        out.push(Column { runtime: None, name: tl!("On this computer").to_string(), off: false });
    }
    for f in facts.shown().filter(|f| f.place == Where::Leaves && facts.status_of(f) != Status::NotInstalled) {
        out.push(Column { runtime: Some(f.id.clone()), name: rows::name(&f.id), off: !f.enabled });
    }
    out
}

/// The window's accounts that have an address.
pub(crate) fn accounts(app: &MailApp) -> Vec<AccountRow> {
    app.session
        .mailbox
        .accounts
        .iter()
        .take(256)
        .filter_map(|a| {
            let address = super::settings_state::clean(&a.email, 120);
            mailcraft_model::ai::runtime::account_key(&address)?;
            Some(AccountRow { name: super::settings_state::clean(&a.display_name, 80), address })
        })
        .collect()
}

/// The consent dialog's title and its parts, each with its lead words in SemiBold.
pub(crate) fn consent_text(f: &RowFacts, account: &str) -> (String, Vec<(String, String)>) {
    let runtime = rows::name(&f.id);
    let fmt = crate::i18n::fmt;
    let n = mailcraft_model::ai::MAX_PASSAGES.to_string();
    let args = [("runtime", runtime.as_str()), ("account", account), ("n", n.as_str())];
    let title = fmt(tl!("Let {runtime} Read Mail from {account}?"), &args);
    let mut parts = vec![(
        tl!("What is sent.").to_string(),
        fmt(
            tl!(
                "When you ask a question, MailCraft picks up to {n} messages from {account} that seem to answer it, and sends your question with each one's subject, its sender's name and address, its date and the passage of its text that seems to answer. Attachments, pictures, hidden text, passwords and sign-in tokens are never sent, and neither is mail from your other accounts."
            ),
            &args,
        ),
    )];
    let host = rows::vendor(f);
    let whom = match f.id.kind() {
        RuntimeKind::ChatGpt => tl!(
            "OpenAI, under your ChatGPT sign-in, over an encrypted connection. MailCraft asks OpenAI not to keep the conversation, but it handles what it receives under its own terms for your plan, and MailCraft can't see what it does with it."
        )
        .to_string(),
        RuntimeKind::ClaudeCode => tl!(
            "Claude Code on this computer, which sends it to Anthropic under your own Claude sign-in. Anthropic handles it under the terms of your Claude plan; MailCraft can't see what it does with it."
        )
        .to_string(),
        RuntimeKind::Codex => tl!(
            "Codex on this computer, which sends it to OpenAI under your own ChatGPT sign-in. The skills and agent instructions kept in your Codex folder go along with it: MailCraft can't turn them off. OpenAI handles it under the terms of your plan; MailCraft can't see what it does with it."
        )
        .to_string(),
        RuntimeKind::Uar => tl!(
            "The Universal Agent Runtime on this computer, which MailCraft didn't start. Where it sends what it receives depends on how it was set up, and MailCraft can't tell."
        )
        .to_string(),
        RuntimeKind::Compat | RuntimeKind::OnDevice => fmt(
            tl!("{host}, over an encrypted connection. It may keep what it receives under its own terms; MailCraft can't see what it does with it."),
            &[("host", &host)],
        ),
    };
    parts.push((tl!("Who receives it.").to_string(), whom));
    let may = match f.id.kind() {
        RuntimeKind::Uar => format!("{} {}", tl!("MailCraft can't turn off its tools."), uar_tools()),
        RuntimeKind::Codex => format!(
            "{} {}",
            tl!(
                "Read what it's given and write an answer, nothing else. It gets no tools: if it tries to run a command, open a file, browse the web or use any other tool, MailCraft stops it."
            ),
            tl!("MailCraft runs Codex inside the system sandbox, which keeps it from writing your files and from reading most of them.")
        ),
        _ => tl!(
            "Read what it's given and write an answer, nothing else. It gets no tools: if it tries to run a command, open a file, browse the web or use any other tool, MailCraft stops it."
        )
        .to_string(),
    };
    parts.push((tl!("What it may do.").to_string(), may));
    let plan = match f.id.kind() {
        RuntimeKind::ChatGpt | RuntimeKind::Codex => Some(tl!("ChatGPT")),
        RuntimeKind::ClaudeCode => Some(tl!("Claude")),
        _ => None,
    };
    if let Some(plan) = plan {
        parts.push((String::new(), fmt(tl!("Each answer counts against your {plan} plan's usage limits."), &[("plan", plan)])));
    }
    parts.push((
        tl!("How to stop.").to_string(),
        fmt(
            tl!(
                "Choose Stop beside {account} under Accounts in File › Options › AI assistance, or turn {runtime} off. What was already sent can't be called back."
            ),
            &args,
        ),
    ));
    (title, parts)
}

/// What a Universal Agent Runtime MailCraft didn't start may do with its own tools, said plainly:
/// a message can set them off, one may have run before MailCraft sees it, and nothing it did can
/// be undone by MailCraft.
pub(crate) fn uar_tools() -> &'static str {
    tl!(
        "Because of what a message says, it may run commands, open files or browse the web. MailCraft stops the answer when it sees a tool used, but by then the tool may already have run, and MailCraft can't undo what it did."
    )
}

/// The words the owner ticks before mail may go to a Universal Agent Runtime MailCraft didn't
/// start, or before it is used at all.
pub(crate) fn uar_understood() -> &'static str {
    tl!("I understand this runtime can run commands because of what a message says")
}

/// Whether going ahead with `f` needs the owner's tick first: the Universal Agent Runtime, which
/// this version only ever uses as one MailCraft didn't start, and whose tools it can't turn off.
pub(crate) fn needs_understood(f: &RowFacts) -> bool {
    f.id.kind() == RuntimeKind::Uar
}

/// The tick box under a question about an external Universal Agent Runtime: unticked until the
/// owner ticks it; the go-ahead stays greyed out until then.
pub(crate) fn understood_box(app: &mut MailApp, ui: &mut Ui, ticked: &mut bool) {
    let r = ui.checkbox(ticked, uar_understood());
    app.hit.add("aiUarUnderstood", 0, r.rect);
}

/// The consent dialog. Don't Allow is the default focus, and what Enter and Escape answer.
pub(crate) fn dialog(
    app: &mut MailApp,
    ctx: &Context,
    runtime: &RuntimeId,
    account: &str,
    facts: &Facts,
    first: bool,
    understood: &mut bool,
) -> Said {
    let t = Tokens::get(ctx);
    let Some(f) = facts.rows.iter().find(|f| &f.id == runtime) else { return Said::Close };
    let (title, parts) = consent_text(f, account);
    // Enter means Don't Allow whatever has the keyboard: taken before the dialog is drawn.
    let enter = crate::keys::take_enter_out(ctx, signin::layer("consent"));
    let (said, escape) = signin::modal(ctx, "consent", &title, |ui| {
        signin::scrolled(ui, |ui| {
            for (lead, words) in &parts {
                let mut job = egui::text::LayoutJob::default();
                if !lead.is_empty() {
                    job.append(lead, 0.0, egui::TextFormat::simple(crate::theme::semibold(12.5), t.text));
                    job.append(" ", 0.0, egui::TextFormat::simple(crate::theme::regular(12.5), t.text));
                }
                job.append(words, 0.0, egui::TextFormat::simple(crate::theme::regular(12.5), t.text));
                ui.add(egui::Label::new(job).wrap());
            }
        });
        let ticked_needed = needs_understood(f);
        if ticked_needed {
            understood_box(app, ui, understood);
        }
        let may_allow = !ticked_needed || *understood;
        let mut said = Said::Stay;
        crate::dialogs::button_row(ui, |ui| {
            let allow = w::button(ui, tl!("Allow"), tl!("Allow"), may_allow);
            let no = w::button(ui, tl!("Don't Allow"), tl!("Don't Allow"), true);
            app.hit.add("aiConsentAllow", 0, allow.rect);
            app.hit.add("aiConsentDont", 0, no.rect);
            if first {
                no.request_focus();
            }
            if no.clicked() {
                said = Said::Close;
            } else if allow.clicked() && may_allow {
                let name = rows::name(runtime);
                match run(app, "ai.setConsent", json!({"runtime": runtime.to_string(), "account": account, "allowed": true})) {
                    Ok(_) => app.status(crate::i18n::fmt(tl!("{account} may now use {runtime}."), &[("account", account), ("runtime", &name)])),
                    Err(e) => app.status(e),
                }
                said = Said::Close;
            }
        });
        said
    });
    if escape || enter { Said::Close } else { said }
}

/// What the matrix asked for this frame: a dialog to open, from the control that opened it.
pub(crate) struct Asked {
    pub(crate) consent: Option<(RuntimeId, String, egui::Id)>,
}

/// The section's caption and the matrix: a grid, or one block per account where the columns
/// don't fit.
pub(crate) fn matrix(app: &mut MailApp, ui: &mut Ui, facts: &Facts, t: &Tokens) -> Asked {
    let mut asked = Asked { consent: None };
    w::caption(
        ui,
        tl!(
            "Runtimes on this computer read every account, and nothing leaves. A runtime that sends text elsewhere gets an account's mail only after you allow that account here. Turning a runtime on doesn't allow anything."
        ),
        t,
    );
    let cols = columns(facts);
    let accounts = accounts(app);
    if cols.is_empty() || accounts.is_empty() {
        w::caption(ui, tl!("There are no accounts or runtimes to show yet."), t);
        return asked;
    }
    let stacked = ACCOUNT_W + RUNTIME_W * cols.len() as f32 > ui.available_width();
    let col_w = ((ui.available_width() - ACCOUNT_W - 8.0 * cols.len() as f32) / cols.len() as f32).max(RUNTIME_W);
    let ncols = u64::try_from(cols.len()).unwrap_or(u64::MAX);
    for (ai, account) in accounts.iter().enumerate() {
        let base = u64::try_from(ai).unwrap_or(0).saturating_mul(ncols);
        if stacked {
            if ai > 0 {
                ui.add_space(4.0);
                ui.separator();
            }
            account_head(ui, account, t);
            for (ci, col) in cols.iter().enumerate() {
                ui.horizontal_wrapped(|ui| {
                    ui.add(egui::Label::new(egui::RichText::new(&col.name).font(crate::theme::medium(12.5)).color(t.text)));
                    cell(app, ui, facts, account, col, base.saturating_add(ci as u64), &mut asked, t);
                });
            }
        } else {
            if ai == 0 {
                head_row(ui, &cols, col_w, t);
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(vec2(ACCOUNT_W, 36.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.set_width(ACCOUNT_W);
                    account_head(ui, account, t);
                });
                for (ci, col) in cols.iter().enumerate() {
                    ui.allocate_ui_with_layout(vec2(col_w, 36.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.set_width(col_w);
                        cell(app, ui, facts, account, col, base.saturating_add(ci as u64), &mut asked, t);
                    });
                }
            });
        }
    }
    asked
}

fn head_row(ui: &mut Ui, cols: &[Column], col_w: f32, t: &Tokens) {
    ui.horizontal(|ui| {
        ui.allocate_exact_size(vec2(ACCOUNT_W, 30.0), egui::Sense::hover());
        for col in cols {
            ui.allocate_ui_with_layout(vec2(col_w, 30.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_width(col_w);
                ui.add(egui::Label::new(egui::RichText::new(&col.name).font(crate::theme::semibold(11.0)).color(t.text)).wrap());
                if col.off {
                    ui.add(egui::Label::new(egui::RichText::new(tl!("Off")).font(crate::theme::regular(11.0)).color(t.text_dim)));
                }
            });
        }
    });
}

fn account_head(ui: &mut Ui, account: &AccountRow, t: &Tokens) {
    if !account.name.is_empty() {
        ui.add(egui::Label::new(egui::RichText::new(&account.name).font(crate::theme::regular(11.0)).color(t.text_dim)).truncate());
    }
    ui.add(egui::Label::new(egui::RichText::new(&account.address).font(crate::theme::regular(12.5)).color(t.text)).truncate());
}

#[allow(clippy::too_many_arguments)]
fn cell(app: &mut MailApp, ui: &mut Ui, facts: &Facts, account: &AccountRow, col: &Column, hit: u64, asked: &mut Asked, t: &Tokens) {
    let Some(runtime) = &col.runtime else {
        w::where_tag(ui, tl!("Stays here"), true, t);
        return;
    };
    let allowed = facts.settings.consents.allows(&account.address, runtime);
    ui.horizontal(|ui| {
        if allowed {
            w::tag(ui, tl!("Allowed"), Some("check"), t.green, egui::Color32::TRANSPARENT, None, t.green);
            let label = tl!("Stop");
            let name = crate::i18n::fmt(
                tl!("{label}: {account} mail to {runtime}"),
                &[("label", label), ("account", &account.address), ("runtime", &col.name)],
            );
            let b = w::button(ui, label, &name, true);
            app.hit.add("aiConsentCell", hit, b.rect);
            if b.clicked() {
                match run(app, "ai.setConsent", json!({"runtime": runtime.to_string(), "account": account.address, "allowed": false})) {
                    Ok(_) => app.status(crate::i18n::fmt(
                        tl!("Stopped. No more mail from {account} goes to {runtime}. What was already sent can't be called back."),
                        &[("account", &account.address), ("runtime", &col.name)],
                    )),
                    Err(e) => app.status(e),
                }
                // The keyboard stays on the cell, which now holds Allow….
                ui.ctx().data_mut(|d| d.insert_temp(refocus_id(), (runtime.to_string(), account.address.to_ascii_lowercase())));
            }
        } else {
            ui.add(egui::Label::new(egui::RichText::new(tl!("Not allowed")).font(crate::theme::regular(11.0)).color(t.text_dim)));
            let label = tl!("Allow…");
            let name = crate::i18n::fmt(
                tl!("{label}: {account} mail to {runtime}"),
                &[("label", label), ("account", &account.address), ("runtime", &col.name)],
            );
            let b = w::button(ui, label, &name, true);
            app.hit.add("aiConsentCell", hit, b.rect);
            let mine = (runtime.to_string(), account.address.to_ascii_lowercase());
            if ui.ctx().data(|d| d.get_temp::<(String, String)>(refocus_id())).is_some_and(|want| want == mine) {
                b.request_focus();
                ui.ctx().data_mut(|d| d.remove::<(String, String)>(refocus_id()));
            }
            if b.clicked() {
                asked.consent = Some((runtime.clone(), account.address.clone(), b.id));
            }
        }
    });
}

/// Where the cell whose Stop was pressed is remembered for a frame, so that the keyboard stays on
/// it (its Allow… button) as it changes.
fn refocus_id() -> egui::Id {
    egui::Id::unique("aiConsentRefocus")
}
