//! Pointer and keyboard interaction for the source-backed editor.
//!
//! Every edit still goes through `Document` transactions; this module only
//! decides *where* an input lands in the canonical byte stream.

use std::ops::Range;

use eframe::egui::{self, Event, ImeEvent, Key, PointerButton, Pos2, Response, Ui};
use rusk_core::markdown::{BlockKind, MarkerKind, PrefixKind};
use rusk_core::{Document, Edit, EditKind, Selection, Transaction};

use super::layout::{DocumentLayout, LaidBlock, LaidKind};
use super::paint;

impl super::Editor {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn handle_pointer(
        &mut self,
        ui: &mut Ui,
        response: &Response,
        layout: &DocumentLayout,
        doc: &mut Document,
        origin: Pos2,
        _focused: bool,
    ) -> Option<String> {
        let pointer = ui.input(|i| i.pointer.interact_pos())?;
        let local = pointer - origin.to_vec2();
        let target = hit_test_local(doc, layout, local);

        if response.hovered() {
            let link = link_at(doc, target);
            if link.is_some() && ui.input(|i| i.modifiers.command) {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            } else {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
            }
        }

        let primary_pressed = ui.input(|i| i.pointer.primary_pressed());
        let primary_released = ui.input(|i| i.pointer.primary_released());
        let primary_down = ui.input(|i| i.pointer.primary_down());
        if primary_pressed {
            self.vertical_goal_x = None;
        }

        if primary_pressed && response.hovered() {
            ui.memory_mut(|memory| memory.request_focus(self.focus_id()));
            if ui.input(|i| i.modifiers.command)
                && let Some(url) = link_at(doc, target)
            {
                return Some(url);
            }
            let extend = ui.input(|i| i.modifiers.shift);
            let anchor = if extend {
                Some(doc.selection().anchor)
            } else {
                None
            };
            self.drag_anchor = Some(anchor.unwrap_or(target));
            if extend {
                doc.set_selection(Selection::new(anchor.unwrap_or(target), target));
            } else {
                doc.set_selection(Selection::caret(target));
            }
            self.scroll_to_caret = false;
            self.last_caret_activity = ui.input(|i| i.time);
        }

        if primary_down
            && response.dragged_by(PointerButton::Primary)
            && let Some(anchor) = self.drag_anchor
        {
            let target = hit_test_local(doc, layout, pointer - origin.to_vec2());
            doc.set_selection(Selection::new(anchor, target));
            self.last_caret_activity = ui.input(|i| i.time);
        }

        if primary_released {
            self.drag_anchor = None;
        }

        if response.double_clicked_by(PointerButton::Primary) {
            let pos = hit_test_local(doc, layout, pointer - origin.to_vec2());
            let range = word_range_at(doc, pos);
            doc.set_selection(Selection::new(range.start, range.end));
            self.last_caret_activity = ui.input(|i| i.time);
        } else if response.triple_clicked_by(PointerButton::Primary) {
            let pos = hit_test_local(doc, layout, pointer - origin.to_vec2());
            let block = layout
                .block_at_source_pos(pos)
                .or_else(|| layout.blocks.last());
            if let Some(block) = block {
                doc.set_selection(Selection::new(
                    block.source_range.start,
                    block.source_range.end,
                ));
                self.last_caret_activity = ui.input(|i| i.time);
            }
        }

        if response.secondary_clicked() {
            // No context menu yet, but keep the caret where the user aimed.
            doc.set_selection(Selection::caret(target));
        }

        None
    }

