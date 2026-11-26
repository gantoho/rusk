//! Layout turns projected blocks into positioned galleys and hit-testable
//! rectangles. It is deliberately retained: input handling and painting both
//! read the same layout, so clicking and what you see can never disagree.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::epaint::text::{LayoutJob, TextWrapping};
use eframe::egui::{
    self, Color32, Context, FontFamily, FontId, Pos2, Rect, Stroke, TextFormat, Vec2,
};
use rusk_core::markdown::{Block, BlockKind, InlineStyle, ListMarker, PrefixKind, TableData};
use rusk_core::projection::{Projection, Segment, SegmentKind, project};
use rusk_core::{Document, Selection, ViewMode};

use crate::editor::code::CodeHighlighter;
use crate::editor::images::ImageCache;
use crate::theme::Theme;

pub const PAGE_MAX_WIDTH: f32 = 824.0;
pub const PAGE_MIN_WIDTH: f32 = 520.0;
pub const PAGE_SIDE_PAD: f32 = 64.0;
pub const TOP_PAD: f32 = 54.0;
pub const BOTTOM_PAD: f32 = 90.0;
/// Minimum distance between two blocks that a collapsed blank line separates.
const COLLAPSED_BLANK_GAP: f32 = 18.0;

#[derive(Clone, Debug, PartialEq)]
pub struct LayoutKey {
    pub revision: u64,
    pub selection: Selection,
    pub width_px: i32,
    pub mode: ViewMode,
    pub dark: bool,
}

#[derive(Clone, Debug)]
pub struct DocumentLayout {
    pub key: LayoutKey,
    pub page: Rect,
    pub height: f32,
    pub content_left: f32,
    pub blocks: Vec<LaidBlock>,
    pub meta: Vec<BlockMeta>,
    pub mode: ViewMode,
    pub empty: bool,
}

/// Cheap per-block identity used to reuse a previously laid-out block after
/// the text before it changed. `reveal` blocks are rebuilt because their
/// projection depends on the current selection.
#[derive(Clone, Copy, Debug, Default)]
pub struct BlockMeta {
    pub content_hash: u64,
    pub reveal: bool,
}

impl DocumentLayout {
    /// The block that owns a source position. When blocks touch at a
    /// boundary, the later one wins so a caret on a blank line stays on it.
    #[must_use]
    pub fn block_at_source_pos(&self, head: usize) -> Option<&LaidBlock> {
        let mut candidate = None;
        for block in &self.blocks {
            if block.source_range.start > head {
                break;
            }
            candidate = Some(block);
        }
        candidate
    }
}

#[derive(Clone, Debug)]
pub struct LaidBlock {
    /// Index into the parsed document. `None` for source-mode lines.
    #[allow(dead_code)]
    pub block: Option<usize>,
    #[allow(dead_code)]
    pub source_line: Option<usize>,
    pub kind: LaidKind,
    /// Full clickable row (page-wide).
    pub rect: Rect,
    pub text_pos: Pos2,
    pub text_width: f32,
    pub line_height: f32,
    pub galley: Option<Arc<egui::Galley>>,
    pub projection: Projection,
    pub source_range: Range<usize>,
    pub quote_depth: usize,
    pub list_marker: Option<ListMarker>,
}

#[derive(Clone, Debug)]
pub enum LaidKind {
    Text {
        heading: Option<u8>,
    },
    Code {
        bg: Rect,
        label: String,
    },
    /// A fenced code block edited as plain source lines, fences included.
    CodeSource,
    TableGrid {
        grid: TableGrid,
    },
    TableSource {
        bg: Rect,
    },
    ImageBlock {
        image: ImageBlock,
    },
    Rule,
    SourceLine {
        number: usize,
        current: bool,
    },
    /// A blank source line, rendered as an empty row so the projection keeps
    /// the source's line structure.
    Blank,
    /// The blank line that ends a table. Markdown needs it, but it reads as
    /// spacing under the card rather than as an empty line of its own.
    Spacer,
}

#[derive(Clone, Debug)]
pub struct TableGrid {
    pub bg: Rect,
    pub columns: usize,
    pub header_height: f32,
    pub row_height: f32,
    /// row, column, rectangle, source range, header, text
    pub cells: Vec<TableCellRect>,
}

#[derive(Clone, Debug)]
pub struct TableCellRect {
    pub row: usize,
    pub rect: Rect,
    pub source: Range<usize>,
    pub header: bool,
    pub text: String,
    pub align: egui::Align,
}

#[derive(Clone, Debug)]
pub struct ImageBlock {
    pub bg: Rect,
    pub image_rect: Option<Rect>,
    pub texture: Option<egui::TextureId>,
    pub alt: String,
    pub url: String,
    pub caption_pos: Pos2,
}

impl LaidBlock {
    fn placeholder() -> Self {
        Self {
            block: None,
            source_line: None,
            kind: LaidKind::Blank,
            rect: Rect::from_min_size(Pos2::ZERO, Vec2::ZERO),
            text_pos: Pos2::ZERO,
            text_width: 0.0,
            line_height: 0.0,
            galley: None,
            projection: Projection::default(),
            source_range: 0..0,
            quote_depth: 0,
            list_marker: None,
        }
    }

    /// Shift every source offset after an edit before this block. Content is
    /// unchanged, only its byte position moved.
    fn shift_source(&mut self, delta: isize) {
        if delta == 0 {
            return;
        }
        let shift = |value: usize| -> usize {
            if delta >= 0 {
                value.saturating_add(delta as usize)
            } else {
                value.saturating_sub(delta.unsigned_abs())
            }
        };
        let shift_range = |range: &mut Range<usize>| {
            range.start = shift(range.start);
            range.end = shift(range.end);
        };
        shift_range(&mut self.source_range);
        self.projection.empty_source = shift(self.projection.empty_source);
        for segment in &mut self.projection.segments {
            shift_range(&mut segment.source);
        }
        if let LaidKind::TableGrid { grid } = &mut self.kind {
            for cell in &mut grid.cells {
                shift_range(&mut cell.source);
            }
        }
    }

    /// Move all visual positions by `dy` points. Fonts and line breaks stay the
    /// same, so reusing a galley is safe.
    fn translate_y(&mut self, dy: f32) {
        if dy == 0.0 {
            return;
        }
        let offset = Vec2::new(0.0, dy);
        self.rect = self.rect.translate(offset);
        self.text_pos.y += dy;
        match &mut self.kind {
            LaidKind::Code { bg, .. } | LaidKind::TableSource { bg } => {
                *bg = bg.translate(offset);
            }
            LaidKind::TableGrid { grid } => {
                grid.bg = grid.bg.translate(offset);
                for cell in &mut grid.cells {
                    cell.rect = cell.rect.translate(offset);
                }
            }
            LaidKind::ImageBlock { image } => {
                image.bg = image.bg.translate(offset);
                if let Some(rect) = &mut image.image_rect {
                    *rect = rect.translate(offset);
                }
                image.caption_pos.y += dy;
            }
            LaidKind::Text { .. }
            | LaidKind::CodeSource
            | LaidKind::Rule
            | LaidKind::SourceLine { .. }
            | LaidKind::Blank
            | LaidKind::Spacer => {}
        }
    }
}

pub struct LayoutEngine {
    pub code: CodeHighlighter,
    pub images: ImageCache,
}

impl Default for LayoutEngine {
    fn default() -> Self {
        Self {
            code: CodeHighlighter::new(),
            images: ImageCache::default(),
        }
    }
}

