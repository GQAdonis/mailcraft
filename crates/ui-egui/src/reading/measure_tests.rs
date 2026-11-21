// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The readable measure of the reading pane (SCN-MEASURE-001): flowing text wraps at 72 `ch` of
//! the window's body font, from the left edge, while tables, pictures and `@media` keep the
//! width the pane has. With it, the sender header's gap and face.

use egui::{Color32, FontFamily};
use egui_kittest::kittest::Queryable as _;
use mailcraft_model::{Address, Attachment, Message};

use super::layout::{BASE_SIZE, Item, ItemKind, Laid, MEASURE_CH, measure, measure_of};
use super::tests::{app_with, copy_of, html_message, in_frame, lay_out, png, select, text_of, window};
use crate::theme::{SystemFont, font_definitions_with, regular, semibold};

/// A paragraph several times longer than any line.
fn long_text() -> String {
    "The harbour committee met on Tuesday to agree the order of the boats for the summer regatta. ".repeat(24).trim_end().to_string()
}

fn sender() -> (&'static str, &'static str) {
    ("Harbour Committee", "committee@harbour.example")
}

/// The readable measure in this context's fonts, as text comes.
fn body_measure(ctx: &egui::Context) -> f32 {
    MEASURE_CH * ctx.fonts_mut(|f| f.glyph_width(&regular(BASE_SIZE), '0'))
}

/// The texts of `laid` that hold `words`.
fn texts_with<'a>(laid: &'a Laid, words: &str) -> Vec<&'a Item> {
    laid.items.iter().filter(|i| text_of(i).contains(words)).collect()
}

/// The right edge of each wrapped row of a laid-out text, from the message's left edge.
fn row_ends(ctx: &egui::Context, item: &Item) -> Vec<f32> {
    let ItemKind::Text(text) = &item.kind else { return Vec::new() };
    let galley = ctx.fonts_mut(|f| f.layout_job((*text.job).clone()));
    galley.rows.iter().map(|row| item.rect.min.x + text.anchor + row.rect().max.x).collect()
}

#[test]
fn the_measure_is_72_zeros_of_the_body_font_and_no_wider_than_the_room() {
    // The rule, on a fixed advance.
    assert_eq!(measure_of(7.5, 2000.0), 540.0);
    assert_eq!(measure_of(7.5, 300.0), 300.0);
    // A font without a '0', or an advance that is not a number, leaves the room as it is.
    for zero in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        assert_eq!(measure_of(zero, 900.0), 900.0, "{zero}");
    }
    in_frame(false, |ctx| {
        let zero = ctx.fonts_mut(|f| f.glyph_width(&regular(BASE_SIZE), '0'));
        assert!(zero > 4.0 && zero < 12.0, "the body font's '0' is {zero} wide");
        assert_eq!(measure(ctx, 1.0, 4000.0), MEASURE_CH * zero);
        assert_eq!(measure(ctx, 1.0, 200.0), 200.0);
        // Text set larger has a measure of as many of its own characters.
        let large = ctx.fonts_mut(|f| f.glyph_width(&regular(BASE_SIZE * 2.0), '0'));
        assert_eq!(measure(ctx, 2.0, 4000.0), MEASURE_CH * large);
        // A zoom that is not a number is the text as it comes.
        assert_eq!(measure(ctx, f32::NAN, 4000.0), MEASURE_CH * zero);
    });
}

/// SCN-MEASURE-001: a long paragraph in a wide pane wraps at the measure, from the left edge.
#[test]
fn a_long_paragraph_wraps_at_the_measure_in_a_wide_pane() {
    in_frame(false, |ctx| {
        let measure = body_measure(ctx);
        let text = long_text();
        let plain = Message { body: mailcraft_model::Body { text: text.clone(), html: None }, ..html_message(0, sender(), "Plain", "") };
        let quoted = format!("<p>Before.</p><blockquote><p>{text}</p></blockquote>");
        let cases = [
            ("html", html_message(0, sender(), "Html", &format!("<p>{text}</p>"))),
            ("plain", plain),
            ("quoted", html_message(0, sender(), "Quoted", &quoted)),
            ("list", html_message(0, sender(), "List", &format!("<ul><li>{text}</li></ul>"))),
        ];
        for (what, m) in cases {
            for width in [900.0, 1200.0, 1400.0] {
                let laid = lay_out(ctx, &m, width);
                let pieces = texts_with(&laid, "harbour committee");
                assert!(!pieces.is_empty(), "{what} at {width}");
                for item in pieces {
                    let ends = row_ends(ctx, item);
                    assert!(ends.len() > 3, "{what} at {width}: the paragraph wraps ({} rows)", ends.len());
                    // Each line, from where the text starts (a quote's indent and bar come first).
                    for end in ends {
                        let line = end - item.rect.min.x;
                        assert!(line <= measure + 0.5, "{what} at {width}: a line is {line} wide, past the measure {measure}");
                    }
                    // Its column starts where the pane's text does (after the quote's indent):
                    // left-aligned, not centred.
                    assert!(item.rect.min.x < 60.0, "{what} at {width}: {:?}", item.rect);
                    assert!(item.rect.width() <= measure + 0.5, "{what} at {width}: {:?}", item.rect);
                }
                // The message as a whole still has the pane's width.
                assert_eq!(laid.size.x, width, "{what}");
            }
        }
    });
}