    pub(super) fn handle_keyboard(
        &mut self,
        ui: &mut Ui,
        layout: &DocumentLayout,
        doc: &mut Document,
        _origin: Pos2,
    ) -> bool {
        let events = ui.input(|i| i.events.clone());
        let mut changed = false;
        let mut scroll = false;
        let time = ui.input(|i| i.time);

        for event in events {
            let vertical = matches!(
                &event,
                Event::Key {
                    key: Key::ArrowUp | Key::ArrowDown | Key::PageUp | Key::PageDown,
                    pressed: true,
                    ..
                }
            );
            if !vertical {
                self.vertical_goal_x = None;
            }
            match event {
                Event::Text(text) if !text.is_empty() => {
                    let interactive_text =
                        ui.input(|i| !i.modifiers.command && !i.modifiers.ctrl && !i.modifiers.alt);
                    if interactive_text {
                        let text = normalize_newlines(&text);
                        if insert_typed(doc, &text, EditKind::Insert) {
                            changed = true;
                            scroll = true;
                            self.ime_preedit.clear();
                            if text.ends_with(['`', '~']) {
                                auto_close_fence(doc);
                            }
                        }
                    }
                }
                Event::Paste(text) => {
                    let text = normalize_newlines(&text);
                    if insert_typed(doc, &text, EditKind::Paste) {
                        changed = true;
                        scroll = true;
                    }
                }
                Event::Copy => {
                    if !doc.selection().is_caret() {
                        ui.ctx().copy_text(doc.selection_text());
                    }
                }
                Event::Cut => {
                    if !doc.selection().is_caret() {
                        ui.ctx().copy_text(doc.selection_text());
                        if delete_selection_guarded(doc) {
                            changed = true;
                            scroll = true;
                        }
                    }
                }
                Event::Ime(ime) => match ime {
                    ImeEvent::Preedit { text, .. } => {
                        self.ime_preedit = text;
                        self.last_caret_activity = time;
                    }
                    ImeEvent::Commit(text) => {
                        self.ime_preedit.clear();
                        if !text.is_empty() && insert_typed(doc, &text, EditKind::Ime) {
                            changed = true;
                            scroll = true;
                        }
                    }
                    ImeEvent::DeleteSurrounding {
                        before_chars,
                        after_chars,
                    } => {
                        for _ in 0..before_chars {
                            if doc.backspace() {
                                changed = true;
                            }
                        }
                        for _ in 0..after_chars {
                            if doc.delete_forward() {
                                changed = true;
                            }
                        }
                        scroll = true;
                    }
                    _ => {}
                },
                Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    let is_command = modifiers.command || modifiers.ctrl;
                    if is_command {
                        match key {
                            Key::Z => {
                                if modifiers.shift {
                                    changed |= doc.redo();
                                } else {
                                    changed |= doc.undo();
                                }
                                scroll = true;
                            }
                            Key::Y => {
                                changed |= doc.redo();
                                scroll = true;
                            }
                            Key::A => {
                                let len = doc.len_bytes();
                                doc.set_selection(Selection::new(0, len));
                            }
                            Key::B => {
                                changed |= toggle_wrap(ui, doc, "**");
                                scroll = true;
                            }
                            Key::I => {
                                changed |= toggle_wrap(ui, doc, "*");
                                scroll = true;
                            }
                            Key::K => {
                                changed |= insert_link(doc);
                                scroll = true;
                            }
                            Key::Backtick => {
                                changed |= toggle_wrap(ui, doc, "`");
                                scroll = true;
                            }
                            Key::ArrowLeft => {
                                move_word(doc, true, modifiers.shift);
                                scroll = true;
                            }
                            Key::ArrowRight => {
                                move_word(doc, false, modifiers.shift);
                                scroll = true;
                            }
                            Key::Home => {
                                doc.set_selection(select_to(doc, 0, modifiers.shift));
                                scroll = true;
                            }
                            Key::End => {
                                doc.set_selection(select_to(doc, doc.len_bytes(), modifiers.shift));
                                scroll = true;
                            }
                            Key::Backspace => {
                                changed |= delete_word(doc, true);
                                scroll = true;
                            }
                            Key::Delete => {
                                changed |= delete_word(doc, false);
                                scroll = true;
                            }
                            _ => {}
                        }
                        continue;
                    }

                    match key {
                        Key::Enter => {
                            changed |= handle_enter(doc, layout);
                            scroll = true;
                        }
                        Key::Tab => {
                            changed |= handle_tab(doc, modifiers.shift);
                            scroll = true;
                        }
                        Key::Backspace => {
                            changed |= backspace_guarded(doc);
                            scroll = true;
                        }
                        Key::Delete => {
                            changed |= delete_guarded(doc);
                            scroll = true;
                        }
                        Key::ArrowLeft => {
                            move_horizontal(doc, true, false, modifiers.shift);
                            scroll = true;
                        }
                        Key::ArrowRight => {
                            move_horizontal(doc, false, false, modifiers.shift);
                            scroll = true;
                        }
                        Key::ArrowUp => {
                            move_vertical(
                                ui,
                                layout,
                                doc,
                                -1.0,
                                modifiers.shift,
                                &mut self.vertical_goal_x,
                            );
                            scroll = true;
                        }
                        Key::ArrowDown => {
                            move_vertical(
                                ui,
                                layout,
                                doc,
                                1.0,
                                modifiers.shift,
                                &mut self.vertical_goal_x,
                            );
                            scroll = true;
                        }
                        Key::PageUp => {
                            let step = ui.clip_rect().height().max(120.0) * 0.9;
                            move_vertical_by(
                                ui,
                                layout,
                                doc,
                                -step,
                                modifiers.shift,
                                &mut self.vertical_goal_x,
                            );
                            scroll = true;
                        }
                        Key::PageDown => {
                            let step = ui.clip_rect().height().max(120.0) * 0.9;
                            move_vertical_by(
                                ui,
                                layout,
                                doc,
                                step,
                                modifiers.shift,
                                &mut self.vertical_goal_x,
                            );
                            scroll = true;
                        }
                        Key::Home => {
                            let pos = line_home(doc);
                            doc.set_selection(select_to(doc, pos, modifiers.shift));
                            scroll = true;
                        }
                        Key::End => {
                            let pos = line_end(doc);
                            doc.set_selection(select_to(doc, pos, modifiers.shift));
                            scroll = true;
                        }
                        Key::Escape => {
                            doc.set_selection(Selection::caret(doc.selection().head));
                            self.ime_preedit.clear();
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }

        if changed {
            self.ime_preedit.clear();
            self.last_caret_activity = time;
        }
        if scroll {
            self.scroll_to_caret = true;
            self.last_caret_activity = time;
        }
        changed
    }
}

fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn select_to(doc: &Document, pos: usize, extend: bool) -> Selection {
    if extend {
        let anchor = if doc.selection().is_caret() {
            doc.selection().head
        } else {
            doc.selection().anchor
        };
        Selection::new(anchor, pos)
    } else {
        Selection::caret(pos)
    }
}

fn move_horizontal(doc: &mut Document, left: bool, word: bool, extend: bool) {
    let selection = doc.selection();
    if !extend && !selection.is_caret() {
        doc.set_selection(Selection::caret(if left {
            selection.start()
        } else {
            selection.end()
        }));
        return;
    }
    let head = selection.head;
    let mut target = if word {
        word_boundary(doc, head, left)
    } else if left {
        doc.buffer().prev_grapheme(head)
    } else {
        doc.buffer().next_grapheme(head)
    };
    if doc.mode() == rusk_core::ViewMode::Wysiwyg {
        target = clamp_to_visible_block(doc, head, target);
    }
    doc.set_selection(select_to(doc, target, extend));
}

/// Keep horizontal caret movement out of hidden block syntax such as `# `,
/// `- `, `> ` or code fences. Inline syntax can still be reached (and is
/// revealed by the projection), but block markers are source-mode territory.
/// A fenced code block is shown as raw source while the caret is inside it,
/// so nothing in it is hidden syntax that the caret must be kept out of.
fn in_raw_code(doc: &mut Document, pos: usize) -> bool {
    doc.parsed().blocks.iter().any(|b| {
        matches!(&b.kind, BlockKind::CodeBlock(info) if info.fenced)
            && b.range.start <= pos
            && pos <= b.edit_end()
    })
}

fn clamp_to_visible_block(doc: &mut Document, from: usize, to: usize) -> usize {
    if in_raw_code(doc, from) {
        return to;
    }
    let parsed = doc.parsed();
    let Some(index) = parsed
        .blocks
        .iter()
        .position(|b| b.content.start <= from && from <= b.content.end)
    else {
        return to;
    };
    let block = &parsed.blocks[index];
    let has_hidden_prefix = !block.prefix.is_empty()
        || block.list_marker.is_some()
        || block.quote_depth > 0
        || block.list_depth > 0;
    if has_hidden_prefix && to < block.content.start {
        return parsed
            .blocks
            .iter()
            .take(index)
            .rev()
            .find(|b| b.content.end <= block.content.start)
            .map_or(block.content.start, |b| b.content.end);
    }
    if to > block.content.end && from <= block.content.end {
        return parsed
            .blocks
            .iter()
            .skip(index + 1)
            .find(|b| b.content.start >= block.content.end)
            .map_or(block.content.end, |b| b.content.start);
    }
    to
}

/// A completed opening fence that swallowed the text below it gets a closing
/// fence right away, so the rest of the document stays outside the block and
/// a later fence can never be paired with it. A fence typed at the end of the
/// document is left alone: it has nothing to swallow, and its author is still
/// writing the block by hand.
fn auto_close_fence(doc: &mut Document) -> bool {
    if doc.mode() != rusk_core::ViewMode::Wysiwyg || !doc.selection().is_caret() {
        return false;
    }
    let head = doc.selection().head;
    let source = doc.buffer().as_str();
    let line = doc.buffer().line_of_byte(head);
    let line_start = doc.buffer().line_start_byte(line);
    let line_end = source[line_start..]
        .find('\n')
        .map_or(source.len(), |offset| line_start + offset);
    let content_end = if source[line_start..line_end].ends_with('\r') {
        line_end - 1
    } else {
        line_end
    };
    let eol = if source.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    if head != content_end {
        return false;
    }
    let line_text = &source[line_start..content_end];
    let Some((indent, fence, run)) = fence_run(line_text) else {
        return false;
    };
    if fence == '`' && line_text[indent + run..].contains('`') {
        return false;
    }
    let Some((closing, empty)) = fenced_block(doc, line_start, line_end) else {
        return false;
    };
    let fence_text = fence.to_string().repeat(run);
    let indent_text = &line_text[..indent];
    if !empty {
        return doc.apply_transaction(Transaction::new(
            vec![Edit::insert(
                content_end,
                format!("{eol}{indent_text}{fence_text}"),
            )],
            Selection::caret(head),
            EditKind::Insert,
        ));
    }
    // A finished empty block keeps its closer exactly as long as its opener.
    let Some(closing) = closing else {
        return false;
    };
    let closer_line_start = source[..closing.start]
        .rfind('\n')
        .map_or(0, |offset| offset + 1);
    let closer_line_end = source[closer_line_start..]
        .find('\n')
        .map_or(source.len(), |offset| closer_line_start + offset);
    let closer_text = source[closer_line_start..closer_line_end].trim_end_matches('\r');
    let Some((closer_indent, closer_fence, closer_run)) = fence_run(closer_text) else {
        return false;
    };
    if closer_fence != fence || closer_run >= run {
        return false;
    }
    let run_range =
        closer_line_start + closer_indent..closer_line_start + closer_indent + closer_run;
    doc.apply_transaction(Transaction::new(
        vec![Edit::replace(
            run_range,
            format!("{}{fence_text}", &closer_text[..closer_indent]),
        )],
        Selection::caret(head),
        EditKind::Insert,
    ))
}

/// Deleting the fence run of an empty fenced block also removes the closing
/// fence that was added for it, so an abandoned fence cannot leave an orphan
/// line that swallows whatever follows.
fn collapse_fence(doc: &mut Document) -> bool {
    if doc.mode() != rusk_core::ViewMode::Wysiwyg || !doc.selection().is_caret() {
        return false;
    }
    let head = doc.selection().head;
    if head == 0 {
        return false;
    }
    let source = doc.buffer().as_str();
    let line = doc.buffer().line_of_byte(head);
    let line_start = doc.buffer().line_start_byte(line);
    let line_end = source[line_start..]
        .find('\n')
        .map_or(source.len(), |offset| line_start + offset);
    let line_text = source[line_start..line_end].trim_end_matches('\r');
    let Some((indent, fence, run)) = fence_run(line_text) else {
        return false;
    };
    let run_start = line_start + indent;
    if run != 3 || head <= run_start || head > run_start + run {
        return false;
    }
    if !source[..head].ends_with(fence) {
        return false;
    }
    let Some((closing, empty)) = fenced_block(doc, line_start, line_end) else {
        return false;
    };
    let Some(closing) = closing else {
        return false;
    };
    if !empty || line_end >= source.len() {
        return false;
    }
    let closer_start = line_end + 1;
    let closer_line_start = source[..closing.start]
        .rfind('\n')
        .map_or(0, |offset| offset + 1);
    if closer_line_start != closer_start {
        return false;
    }
    let closer_line_end = source[closer_start..]
        .find('\n')
        .map_or(source.len(), |offset| closer_start + offset);
    let closer_text = source[closer_start..closer_line_end].trim_end_matches('\r');
    let closer = closer_text.trim();
    if closer.chars().count() < 3 || !closer.chars().all(|c| c == fence) {
        return false;
    }
    let removed = head - fence.len_utf8();
    let cut_end = (closer_line_end + usize::from(closer_line_end < source.len())).min(source.len());
    doc.apply_transaction(Transaction::new(
        vec![Edit::delete(removed..head), Edit::delete(line_end..cut_end)],
        Selection::caret(removed),
        EditKind::Delete,
    ))
}

/// The parsed fenced code block starting on the line `line_start..line_end`,
/// as its closing fence range (when present) and whether its content is empty.
fn fenced_block(
    doc: &mut Document,
    line_start: usize,
    line_end: usize,
) -> Option<(Option<Range<usize>>, bool)> {
    let parsed = doc.parsed();
    let block = parsed.blocks.iter().find(|b| {
        matches!(&b.kind, BlockKind::CodeBlock(info) if info.fenced)
            && b.range.start >= line_start
            && b.range.start <= line_end
    })?;
    let closing = block
        .prefix
        .iter()
        .filter(|p| p.kind == PrefixKind::Fence)
        .nth(1)
        .map(|p| p.range.clone());
    Some((closing, block.content.start == block.content.end))
}

/// Leading fence run of a line: indentation width, fence character, run length.
fn fence_run(line: &str) -> Option<(usize, char, usize)> {
    let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let fence = rest.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let count = rest.chars().take_while(|c| *c == fence).count();
    if count < 3 {
        return None;
    }
    Some((indent, fence, count))
}

fn remove_block_prefix_at(doc: &mut Document, head: usize) -> bool {
    let text = doc.text();
    let ranges = {
        let parsed = doc.parsed();
        let Some(block) = parsed.blocks.iter().find(|block| {
            block.range.start <= head
                && head <= block.range.end
                && (block.content.start == head
                    || block.prefix.iter().any(|prefix| prefix.range.end == head))
        }) else {
            return false;
        };
        let has_list = block.list_marker.is_some()
            || block
                .prefix
                .iter()
                .any(|prefix| matches!(prefix.kind, PrefixKind::ListMarker | PrefixKind::Task));
        if has_list {
            return false;
        }
        let mut ranges: Vec<Range<usize>> = block
            .prefix
            .iter()
            .filter(|prefix| matches!(prefix.kind, PrefixKind::Heading | PrefixKind::Quote))
            .map(|prefix| prefix.range.clone())
            .collect();
        if head == block.content.start
            && (block.quote_depth > 0
                || block
                    .prefix
                    .iter()
                    .any(|prefix| prefix.kind == PrefixKind::Heading))
        {
            let line_start = text[..head].rfind('\n').map_or(0, |offset| offset + 1);
            ranges.push(line_start..head);
        }
        if block.quote_depth > 0 {
            ranges.extend(quote_prefixes(&text, block.range.start, block.range.end));
        }
        ranges.sort_unstable_by_key(|range| (range.start, range.end));
        ranges.dedup();
        ranges
    };
    if ranges.is_empty() {
        return false;
    }
    remove_marker_ranges(doc, ranges, Selection::caret(head))
}

/// Leading `>` markers on every line the blockquote occupies. The parser keeps
/// those bytes outside `block.range`, so they need a small source scan.
fn quote_prefixes(text: &str, start: usize, end: usize) -> Vec<Range<usize>> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut ranges = Vec::new();
    let mut line_start = text[..start.min(text.len())]
        .rfind('\n')
        .map_or(0, |offset| offset + 1);
    let last = end.saturating_sub(1).min(text.len().saturating_sub(1));
    while line_start <= last {
        let line_end = text[line_start..]
            .find('\n')
            .map_or(text.len(), |offset| line_start + offset);
        let line = &text[line_start..line_end];
        let mut index = line.len() - line.trim_start_matches(' ').len();
        if index > 3 {
            index = 0;
        }
        let original = index;
        while line.as_bytes().get(index) == Some(&b'>') {
            index += 1;
            if line.as_bytes().get(index) == Some(&b' ') {
                index += 1;
            }
        }
        if index > original {
            ranges.push(line_start..line_start + index);
        }
        if line_end >= text.len() {
            break;
        }
        line_start = line_end + 1;
    }
    ranges
}

fn remove_inline_formatting_at(doc: &mut Document, head: usize, backspace: bool) -> bool {
    let target = {
        let parsed = doc.parsed();
        let mut found = None;
        'blocks: for block in &parsed.blocks {
            if block.content.start > head || head > block.content.end {
                continue;
            }
            for marker in &block.markers {
                if !matches!(
                    marker.kind,
                    MarkerKind::Strong
                        | MarkerKind::Emphasis
                        | MarkerKind::Strikethrough
                        | MarkerKind::Code
                        | MarkerKind::Link
                        | MarkerKind::Math
                ) {
                    continue;
                }
                let boundary = if backspace {
                    marker.range.start == marker.outer.start && head == marker.range.end
                } else {
                    marker.range.end == marker.outer.end && head == marker.range.start
                };
                if boundary {
                    found = Some((marker.outer.clone(), marker.kind));
                    break 'blocks;
                }
            }
        }
        found
    };
    let Some((outer, kind)) = target else {
        return false;
    };
    let mut ranges: Vec<Range<usize>> = doc
        .parsed()
        .blocks
        .iter()
        .flat_map(|block| &block.markers)
        .filter(|marker| marker.kind == kind && marker.outer == outer)
        .map(|marker| marker.range.clone())
        .collect();
    ranges.sort_unstable_by_key(|range| (range.start, range.end));
    ranges.dedup();
    if ranges.len() < 2 {
        return false;
    }
    remove_marker_ranges(doc, ranges, Selection::caret(head))
}

fn delete_selection_guarded(doc: &mut Document) -> bool {
    let selection = doc.selection();
    if selection.is_caret() {
        return false;
    }
    if let Some(outer) = formatting_outer_for_inner(doc, &selection.range()) {
        let head = outer.start;
        return doc.apply_transaction(Transaction::new(
            vec![Edit::delete(outer)],
            Selection::caret(head),
            EditKind::Delete,
        ));
    }
    doc.delete_selection(EditKind::Delete)
}

/// If the selection is exactly the visible text between a marker pair, remove
/// the whole construct. Deleting only the text would leave empty `****`,
/// `[](url)` or ``` `` ``` behind.
fn formatting_outer_for_inner(doc: &mut Document, range: &Range<usize>) -> Option<Range<usize>> {
    let mut best: Option<Range<usize>> = None;
    for block in &doc.parsed().blocks {
        let mut outers: Vec<Range<usize>> = block
            .markers
            .iter()
            .filter(|marker| marker.kind != MarkerKind::Image)
            .map(|marker| marker.outer.clone())
            .collect();
        outers.sort_unstable_by_key(|outer| (outer.start, outer.end));
        outers.dedup();
        for outer in outers {
            let open = block.markers.iter().find(|marker| {
                marker.kind != MarkerKind::Image
                    && marker.outer == outer
                    && marker.range.start == outer.start
            });
            let close = block.markers.iter().find(|marker| {
                marker.kind != MarkerKind::Image
                    && marker.outer == outer
                    && marker.range.end == outer.end
            });
            let (Some(open), Some(close)) = (open, close) else {
                continue;
            };
            if *range == (open.range.end..close.range.start)
                && best.as_ref().is_none_or(|best| best.len() > outer.len())
            {
                best = Some(outer);
            }
        }
    }
    best
}

fn backspace_guarded(doc: &mut Document) -> bool {
    if !doc.selection().is_caret() {
        return delete_selection_guarded(doc);
    }
    let head = doc.selection().head;
    if doc.mode() == rusk_core::ViewMode::Wysiwyg && collapse_fence(doc) {
        return true;
    }
    if doc.mode() == rusk_core::ViewMode::Wysiwyg && in_raw_code(doc, head) {
        return doc.backspace();
    }
    if doc.mode() == rusk_core::ViewMode::Wysiwyg && remove_block_prefix_at(doc, head) {
        return true;
    }
    if doc.mode() == rusk_core::ViewMode::Wysiwyg && remove_inline_formatting_at(doc, head, true) {
        return true;
    }
    if doc.mode() == rusk_core::ViewMode::Wysiwyg
        && let Some((start, end)) = line_break_ending_at(&doc.buffer().as_str(), head)
        && deleting_break_merges_blocks(doc, start, end)
    {
        return false;
    }
    if doc.mode() == rusk_core::ViewMode::Wysiwyg
        && let Some((content_start, content_end, hidden)) = block_at_source(doc, head)
        && hidden
        && head == content_start
        && head == content_end
    {
        // An empty item only exists as its marker ("- ", "> ", "# ").
        // Backspace removes the marker and leaves a plain line, the same
        // exit the Enter key performs; the generic guard would jump into
        // the previous block and leave the item behind.
        let line = doc.buffer().line_of_byte(head);
        let line_start = doc.buffer().line_start_byte(line);
        if line_start < head {
            let selection = Selection::caret(line_start);
            return doc.apply_transaction(Transaction::new(
                vec![Edit::delete(line_start..head)],
                selection,
                EditKind::Structural,
            ));
        }
    }
    if doc.mode() == rusk_core::ViewMode::Wysiwyg
        && let Some((content_start, _, hidden)) = block_at_source(doc, head)
        && head == content_start
        && hidden
    {
        let target = clamp_to_visible_block(doc, head, head.saturating_sub(1));
        doc.set_selection(Selection::caret(target));
        return false;
    }
    doc.backspace()
}

fn delete_guarded(doc: &mut Document) -> bool {
    if !doc.selection().is_caret() {
        return delete_selection_guarded(doc);
    }
    let head = doc.selection().head;
    if doc.mode() == rusk_core::ViewMode::Wysiwyg && in_raw_code(doc, head) {
        return doc.delete_forward();
    }
    if doc.mode() == rusk_core::ViewMode::Wysiwyg && remove_inline_formatting_at(doc, head, false) {
        return true;
    }
    if doc.mode() == rusk_core::ViewMode::Wysiwyg
        && let Some((start, end)) = line_break_starting_at(&doc.buffer().as_str(), head)
        && deleting_break_merges_blocks(doc, start, end)
    {
        return false;
    }
    if doc.mode() == rusk_core::ViewMode::Wysiwyg
        && let Some((_, content_end, hidden)) = block_at_source(doc, head)
        && head == content_end
        && hidden
    {
        let target = clamp_to_visible_block(doc, head, head.saturating_add(1));
        doc.set_selection(Selection::caret(target));
        return false;
    }
    doc.delete_forward()
}

fn block_at_source(doc: &mut Document, pos: usize) -> Option<(usize, usize, bool)> {
    let parsed = doc.parsed();
    parsed
        .blocks
        .iter()
        .find(|b| b.content.start <= pos && pos <= b.content.end)
        .map(|b| {
            let hidden = !b.prefix.is_empty()
                || b.list_marker.is_some()
                || b.quote_depth > 0
                || b.list_depth > 0;
            (b.content.start, b.content.end, hidden)
        })
}

fn move_word(doc: &mut Document, left: bool, extend: bool) {
    move_horizontal(doc, left, true, extend);
}

fn word_boundary(doc: &Document, at: usize, left: bool) -> usize {
    let buffer = doc.buffer();
    let text = buffer.as_str();
    let at = buffer.snap_byte(at);
    let mut index = at;
    if left {
        if index == 0 {
            return 0;
        }
        // Skip whitespace, then the word.
        while index > 0 {
            let prev = buffer.prev_grapheme(index);
            let ch = text[prev..index].chars().next().unwrap_or(' ');
            if !ch.is_whitespace() {
                break;
            }
            index = prev;
        }
        while index > 0 {
            let prev = buffer.prev_grapheme(index);
            let ch = text[prev..index].chars().next().unwrap_or(' ');
            if ch.is_whitespace() {
                break;
            }
            index = prev;
        }
    } else {
        let len = text.len();
        while index < len {
            let next = buffer.next_grapheme(index);
            let ch = text[index..next].chars().next().unwrap_or(' ');
            if !ch.is_whitespace() {
                break;
            }
            index = next;
        }
        while index < len {
            let next = buffer.next_grapheme(index);
            let ch = text[index..next].chars().next().unwrap_or(' ');
            if ch.is_whitespace() {
                break;
            }
            index = next;
        }
    }
    index
}

fn delete_word(doc: &mut Document, left: bool) -> bool {
    if !doc.selection().is_caret() {
        return delete_selection_guarded(doc);
    }
    let head = doc.selection().head;
    let target = word_boundary(doc, head, left);
    if target == head {
        return false;
    }
    let mut start = target.min(head);
    let mut end = target.max(head);
    if doc.mode() == rusk_core::ViewMode::Wysiwyg {
        let mut clipped = false;
        for block in &doc.parsed().blocks {
            for marker in &block.markers {
                if left
                    && marker.range.end <= head
                    && marker.range.start >= start
                    && marker.range.end <= end
                {
                    start = marker.range.end;
                    clipped = true;
                } else if !left
                    && marker.range.start >= head
                    && marker.range.end <= end
                    && marker.range.start >= start
                {
                    end = marker.range.start;
                    clipped = true;
                }
            }
        }
        if clipped {
            if start >= end {
                return false;
            }
            doc.set_selection(Selection::new(start, end));
            return delete_selection_guarded(doc);
        }
    }
    doc.apply_transaction(Transaction::new(
        vec![Edit::delete(start..end)],
        Selection::caret(target.min(head)),
        EditKind::Delete,
    ))
}

fn move_vertical(
    ui: &mut Ui,
    layout: &DocumentLayout,
    doc: &mut Document,
    direction: f32,
    extend: bool,
    goal_x: &mut Option<f32>,
) {
    let Some(caret) = paint::caret_rect_local(layout, doc) else {
        return;
    };
    let delta = caret.height().max(20.0) * 1.15 * direction;
    move_vertical_by(ui, layout, doc, delta, extend, goal_x);
}

fn move_vertical_by(
    _ui: &mut Ui,
    layout: &DocumentLayout,
    doc: &mut Document,
    delta: f32,
    extend: bool,
    goal_x: &mut Option<f32>,
) {
    let Some(caret) = paint::caret_rect_local(layout, doc) else {
        return;
    };
    let x = goal_x
        .get_or_insert_with(|| caret.left().max(layout.content_left))
        .max(layout.content_left);
    let target = Pos2::new(x, caret.center().y + delta);
    if target.y < TOP_MARGIN {
        let pos = if delta < 0.0 { 0 } else { doc.len_bytes() };
        doc.set_selection(select_to(doc, pos, extend));
        return;
    }
    let head = doc.selection().head;
    let len = doc.len_bytes();
    let mut pos = if delta < 0.0 && caret.top() < 4.0 {
        0
    } else {
        hit_test_local(doc, layout, target)
    };
    if stalled(layout, head, pos) {
        // Padded blocks (code cards, table source, images) and rules map a
        // step from their first/last row back into the same row. Step to the
        // neighbouring block instead of standing still.
        if let Some(index) = block_index_at(layout, head) {
            pos = neighbour_pos(layout, len, index, delta);
        }
    }
    // Vertical movement must not unfold a blank separator under the caret:
    // folded separators are spacing, not rows to stop on.
    pos = skip_folded_blanks(layout, len, pos, delta);
    doc.set_selection(select_to(doc, pos, extend));
}

/// True when a vertical step came back to where it started, either at the
/// same byte or on the same visual row of the same block.
fn stalled(layout: &DocumentLayout, head: usize, pos: usize) -> bool {
    if pos == head {
        return true;
    }
    let (Some(from), Some(to)) = (block_index_at(layout, head), block_index_at(layout, pos)) else {
        return false;
    };
    if from != to {
        return false;
    }
    let block = &layout.blocks[from];
    let (Some(start), Some(end)) = (
        cursor_row_top(block, head),
        cursor_row_top(&layout.blocks[to], pos),
    ) else {
        return false;
    };
    start == end
}

fn cursor_row_top(block: &LaidBlock, pos: usize) -> Option<f32> {
    let galley = block.galley.as_ref()?;
    let display = block.projection.source_to_display(pos, true);
    Some(
        galley
            .pos_from_cursor(egui::text::CCursor::new(display))
            .top(),
    )
}

fn block_index_at(layout: &DocumentLayout, pos: usize) -> Option<usize> {
    layout
        .block_at_source_pos(pos)
        .and_then(|block| layout.blocks.iter().position(|b| std::ptr::eq(b, block)))
}

fn neighbour_pos(layout: &DocumentLayout, len: usize, index: usize, delta: f32) -> usize {
    if delta < 0.0 {
        layout.blocks[..index]
            .last()
            .map_or(0, |b| b.source_range.end)
    } else {
        layout
            .blocks
            .get(index + 1)
            .map_or(len, |b| b.source_range.start)
    }
}

fn skip_folded_blanks(layout: &DocumentLayout, len: usize, mut pos: usize, delta: f32) -> usize {
    for _ in 0..layout.blocks.len() {
        let Some(index) = block_index_at(layout, pos) else {
            break;
        };
        if !is_folded_blank(layout, index) {
            break;
        }
        let next = neighbour_pos(layout, len, index, delta);
        if next == pos {
            break;
        }
        pos = next;
    }
    pos
}

fn is_folded_blank(layout: &DocumentLayout, index: usize) -> bool {
    let block = &layout.blocks[index];
    match block.kind {
        LaidKind::Spacer => true,
        LaidKind::Blank => {
            block.quote_depth == 0
                && !matches!(
                    layout.blocks[..index].last().map(|b| &b.kind),
                    Some(LaidKind::Blank | LaidKind::Spacer)
                )
                && layout.blocks[index + 1..]
                    .iter()
                    .any(|b| !matches!(b.kind, LaidKind::Blank))
        }
        _ => false,
    }
}

const TOP_MARGIN: f32 = 8.0;

fn line_home(doc: &mut Document) -> usize {
    let head = doc.selection().head;
    if doc.mode() == rusk_core::ViewMode::Wysiwyg
        && !in_raw_code(doc, head)
        && let Some((start, _, _)) = block_at_source(doc, head)
    {
        return start;
    }
    let line = doc.buffer().line_of_byte(head);
    let line_start = doc.buffer().line_start_byte(line);
    let text = doc.buffer().as_str();
    let line_end = text[line_start..]
        .find('\n')
        .map_or(text.len(), |offset| line_start + offset);
    let before = &text[line_start..head.min(line_end)];
    let indent = before.len() - before.trim_start_matches([' ', '\t']).len();
    if indent > 0 && head <= line_start + indent {
        line_start
    } else {
        line_start + indent
    }
}

fn line_end(doc: &mut Document) -> usize {
    let head = doc.selection().head;
    if doc.mode() == rusk_core::ViewMode::Wysiwyg
        && !in_raw_code(doc, head)
        && let Some((_, end, _)) = block_at_source(doc, head)
    {
        return end;
    }
    let line = doc.buffer().line_of_byte(head);
    let text = doc.buffer().as_str();
    let start = doc.buffer().line_start_byte(line);
    let mut end = text[start..]
        .find('\n')
        .map_or(text.len(), |offset| start + offset);
    if end > start && text.as_bytes()[end - 1] == b'\r' {
        end -= 1;
    }
    end
}

/// Insert typed, pasted, or IME-committed text, without letting a caret on an
/// empty line change what the surrounding Markdown means. A blank line between
/// two blocks is the one line Markdown requires (the layout folds it into
/// spacing), so text typed there becomes a paragraph of its own with a blank
/// line on either side; on a visible empty row only the block below is kept
/// apart. Without this the parser silently absorbs the text into a neighbouring
/// block, or a line above `---` / `===` turns it into a setext heading.
fn insert_typed(doc: &mut Document, text: &str, kind: EditKind) -> bool {
    if text.is_empty() {
        return false;
    }
    let caret = doc.selection().head;
    let on_empty_line_start = doc.selection().is_caret()
        && caret
            == doc
                .buffer()
                .line_start_byte(doc.buffer().line_of_byte(caret))
        && doc.text()[caret..]
            .split('\n')
            .next()
            .is_some_and(|line| line.trim().is_empty());
    if !on_empty_line_start || doc.mode() != rusk_core::ViewMode::Wysiwyg || in_raw_code(doc, caret)
    {
        return doc.insert_text(text, kind);
    }
    let collapsed = collapsed_blank_at(doc, caret);
    let block_below = a_block_starts_below(doc, caret);
    if !collapsed && !block_below {
        return doc.insert_text(text, kind);
    }
    let newline = if doc.text().contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut insertion = String::new();
    if collapsed {
        insertion.push_str(newline);
    }
    insertion.push_str(text);
    let caret_after = caret + insertion.len();
    if block_below {
        insertion.push_str(newline);
    }
    doc.apply_transaction(Transaction::new(
        vec![Edit::replace(caret..caret, &insertion)],
        Selection::caret(caret_after),
        kind,
    ))
}

/// True when `at` is the blank line the layout folds into spacing: the first
/// blank line of a run between two blocks.
fn collapsed_blank_at(doc: &mut Document, at: usize) -> bool {
    if at == 0 {
        return false;
    }
    let text = doc.buffer().as_str();
    if super::layout::previous_line_is_blank(&text, at) {
        return false;
    }
    let line_end = text[at..]
        .find('\n')
        .map_or(text.len(), |offset| at + offset);
    let blocks: Vec<(usize, usize)> = doc
        .parsed()
        .blocks
        .iter()
        .map(|block| (block.range.start, block.range.end))
        .collect();
    // A blank line inside a code or HTML block is content, not a separator.
    let inside_block = blocks.iter().any(|(start, end)| *start <= at && at < *end);
    !inside_block && blocks.iter().any(|(start, _)| *start > line_end)
}

/// True when the line after the caret's line starts a block. Text typed on the
/// blank line must then add a line break below itself, or it becomes a lazy
/// continuation of that block (or its setext underline).
fn a_block_starts_below(doc: &mut Document, at: usize) -> bool {
    let line = doc.buffer().line_of_byte(at);
    let starts: Vec<usize> = doc
        .parsed()
        .blocks
        .iter()
        .map(|block| block.range.start)
        .collect();
    let buffer = doc.buffer();
    starts
        .iter()
        .any(|start| buffer.line_of_byte(*start) == line + 1)
}

fn setext_underline(line: &str) -> bool {
    !line.is_empty()
        && (line.bytes().all(|byte| byte == b'-') || line.bytes().all(|byte| byte == b'='))
}

/// The line break that ends right before `at`, when `at` sits at a line start.
fn line_break_ending_at(text: &str, at: usize) -> Option<(usize, usize)> {
    if at == 0 {
        return None;
    }
    if text[..at].ends_with("\r\n") {
        Some((at - 2, at))
    } else if text[..at].ends_with('\n') {
        Some((at - 1, at))
    } else {
        None
    }
}

/// The line break that starts at `at`.
fn line_break_starting_at(text: &str, at: usize) -> Option<(usize, usize)> {
    if text[at..].starts_with("\r\n") {
        Some((at, at + 2))
    } else if text[at..].starts_with('\n') {
        Some((at, at + 1))
    } else {
        None
    }
}

fn newline_width(text: &str, at: usize) -> usize {
    line_break_starting_at(text, at).map_or(0, |(start, end)| end - start)
}

/// True when deleting the line break `start..end` lets the parser read two
/// separate blocks as one: text directly under a table becomes one of its rows,
/// or a text line directly above a `---` / `===` line becomes a setext heading.
/// Both blank lines are required by Markdown, so the deletion is refused rather
/// than silently reshaping the document.
fn deleting_break_merges_blocks(doc: &mut Document, start: usize, end: usize) -> bool {
    let text = doc.buffer().as_str();
    let joined_start = text[..start].rfind('\n').map_or(0, |offset| offset + 1);
    let joined_end = text[end..]
        .find('\n')
        .map_or(text.len(), |offset| end + offset);
    let before = text[joined_start..start].trim_end_matches('\r');
    let after = text[end..joined_end].trim_end_matches('\r');
    let joined = format!("{before}{after}");
    if joined.trim().is_empty() {
        return false;
    }
    let next = text
        .get(joined_end + 1..)
        .map(|rest| {
            rest.split('\n')
                .next()
                .unwrap_or("")
                .trim_end_matches('\r')
                .trim()
        })
        .filter(|line| !line.is_empty());

    if setext_underline(joined.trim()) {
        // The merged line is the underline itself; the line above it decides.
        let above_end = joined_start.saturating_sub(1);
        let above_start = text[..above_end].rfind('\n').map_or(0, |offset| offset + 1);
        return joined_start > 0 && !text[above_start..above_end].trim().is_empty();
    }
    if next.is_some_and(setext_underline) && after.trim().is_empty() {
        return true;
    }

    let parsed = doc.parsed();
    parsed.blocks.iter().any(|block| {
        if !matches!(block.kind, BlockKind::Table) || block.content.is_empty() {
            return false;
        }
        let last_line_start = text[..block.content.end]
            .rfind('\n')
            .map_or(0, |offset| offset + 1);
        if joined_start == last_line_start {
            // The break ended the table's last row, so the next line now sits
            // directly under the table.
            return next.is_some();
        }
        // The merged line now sits directly under the table's last line.
        let gap = newline_width(&text, block.content.end);
        gap > 0 && joined_start == block.content.end + gap
    })
}

fn handle_enter(doc: &mut Document, layout: &DocumentLayout) -> bool {
    if !doc.selection().is_caret() {
        delete_selection_guarded(doc);
    }
    let caret = doc.selection().head;
    let line = doc.buffer().line_of_byte(caret);
    let line_start = doc.buffer().line_start_byte(line);
    let text = doc.buffer().as_str();
    let line_end = text[line_start..]
        .find('\n')
        .map_or(text.len(), |offset| line_start + offset);
    let line_text = &text[line_start..line_end];
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };

