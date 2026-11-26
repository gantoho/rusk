//! Layout turns projected blocks into positioned galleys and hit-testable
//! rectangles. It is deliberately retained: input handling and painting both
//! read the same layout, so clicking and what you see can never disagree.

use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::epaint::text::{LayoutJob, TextWrapping};
use eframe::egui::{
    self, Color32, Context, FontFamily, FontId, Pos2, Rect, Stroke, TextFormat, Vec2,
};
use rusk_core::markdown::{Block, BlockKind, InlineStyle, ListMarker, TableData};
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
    pub mode: ViewMode,
    pub empty: bool,
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
    Text { heading: Option<u8> },
    Code { bg: Rect, label: String },
    TableGrid { grid: TableGrid },
    TableSource { bg: Rect },
    ImageBlock { image: ImageBlock },
    Rule,
    SourceLine { number: usize, current: bool },
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
            }
            ViewMode::Wysiwyg => {
                for (index, block) in parsed.blocks.iter().enumerate() {
                    let (top, bottom) = spacing_for(block, blocks.last());
                    y += top;
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
                        &mut self.code,
                        &mut self.images,
                    );
                    y += laid.rect.height() + bottom;
                    blocks.push(laid);
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
            mode,
            empty: text.is_empty(),
        }
    }
}

fn spacing_for(block: &Block, previous: Option<&LaidBlock>) -> (f32, f32) {
    let heading = block.heading_level();
    let previous_heading =
        previous.is_some_and(|p| matches!(p.kind, LaidKind::Text { heading: Some(_) }));
    match heading {
        Some(1) => (if previous.is_some() { 30.0 } else { 0.0 }, 14.0),
        Some(2) => (if previous.is_some() { 30.0 } else { 0.0 }, 10.0),
        Some(3) => (if previous.is_some() { 24.0 } else { 0.0 }, 8.0),
        Some(_) => (if previous.is_some() { 20.0 } else { 0.0 }, 6.0),
        None => match &block.kind {
            BlockKind::CodeBlock(_) | BlockKind::Html => (12.0, 18.0),
            BlockKind::Table => (12.0, 18.0),
            BlockKind::Image(_) => (12.0, 18.0),
            BlockKind::Rule => (18.0, 20.0),
            BlockKind::Heading(_) => (0.0, 0.0),
            BlockKind::Paragraph => {
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
        },
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
        BlockKind::Rule => LaidBlock {
            block: Some(block_index),
            source_line: Some(block.line),
            kind: LaidKind::Rule,
            rect: Rect::from_min_size(Pos2::new(content_left, y), Vec2::new(content_width, 20.0)),
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
        },
        BlockKind::Table => {
            let editing = block.range.start < selection.head && selection.head <= block.range.end;
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
            let editing = block.range.start < selection.head && selection.head <= block.range.end;
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
        background = theme.code_bg;
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
    let job = code.job(&display, Some("markdown"), theme, bg_width - 40.0);
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
