// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The status bar's AI chip (#ai, ai-status-chip): where a question to a model is answered, said
//! in words and with an icon of its own, never by colour alone. A click opens File › Options ›
//! AI assistance (`ui.aiSettings`).
//!
//! It reads what the host says of the providers ([`Providers::of`], once a frame) and nothing
//! else: drawing it asks no model and no server anything, and it never changes a setting or a
//! consent. A build without a provider service (the browser) draws no chip.

use egui::{Sense, Ui, pos2, vec2};
use serde_json::json;

use mailcraft_model::ai::Where;

use super::Providers;
use super::runtime_rows::{self as rows, Facts};
use crate::MailApp;
use crate::theme::{Radius, Tokens, TypeRung};

/// The chip's height, inside the status bar's 30 pt.
const CHIP_H: f32 = 20.0;
/// The icon's size, and the room round it and the words.
const ICON: f32 = 14.0;
const PAD_LEFT: f32 = 6.0;
const GAP: f32 = 5.0;
const PAD_RIGHT: f32 = 8.0;
/// The widest the chip grows; at a narrower window it takes what room there is and its words
/// end in an ellipsis.
const MOST_W: f32 = 280.0;
/// The narrowest it is drawn at all: the icon and a letter or two (so it is never colour alone).
const LEAST_W: f32 = PAD_LEFT + ICON + GAP + 12.0;
/// Most accounts named under the pointer.
const MAX_NAMED: usize = 5;

/// Where AI runs, as the chip says it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Chip {
    /// No model is used: nothing is asked of one.
    Off,
    /// Questions are answered on this computer: a model here, or the on-device backup.
    Local,
    /// The provider in use is a service elsewhere (or a gateway to one) that no account allowed:
    /// no mail goes to it, but what the owner types into Find by Description and Ask Mail's
    /// question box does. `host` is the provider's host name alone (`None`: its address couldn't
    /// be read).
    Typed { host: Option<String>, gateway: bool },
    /// The mail of these accounts may go to a service elsewhere, or through a gateway to one.
    Remote { accounts: Vec<String>, host: String, gateway: bool },
}

impl Chip {
    /// What the providers' settings say, and nothing else.
    pub(crate) fn of(providers: &Providers) -> Chip {
        let Some(active) = providers.active() else {
            return if providers.backup_confirmed().is_some() { Chip::Local } else { Chip::Off };
        };
        if !active.needs_consent() {
            return Chip::Local;
        }
        let mut accounts: Vec<String> = Vec::new();
        for a in active.consented_accounts.iter().take(256) {
            let a = super::settings_state::clean(a, 120);
            if !a.is_empty() && !accounts.iter().any(|b| b.eq_ignore_ascii_case(&a)) {
                accounts.push(a);
            }
        }
        if accounts.is_empty() {
            Chip::Typed { host: host_only(&active.base_url), gateway: active.gateway() }
        } else {
            Chip::Remote { accounts, host: active.host(), gateway: active.gateway() }
        }
    }

    /// The icon's name in [`crate::icons::paint`].
    pub(crate) fn icon(&self) -> &'static str {
        match self {
            Chip::Off => "aiOff",
            Chip::Local => "aiLocal",
            Chip::Typed { .. } | Chip::Remote { .. } => "aiRemote",
        }
    }

    /// Its words, which are its accessible name too.
    pub(crate) fn text(&self) -> String {
        match self {
            Chip::Off => tl!("AI off").to_string(),
            Chip::Local => tl!("AI: on this computer").to_string(),
            Chip::Typed { host: Some(host), .. } => crate::i18n::fmt(tl!("AI: your typed searches go to {host}"), &[("host", host)]),
            Chip::Typed { host: None, gateway: false } => tl!("AI: your typed searches go to a remote service").to_string(),
            Chip::Typed { host: None, gateway: true } => tl!("AI: your typed searches go to the gateway").to_string(),
            Chip::Remote { accounts, .. } => tln!(accounts.len(), "AI: remote allowed for {n} account", "AI: remote allowed for {n} accounts"),
        }
    }

    /// What it says under the pointer: what is sent, to whom, and where to stop it.
    pub(crate) fn tip(&self) -> String {
        match self {
            Chip::Off => tl!("No model is used, and nothing is sent anywhere. Click to set up AI assistance.").to_string(),
            Chip::Local => tl!("Questions are answered on this computer, and no mail leaves it. Click to change AI assistance.").to_string(),
            Chip::Typed { host, gateway } => {
                let said = match (host, gateway) {
                    (Some(_), false) => tl!(
                        "Only what you type into Find by Description and Ask Mail's question box goes to {host}. No mail is sent there until you allow an account. Click to change or stop it in AI assistance."
                    ),
                    (Some(_), true) => tl!(
                        "Only what you type into Find by Description and Ask Mail's question box goes through the gateway at {host} to a service on the internet. No mail is sent there until you allow an account. Click to change or stop it in AI assistance."
                    ),
                    (None, false) => tl!(
                        "Only what you type into Find by Description and Ask Mail's question box goes to a remote service. No mail is sent there until you allow an account. Click to change or stop it in AI assistance."
                    ),
                    (None, true) => tl!(
                        "Only what you type into Find by Description and Ask Mail's question box goes through the gateway to a service on the internet. No mail is sent there until you allow an account. Click to change or stop it in AI assistance."
                    ),
                };
                crate::i18n::fmt(said, &[("host", host.as_deref().unwrap_or_default())])
            }
            Chip::Remote { accounts, host, gateway } => {
                let mut named = accounts.iter().take(MAX_NAMED).cloned().collect::<Vec<_>>().join(", ");
                let more = accounts.len().saturating_sub(MAX_NAMED);
                if more > 0 {
                    named = format!("{named} {}", tln!(more, "and {n} more", "and {n} more"));
                }
                let said = if *gateway {
                    tl!(
                        "Mail of {accounts} may be sent through the gateway at {host} to a service on the internet. Click to change or stop it in AI assistance."
                    )
                } else {
                    tl!("Mail of {accounts} may be sent to {host}. Click to change or stop it in AI assistance.")
                };
                crate::i18n::fmt(said, &[("accounts", &named), ("host", host)])
            }
        }
    }

    /// Its ink and its fill.
    fn colors(&self, t: &Tokens) -> (egui::Color32, egui::Color32) {
        match self {
            Chip::Off => (t.text_dim, egui::Color32::TRANSPARENT),
            Chip::Local => (t.ai_local, t.ai_local_soft),
            Chip::Typed { .. } | Chip::Remote { .. } => (t.ai_remote, t.ai_remote_soft),
        }
    }
}