    // Code blocks keep literal newlines.
    let in_code = layout
        .blocks
        .iter()
        .find(|b| b.source_range.start <= caret && caret <= b.source_range.end)
        .is_some_and(|b| {
            matches!(
                b.kind,
                LaidKind::Code { .. } | LaidKind::CodeSource | LaidKind::TableSource { .. }
            )
        });
    if in_code {
        return doc.insert_text(newline, EditKind::Insert);
    }

    let Some(prefix) = parse_prefix(line_text) else {
        return doc.insert_text(newline, EditKind::Insert);
    };
    let content = &line_text[prefix.content_start..];
    if content.trim().is_empty() {
        // Exit the list/quote rather than creating an endless empty item.
        let range = line_start..line_end;
        let selection = Selection::caret(line_start);
        return doc.apply_transaction(Transaction::new(
            vec![Edit::delete(range)],
            selection,
            EditKind::Structural,
        ));
    }
    let continuation = prefix.continuation();
    let insertion = format!("{newline}{continuation}");
    doc.insert_text(&insertion, EditKind::Insert)
}

struct LinePrefix {
    content_start: usize,
    indent: String,
    quote: String,
    list: Option<ListPrefix>,
}

struct ListPrefix {
    marker: String,
    number: Option<u64>,
    delimiter: char,
    task: Option<bool>,
}

