// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Markdown written by a model as display blocks.
//!
//! The text of an answer is untrusted: it may carry raw HTML, pictures, scripts and links to
//! anywhere. [`markdown_blocks`] reads it as plain CommonMark (no extensions but strikethrough)
//! and keeps paragraphs, emphasis, lists, quotes, code, rules and headings, all as the same
//! [`Block`]s the reading pane draws for mail. Raw HTML, block and inline, is dropped whole; a
//! picture is dropped and only its alternative text stays, as plain text; a link stays a link
//! only when its address passes the same checks as a link in mail and its scheme is `http`,
//! `https`, `mailto` or `tel`, and is plain text otherwise. Nothing is fetched or run here.
//!
//! A heading is bold text in a paragraph of its own, at the reading pane's size. The input is cut
//! at [`MAX_MARKDOWN_INPUT`], and events, blocks, spans per block, list depth and quote depth are
//! capped; the work is linear in what is read.

use std::sync::Arc;

use mailcraft_link::WebLink;
use pulldown_cmark::{Event, LinkType, Options, Parser, Tag, TagEnd};

use crate::{Block, BlockKind, Marker, Span, is_format, is_safe_url, truncate};

/// Longest markdown read, in bytes; the rest is ignored.
pub const MAX_MARKDOWN_INPUT: usize = 128 * 1024;

/// Most parser events read; what follows is ignored.
pub const MAX_MARKDOWN_EVENTS: usize = 50_000;

/// Most blocks an answer gives.
pub const MAX_MARKDOWN_BLOCKS: usize = 2_000;

/// Most spans one block keeps.
const MAX_SPANS: usize = 1_000;

/// Deepest list indent kept; deeper lists are drawn at this depth.
const MAX_LIST_DEPTH: usize = 8;

/// Deepest quote kept; deeper quotes are drawn at this depth.
const MAX_QUOTE_DEPTH: u32 = 8;

/// Longest link address kept; a longer one is plain text.
const MAX_LINK_LEN: usize = 2048;

/// Most bytes of text one answer gives.
const MAX_TEXT: usize = 256 * 1024;

/// Markdown from a model as display blocks. Any input, never panics, bounded (see the module).
pub fn markdown_blocks(markdown: &str) -> Vec<Block> {
    let text = truncate(markdown, MAX_MARKDOWN_INPUT);
    let mut build = Build::default();
    for event in Parser::new_ext(text, Options::ENABLE_STRIKETHROUGH).take(MAX_MARKDOWN_EVENTS) {
        if build.blocks.len() >= MAX_MARKDOWN_BLOCKS {
            break;
        }
        build.event(event);
    }
    build.flush();
    build.blocks
}

/// What is being built: the block in progress and the state the events open and close.
#[derive(Default)]
struct Build {
    blocks: Vec<Block>,
    /// The kind of the block in progress, if one is open.
    open: Option<BlockKind>,
    /// Whether the block in progress is a heading (bold).
    heading: bool,
    spans: Vec<Span>,
    bold: u32,
    italic: u32,
    strike: u32,
    /// Inside a raw HTML block: whatever comes is dropped.
    html: bool,
    /// Inside a picture: its alternative text stays, its links do not.
    image: u32,
    link: Option<Arc<str>>,
    quote: u32,
    lists: Vec<Option<u64>>,
    /// Lists nested beyond [`MAX_LIST_DEPTH`], to match their ends.
    overflow: u32,
    /// Items open, innermost last: whether each still waits for its marker.
    items: Vec<bool>,
    markers: Vec<Marker>,
    text: usize,
}

