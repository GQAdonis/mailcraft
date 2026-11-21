// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A6, an answer's sources and where it came from: the Sources list is exactly the citations, a
//! row shows its message, a message gone since says so and opens nothing, a stopped answer keeps
//! only what it cited so far, the provenance line names the model and a hostname alone, nothing
//! in an answer is a link, the pane's text is at least 11 points, and the rows have names and a
//! focus ring.

use egui::{Event, Modifiers, PointerButton, Shape};
use egui_kittest::kittest::Queryable;
use serde_json::{Value, json};

use super::sources::{Place, cited_so_far, hostname, place_of, provenance};
use super::{Cited, Stage};
use crate::ai::Providers;
use crate::chrome::type_floor_tests::{assert_type_floor, text_count};
use crate::people_view::testing::{self, Window};
use crate::theme::Tokens;

/// A question the sample mailbox answers with several messages.
const QUESTION: &str = "team update";

fn window() -> Window {
    let mut w = testing::window("view.mail");
    let _ = w.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    w.run_steps(2);
    w
}

/// Asks [`QUESTION`]: the request, and how many messages went with it.
fn ask(w: &mut Window) -> (u64, usize) {
    let asked = w.state_mut().execute("ui.answers", json!({"question": QUESTION})).unwrap();
    w.run_steps(2);
    let request = asked.get("request").and_then(Value::as_u64).expect("the sample answers the question");
    let sent = w.state().ai.answers.as_ref().map_or(0, |p| p.sent);
    (request, sent)
}

fn came(w: &mut Window, request: u64, event: Value) {
    w.state_mut().execute("ai.apply", json!({"requestId": request, "event": event})).unwrap();
    w.run_steps(4);
}

fn numbers(w: &Window) -> Vec<usize> {
    w.state().ai.answers.as_ref().map(|p| p.cited.iter().map(|c| c.number).collect()).unwrap_or_default()
}

fn cited(number: usize) -> Cited {
    Cited { number, id: 100 + number as u64, sender: "Ada".into(), subject: "Venue".into(), date: "14 Oct 2026".into() }
}

/// Every shape of the last frame, flattened.
fn shapes(w: &Window) -> Vec<Shape> {
    fn walk(shape: &Shape, out: &mut Vec<Shape>) {
        match shape {
            Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
            other => out.push(other.clone()),
        }
    }
    let mut out = Vec::new();
    for clipped in &w.output().shapes {
        walk(&clipped.shape, &mut out);
    }
    out
}