impl LinePrefix {
    fn continuation(&self) -> String {
        let mut out = self.indent.clone();
        out.push_str(&self.quote);
        if let Some(list) = &self.list {
            if let Some(number) = list.number {
                out.push_str(&format!("{}{} ", number + 1, list.delimiter));
            } else {
                out.push_str(&list.marker);
                out.push(' ');
            }
            if list.task.is_some() {
                out.push_str("[ ] ");
            }
        }
        out
    }
}

fn parse_prefix(line: &str) -> Option<LinePrefix> {
    let mut index = 0usize;
    let indent_len = line.len() - line.trim_start_matches([' ', '\t']).len();
    let indent = line[..indent_len].to_owned();
    index += indent_len;

    let mut quote = String::new();
    loop {
        if line[index..].starts_with("> ") {
            quote.push_str("> ");
            index += 2;
        } else if line[index..].starts_with('>') {
            quote.push('>');
            index += 1;
            if line[index..].starts_with(' ') {
                quote.push(' ');
                index += 1;
            }
        } else {
            break;
        }
    }

    let list_start = index;
    let rest = &line[index..];
    let mut marker = None;
    let mut number = None;
    let mut delimiter = '.';
    let mut task = None;

    if let Some(first) = rest.chars().next() {
        if matches!(first, '-' | '+' | '*') {
            marker = Some(first.to_string());
            index += first.len_utf8();
        } else {
            let digits = rest.chars().take_while(char::is_ascii_digit).count();
            if digits > 0 {
                let after_digits = index + digits;
                if after_digits < line.len() {
                    let ch = line[after_digits..].chars().next().unwrap_or('.');
                    if ch == '.' || ch == ')' {
                        number = line[index..after_digits].parse().ok();
                        delimiter = ch;
                        marker = Some(line[index..after_digits + 1].to_owned());
                        index = after_digits + 1;
                    }
                }
            }
        }
    }

    let list = marker.map(|marker| {
        while index < line.len() && matches!(line.as_bytes()[index], b' ' | b'\t') {
            index += 1;
        }
        if index + 2 < line.len()
            && line.as_bytes()[index] == b'['
            && line.as_bytes()[index + 2] == b']'
            && matches!(line.as_bytes()[index + 1], b' ' | b'x' | b'X' | b'-')
        {
            task = Some(line.as_bytes()[index + 1] != b' ');
            index += 3;
            while index < line.len() && matches!(line.as_bytes()[index], b' ' | b'\t') {
                index += 1;
            }
        }
        ListPrefix {
            marker,
            number,
            delimiter,
            task,
        }
    });

    if list.is_none() && quote.is_empty() {
        return None;
    }
    let _ = list_start;
    Some(LinePrefix {
        content_start: index,
        indent,
        quote,
        list,
    })
}

