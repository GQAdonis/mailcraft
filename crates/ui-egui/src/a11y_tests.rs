// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! SCN-A11Y-001, every new control of the refresh (A1–A8) and of the runtime settings (B11) is
//! found by its accessible name and reached by the keyboard: the controls listed here are each
//! found by name and the count found is the count listed; Tab reaches the AI chip, the ribbon's AI
//! group and the Agents button (PLAN §5.9); Escape closes every panel they open. The negative
//! control shows the name query isn't vacuous: a control with no name is never found by one.

use std::collections::BTreeSet;

use egui::Key;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;

use crate::people_view::testing::{self, Window};

/// The main window's new controls, by name (and the AI chip, whose name says where AI runs).
const MAIN_WINDOW: [&str; 4] = ["Ask Mail", "Find by Description", "AI Settings", "Agents"];
/// The Agents panel's (A8).
const AGENTS_PANEL: [&str; 2] = ["Close Agents", "Stop Agent Access"];
/// The Answers pane's (A6).
const ANSWERS_PANE: [&str; 1] = ["Close Answer"];
/// Find by Description's panel (A5).
const DESCRIBE_PANEL: [&str; 3] = ["Suggest Search", "Answer Instead", "Cancel"];
/// File › Options › AI assistance as the runtime chain (B11), with Claude Code on and Codex off and
/// signed out.
const RUNTIMES_PAGE: [&str; 12] = [
    "Move Claude Code down",
    "Use Claude Code",
    "Details: Claude Code",
    "Move Codex up",
    "Copy codex login",
    "Look Again: Codex",
    "Use Codex",
    "Details: Codex",
    "Add OpenAI-Compatible Provider…",
    "Try the next one",
    "Ask me first",
    "Stop and tell me",
];

/// The window with a provider service, so the AI controls are drawn.
fn window() -> Window {
    crate::ribbon::ai_group_tests::window_with_host(1440.0).0
}

/// How many of `names` are found by their accessible name (Find by Description is both a ribbon
/// button and the search box's).
fn found(w: &Window, names: &[&str]) -> usize {
    names.iter().filter(|name| w.query_all_by_label(name).next().is_some()).count()
}

/// The name of what has the keyboard.
fn focus_name(w: &Window) -> Option<String> {
    w.query_by(|n| n.is_focused()).and_then(|n| n.accesskit_node().label())
}

/// The AI chip in the status bar: its name says where AI runs ("AI: …").
fn chip_name(w: &Window) -> Option<String> {
    w.query_by(|n| n.label().is_some_and(|l| l.starts_with("AI: "))).and_then(|n| n.accesskit_node().label())
}

#[test]
fn neg_a11y_a_control_without_a_name_is_never_found_by_one() {
    let mut h = Harness::new_ui(|ui| {
        // A control with no AccessKit name, and one with a name beside it.
        let _ = ui.allocate_response(egui::vec2(40.0, 20.0), egui::Sense::click());
        let _ = ui.button("Named fixture");
    });
    h.run();
    assert!(h.query_by_label("Named fixture").is_some(), "the query finds a named control");
    for guess in ["", "fixture", "Unnamed fixture", "button"] {
        assert!(h.query_by_label(guess).is_none(), "{guess:?} finds nothing");
    }
    assert_eq!(h.query_all_by(|n| n.label().is_some_and(|l| !l.is_empty())).count(), 1, "one named node");
}

#[test]
fn a11y_every_listed_control_is_found_by_its_name_and_the_count_matches() {
    let mut listed = 0;
    let mut seen = 0;
    let mut w = window();
    listed += MAIN_WINDOW.len() + 1;
    seen += found(&w, &MAIN_WINDOW) + usize::from(chip_name(&w).is_some());
    w.state_mut().run("ui.agents", json!({})).unwrap();
    w.run_steps(4);
    listed += AGENTS_PANEL.len();
    seen += found(&w, &AGENTS_PANEL);
    w.state_mut().run("ui.agents", json!({})).unwrap();
    w.state_mut().execute("ui.answers", json!({"open": true})).unwrap();
    w.run_steps(4);
    listed += ANSWERS_PANE.len();
    seen += found(&w, &ANSWERS_PANE);
    w.state_mut().execute("ui.answers", json!({"open": false})).unwrap();
    w.state_mut().execute("ui.findByDescription", json!({"open": true, "text": "invoices from Ada"})).unwrap();
    w.run_steps(4);
    listed += DESCRIBE_PANEL.len();
    seen += found(&w, &DESCRIBE_PANEL);
    let host = crate::ai::settings_runtimes_tests::host();
    let w = crate::ai::settings_runtimes_tests::window(&host);
    listed += RUNTIMES_PAGE.len();
    let missing: Vec<_> = RUNTIMES_PAGE.iter().filter(|n| found(&w, &[n]) == 0).collect();
    seen += RUNTIMES_PAGE.len() - missing.len();
    assert!(missing.is_empty(), "not found by name: {missing:?}");
    assert_eq!(seen, listed, "every listed control is found by its name");
}

#[test]
fn a11y_tab_reaches_the_ai_chip_the_ai_group_and_the_agents_button() {
    let mut w = window();
    let chip = chip_name(&w).expect("the AI chip is in the status bar");
    let wanted: BTreeSet<String> =
        ["Ask Mail", "Find by Description", "AI Settings", "Agents", chip.as_str()].into_iter().map(String::from).collect();
    let mut reached = BTreeSet::new();
    // Round the window's controls once and a bit, however many there are.
    for _ in 0..400 {
        w.key_press(Key::Tab);
        w.run_steps(1);
        if let Some(name) = focus_name(&w) {
            reached.insert(name);
        }
        if wanted.is_subset(&reached) {
            break;
        }
    }
    let missed: Vec<_> = wanted.difference(&reached).collect();
    assert!(missed.is_empty(), "Tab never reached {missed:?}");
}

#[test]
fn a11y_escape_closes_every_panel_the_new_controls_open() {
    let mut w = window();
    w.get_by_label("Agents").click();
    w.run_steps(4);
    assert!(w.state().ai.agents.open, "Agents opens its panel");
    testing::key(&mut w, Key::Escape);
    w.run_steps(2);
    assert!(!w.state().ai.agents.open, "Escape closes the Agents panel");

    w.query_all_by_label("Find by Description").next().expect("Find by Description").click();
    w.run_steps(4);
    assert!(w.state().ai.narrow.is_some(), "Find by Description opens its panel");
    testing::key(&mut w, Key::Escape);
    w.run_steps(2);
    assert!(w.state().ai.narrow.is_none(), "Escape closes Find by Description");

    w.state_mut().execute("ui.answers", json!({"open": true})).unwrap();
    w.run_steps(4);
    w.get_by_label("Close Answer").focus();
    w.run_steps(1);
    testing::key(&mut w, Key::Escape);
    w.run_steps(2);
    assert!(w.state().ai.answers.is_none(), "Escape closes the Answers pane");

    let host = crate::ai::settings_runtimes_tests::host();
    let mut w = crate::ai::settings_runtimes_tests::window(&host);
    w.get_by_label("Details: Claude Code").click_accesskit();
    w.run_steps(3);
    assert!(w.state().ai.runtimes.open.is_some(), "Details opens its panel");
    w.key_press(Key::Escape);
    w.run_steps(3);
    assert!(w.state().ai.runtimes.open.is_none(), "Escape closes the details panel");
}
