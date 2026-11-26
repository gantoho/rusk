//! Painting for the editor surface. This module only reads layout and source
//! projections; it never mutates the document.

use std::sync::Arc;

use eframe::egui::epaint::text::{LayoutJob, TextWrapping};
use eframe::egui::{self, Align2, Color32, Painter, Pos2, Rect, Stroke, StrokeKind, Ui, Vec2};
use rusk_core::markdown::{ListKind, ListMarker};
use rusk_core::{Document, Selection, ViewMode};

use super::layout::{DocumentLayout, ImageBlock, LaidBlock, LaidKind, TOP_PAD, TableGrid};
use crate::theme::Theme;

#[allow(clippy::too_many_arguments)]
pub fn paint(
    ui: &Ui,
    painter: &Painter,
    layout: &DocumentLayout,
    doc: &Document,
    theme: &Theme,
    origin: Pos2,
    focused: bool,
    caret_visible: bool,
    ime_preedit: &str,
) {
    let shift = origin.to_vec2();
    let page = layout.page.translate(shift);
    let shadow = Rect::from_min_size(page.min + Vec2::new(1.5, 5.0), page.size());
    painter.rect_filled(
        shadow,
        4.0,
        Color32::from_black_alpha(if theme.kind.is_dark() { 42 } else { 10 }),
    );
    painter.rect(
        page,
        3.0,
        theme.page,
        Stroke::new(1.0, theme.page_edge),
        StrokeKind::Inside,
    );

    if layout.empty && layout.mode == ViewMode::Wysiwyg {
        paint_empty_state(ui, painter, layout, theme, shift);
    }

    // Backgrounds first.
    for block in &layout.blocks {
        paint_block_background(ui, painter, block, theme, shift);
        paint_inline_code_chips(painter, block, theme, shift);
        paint_quote_bars(painter, block, theme, shift);
    }

    // Selection sits under the glyphs.
    if !doc.selection().is_caret() {
        for block in &layout.blocks {
            paint_selection(painter, block, doc.selection(), theme, shift);
        }
    }

    // Text and decorations.
    for block in &layout.blocks {
        paint_block_text(ui, painter, block, theme, shift);
        paint_list_marker(painter, block, theme, shift);
    }

    if focused || layout.empty {
        let caret = caret_rect_local(layout, doc).map(|r| r.translate(shift));
        if let Some(caret) = caret {
            if focused && caret_visible {
                painter.rect_filled(caret, 1.0, theme.caret);
            } else if focused {
                painter.rect_filled(caret, 1.0, theme.caret.gamma_multiply(0.15));
            }
            if !ime_preedit.is_empty() {
                paint_preedit(ui, painter, theme, caret, ime_preedit);
            }
        }
    }
}

fn paint_empty_state(
    _ui: &Ui,
    painter: &Painter,
    layout: &DocumentLayout,
    theme: &Theme,
    shift: Vec2,
) {
    let x = layout.content_left + shift.x;
    let y = TOP_PAD + shift.y;
    painter.text(
        Pos2::new(x, y),
        Align2::LEFT_TOP,
        "Start writing…",
        theme.italic_font(24.0),
        theme.faint,
    );
    painter.text(
        Pos2::new(x, y + 38.0),
        Align2::LEFT_TOP,
        "The Markdown source is the document.",
        theme.body_font(13.5),
        theme.faint,
    );
    let hints = [
        "Ctrl+/   switch to source",
        "Ctrl+S   save",
        "Ctrl+B   bold",
    ];
    let mut hint_y = y + 92.0;
    for hint in hints {
        painter.text(
            Pos2::new(x, hint_y),
            Align2::LEFT_TOP,
            hint,
            theme.mono_font(11.5),
            theme.faint,
        );
        hint_y += 20.0;
    }
}

fn paint_block_background(
    _ui: &Ui,
    painter: &Painter,
    block: &LaidBlock,
    theme: &Theme,
    shift: Vec2,
) {
    match &block.kind {
        LaidKind::Code { bg, .. } | LaidKind::TableSource { bg } => {
            let bg = bg.translate(shift);
            painter.rect(
                bg,
                10.0,
                theme.code_bg,
                Stroke::new(1.0, theme.code_border),
                StrokeKind::Inside,
            );
        }
        LaidKind::TableGrid { grid } => paint_table_background(painter, grid, theme, shift),
        LaidKind::ImageBlock { image } => paint_image_background(painter, image, theme, shift),
        LaidKind::SourceLine { current, .. } if *current => {
            painter.rect_filled(
                block.rect.translate(shift).shrink2(Vec2::new(6.0, 0.0)),
                5.0,
                theme.accent_soft.gamma_multiply(0.60),
            );
        }
        _ => {}
    }
}