fn list_indent_width(line: &str) -> usize {
    let Some(prefix) = parse_prefix(line) else {
        return 2;
    };
    let Some(list) = prefix.list else {
        return 2;
    };
    if let Some(number) = list.number {
        format!("{number}{} ", list.delimiter).len()
    } else {
        2
    }
}

fn handle_tab(doc: &mut Document, outdent: bool) -> bool {
    let selection = doc.selection();
    let (line_starts, line_texts) = {
        let buffer = doc.buffer();
        let start_line = buffer.line_of_byte(selection.start());
        let mut end_line = buffer.line_of_byte(selection.end());
        if end_line > start_line
            && selection.end() == buffer.line_start_byte(end_line)
            && !selection.is_caret()
        {
            end_line -= 1;
        }
        let mut line_starts = Vec::new();
        let mut line_texts = Vec::new();
        for line in start_line..=end_line {
            line_starts.push(buffer.line_start_byte(line));
            line_texts.push(buffer.line_text(line));
        }
        (line_starts, line_texts)
    };
    if selection.is_caret() && !outdent {
        let line = doc.buffer().line_of_byte(selection.head);
        let line_text = doc.buffer().line_text(line);
        let is_block_prefix = parse_prefix(&line_text).is_some();
        if !is_block_prefix {
            return doc.insert_text("  ", EditKind::Insert);
        }
    }

    let mut edits = Vec::new();
    let mut start_shift = 0isize;
    let mut end_shift = 0isize;
    for (line, line_start) in line_starts.iter().copied().enumerate() {
        let line_text = line_texts.get(line).map_or("", String::as_str);
        if outdent {
            let mut remove = 0usize;
            while remove < 4 && line_text.as_bytes().get(remove) == Some(&b' ') {
                remove += 1;
            }
            if remove > 0 {
                edits.push(Edit::delete(line_start..line_start + remove));
                if line_start <= selection.start() {
                    start_shift -= remove as isize;
                }
                if line_start < selection.end() {
                    end_shift -= remove as isize;
                }
            }
        } else {
            let indent = list_indent_width(line_text);
            edits.push(Edit::insert(line_start, " ".repeat(indent)));
            if line_start <= selection.start() {
                start_shift += indent as isize;
            }
            if line_start < selection.end() {
                end_shift += indent as isize;
            }
        }
    }
    if edits.is_empty() {
        return false;
    }
    let selection_after = Selection::new(
        (selection.start() as isize + start_shift).max(0) as usize,
        (selection.end() as isize + end_shift).max(0) as usize,
    );
    doc.apply_transaction(Transaction::new(edits, selection_after, EditKind::Format))
}

fn toggle_wrap(ui: &mut Ui, doc: &mut Document, marker: &str) -> bool {
    let _ = ui;
    let Some(kind) = marker_kind(marker) else {
        return false;
    };
    let selection = doc.selection();
    if doc.mode() == rusk_core::ViewMode::Wysiwyg && in_raw_code(doc, selection.head) {
        return false;
    }

    let ranges = if doc.mode() == rusk_core::ViewMode::Wysiwyg {
        formatting_ranges(doc, kind, selection)
    } else {
        Vec::new()
    };
    if !ranges.is_empty() {
        return remove_marker_ranges(doc, ranges, selection);
    }

    let range = selection.range();
    if selection.is_caret() {
        let at = selection.head;
        let word = word_range_at(doc, at);
        let word_text = doc.buffer().as_str()[word.clone()].to_owned();
        if word_text.chars().any(is_word_char) {
            let replacement = format!("{marker}{word_text}{marker}");
            return doc.apply_transaction(Transaction::new(
                vec![Edit::replace(word, replacement)],
                Selection::caret(at + marker.len()),
                EditKind::Format,
            ));
        }
        let text = format!("{marker}{marker}");
        let caret = at + marker.len();
        return doc.apply_transaction(Transaction::new(
            vec![Edit::insert(at, text)],
            Selection::caret(caret),
            EditKind::Format,
        ));
    }

    // A marker pair cannot span paragraphs or list items. Refusing is better
    // than writing Markdown that renders as unbalanced text.
    if !selection_fits_one_block(doc, &range) {
        return false;
    }
    if doc.buffer().as_str()[range.clone()].trim().is_empty() {
        return false;
    }

    let selected = doc.buffer().as_str()[range.clone()].to_owned();
    let replacement = format!("{marker}{selected}{marker}");
    let selection_after = Selection::new(
        range.start + marker.len(),
        range.start + marker.len() + selected.len(),
    );
    doc.apply_transaction(Transaction::new(
        vec![Edit::replace(range, replacement)],
        selection_after,
        EditKind::Format,
    ))
}

fn marker_kind(marker: &str) -> Option<MarkerKind> {
    match marker {
        "**" => Some(MarkerKind::Strong),
        "*" => Some(MarkerKind::Emphasis),
        "`" => Some(MarkerKind::Code),
        "~~" => Some(MarkerKind::Strikethrough),
        _ => None,
    }
}

/// Delimiters of an existing construct of `kind` that the selection touches.
/// Both halves are returned, so formatting is removed rather than adding a
/// second pair around text that is already formatted.
fn formatting_ranges(
    doc: &mut Document,
    kind: MarkerKind,
    selection: Selection,
) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    for block in &doc.parsed().blocks {
        for marker in &block.markers {
            if marker.kind != kind {
                continue;
            }
            let touched = if selection.is_caret() {
                marker.outer.start <= selection.head && selection.head <= marker.outer.end
            } else {
                marker.outer.start <= selection.start() && selection.end() <= marker.outer.end
            };
            if touched {
                ranges.push(marker.range.clone());
            }
        }
    }
    ranges.sort_unstable_by_key(|range| (range.start, range.end));
    ranges.dedup();
    ranges
}

fn selection_fits_one_block(doc: &mut Document, range: &Range<usize>) -> bool {
    if doc.len_bytes() == 0 {
        return true;
    }
    doc.parsed()
        .blocks
        .iter()
        .any(|block| block.content.start <= range.start && range.end <= block.content.end)
}

