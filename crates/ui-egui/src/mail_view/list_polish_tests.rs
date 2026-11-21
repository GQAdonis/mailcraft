// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A3, the list and the window's chrome: selection over a flag's tint (G3), an ellipsis on a
//! preview cut short (G9 a), the type floor on the list and the status bar, a disabled
//! Send/Receive drawn small (G14 a), focus rings (G16 a) and Enter on the list (G18 a,
//! SCN-ENTER-001).

use egui::{Color32, Event, Key, Modifiers, Rect, Shape};
use mailcraft_model::Id;
use serde_json::json;

use super::{EnterOpens, FollowUp, PREVIEW_ELLIPSIS, PREVIEW_PITCH, enter_opens, row_fill};
use crate::chrome::type_floor_tests::{assert_type_floor, text_count};
use crate::people_view::testing::{self, Window};
use crate::ribbon::{self, hit_id};
use crate::theme::Tokens;

/// The window on the sample's Inbox, with the reminders out of the way.
fn inbox() -> Window {
    let mut w = testing::window("view.mail");
    let _ = w.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    w.state_mut().execute("folder.select", json!({"kind": "inbox"})).unwrap();
    w.run_steps(3);
    w
}

fn enter(w: &mut Window) {
    for pressed in [true, false] {
        w.input_mut().events.push(Event::Key { key: Key::Enter, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE });
        w.step();
    }
    w.run_steps(3);
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

/// The fills of the rectangles drawn inside `within`.
fn fills(w: &Window, within: Rect) -> Vec<Color32> {
    shapes(w)
        .iter()
        .filter_map(|s| match s {
            Shape::Rect(r) if within.contains_rect(r.rect) && r.fill != Color32::TRANSPARENT => Some(r.fill),
            _ => None,
        })
        .collect()
}

fn first_message(w: &mut Window) -> Id {
    w.state_mut().session.rows().iter().find(|r| r.size() == 1).map(|r| r.id).unwrap()
}

// ---- 3.2: selection wins over the flag's tint ----

#[test]
fn row_fill_is_the_selection_whether_or_not_the_row_is_flagged() {
    for t in [Tokens::light(), Tokens::dark()] {
        for focus in [true, false] {
            let selection = if focus { t.selection } else { t.selection_unfocused };
            assert_eq!(row_fill(&t, FollowUp::Flagged, true, focus), Some(selection), "selected and flagged");
            assert_eq!(row_fill(&t, FollowUp::Overdue, true, focus), Some(selection), "selected and overdue");
            assert_eq!(row_fill(&t, FollowUp::None, true, focus), Some(selection), "selected, no flag");
            assert_eq!(row_fill(&t, FollowUp::Flagged, false, focus), Some(t.flagged_row), "flagged, not selected");
            assert_eq!(row_fill(&t, FollowUp::None, false, focus), None, "neither");
        }
    }
}

#[test]
fn a_selected_flagged_row_is_the_selection_fill_and_keeps_its_flag() {
    let mut w = inbox();
    let id = first_message(&mut w);
    w.state_mut().execute("mail.toggleFlag", json!({"ids": [id]})).unwrap();
    w.state_mut().execute("mail.select", json!({"id": id})).unwrap();
    // The pointer away from the list, so the row shows its marks and not the quick actions.
    w.input_mut().events.push(Event::PointerMoved(egui::pos2(1270.0, 790.0)));
    w.run_steps(4);
    let t = Tokens::get(&w.ctx);
    let row = w.state().hit.rect("message", id).expect("the row is drawn");
    let row_fills = fills(&w, row);
    let selection = [t.selection, t.selection_unfocused];
    assert!(row_fills.iter().any(|f| selection.contains(f)), "the selection's fill: {row_fills:?}");
    assert!(!row_fills.contains(&t.flagged_row), "no flag tint under the selection");
    assert!(!row_fills.iter().any(|f| selection.iter().any(|s| *f == s.gamma_multiply(0.72))), "no blend");
    assert!(w.state().hit.rect("messageFlagMark", id).is_some(), "the flag's mark is drawn");
    assert!(w.state().hit.rect("messageFlagShade", id).is_none(), "the tint is a row's that isn't selected");
}

// ---- 3.3: an ellipsis on a preview cut short ----

#[test]
fn a_clipped_preview_ends_with_an_ellipsis() {
    let mut w = inbox();
    // A narrow list: every preview is longer than its line.
    w.state_mut().ui.list_width = 260.0;
    w.run_steps(4);
    let list = w.state().hit.rect("messageList", 0).unwrap();
    let previews: Vec<_> = shapes(&w)
        .into_iter()
        .filter_map(|s| match s {
            Shape::Text(text) if list.contains(text.pos) && text.galley.job.sections.iter().any(|s| s.format.line_height == Some(PREVIEW_PITCH)) => {
                Some(text.galley)
            }
            _ => None,
        })
        .collect();
    assert!(!previews.is_empty(), "previews are drawn");
    let clipped: Vec<_> = previews.iter().filter(|g| g.elided).collect();
    assert!(!clipped.is_empty(), "at 260 points some preview is cut short");
    for galley in clipped {
        let last = galley.rows.last().and_then(|r| r.glyphs.last()).map(|g| g.chr);
        assert_eq!(last, Some(PREVIEW_ELLIPSIS), "{:?}", galley.job.text);
    }
}

// ---- 3.4: the type floor on the list and the status bar ----

fn floor_holds(w: &mut Window) {
    w.state_mut().execute("view.conversations", json!({"value": true})).unwrap();
    w.run_steps(4);
    for region in ["messageList", "statusBar"] {
        let rect = w.state().hit.rect(region, 0).unwrap_or_else(|| panic!("{region} is drawn"));
        assert!(text_count(w.output(), rect) > 0, "{region}: text is seen");
        assert_type_floor(w, rect, 11.0);
    }
}

#[test]
fn the_list_and_the_status_bar_are_at_least_eleven_points() {
    floor_holds(&mut inbox());
}

#[test]
fn the_list_and_the_status_bar_are_at_least_eleven_points_at_200_percent() {
    let mut w = inbox();
    w.set_pixels_per_point(2.0);
    w.run_steps(3);
    floor_holds(&mut w);
}

// ---- 3.5: a disabled Send/Receive is small ----

#[test]
fn sync_item_is_large_while_it_can_run_and_small_while_it_cannot() {
    assert_eq!(ribbon::sync_item(true).size, ribbon::Size::Big);
    assert_eq!(ribbon::sync_item(false).size, ribbon::Size::Small);
    assert_eq!(ribbon::sync_item(false).key, ribbon::sync_item(true).key);
}

#[test]
fn a_disabled_send_receive_is_as_tall_as_a_small_item_in_classic() {
    let mut w = inbox();
    w.state_mut().execute("ui.ribbonMode", json!({"value": "classic"})).unwrap();
    w.run_steps(4);
    assert!(!w.state().enabled("ui.sync"), "the test window can't send or receive");
    let sync = w.state().hit.rect("ribbon", hit_id("ui.sync")).expect("Send/Receive is drawn");
    let small = w.state().hit.rect("ribbon", hit_id("mail.pin")).expect("Pin, a small item, is drawn");
    let big = w.state().hit.rect("ribbon", hit_id("mail.new")).expect("New Email, a large item, is drawn");
    assert!((sync.height() - small.height()).abs() < 0.5, "{sync:?} against {small:?}");
    assert!(sync.height() < big.height() - 10.0, "{sync:?} against {big:?}");
}

#[test]
fn a_disabled_send_receive_is_its_icon_alone_in_the_simplified_row() {
    let mut w = inbox();
    w.state_mut().execute("ui.ribbonMode", json!({"value": "simplified"})).unwrap();
    w.run_steps(4);
    let Some(sync) = w.state().hit.rect("ribbon", hit_id("ui.sync")) else {
        // Behind `…` at this width: nothing of it in the row.
        assert!(ribbon::behind_more(&w.ctx).contains(&"ui.sync"));
        return;
    };
    let other = w.state().hit.rect("ribbon", hit_id("mail.new")).unwrap();
    assert!((sync.height() - other.height()).abs() < 0.5, "one height in the row");
    assert!(sync.width() <= 40.0, "no label: {sync:?}");
}

// ---- 3.6: focus rings ----

#[test]
fn the_focus_ring_is_the_keyboards_alone() {
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(200.0, 100.0)).build_ui_state(
        |ui, focus: &mut bool| {
            let rect = Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(60.0, 24.0));
            let resp = ui.interact(rect, egui::Id::unique("ring"), egui::Sense::click());
            if *focus {
                resp.request_focus();
            }
            crate::chrome::focus_ring(ui, &resp, 4.0);
        },
        false,
    );
    h.run_steps(2);
    let ring = |h: &egui_kittest::Harness<'_, bool>| {
        let t = Tokens::get(&h.ctx);
        h.output().shapes.iter().any(|c| matches!(&c.shape, Shape::Rect(r) if r.stroke.color == t.focus_ring && r.stroke.width >= 2.0))
    };
    assert!(!ring(&h), "no ring without focus");
    *h.state_mut() = true;
    h.run_steps(2);
    assert!(ring(&h), "a ring with focus");
}

