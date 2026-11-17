// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An answer's text: source markers, links and their addresses (#ai).

use super::answer_text::{Piece, link_words, pieces, says_where, span_pieces};

#[test]
fn only_the_numbers_of_sources_shown_become_markers() {
    let got = pieces("Due Friday [1], see [2] and [9] or [x] [", &[1, 2]);
    assert_eq!(
        got,
        vec![
            Piece::Text("Due Friday ".into()),
            Piece::Source(1),
            Piece::Text(", see ".into()),
            Piece::Source(2),
            Piece::Text(" and [9] or [x] [".into()),
        ]
    );
    assert_eq!(pieces("", &[1]), Vec::<Piece>::new());
    assert_eq!(pieces("[1]", &[]), vec![Piece::Text("[1]".into())]);
    // Any text at all: no panic, nothing lost but the markers' brackets.
    for text in ["[[[1]]]", "[1", "]]", "é[1]é", "[0001]", "[99999999999999999999]"] {
        let kept: String = pieces(text, &[1])
            .into_iter()
            .map(|p| match p {
                Piece::Text(t) => t,
                Piece::Source(n) => format!("[{n}]"),
            })
            .collect();
        assert!(kept.len() <= text.len(), "{text}");
    }
}

/// A link a model wrote is never something to click: its words and its whole address are plain
/// text, so nothing of the mail can leave in an address the model made up.
#[test]
fn a_link_in_an_answer_is_plain_text_with_its_address() {
    let blocks = mailcraft_html::markdown_blocks("See [click](https://x.example/?q=secret) for more [1].");
    let shown: Vec<Piece> = blocks.iter().flat_map(|b| b.spans.iter()).flat_map(|s| span_pieces(s, &[1])).collect();
    let text: String = shown
        .iter()
        .filter_map(|p| match p {
            Piece::Text(t) => Some(t.as_str()),
            Piece::Source(_) => None,
        })
        .collect();
    assert!(text.contains("click (https://x.example/?q=secret)"), "{text}");
    // The one link left is the marker of source 1, which shows a message, not an address.
    assert_eq!(shown.iter().filter(|p| matches!(p, Piece::Source(_))).collect::<Vec<_>>(), vec![&Piece::Source(1)]);
    assert_eq!(link_words("example.org", "https://example.org/"), "example.org");
}

#[test]
fn words_that_say_the_address_need_no_address_after_them() {
    assert!(says_where("example.org", "https://example.org/"));
    assert!(says_where("ada@example.org", "mailto:ada@example.org"));
    assert!(!says_where("your bank", "https://attacker.example/login"));
}
