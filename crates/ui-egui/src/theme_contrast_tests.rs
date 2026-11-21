// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The contrast of the colours the interface sets against each other (WCAG 2.x relative
//! luminance): text 4.5:1, icons and the outlines of controls 3:1. Disabled controls are exempt
//! from the rule (checked at 3:1 so that they stay legible).

use egui::Color32;

use super::{Accent, Tokens};

const TEXT: f32 = 4.5;
const GRAPHIC: f32 = 3.0;
const STRONG: f32 = 7.0;

fn channel(v: u8) -> f32 {
    let c = f32::from(v) / 255.0;
    if c <= 0.039_28 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn luminance(c: Color32) -> f32 {
    0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
}

/// The contrast ratio of two opaque colours.
fn ratio(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

type Pair = (&'static str, fn(&Tokens) -> Color32, fn(&Tokens) -> Color32, f32);

const PAIRS: &[Pair] = &[
    ("text on surface", |t| t.text, |t| t.surface, TEXT),
    ("text on panel", |t| t.text, |t| t.panel, TEXT),
    ("text on window", |t| t.text, |t| t.canvas, TEXT),
    ("text on ribbon", |t| t.text, |t| t.ribbon, TEXT),
    ("text on selection", |t| t.text, |t| t.selection, TEXT),
    ("text on flagged_row", |t| t.text, |t| t.flagged_row, TEXT),
    ("text on hover", |t| t.text, |t| t.hover, TEXT),
    ("text on checked", |t| t.text, |t| t.checked, TEXT),
    ("text on today", |t| t.text, |t| t.today, TEXT),
    ("text_dim on surface", |t| t.text_dim, |t| t.surface, TEXT),
    ("text_dim on panel", |t| t.text_dim, |t| t.panel, TEXT),
    ("text_dim on window", |t| t.text_dim, |t| t.canvas, TEXT),
    ("text_dim on selection", |t| t.text_dim, |t| t.selection, TEXT),
    ("text_dim on flagged_row", |t| t.text_dim, |t| t.flagged_row, TEXT),
    ("text_disabled on surface", |t| t.text_disabled, |t| t.surface, GRAPHIC),
    ("text_disabled on ribbon", |t| t.text_disabled, |t| t.ribbon, GRAPHIC),
    ("group_label on ribbon", |t| t.group_label, |t| t.ribbon, TEXT),
    ("group_label on window", |t| t.group_label, |t| t.canvas, TEXT),
    ("accent_text on surface", |t| t.accent_text, |t| t.surface, TEXT),
    ("accent_text on panel", |t| t.accent_text, |t| t.panel, TEXT),
    ("accent_text on selection", |t| t.accent_text, |t| t.selection, TEXT),
    ("on_accent on accent", |t| t.on_accent, |t| t.accent, TEXT),
    ("overdue on surface", |t| t.overdue, |t| t.surface, TEXT),
    ("overdue on flagged_row", |t| t.overdue, |t| t.flagged_row, TEXT),
    ("icon on ribbon", |t| t.icon, |t| t.ribbon, GRAPHIC),
    ("input_border on surface", |t| t.input_border, |t| t.surface, GRAPHIC),
    ("red on surface", |t| t.red, |t| t.surface, TEXT),
    ("green on surface", |t| t.green, |t| t.surface, TEXT),
    ("orange on surface", |t| t.orange, |t| t.surface, GRAPHIC),
    ("blue on surface", |t| t.blue, |t| t.surface, TEXT),
    ("icon_red on surface", |t| t.icon_red, |t| t.surface, GRAPHIC),
    ("icon_yellow on surface", |t| t.icon_yellow, |t| t.surface, GRAPHIC),
    ("icon_green on surface", |t| t.icon_green, |t| t.surface, GRAPHIC),
    ("icon_purple on surface", |t| t.icon_purple, |t| t.surface, GRAPHIC),
    ("input_border on ribbon", |t| t.input_border, |t| t.ribbon, GRAPHIC),
    ("field_edge on window", |t| t.field_edge, |t| t.canvas, GRAPHIC),
    ("accent on surface", |t| t.accent, |t| t.surface, GRAPHIC),
    ("accent on panel", |t| t.accent, |t| t.panel, GRAPHIC),
    ("accent_soft_text on accent_soft", |t| t.accent_soft_text, |t| t.accent_soft, TEXT),
    ("text on accent_soft", |t| t.text, |t| t.accent_soft, TEXT),
    ("text_dim on pill_quiet", |t| t.text_dim, |t| t.pill_quiet, TEXT),
    ("text_dim on accent_soft", |t| t.text_dim, |t| t.accent_soft, TEXT),
    ("backstage_ink on column", |t| t.backstage_ink, |t| t.backstage_column, TEXT),
    ("text_disabled on window", |t| t.text_disabled, |t| t.canvas, GRAPHIC),
    // The flag on a flagged row (the row's text, dim text and overdue text are above).
    ("icon_red on flagged_row", |t| t.icon_red, |t| t.flagged_row, GRAPHIC),
    // The AI status chip and the narrowing badge: their words, in the location's colour or in
    // `text`, on its soft fill; and the location's colour as a mark on the window and a sheet.
    ("ai_local on ai_local_soft", |t| t.ai_local, |t| t.ai_local_soft, TEXT),
    ("text on ai_local_soft", |t| t.text, |t| t.ai_local_soft, TEXT),
    ("ai_remote on ai_remote_soft", |t| t.ai_remote, |t| t.ai_remote_soft, TEXT),
    ("text on ai_remote_soft", |t| t.text, |t| t.ai_remote_soft, TEXT),
    ("ai_local on window", |t| t.ai_local, |t| t.canvas, GRAPHIC),
    ("ai_local on surface", |t| t.ai_local, |t| t.surface, GRAPHIC),
    ("ai_remote on window", |t| t.ai_remote, |t| t.canvas, GRAPHIC),
    ("ai_remote on surface", |t| t.ai_remote, |t| t.surface, GRAPHIC),
];

/// Pairs that only high contrast promises: its borders and hairlines are lines you can see.
const STRONG_PAIRS: &[Pair] = &[
    ("border on surface", |t| t.border, |t| t.surface, STRONG),
    ("border on window", |t| t.border, |t| t.canvas, STRONG),
    ("border on ribbon", |t| t.border, |t| t.ribbon, STRONG),
    ("border_strong on surface", |t| t.border_strong, |t| t.surface, STRONG),
    ("ribbon_edge on window", |t| t.ribbon_edge.color, |t| t.canvas, STRONG),
    ("rule on surface", |t| t.rule, |t| t.surface, GRAPHIC),
    ("border_faint on surface", |t| t.border_faint, |t| t.surface, GRAPHIC),
    ("selection_edge on selection", |t| t.selection_edge, |t| t.selection, STRONG),
    ("selection_edge_unfocused on selection_unfocused", |t| t.selection_edge_unfocused, |t| t.selection_unfocused, GRAPHIC),
    ("accent_soft_text on checked", |t| t.accent_soft_text, |t| t.checked, TEXT),
    ("text on pill_quiet", |t| t.text, |t| t.pill_quiet, TEXT),
];

/// Every accent, light and dark (in both its styles, #1384), with high contrast off and on.
fn every_theme() -> Vec<(String, Tokens, bool)> {
    let mut all = Vec::new();
    for accent in Accent::ALL {
        for dark in [false, true] {
            for high in [false, true] {
                let name = format!("{} {} {}", accent.code(), if dark { "dark" } else { "light" }, if high { "high contrast" } else { "normal" });
                all.push((name, Tokens::build(dark, accent, high), high));
                if dark {
                    let name = format!("{} blue dark {}", accent.code(), if high { "high contrast" } else { "normal" });
                    all.push((name, Tokens::build_dark(super::DarkStyle::Blue, accent, high), high));
                }
            }
        }
    }
    all
}

#[test]
fn blue_normal_is_the_look_that_shipped() {
    let (light, dark) = (Tokens::build(false, Accent::Blue, false), Tokens::build(true, Accent::Blue, false));
    assert_eq!(light.accent, Tokens::light().accent);
    assert_eq!(light.selection, Tokens::light().selection);
    assert_eq!(dark.accent_soft, Tokens::dark().accent_soft);
    assert_eq!(dark.text_selection, Tokens::dark().text_selection);
    // Re-pinned on purpose (gate G3 a, ui-theme-additions): the flagged row is a quieter wash
    // than the one that shipped (#FCF0F2 light, #3A2A2E dark). Only this token moved.
    assert_eq!(light.flagged_row, Color32::from_rgb(0xFE, 0xF7, 0xF8));
    assert_eq!(dark.flagged_row, Color32::from_rgb(0x2A, 0x22, 0x24));
    assert_eq!(light.flagged_row, Tokens::light().flagged_row);
    assert_eq!(dark.flagged_row, Tokens::dark().flagged_row);
}

/// What falls short among `pairs` in every theme: one line per pair and theme.
fn shortfalls(pairs: &[Pair]) -> Vec<String> {
    let mut wrong = Vec::new();
    for (mode, t, high) in every_theme() {
        let extra: &[Pair] = if high { STRONG_PAIRS } else { &[] };
        for (name, fg, bg, min) in pairs.iter().chain(extra) {
            // Disabled controls are exempt: they stay at 3:1 in both. In high contrast text and
            // outlines need 7:1; icons stay at 3:1.
            let outline = name.contains("border") || name.contains("edge") || name.starts_with("accent on");
            let min = if !high || name.contains("disabled") {
                *min
            } else if *min == TEXT
                || (*min == GRAPHIC && outline && !name.starts_with("rule") && !name.contains("faint") && !name.contains("unfocused"))
            {
                STRONG
            } else {
                *min
            };
            let got = ratio(fg(&t), bg(&t));
            if got < min {
                wrong.push(format!("{name} ({mode}) is {got:.2}, needs {min}"));
            }
        }
    }
    wrong
}

#[test]
fn token_pairs_meet_their_contrast() {
    let wrong = shortfalls(PAIRS);
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// The check itself fails a pair that falls short: a colour on itself is 1:1 in every theme.
#[test]
fn neg_a_pair_below_its_contrast_is_reported() {
    let fixture: &[Pair] = &[("ai_local_soft on ai_local_soft", |t| t.ai_local_soft, |t| t.ai_local_soft, TEXT)];
    let wrong: Vec<String> = shortfalls(fixture).into_iter().filter(|w| w.starts_with("ai_local_soft on")).collect();
    assert_eq!(wrong.len(), every_theme().len(), "{}", wrong.join("\n"));
}