fn paint_quote_bars(painter: &Painter, block: &LaidBlock, theme: &Theme, shift: Vec2) {
    if block.quote_depth == 0 {
        return;
    }
    let base = block.text_pos.x - block.quote_depth as f32 * 18.0;
    for level in 0..block.quote_depth {
        let x = base - 8.0 + level as f32 * 18.0;
        let top = block.rect.top() + 2.0;
        let bottom = block.rect.bottom() - 2.0;
        painter.line_segment(
            [
                Pos2::new(x + shift.x, top + shift.y),
                Pos2::new(x + shift.x, bottom + shift.y),
            ],
            Stroke::new(2.5, theme.quote_bar),
        );
    }
}

fn paint_selection(
    painter: &Painter,
    block: &LaidBlock,
    selection: Selection,
    theme: &Theme,
    shift: Vec2,
) {
    let Some(display) = block.projection.selection_display(selection) else {
        return;
    };
    if display.is_empty() {
        return;
    }
    let Some(galley) = &block.galley else {
        return;
    };
    let total_chars = galley.job.text.chars().count();
    let display = display.start.min(total_chars)..display.end.min(total_chars);
    if display.is_empty() {
        return;
    }
    let start_cursor = galley.layout_from_cursor(egui::text::CCursor::new(display.start));
    let end_cursor = galley.layout_from_cursor(egui::text::CCursor::new(display.end));
    for (row_index, row) in galley.rows.iter().enumerate() {
        if row_index < start_cursor.row || row_index > end_cursor.row {
            continue;
        }
        let columns = row.char_count_excluding_newline().0;
        let start_column = if row_index == start_cursor.row {
            start_cursor.column.0
        } else {
            0
        };
        let end_column = if row_index == end_cursor.row {
            end_cursor.column.0
        } else {
            columns
        }
        .min(columns);
        if start_column >= end_column {
            continue;
        }
        let x1 = row.x_offset(egui::text::CharIndex(start_column));
        let x2 = row.x_offset(egui::text::CharIndex(end_column));
        let rect = Rect::from_min_max(
            Pos2::new(
                block.text_pos.x + x1 + shift.x,
                block.text_pos.y + row.min_y() + shift.y,
            ),
            Pos2::new(
                block.text_pos.x + x2 + shift.x,
                block.text_pos.y + row.max_y() + shift.y,
            ),
        );
        painter.rect_filled(rect, 2.0, theme.selection);
    }
}

fn paint_inline_code_chips(painter: &Painter, block: &LaidBlock, theme: &Theme, shift: Vec2) {
    if !matches!(block.kind, LaidKind::Text { .. }) {
        return;
    }
    let Some(galley) = &block.galley else {
        return;
    };
    let total_chars = galley.job.text.chars().count();
    for segment in &block.projection.segments {
        if !segment.style.code {
            continue;
        }
        let start = segment.display.start.min(total_chars);
        let end = segment.display.end.min(total_chars);
        if start >= end {
            continue;
        }
        let mut row_start = 0usize;
        for row in &galley.rows {
            let count = row.char_count_excluding_newline().0;
            let row_end = row_start + count;
            let from = start.max(row_start);
            let to = end.min(row_end);
            if from < to {
                let p1 = galley.pos_from_cursor(egui::text::CCursor::new(from));
                let p2 = galley.pos_from_cursor(egui::text::CCursor::new(to));
                let rect = Rect::from_min_max(
                    Pos2::new(
                        block.text_pos.x + p1.left() - 3.0 + shift.x,
                        block.text_pos.y + row.min_y() + 2.0 + shift.y,
                    ),
                    Pos2::new(
                        block.text_pos.x + p2.left() + 3.0 + shift.x,
                        block.text_pos.y + row.max_y() - 2.0 + shift.y,
                    ),
                );
                painter.rect(
                    rect,
                    3.5,
                    theme.code_bg,
                    Stroke::new(1.0, theme.code_border),
                    StrokeKind::Inside,
                );
            }
            row_start += row.char_count_including_newline().0;
        }
    }
}