impl LayoutEngine {
    pub fn build(
        &mut self,
        ctx: &Context,
        doc: &mut Document,
        theme: &Theme,
        width: f32,
        min_height: f32,
        mut previous: Option<&mut DocumentLayout>,
    ) -> DocumentLayout {
        let text = doc.text();
        let path = doc.path().map(PathBuf::from);
        let selection = doc.selection();
        let mode = doc.mode();
        let revision = doc.revision();
        let parsed = doc.parsed();

        let page_width = (width - 48.0)
            .clamp(PAGE_MIN_WIDTH, PAGE_MAX_WIDTH)
            .min(width.max(PAGE_MIN_WIDTH));
        let page_left = ((width - page_width) * 0.5).max(0.0);
        let content_left = page_left + PAGE_SIDE_PAD;
        let content_width = (page_width - PAGE_SIDE_PAD * 2.0).max(240.0);
        let key = LayoutKey {
            revision,
            selection,
            width_px: width.round() as i32,
            mode,
            dark: theme.kind.is_dark(),
        };

        let mut blocks = Vec::new();
        let mut meta = Vec::new();
        let mut y = TOP_PAD;

        match mode {
            ViewMode::Source => {
                build_source(
                    ctx,
                    theme,
                    &text,
                    selection,
                    content_left,
                    content_width,
                    &mut y,
                    &mut blocks,
                );
                meta.resize(blocks.len(), BlockMeta::default());
            }
            ViewMode::Wysiwyg => {
                let blanks = blank_lines(&text);
                let reusable = previous.as_ref().is_some_and(|old| {
                    old.key.width_px == key.width_px
                        && old.key.mode == mode
                        && old.key.dark == key.dark
                        && old.meta.len() == old.blocks.len()
                });
                let mut next_blank = 0;
                let mut source_cursor = 0;
                let mut last_bottom = 0.0;
                for (index, block) in parsed.blocks.iter().enumerate() {
                    let live_source = live_source_view(block, &text, selection);
                    let (top, bottom) = spacing_for(block, blocks.last(), live_source);
                    while next_blank < blanks.len() && blanks[next_blank].start < block.range.start
                    {
                        let blank = blanks[next_blank];
                        next_blank += 1;
                        if blank.start >= source_cursor {
                            // The first blank line of a run is the separator the
                            // parser needs; it reads as spacing, not as a row.
                            let spacer = (index > 0)
                                .then(|| separator_gap(&text, &blank, last_bottom, top, selection))
                                .flatten();
                            let reveal = selection_touches_line(selection, &text, blank.start);
                            let content_hash = blank_hash(&blank, spacer, reveal);
                            let reused = !reveal
                                && reusable
                                && previous.as_ref().is_some_and(|old| {
                                    old.meta.get(blocks.len()).is_some_and(|meta| {
                                        !meta.reveal && meta.content_hash == content_hash
                                    })
                                })
                                && previous.as_deref_mut().is_some_and(|old| {
                                    reuse_blank(
                                        old,
                                        blocks.len(),
                                        &mut blocks,
                                        &mut meta,
                                        &mut y,
                                        blank,
                                        spacer,
                                        content_hash,
                                        reveal,
                                    )
                                });
                            if !reused {
                                push_blank_line(
                                    &mut blocks,
                                    content_left,
                                    content_width,
                                    &mut y,
                                    blank,
                                    spacer,
                                );
                                meta.push(BlockMeta {
                                    content_hash,
                                    reveal,
                                });
                            }
                            source_cursor = blank.start;
                        }
                    }
                    y += top;

                    let reveal = selection_touches_block(block, selection);
                    let content_hash = block_hash(block, &text);
                    if reusable
                        && let Some(old) = previous.as_deref_mut()
                        && let Some(old_meta) = old.meta.get(blocks.len()).copied()
                        && !old_meta.reveal
                        && !reveal
                        && old_meta.content_hash == content_hash
                        && let Some(slot) = old.blocks.get_mut(blocks.len())
                    {
                        let mut laid = std::mem::replace(slot, LaidBlock::placeholder());
                        let source_delta =
                            block.content.start as isize - laid.source_range.start as isize;
                        laid.shift_source(source_delta);
                        laid.translate_y(y - laid.rect.top());
                        y += laid.rect.height() + bottom;
                        last_bottom = bottom;
                        blocks.push(laid);
                        meta.push(BlockMeta {
                            content_hash,
                            reveal,
                        });
                        source_cursor = source_cursor.max(block.range.end);
                        continue;
                    }

                    let laid = build_block(
                        ctx,
                        theme,
                        block,
                        index,
                        &text,
                        path.as_deref(),
                        selection,
                        content_left,
                        content_width,
                        y,
                        live_source,
                        &mut self.code,
                        &mut self.images,
                    );
                    y += laid.rect.height() + bottom;
                    last_bottom = bottom;
                    blocks.push(laid);
                    meta.push(BlockMeta {
                        content_hash,
                        reveal,
                    });
                    source_cursor = source_cursor.max(block.range.end);
                }
                // Trailing blank lines separate nothing, so they keep their rows.
                while next_blank < blanks.len() {
                    let blank = blanks[next_blank];
                    next_blank += 1;
                    if blank.start >= source_cursor {
                        let reveal = selection_touches_line(selection, &text, blank.start);
                        let content_hash = blank_hash(&blank, None, reveal);
                        let reused = !reveal
                            && reusable
                            && previous.as_ref().is_some_and(|old| {
                                old.meta.get(blocks.len()).is_some_and(|meta| {
                                    !meta.reveal && meta.content_hash == content_hash
                                })
                            })
                            && previous.as_deref_mut().is_some_and(|old| {
                                reuse_blank(
                                    old,
                                    blocks.len(),
                                    &mut blocks,
                                    &mut meta,
                                    &mut y,
                                    blank,
                                    None,
                                    content_hash,
                                    reveal,
                                )
                            });
                        if !reused {
                            push_blank_line(
                                &mut blocks,
                                content_left,
                                content_width,
                                &mut y,
                                blank,
                                None,
                            );
                            meta.push(BlockMeta {
                                content_hash,
                                reveal,
                            });
                        }
                        source_cursor = blank.start;
                    }
                }
            }
        }

        let height = (y + BOTTOM_PAD).max(min_height).max(320.0);
        DocumentLayout {
            key,
            page: Rect::from_min_size(Pos2::new(page_left, 0.0), Vec2::new(page_width, height)),
            height,
            content_left,
            blocks,
            meta,
            mode,
            empty: text.is_empty(),
        }
    }
}

/// A source line with no content of its own: fully blank lines and lines
/// holding nothing but quote markers. The parser drops both, so without an
/// entry of their own the caret has nowhere to sit. The first blank line of a
/// run between two blocks becomes a spacer; the rest keep their row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BlankLine {
    /// Byte offset of the line start.
    start: usize,
    /// Byte offset where typing on this line belongs, after quote markers.
    content: usize,
    quote_depth: usize,
    /// Whether the physical line immediately before this one was fully blank.
    previous_blank: bool,
}

/// Lines whose content is empty (including the line after a trailing
/// newline, matching source-mode line numbering).
fn blank_lines(text: &str) -> Vec<BlankLine> {
    let mut blanks = Vec::new();
    let mut start = 0;
    let mut previous_blank = false;
    for line in text.split_inclusive('\n') {
        let content = line.trim_end_matches(['\n', '\r']);
        if content.trim().is_empty() {
            blanks.push(BlankLine {
                start,
                content: start,
                quote_depth: 0,
                previous_blank,
            });
            previous_blank = true;
        } else if let Some(depth) = quote_only_prefix(content) {
            blanks.push(BlankLine {
                start,
                content: start + content.len(),
                quote_depth: depth,
                previous_blank,
            });
            previous_blank = false;
        } else {
            previous_blank = false;
        }
        start += line.len();
    }
    if text.ends_with('\n') {
        blanks.push(BlankLine {
            start: text.len(),
            content: text.len(),
            quote_depth: 0,
            previous_blank,
        });
    }
    blanks
}

/// For a line made only of quote markers (`>`, `> `, `> >`), the nesting
/// depth. Anything after the markers means the line has real content and the
/// parser keeps it.
fn quote_only_prefix(line: &str) -> Option<usize> {
    let mut depth = 0;
    let mut rest = line;
    loop {
        let spaces = rest.len() - rest.trim_start_matches(' ').len();
        if spaces > 3 {
            return None;
        }
        rest = &rest[spaces..];
        let Some(after) = rest.strip_prefix('>') else {
            break;
        };
        depth += 1;
        rest = after.strip_prefix(' ').unwrap_or(after);
    }
    if depth > 0 && rest.trim().is_empty() {
        Some(depth)
    } else {
        None
    }
}

/// A blank line that separates two blocks is the one empty line Markdown
/// requires; it reads as spacing. `None` means the line keeps a visible row.
/// The spacer tops the two blocks' own margins up to `COLLAPSED_BLANK_GAP`, so
/// cards and rules that already carry a large margin keep their tighter look.
///
/// A line the caret or selection touches always keeps its row: folding under
/// the caret would move everything below while typing on it.
fn separator_gap(
    text: &str,
    blank: &BlankLine,
    last_bottom: f32,
    next_top: f32,
    selection: Selection,
) -> Option<f32> {
    let truly_blank = blank.quote_depth == 0 && blank.content == blank.start;
    if !truly_blank || blank.previous_blank || selection_touches_line(selection, text, blank.start)
    {
        return None;
    }
    Some((COLLAPSED_BLANK_GAP - last_bottom - next_top).max(0.0))
}

/// True when the caret, or either end of a selection, sits on the source line
/// that starts at `start`. The line's newline is not part of it, so a caret on
/// the next line does not count.
fn selection_touches_line(selection: Selection, text: &str, start: usize) -> bool {
    let end = text[start..]
        .find('\n')
        .map_or(text.len(), |offset| start + offset);
    if selection.is_caret() {
        start <= selection.head && selection.head <= end
    } else {
        selection.start() <= end && start <= selection.end()
    }
}

/// A block that renders as its own source while the caret is on the line that
/// makes it: a rule, or a setext heading's underline. These constructs change
/// the lines *around* them, so drawing them only after the caret leaves keeps
/// the page still while they are being typed.
fn live_source_view(block: &Block, source: &str, selection: Selection) -> bool {
    match &block.kind {
        BlockKind::Rule => selection_touches_line(selection, source, block.range.start),
        BlockKind::Heading(_) => setext_underline(block)
            .is_some_and(|underline| selection_touches_line(selection, source, underline.start)),
        _ => false,
    }
}

/// The underline line of a setext heading. ATX headings and closing hashes
/// start at or before the content end, so only a setext underline sits past it.
fn setext_underline(block: &Block) -> Option<Range<usize>> {
    block
        .prefix
        .iter()
        .find(|prefix| prefix.kind == PrefixKind::Heading && prefix.range.start > block.content.end)
        .map(|prefix| prefix.range.clone())
}

/// True when the line ending right before `at` is empty; false at the very
/// start of the document.
pub(super) fn previous_line_is_blank(text: &str, at: usize) -> bool {
    if at == 0 {
        return false;
    }
    let end = at - 1;
    let start = text[..end].rfind('\n').map_or(0, |offset| offset + 1);
    text[start..end].trim().is_empty()
}

