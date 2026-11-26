//! Projection: turn source ranges into a display string plus a segment map.
//!
//! This is the heart of the source-first WYSIWYG model. The document is never
//! changed by projection; hidden syntax bytes are simply omitted from the
//! display string, and every displayed character points back into the source.

use std::ops::Range;

use crate::markdown::{Block, InlineStyle, MarkerKind, PrefixKind, TokenKind};
use crate::selection::Selection;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentKind {
    Text,
    Syntax,
    Code,
    Math,
    Image,
}

#[derive(Clone, Debug)]
pub struct Segment {
    /// Character range in [`Projection::text`].
    pub display: Range<usize>,
    /// Byte range in [`Projection::text`].
    pub display_bytes: Range<usize>,
    /// Byte range in the canonical source.
    pub source: Range<usize>,
    pub style: InlineStyle,
    pub kind: SegmentKind,
    /// The displayed text is not a byte-for-byte copy of the source range.
    pub atomic: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Projection {
    pub text: String,
    pub segments: Vec<Segment>,
    /// Fallback source offset for an empty projection.
    pub empty_source: usize,
}

impl Projection {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Convert a source byte offset to a character offset in the projection.
    ///
    /// `prefer_next` controls which side of a hidden run a caret lands on.
    #[must_use]
    pub fn source_to_display(&self, source_byte: usize, prefer_next: bool) -> usize {
        if self.segments.is_empty() {
            return 0;
        }
        for seg in &self.segments {
            if seg.source.start <= source_byte && source_byte <= seg.source.end {
                if seg.atomic {
                    return if source_byte <= seg.source.start {
                        seg.display.start
                    } else {
                        seg.display.end
                    };
                }
                let delta = source_byte.saturating_sub(seg.source.start);
                let display_byte = seg.display_bytes.start + delta.min(seg.display_bytes.len());
                return self.text[..display_byte].chars().count();
            }
        }
        // Hidden gap or outside the block: pick the closest visible boundary.
        if let Some(next) = self.segments.iter().find(|s| source_byte < s.source.start) {
            if prefer_next {
                return next.display.start;
            }
            let previous = self
                .segments
                .iter()
                .take_while(|s| s.source.start < next.source.start)
                .last();
            return previous.map_or(next.display.start, |s| s.display.end);
        }
        self.segments.last().map_or(0, |s| s.display.end)
    }

    /// Convert a character offset in the projection back to a source byte.
    #[must_use]
    pub fn display_to_source(&self, display_char: usize) -> usize {
        if self.segments.is_empty() {
            return self.empty_source;
        }
        let display_char = display_char.min(self.text.chars().count());
        for (index, seg) in self.segments.iter().enumerate() {
            let is_last = index + 1 == self.segments.len();
            let contains = if is_last {
                seg.display.start <= display_char && display_char <= seg.display.end
            } else {
                seg.display.start <= display_char && display_char < seg.display.end
            };
            if contains {
                if seg.atomic {
                    return if display_char <= seg.display.start {
                        seg.source.start
                    } else {
                        seg.source.end
                    };
                }
                let display_byte = char_to_byte(&self.text, display_char);
                let delta = display_byte.saturating_sub(seg.display_bytes.start);
                return (seg.source.start + delta).min(seg.source.end);
            }
        }
        self.segments
            .last()
            .map_or(self.empty_source, |s| s.source.end)
    }

    /// Map a source range to a display character range.
    #[must_use]
    pub fn source_range_to_display(&self, range: &Range<usize>) -> Range<usize> {
        let start = self.source_to_display(range.start, true);
        let end = self.source_to_display(range.end, false);
        start.min(end)..start.max(end)
    }

