//! Pointer and keyboard interaction for the source-backed editor.
//!
//! Every edit still goes through `Document` transactions; this module only
//! decides *where* an input lands in the canonical byte stream.

use std::ops::Range;

use eframe::egui::{self, Event, ImeEvent, Key, PointerButton, Pos2, Response, Ui};
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
                .blocks
                .iter()
                .find(|b| b.source_range.start <= pos && pos <= b.source_range.end)
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
            match event {
                Event::Text(text) if !text.is_empty() => {
                    let interactive_text =
                        ui.input(|i| !i.modifiers.command && !i.modifiers.ctrl && !i.modifiers.alt);
                    if interactive_text {
                        let text = normalize_newlines(&text);
                        if doc.insert_text(&text, EditKind::Insert) {
                            changed = true;
                            scroll = true;
                            self.ime_preedit.clear();
                        }
                    }
                }
                Event::Paste(text) => {
                    let text = normalize_newlines(&text);
                    if doc.insert_text(&text, EditKind::Paste) {
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
                        if doc.delete_selection(EditKind::Delete) {
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
                        if !text.is_empty() && doc.insert_text(&text, EditKind::Ime) {
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
                            move_vertical(ui, layout, doc, -1.0, modifiers.shift);
                            scroll = true;
                        }
                        Key::ArrowDown => {
                            move_vertical(ui, layout, doc, 1.0, modifiers.shift);
                            scroll = true;
                        }
                        Key::PageUp => {
                            let step = ui.clip_rect().height().max(120.0) * 0.9;
                            move_vertical_by(ui, layout, doc, -step, modifiers.shift);
                            scroll = true;
                        }
                        Key::PageDown => {
                            let step = ui.clip_rect().height().max(120.0) * 0.9;
                            move_vertical_by(ui, layout, doc, step, modifiers.shift);
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
fn clamp_to_visible_block(doc: &mut Document, from: usize, to: usize) -> usize {
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

fn backspace_guarded(doc: &mut Document) -> bool {
    if !doc.selection().is_caret() {
        return doc.backspace();
    }
    let head = doc.selection().head;
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
        return doc.delete_forward();
    }
    let head = doc.selection().head;
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
        return doc.delete_selection(EditKind::Delete);
    }
    let head = doc.selection().head;
    let target = word_boundary(doc, head, left);
    if target == head {
        return false;
    }
    let range = if left { target..head } else { head..target };
    doc.apply_transaction(Transaction::new(
        vec![Edit::delete(range)],
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
) {
    let Some(caret) = paint::caret_rect_local(layout, doc) else {
        return;
    };
    let delta = if direction < 0.0 {
        -caret.height().max(20.0) * 1.15
    } else {
        caret.height().max(20.0) * 1.15
    };
    move_vertical_by(ui, layout, doc, delta, extend);
}

fn move_vertical_by(
    _ui: &mut Ui,
    layout: &DocumentLayout,
    doc: &mut Document,
    delta: f32,
    extend: bool,
) {
    let Some(caret) = paint::caret_rect_local(layout, doc) else {
        return;
    };
    let target = Pos2::new(
        caret.left().max(layout.content_left),
        caret.center().y + delta,
    );
    if target.y < TOP_MARGIN {
        let pos = if delta < 0.0 { 0 } else { doc.len_bytes() };
        doc.set_selection(select_to(doc, pos, extend));
        return;
    }
    let pos = if delta < 0.0 && caret.top() < 4.0 {
        0
    } else {
        hit_test_local(doc, layout, target)
    };
    doc.set_selection(select_to(doc, pos, extend));
}

const TOP_MARGIN: f32 = 8.0;

fn line_home(doc: &mut Document) -> usize {
    let head = doc.selection().head;
    if doc.mode() == rusk_core::ViewMode::Wysiwyg
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

fn handle_enter(doc: &mut Document, layout: &DocumentLayout) -> bool {
    if !doc.selection().is_caret() {
        doc.delete_selection(EditKind::Delete);
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
        .is_some_and(|b| matches!(b.kind, LaidKind::Code { .. } | LaidKind::TableSource { .. }));
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
    let selection = doc.selection();
    let range = selection.range();
    let source = doc.buffer().as_str();
    let selected = source[range.clone()].to_owned();
    let before_has = source[..range.start].ends_with(marker);
    let after_has = source[range.end..].starts_with(marker);

    if before_has && after_has {
        let start = range.start - marker.len();
        let end = range.end + marker.len();
        return doc.apply_transaction(Transaction::new(
            vec![Edit::replace(start..end, selected.clone())],
            Selection::new(start, start + selected.len()),
            EditKind::Format,
        ));
    }

    if selection.is_caret() {
        let at = selection.head;
        let text = format!("{marker}{marker}");
        let caret = at + marker.len();
        return doc.apply_transaction(Transaction::new(
            vec![Edit::insert(at, text)],
            Selection::caret(caret),
            EditKind::Format,
        ));
    }

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

fn insert_link(doc: &mut Document) -> bool {
    let selection = doc.selection();
    let source = doc.buffer().as_str();
    let selected = &source[selection.range()];
    let text = if selected.is_empty() {
        "text"
    } else {
        selected
    };
    let replacement = format!("[{text}](url)");
    let range = selection.range();
    let url_start = range.start + 1 + text.len() + 2;
    doc.apply_transaction(Transaction::new(
        vec![Edit::replace(range, replacement)],
        Selection::new(url_start, url_start + 3),
        EditKind::Format,
    ))
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
        LaidKind::Rule => block.source_range.start,
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
    fn backspace_does_not_eat_heading_marker() {
        let mut doc = Document::from_text("# Title");
        doc.set_selection(Selection::caret(2));
        assert!(!backspace_guarded(&mut doc));
        assert_eq!(doc.text(), "# Title");
        assert_eq!(doc.selection().head, 2);
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
}