/// One click at `pos`, the output of each frame kept: whether any of them asked to open a page.
fn click_opens_a_page(w: &mut Window, pos: egui::Pos2) -> bool {
    w.run_steps(45);
    w.input_mut().events.push(Event::PointerMoved(pos));
    w.step();
    let mut opened = false;
    for pressed in [true, false] {
        w.input_mut().events.push(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        w.step();
        opened |= w.output().platform_output.commands.iter().any(|c| matches!(c, egui::OutputCommand::OpenUrl(_)));
    }
    w.step();
    opened || w.output().platform_output.commands.iter().any(|c| matches!(c, egui::OutputCommand::OpenUrl(_)))
}

// ---- 6.1: the Sources list is the citations ----

#[test]
fn the_sources_are_exactly_the_citations_in_their_order() {
    let mut w = window();
    let (request, sent) = ask(&mut w);
    assert!(sent >= 3, "the sample sends at least three messages for {QUESTION:?}");
    came(&mut w, request, json!({"type": "done", "text": "It moved to Monday [1]; the room is booked [3]."}));
    assert_eq!(w.state().ai.answers.as_ref().map(|p| p.stage), Some(Stage::Done));
    assert_eq!(numbers(&w), [1, 3]);
    let hit = &w.state().hit;
    assert!(hit.rect("answerSource", 1).is_some() && hit.rect("answerSource", 3).is_some());
    assert!(hit.rect("answerSource", 2).is_none(), "no row for a message the answer didn't cite");
    let said = w.state_mut().execute("ui.answers", json!({"open": true})).unwrap();
    let listed: Vec<u64> = said["sources"].as_array().unwrap().iter().filter_map(|s| s["number"].as_u64()).collect();
    assert_eq!(listed, [1, 3]);
}

#[test]
fn a_row_shows_its_message_in_the_reading_pane() {
    let mut w = window();
    let (request, _) = ask(&mut w);
    came(&mut w, request, json!({"type": "done", "text": "Monday [2]."}));
    let id = w.state().ai.answers.as_ref().and_then(|p| p.cited.first().map(|c| c.id)).unwrap();
    let row = w.state().hit.rect("answerSource", 2).expect("the row is drawn");
    testing::click(&mut w, row.center(), PointerButton::Primary, 1);
    w.run_steps(3);
    assert_eq!(w.state().session.sel.focus(), Some(id), "the cited message is selected");
    assert!(w.state().ai.answers.is_some(), "the answer stays in view");
}

// ---- 6.3: a cited message that is gone ----

#[test]
fn a_cited_message_gone_since_says_so_and_opens_nothing() {
    let mut w = window();
    let (request, _) = ask(&mut w);
    came(&mut w, request, json!({"type": "done", "text": "Monday [1]."}));
    let id = w.state().ai.answers.as_ref().and_then(|p| p.cited.first().map(|c| c.id)).unwrap();
    let before = w.state().session.sel.focus();
    w.state_mut().session.mailbox.remove_message(id).unwrap();
    w.run_steps(3);
    assert!(w.state().hit.rect("answerSource", 1).is_none());
    let row = w.state().hit.rect("answerSourceGone", 1).expect("the row stays and says the message is gone");
    assert!(w.query_all_by_label_contains("message no longer available").next().is_some(), "the row's name says the message is no longer available");
    testing::click(&mut w, row.center(), PointerButton::Primary, 2);
    assert_eq!(w.state().session.sel.focus(), before, "nothing was opened");
    assert!(w.state().ai.answers.is_some());
}

// ---- 6.4: after Stop ----

#[test]
fn a_stopped_answer_keeps_only_the_sources_cited_so_far() {
    let mut w = window();
    let (request, _) = ask(&mut w);
    came(&mut w, request, json!({"type": "delta", "text": "The deposit is due Friday [1], and the"}));
    w.state_mut().execute("ui.answers", json!({"stop": true})).unwrap();
    w.run_steps(4);
    assert_eq!(w.state().ai.answers.as_ref().map(|p| p.stage), Some(Stage::Stopped));
    assert_eq!(numbers(&w), [1]);
    assert!(w.state().hit.rect("answerSource", 1).is_some());
}

#[test]
fn only_numbers_the_text_named_are_kept_after_stop() {
    let all = vec![cited(1), cited(2), cited(3)];
    let kept: Vec<usize> = cited_so_far(all.clone(), "First [2], then [2] again, and [9]").iter().map(|c| c.number).collect();
    assert_eq!(kept, [2]);
    assert!(cited_so_far(all.clone(), "Nothing named yet [").is_empty());
    let order: Vec<usize> = cited_so_far(all, "[3] before [1]").iter().map(|c| c.number).collect();
    assert_eq!(order, [1, 3], "the host's order stays");
}

// ---- 6.2: the provenance line ----

fn providers(kind: &str, base_url: &str) -> Providers {
    Providers::read(&json!({"active": "p1", "providers": [{"id": "p1", "kind": kind, "base_url": base_url, "model": "qwen-small"}]}))
}

#[test]
fn a_remote_answer_names_the_model_and_the_hostname_alone() {
    let p = providers("remote", "https://user:pw@models.example:8443/v1/chat/completions?key=sk-SECRET#frag");
    let line = provenance(Some(&("p1".into(), "qwen-small".into())), &p, 4);
    assert!(line.contains("qwen-small"), "{line}");
    assert!(line.contains("at models.example,"), "{line}");
    for never in ["8443", "/v1", "chat", "key", "sk-SECRET", "frag", "user", "pw", "https"] {
        assert!(!line.contains(never), "{never:?} in {line}");
    }
    assert!(line.contains("4 passages"), "{line}");
    assert!(line.contains("treated as data"), "{line}");
}

#[test]
fn where_the_model_ran_is_said_for_each_kind() {
    let here = provenance(Some(&("p1".into(), String::new())), &providers("local", "http://127.0.0.1:11434/v1"), 1);
    assert!(here.contains("qwen-small on this computer") && here.contains("1 passage "), "{here}");
    let gateway = provenance(None, &providers("gateway", "http://localhost:4000/v1?token=x"), 2);
    assert!(gateway.contains("through the gateway at localhost,") && !gateway.contains("4000") && !gateway.contains("token"), "{gateway}");
    let device = provenance(Some(&("apple-on-device".into(), "apple".into())), &Providers::default(), 3);
    assert!(device.contains("apple on this computer"), "{device}");
    let unknown = provenance(Some(&("elsewhere".into(), "m".into())), &Providers::default(), 3);
    assert!(unknown.starts_with("Answered by m, from 3 passages"), "{unknown}");
    assert!(provenance(None, &Providers::default(), 0).contains("a model"));
    assert_eq!(place_of("x", None), Place::Unknown);
    assert_eq!(hostname("https://[::1]:8080/v1"), "[::1]");
    assert_eq!(hostname("not an address"), hostname(""));
}

#[test]
fn the_provenance_line_is_drawn_under_a_finished_answer() {
    let mut w = window();
    let (request, _) = ask(&mut w);
    came(&mut w, request, json!({"type": "done", "text": "Monday [1].", "answeredBy": {"provider": "elsewhere", "model": "tiny-model"}}));
    assert!(w.state().hit.rect("answerBy", 0).is_some());
    assert!(w.query_all_by_label_contains("Answered by tiny-model").next().is_some());
    assert!(w.query_all_by_label_contains("treated as data").next().is_some());
}

// ---- answers stay inert ----

#[test]
fn an_address_in_an_answer_is_plain_text_and_opens_nothing() {
    let mut w = window();
    let (request, _) = ask(&mut w);
    came(&mut w, request, json!({"type": "done", "text": "Read https://tracker.example/steal?q=1 and [see](https://evil.example/x) [1]."}));
    let text = w.state().hit.rect("answerText", 0).expect("the answer is drawn");
    assert!(w.query_all_by_label_contains("https://tracker.example/steal?q=1").next().is_some(), "the address is shown whole");
    for x in [0.15, 0.4, 0.7] {
        let at = egui::pos2(text.left() + text.width() * x, text.top() + 6.0);
        assert!(!click_opens_a_page(&mut w, at), "a click at {at:?} opened a page");
    }
    assert!(w.state().ai.answers.is_some());
}

// ---- type floor, names and focus ----

#[test]
fn the_pane_is_at_least_eleven_points_while_streaming_and_when_done() {
    let mut w = window();
    let (request, _) = ask(&mut w);
    came(&mut w, request, json!({"type": "delta", "text": "Writing about the venue [1]"}));
    let pane = w.state().hit.rect("answers", 0).expect("the pane is drawn");
    assert!(text_count(w.output(), pane) > 0);
    assert_type_floor(&w, pane, 11.0);
    came(&mut w, request, json!({"type": "done", "text": "The venue moved [1] and the date [2]."}));
    let pane = w.state().hit.rect("answers", 0).expect("the pane is drawn");
    assert!(text_count(w.output(), pane) > 0);
    assert_type_floor(&w, pane, 11.0);
}

#[test]
fn the_rows_have_names_and_the_keyboard_wears_a_focus_ring() {
    let mut w = window();
    let (request, _) = ask(&mut w);
    came(&mut w, request, json!({"type": "done", "text": "Monday [1], the room [2]."}));
    assert!(w.query_all_by_label_contains("Source 1:").next().is_some());
    assert!(w.query_all_by_label_contains("Source 2:").next().is_some());
    // The first source has the keyboard when the answer is done: it wears the ring.
    let row = w.state().hit.rect("answerSource", 1).expect("drawn");
    let ring = Tokens::get(&w.ctx).focus_ring;
    let worn = shapes(&w).iter().any(|s| matches!(s, Shape::Rect(r) if r.stroke.color == ring && r.stroke.width >= 2.0 && row.contains_rect(r.rect)));
    assert!(worn, "a focus ring inside the first row");
    // Down moves it to the next row.
    testing::key(&mut w, egui::Key::ArrowDown);
    let second = w.state().hit.rect("answerSource", 2).expect("drawn");
    let moved = shapes(&w).iter().any(|s| matches!(s, Shape::Rect(r) if r.stroke.color == ring && second.contains_rect(r.rect)));
    assert!(moved, "the ring moved to the second row");
}
