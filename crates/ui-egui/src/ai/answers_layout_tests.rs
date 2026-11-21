// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Answers pane beside Mail (P1): at 1024 points with the folder pane shown, opening the pane
//! leaves the reading pane readable (the folder pane stands aside as its strip, the owner's own
//! setting untouched), and the strip's button closes Answers then; in a wide window the folder
//! pane stays where it is.

use egui_kittest::kittest::Queryable;
use serde_json::json;

use crate::people_view::testing::{self, Window};

/// The least a reading pane beside the list is given, with the Answers pane open or not: the
/// least `mail_view` leaves it beside the list.
const READABLE: f32 = 259.5;

fn window(width: f32) -> Window {
    let mut w = testing::window("view.mail");
    let _ = w.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    w.set_size(egui::vec2(width, 768.0));
    w.state_mut().execute("view.folderPane", json!({"value": true})).unwrap();
    w.state_mut().execute("view.readingPane", json!({"value": "right"})).unwrap();
    w.run_steps(2);
    w.state_mut().execute("ui.answers", json!({"open": true})).unwrap();
    w.run_steps(4);
    w
}

#[test]
fn at_1024_points_the_reading_pane_stays_readable_beside_the_answers_pane() {
    let mut w = window(1024.0);
    let answers = w.state().hit.rect("answers", 0).expect("the Answers pane is drawn");
    let reading = w.state().hit.rect("readingPane", 0).expect("the reading pane is drawn");
    assert!(reading.width() >= READABLE, "the reading pane is {} wide", reading.width());
    assert!(reading.max.x <= answers.min.x + 1.0, "the reading pane is left of Answers");
    assert!(w.state().ai.folders_yield, "the folder pane stands aside");
    assert!(w.state().session.view.folder_pane, "the owner's setting is untouched");
    // The strip's button closes Answers, and the folder pane is back.
    w.get_by_label("Show the folder pane (closes Answers)").click();
    w.run_steps(4);
    assert!(w.state().ai.answers.is_none(), "Answers closed");
    assert!(!w.state().ai.folders_yield, "the folder pane is back");
}

#[test]
fn a_wide_window_keeps_the_folder_pane_beside_the_answers_pane() {
    let w = window(1600.0);
    assert!(w.state().hit.rect("answers", 0).is_some());
    assert!(!w.state().ai.folders_yield, "nothing stands aside");
    let reading = w.state().hit.rect("readingPane", 0).expect("the reading pane is drawn");
    assert!(reading.width() >= READABLE, "the reading pane is {} wide", reading.width());
}
