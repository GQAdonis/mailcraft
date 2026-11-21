// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The runtime chain page's sign-in dialogs (B11, 21.3): Claude Code's sign-in opens the owner's
//! terminal through the host (`claude auth login`, never a login of MailCraft's own) and moves on
//! once the host says it is signed in; where no terminal can be opened the command is shown with
//! Copy as the default; Escape cancels every dialog; the waiting minutes are counted down; the
//! question before using a runtime MailCraft didn't start answers no to Enter.

use egui::Key;
use egui_kittest::kittest::Queryable;
use mailcraft_model::ai::RuntimeId;

use super::runtime_rows::Facts;
use super::settings_runtimes::dialog_name;
use super::settings_runtimes_tests::{focus_name, focused, host, window};
use super::signin::{ClaudeStep, GptStep, RtDialog, WAIT_MS, follow, minutes_left};
use crate::control::RuntimeState;

#[test]
fn claude_code_sign_in_opens_the_owners_terminal_and_waits_for_the_host() {
    let h = host();
    h.set_state("claude_code", RuntimeState::SignedOut);
    let mut w = window(&h);
    w.get_by_label("Sign In…: Claude Code").click_accesskit();
    w.run_steps(4);
    assert_eq!(dialog_name(w.state()), Some("aiSignInClaudeCode"));
    assert!(h.calls().contains(&"signIn ClaudeCodeTerminal".to_string()), "{:?}", h.calls());
    let _ = w.get_by_label("Copy claude auth login");
    assert!(focused(&w, "Cancel"), "Cancel is the default while waiting");
    // The host says it is signed in: the dialog says so.
    h.set_state("claude_code", RuntimeState::Ready);
    w.run_steps(4);
    assert!(matches!(w.state().ai.runtimes.dialog.as_ref().map(|o| &o.dialog), Some(RtDialog::Claude(ClaudeStep::Done))));
    assert!(focused(&w, "Done"));
    w.key_press(Key::Enter);
    w.run_steps(4);
    assert_eq!(dialog_name(w.state()), None);
}

#[test]
fn with_no_terminal_the_command_is_shown_and_copy_has_the_keyboard() {
    let h = host();
    h.set_state("claude_code", RuntimeState::SignedOut);
    h.0.borrow_mut().sign_in_fails = true;
    let mut w = window(&h);
    w.get_by_label("Sign In…: Claude Code").click_accesskit();
    w.run_steps(4);
    assert!(matches!(w.state().ai.runtimes.dialog.as_ref().map(|o| &o.dialog), Some(RtDialog::Claude(ClaudeStep::NoTerminal { .. }))));
    assert!(focused(&w, "Copy claude auth login"));
    w.key_press(Key::Escape);
    w.run_steps(4);
    assert_eq!(dialog_name(w.state()), None, "Escape cancels");
    assert!(focused(&w, "Sign In…: Claude Code"), "the keyboard goes back to what opened it, not {:?}", focus_name(&w));
}

#[test]
fn every_dialog_closes_on_escape_and_the_questions_default_to_no() {
    let h = host();
    let mut w = window(&h);
    for (name, default) in [("chatgpt", "Open Sign-in Page"), ("chatgptWaiting", "Cancel"), ("uar", "Don't Use It"), ("signOut", "Cancel")] {
        w.state_mut().stand_in_runtime_dialog(name);
        w.run_steps(4);
        assert!(dialog_name(w.state()).is_some(), "{name} opens");
        assert!(focused(&w, default), "{name}: {default} has the keyboard");
        w.key_press(Key::Escape);
        w.run_steps(4);
        assert_eq!(dialog_name(w.state()), None, "{name} closes on Escape");
    }
    // Enter on the UAR question is Don't Use It: nothing is turned on.
    w.state_mut().stand_in_runtime_dialog("uar");
    w.run_steps(4);
    w.key_press(Key::Enter);
    w.run_steps(4);
    assert_eq!(dialog_name(w.state()), None);
    assert!(!h.enabled("uar"));
    assert!(!h.calls().iter().any(|c| c.starts_with("signIn") || c.starts_with("signOut")), "{:?}", h.calls());
}

#[test]
fn a_wait_says_the_minutes_left_and_ends_after_ten_minutes() {
    assert_eq!(minutes_left(WAIT_MS, 0.0), 10);
    assert_eq!(minutes_left(WAIT_MS, 1.0), 9);
    assert_eq!(minutes_left(WAIT_MS, WAIT_MS), 1);
    assert_eq!(minutes_left(f64::NAN, 0.0), 1);
    let facts = Facts::default();
    let waiting = RtDialog::ChatGpt(GptStep::Waiting { until_ms: 100.0 });
    assert_eq!(follow(&waiting, &facts, 50.0), None);
    assert_eq!(follow(&waiting, &facts, 100.0), Some(RtDialog::ChatGpt(GptStep::TimedOut)));
    let claude = RtDialog::Claude(ClaudeStep::Terminal { until_ms: 100.0 });
    assert_eq!(follow(&claude, &facts, 101.0), Some(RtDialog::Claude(ClaudeStep::TimedOut)));
    assert_eq!(waiting.waiting_for(), Some(RuntimeId::ChatGpt));
    assert!(RtDialog::Consent { runtime: RuntimeId::Codex, account: "a@example.org".into() }.asks());
}
