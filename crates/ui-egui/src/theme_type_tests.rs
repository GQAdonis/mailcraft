// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The type rungs: the 11 pt floor for the surfaces the AI and reading changes touch, while the
//! global `Small` style stays where every other module expects it.

use super::{Appearance, InterfaceSize, ThemeChoice, TypeRung, apply};

#[test]
fn the_note_rung_is_eleven_points_and_small_stays_ten_and_a_half() {
    assert_eq!(TypeRung::Note.size(), 11.0);
    assert_eq!(TypeRung::Note.regular().size, 11.0);
    assert_eq!(TypeRung::Note.semibold().size, 11.0);
    assert!(TypeRung::Note.line_height() >= TypeRung::Note.size());
    for percent in InterfaceSize::PRESETS {
        let ctx = egui::Context::default();
        apply(&ctx, Appearance::Light.preference(None), &ThemeChoice::default());
        let size = InterfaceSize::from_percent(f64::from(percent)).unwrap();
        ctx.set_zoom_factor(size.zoom_factor());
        // Sizes are logical points: the interface size scales them all alike.
        assert_eq!(TypeRung::Note.size(), 11.0, "{percent} %");
        let small = ctx.global_style().text_styles.get(&egui::TextStyle::Small).map(|f| f.size);
        assert_eq!(small, Some(10.5), "{percent} %: the global Small style");
    }
}

#[test]
fn no_rung_is_below_the_note_floor() {
    for rung in [TypeRung::Chrome, TypeRung::Control, TypeRung::Caption, TypeRung::Note] {
        assert!(rung.size() >= TypeRung::Note.size(), "{rung:?}");
    }
}