    /// The visual span for a selection, restricted to this projection.
    #[must_use]
    pub fn selection_display(&self, selection: Selection) -> Option<Range<usize>> {
        let range = selection.range();
        if self.segments.is_empty() {
            return None;
        }
        let first = self.segments.first()?;
        let last = self.segments.last()?;
        if range.end < first.source.start || range.start > last.source.end {
            return None;
        }
        let start = self.source_to_display(range.start, true);
        let end = self.source_to_display(range.end, false);
        Some(start.min(end)..start.max(end))
    }
}

fn char_to_byte(text: &str, char_index: usize) -> usize {
    if char_index == 0 {
        return 0;
    }
    text.char_indices()
        .nth(char_index)
        .map_or(text.len(), |(byte, _)| byte)
}

/// Reveal syntax when the caret/selection is *inside* the construct. Callers
/// additionally reveal at both outer boundaries for inline constructs, where
/// a hidden marker still represents a real caret stop.
fn construct_revealed(reveal: Selection, outer: &Range<usize>, marker: &Range<usize>) -> bool {
    if marker.start < reveal.head && reveal.head < marker.end {
        return true;
    }
    if reveal.is_caret() {
        return outer.start < reveal.head && reveal.head < outer.end;
    }
    reveal.start() < outer.end && outer.start < reveal.end()
}

#[derive(Clone, Debug)]
struct Piece {
    source: Range<usize>,
    text: String,
    style: InlineStyle,
    kind: SegmentKind,
    atomic: bool,
}

/// Build the display projection for a block.
///
/// `reveal` is the current selection. Syntax markers whose construct touches
/// the caret/selection are shown in a dim syntax style; otherwise they are
/// omitted entirely.
#[must_use]
pub fn project(block: &Block, source: &str, reveal: Selection) -> Projection {
    let mut pieces: Vec<Piece> = Vec::new();

    for token in &block.tokens {
        if token.text.is_empty() {
            continue;
        }
        let kind = match token.kind {
            TokenKind::Code => SegmentKind::Code,
            TokenKind::Math => SegmentKind::Math,
            TokenKind::Image => SegmentKind::Image,
            TokenKind::Html => SegmentKind::Code,
            TokenKind::Footnote => SegmentKind::Text,
            TokenKind::Text => SegmentKind::Text,
        };
        pieces.push(Piece {
            source: token.source.clone(),
            text: token.text.clone(),
            style: token.style,
            kind,
            atomic: token.atomic,
        });
    }

    for marker in &block.markers {
        // Both edges of an inline construct are real caret positions even
        // though the marker bytes are hidden. Reveal there as well as inside,
        // otherwise the same invisible boundary behaves differently depending
        // on whether the caret arrived from the left or the right.
        let at_construct_boundary = reveal.is_caret()
            && (reveal.head == marker.outer.start || reveal.head == marker.outer.end);
        let touched =
            at_construct_boundary || construct_revealed(reveal, &marker.outer, &marker.range);
        if !touched {
            continue;
        }
        let raw = &source[marker.range.clone()];
        if raw.is_empty() {
            continue;
        }
        pieces.push(Piece {
            source: marker.range.clone(),
            text: raw.to_owned(),
            style: InlineStyle {
                syntax: true,
                ..InlineStyle::default()
            },
            kind: SegmentKind::Syntax,
            atomic: false,
        });
    }

    // Headings reveal their hashes while the caret is in the heading.
    let heading_touched = block.heading_level().is_some()
        && if reveal.is_caret() {
            block.content.start < reveal.head && reveal.head <= block.content.end
        } else {
            reveal.start() < block.content.end && block.content.start < reveal.end()
        };
    if heading_touched {
        for prefix in &block.prefix {
            if !matches!(prefix.kind, PrefixKind::Heading) {
                continue;
            }
            let raw = &source[prefix.range.clone()];
            if raw.is_empty() {
                continue;
            }
            pieces.push(Piece {
                source: prefix.range.clone(),
                text: raw.to_owned(),
                style: InlineStyle {
                    syntax: true,
                    ..InlineStyle::default()
                },
                kind: SegmentKind::Syntax,
                atomic: false,
            });
        }
    }

    pieces.sort_by_key(|p| {
        (
            p.source.start,
            p.source.end,
            if p.kind == SegmentKind::Syntax { 0 } else { 1 },
        )
    });

    let mut projection = Projection {
        empty_source: block.content.start,
        ..Projection::default()
    };
    let mut display_chars = 0usize;
    let mut display_bytes = 0usize;

    for piece in pieces {
        if piece.text.is_empty() {
            continue;
        }
        let char_count = piece.text.chars().count();
        let byte_count = piece.text.len();
        projection.text.push_str(&piece.text);
        projection.segments.push(Segment {
            display: display_chars..display_chars + char_count,
            display_bytes: display_bytes..display_bytes + byte_count,
            source: piece.source,
            style: piece.style,
            kind: piece.kind,
            atomic: piece.atomic,
        });
        display_chars += char_count;
        display_bytes += byte_count;
    }

    projection
}

/// The display text for a marker, useful for source-mode overlays.
#[must_use]
pub fn marker_text<'a>(source: &'a str, marker: &crate::markdown::InlineMarker) -> &'a str {
    &source[marker.range.clone()]
}