fn blank_hash(blank: &BlankLine, spacer: Option<f32>, reveal: bool) -> u64 {
    let mut hasher = DefaultHasher::new();
    blank.quote_depth.hash(&mut hasher);
    (blank.content == blank.start).hash(&mut hasher);
    spacer.map(f32::to_bits).hash(&mut hasher);
    reveal.hash(&mut hasher);
    hasher.finish()
}

#[allow(clippy::too_many_arguments)]
fn reuse_blank(
    old: &mut DocumentLayout,
    index: usize,
    blocks: &mut Vec<LaidBlock>,
    meta: &mut Vec<BlockMeta>,
    y: &mut f32,
    blank: BlankLine,
    spacer: Option<f32>,
    content_hash: u64,
    reveal: bool,
) -> bool {
    let Some(old_meta) = old.meta.get(index).copied() else {
        return false;
    };
    if old_meta.reveal || old_meta.content_hash != content_hash {
        return false;
    }
    let Some(slot) = old.blocks.get_mut(index) else {
        return false;
    };
    let kind_matches = matches!(
        (&slot.kind, spacer),
        (LaidKind::Blank, None) | (LaidKind::Spacer, Some(_))
    );
    if !kind_matches {
        return false;
    }
    let mut laid = std::mem::replace(slot, LaidBlock::placeholder());
    let source_delta = blank.content as isize - laid.source_range.start as isize;
    laid.shift_source(source_delta);
    laid.translate_y(*y - laid.rect.top());
    *y += laid.rect.height();
    blocks.push(laid);
    meta.push(BlockMeta {
        content_hash,
        reveal,
    });
    true
}

fn push_blank_line(
    blocks: &mut Vec<LaidBlock>,
    content_left: f32,
    content_width: f32,
    y: &mut f32,
    blank: BlankLine,
    spacer: Option<f32>,
) {
    let (kind, line_height, row_height) = match spacer {
        Some(gap) => (LaidKind::Spacer, 18.0, gap),
        None => (LaidKind::Blank, 29.5, 29.5),
    };
    let quote_indent = blank.quote_depth as f32 * 18.0;
    blocks.push(LaidBlock {
        block: None,
        source_line: None,
        kind,
        rect: Rect::from_min_size(
            Pos2::new(content_left, *y),
            Vec2::new(content_width, row_height),
        ),
        text_pos: Pos2::new(content_left + quote_indent, *y),
        text_width: (content_width - quote_indent).max(180.0),
        line_height,
        galley: None,
        projection: Projection {
            empty_source: blank.content,
            ..Projection::default()
        },
        source_range: blank.content..blank.content,
        quote_depth: blank.quote_depth,
        list_marker: None,
    });
    *y += row_height;
}

fn selection_touches_block(block: &Block, selection: Selection) -> bool {
    selection.start() <= block.range.end && selection.end() >= block.range.start
}

fn block_hash(block: &Block, source: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    source[block.range.clone()].hash(&mut hasher);
    std::mem::discriminant(&block.kind).hash(&mut hasher);
    block.quote_depth.hash(&mut hasher);
    block.list_depth.hash(&mut hasher);
    block.list_marker.is_some().hash(&mut hasher);
    hasher.finish()
}

fn spacing_for(block: &Block, previous: Option<&LaidBlock>, live_source: bool) -> (f32, f32) {
    if live_source {
        // Still typing it: keep the paragraph spacing the line had before the
        // parser recognized the construct, so nothing jumps on that keystroke.
        return paragraph_spacing(block, previous);
    }
    let heading = block.heading_level();
    match heading {
        Some(1) => (if previous.is_some() { 30.0 } else { 0.0 }, 14.0),
        Some(2) => (if previous.is_some() { 30.0 } else { 0.0 }, 10.0),
        Some(3) => (if previous.is_some() { 24.0 } else { 0.0 }, 8.0),
        Some(_) => (if previous.is_some() { 20.0 } else { 0.0 }, 6.0),
        None => match &block.kind {
            BlockKind::CodeBlock(_) | BlockKind::Html => (12.0, 18.0),
            BlockKind::Table => (12.0, 18.0),
            BlockKind::Image(_) => (12.0, 18.0),
            // A rule always has a required blank line above it, which folds
            // into spacing; its own margins stay small so the line does not
            // read as a skipped empty row.
            BlockKind::Rule => (9.0, 9.0),
            BlockKind::Heading(_) => (0.0, 0.0),
            BlockKind::Paragraph => paragraph_spacing(block, previous),
        },
    }
}

fn paragraph_spacing(block: &Block, previous: Option<&LaidBlock>) -> (f32, f32) {
    let previous_heading =
        previous.is_some_and(|p| matches!(p.kind, LaidKind::Text { heading: Some(_) }));
    if block.quote_depth > 0 {
        (6.0, 9.0)
    } else if block.list_marker.is_some() {
        (0.0, 3.0)
    } else if previous_heading {
        (0.0, 10.0)
    } else {
        (0.0, 9.0)
    }
}

#[allow(clippy::too_many_arguments)]
fn build_block(
    ctx: &Context,
    theme: &Theme,
    block: &Block,
    block_index: usize,
    source: &str,
    doc_path: Option<&std::path::Path>,
    selection: Selection,
    content_left: f32,
    content_width: f32,
    y: f32,
    live_source: bool,
    code: &mut CodeHighlighter,
    images: &mut ImageCache,
) -> LaidBlock {
    let quote_indent = block.quote_depth as f32 * 18.0;
    let list_indent = block.list_depth.saturating_sub(1) as f32 * 24.0;
    let indent = quote_indent + list_indent;
    let x = content_left + indent;
    let width = (content_width - indent).max(180.0);

    match &block.kind {
        BlockKind::Paragraph | BlockKind::Heading(_) => {
            if live_source && let Some(underline) = setext_underline(block) {
                return setext_source_block(
                    block,
                    block_index,
                    source,
                    theme,
                    x,
                    y,
                    width,
                    content_left,
                    content_width,
                    ctx,
                    &underline,
                );
            }
            let (size, line_height) = text_metrics(block, theme);
            let projection = project(block, source, selection);
            let job = text_job(theme, block, &projection, size, line_height, width);
            let galley = if projection.is_empty() {
                None
            } else {
                Some(ctx.fonts_mut(|fonts| fonts.layout_job(job)))
            };
            let height = galley
                .as_ref()
                .map_or(line_height, |g| g.size().y.max(line_height));
            let text_pos = Pos2::new(x, y);
            LaidBlock {
                block: Some(block_index),
                source_line: Some(block.line),
                kind: LaidKind::Text {
                    heading: block.heading_level(),
                },
                rect: Rect::from_min_size(
                    Pos2::new(content_left, y),
                    Vec2::new(content_width, height),
                ),
                text_pos,
                text_width: width,
                line_height,
                galley,
                projection,
                source_range: block.content.clone(),
                quote_depth: block.quote_depth,
                list_marker: block.list_marker.clone(),
            }
        }
        BlockKind::CodeBlock(info) => {
            if info.fenced && code_raw_view(block, selection) {
                return code_raw_block(
                    block,
                    block_index,
                    source,
                    theme,
                    x,
                    y,
                    width,
                    content_left,
                    content_width,
                    ctx,
                );
            }
            let display = source[block.content.clone()]
                .trim_end_matches(['\n', '\r'])
                .to_owned();
            let language = info.language.clone().unwrap_or_else(|| "text".to_owned());
            let bg_width = width.max(220.0);
            let job = code.job(&display, info.language.as_deref(), theme, bg_width - 40.0);
            let galley = if display.is_empty() {
                None
            } else {
                Some(ctx.fonts_mut(|fonts| fonts.layout_job(job)))
            };
            let body_height = galley.as_ref().map_or(12.0, |g| g.size().y);
            let bg_height = 38.0 + body_height + 16.0;
            let bg = Rect::from_min_size(Pos2::new(x, y), Vec2::new(bg_width, bg_height.max(62.0)));
            let source_end = block.content.start + display.len();
            LaidBlock {
                block: Some(block_index),
                source_line: Some(block.line),
                kind: LaidKind::Code {
                    bg,
                    label: format!("{}  ·  CODE", language.to_uppercase()),
                },
                rect: Rect::from_min_size(
                    Pos2::new(content_left, y),
                    Vec2::new(content_width, bg.height()),
                ),
                text_pos: Pos2::new(bg.left() + 20.0, bg.top() + 38.0),
                text_width: bg_width - 40.0,
                line_height: 23.0,
                galley,
                projection: raw_projection(&display, block.content.start..source_end),
                source_range: block.content.clone(),
                quote_depth: block.quote_depth,
                list_marker: block.list_marker.clone(),
            }
        }
        BlockKind::Html => {
            let display = source[block.content.clone()]
                .trim_end_matches(['\n', '\r'])
                .to_owned();
            let bg_width = width.max(220.0);
            let job = code.job(&display, Some("html"), theme, bg_width - 40.0);
            let galley = if display.is_empty() {
                None
            } else {
                Some(ctx.fonts_mut(|fonts| fonts.layout_job(job)))
            };
            let body_height = galley.as_ref().map_or(12.0, |g| g.size().y);
            let bg_height = 38.0 + body_height + 16.0;
            let bg = Rect::from_min_size(Pos2::new(x, y), Vec2::new(bg_width, bg_height));
            let source_end = block.content.start + display.len();
            LaidBlock {
                block: Some(block_index),
                source_line: Some(block.line),
                kind: LaidKind::Code {
                    bg,
                    label: "HTML  ·  BLOCK".to_owned(),
                },
                rect: Rect::from_min_size(
                    Pos2::new(content_left, y),
                    Vec2::new(content_width, bg.height()),
                ),
                text_pos: Pos2::new(bg.left() + 20.0, bg.top() + 38.0),
                text_width: bg_width - 40.0,
                line_height: 23.0,
                galley,
                projection: raw_projection(&display, block.content.start..source_end),
                source_range: block.content.clone(),
                quote_depth: block.quote_depth,
                list_marker: block.list_marker.clone(),
            }
        }
        BlockKind::Rule => {
            if live_source {
                return rule_source_block(
                    block,
                    block_index,
                    source,
                    theme,
                    x,
                    y,
                    width,
                    content_left,
                    content_width,
                    ctx,
                );
            }
            LaidBlock {
                block: Some(block_index),
                source_line: Some(block.line),
                kind: LaidKind::Rule,
                rect: Rect::from_min_size(
                    Pos2::new(content_left, y),
                    Vec2::new(content_width, 20.0),
                ),
                text_pos: Pos2::new(x, y),
                text_width: width,
                line_height: 20.0,
                galley: None,
                projection: Projection {
                    empty_source: block.content.start,
                    ..Projection::default()
                },
                source_range: block.content.clone(),
                quote_depth: block.quote_depth,
                list_marker: block.list_marker.clone(),
            }
        }
        BlockKind::Table => {
            let editing = block.range.start <= selection.head && selection.head <= block.edit_end();
            if editing {
                table_source_block(
                    block,
                    block_index,
                    source,
                    theme,
                    x,
                    y,
                    width,
                    content_left,
                    content_width,
                    ctx,
                    code,
                )
            } else if let Some(table) = &block.table {
                table_grid(
                    block,
                    block_index,
                    table,
                    theme,
                    x,
                    y,
                    width,
                    content_left,
                    content_width,
                )
            } else {
                table_source_block(
                    block,
                    block_index,
                    source,
                    theme,
                    x,
                    y,
                    width,
                    content_left,
                    content_width,
                    ctx,
                    code,
                )
            }
        }
        BlockKind::Image(_) => {
            let editing = block.range.start <= selection.head && selection.head <= block.edit_end();
            if editing {
                image_source_block(
                    block,
                    block_index,
                    source,
                    theme,
                    x,
                    y,
                    width,
                    content_left,
                    content_width,
                    ctx,
                )
            } else {
                image_card(
                    block,
                    block_index,
                    theme,
                    doc_path,
                    x,
                    y,
                    width,
                    content_left,
                    content_width,
                    ctx,
                    images,
                )
            }
        }
    }
}

