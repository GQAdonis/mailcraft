// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File › Options › AI assistance, the owner's questions (#ai): before an account's mail may go
//! to a model service on the internet or a gateway to one (`ui.aiConsent`), and the level-2
//! gateway question (does this server on this computer send mail to the internet?).
//!
//! Each is a question to the person at the screen, never a script's or an agent's: while one is
//! open no input from the control channel reaches the window ([`super::follow`]). The gateway
//! question closed without an answer (Not Now, Escape) sends nothing and is asked again next
//! time; the default answer of each is the one that sends nothing.

use mailcraft_model::ai::AiKind;
use serde_json::{Value, json};

use super::settings::{Kind, MAX_ADDRESS};
use super::settings_state::GatewayAsk;
use super::{Provider, Providers, flag, text};
use crate::MailApp;
use crate::dialogs::{Dialog, Question};

/// Most passages sent with one question, as the question to the owner says.
const MAX_PASSAGES: usize = mailcraft_model::ai::MAX_PASSAGES;

/// The question before `account`'s mail may go to the model service (or gateway) in use.
fn ask_consent(app: &mut MailApp, account: &str, provider: &Provider) -> Result<(), String> {
    if app.dialog.is_some() {
        return Err(tl!("Close the dialog that is open first").to_string());
    }
    let host = provider.host();
    let args = [("host", host.as_str()), ("address", account), ("n", &MAX_PASSAGES.to_string())];
    // With an embeddings model, every message of the account goes too, in the background.
    let template = if provider.embedding_model.trim().is_empty() {
        tl!(
            "What is sent. When you ask a question, MailCraft picks up to {n} messages from {address} that seem to answer it, and sends {host} your question with each one's subject, its sender's name and address, its date and the passage of its text that seems to answer. Attachments, pictures, hidden text, passwords and sign-in tokens are never sent, and neither is mail from your other accounts.\n\nWho receives it. {host}, over an encrypted connection. It may keep what it receives under its own terms; MailCraft can't see what it does with it.\n\nHow to stop. Choose Stop beside {address} in File › Options › AI assistance, or set Model to None. Anything already sent can't be called back."
        )
    } else {
        tl!(
            "What is sent. In the background, every message of {address} has its subject and its visible text sent to {host}, to prepare searching by meaning. When you ask a question, MailCraft picks up to {n} messages from {address} that seem to answer it, and sends {host} your question with each one's subject, its sender's name and address, its date and the passage of its text that seems to answer. Attachments, pictures, hidden text, passwords and sign-in tokens are never sent, and neither is mail from your other accounts.\n\nWho receives it. {host}, over an encrypted connection. It may keep what it receives under its own terms; MailCraft can't see what it does with it.\n\nHow to stop. Choose Stop beside {address} in File › Options › AI assistance, or set Model to None. Anything already sent can't be called back."
        )
    };
    let text = crate::i18n::fmt(template, &args);
    let ask = Question {
        title: crate::i18n::fmt(tl!("Let {host} Read Mail from {address}?"), &args),
        text,
        action: tl!("Allow").to_string(),
        with: Value::Null,
        more: Vec::new(),
        note: None,
        cancel: Some(tl!("Don't Allow").to_string()),
        tick: None,
    };
    app.dialog = Some(Dialog::Confirm { command: "ui.aiConsent".to_string(), params: json!({"account": account, "allowed": true}), ask });
    Ok(())
}

/// The gateway question (level 2), as a dialog: Yes, It Does; No, It Doesn't; Not Now.
fn ask_gateway(app: &mut MailApp, ask: &GatewayAsk) {
    let question = Question {
        title: tl!("Does This Server Send Your Mail to the Internet?").to_string(),
        text: ask.text(),
        action: tl!("Yes, It Does").to_string(),
        with: json!({"gatewayAnswer": true}),
        more: vec![(tl!("No, It Doesn't").to_string(), json!({"gatewayAnswer": false}))],
        note: None,
        cancel: Some(tl!("Not Now").to_string()),
        tick: None,
    };
    app.dialog = Some(Dialog::Confirm {
        command: "ui.aiSettings".to_string(),
        params: json!({"gatewayProvider": ask.provider, "gatewayAddress": ask.address, "gatewayModels": ask.models}),
        ask: question,
    });
    app.ai.settings.gateway_shown = Some(ask.provider.clone());
}

/// Whether the dialog open is the gateway question.
pub(crate) fn gateway_dialog_open(app: &MailApp) -> bool {
    matches!(&app.dialog, Some(Dialog::Confirm { command, params, .. }) if command == "ui.aiSettings" && params.get("gatewayProvider").is_some())
}

/// Whether the gateway question open is about `pending` (the same provider, address and list
/// of models).
fn shows(app: &MailApp, pending: Option<&GatewayAsk>) -> bool {
    let Some(Dialog::Confirm { params, .. }) = &app.dialog else { return false };
    let Some(ask) = pending else { return false };
    let of = |key: &str| params.get(key).and_then(Value::as_str).unwrap_or_default();
    of("gatewayProvider") == ask.provider && of("gatewayAddress") == ask.address && of("gatewayModels") == ask.models
}