fn paint_block_text(ui: &Ui, painter: &Painter, block: &LaidBlock, theme: &Theme, shift: Vec2) {
    let _ = ui;
    if let Some(galley) = &block.galley {
        if matches!(block.kind, LaidKind::SourceLine { .. }) {
            painter.galley_with_override_text_color(
                block.text_pos + shift,
                Arc::clone(galley),
                theme.text,
            );
        } else {
            painter.galley(block.text_pos + shift, Arc::clone(galley), theme.text);
        }
    }
    match &block.kind {
        LaidKind::Code {
            bg,
            label,
            label_visible,
            ..
        } => {
            if !label_visible {
                return;
            }
            let bg = bg.translate(shift);
            painter.text(
                Pos2::new(bg.right() - 16.0, bg.top() + 7.0),
                Align2::RIGHT_TOP,
                label,
                theme.mono_font(9.5),
                theme.muted,
            );
        }
        LaidKind::TableSource { bg } => {
            let bg = bg.translate(shift);
            painter.circle_filled(
                Pos2::new(bg.left() + 18.0, bg.top() + 15.0),
                3.0,
                theme.link,
            );
            painter.text(
                Pos2::new(bg.left() + 28.0, bg.top() + 11.0),
                Align2::LEFT_TOP,
                "TABLE  ·  SOURCE",
                theme.mono_font(10.0),
                theme.muted,
            );
            painter.text(
                Pos2::new(bg.right() - 16.0, bg.top() + 11.0),
                Align2::RIGHT_TOP,
                "click outside to render",
                theme.body_font(10.0),
                theme.faint,
            );
        }
        LaidKind::TableGrid { grid } => paint_table_text(painter, grid, theme, shift),
        LaidKind::ImageBlock { image } => paint_image_content(painter, image, theme, shift),
        LaidKind::Rule => {
            let y = block.rect.center().y + shift.y;
            let x1 = block.text_pos.x + shift.x;
            let x2 = x1 + block.text_width;
            painter.line_segment(
                [Pos2::new(x1, y), Pos2::new(x2, y)],
                Stroke::new(1.0, theme.border),
            );
            painter.circle_filled(Pos2::new((x1 + x2) * 0.5, y), 2.0, theme.faint);
        }
        LaidKind::SourceLine { number, current } => {
            let color = if *current { theme.accent } else { theme.faint };
            painter.text(
                Pos2::new(
                    block.text_pos.x - 17.0 + shift.x,
                    block.rect.top() + 3.0 + shift.y,
                ),
                Align2::RIGHT_TOP,
                number.to_string(),
                theme.mono_font(10.5),
                color,
            );
        }
        LaidKind::Text { .. } | LaidKind::Blank | LaidKind::Spacer | LaidKind::CodeSource => {}
    }
}

fn paint_list_marker(painter: &Painter, block: &LaidBlock, theme: &Theme, shift: Vec2) {
    let Some(marker) = &block.list_marker else {
        return;
    };
    let y = block.rect.top() + block.line_height * 0.56 + shift.y;
    let x = block.text_pos.x - 16.0 + shift.x;
    if marker.checked.is_some() {
        paint_checkbox(painter, marker, Pos2::new(x - 1.0, y), theme);
        return;
    }
    match &marker.kind {
        ListKind::Bullet(ch) => {
            if *ch == '-' || *ch == '*' || *ch == '+' {
                painter.circle_filled(Pos2::new(x, y), 2.8, theme.muted);
            } else {
                painter.text(
                    Pos2::new(x, y),
                    Align2::CENTER_CENTER,
                    ch.to_string(),
                    theme.body_font(14.0),
                    theme.muted,
                );
            }
        }
        ListKind::Ordered(text) => {
            painter.text(
                Pos2::new(block.text_pos.x - 10.0 + shift.x, y),
                Align2::RIGHT_CENTER,
                text,
                theme.mono_font(12.5),
                theme.muted,
            );
        }
    }
}