fn text_metrics(block: &Block, theme: &Theme) -> (f32, f32) {
    let _ = theme;
    match block.heading_level() {
        Some(1) => (35.0, 46.0),
        Some(2) => (28.0, 38.0),
        Some(3) => (23.0, 33.0),
        Some(4) => (20.0, 29.0),
        Some(5) => (18.0, 27.0),
        Some(6) => (16.0, 25.0),
        _ => (17.5, 29.5),
    }
}

fn text_job(
    theme: &Theme,
    block: &Block,
    projection: &Projection,
    size: f32,
    line_height: f32,
    width: f32,
) -> LayoutJob {
    let heading = block.heading_level();
    let base_bold = heading.is_some();
    let mut job = LayoutJob {
        wrap: TextWrapping::wrap_at_width(width),
        break_on_newline: true,
        keep_trailing_whitespace: true,
        ..Default::default()
    };
    for seg in &projection.segments {
        let text = &projection.text[seg.display_bytes.clone()];
        if text.is_empty() {
            continue;
        }
        let format = format_for(theme, seg.style, size, line_height, base_bold, seg.kind);
        job.append(text, 0.0, format);
    }
    job
}

fn format_for(
    theme: &Theme,
    style: InlineStyle,
    size: f32,
    line_height: f32,
    base_bold: bool,
    kind: SegmentKind,
) -> TextFormat {
    let bold = base_bold || style.bold;
    let italic = style.italic;
    let mut font = match (bold, italic) {
        (true, true) => theme.bold_italic_font(size),
        (true, false) => theme.bold_font(size),
        (false, true) => theme.italic_font(size),
        (false, false) => theme.body_font(size),
    };
    let mut color = theme.text;
    let mut underline = Stroke::NONE;
    let mut strikethrough = Stroke::NONE;
    let mut background = Color32::TRANSPARENT;
    let mut line = line_height;

    if style.code || matches!(kind, SegmentKind::Code) {
        font = theme.mono_font((size * 0.88).max(12.0));
        color = theme.inline_code;
        // Inline code chips are painted as rounded rectangles in paint.rs;
        // egui's TextFormat background is a hard rectangle with no padding.
        background = Color32::TRANSPARENT;
        line = line_height;
    }
    if style.math {
        font = theme.mono_font((size * 0.92).max(12.0));
        color = theme.accent;
    }
    if style.link {
        color = theme.link;
        underline = Stroke::new(1.0, theme.link);
    }
    if style.image {
        color = theme.muted;
        font = theme.italic_font(size * 0.95);
    }
    if style.footnote {
        color = theme.accent;
        font = theme.bold_font(size * 0.78);
    }
    if style.syntax {
        color = theme.syntax;
        font = theme.mono_font((size * 0.82).max(11.0));
        background = Color32::TRANSPARENT;
    }
    if style.strikethrough {
        strikethrough = Stroke::new(1.2, color);
    }

    TextFormat {
        font_id: font,
        color,
        background,
        expand_bg: 2.5,
        line_height: Some(line.round()),
        underline,
        strikethrough,
        ..Default::default()
    }
}

fn raw_projection(text: &str, source: Range<usize>) -> Projection {
    let mut projection = Projection {
        text: text.to_owned(),
        empty_source: source.start,
        ..Projection::default()
    };
    if !text.is_empty() {
        let chars = text.chars().count();
        projection.segments.push(Segment {
            display: 0..chars,
            display_bytes: 0..text.len(),
            source,
            style: InlineStyle {
                code: true,
                ..InlineStyle::default()
            },
            kind: SegmentKind::Code,
            atomic: false,
        });
    }
    projection
}

/// Fenced code is edited as plain source while the caret is inside it (or
/// while the fence is still unclosed), so typing the fence, its language and
/// its body all look like text. The card is the preview for everywhere else.
fn code_raw_view(block: &Block, selection: Selection) -> bool {
    let fences = block
        .prefix
        .iter()
        .filter(|p| p.kind == PrefixKind::Fence)
        .count();
    if fences < 2 {
        return true;
    }
    block.range.start <= selection.head && selection.head <= block.edit_end()
}

/// The raw source of a fenced code block, laid out as plain monospace lines
/// without a card so it reads as the surrounding text does while typing.
#[allow(clippy::too_many_arguments)]
fn code_raw_block(
    block: &Block,
    block_index: usize,
    source: &str,
    theme: &Theme,
    x: f32,
    y: f32,
    width: f32,
    content_left: f32,
    content_width: f32,
    ctx: &Context,
) -> LaidBlock {
    let display = source[block.range.clone()]
        .trim_end_matches(['\n', '\r'])
        .to_owned();
    let mut job = LayoutJob {
        wrap: TextWrapping::wrap_at_width(width),
        break_on_newline: true,
        keep_trailing_whitespace: true,
        ..Default::default()
    };
    job.append(
        &display,
        0.0,
        TextFormat {
            font_id: theme.mono_font(15.0),
            color: theme.text,
            line_height: Some(23.0),
            ..Default::default()
        },
    );
    let galley = if display.is_empty() {
        None
    } else {
        Some(ctx.fonts_mut(|fonts| fonts.layout_job(job)))
    };
    let height = galley.as_ref().map_or(23.0, |g| g.size().y.max(23.0));
    LaidBlock {
        block: Some(block_index),
        source_line: Some(block.line),
        kind: LaidKind::CodeSource,
        rect: Rect::from_min_size(Pos2::new(content_left, y), Vec2::new(content_width, height)),
        text_pos: Pos2::new(x, y),
        text_width: width,
        line_height: 23.0,
        galley,
        projection: raw_projection(
            &display,
            block.range.start..block.range.start + display.len(),
        ),
        source_range: block.range.start..block.range.start + display.len(),
        quote_depth: block.quote_depth,
        list_marker: None,
    }
}