impl Build {
    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(end) => self.end(end),
            Event::Text(text) => self.text(&text, false),
            Event::Code(text) => self.text(&text, true),
            Event::SoftBreak => self.text(" ", false),
            Event::HardBreak => self.text("\n", false),
            Event::Rule => {
                self.flush();
                self.push(BlockKind::Rule, Vec::new());
            }
            // Raw HTML, inline or block, is dropped; so is anything else not asked for.
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                self.flush();
                self.ensure();
            }
            Tag::Heading { .. } => {
                self.flush();
                self.open = Some(BlockKind::Paragraph);
                self.heading = true;
            }
            Tag::BlockQuote(_) => {
                self.flush();
                self.quote = self.quote.saturating_add(1);
            }
            Tag::CodeBlock(_) => {
                self.flush();
                self.open = Some(BlockKind::Preformatted);
            }
            Tag::HtmlBlock => {
                self.flush();
                self.html = true;
            }
            Tag::List(first) => {
                self.flush();
                if self.lists.len() < MAX_LIST_DEPTH {
                    self.lists.push(first);
                } else {
                    self.overflow = self.overflow.saturating_add(1);
                }
            }
            Tag::Item => {
                self.flush();
                let mut marker = Marker::Bullet;
                if let Some(Some(n)) = self.lists.last_mut() {
                    marker = Marker::Number(u32::try_from(*n).unwrap_or(u32::MAX));
                    *n = n.saturating_add(1);
                }
                self.items.push(true);
                self.markers.push(marker);
            }
            Tag::Emphasis => self.italic = self.italic.saturating_add(1),
            Tag::Strong => self.bold = self.bold.saturating_add(1),
            Tag::Strikethrough => self.strike = self.strike.saturating_add(1),
            Tag::Link { link_type, dest_url, .. } => {
                if self.image == 0 {
                    self.link = link_target(link_type, &dest_url);
                }
            }
            Tag::Image { .. } => self.image = self.image.saturating_add(1),
            _ => {}
        }
    }

    fn end(&mut self, end: TagEnd) {
        match end {
            TagEnd::Paragraph | TagEnd::CodeBlock => self.flush(),
            TagEnd::Heading(_) => {
                self.flush();
                self.heading = false;
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.quote = self.quote.saturating_sub(1);
            }
            TagEnd::HtmlBlock => self.html = false,
            TagEnd::List(_) => {
                self.flush();
                if self.overflow > 0 {
                    self.overflow = self.overflow.saturating_sub(1);
                } else {
                    self.lists.pop();
                }
            }
            TagEnd::Item => {
                self.flush();
                // An item with nothing in it still shows its marker.
                if self.items.pop() == Some(true) {
                    let marker = self.markers.pop().unwrap_or(Marker::Bullet);
                    self.push(BlockKind::ListItem { marker, depth: self.depth() }, Vec::new());
                } else {
                    self.markers.pop();
                }
            }
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link => {
                self.link = None;
            }
            TagEnd::Image => self.image = self.image.saturating_sub(1),
            _ => {}
        }
    }

    /// The list depth of the block being made: 0 for a top-level list.
    fn depth(&self) -> u8 {
        let lists = self.lists.len().saturating_add(self.overflow as usize);
        u8::try_from(lists.saturating_sub(1).min(MAX_LIST_DEPTH)).unwrap_or(0)
    }

    /// Open the block text goes in, if none is: an item's first line takes its marker, a further
    /// paragraph of an item is indented under it, anything else is a paragraph.
    fn ensure(&mut self) {
        if self.open.is_some() {
            return;
        }
        let kind = match self.items.last().copied() {
            Some(true) => {
                if let Some(waiting) = self.items.last_mut() {
                    *waiting = false;
                }
                let marker = self.markers.last().cloned().unwrap_or(Marker::Bullet);
                BlockKind::ListItem { marker, depth: self.depth() }
            }
            Some(false) => BlockKind::ListItem { marker: Marker::None, depth: self.depth() },
            None => BlockKind::Paragraph,
        };
        self.open = Some(kind);
    }

    fn text(&mut self, text: &str, code: bool) {
        if self.html || self.text >= MAX_TEXT {
            return;
        }
        self.ensure();
        let preformatted = matches!(self.open, Some(BlockKind::Preformatted));
        let clean: String = text.chars().filter(|c| *c == '\n' || *c == '\t' || !(c.is_control() || is_format(*c))).collect();
        if clean.is_empty() {
            return;
        }
        self.text = self.text.saturating_add(clean.len());
        let span = Span {
            text: clean,
            bold: self.bold > 0 || self.heading,
            italic: self.italic > 0,
            strike: self.strike > 0,
            code: code || preformatted,
            link: self.link.clone(),
            ..Span::default()
        };
        let room = self.spans.len() < MAX_SPANS;
        match self.spans.last_mut() {
            Some(last) if last.same_format(&span) => last.text.push_str(&span.text),
            _ if room => self.spans.push(span),
            _ => {}
        }
    }

    /// End the block in progress, if it has any text.
    fn flush(&mut self) {
        let Some(kind) = self.open.take() else { return };
        let mut spans = std::mem::take(&mut self.spans);
        if matches!(kind, BlockKind::Preformatted)
            && let Some(last) = spans.last_mut()
        {
            let kept = last.text.trim_end_matches('\n').len();
            last.text.truncate(kept);
        }
        if spans.iter().all(|s| s.text.is_empty()) && !matches!(kind, BlockKind::Preformatted) {
            return;
        }
        self.push(kind, spans);
    }

    fn push(&mut self, kind: BlockKind, spans: Vec<Span>) {
        if self.blocks.len() >= MAX_MARKDOWN_BLOCKS {
            return;
        }
        let follows_item =
            matches!(kind, BlockKind::ListItem { .. }) && matches!(self.blocks.last().map(|b| &b.kind), Some(BlockKind::ListItem { .. }));
        let space = u8::from(!self.blocks.is_empty() && !follows_item && !matches!(kind, BlockKind::ListItem { marker: Marker::None, .. }));
        let quote = u8::try_from(self.quote.min(MAX_QUOTE_DEPTH)).unwrap_or(0);
        self.blocks.push(Block { quote, space, ..Block::new(kind, spans) });
    }
}

/// Where a link leads, when it may be followed: an `http`, `https`, `mailto` or `tel` address the
/// checks for mail accept, written the way the link crate writes it. `None` is plain text.
fn link_target(link_type: LinkType, dest: &str) -> Option<Arc<str>> {
    let dest = dest.trim_matches(' ');
    if dest.is_empty() || dest.len() > MAX_LINK_LEN {
        return None;
    }
    let owned;
    let dest = if matches!(link_type, LinkType::Email) {
        owned = format!("mailto:{dest}");
        owned.as_str()
    } else {
        dest
    };
    let (scheme, _) = dest.split_once(':')?;
    if !["http", "https", "mailto", "tel"].iter().any(|s| scheme.eq_ignore_ascii_case(s)) || !is_safe_url(dest) {
        return None;
    }
    if scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https") {
        return WebLink::parse(dest).map(|link| Arc::from(link.as_str()));
    }
    Some(Arc::from(dest))
}
