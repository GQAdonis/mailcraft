// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Model markdown becomes display blocks: no HTML, nothing fetchable, links only where mail's
//! links may go, and every size bounded.

use super::{Block, BlockKind, MAX_MARKDOWN_BLOCKS, MAX_MARKDOWN_INPUT, Marker, markdown_blocks};

fn links(blocks: &[Block]) -> Vec<String> {
    blocks.iter().flat_map(|b| &b.spans).filter_map(|s| s.link.as_deref().map(str::to_string)).collect()
}

fn text(blocks: &[Block]) -> String {
    blocks.iter().map(Block::text).collect::<Vec<_>>().join("\n")
}

#[test]
fn raw_html_is_dropped_block_and_inline() {
    let blocks = markdown_blocks(
        "<script>alert(1)</script>\n\nhello <b>bold</b> <iframe src=\"http://x.example/\"></iframe> world\n\n<div onclick=\"x()\">\nin a block\n</div>",
    );
    let all = text(&blocks);
    assert!(!all.contains('<') && !all.contains("script") && !all.contains("iframe") && !all.contains("onclick"), "{all:?}");
    assert!(all.contains("hello") && all.contains("world"), "{all:?}");
    assert!(links(&blocks).is_empty());
}

#[test]
fn pictures_are_dropped_and_nothing_fetchable_is_left() {
    let blocks =
        markdown_blocks("![tracker](http://tracker.example/p.gif) and <img src=\"https://tracker.example/q.png\"> ![](https://t.example/r.png)");
    assert!(blocks.iter().all(|b| !matches!(b.kind, BlockKind::Image { .. })));
    assert!(links(&blocks).is_empty());
    let all = text(&blocks);
    assert!(!all.contains("tracker.example") && !all.contains("http"), "{all:?}");
    assert!(all.contains("tracker"), "the alt text stays: {all:?}");
}

#[test]
fn a_linked_picture_keeps_its_alt_text_and_its_checked_link_only() {
    let blocks = markdown_blocks("[![logo](http://img.example/l.png)](https://example.org/home)");
    assert_eq!(text(&blocks), "logo");
    assert_eq!(links(&blocks), ["https://example.org/home"]);
}

#[test]
fn links_to_other_schemes_become_plain_text() {
    for dest in [
        "javascript:alert(1)",
        "JaVaScRiPt:alert(1)",
        "&#106;avascript:alert(1)",
        "data:text/html,x",
        "file:///etc/passwd",
        "ms-settings:network",
        "cid:part1",
        "//example.org/x",
        "/relative",
        "http://",
        "https://",
    ] {
        let blocks = markdown_blocks(&format!("see [here]({dest}) now"));
        assert!(links(&blocks).is_empty(), "{dest:?}: {:?}", links(&blocks));
        assert!(text(&blocks).contains("here"), "{dest:?}");
    }
    let blocks = markdown_blocks("[x](<javascript:alert(1)>) [y](<https://exa mple.org/>)");
    assert!(links(&blocks).is_empty());
}

#[test]
fn allowed_links_stay_links() {
    let blocks = markdown_blocks(
        "[a](https://example.org/x?y=1) [b](HTTP://EXAMPLE.org/z) [c](mailto:ada@example.org) [d](tel:+15550100) <https://example.org/auto> <ada@example.org>",
    );
    assert_eq!(
        links(&blocks),
        [
            "https://example.org/x?y=1",
            "http://example.org/z",
            "mailto:ada@example.org",
            "tel:+15550100",
            "https://example.org/auto",
            "mailto:ada@example.org"
        ]
    );
    assert!(links(&blocks).iter().all(|l| super::is_safe_url(l)));
}

#[test]
fn format_and_control_characters_are_removed_from_text() {
    let all = text(&markdown_blocks("a\u{202e}b\u{200b}c\u{0007}d"));
    assert_eq!(all, "abcd");
}

