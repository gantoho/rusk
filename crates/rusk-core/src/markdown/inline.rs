//! Inline event collection.
//!
//! Inline runs are taken directly from pulldown-cmark's event stream, so their
//! ranges are exact even inside block quotes, where continuation lines carry
//! `> ` prefixes that are not part of the paragraph text.

use std::ops::Range;

use pulldown_cmark::{Event, Tag, TagEnd};

use super::blocks::{InlineMarker, InlineStyle, InlineToken, MarkerKind, TokenKind};

#[derive(Clone, Debug)]
struct Frame {
    kind: MarkerKind,
    range: Range<usize>,
    first: Option<usize>,
    last: Option<usize>,
    url: Option<String>,
}

#[derive(Default, Debug)]
pub struct InlineCollector {
    frames: Vec<Frame>,
    pub tokens: Vec<InlineToken>,
    pub markers: Vec<InlineMarker>,
    pub plain_text: String,
    pub image: Option<(Range<usize>, String, String)>,
}

impl InlineCollector {
    #[must_use]
    pub fn style(&self) -> InlineStyle {
        let mut style = InlineStyle::default();
        for frame in &self.frames {
            match frame.kind {
                MarkerKind::Strong => style.bold = true,
                MarkerKind::Emphasis => style.italic = true,
                MarkerKind::Strikethrough => style.strikethrough = true,
                MarkerKind::Link => style.link = true,
                MarkerKind::Image => style.image = true,
                MarkerKind::Code => style.code = true,
                MarkerKind::Math => style.math = true,
            }
        }
        style
    }

    fn url(&self) -> Option<String> {
        self.frames.iter().rev().find_map(|frame| frame.url.clone())
    }

    fn note_token(&mut self, range: &Range<usize>) {
        for frame in &mut self.frames {
            frame.first.get_or_insert(range.start);
            frame.last = Some(range.end);
        }
    }

    fn push_token(&mut self, token: InlineToken) {
        self.note_token(&token.source);
        self.plain_text.push_str(&token.text);
        self.tokens.push(token);
    }

    pub fn start(&mut self, tag: &Tag<'_>, range: Range<usize>) {
        let (kind, url) = match tag {
            Tag::Strong => (MarkerKind::Strong, None),
            Tag::Emphasis => (MarkerKind::Emphasis, None),
            Tag::Strikethrough => (MarkerKind::Strikethrough, None),
            Tag::Link { dest_url, .. } => (MarkerKind::Link, Some(dest_url.to_string())),
            Tag::Image { dest_url, .. } => (MarkerKind::Image, Some(dest_url.to_string())),
            _ => return,
        };
        if kind == MarkerKind::Image {
            self.image = Some((
                range.clone(),
                String::new(),
                url.clone().unwrap_or_default(),
            ));
        }
        self.frames.push(Frame {
            kind,
            range,
            first: None,
            last: None,
            url,
        });
    }

    pub fn end(&mut self, end: TagEnd) {
        let kind = match end {
            TagEnd::Strong => MarkerKind::Strong,
            TagEnd::Emphasis => MarkerKind::Emphasis,
            TagEnd::Strikethrough => MarkerKind::Strikethrough,
            TagEnd::Link => MarkerKind::Link,
            TagEnd::Image => MarkerKind::Image,
            _ => return,
        };
        let Some(index) = self.frames.iter().rposition(|frame| frame.kind == kind) else {
            return;
        };
        let frame = self.frames.remove(index);
        if let (Some(first), Some(last)) = (frame.first, frame.last) {
            if frame.range.start < first {
                self.markers.push(InlineMarker {
                    range: frame.range.start..first,
                    outer: frame.range.clone(),
                    kind,
                });
            }
            if last < frame.range.end {
                self.markers.push(InlineMarker {
                    range: last..frame.range.end,
                    outer: frame.range.clone(),
                    kind,
                });
            }
            if kind == MarkerKind::Image
                && let Some((range, alt, url)) = self.image.take()
            {
                let alt_text = self
                    .tokens
                    .iter()
                    .filter(|t| range.start <= t.source.start && t.source.end <= range.end)
                    .map(|t| t.text.as_str())
                    .collect::<String>();
                self.image = Some((range, alt_text, url));
                let _ = alt;
            }
        }
    }

    pub fn text(&mut self, text: &str, range: Range<usize>, atomic: bool) {
        let style = self.style();
        let url = self.url();
        self.push_token(InlineToken {
            text: text.to_owned(),
            source: range,
            style,
            kind: TokenKind::Text,
            url,
            atomic,
        });
    }

    pub fn code(&mut self, text: &str, range: Range<usize>, source: &str) {
        let raw = &source[range.clone()];
        let leading = raw.chars().take_while(|c| *c == '`').count();
        let trailing = raw.chars().rev().take_while(|c| *c == '`').count();
        if leading == 0 || trailing == 0 || leading + trailing > raw.len() {
            return;
        }
        let inner_start = range.start + leading;
        let inner_end = range.end.saturating_sub(trailing);
        if inner_start > inner_end {
            return;
        }
        let inner = &source[inner_start..inner_end];
        let display = text.to_owned();
        let marker_style = self.style();
        let _ = marker_style;
        self.push_token(InlineToken {
            text: display.clone(),
            source: inner_start..inner_end,
            style: InlineStyle {
                code: true,
                ..self.style()
            },
            kind: TokenKind::Code,
            url: self.url(),
            atomic: inner.trim() != display || inner != display,
        });
        if range.start < inner_start {
            self.markers.push(InlineMarker {
                range: range.start..inner_start,
                outer: range.clone(),
                kind: MarkerKind::Code,
            });
        }
        if inner_end < range.end {
            self.markers.push(InlineMarker {
                range: inner_end..range.end,
                outer: range.clone(),
                kind: MarkerKind::Code,
            });
        }
    }