/// The rule line as the source that defines it, dimmed, while the caret is on
/// it. Committing it draws the line itself.
#[allow(clippy::too_many_arguments)]
fn rule_source_block(
    block: &Block,
    block_index: usize,
    source: &str,
    theme: &Theme,
    x: f32,
    y: f32,
    width: f32,
    content_left: f32,
    content_width: f32,
    ctx: &Context,
) -> LaidBlock {
    let end = raw_source_end(block, source);
    let display = &source[block.range.start..end];
    let mut job = plain_source_job(width);
    job.append(display, 0.0, plain_source_format(theme, theme.syntax));
    let galley = if display.is_empty() {
        None
    } else {
        Some(ctx.fonts_mut(|fonts| fonts.layout_job(job)))
    };
    let height = galley.as_ref().map_or(29.5, |g| g.size().y.max(29.5));
    LaidBlock {
        block: Some(block_index),
        source_line: Some(block.line),
        kind: LaidKind::Text { heading: None },
        rect: Rect::from_min_size(Pos2::new(content_left, y), Vec2::new(content_width, height)),
        text_pos: Pos2::new(x, y),
        text_width: width,
        line_height: 29.5,
        galley,
        projection: plain_source_projection(display, block.range.start..end),
        source_range: block.range.start..end,
        quote_depth: block.quote_depth,
        list_marker: None,
    }
}

/// A setext heading as its source: the paragraph lines as they were, then the
/// dimmed underline. Only while the caret is on the underline line.
#[allow(clippy::too_many_arguments)]
fn setext_source_block(
    block: &Block,
    block_index: usize,
    source: &str,
    theme: &Theme,
    x: f32,
    y: f32,
    width: f32,
    content_left: f32,
    content_width: f32,
    ctx: &Context,
    underline: &Range<usize>,
) -> LaidBlock {
    let end = raw_source_end(block, source);
    let raw = &source[block.range.start..end];
    let split = underline.start.clamp(block.range.start, end) - block.range.start;
    let mut job = plain_source_job(width);
    job.append(&raw[..split], 0.0, plain_source_format(theme, theme.text));
    if split < raw.len() {
        job.append(&raw[split..], 0.0, plain_source_format(theme, theme.syntax));
    }
    let galley = if raw.is_empty() {
        None
    } else {
        Some(ctx.fonts_mut(|fonts| fonts.layout_job(job)))
    };
    let height = galley.as_ref().map_or(29.5, |g| g.size().y.max(29.5));
    let mut projection = Projection {
        text: raw.to_owned(),
        empty_source: block.range.start,
        ..Projection::default()
    };
    let body_chars = raw[..split].chars().count();
    if split > 0 {
        projection.segments.push(Segment {
            display: 0..body_chars,
            display_bytes: 0..split,
            source: block.range.start..underline.start,
            style: InlineStyle::default(),
            kind: SegmentKind::Text,
            atomic: false,
        });
    }
    if split < raw.len() {
        projection.segments.push(Segment {
            display: body_chars..raw.chars().count(),
            display_bytes: split..raw.len(),
            source: underline.start..end,
            style: InlineStyle {
                syntax: true,
                ..InlineStyle::default()
            },
            kind: SegmentKind::Syntax,
            atomic: false,
        });
    }
    LaidBlock {
        block: Some(block_index),
        source_line: Some(block.line),
        kind: LaidKind::Text { heading: None },
        rect: Rect::from_min_size(Pos2::new(content_left, y), Vec2::new(content_width, height)),
        text_pos: Pos2::new(x, y),
        text_width: width,
        line_height: 29.5,
        galley,
        projection,
        source_range: block.range.start..end,
        quote_depth: block.quote_depth,
        list_marker: None,
    }
}

fn raw_source_end(block: &Block, source: &str) -> usize {
    block.range.start
        + source[block.range.clone()]
            .trim_end_matches(['\n', '\r'])
            .len()
}

fn plain_source_job(width: f32) -> LayoutJob {
    LayoutJob {
        wrap: TextWrapping::wrap_at_width(width),
        break_on_newline: true,
        keep_trailing_whitespace: true,
        ..Default::default()
    }
}

fn plain_source_format(theme: &Theme, color: Color32) -> TextFormat {
    TextFormat {
        font_id: theme.body_font(17.5),
        color,
        line_height: Some(29.5),
        ..Default::default()
    }
}

fn plain_source_projection(text: &str, source: Range<usize>) -> Projection {
    let mut projection = Projection {
        text: text.to_owned(),
        empty_source: source.start,
        ..Projection::default()
    };
    if !text.is_empty() {
        projection.segments.push(Segment {
            display: 0..text.chars().count(),
            display_bytes: 0..text.len(),
            source,
            style: InlineStyle::default(),
            kind: SegmentKind::Text,
            atomic: false,
        });
    }
    projection
}

#[allow(clippy::too_many_arguments)]
fn table_source_block(
    block: &Block,
    block_index: usize,
    source: &str,
    theme: &Theme,
    x: f32,
    y: f32,
    width: f32,
    content_left: f32,
    content_width: f32,
    ctx: &Context,
    code: &mut CodeHighlighter,
) -> LaidBlock {
    let display = source[block.content.clone()]
        .trim_end_matches(['\n', '\r'])
        .to_owned();
    let bg_width = width.max(220.0);
    // No syntax highlighting here: syntect's markdown grammar costs ~9ms per
    // rebuild (the whole card re-lays out on every caret move) and yields no
    // visible colors for table source.
    let job = code.job(&display, None, theme, bg_width - 40.0);
    let galley = if display.is_empty() {
        None
    } else {
        Some(ctx.fonts_mut(|fonts| fonts.layout_job(job)))
    };
    let body_height = galley.as_ref().map_or(12.0, |g| g.size().y);
    let bg_height = 38.0 + body_height + 16.0;
    let bg = Rect::from_min_size(Pos2::new(x, y), Vec2::new(bg_width, bg_height));
    LaidBlock {
        block: Some(block_index),
        source_line: Some(block.line),
        kind: LaidKind::TableSource { bg },
        rect: Rect::from_min_size(
            Pos2::new(content_left, y),
            Vec2::new(content_width, bg.height()),
        ),
        text_pos: Pos2::new(bg.left() + 20.0, bg.top() + 38.0),
        text_width: bg_width - 40.0,
        line_height: 23.0,
        galley,
        projection: raw_projection(
            &display,
            block.content.start..block.content.start + display.len(),
        ),
        source_range: block.content.clone(),
        quote_depth: block.quote_depth,
        list_marker: block.list_marker.clone(),
    }
}

#[allow(clippy::too_many_arguments)]
fn table_grid(
    block: &Block,
    block_index: usize,
    table: &TableData,
    theme: &Theme,
    x: f32,
    y: f32,
    width: f32,
    content_left: f32,
    content_width: f32,
) -> LaidBlock {
    let columns = table
        .rows
        .iter()
        .map(|r| r.cells.len())
        .max()
        .unwrap_or(0)
        .max(1);
    let columns = columns.min(8);
    let header_height = 38.0;
    let row_height = 34.0;
    let body_rows = table.rows.len().saturating_sub(1).max(1);
    let height = header_height + body_rows as f32 * row_height;
    let bg = Rect::from_min_size(Pos2::new(x, y), Vec2::new(width, height));
    let col_width = width / columns as f32;
    let mut cells = Vec::new();
    for (row_index, row) in table.rows.iter().enumerate() {
        let row_top = bg.top()
            + if row.header {
                0.0
            } else {
                header_height + (row_index - 1) as f32 * row_height
            };
        for col in 0..columns {
            let rect = Rect::from_min_size(
                Pos2::new(bg.left() + col as f32 * col_width, row_top),
                Vec2::new(
                    col_width,
                    if row.header {
                        header_height
                    } else {
                        row_height
                    },
                ),
            );
            let (source, text) = row.cells.get(col).map_or(
                (block.content.start..block.content.start, String::new()),
                |c| (c.range.clone(), c.text.clone()),
            );
            cells.push(TableCellRect {
                row: row_index,
                rect,
                source,
                header: row.header,
                text,
                align: table
                    .alignments
                    .get(col)
                    .copied()
                    .map_or(egui::Align::Min, |a| match a {
                        pulldown_cmark::Alignment::Center => egui::Align::Center,
                        pulldown_cmark::Alignment::Right => egui::Align::Max,
                        _ => egui::Align::Min,
                    }),
            });
        }
    }
    let _ = theme;
    LaidBlock {
        block: Some(block_index),
        source_line: Some(block.line),
        kind: LaidKind::TableGrid {
            grid: TableGrid {
                bg,
                columns,
                header_height,
                row_height,
                cells,
            },
        },
        rect: Rect::from_min_size(Pos2::new(content_left, y), Vec2::new(content_width, height)),
        text_pos: Pos2::new(x, y),
        text_width: width,
        line_height: row_height,
        galley: None,
        projection: Projection {
            empty_source: block.content.start,
            ..Projection::default()
        },
        source_range: block.content.clone(),
        quote_depth: block.quote_depth,
        list_marker: block.list_marker.clone(),
    }
}