#[test]
fn structure_is_kept() {
    let blocks = markdown_blocks(
        "# Title\n\nSome *em* and **strong** and `code` and ~~gone~~.\n\n- one\n- two\n  1. deep\n\n3. three\n\n> quoted\n\n    indented code\n\n---\n",
    );
    let first = &blocks[0];
    assert!(matches!(first.kind, BlockKind::Paragraph) && first.spans.iter().all(|s| s.bold), "heading is bold text: {first:?}");
    let para = &blocks[1];
    assert!(para.spans.iter().any(|s| s.italic && s.text == "em"));
    assert!(para.spans.iter().any(|s| s.bold && s.text == "strong"));
    assert!(para.spans.iter().any(|s| s.code && s.text == "code"));
    assert!(para.spans.iter().any(|s| s.strike && s.text == "gone"));
    let items: Vec<_> = blocks
        .iter()
        .filter_map(|b| if let BlockKind::ListItem { marker, depth } = &b.kind { Some((marker.clone(), *depth, b.text())) } else { None })
        .collect();
    assert_eq!(items[0], (Marker::Bullet, 0, "one".to_string()));
    assert_eq!(items[1], (Marker::Bullet, 0, "two".to_string()));
    assert_eq!(items[2], (Marker::Number(1), 1, "deep".to_string()));
    assert_eq!(items[3], (Marker::Number(3), 0, "three".to_string()));
    assert!(blocks.iter().any(|b| b.quote == 1 && b.text() == "quoted"));
    assert!(blocks.iter().any(|b| matches!(b.kind, BlockKind::Preformatted) && b.text() == "indented code"));
    assert!(blocks.iter().any(|b| matches!(b.kind, BlockKind::Rule)));
}

#[test]
fn deep_nesting_is_bounded() {
    for source in [
        ">".repeat(5_000) + " x",
        "- ".repeat(5_000) + "x",
        "*".repeat(50_000) + "x" + &"*".repeat(50_000),
        "[".repeat(20_000) + &"]".repeat(20_000),
        "![".repeat(20_000) + "x",
    ] {
        let blocks = markdown_blocks(&source);
        assert!(blocks.len() <= MAX_MARKDOWN_BLOCKS);
        assert!(blocks.iter().all(|b| b.quote <= 8));
        assert!(blocks.iter().all(|b| match b.kind {
            BlockKind::ListItem { depth, .. } => depth <= 8,
            _ => true,
        }));
    }
}

#[test]
fn huge_input_is_capped() {
    let big = "word ".repeat(MAX_MARKDOWN_INPUT);
    let held: usize = markdown_blocks(&big).iter().map(|b| b.text().len()).sum();
    assert!(held <= MAX_MARKDOWN_INPUT, "{held}");
    let many = "a\n\n".repeat(MAX_MARKDOWN_INPUT);
    assert!(markdown_blocks(&many).len() <= MAX_MARKDOWN_BLOCKS);
    let wikilinks = "[[a|]]".repeat(100_000) + &"![[".repeat(100_000);
    let _ = markdown_blocks(&wikilinks);
}

#[test]
fn input_is_cut_on_a_character_boundary() {
    let source = "é".repeat(MAX_MARKDOWN_INPUT);
    let blocks = markdown_blocks(&source);
    assert!(!blocks.is_empty());
}

#[test]
fn empty_and_odd_input_gives_no_panic() {
    assert!(markdown_blocks("").is_empty());
    for s in [
        "\0",
        "\r\n\r\n",
        "```",
        "```\ncode",
        "- \n-\n",
        "1. \n",
        "|a|b|\n|-|-|\n|1|2|",
        "[^1]: x\n[^1]",
        "$x$ $$y$$",
        "<",
        "&",
        "[a]: javascript:alert(1)\n[a]",
    ] {
        let blocks = markdown_blocks(s);
        assert!(links(&blocks).is_empty(), "{s:?}");
    }
}

proptest::proptest! {
    #[test]
    fn any_text_gives_only_safe_links(s in "[ -~\n\t<>\\[\\]()!*_`#>&;:/.a-c-]{0,300}") {
        for block in markdown_blocks(&s) {
            assert!(!matches!(block.kind, BlockKind::Image { .. } | BlockKind::Table(_)));
            for span in &block.spans {
                assert!(span.link.as_deref().is_none_or(|l| super::is_safe_url(l) && !l.to_ascii_lowercase().starts_with("cid:")));
            }
        }
    }
}
