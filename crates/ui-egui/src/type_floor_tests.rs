// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The type floor (A3.0): one way for every change to say "no text in this part of the window is
//! smaller than so many points". It reads the last frame a test harness drew: every text shape
//! that shows inside the region, and every section of its text, by the size of its font in
//! logical points (so the same check holds at 200 %: `pixels_per_point` doesn't change a font's
//! size in points). Regions come from the window's hit map (`app.hit.rect(kind, id)`) or any
//! rectangle a test works out.
//!
//! Call it from your own test file as `crate::chrome::type_floor_tests::assert_type_floor`.

use egui::{Rect, Shape};
use egui_kittest::Harness;

/// One piece of text drawn too small: its words (trimmed, at most 40 characters), its size in
/// points and where it was drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct SmallText {
    pub text: String,
    pub size: f32,
    pub rect: Rect,
}

/// Every section of text the last frame drew inside `region` (in part is enough) at a size
/// below `floor` points. White space alone doesn't count: it shows nothing to read.
pub fn small_text(output: &egui::FullOutput, region: Rect, floor: f32) -> Vec<SmallText> {
    let mut found = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, clipped.clip_rect, region, floor, &mut found);
    }
    found
}

/// Fails, naming each piece of text, when the last frame drew any text in `region` smaller than
/// `floor` points.
#[track_caller]
pub fn assert_type_floor<S>(harness: &Harness<'_, S>, region: Rect, floor: f32) {
    let small = small_text(harness.output(), region, floor);
    assert!(small.is_empty(), "text below {floor} pt in {region:?}: {small:#?}");
}

/// How many sections of text the last frame drew in `region`, at any size: a test of the floor
/// says this is more than none, so that a region that missed the text can't pass by itself.
pub fn text_count(output: &egui::FullOutput, region: Rect) -> usize {
    small_text(output, region, f32::INFINITY).len()
}

fn walk(shape: &Shape, clip: Rect, region: Rect, floor: f32, found: &mut Vec<SmallText>) {
    match shape {
        Shape::Vec(shapes) => {
            for shape in shapes {
                walk(shape, clip, region, floor, found);
            }
        }
        Shape::Text(text) => {
            let shown = text.visual_bounding_rect().intersect(clip);
            if !shown.is_positive() || !shown.intersects(region) {
                return;
            }
            let job = &text.galley.job;
            for section in &job.sections {
                let size = section.format.font_id.size;
                if size >= floor {
                    continue;
                }
                let words = job.text.get(section.byte_range.start.0..section.byte_range.end.0).unwrap_or("").trim();
                if words.is_empty() {
                    continue;
                }
                found.push(SmallText { text: words.chars().take(40).collect(), size, rect: shown });
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod self_tests {
    use egui::{FontId, RichText, vec2};
    use egui_kittest::Harness;

    use super::{assert_type_floor, small_text, text_count};

    /// Two lines of text, one at each size, one over the other.
    fn harness(sizes: [f32; 2], pixels_per_point: f32) -> Harness<'static, ()> {
        let mut h = Harness::builder().with_size(vec2(400.0, 200.0)).with_pixels_per_point(pixels_per_point).build_ui_state(
            move |ui, _: &mut ()| {
                for (i, size) in sizes.into_iter().enumerate() {
                    ui.label(RichText::new(format!("Line {i} at {size}")).font(FontId::proportional(size)));
                }
            },
            (),
        );
        h.run_steps(2);
        h
    }

    fn whole() -> egui::Rect {
        egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(400.0, 200.0))
    }

    #[test]
    fn the_helper_catches_small_text_and_names_it() {
        let h = harness([10.5, 13.0], 1.0);
        let small = small_text(h.output(), whole(), 11.0);
        assert_eq!(small.len(), 1, "{small:?}");
        assert_eq!(small[0].text, "Line 0 at 10.5");
        assert!((small[0].size - 10.5).abs() < 1e-3);
        let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| assert_type_floor(&h, whole(), 11.0)));
        let message = failed.err().and_then(|e| e.downcast_ref::<String>().cloned()).unwrap_or_default();
        assert!(message.contains("Line 0 at 10.5"), "the failure names the text: {message}");
    }

    #[test]
    fn the_helper_passes_eleven_points_at_200_percent() {
        let h = harness([11.0, 14.0], 2.0);
        assert_eq!(text_count(h.output(), whole()), 2, "both lines are seen");
        assert_type_floor(&h, whole(), 11.0);
    }

    #[test]
    fn text_outside_the_region_is_not_counted() {
        let h = harness([13.0, 9.0], 1.0);
        let first = small_text(h.output(), whole(), f32::INFINITY);
        let top = first.iter().find(|s| s.text.starts_with("Line 0")).map(|s| s.rect).unwrap();
        // Only the first line's rectangle: the 9 pt line under it is out of the region.
        assert_type_floor(&h, top.shrink2(vec2(0.0, 1.0)), 11.0);
        assert!(!small_text(h.output(), whole(), 11.0).is_empty());
    }
}