/// The host name alone of a provider's `base` URL: never a user name or password, port, path,
/// query or key. It is the badge's reading ([`super::narrow::badge_host`], which keeps a port
/// that isn't the scheme's own) with that port taken off. `None` when the URL can't be read.
pub(crate) fn host_only(base: &str) -> Option<String> {
    let shown = super::narrow::badge_host(base)?;
    let host = match shown.strip_prefix('[') {
        // A bracketed IPv6 address: up to its closing bracket.
        Some(inner) => format!("[{}]", inner.split(']').next().unwrap_or_default()),
        None => shown.split(':').next().unwrap_or_default().to_string(),
    };
    (!host.is_empty() && host != "[]").then_some(host)
}

/// The chip, in the status bar's right-to-left block: from the runtime chain when the window has
/// one ([`ChainChip`]), else from the providers' settings. Nothing when the window has neither,
/// or no room for it.
pub(crate) fn status_item(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    let (text, tip, icon, (ink, fill), off) = if app.services.ai_runtimes.is_some() {
        let chip = ChainChip::of(&super::settings_runtimes::facts_of(app));
        (chip.text(), chip.tip(), chip.icon(), chip.colors(t), chip == ChainChip::Off)
    } else if app.services.ai_providers.is_some() {
        let chip = Chip::of(&Providers::of(app));
        (chip.text(), chip.tip(), chip.icon(), chip.colors(t), chip == Chip::Off)
    } else {
        return;
    };
    let font = TypeRung::Note.regular();
    let whole = crate::theme::text_width(ui.painter(), &text, &font, 0.0, t.text);
    let room = ui.available_width() - 8.0;
    let w = (PAD_LEFT + ICON + GAP + whole + PAD_RIGHT).min(MOST_W).min(room);
    if w < LEAST_W {
        return;
    }
    ui.add_space(6.0);
    let (r, resp) = ui.allocate_exact_size(vec2(w, CHIP_H), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &text));
    let painter = ui.painter();
    painter.rect_filled(r, Radius::CARD, if resp.hovered() && off { t.hover } else { fill });
    if resp.hovered() && !off {
        painter.rect_stroke(r, Radius::CARD, egui::Stroke::new(1.0, ink), egui::StrokeKind::Inside);
    }
    let icon_r = egui::Rect::from_center_size(pos2(r.min.x + PAD_LEFT + ICON / 2.0, r.center().y), vec2(ICON, ICON));
    crate::icons::paint(painter, icon_r, icon, ink, ink);
    let x = icon_r.max.x + GAP;
    let words = (r.max.x - PAD_RIGHT - x).max(0.0);
    crate::theme::paint_clipped(painter, pos2(x, r.center().y), words + 1.0, &text, font, ink);
    crate::chrome::focus_ring(ui, &resp, Radius::CARD);
    app.hit.add("aiStatusChip", 0, r);
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if resp.on_hover_text(tip).clicked()
        && let Err(e) = app.run("ui.aiSettings", json!({}))
    {
        log::debug!("AI settings couldn't be opened: {e}");
    }
    ui.add_space(10.0);
}