#[allow(clippy::too_many_arguments)]
fn image_source_block(
    block: &Block,
    block_index: usize,
    source: &str,
    theme: &Theme,
    x: f32,
    y: f32,
    width: f32,
    content_left: f32,
    content_width: f32,
    ctx: &Context,
) -> LaidBlock {
    let display = source[block.content.clone()]
        .trim_end_matches(['\n', '\r'])
        .to_owned();
    let bg_width = width.clamp(220.0, 620.0);
    let job = text_job(
        theme,
        block,
        &raw_projection(
            &display,
            block.content.start..block.content.start + display.len(),
        ),
        15.0,
        24.0,
        bg_width - 40.0,
    );
    let galley = if display.is_empty() {
        None
    } else {
        Some(ctx.fonts_mut(|fonts| fonts.layout_job(job)))
    };
    let body_height = galley.as_ref().map_or(18.0, |g| g.size().y);
    let bg_height = 34.0 + body_height + 14.0;
    let bg = Rect::from_min_size(Pos2::new(x, y), Vec2::new(bg_width, bg_height));
    LaidBlock {
        block: Some(block_index),
        source_line: Some(block.line),
        kind: LaidKind::Code {
            bg,
            label: "IMAGE  ·  SOURCE".to_owned(),
        },
        rect: Rect::from_min_size(
            Pos2::new(content_left, y),
            Vec2::new(content_width, bg.height()),
        ),
        text_pos: Pos2::new(bg.left() + 20.0, bg.top() + 34.0),
        text_width: bg_width - 40.0,
        line_height: 24.0,
        galley,
        projection: raw_projection(
            &display,
            block.content.start..block.content.start + display.len(),
        ),
        source_range: block.content.clone(),
        quote_depth: block.quote_depth,
        list_marker: block.list_marker.clone(),
    }
}

#[allow(clippy::too_many_arguments)]
fn image_card(
    block: &Block,
    block_index: usize,
    theme: &Theme,
    doc_path: Option<&std::path::Path>,
    x: f32,
    y: f32,
    width: f32,
    content_left: f32,
    content_width: f32,
    ctx: &Context,
    images: &mut ImageCache,
) -> LaidBlock {
    let _ = theme;
    let info = match &block.kind {
        BlockKind::Image(info) => info,
        _ => unreachable!(),
    };
    let card_width = width.clamp(240.0, 640.0);
    let mut texture = None;
    let mut image_rect = None;
    let mut image_height = 0.0;
    if let Some(path) = resolve_image_path(doc_path, &info.url)
        && let Some(cached) = images.load(ctx, &path)
    {
        let available = card_width - 36.0;
        let scale = (available / cached.size.x)
            .min(360.0 / cached.size.y)
            .min(1.5);
        let display = cached.size * scale;
        let rect = Rect::from_min_size(
            Pos2::new(x + (card_width - display.x) * 0.5, y + 16.0),
            display,
        );
        image_rect = Some(rect);
        texture = Some(cached.texture_id);
        image_height = display.y;
    }
    let card_height = if image_rect.is_some() {
        image_height + 16.0 + 44.0
    } else {
        150.0
    };
    let bg = Rect::from_min_size(Pos2::new(x, y), Vec2::new(card_width, card_height));
    LaidBlock {
        block: Some(block_index),
        source_line: Some(block.line),
        kind: LaidKind::ImageBlock {
            image: ImageBlock {
                bg,
                image_rect,
                texture,
                alt: info.alt.clone(),
                url: info.url.clone(),
                caption_pos: Pos2::new(bg.left() + 18.0, bg.bottom() - 44.0),
            },
        },
        rect: Rect::from_min_size(
            Pos2::new(content_left, y),
            Vec2::new(content_width, card_height),
        ),
        text_pos: Pos2::new(bg.left() + 18.0, bg.top() + 18.0),
        text_width: card_width - 36.0,
        line_height: 26.0,
        galley: None,
        projection: Projection {
            empty_source: block.content.start,
            ..Projection::default()
        },
        source_range: block.content.clone(),
        quote_depth: block.quote_depth,
        list_marker: block.list_marker.clone(),
    }
}

/// Image cards need the egui context, which the layout signature does not
/// carry down here. The caller patches image textures after building; keeping
/// this helper independent makes layout deterministic for tests.
#[allow(dead_code)]
pub fn resolve_image_path(doc_path: Option<&std::path::Path>, url: &str) -> Option<PathBuf> {
    let url = url.trim();
    if url.is_empty() || url.starts_with("http://") || url.starts_with("https://") {
        return None;
    }
    let url = url.strip_prefix("file://").unwrap_or(url);
    let url = url.replace("%20", " ");
    let path = std::path::Path::new(&url);
    if path.is_absolute() {
        Some(path.to_path_buf())
    } else {
        doc_path
            .and_then(|p| p.parent())
            .map(|parent| parent.join(path))
    }
}