    pub fn soft_break(&mut self, range: Range<usize>) {
        let style = self.style();
        self.push_token(InlineToken {
            text: " ".to_owned(),
            source: range,
            style,
            kind: TokenKind::Text,
            url: self.url(),
            atomic: true,
        });
    }

    pub fn hard_break(&mut self, range: Range<usize>) {
        let style = self.style();
        self.push_token(InlineToken {
            text: "\n".to_owned(),
            source: range,
            style,
            kind: TokenKind::Text,
            url: self.url(),
            atomic: true,
        });
    }

    pub fn math(&mut self, text: &str, range: Range<usize>, source: &str, display_block: bool) {
        let raw = &source[range.clone()];
        let delimiter = if display_block { 2 } else { 1 };
        let inner_start = range.start + delimiter;
        let inner_end = range.end.saturating_sub(delimiter);
        if inner_start > inner_end || !raw.starts_with('$') {
            return;
        }
        self.push_token(InlineToken {
            text: text.to_owned(),
            source: inner_start..inner_end,
            style: InlineStyle {
                math: true,
                ..self.style()
            },
            kind: TokenKind::Math,
            url: None,
            atomic: true,
        });
        if range.start < inner_start {
            self.markers.push(InlineMarker {
                range: range.start..inner_start,
                outer: range.clone(),
                kind: MarkerKind::Math,
            });
        }
        if inner_end < range.end {
            self.markers.push(InlineMarker {
                range: inner_end..range.end,
                outer: range.clone(),
                kind: MarkerKind::Math,
            });
        }
    }

    pub fn footnote(&mut self, label: &str, range: Range<usize>) {
        self.push_token(InlineToken {
            text: format!("[^{label}]"),
            source: range,
            style: InlineStyle {
                footnote: true,
                ..self.style()
            },
            kind: TokenKind::Footnote,
            url: None,
            atomic: true,
        });
    }

    pub fn html(&mut self, text: &str, range: Range<usize>) {
        self.push_token(InlineToken {
            text: text.to_owned(),
            source: range,
            style: InlineStyle {
                code: true,
                ..self.style()
            },
            kind: TokenKind::Html,
            url: None,
            atomic: true,
        });
    }

    /// Images are represented by their alt text but keep the complete image
    /// syntax as markers, so the image can be revealed and edited.
    pub fn finish_image_alt(&mut self) {
        if let Some((range, _alt, _url)) = &self.image {
            let alt = self
                .tokens
                .iter()
                .filter(|t| range.start <= t.source.start && t.source.end <= range.end)
                .map(|t| t.text.as_str())
                .collect::<String>();
            if let Some((_, existing, _)) = &mut self.image {
                *existing = alt;
            }
        }
    }

    #[allow(clippy::type_complexity)]
    #[must_use]
    pub fn into_parts(
        mut self,
    ) -> (
        Vec<InlineToken>,
        Vec<InlineMarker>,
        Option<(Range<usize>, String, String)>,
    ) {
        self.finish_image_alt();
        (self.tokens, self.markers, self.image)
    }
}

/// Translate a pulldown event into collector calls. Returns true when the event
/// was an inline event (and the caller should not treat it as block-level).
pub fn handle_inline(
    collector: &mut InlineCollector,
    event: &Event<'_>,
    range: Range<usize>,
    source: &str,
) -> bool {
    match event {
        Event::Start(
            tag @ (Tag::Strong
            | Tag::Emphasis
            | Tag::Strikethrough
            | Tag::Link { .. }
            | Tag::Image { .. }),
        ) => {
            collector.start(tag, range);
            true
        }
        Event::End(
            end @ (TagEnd::Strong
            | TagEnd::Emphasis
            | TagEnd::Strikethrough
            | TagEnd::Link
            | TagEnd::Image),
        ) => {
            collector.end(*end);
            true
        }
        Event::Text(text) => {
            let atomic = &source[range.clone()] != text.as_ref();
            collector.text(text, range, atomic);
            true
        }
        Event::Code(text) => {
            collector.code(text, range, source);
            true
        }
        Event::SoftBreak => {
            collector.soft_break(range);
            true
        }
        Event::HardBreak => {
            collector.hard_break(range);
            true
        }
        Event::InlineMath(text) => {
            collector.math(text, range, source, false);
            true
        }
        Event::DisplayMath(text) => {
            collector.math(text, range, source, true);
            true
        }
        Event::FootnoteReference(label) => {
            collector.footnote(label, range);
            true
        }
        Event::InlineHtml(text) => {
            collector.html(text, range);
            true
        }
        _ => false,
    }
}
