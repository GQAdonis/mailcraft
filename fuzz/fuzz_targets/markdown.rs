// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A model's markdown as display blocks, from any bytes: no panic, no picture or table, only
//! links the mail checks accept, and a bounded number of blocks.
#![no_main]

use mailcraft_html::{BlockKind, MAX_MARKDOWN_BLOCKS, MAX_MARKDOWN_INPUT};

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let blocks = mailcraft_html::markdown_blocks(&text);
    assert!(blocks.len() <= MAX_MARKDOWN_BLOCKS);
    let mut held = 0usize;
    for block in &blocks {
        assert!(!matches!(block.kind, BlockKind::Image { .. } | BlockKind::Table(_)));
        for span in &block.spans {
            held += span.text.len();
            if let Some(link) = span.link.as_deref() {
                assert!(mailcraft_html::is_safe_url(link) && !link.get(..4).is_some_and(|s| s.eq_ignore_ascii_case("cid:")));
            }
        }
    }
    assert!(held <= 4 * MAX_MARKDOWN_INPUT, "{held} bytes of text from {} of input", text.len());
});