fn paint_checkbox(painter: &Painter, marker: &ListMarker, center: Pos2, theme: &Theme) {
    let checked = marker.checked.unwrap_or(false);
    let rect = Rect::from_center_size(center, Vec2::new(13.0, 13.0));
    painter.rect(
        rect,
        3.5,
        if checked { theme.accent } else { theme.page },
        Stroke::new(1.2, if checked { theme.accent } else { theme.faint }),
        StrokeKind::Inside,
    );
    if checked {
        let s = Stroke::new(1.6, Color32::WHITE);
        painter.line_segment(
            [
                Pos2::new(rect.left() + 3.0, rect.center().y),
                Pos2::new(rect.center().x - 0.5, rect.bottom() - 3.5),
            ],
            s,
        );
        painter.line_segment(
            [
                Pos2::new(rect.center().x - 0.5, rect.bottom() - 3.5),
                Pos2::new(rect.right() - 2.8, rect.top() + 3.5),
            ],
            s,
        );
    }
}

fn paint_table_background(painter: &Painter, grid: &TableGrid, theme: &Theme, shift: Vec2) {
    let bg = grid.bg.translate(shift);
    painter.rect(
        bg,
        9.0,
        theme.page,
        Stroke::new(1.0, theme.border),
        StrokeKind::Inside,
    );
    for cell in &grid.cells {
        let rect = cell.rect.translate(shift);
        if cell.header {
            painter.rect_filled(rect, 0.0, theme.table_head);
        } else if cell.row % 2 == 0 {
            painter.rect_filled(rect, 0.0, theme.table_stripe);
        }
    }
    // Grid lines.
    let stroke = Stroke::new(1.0, theme.border);
    let header_bottom = bg.top() + grid.header_height;
    painter.line_segment(
        [
            Pos2::new(bg.left(), header_bottom),
            Pos2::new(bg.right(), header_bottom),
        ],
        stroke,
    );
    for row in 1..=grid.cells.len().div_ceil(grid.columns).saturating_sub(1) {
        let y = header_bottom + row as f32 * grid.row_height;
        if y < bg.bottom() {
            painter.line_segment([Pos2::new(bg.left(), y), Pos2::new(bg.right(), y)], stroke);
        }
    }
    for col in 1..grid.columns {
        let x = bg.left() + col as f32 * (bg.width() / grid.columns as f32);
        painter.line_segment([Pos2::new(x, bg.top()), Pos2::new(x, bg.bottom())], stroke);
    }
    // Outer contour, redrawn for crispness.
    painter.rect_stroke(bg, 9.0, Stroke::new(1.0, theme.border), StrokeKind::Inside);
}

fn paint_table_text(painter: &Painter, grid: &TableGrid, theme: &Theme, shift: Vec2) {
    for cell in &grid.cells {
        let rect = cell.rect.translate(shift);
        let clip = rect.shrink2(Vec2::new(9.0, 3.0));
        let p = painter.with_clip_rect(clip);
        if let Some(galley) = &cell.galley {
            p.galley(cell.text_origin + shift, galley.clone(), theme.text);
            continue;
        }
        let (anchor_x, align) = match cell.align {
            egui::Align::Center => (rect.center().x, Align2::CENTER_CENTER),
            egui::Align::Max => (rect.right() - 10.0, Align2::RIGHT_CENTER),
            _ => (rect.left() + 10.0, Align2::LEFT_CENTER),
        };
        let font = if cell.header {
            theme.bold_font(13.5)
        } else {
            theme.body_font(14.0)
        };
        p.text(
            Pos2::new(anchor_x, rect.center().y),
            align,
            &cell.text,
            font,
            theme.text,
        );
    }
}