/// Whether the chip says where typed searches go when a runtime that leaves this computer is on
/// and no account allows it. `false` (the signed scenario SCN-RT-CHIP-001): that state reads "on
/// this computer". Carrying the owner's typed-search wording over to the chain is the owner's to
/// decide, with an amended scenario.
pub(crate) const TYPED_STATE: bool = false;

/// Where AI runs, as the chip says it, read from the runtime chain: the chain, each runtime's
/// `Where` and the consents, and nothing else. Drawing it asks nothing of anyone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ChainChip {
    /// No runtime is on.
    Off,
    /// Every runtime that is on keeps text on this computer.
    Local,
    /// A runtime that leaves this computer is on, and no account allows any that is on: what the
    /// owner types may go to the first of them (`runtimes`, in chain order), and no mail.
    Typed { runtimes: Vec<String> },
    /// The mail of these accounts may go to these runtimes.
    Remote { accounts: Vec<String>, runtimes: Vec<String> },
}

impl ChainChip {
    /// What the chain says.
    pub(crate) fn of(facts: &Facts) -> ChainChip {
        let on: Vec<&super::runtime_rows::RowFacts> = facts.shown().filter(|f| f.enabled).collect();
        if on.is_empty() {
            return ChainChip::Off;
        }
        let leaving: Vec<String> = on.iter().filter(|f| f.place == Where::Leaves).map(|f| rows::name(&f.id)).collect();
        if leaving.is_empty() {
            return ChainChip::Local;
        }
        let pairs = rows::leaving_pairs(facts);
        if pairs.is_empty() {
            return if TYPED_STATE { ChainChip::Typed { runtimes: leaving } } else { ChainChip::Local };
        }
        let mut runtimes: Vec<String> = Vec::new();
        for f in &on {
            if pairs.iter().any(|(_, ids)| ids.contains(&f.id)) {
                runtimes.push(rows::name(&f.id));
            }
        }
        ChainChip::Remote { accounts: pairs.into_iter().map(|(a, _)| a).collect(), runtimes }
    }

    pub(crate) fn icon(&self) -> &'static str {
        match self {
            ChainChip::Off => "aiOff",
            ChainChip::Local => "aiLocal",
            ChainChip::Typed { .. } | ChainChip::Remote { .. } => "aiRemote",
        }
    }

    /// Its words, which are its accessible name too.
    pub(crate) fn text(&self) -> String {
        match self {
            ChainChip::Off => tl!("AI off").to_string(),
            ChainChip::Local => tl!("AI: on this computer").to_string(),
            ChainChip::Typed { runtimes } => crate::i18n::fmt(
                tl!("AI: your typed searches may go to {runtime}"),
                &[("runtime", runtimes.first().map(String::as_str).unwrap_or_default())],
            ),
            ChainChip::Remote { accounts, .. } if accounts.len() == 1 => {
                crate::i18n::fmt(tl!("AI: may leave for {account}"), &[("account", accounts.first().map(String::as_str).unwrap_or_default())])
            }
            ChainChip::Remote { accounts, .. } => tln!(accounts.len(), "AI: may leave for {n} account", "AI: may leave for {n} accounts"),
        }
    }

    /// What it says under the pointer: what is sent, to whom, and how to stop it.
    pub(crate) fn tip(&self) -> String {
        match self {
            ChainChip::Off => tl!("No model is used, and nothing is sent anywhere. Click to set up AI assistance.").to_string(),
            ChainChip::Local => tl!("Questions are answered on this computer, and no mail leaves it. Click to change AI assistance.").to_string(),
            ChainChip::Typed { runtimes } => crate::i18n::fmt(
                tl!(
                    "Only what you type into Find by Description and Ask Mail's question box may go to {runtimes}. No mail is sent there until you allow an account. Click to change or stop it in AI assistance."
                ),
                &[("runtimes", &named(runtimes))],
            ),
            ChainChip::Remote { accounts, runtimes } => crate::i18n::fmt(
                tl!("Mail of {accounts} may go to {runtimes}. Click to change or stop it in AI assistance."),
                &[("accounts", &named(accounts)), ("runtimes", &named(runtimes))],
            ),
        }
    }

    fn colors(&self, t: &Tokens) -> (egui::Color32, egui::Color32) {
        match self {
            ChainChip::Off => (t.text_dim, egui::Color32::TRANSPARENT),
            ChainChip::Local => (t.ai_local, t.ai_local_soft),
            ChainChip::Typed { .. } | ChainChip::Remote { .. } => (t.ai_remote, t.ai_remote_soft),
        }
    }
}

/// At most [`MAX_NAMED`] names joined with commas, then "and {n} more".
pub(crate) fn named(names: &[String]) -> String {
    let mut out = names.iter().take(MAX_NAMED).cloned().collect::<Vec<_>>().join(", ");
    let more = names.len().saturating_sub(MAX_NAMED);
    if more > 0 {
        out = format!("{out} {}", tln!(more, "and {n} more", "and {n} more"));
    }
    out
}