/// SCN-MEASURE-001, negative control: what is not flowing text keeps the pane's width.
#[test]
fn neg_tables_pictures_and_text_a_sender_set_keep_the_whole_width() {
    in_frame(false, |ctx| {
        let measure = body_measure(ctx);
        let text = long_text();
        let width = 1400.0;
        // A data table wider than the measure is laid out at the width it asks for.
        let table = format!(
            "<table width=\"1100\" border=\"1\"><tr><td>Boat</td><td>Skipper</td><td>Notes</td></tr>\
             <tr><td>Kestrel</td><td>Dana</td><td>{text}</td></tr></table>"
        );
        let laid = lay_out(ctx, &html_message(0, sender(), "Table", &table), width);
        let right = laid.items.iter().map(|i| i.rect.max.x).fold(0.0f32, f32::max);
        assert!(right > measure + 300.0, "the table ends at {right}, the measure is {measure}");
        // Text in a cell has the cell's room, not the measure.
        let notes = texts_with(&laid, "harbour committee");
        assert!(notes.iter().any(|i| row_ends(ctx, i).into_iter().any(|end| end > measure + 100.0)), "{notes:?}");

        // A layout table across the pane (a newsletter's two columns) is as wide as it says: its
        // second column ends near the pane's right edge. (A table of one cell and no paint is
        // read as the text it holds, and that text wraps at the measure.)
        let columns = format!("<table width=\"100%\"><tr><td>{text}</td><td>{text}</td></tr></table>");
        let laid = lay_out(ctx, &html_message(0, sender(), "Columns", &columns), width);
        let ends: Vec<f32> = texts_with(&laid, "harbour committee").iter().flat_map(|i| row_ends(ctx, i)).collect();
        assert!(ends.iter().any(|end| *end > measure + 300.0), "{ends:?}");

        // A picture wider than the measure is shown at its own width.
        let mut m =
            html_message(0, sender(), "Picture", "<p>Above.</p><img src=\"cid:wide@harbour.example\" width=\"1000\" height=\"40\" alt=\"Map\">");
        m.attachments =
            vec![Attachment { content_id: "wide@harbour.example".into(), inline: true, ..Attachment::new("wide.png", "image/png", png(1000, 40)) }];
        let laid = lay_out(ctx, &m, width);
        let picture = laid.items.iter().find(|i| matches!(i.kind, ItemKind::Picture(_))).expect("the picture is shown");
        assert!(picture.rect.width() > measure + 300.0, "{:?}", picture.rect);

        // Text the sender centred keeps the room they centred it in.
        let laid = lay_out(ctx, &html_message(0, sender(), "Centred", &format!("<p align=\"center\">{text}</p>")), width);
        let centred = texts_with(&laid, "harbour committee");
        assert!(centred.iter().all(|i| (i.rect.width() - width).abs() < 0.5), "{centred:?}");
    });
}

/// SCN-MEASURE-001, negative control: the message's `@media` is tested against the pane's
/// width, not the measure.
#[test]
fn neg_media_queries_see_the_whole_width_of_the_pane() {
    let html = "<style>@media (max-width: 699px) { .wide { display: none } } @media (min-width: 700px) { .narrow { display: none } }</style>\
                <p class=\"wide\">Shown in a wide pane</p><p class=\"narrow\">Shown in a narrow pane</p>";
    let m = html_message(0, sender(), "Responsive", html);
    let (app, ids) = app_with(vec![m]);
    let mut harness = window(app, (1440.0, 900.0));
    select(&mut harness, ids[0]);
    harness.run_steps(4);
    let (width, measure) =
        super::view::kept(&harness.ctx, ids[0], |laid, _| laid.width()).map(|w| (w, body_measure(&harness.ctx))).expect("the message is laid out");
    // (The case this is about: a pane wider than the query, and a measure narrower than it.)
    assert!(width >= 700.0 && measure < 700.0, "pane {width}, measure {measure}");
    let shown =
        super::view::kept(&harness.ctx, ids[0], |laid, _| laid.items.iter().map(|i| text_of(i).to_string()).collect::<Vec<_>>()).unwrap_or_default();
    assert!(shown.iter().any(|t| t.contains("Shown in a wide pane")), "{shown:?}");
    assert!(!shown.iter().any(|t| t.contains("Shown in a narrow pane")), "{shown:?}");
}

/// A copy of a paragraph is the same whether it was wrapped at the measure or at the pane.
#[test]
fn a_copy_is_the_same_text_whatever_the_measure() {
    in_frame(false, |ctx| {
        let text = long_text();
        let m = html_message(0, sender(), "Copy", &format!("<p>{text}</p><blockquote><p>{text}</p></blockquote>"));
        let measured = copy_of(&lay_out(ctx, &m, 1400.0));
        // Narrower than the measure: the pane's own width wraps it.
        let narrow = copy_of(&lay_out(ctx, &m, 360.0));
        assert_eq!(measured, narrow);
        assert!(measured.contains(&text), "{measured}");
    });
}