/// Remove complete marker pairs. The caret/selection is shifted by the bytes
/// actually removed before it, so deleting the opening delimiter does not drag
/// the caret into the following text.
fn remove_marker_ranges(
    doc: &mut Document,
    ranges: Vec<Range<usize>>,
    selection: Selection,
) -> bool {
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges {
        if let Some(last) = merged.last_mut()
            && range.start <= last.end
        {
            last.end = last.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    if merged.is_empty() {
        return false;
    }

    let shift = |pos: usize| -> usize {
        let mut removed = 0;
        for range in &merged {
            if range.end <= pos {
                removed += range.end - range.start;
            } else if range.start < pos {
                removed += pos - range.start;
            }
        }
        pos.saturating_sub(removed)
    };
    let selection_after = Selection::new(shift(selection.anchor), shift(selection.head));
    let edits: Vec<Edit> = merged.into_iter().map(Edit::delete).collect();
    doc.apply_transaction(Transaction::new(edits, selection_after, EditKind::Format))
}

fn insert_link(doc: &mut Document) -> bool {
    if doc.mode() == rusk_core::ViewMode::Wysiwyg && in_raw_code(doc, doc.selection().head) {
        return false;
    }
    let selection = doc.selection();
    if let Some(url) = link_destination_at(doc, selection) {
        doc.set_selection(Selection::new(url.start, url.end));
        return true;
    }
    let mut range = selection.range();
    let mut selected = doc.buffer().as_str()[range.clone()].to_owned();
    if selection.is_caret() {
        let word = word_range_at(doc, selection.head);
        let word_text = doc.buffer().as_str()[word.clone()].to_owned();
        if word_text.chars().any(is_word_char) {
            range = word;
            selected = word_text;
        }
    }
    if !selection.is_caret() && !selection_fits_one_block(doc, &range) {
        return false;
    }
    let text = if selected.is_empty() {
        "text".to_owned()
    } else {
        escape_link_text(&selected)
    };
    let replacement = format!("[{text}](url)");
    let url_start = range.start + 1 + text.len() + 2;
    doc.apply_transaction(Transaction::new(
        vec![Edit::replace(range, replacement)],
        Selection::new(url_start, url_start + 3),
        EditKind::Format,
    ))
}

fn escape_link_text(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        if matches!(ch, '\\' | '[' | ']') {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

/// The inline destination of the link surrounding the caret. Ctrl+K on a
/// finished link edits its URL instead of nesting another link inside it.
fn link_destination_at(doc: &mut Document, selection: Selection) -> Option<Range<usize>> {
    let text = doc.text();
    for block in &doc.parsed().blocks {
        for marker in &block.markers {
            if marker.kind != MarkerKind::Link {
                continue;
            }
            let touched = if selection.is_caret() {
                marker.outer.start <= selection.head && selection.head <= marker.outer.end
            } else {
                marker.outer.start < selection.end() && selection.start() < marker.outer.end
            };
            if !touched || marker.range.end != marker.outer.end {
                continue;
            }
            let closing = &text[marker.range.clone()];
            if closing.starts_with("](") && closing.ends_with(')') {
                return Some(marker.range.start + 2..marker.range.end - 1);
            }
            if closing == ">" && text[marker.outer.start..].starts_with('<') {
                return Some(marker.outer.start + 1..marker.outer.end - 1);
            }
        }
    }
    None
}

fn hit_test_local(doc: &Document, layout: &DocumentLayout, local: Pos2) -> usize {
    if layout.blocks.is_empty() {
        return 0;
    }
    if local.y < layout.blocks[0].rect.top() {
        return layout.blocks[0].source_range.start;
    }
    let mut chosen: Option<&LaidBlock> = None;
    for block in &layout.blocks {
        if block.rect.top() <= local.y {
            chosen = Some(block);
        } else {
            break;
        }
    }
    let Some(block) = chosen else {
        return layout.blocks[0].source_range.start;
    };
    if local.y > block.rect.bottom() + 8.0 {
        // Between blocks: snap to the nearest edge.
        if let Some(next) = layout
            .blocks
            .iter()
            .find(|b| b.rect.top() > block.rect.bottom())
        {
            if matches!(next.kind, LaidKind::Spacer) {
                // The space right under a table belongs to the paragraph
                // slot below it, not to the table's last row.
                return next.source_range.start;
            }
            if local.y - block.rect.bottom() < next.rect.top() - local.y {
                return block.source_range.end;
            }
            return next.source_range.start;
        }
        return doc.len_bytes();
    }

    match &block.kind {
        LaidKind::Text { .. }
        | LaidKind::Code { .. }
        | LaidKind::CodeSource
        | LaidKind::TableSource { .. }
        | LaidKind::SourceLine { .. } => {
            if let Some(client) = &block.galley {
                let local_text = local - block.text_pos.to_vec2();
                let cursor = client.cursor_from_pos(local_text.to_vec2());
                let source = block.projection.display_to_source(cursor.index.0);
                return source.clamp(block.source_range.start, block.source_range.end);
            }
            if local.x >= block.text_pos.x {
                block.source_range.end
            } else {
                block.source_range.start
            }
        }
        LaidKind::TableGrid { grid } => {
            for cell in &grid.cells {
                if cell.rect.contains(local) {
                    return cell.source.start;
                }
            }
            block.source_range.start
        }
        LaidKind::ImageBlock { .. } => doc
            .buffer()
            .next_grapheme(block.source_range.start)
            .min(block.source_range.end),
        LaidKind::Rule | LaidKind::Blank | LaidKind::Spacer => block.source_range.start,
    }
}

fn link_at(doc: &mut Document, byte: usize) -> Option<String> {
    let parsed = doc.parsed();
    for block in &parsed.blocks {
        if !(block.content.start <= byte && byte <= block.content.end) {
            continue;
        }
        for token in &block.tokens {
            if token.style.link
                && token.source.start <= byte
                && byte <= token.source.end
                && let Some(url) = &token.url
            {
                return Some(url.clone());
            }
        }
    }
    None
}

fn word_range_at(doc: &Document, pos: usize) -> Range<usize> {
    let text = doc.buffer().as_str();
    let pos = doc.buffer().snap_byte(pos);
    let mut start = pos;
    while start > 0 {
        let prev = doc.buffer().prev_grapheme(start);
        let ch = text[prev..start].chars().next().unwrap_or(' ');
        if !is_word_char(ch) {
            break;
        }
        start = prev;
    }
    let mut end = pos;
    while end < text.len() {
        let next = doc.buffer().next_grapheme(end);
        let ch = text[end..next].chars().next().unwrap_or(' ');
        if !is_word_char(ch) {
            break;
        }
        end = next;
    }
    if start == end {
        // Select one grapheme when the click landed on whitespace/punctuation.
        if pos < text.len() {
            end = doc.buffer().next_grapheme(pos);
        } else if pos > 0 {
            start = doc.buffer().prev_grapheme(pos);
        }
    }
    start..end
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_' || ch == '-'
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusk_core::{Document, Selection};

    fn move_once(
        ui: &mut Ui,
        layout: &DocumentLayout,
        doc: &mut Document,
        direction: f32,
        extend: bool,
    ) {
        let mut goal = None;
        move_vertical(ui, layout, doc, direction, extend, &mut goal);
    }

    fn in_ui<R>(f: impl FnOnce(&mut Ui) -> R) -> R {
        let ctx = egui::Context::default();
        let mut f = Some(f);
        let mut value = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            if let Some(f) = f.take() {
                value = Some(f(ui));
            }
        });
        output.textures_delta.clear();
        value.expect("ui result")
    }

    #[test]
    fn vertical_moves_preserve_the_column_on_a_wrapped_line() {
        use crate::editor::layout::LayoutEngine;
        use crate::theme::{Theme, ThemeKind};
        let ctx = egui::Context::default();
        let theme = Theme::new(ThemeKind::Paper);
        let mut engine = LayoutEngine::default();
        let text = "alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo lima mike november oscar papa quebec romeo sierra tango uniform victor whiskey xray yankee zulu\n\nshort\n";
        let mut doc = Document::from_text(text);
        let mut goal = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let mut layout = engine.build(&ctx, &mut doc, &theme, 600.0, 600.0, None);
            let first = layout.blocks.first().expect("first block");
            let target = Pos2::new(layout.content_left + 180.0, first.rect.top() + 10.0);
            let head = hit_test_local(&doc, &layout, target);
            doc.set_selection(Selection::caret(head));
            layout = engine.build(&ctx, &mut doc, &theme, 600.0, 600.0, None);
            let before = paint::caret_rect_local(&layout, &doc).expect("caret before");
            let block = block_index_at(&layout, head);
            move_vertical(ui, &layout, &mut doc, 1.0, false, &mut goal);
            layout = engine.build(&ctx, &mut doc, &theme, 600.0, 600.0, None);
            let after = paint::caret_rect_local(&layout, &doc).expect("caret after");
            assert_eq!(block_index_at(&layout, doc.selection().head), block);
            assert!(after.top() > before.top());
            assert!(
                (after.left() - before.left()).abs() < 2.0,
                "column drifted from {} to {}",
                before.left(),
                after.left()
            );
        });
        output.textures_delta.clear();
    }

    #[test]
    fn vertical_moves_keep_the_goal_column_across_a_short_line() {
        use crate::editor::layout::LayoutEngine;
        use crate::theme::{Theme, ThemeKind};
        let ctx = egui::Context::default();
        let theme = Theme::new(ThemeKind::Paper);
        let mut engine = LayoutEngine::default();
        let text = "alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo lima mike november\n\nshort\n\nalpha bravo charlie delta echo foxtrot golf hotel india juliet kilo lima mike november\n";
        let mut doc = Document::from_text(text);
        let mut goal = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let mut layout = engine.build(&ctx, &mut doc, &theme, 900.0, 600.0, None);
            let first = layout.blocks.first().expect("first block");
            let target = Pos2::new(layout.content_left + 400.0, first.rect.top() + 10.0);
            let head = hit_test_local(&doc, &layout, target);
            doc.set_selection(Selection::caret(head));
            layout = engine.build(&ctx, &mut doc, &theme, 900.0, 600.0, None);
            let x0 = paint::caret_rect_local(&layout, &doc)
                .expect("caret")
                .left();
            let start_block = block_index_at(&layout, head).expect("block");

            for _ in 0..8 {
                move_vertical(ui, &layout, &mut doc, 1.0, false, &mut goal);
                layout = engine.build(&ctx, &mut doc, &theme, 900.0, 600.0, None);
                if block_index_at(&layout, doc.selection().head) != Some(start_block) {
                    break;
                }
            }
            move_vertical(ui, &layout, &mut doc, 1.0, false, &mut goal);
            layout = engine.build(&ctx, &mut doc, &theme, 900.0, 600.0, None);
            let x2 = paint::caret_rect_local(&layout, &doc)
                .expect("caret after short line")
                .left();
            assert!(
                (x2 - x0).abs() < 2.0,
                "goal column drifted from {x0} to {x2}"
            );
        });
        output.textures_delta.clear();
    }

    #[test]
    fn bold_toggle_removes_markers_when_the_selection_includes_them() {
        let mut doc = Document::from_text("A **bold** word");
        doc.set_selection(Selection::new(2, 10));
        in_ui(|ui| toggle_wrap(ui, &mut doc, "**"));
        assert_eq!(doc.text(), "A bold word");
        assert_eq!(doc.selection(), Selection::new(2, 6));
    }

    #[test]
    fn bold_toggle_removes_markers_around_a_caret_inside_bold_text() {
        let mut doc = Document::from_text("A **bold** word");
        doc.set_selection(Selection::caret(5));
        in_ui(|ui| toggle_wrap(ui, &mut doc, "**"));
        assert_eq!(doc.text(), "A bold word");
        assert_eq!(doc.selection().head, 3);
    }

    #[test]
    fn bold_toggle_wraps_the_word_under_a_caret() {
        let mut doc = Document::from_text("word");
        doc.set_selection(Selection::caret(2));
        in_ui(|ui| toggle_wrap(ui, &mut doc, "**"));
        assert_eq!(doc.text(), "**word**");
        assert_eq!(doc.selection().head, 4);
        in_ui(|ui| toggle_wrap(ui, &mut doc, "**"));
        assert_eq!(doc.text(), "word");
        assert_eq!(doc.selection().head, 2);
    }

    #[test]
    fn bold_toggle_wraps_a_plain_selection() {
        let mut doc = Document::from_text("word");
        doc.set_selection(Selection::new(0, 4));
        in_ui(|ui| toggle_wrap(ui, &mut doc, "**"));
        assert_eq!(doc.text(), "**word**");
        assert_eq!(doc.selection(), Selection::new(2, 6));
    }

    #[test]
    fn backspace_at_the_start_of_bold_removes_both_markers() {
        let mut doc = Document::from_text("A **bold** word");
        doc.set_selection(Selection::caret(4));
        assert!(backspace_guarded(&mut doc));
        assert_eq!(doc.text(), "A bold word");
        assert_eq!(doc.selection().head, 2);
    }

    #[test]
    fn delete_at_the_end_of_bold_removes_both_markers() {
        let mut doc = Document::from_text("A **bold** word");
        doc.set_selection(Selection::caret(8));
        assert!(delete_guarded(&mut doc));
        assert_eq!(doc.text(), "A bold word");
        assert_eq!(doc.selection().head, 6);
    }

    #[test]
    fn ctrl_k_selects_the_destination_of_an_existing_link() {
        let mut doc = Document::from_text("[text](https://example.com)");
        doc.set_selection(Selection::caret(2));
        assert!(in_ui(|ui| {
            let _ = ui;
            insert_link(&mut doc)
        }));
        assert_eq!(doc.selection(), Selection::new(7, 26));
        assert_eq!(doc.text(), "[text](https://example.com)");
    }

    #[test]
    fn ctrl_k_selects_the_destination_of_an_autolink() {
        let mut doc = Document::from_text("<https://example.com>");
        doc.set_selection(Selection::caret(3));
        assert!(in_ui(|ui| {
            let _ = ui;
            insert_link(&mut doc)
        }));
        assert_eq!(doc.selection(), Selection::new(1, 20));
    }

    #[test]
    fn ctrl_k_escapes_brackets_in_link_text() {
        let mut doc = Document::from_text("a]b");
        doc.set_selection(Selection::new(0, 3));
        assert!(in_ui(|ui| {
            let _ = ui;
            insert_link(&mut doc)
        }));
        assert_eq!(doc.text(), "[a\\]b](url)");
    }

    #[test]
    fn backspace_at_the_start_of_a_link_removes_the_link_syntax() {
        let mut doc = Document::from_text("[text](url)");
        doc.set_selection(Selection::caret(1));
        assert!(backspace_guarded(&mut doc));
        assert_eq!(doc.text(), "text");
        assert_eq!(doc.selection().head, 0);
    }

    #[test]
    fn backspace_at_a_heading_start_removes_the_heading_prefix() {
        let mut doc = Document::from_text("# Heading");
        doc.set_selection(Selection::caret(2));
        assert!(backspace_guarded(&mut doc));
        assert_eq!(doc.text(), "Heading");
        assert_eq!(doc.selection().head, 0);
    }

    #[test]
    fn backspace_at_a_list_start_keeps_the_marker() {
        let mut doc = Document::from_text("- item");
        doc.set_selection(Selection::caret(2));
        assert!(!backspace_guarded(&mut doc));
        assert_eq!(doc.text(), "- item");
    }

    #[test]
    fn backspace_at_a_quote_start_removes_the_quote_marker() {
        let mut doc = Document::from_text("> quote");
        doc.set_selection(Selection::caret(2));
        assert!(backspace_guarded(&mut doc));
        assert_eq!(doc.text(), "quote");
    }

    #[test]
    fn backspace_at_a_quote_start_removes_every_quote_marker() {
        let mut doc = Document::from_text("> a\n> b\n");
        doc.set_selection(Selection::caret(2));
        assert!(backspace_guarded(&mut doc));
        assert_eq!(doc.text(), "a\nb\n");
    }

    #[test]
    fn backspace_at_an_indented_list_start_keeps_the_list() {
        let mut doc = Document::from_text("   - item");
        doc.set_selection(Selection::caret(5));
        assert!(!backspace_guarded(&mut doc));
        assert_eq!(doc.text(), "   - item");
    }

    #[test]
    fn backspace_at_a_task_start_keeps_the_task() {
        let mut doc = Document::from_text("- [ ] task");
        doc.set_selection(Selection::caret(6));
        assert!(!backspace_guarded(&mut doc));
        assert_eq!(doc.text(), "- [ ] task");
    }

    #[test]
    fn deleting_the_visible_text_of_bold_removes_the_empty_markers() {
        let mut doc = Document::from_text("**bold**");
        doc.set_selection(Selection::new(2, 6));
        assert!(backspace_guarded(&mut doc));
        assert_eq!(doc.text(), "");
        assert_eq!(doc.selection().head, 0);
    }

    #[test]
    fn deleting_the_text_of_a_link_removes_the_link() {
        let mut doc = Document::from_text("[text](url)");
        doc.set_selection(Selection::new(1, 5));
        assert!(backspace_guarded(&mut doc));
        assert_eq!(doc.text(), "");
    }

    #[test]
    fn ctrl_backspace_at_the_end_of_bold_removes_the_word_and_markers() {
        let mut doc = Document::from_text("A **bold** word");
        doc.set_selection(Selection::caret(8));
        assert!(delete_word(&mut doc, true));
        assert_eq!(doc.text(), "A  word");
    }

    #[test]
    fn formatting_does_not_span_paragraphs() {
        let mut doc = Document::from_text("one\n\ntwo");
        doc.set_selection(Selection::new(0, 8));
        in_ui(|ui| toggle_wrap(ui, &mut doc, "**"));
        assert_eq!(doc.text(), "one\n\ntwo");
    }

    #[test]
    fn arrow_down_leaves_a_code_card() {
        use crate::editor::layout::LayoutEngine;
        use crate::theme::{Theme, ThemeKind};
        let ctx = egui::Context::default();
        let theme = Theme::new(ThemeKind::Paper);
        let mut doc = Document::from_text(
            "```
x
```

a
",
        );
        doc.set_selection(Selection::caret(4));
        let mut engine = LayoutEngine::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let layout = engine.build(&ctx, &mut doc, &theme, 900.0, 600.0, None);
            move_once(ui, &layout, &mut doc, 1.0, false);
        });
        output.textures_delta.clear();
        // The closing fence is its own source line under the caret.
        assert_eq!(doc.selection().head, 6);
    }

    #[test]
    fn arrow_up_leaves_a_table_card() {
        use crate::editor::layout::LayoutEngine;
        use crate::theme::{Theme, ThemeKind};
        let ctx = egui::Context::default();
        let theme = Theme::new(ThemeKind::Paper);
        let mut doc = Document::from_text(
            "a

| h |
|---|
| 1 |
",
        );
        doc.set_selection(Selection::caret(3));
        let mut engine = LayoutEngine::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let layout = engine.build(&ctx, &mut doc, &theme, 900.0, 600.0, None);
            move_once(ui, &layout, &mut doc, -1.0, false);
        });
        output.textures_delta.clear();
        assert_eq!(doc.selection().head, 1);
    }

    #[test]
    fn vertical_moves_skip_the_folded_separator() {
        use crate::editor::layout::LayoutEngine;
        use crate::theme::{Theme, ThemeKind};
        let ctx = egui::Context::default();
        let theme = Theme::new(ThemeKind::Paper);
        let mut doc = Document::from_text(
            "# A

| h |
|---|
| 1 |
",
        );
        doc.set_selection(Selection::caret(2));
        let mut engine = LayoutEngine::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let mut layout = engine.build(&ctx, &mut doc, &theme, 900.0, 600.0, None);
            move_once(ui, &layout, &mut doc, 1.0, false);
            assert_eq!(doc.selection().head, 7, "down lands in the table");
            layout = engine.build(&ctx, &mut doc, &theme, 900.0, 600.0, None);
            move_once(ui, &layout, &mut doc, -1.0, false);
            assert_eq!(doc.selection().head, 3, "up lands at the heading end");
        });
        output.textures_delta.clear();
    }

    #[test]
    fn arrow_down_leaves_the_last_row_of_a_table_card() {
        use crate::editor::layout::LayoutEngine;
        use crate::theme::{Theme, ThemeKind};
        let ctx = egui::Context::default();
        let theme = Theme::new(ThemeKind::Paper);
        let mut doc = Document::from_text(
            "a

| h |
|---|
| 1 |
",
        );
        doc.set_selection(Selection::caret(20));
        let mut engine = LayoutEngine::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let layout = engine.build(&ctx, &mut doc, &theme, 900.0, 600.0, None);
            move_once(ui, &layout, &mut doc, 1.0, false);
        });
        output.textures_delta.clear();
        assert_eq!(doc.selection().head, 21);
    }

    #[test]
    fn arrow_down_from_a_rule_line_moves_on() {
        use crate::editor::layout::LayoutEngine;
        use crate::theme::{Theme, ThemeKind};
        let ctx = egui::Context::default();
        let theme = Theme::new(ThemeKind::Paper);
        let mut doc = Document::from_text(
            "a

---

b
",
        );
        doc.set_selection(Selection::caret(3));
        let mut engine = LayoutEngine::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let layout = engine.build(&ctx, &mut doc, &theme, 900.0, 600.0, None);
            move_once(ui, &layout, &mut doc, 1.0, false);
        });
        output.textures_delta.clear();
        assert_eq!(doc.selection().head, 8);
    }

    #[test]
    fn backspace_removes_an_empty_list_item() {
        let mut doc = Document::from_text(
            "- a
- 
- b
",
        );
        doc.set_selection(Selection::caret(6));
        assert!(backspace_guarded(&mut doc));
        assert_eq!(
            doc.text(),
            "- a

- b
"
        );
        assert_eq!(doc.selection().head, 4);
        assert!(backspace_guarded(&mut doc));
        assert_eq!(
            doc.text(),
            "- a
- b
"
        );
    }

    #[test]
    fn backspace_on_a_non_empty_item_still_jumps() {
        let mut doc = Document::from_text(
            "- a
- b
",
        );
        doc.set_selection(Selection::caret(6));
        assert!(!backspace_guarded(&mut doc));
        assert_eq!(
            doc.text(),
            "- a
- b
"
        );
        assert_eq!(doc.selection().head, 3);
    }

    #[test]
    fn bullet_prefix_continues() {
        let prefix = parse_prefix("- item").expect("list");
        assert_eq!(prefix.continuation(), "- ");
        assert_eq!(prefix.content_start, 2);
    }

    #[test]
    fn ordered_tasks_continue_with_next_number_and_unchecked_box() {
        let prefix = parse_prefix("  3) [x] done").expect("task");
        assert_eq!(prefix.continuation(), "  4) [ ] ");
    }

    #[test]
    fn quotes_continue() {
        let prefix = parse_prefix("> quoted").expect("quote");
        assert_eq!(prefix.continuation(), "> ");
    }

    #[test]
    fn plain_paragraph_is_not_a_prefix() {
        assert!(parse_prefix("plain text").is_none());
    }

    #[test]
    fn backspace_at_a_heading_start_removes_the_heading_marker() {
        let mut doc = Document::from_text("# Title");
        doc.set_selection(Selection::caret(2));
        assert!(backspace_guarded(&mut doc));
        assert_eq!(doc.text(), "Title");
        assert_eq!(doc.selection().head, 0);
    }

    #[test]
    fn word_selection_handles_cjk() {
        let doc = Document::from_text("hello 世界 world");
        let range = word_range_at(&doc, 8);
        assert_eq!(&doc.text()[range], "世界");
    }

    #[test]
    fn line_end_excludes_carriage_return() {
        let mut doc = Document::from_text("one\r\ntwo");
        doc.set_mode(rusk_core::ViewMode::Source);
        doc.set_selection(Selection::caret(1));
        assert_eq!(line_end(&mut doc), 3);
    }

    #[test]
    fn tab_indents_a_list_item_at_line_start() {
        let mut doc = Document::from_text("- item");
        doc.set_selection(Selection::caret(6));
        assert!(handle_tab(&mut doc, false));
        assert_eq!(doc.text(), "  - item");
        assert_eq!(doc.selection().head, 8);
    }

    #[test]
    fn shift_tab_outdents_a_list_item() {
        let mut doc = Document::from_text("  - item");
        doc.set_selection(Selection::caret(8));
        assert!(handle_tab(&mut doc, true));
        assert_eq!(doc.text(), "- item");
        assert_eq!(doc.selection().head, 6);
    }

    #[test]
    fn a_fence_at_the_end_is_left_to_its_author() {
        let mut doc = Document::from_text("first\n");
        doc.set_selection(Selection::caret(6));
        doc.insert_text("```", EditKind::Insert);
        assert!(!auto_close_fence(&mut doc));
        assert_eq!(doc.text(), "first\n```");

        doc.insert_text("rust", EditKind::Insert);
        assert!(!auto_close_fence(&mut doc));
        assert_eq!(doc.text(), "first\n```rust");
    }

    #[test]
    fn an_indented_fence_keeps_its_indent_on_the_closer() {
        let mut doc = Document::from_text("  ```\nkeep");
        doc.set_selection(Selection::caret(5));
        assert!(auto_close_fence(&mut doc));
        assert_eq!(doc.text(), "  ```\n  ```\nkeep");
    }

    #[test]
    fn a_longer_fence_extends_its_closer() {
        let mut doc = Document::from_text("```\n```\n");
        doc.set_selection(Selection::caret(3));
        doc.insert_text("`", EditKind::Insert);
        assert!(auto_close_fence(&mut doc));
        assert_eq!(doc.text(), "````\n````\n");
    }

    #[test]
    fn inline_backticks_do_not_close() {
        let mut doc = Document::from_text("text ``");
        doc.set_selection(Selection::caret(7));
        assert!(!auto_close_fence(&mut doc));
        assert_eq!(doc.text(), "text ``");
    }

    #[test]
    fn removing_an_empty_fence_removes_its_closer() {
        let mut doc = Document::from_text("```\n```\n");
        doc.set_selection(Selection::caret(3));
        assert!(backspace_guarded(&mut doc));
        assert_eq!(doc.text(), "``");
        assert_eq!(doc.selection().head, 2);
    }

    #[test]
    fn a_new_fence_never_pairs_with_a_later_fence() {
        let mut doc = Document::from_text("para\n\n```js\nx\n```\n");
        doc.set_selection(Selection::caret(5));
        doc.insert_text("\n```", EditKind::Insert);
        assert!(auto_close_fence(&mut doc));
        assert_eq!(doc.text(), "para\n\n```\n```\n```js\nx\n```\n");
        let fence_blocks = doc
            .parsed()
            .blocks
            .iter()
            .filter(|b| matches!(b.kind, BlockKind::CodeBlock(_)))
            .count();
        assert_eq!(fence_blocks, 2, "the new fence stays separate");
    }

    #[test]
    fn crlf_fences_keep_crlf() {
        let mut doc = Document::from_text("a\r\n```\r\nx");
        doc.set_selection(Selection::caret(6));
        assert!(auto_close_fence(&mut doc));
        assert_eq!(doc.text(), "a\r\n```\r\n```\r\nx");
    }

    #[test]
    fn enter_in_a_quote_lands_on_a_visible_row_and_takes_text() {
        let ctx = eframe::egui::Context::default();
        let theme = crate::theme::Theme::new(crate::theme::ThemeKind::Paper);
        let mut engine = crate::editor::layout::LayoutEngine::default();
        let text = "> A quiet page.\n> Move the caret into formatted text.\n\n## Try it\n";
        let mut doc = Document::from_text(text);
        let end = text.find("formatted text.").expect("quote text") + "formatted text.".len();
        doc.set_selection(Selection::caret(end));
        let build = |engine: &mut crate::editor::layout::LayoutEngine, doc: &mut Document| {
            let mut built = None;
            let mut output = ctx.run_ui(egui::RawInput::default(), |_ui| {
                built = Some(engine.build(&ctx, doc, &theme, 900.0, 600.0, None));
            });
            output.textures_delta.clear();
            built.expect("layout built")
        };

        let layout = build(&mut engine, &mut doc);
        assert!(handle_enter(&mut doc, &layout));
        assert_eq!(&doc.text()[end..], "\n> \n\n## Try it\n");
        let caret = doc.selection().head;
        assert_eq!(caret, end + 3, "caret sits after the quote marker");

        let layout = build(&mut engine, &mut doc);
        let row = layout
            .block_at_source_pos(caret)
            .expect("a row for the caret");
        assert!(matches!(row.kind, LaidKind::Blank));
        assert_eq!(row.quote_depth, 1);
        assert_eq!(row.source_range.start, caret);

        assert!(doc.insert_text("more", EditKind::Insert));
        assert_eq!(
            &doc.text()[end..],
            "\n> more\n\n## Try it\n",
            "typed text becomes the quote's new line"
        );
    }

    #[test]
    fn typing_below_a_table_keeps_the_separator_line() {
        let mut doc = Document::from_text("| a | b |\n|---|---|\n| 1 | 2 |\n\nbelow\n");
        doc.set_selection(Selection::caret(30));
        assert!(insert_typed(&mut doc, "yo", EditKind::Insert));
        assert_eq!(
            &doc.text()[20..],
            "| 1 | 2 |\n\nyo\n\nbelow\n",
            "the typed line becomes a paragraph of its own"
        );
        let table_range = doc
            .parsed()
            .blocks
            .iter()
            .find(|b| matches!(b.kind, BlockKind::Table))
            .map(|b| b.content.clone())
            .expect("still a table");
        assert_eq!(&doc.text()[table_range], "| a | b |\n|---|---|\n| 1 | 2 |");
        assert_eq!(doc.selection().head, 33, "caret follows the typed text");
    }

    #[test]
    fn typing_on_an_extra_blank_line_keeps_the_block_below_apart() {
        let mut doc = Document::from_text("| a | b |\n|---|---|\n| 1 | 2 |\n\n\nbelow\n");
        doc.set_selection(Selection::caret(31));
        assert!(insert_typed(&mut doc, "yo", EditKind::Insert));
        assert_eq!(
            &doc.text()[30..],
            "\nyo\n\nbelow\n",
            "the extra blank row keeps its place and the text does not merge into `below`"
        );
    }

    #[test]
    fn crlf_tables_keep_crlf_when_typing_below_them() {
        let mut doc = Document::from_text("| a | b |\r\n|---|---|\r\n| 1 | 2 |\r\n\r\nbelow\r\n");
        doc.set_selection(Selection::caret(33));
        assert!(insert_typed(&mut doc, "yo", EditKind::Insert));
        assert_eq!(&doc.text()[22..], "| 1 | 2 |\r\n\r\nyo\r\n\r\nbelow\r\n");
    }

    #[test]
    fn typing_below_a_quote_or_list_item_keeps_its_separator() {
        let mut doc = Document::from_text("> a\n\nx\n");
        doc.set_selection(Selection::caret(4));
        assert!(insert_typed(&mut doc, "yo", EditKind::Insert));
        assert_eq!(doc.text(), "> a\n\nyo\n\nx\n");

        let mut doc = Document::from_text("- a\n\nx\n");
        doc.set_selection(Selection::caret(4));
        assert!(insert_typed(&mut doc, "yo", EditKind::Insert));
        assert_eq!(doc.text(), "- a\n\nyo\n\nx\n");
    }

    #[test]
    fn typing_between_paragraphs_opens_a_new_paragraph() {
        let mut doc = Document::from_text("a\n\nb\n");
        doc.set_selection(Selection::caret(2));
        assert!(insert_typed(&mut doc, "yo", EditKind::Insert));
        assert_eq!(doc.text(), "a\n\nyo\n\nb\n");
    }

    #[test]
    fn typing_above_a_rule_stays_a_paragraph() {
        let mut doc = Document::from_text("| a | b |\n|---|---|\n| 1 | 2 |\n\n---\n\nafter\n");
        doc.set_selection(Selection::caret(30));
        assert!(insert_typed(&mut doc, "yo", EditKind::Insert));
        assert_eq!(
            doc.text(),
            "| a | b |\n|---|---|\n| 1 | 2 |\n\nyo\n\n---\n\nafter\n"
        );
        assert_eq!(doc.selection().head, 33, "caret stays after the typed text");
        let kinds: Vec<_> = doc.parsed().blocks.iter().map(|b| b.kind.clone()).collect();
        assert!(
            kinds.iter().any(|kind| matches!(kind, BlockKind::Rule)),
            "the rule survives"
        );
        assert!(
            !kinds
                .iter()
                .any(|kind| matches!(kind, BlockKind::Heading(_))),
            "no setext heading eats the rule"
        );

        let mut doc = Document::from_text("x\n\n---\n\nafter\n");
        doc.set_selection(Selection::caret(2));
        assert!(insert_typed(&mut doc, "yo", EditKind::Insert));
        assert_eq!(doc.text(), "x\n\nyo\n\n---\n\nafter\n");
        assert!(
            !doc.parsed()
                .blocks
                .iter()
                .any(|b| matches!(b.kind, BlockKind::Heading(_)))
        );
    }

    #[test]
    fn deleting_the_break_above_a_rule_is_refused() {
        let text = "x\n\n---\n\nafter\n";
        // Backspace on the `---` line would leave `x` directly above it.
        let mut doc = Document::from_text(text);
        doc.set_selection(Selection::caret(3));
        assert!(!backspace_guarded(&mut doc));
        assert_eq!(doc.text(), text);

        // Delete at the end of `x` joins the same break.
        doc.set_selection(Selection::caret(1));
        assert!(!delete_guarded(&mut doc));
        assert_eq!(doc.text(), text);
        assert!(
            !doc.parsed()
                .blocks
                .iter()
                .any(|b| matches!(b.kind, BlockKind::Heading(_)))
        );
    }

    #[test]
    fn an_ordinary_blank_line_still_deletes() {
        let mut doc = Document::from_text("a\n\nb\n");
        doc.set_selection(Selection::caret(3));
        assert!(backspace_guarded(&mut doc));
        assert_eq!(doc.text(), "a\nb\n");
    }

    #[test]
    fn the_break_that_separates_text_from_a_table_is_kept() {
        let text = "| a | b |\n|---|---|\n| 1 | 2 |\n\nbelow\n";
        for (caret, backspace) in [(31, true), (30, true), (29, false)] {
            let mut doc = Document::from_text(text);
            doc.set_selection(Selection::caret(caret));
            let changed = if backspace {
                backspace_guarded(&mut doc)
            } else {
                delete_guarded(&mut doc)
            };
            assert!(!changed, "caret {caret} must not eat the separator");
            assert_eq!(doc.text(), text);
            let table = doc
                .parsed()
                .blocks
                .iter()
                .find(|b| matches!(b.kind, BlockKind::Table))
                .map(|b| b.content.clone())
                .expect("still a table");
            assert_eq!(&doc.text()[table], "| a | b |\n|---|---|\n| 1 | 2 |");
        }
    }

    #[test]
    fn clicking_the_gap_under_a_table_targets_the_slot_below_it() {
        let text = "| a | b |\n|---|---|\n| 1 | 2 |\n\nbelow\n";
        let ctx = eframe::egui::Context::default();
        let theme = crate::theme::Theme::new(crate::theme::ThemeKind::Paper);
        let mut doc = Document::from_text(text);
        doc.set_selection(Selection::caret(text.len() - 1));
        let mut engine = crate::editor::layout::LayoutEngine::default();
        let mut built = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |_ui| {
            built = Some(engine.build(&ctx, &mut doc, &theme, 900.0, 600.0, None));
        });
        output.textures_delta.clear();
        let layout = built.expect("layout built");
        let table = layout
            .blocks
            .iter()
            .find(|b| b.source_range.start == 0)
            .expect("table block");
        let point = egui::Pos2::new(layout.content_left + 20.0, table.rect.bottom() + 12.0);
        assert_eq!(
            hit_test_local(&doc, &layout, point),
            30,
            "a click right under the table is not a click inside its last row"
        );
    }
}