/// Once a frame: the gateway question the host is waiting on is put to the owner, once; closed
/// without an answer (Not Now, Escape), nothing is sent and the host asks again next time. When
/// the host's question changes (another provider, address or list of models) or goes while it
/// is open, it is closed, unanswered and not dismissed, and the new one is put instead.
pub(crate) fn follow_gateway(app: &mut MailApp) {
    let Some(host) = app.services.ai_providers.as_ref() else { return };
    let mut open = gateway_dialog_open(app);
    let pending = host.gateway_question().as_ref().and_then(GatewayAsk::read);
    if open && !shows(app, pending.as_ref()) {
        // Not the owner's Not Now: the question itself changed. Nothing is answered for it.
        app.dialog = None;
        app.ai.settings.gateway_shown = None;
        open = false;
    }
    if !open && app.ai.settings.gateway_shown.is_some() {
        app.ai.settings.gateway_shown = None;
        if pending.is_some() {
            host.dismiss_gateway_question();
        }
        super::providers_changed(app);
        if let Some(ask) = pending {
            app.status(crate::i18n::fmt(tl!("Waiting for your answer about {address}. Nothing was sent to it."), &[("address", &ask.address)]));
            app.ai.settings.gateway_waiting = Some(ask.address);
        }
        return;
    }
    if open || app.dialog.is_some() {
        return;
    }
    if let Some(ask) = pending {
        ask_gateway(app, &ask);
    }
}

/// The owner's answer to the gateway question. It is an answer to the question the host is
/// asking now, with that question's fingerprint of the list of models; one given for a question
/// that changed or went (`gatewayModels` of another list) is refused, and the new one is asked.
pub(crate) fn answer_gateway(app: &mut MailApp, p: &Value, sends: bool) -> Result<(), String> {
    let provider = text(p, "gatewayProvider", 512)?.ok_or("`gatewayProvider` is the id of the provider asked about")?;
    let address = text(p, "gatewayAddress", MAX_ADDRESS)?.unwrap_or_default();
    let models = text(p, "gatewayModels", MAX_ADDRESS)?;
    // The question's dialog is closed by now: not a Not Now, whatever comes of the answer.
    app.ai.settings.gateway_shown = None;
    let host = app.services.ai_providers.as_ref().ok_or_else(|| tl!("This build can't use a model").to_string())?;
    let current = host.gateway_question().as_ref().and_then(GatewayAsk::read);
    let Some(current) = current.filter(|q| q.provider == provider && models.as_ref().is_none_or(|m| *m == q.models)) else {
        return Err(tl!("The question changed before your answer. Nothing was sent; you'll be asked again.").to_string());
    };
    app.ai.settings.gateway_waiting = None;
    let answered = host.answer_gateway(&provider, &current.models, sends);
    super::providers_changed(app);
    answered.map_err(|_| tl!("The answer couldn't be kept. You'll be asked again.").to_string())?;
    let providers = Providers::of(app);
    let kind = if sends { Kind::Gateway } else { Kind::Local };
    app.ai.settings.load(&providers, Some(kind));
    app.ai.settings.refused = None;
    let said = if sends {
        tl!("{address} is used as a gateway. No account's mail goes to it until you allow it.")
    } else {
        tl!("{address} is used as a model on this computer.")
    };
    app.status(crate::i18n::fmt(said, &[("address", &address)]));
    Ok(())
}

/// `ui.aiConsent`: `{"account": address, "allowed"?: bool, "confirmed"?: bool}`. Without
/// `allowed` (or with `true` and not `confirmed`) it asks the owner; `false` stops at once.
pub(crate) fn consent_command(app: &mut MailApp, p: &Value) -> Result<Value, String> {
    let wanted = text(p, "account", 320)?.ok_or("`account` is the address of one of the mailbox's accounts")?;
    let account = app
        .session
        .mailbox
        .accounts
        .iter()
        .map(|a| a.email.trim().to_string())
        .find(|a| a.eq_ignore_ascii_case(wanted.trim()))
        .ok_or("`account` is the address of one of the mailbox's accounts")?;
    let allowed = flag(p, "allowed")?;
    let confirmed = flag(p, "confirmed")? == Some(true);
    let providers = Providers::of(app);
    let provider = providers.active().filter(|p| p.needs_consent()).cloned().ok_or_else(|| {
        tl!("Only a model service on the internet, or a gateway to one, needs your permission for an account's mail, and none is in use").to_string()
    })?;
    match allowed {
        Some(false) => stop_consent(app, &provider, &account)?,
        Some(true) if confirmed => {
            let host = app.services.ai_providers.as_ref().ok_or_else(|| tl!("This build can't use a model").to_string())?;
            host.set_consent(&provider.id, &account, true).map_err(|_| tl!("The permission couldn't be kept").to_string())?;
            super::providers_changed(app);
            let said = crate::i18n::fmt(
                tl!("Mail from {address} may now go to {host} when you ask a question."),
                &[("address", &account), ("host", &provider.host())],
            );
            app.status(said);
        }
        _ => ask_consent(app, &account, &provider)?,
    }
    let now = Providers::of(app).active().is_some_and(|p| p.allows(&account));
    Ok(json!({"account": account, "allowed": now}))
}

/// Stop beside an account: at once, with no question, and what is under way that may carry its
/// mail is called off.
fn stop_consent(app: &mut MailApp, provider: &Provider, account: &str) -> Result<(), String> {
    let host = app.services.ai_providers.as_ref().ok_or_else(|| tl!("This build can't use a model").to_string())?;
    let taken = host.set_consent(&provider.id, account, false);
    super::providers_changed(app);
    taken.map_err(|_| tl!("The permission couldn't be taken back").to_string())?;
    super::cancel_live(app, Some(AiKind::Answer));
    let said = crate::i18n::fmt(
        tl!("Mail from {address} no longer goes to {host}. What was sent before can't be called back."),
        &[("address", account), ("host", &provider.host())],
    );
    app.status(said);
    Ok(())
}