/// The header and the attachment strip stand in the body's column.
#[test]
fn the_header_and_attachments_stand_in_the_measure() {
    let mut m = html_message(
        0,
        sender(),
        "The order of the boats for the summer regatta, with the moorings and the times of the tides",
        &format!("<p>{}</p>", long_text()),
    );
    m.attachments = (0..12).map(|n| Attachment::new(format!("mooring-plan-{n}.pdf"), "application/pdf", vec![b'%'; 64])).collect();
    let (app, ids) = app_with(vec![m]);
    let mut harness = window(app, (1440.0, 900.0));
    select(&mut harness, ids[0]);
    harness.run_steps(4);
    let measure = body_measure(&harness.ctx);
    let origin = super::view::kept(&harness.ctx, ids[0], |_, origin| origin).expect("the body is shown");
    let right = origin.x + measure + 0.5;
    let date = harness.get_by_label_contains("2026").rect();
    assert!(date.max.x <= right, "the date ends at {} past {right}", date.max.x);
    for n in 0..12 {
        let chip = harness.state().hit.rect("attachmentMenu", super::attachment_key(ids[0], n)).expect("the attachment is shown");
        assert!(chip.max.x <= right, "attachment {n} ends at {}, past {right}", chip.max.x);
    }
}

/// The sender's address starts 8 points after the name.
#[test]
fn the_address_stands_eight_points_after_the_name() {
    assert_eq!(super::SENDER_ADDRESS_GAP, 8.0);
    let email = "maya.okafor@harbour.example";
    let mut m = html_message(0, ("Maya Okafor", email), "Moorings", "<p>Hello.</p>");
    m.from = Address::new("Maya Okafor", email);
    let (app, ids) = app_with(vec![m]);
    let mut harness = window(app, (1440.0, 900.0));
    select(&mut harness, ids[0]);
    let name = harness.get_by_label("Maya Okafor").rect();
    let address = harness.get_by_label("<maya.okafor@harbour.example>").rect();
    assert!((address.min.y - name.min.y).abs() < 4.0, "on one line: {name:?} {address:?}");
    assert!((address.min.x - name.max.x - 8.0).abs() < 0.5, "{name:?} {address:?}");
}

/// The sender's name is in a semibold face wherever the program runs: the system's when it has
/// one, else the bundled Inter SemiBold, and never a regular face standing in for it.
#[test]
fn the_sender_name_is_set_in_a_semibold_face() {
    for (what, system) in [("this system", SystemFont::load()), ("no system font", SystemFont::none())] {
        let fonts = font_definitions_with(&system);
        let chain = fonts.families.get(&FontFamily::Name("semibold".into())).cloned().unwrap_or_default();
        let first = chain.first().cloned().unwrap_or_default();
        eprintln!("{what}: semibold resolves to {first}");
        assert!(first == "SystemSemibold" || first == "InterSemiBold", "{what}: {chain:?}");
        assert!(fonts.font_data.contains_key(&first), "{what}: {first} is not loaded");
    }
    in_frame(false, |ctx| {
        let width = |font| ctx.fonts_mut(|f| f.layout_no_wrap("Harbour Committee".into(), font, Color32::WHITE).size().x);
        let (heavy, plain) = (width(semibold(13.0)), width(regular(13.0)));
        assert!(heavy > plain, "the semibold face draws as the regular one: {heavy} and {plain}");
    });
}

/// The reading pane meets the 11 pt type floor at 100 % and at 200 %: the sender header (the
/// External tag and the attachments' sizes with it) and text at the body's base size.
#[test]
fn the_reading_pane_meets_the_type_floor() {
    use crate::chrome::type_floor_tests::{assert_type_floor, text_count};
    for percent in [100.0, 200.0] {
        let email = "newsletter@harbour-and-moorings.example";
        let mut m = super::tests::web_message(0, email, "The order of the boats", &[]);
        m.from = Address::new("Harbour Committee", email);
        m.body = mailcraft_model::Body { text: long_text(), html: None };
        m.attachments = (0..3).map(|n| Attachment::new(format!("mooring-plan-{n}.pdf"), "application/pdf", vec![b'%'; 2048])).collect();
        let (mut app, ids) = app_with(vec![m]);
        app.ui.interface_size = crate::theme::InterfaceSize::from_percent(percent).unwrap_or_default();
        let mut harness = window(app, (1440.0, 900.0));
        select(&mut harness, ids[0]);
        harness.run_steps(4);
        assert!(harness.state().hit.rect("externalTag", ids[0]).is_some(), "at {percent} %: the External tag is shown");
        let pane = harness.state().hit.rect("readingPane", 0).expect("the reading pane is shown");
        assert!(text_count(harness.output(), pane) > 5, "at {percent} %: the pane's text is seen");
        assert_type_floor(&harness, pane, 11.0);
    }
}