#[allow(clippy::too_many_arguments)]
fn build_source(
    ctx: &Context,
    theme: &Theme,
    source: &str,
    selection: Selection,
    content_left: f32,
    content_width: f32,
    y: &mut f32,
    blocks: &mut Vec<LaidBlock>,
) {
    let line_count =
        source.lines().count() + usize::from(source.is_empty() || source.ends_with('\n'));
    let current_line = source[..selection.head.min(source.len())]
        .bytes()
        .filter(|b| *b == b'\n')
        .count();
    let gutter = 46.0;
    let text_x = content_left + gutter;
    let text_width = (content_width - gutter).max(200.0);
    let mono = FontId::new(15.0, FontFamily::Name("rusk-mono".into()));
    let mut start = 0usize;
    for line in 0..line_count.max(1) {
        let end = source[start..]
            .find('\n')
            .map_or(source.len(), |offset| start + offset);
        let display_end = if end > start && source.as_bytes()[end - 1] == b'\r' {
            end - 1
        } else {
            end
        };
        let text = &source[start..display_end];
        let galley = if text.is_empty() {
            None
        } else {
            let mut job = LayoutJob::simple(text.to_owned(), mono.clone(), theme.text, text_width);
            // Include the palette in the layout job identity. This keeps the
            // galley cache from reusing glyph meshes colored for the old theme.
            job.first_row_min_height = if theme.kind.is_dark() { 0.01 } else { 0.0 };
            Some(ctx.fonts_mut(|fonts| fonts.layout_job(job)))
        };
        let height = galley.as_ref().map_or(24.0, |g| g.size().y.max(24.0));
        let projection = raw_projection(text, start..display_end);
        blocks.push(LaidBlock {
            block: None,
            source_line: Some(line),
            kind: LaidKind::SourceLine {
                number: line + 1,
                current: line == current_line,
            },
            rect: Rect::from_min_size(
                Pos2::new(content_left - 8.0, *y),
                Vec2::new(content_width + 16.0, height),
            ),
            text_pos: Pos2::new(text_x, *y),
            text_width,
            line_height: 24.0,
            galley,
            projection,
            source_range: start..display_end,
            quote_depth: 0,
            list_marker: None,
        });
        *y += height;
        start = end.saturating_add(1);
        if start > source.len() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::paint;
    use crate::theme::{Theme, ThemeKind};
    use rusk_core::EditKind;

    #[test]
    fn blank_lines_follow_source_lines() {
        let starts =
            |text: &str| -> Vec<usize> { blank_lines(text).iter().map(|b| b.start).collect() };
        assert_eq!(starts("a\nb"), Vec::<usize>::new());
        assert_eq!(starts("a\n\nb"), vec![2]);
        assert_eq!(starts("a\n\n\nb"), vec![2, 3]);
        assert_eq!(starts("a\n"), vec![2]);
        assert_eq!(starts("a\n\n"), vec![2, 3]);
        assert_eq!(starts("\n\n# h\n"), vec![0, 1, 6]);
        assert_eq!(starts("a\r\n\r\nb"), vec![3]);
        assert!(blank_lines("").is_empty());
    }

    #[test]
    fn quote_only_lines_become_rows_after_their_markers() {
        let blanks = blank_lines("> a\n> \n> >\n>\n");
        assert_eq!(blanks.len(), 4, "three quoted lines plus the trailing one");
        assert_eq!(blanks[0].content, 6, "caret goes after '> '");
        assert_eq!(blanks[0].quote_depth, 1);
        assert_eq!(blanks[1].content, 10);
        assert_eq!(blanks[1].quote_depth, 2);
        assert_eq!(blanks[2].content, 12);
        assert_eq!(blanks[2].quote_depth, 1);
        assert_eq!((blanks[3].start, blanks[3].quote_depth), (13, 0));
        assert_eq!(
            blank_lines("a\n> b\n")
                .last()
                .map(|b| (b.start, b.quote_depth)),
            Some((6, 0)),
            "the virtual line after a trailing newline stays plain"
        );

        let ctx = Context::default();
        let theme = Theme::new(ThemeKind::Paper);
        let mut doc = doc_with_selection("> a\n> \n> b\n", 6);
        let mut engine = LayoutEngine::default();
        let mut built = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |_ui| {
            built = Some(engine.build(&ctx, &mut doc, &theme, 900.0, 600.0, None));
        });
        output.textures_delta.clear();
        let layout = built.expect("layout built");

        let row = layout
            .blocks
            .iter()
            .find(|b| matches!(b.kind, LaidKind::Blank))
            .expect("an empty quoted line gets a row");
        assert_eq!(row.quote_depth, 1);
        assert_eq!(row.source_range.start, 6);
        assert!(
            row.text_pos.x > layout.content_left,
            "row is indented inside the quote"
        );
        assert_eq!(
            layout.block_at_source_pos(6).map(|b| b.rect.top()),
            Some(row.rect.top()),
            "the caret sits on the quoted blank row"
        );
        let caret = paint::caret_rect_local(&layout, &doc);
        assert_eq!(caret.map(|r| r.top()), Some(row.rect.top()));
    }

    #[test]
    fn quote_markers_inside_code_are_not_blank_rows() {
        let text = "```\n> \n```\n\nbelow\n";
        let ctx = Context::default();
        let theme = Theme::new(ThemeKind::Paper);
        let mut doc = doc_with_selection(text, text.len() - 1);
        let mut engine = LayoutEngine::default();
        let mut built = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |_ui| {
            built = Some(engine.build(&ctx, &mut doc, &theme, 900.0, 600.0, None));
        });
        output.textures_delta.clear();
        let layout = built.expect("layout built");
        assert!(
            !layout
                .blocks
                .iter()
                .any(|b| matches!(b.kind, LaidKind::Blank) && b.quote_depth > 0),
            "'> ' inside a fence is code, not an empty quote line"
        );
    }

    #[test]
    fn the_blank_line_after_a_table_reads_as_spacing() {
        let text = "| a | b |\n|---|---|\n| 1 | 2 |\n\nbelow\n";
        let ctx = Context::default();
        let theme = Theme::new(ThemeKind::Paper);
        let mut doc = doc_with_selection(text, text.len() - 1);
        let mut engine = LayoutEngine::default();
        let mut built = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |_ui| {
            built = Some(engine.build(&ctx, &mut doc, &theme, 900.0, 600.0, None));
        });
        output.textures_delta.clear();
        let layout = built.expect("layout built");

        let table = layout
            .blocks
            .iter()
            .find(|b| matches!(b.kind, LaidKind::TableGrid { .. }))
            .expect("table grid");
        let spacer = layout
            .blocks
            .iter()
            .find(|b| b.source_range.start == 30)
            .expect("the separator line keeps a caret home");
        assert!(matches!(spacer.kind, LaidKind::Spacer));
        assert_eq!(spacer.rect.height(), 0.0, "no empty row under the table");
        assert!(spacer.rect.top() > table.rect.bottom());
        assert_eq!(
            layout.block_at_source_pos(30).map(|b| b.rect.top()),
            Some(spacer.rect.top()),
            "the caret goes to the paragraph slot below the table"
        );

        let paragraph = layout
            .blocks
            .iter()
            .find(|b| b.source_range.start == 31)
            .expect("paragraph below");
        assert!(
            paragraph.rect.top() - table.rect.bottom() < 40.0,
            "the gap below the table reads as spacing"
        );
    }

    #[test]
    fn the_blank_line_between_paragraphs_reads_as_spacing() {
        let text = "a\n\nb\n";
        let ctx = Context::default();
        let theme = Theme::new(ThemeKind::Paper);
        let mut doc = doc_with_selection(text, 0);
        let mut engine = LayoutEngine::default();
        let mut built = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |_ui| {
            built = Some(engine.build(&ctx, &mut doc, &theme, 900.0, 600.0, None));
        });
        output.textures_delta.clear();
        let layout = built.expect("layout built");

        let spacer = layout
            .blocks
            .iter()
            .find(|b| b.source_range.start == 2)
            .expect("the separator keeps a caret home");
        assert!(matches!(spacer.kind, LaidKind::Spacer));
        assert!(
            !layout
                .blocks
                .iter()
                .any(|b| matches!(b.kind, LaidKind::Blank) && b.source_range.start == 2),
            "no empty row between two paragraphs"
        );
        let first = layout
            .blocks
            .iter()
            .find(|b| b.source_range.start == 0)
            .expect("first paragraph");
        let paragraph = layout
            .blocks
            .iter()
            .find(|b| b.source_range.start == 3)
            .expect("second paragraph");
        let gap = paragraph.rect.top() - first.rect.bottom();
        assert!(
            (17.0..=19.0).contains(&gap),
            "the paragraphs keep a readable gap, got {gap}"
        );
    }

    #[test]
    fn the_blank_line_before_a_rule_reads_as_spacing() {
        let text = "16316\n\n---\n\nStart writing.\n";
        let ctx = Context::default();
        let theme = Theme::new(ThemeKind::Paper);
        let mut doc = doc_with_selection(text, 5);
        let mut engine = LayoutEngine::default();
        let mut built = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |_ui| {
            built = Some(engine.build(&ctx, &mut doc, &theme, 900.0, 600.0, None));
        });
        output.textures_delta.clear();
        let layout = built.expect("layout built");

        let spacer = layout
            .blocks
            .iter()
            .find(|b| b.source_range.start == 6)
            .expect("the separator before the rule");
        assert!(
            matches!(spacer.kind, LaidKind::Spacer),
            "the required blank line folds into spacing"
        );
        let paragraph = layout
            .blocks
            .iter()
            .find(|b| b.source_range.start == 0)
            .expect("the paragraph above the rule");
        let rule = layout
            .blocks
            .iter()
            .find(|b| matches!(b.kind, LaidKind::Rule))
            .expect("the rule");
        let gap = rule.rect.top() - paragraph.rect.bottom();
        assert!(
            (17.0..=19.0).contains(&gap),
            "the rule keeps ordinary block spacing, got {gap}"
        );
    }

    #[test]
    fn the_blank_line_under_the_caret_keeps_its_row() {
        let text = "a\n\nb\n";
        let ctx = Context::default();
        let theme = Theme::new(ThemeKind::Paper);
        let mut doc = doc_with_selection(text, 2);
        let mut engine = LayoutEngine::default();
        let mut built = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |_ui| {
            built = Some(engine.build(&ctx, &mut doc, &theme, 900.0, 600.0, None));
        });
        output.textures_delta.clear();
        let layout = built.expect("layout built");

        assert!(
            layout
                .blocks
                .iter()
                .any(|b| matches!(b.kind, LaidKind::Blank) && b.source_range.start == 2),
            "a blank line being edited keeps a full row"
        );
        assert!(
            !layout
                .blocks
                .iter()
                .any(|b| matches!(b.kind, LaidKind::Spacer)),
            "nothing collapses under the caret"
        );
        assert_eq!(
            layout.block_at_source_pos(2).map(|b| b.source_range.start),
            Some(2)
        );
    }

    #[test]
    fn a_rule_reads_as_source_while_the_caret_is_on_it() {
        let text = "a\n\n---\n\nb\n";
        let ctx = Context::default();
        let theme = Theme::new(ThemeKind::Paper);
        let mut engine = LayoutEngine::default();
        let build = |engine: &mut LayoutEngine, doc: &mut Document| {
            let mut built = None;
            let mut output = ctx.run_ui(egui::RawInput::default(), |_ui| {
                built = Some(engine.build(&ctx, doc, &theme, 900.0, 600.0, None));
            });
            output.textures_delta.clear();
            built.expect("layout built")
        };

        let rule_start = text.find("---").unwrap();
        let mut typing = doc_with_selection(text, rule_start + 1);
        let layout = build(&mut engine, &mut typing);
        let source = layout
            .blocks
            .iter()
            .find(|b| b.source_range.start == rule_start)
            .expect("the rule line");
        assert!(
            matches!(source.kind, LaidKind::Text { heading: None }),
            "while the caret is on it the line is still the source"
        );
        assert_eq!(source.projection.text, "---");
        assert!(
            !layout
                .blocks
                .iter()
                .any(|b| matches!(b.kind, LaidKind::Rule)),
            "the line itself is not drawn yet"
        );

        let mut committed = doc_with_selection(text, 0);
        let layout = build(&mut engine, &mut committed);
        assert!(
            layout
                .blocks
                .iter()
                .any(|b| matches!(b.kind, LaidKind::Rule)),
            "away from the line the rule renders"
        );
    }

    #[test]
    fn a_setext_underline_reads_as_source_while_the_caret_is_on_it() {
        let text = "title\n---\n\nnext\n";
        let ctx = Context::default();
        let theme = Theme::new(ThemeKind::Paper);
        let mut engine = LayoutEngine::default();
        let build = |engine: &mut LayoutEngine, doc: &mut Document| {
            let mut built = None;
            let mut output = ctx.run_ui(egui::RawInput::default(), |_ui| {
                built = Some(engine.build(&ctx, doc, &theme, 900.0, 600.0, None));
            });
            output.textures_delta.clear();
            built.expect("layout built")
        };

        let underline = text.find("---").unwrap();
        let mut typing = doc_with_selection(text, underline + 1);
        let layout = build(&mut engine, &mut typing);
        let source = layout
            .blocks
            .iter()
            .find(|b| b.source_range.start == 0)
            .expect("the heading line");
        assert!(
            matches!(source.kind, LaidKind::Text { heading: None }),
            "the underline under the caret stays as text"
        );
        assert_eq!(source.projection.text, "title\n---");
        assert!(
            source
                .projection
                .segments
                .iter()
                .any(|s| s.style.syntax && s.source == (underline..underline + 3)),
            "only the underline reads as syntax"
        );

        let mut committed = doc_with_selection(text, 0);
        let layout = build(&mut engine, &mut committed);
        let heading = layout
            .blocks
            .iter()
            .find(|b| b.source_range.start == 0)
            .expect("the heading");
        assert!(
            matches!(heading.kind, LaidKind::Text { heading: Some(2) }),
            "away from the underline the setext heading commits"
        );
    }

    #[test]
    fn wysiwyg_keeps_blank_lines_and_their_caret() {
        let ctx = Context::default();
        let theme = Theme::new(ThemeKind::Paper);
        let mut doc = Document::from_text("a\n\n\nb\n");
        let mut engine = LayoutEngine::default();
        let mut built = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |_ui| {
            built = Some(engine.build(&ctx, &mut doc, &theme, 900.0, 600.0, None));
        });
        output.textures_delta.clear();
        let layout = built.expect("layout built");

        let spacer = layout
            .blocks
            .iter()
            .find(|b| b.source_range.start == 2)
            .expect("the required separator keeps a caret home");
        assert!(
            matches!(spacer.kind, LaidKind::Spacer),
            "the first blank line between two paragraphs reads as spacing"
        );
        let blanks: Vec<&LaidBlock> = layout
            .blocks
            .iter()
            .filter(|b| matches!(b.kind, LaidKind::Blank))
            .collect();
        assert_eq!(
            blanks.len(),
            2,
            "the extra empty line and the trailing one keep their rows"
        );
        assert!(spacer.rect.top() < blanks[0].rect.top());
        assert!(blanks[0].rect.top() < blanks[1].rect.top());
        assert_eq!(blanks[0].source_range.start, 3);
        assert_eq!(blanks[1].source_range.start, "a\n\n\nb\n".len());

        let caret = paint::caret_rect_local(&layout, &doc_with_selection("a\n\n\nb\n", 3));
        let blank = &blanks[0];
        assert_eq!(caret.map(|r| r.top()), Some(blank.rect.top()));
    }

    fn doc_with_selection(text: &str, head: usize) -> Document {
        let mut doc = Document::from_text(text);
        doc.set_selection(Selection::caret(head));
        doc
    }

    #[test]
    fn fenced_code_edits_as_plain_source_and_previews_as_a_card() {
        let ctx = Context::default();
        let theme = Theme::new(ThemeKind::Paper);
        let mut engine = LayoutEngine::default();
        let text = "intro\n\n```rust\nfn main() {}\n```\n";
        let build = |engine: &mut LayoutEngine, doc: &mut Document| {
            let mut built = None;
            let mut output = ctx.run_ui(egui::RawInput::default(), |_ui| {
                built = Some(engine.build(&ctx, doc, &theme, 900.0, 600.0, None));
            });
            output.textures_delta.clear();
            built.expect("layout built")
        };

        let mut typing = doc_with_selection(text, 10);
        let layout = build(&mut engine, &mut typing);
        let raw = layout
            .blocks
            .iter()
            .find(|b| matches!(b.kind, LaidKind::CodeSource))
            .expect("raw fence lines");
        assert_eq!(raw.source_range, 7..31);
        assert_eq!(raw.projection.text, "```rust\nfn main() {}\n```");

        let mut on_close = doc_with_selection(text, 29);
        let layout = build(&mut engine, &mut on_close);
        assert!(
            layout
                .blocks
                .iter()
                .any(|b| matches!(b.kind, LaidKind::CodeSource))
        );

        let mut preview = doc_with_selection(text, 2);
        let layout = build(&mut engine, &mut preview);
        let card = layout
            .blocks
            .iter()
            .find(|b| matches!(b.kind, LaidKind::Code { .. }))
            .expect("code card");
        assert_eq!(card.source_range, 15..27);
        assert!(matches!(
            &card.kind,
            LaidKind::Code { label, .. } if label == "RUST  ·  CODE"
        ));

        let mut unterminated = doc_with_selection("```", 3);
        let layout = build(&mut engine, &mut unterminated);
        assert!(
            layout
                .blocks
                .iter()
                .any(|b| matches!(b.kind, LaidKind::CodeSource) && b.source_range == (0..3))
        );
    }

    #[test]
    fn source_views_stop_at_the_last_line_of_the_block() {
        let ctx = Context::default();
        let theme = Theme::new(ThemeKind::Paper);
        let mut engine = LayoutEngine::default();
        let build = |engine: &mut LayoutEngine, doc: &mut Document| {
            let mut built = None;
            let mut output = ctx.run_ui(egui::RawInput::default(), |_ui| {
                built = Some(engine.build(&ctx, doc, &theme, 900.0, 600.0, None));
            });
            output.textures_delta.clear();
            built.expect("layout built")
        };

        let text = "intro\n\n```rust\nfn main() {}\n```\nafter\n";
        let mut below_fence = doc_with_selection(text, text.find("after").unwrap());
        let layout = build(&mut engine, &mut below_fence);
        assert!(
            !layout
                .blocks
                .iter()
                .any(|b| matches!(b.kind, LaidKind::CodeSource)),
            "the line after the closing fence is outside the block"
        );
        assert!(
            layout
                .blocks
                .iter()
                .any(|b| matches!(b.kind, LaidKind::Code { .. }))
        );

        let table = "intro\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\nbelow\n";
        let mut below_table = doc_with_selection(table, table.find("below").unwrap());
        let layout = build(&mut engine, &mut below_table);
        assert!(
            !layout
                .blocks
                .iter()
                .any(|b| matches!(b.kind, LaidKind::TableSource { .. })),
            "the line after the table is outside the block"
        );
        assert!(
            layout
                .blocks
                .iter()
                .any(|b| matches!(b.kind, LaidKind::TableGrid { .. }))
        );

        let mut inside_table = doc_with_selection(table, 8);
        let layout = build(&mut engine, &mut inside_table);
        assert!(
            layout
                .blocks
                .iter()
                .any(|b| matches!(b.kind, LaidKind::TableSource { .. }))
        );
    }

    #[test]
    fn layout_reuses_shifted_blocks_after_an_insertion() {
        let ctx = Context::default();
        let theme = Theme::new(ThemeKind::Paper);
        let mut engine = LayoutEngine::default();
        let mut doc = doc_with_selection("alpha\n\n# title\n\nomega\n", 0);

        let mut first = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |_ui| {
            first = Some(engine.build(&ctx, &mut doc, &theme, 900.0, 600.0, None));
        });
        output.textures_delta.clear();
        let mut first = first.expect("first layout");
        let old_title = first
            .blocks
            .iter()
            .find(|block| matches!(block.kind, LaidKind::Text { heading: Some(1) }))
            .expect("heading");
        let old_title_start = old_title.source_range.start;
        let old_title_y = old_title.text_pos.y;
        let blank_index = first
            .blocks
            .iter()
            .position(|block| matches!(block.kind, LaidKind::Blank | LaidKind::Spacer))
            .expect("blank row");
        let old_blank_start = first.blocks[blank_index].source_range.start;

        doc.insert_text("x", EditKind::Insert);
        let mut second = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |_ui| {
            second = Some(engine.build(&ctx, &mut doc, &theme, 900.0, 600.0, Some(&mut first)));
        });
        output.textures_delta.clear();
        let second = second.expect("second layout");
        let title = second
            .blocks
            .iter()
            .find(|block| matches!(block.kind, LaidKind::Text { heading: Some(1) }))
            .expect("heading after insertion");
        assert_eq!(title.source_range.start, old_title_start + 1);
        assert_eq!(title.text_pos.y, old_title_y);
        assert_eq!(
            second.blocks[blank_index].source_range.start,
            old_blank_start + 1
        );
        assert_eq!(second.blocks.len(), first.blocks.len());
    }

    #[test]
    fn italic_markers_reveal_after_a_selection_change() {
        let ctx = Context::default();
        let theme = Theme::new(ThemeKind::Paper);
        let mut engine = LayoutEngine::default();
        let text = "- **Bold**, *italic*, ~~strikethrough~~, and `inline code` render in place.";
        let mut doc = doc_with_selection(text, 2);

        let mut first = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |_ui| {
            first = Some(engine.build(&ctx, &mut doc, &theme, 900.0, 600.0, None));
        });
        output.textures_delta.clear();
        let mut first = first.expect("first layout");

        let head = text.find("italic").expect("italic") + 2;
        doc.set_selection(Selection::caret(head));
        let mut second = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |_ui| {
            second = Some(engine.build(&ctx, &mut doc, &theme, 900.0, 600.0, Some(&mut first)));
        });
        output.textures_delta.clear();
        let second = second.expect("second layout");
        let block = second
            .blocks
            .iter()
            .find(|block| block.source_range.start <= head && head <= block.source_range.end)
            .expect("italic block");
        assert!(
            block.projection.text.contains("*italic*"),
            "projection was {}",
            block.projection.text
        );
    }

    #[test]
    fn table_cells_shift_when_a_reused_layout_moves() {
        let ctx = Context::default();
        let theme = Theme::new(ThemeKind::Paper);
        let mut engine = LayoutEngine::default();
        let text = "intro\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\nbelow\n";
        let mut doc = doc_with_selection(text, 0);

        let mut first = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |_ui| {
            first = Some(engine.build(&ctx, &mut doc, &theme, 900.0, 600.0, None));
        });
        output.textures_delta.clear();
        let mut first = first.expect("first layout");
        let old_cell = first
            .blocks
            .iter()
            .find_map(|block| match &block.kind {
                LaidKind::TableGrid { grid } => grid.cells.first().map(|cell| cell.source.start),
                _ => None,
            })
            .expect("table cell");

        doc.insert_text("x", EditKind::Insert);
        let mut second = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |_ui| {
            second = Some(engine.build(&ctx, &mut doc, &theme, 900.0, 600.0, Some(&mut first)));
        });
        output.textures_delta.clear();
        let second = second.expect("second layout");
        let new_cell = second
            .blocks
            .iter()
            .find_map(|block| match &block.kind {
                LaidKind::TableGrid { grid } => grid.cells.first().map(|cell| cell.source.start),
                _ => None,
            })
            .expect("table cell after insertion");
        assert_eq!(new_cell, old_cell + 1);
    }
}