#[must_use]
pub fn marker_kind_name(kind: MarkerKind) -> &'static str {
    match kind {
        MarkerKind::Strong => "strong",
        MarkerKind::Emphasis => "emphasis",
        MarkerKind::Strikethrough => "strikethrough",
        MarkerKind::Code => "code",
        MarkerKind::Link => "link",
        MarkerKind::Image => "image",
        MarkerKind::Math => "math",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::parse;

    #[test]
    fn hidden_markers_are_omitted() {
        let md = "A **bold** word";
        let doc = parse(md);
        let p = project(&doc.blocks[0], md, Selection::caret(0));
        assert_eq!(p.text, "A bold word");
        assert_eq!(p.display_to_source(2), 4);
        assert_eq!(p.display_to_source(6), 10);
        assert_eq!(p.source_to_display(2, true), 2);
    }

    #[test]
    fn markers_reveal_near_caret() {
        let md = "A **bold** word";
        let doc = parse(md);
        let p = project(&doc.blocks[0], md, Selection::caret(5));
        assert_eq!(p.text, "A **bold** word");
    }

    #[test]
    fn links_hide_destination_when_the_caret_is_outside() {
        let md = "[Rusk](https://example.com) is here";
        let doc = parse(md);
        let p = project(&doc.blocks[0], md, Selection::caret(30));
        assert_eq!(p.text, "Rusk is here");
        assert_eq!(p.display_to_source(4), 27);
    }

    #[test]
    fn link_reveals_at_both_boundaries() {
        let md = "[Rusk](https://example.com) is here";
        let doc = parse(md);
        for head in [0, 27] {
            let p = project(&doc.blocks[0], md, Selection::caret(head));
            assert_eq!(p.text, md, "boundary {head}");
        }
    }

    #[test]
    fn italic_marker_reveals_at_its_opening_boundary() {
        let md = "A *italic* word";
        let doc = parse(md);
        let p = project(&doc.blocks[0], md, Selection::caret(2));
        assert_eq!(p.text, "A *italic* word");
    }

    #[test]
    fn italic_marker_reveals_at_its_closing_boundary() {
        let md = "A *italic* word";
        let doc = parse(md);
        let p = project(&doc.blocks[0], md, Selection::caret(10));
        assert_eq!(p.text, "A *italic* word");
    }

    #[test]
    fn italic_marker_reveals_inside_a_list_item() {
        let md = "- **Bold**, *italic*, ~~strikethrough~~, and `inline code` render in place.";
        let doc = parse(md);
        let head = md.find("italic").expect("italic") + 2;
        let p = project(&doc.blocks[0], md, Selection::caret(head));
        assert!(p.text.contains("*italic*"), "projection was {}", p.text);
    }

    #[test]
    fn heading_marker_is_hidden_at_content_start() {
        let md = "# Title";
        let doc = parse(md);
        let p = project(&doc.blocks[0], md, Selection::caret(2));
        assert_eq!(p.text, "Title");
        assert_eq!(p.display_to_source(0), 2);
    }

    #[test]
    fn heading_marker_is_revealed_inside_heading() {
        let md = "# Title";
        let doc = parse(md);
        let p = project(&doc.blocks[0], md, Selection::caret(4));
        assert_eq!(p.text, "# Title");
    }

    #[test]
    fn footnote_is_not_styled_as_code() {
        let md = "Text[^1]\n\n[^1]: note";
        let doc = parse(md);
        let p = project(&doc.blocks[0], md, Selection::caret(0));
        let footnote = p
            .segments
            .iter()
            .find(|s| s.style.footnote)
            .expect("footnote segment");
        assert_eq!(footnote.kind, SegmentKind::Text);
    }

    #[test]
    fn soft_break_stays_a_line_break_in_the_projection() {
        let md = "one\ntwo";
        let doc = parse(md);
        let p = project(&doc.blocks[0], md, Selection::caret(0));
        assert_eq!(p.text, "one\ntwo");
        assert_eq!(p.display_to_source(3), 3);
        assert_eq!(p.display_to_source(4), 4);
        assert_eq!(p.source_to_display(4, true), 4);
    }
}