fn paint_image_background(painter: &Painter, image: &ImageBlock, theme: &Theme, shift: Vec2) {
    let bg = image.bg.translate(shift);
    painter.rect(
        bg,
        10.0,
        theme.image_placeholder,
        Stroke::new(1.0, theme.border),
        StrokeKind::Inside,
    );
    if let (Some(texture), Some(rect)) = (image.texture, image.image_rect) {
        painter.image(
            texture,
            rect.translate(shift),
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
    }
}

fn paint_image_content(painter: &Painter, image: &ImageBlock, theme: &Theme, shift: Vec2) {
    let bg = image.bg.translate(shift);
    if image.texture.is_none() {
        let center = Pos2::new(bg.center().x, bg.top() + 48.0);
        painter.circle_filled(center + Vec2::new(24.0, -14.0), 7.0, theme.faint);
        let mountain = [
            center + Vec2::new(-56.0, 30.0),
            center + Vec2::new(-18.0, -8.0),
            center + Vec2::new(10.0, 18.0),
            center + Vec2::new(30.0, 2.0),
            center + Vec2::new(58.0, 30.0),
        ];
        for pair in mountain.windows(2) {
            painter.line_segment([pair[0], pair[1]], Stroke::new(1.5, theme.faint));
        }
    }
    let caption = if image.alt.is_empty() {
        "Image"
    } else {
        &image.alt
    };
    painter.text(
        image.caption_pos + shift,
        Align2::LEFT_TOP,
        caption,
        theme.italic_font(13.5),
        theme.muted,
    );
    if !image.url.is_empty() {
        let clip = Rect::from_min_max(
            Pos2::new(bg.left() + 18.0, bg.bottom() - 24.0),
            Pos2::new(bg.right() - 18.0, bg.bottom() - 4.0),
        );
        let p = painter.with_clip_rect(clip);
        p.text(
            Pos2::new(bg.left() + 18.0, bg.bottom() - 22.0),
            Align2::LEFT_TOP,
            &image.url,
            theme.mono_font(10.0),
            theme.faint,
        );
    }
}

fn paint_preedit(ui: &Ui, painter: &Painter, theme: &Theme, caret: Rect, text: &str) {
    let font = theme.body_font(17.5);
    let job = LayoutJob {
        wrap: TextWrapping::no_max_width(),
        ..LayoutJob::simple(text.to_owned(), font.clone(), theme.text, f32::INFINITY)
    };
    let galley = painter.layout_job(job);
    let size = galley.size();
    let mut x = caret.left();
    let max_x = ui.clip_rect().right() - 24.0;
    if x + size.x + 14.0 > max_x {
        x = (max_x - size.x - 14.0).max(ui.clip_rect().left() + 8.0);
    }
    let rect = Rect::from_min_size(
        Pos2::new(x, caret.top() - 1.0),
        Vec2::new(size.x + 14.0, size.y.max(24.0) + 4.0),
    );
    painter.rect_filled(rect, 4.0, theme.accent_soft);
    painter.rect_stroke(
        rect,
        4.0,
        Stroke::new(1.0, theme.accent),
        StrokeKind::Inside,
    );
    painter.galley(rect.min + Vec2::new(7.0, 2.0), galley, theme.text);
}

/// The caret rectangle in layout coordinates (relative to the scroll content
/// origin). This is shared by painting, hit testing and IME placement.
#[must_use]
pub fn caret_rect_local(layout: &DocumentLayout, doc: &Document) -> Option<Rect> {
    let head = doc.selection().head;
    if layout.blocks.is_empty() {
        let (x, line_height) = if layout.mode == ViewMode::Source {
            (layout.content_left + 46.0, 24.0)
        } else {
            (layout.content_left, 29.5)
        };
        return Some(Rect::from_min_size(
            Pos2::new(x, TOP_PAD),
            Vec2::new(2.0, line_height),
        ));
    }

    let block = match layout.block_at_source_pos(head) {
        Some(block) => block,
        None => layout.blocks.first().unwrap(),
    };
    if let LaidKind::TableGrid { grid } = &block.kind {
        for cell in &grid.cells {
            if cell.source.start <= head && head <= cell.source.end {
                let Some(galley) = &cell.galley else {
                    break;
                };
                let byte = head.saturating_sub(cell.source.start).min(cell.text.len());
                let index = cell.text[..byte].chars().count();
                let cursor = galley.pos_from_cursor(egui::text::CCursor::new(index));
                return Some(Rect::from_min_size(
                    Pos2::new(
                        cell.text_origin.x + cursor.left(),
                        cell.text_origin.y + cursor.top(),
                    ),
                    Vec2::new(2.0, cursor.height().max(cell.rect.height() * 0.55)),
                ));
            }
        }
    }
    let display = block.projection.source_to_display(head, true);
    if let Some(galley) = &block.galley {
        let cursor = galley.pos_from_cursor(egui::text::CCursor::new(display));
        let height = cursor.height().max(block.line_height);
        Some(Rect::from_min_size(
            Pos2::new(
                block.text_pos.x + cursor.left(),
                block.text_pos.y + cursor.top(),
            ),
            Vec2::new(2.0, height),
        ))
    } else {
        Some(Rect::from_min_size(
            block.text_pos,
            Vec2::new(2.0, block.line_height),
        ))
    }
}