// ---- 3.7: Enter on the list (SCN-ENTER-001) ----

#[test]
fn scn_enter_001_enter_opens_the_selected_received_message() {
    let mut w = inbox();
    let id = first_message(&mut w);
    w.state_mut().execute("mail.select", json!({"id": id})).unwrap();
    w.run_steps(2);
    assert_eq!(enter_opens(w.state_mut()), Some(EnterOpens::Window(id)));
    assert_eq!(w.state().windows.len(), 0);
    enter(&mut w);
    assert_eq!(w.state().windows.len(), 1, "a window of its own");
    assert!(w.state().windows.of_message(id).is_some(), "the selected message's");
    assert!(w.state().session.compose.is_none(), "read, not written");
}

#[test]
fn neg_enter_with_nothing_selected_nothing_opens() {
    let mut w = inbox();
    w.state_mut().session.sel.messages.clear();
    w.run_steps(2);
    assert_eq!(enter_opens(w.state_mut()), None);
    enter(&mut w);
    assert_eq!(w.state().windows.len(), 0);
    assert!(w.state().session.compose.is_none());
}

#[test]
fn neg_enter_on_a_closed_conversation_nothing_opens() {
    let mut w = inbox();
    w.state_mut().execute("view.conversations", json!({"value": true})).unwrap();
    w.run_steps(3);
    let head = w.state_mut().session.rows().iter().find(|r| r.size() > 1 && !r.open).map(|r| r.id).expect("a conversation");
    w.state_mut().execute("mail.select", json!({"id": head})).unwrap();
    w.run_steps(2);
    assert_eq!(enter_opens(w.state_mut()), None, "which of its messages is not chosen yet");
    enter(&mut w);
    assert_eq!(w.state().windows.len(), 0);
}

#[test]
fn enter_on_a_draft_still_writes_it() {
    let mut w = inbox();
    w.state_mut().execute("folder.select", json!({"kind": "drafts"})).unwrap();
    w.state_mut().execute("mail.first", json!({})).unwrap();
    w.run_steps(3);
    assert_eq!(enter_opens(w.state_mut()), Some(EnterOpens::Draft));
    enter(&mut w);
    assert!(w.state().session.compose.is_some(), "the draft is being written");
}

#[test]
fn enter_with_a_selection_gone_from_the_mailbox_does_nothing() {
    let mut w = inbox();
    w.state_mut().session.sel.messages = vec![Id::MAX];
    assert_eq!(enter_opens(w.state_mut()), None);
    enter(&mut w);
    assert_eq!(w.state().windows.len(), 0);
}
