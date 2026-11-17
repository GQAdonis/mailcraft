// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An answer's text on screen (#ai): the display blocks [`mailcraft_html::markdown_blocks`] made
//! of what the model wrote, drawn as plain text with its emphasis, lists, quotes and code.
//!
//! What a model writes is untrusted. Nothing of it is HTML here, nothing runs, and nothing it
//! names is fetched: a picture or a table never reaches the screen (the mapper drops pictures
//! and makes no tables; one that came anyway is left out), and a link is plain text with its
//! whole address after its words, never something to click: an address a model wrote could carry
//! what it read of the mail to anybody's server. A `[n]` in the text is a link to source `n`
//! only when that source is among the citations; any other bracketed number is plain text.

use egui::{RichText, Stroke, Ui, vec2};
use mailcraft_html::{Block, BlockKind, Marker, Span};

use crate::theme::{Tokens, mono, regular, semibold};

/// The answer's text size: the reading pane's base size.
const SIZE: f32 = 13.0;
/// From one line to the next: 1.5 times the size.
const LINE: f32 = 19.5;

/// A piece of an answer's text: words, or a marker that names a source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Piece {
    Text(String),
    Source(usize),
}

/// `text` cut where it says `[n]` for a number `n` among `cited`; any other bracket stays text.
pub(crate) fn pieces(text: &str, cited: &[usize]) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut plain = String::new();
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let (before, from) = rest.split_at(open);
        plain.push_str(before);
        let inside = from.get(1..).unwrap_or_default();
        let digits: String = inside.chars().take_while(char::is_ascii_digit).take(3).collect();
        let closes = inside.get(digits.len()..).is_some_and(|after| after.starts_with(']'));
        match digits.parse::<usize>().ok().filter(|n| closes && cited.contains(n)) {
            Some(n) => {
                if !plain.is_empty() {
                    out.push(Piece::Text(std::mem::take(&mut plain)));
                }
                out.push(Piece::Source(n));
                // `[`, the digits, `]`: all ASCII.
                rest = from.get(digits.len().saturating_add(2)..).unwrap_or_default();
            }
            None => {
                plain.push('[');
                rest = inside;
            }
        }
    }
    plain.push_str(rest);
    if !plain.is_empty() {
        out.push(Piece::Text(plain));
    }
    out
}

/// Whether a link's words already say where it leads (so the address needn't follow them).
pub(crate) fn says_where(words: &str, url: &str) -> bool {
    let plain = |s: &str| {
        let s = s.trim().to_ascii_lowercase();
        let s = s.strip_prefix("mailto:").or_else(|| s.strip_prefix("tel:")).map(str::to_string).unwrap_or(s);
        let s = s.strip_prefix("https://").or_else(|| s.strip_prefix("http://")).map(str::to_string).unwrap_or(s);
        s.trim_end_matches('/').to_string()
    };
    plain(words) == plain(url)
}

/// A span's words as they are drawn.
fn rich(span: &Span, words: &str, heading: bool, t: &Tokens) -> RichText {
    let mut r = RichText::new(words).color(t.text).line_height(Some(LINE));
    r = if span.code {
        r.font(mono(12.0)).background_color(t.surface_sunken)
    } else if span.bold || heading {
        r.font(semibold(SIZE))
    } else {
        r.font(regular(SIZE))
    };
    if span.italic {
        r = r.italics();
    }
    if span.underline {
        r = r.underline();
    }
    if span.strike {
        r = r.strikethrough();
    }
    r
}

/// A list item's marker.
fn marker_text(marker: &Marker) -> Option<String> {
    match marker {
        Marker::Bullet => Some("•".to_string()),
        Marker::Number(n) => Some(format!("{n}.")),
        Marker::Label(l) => Some(super::cut(l, 8)),
        Marker::None => None,
    }
}

/// What a span shows: its words with the source markers in them, or, for a link, its words and
/// where it really leads, as plain text. A link a model wrote is never followed from here: its
/// address could carry what the model read of the mail to anybody's server.
pub(crate) fn span_pieces(s: &Span, cited: &[usize]) -> Vec<Piece> {
    match &s.link {
        Some(url) => vec![Piece::Text(link_words(&s.text, url))],
        None => pieces(&s.text, cited),
    }
}

/// A link's words, and its whole address after them when they say something else.
pub(crate) fn link_words(words: &str, url: &str) -> String {
    if says_where(words, url) { words.to_string() } else { format!("{words} ({url})") }
}

/// One span, drawn: only a marker of a source shown under the answer is a link (to that source).
fn span(ui: &mut Ui, s: &Span, heading: bool, cited: &[usize], t: &Tokens, picked: &mut Option<usize>) {
    for piece in span_pieces(s, cited) {
        match piece {
            Piece::Text(words) => {
                ui.add(egui::Label::new(rich(s, &words, heading, t)));
            }
            Piece::Source(n) => {
                let marker = RichText::new(format!("[{n}]")).font(semibold(12.0)).color(t.accent_text);
                let link = ui.add(egui::Link::new(marker)).on_hover_text(crate::i18n::fmt(tl!("Show source {n}"), &[("n", &n.to_string())]));
                if link.clicked() {
                    *picked = Some(n);
                }
            }
        }
    }
}

/// The spans of a block, wrapped in the room there is, `indent` in from the left.
fn line(ui: &mut Ui, block: &Block, indent: f32, lead: Option<String>, cited: &[usize], t: &Tokens, picked: &mut Option<usize>) {
    let heading = matches!(block.kind, BlockKind::Heading(_));
    ui.horizontal(|ui| {
        ui.add_space(indent);
        if let Some(lead) = lead {
            ui.add_sized(vec2(18.0, LINE), egui::Label::new(RichText::new(lead).font(regular(SIZE)).color(t.text_dim)));
        }
        ui.vertical(|ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                for s in &block.spans {
                    span(ui, s, heading, cited, t, picked);
                }
            });
        });
    });
}

/// Draws `blocks`; the source the owner clicked a marker of, if any. `cited` are the numbers of
/// the sources shown under the answer.
pub(crate) fn show(ui: &mut Ui, blocks: &[Block], cited: &[usize], t: &Tokens) -> Option<usize> {
    let mut picked = None;
    for (i, block) in blocks.iter().enumerate() {
        if i > 0 {
            ui.add_space(if block.space > 0 { 8.0 } else { 2.0 });
        }
        let indent = f32::from(block.quote.min(8)) * 14.0;
        match &block.kind {
            BlockKind::Paragraph | BlockKind::Heading(_) => line(ui, block, indent, None, cited, t, &mut picked),
            BlockKind::ListItem { marker, depth } => {
                let indent = indent + f32::from((*depth).min(8)) * 18.0;
                line(ui, block, indent, marker_text(marker), cited, t, &mut picked);
            }
            BlockKind::Preformatted => {
                ui.horizontal(|ui| {
                    ui.add_space(indent);
                    egui::Frame::NONE.fill(t.surface_sunken).corner_radius(4.0).inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.add(egui::Label::new(RichText::new(block.text()).font(mono(12.0)).color(t.text)).wrap());
                    });
                });
            }
            BlockKind::Rule => {
                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 9.0), egui::Sense::hover());
                ui.painter().hline(r.x_range(), r.center().y, Stroke::new(1.0, t.rule));
            }
            // Never drawn: an answer shows no picture and no table.
            BlockKind::Image { .. } | BlockKind::Table(_) => {}
        }
    }
    picked
}
