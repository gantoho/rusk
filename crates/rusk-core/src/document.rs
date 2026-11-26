//! The document: canonical text, selection, view mode, undo history, and a
//! cached concrete parse. Nothing here knows about a GUI toolkit.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::buffer::TextBuffer;
use crate::markdown::{ParsedDocument, parse};
use crate::selection::Selection;
use crate::transaction::{AppliedTransaction, Edit, EditKind, Transaction, apply};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ViewMode {
    #[default]
    Wysiwyg,
    Source,
}

impl ViewMode {
    #[must_use]
    pub fn toggled(self) -> Self {
        match self {
            Self::Wysiwyg => Self::Source,
            Self::Source => Self::Wysiwyg,
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Wysiwyg => "Write",
            Self::Source => "Source",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DocumentStats {
    pub words: usize,
    pub chars: usize,
    pub lines: usize,
    pub reading_minutes: usize,
}

#[derive(Clone, Debug)]
struct HistoryEntry {
    before: String,
    after: String,
    selection_before: Selection,
    selection_after: Selection,
    kind: EditKind,
    at: Instant,
}

#[derive(Clone, Debug)]
pub struct Document {
    buffer: TextBuffer,
    selection: Selection,
    mode: ViewMode,
    path: Option<PathBuf>,
    revision: u64,
    saved_text: String,
    dirty: bool,
    undo: Vec<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    parsed: ParsedDocument,
    parsed_revision: u64,
    last_edit_at: Instant,
    history_limit: usize,
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl Document {
    #[must_use]
    pub fn new() -> Self {
        let welcome = welcome_document();
        Self::from_text(&welcome)
    }

    #[must_use]
    pub fn empty() -> Self {
        let (parsed, _) = (parse(""), 0);
        Self {
            buffer: TextBuffer::new(""),
            selection: Selection::caret(0),
            mode: ViewMode::Wysiwyg,
            path: None,
            revision: 0,
            saved_text: String::new(),
            dirty: false,
            undo: Vec::new(),
            redo: Vec::new(),
            parsed,
            parsed_revision: 0,
            last_edit_at: Instant::now() - Duration::from_secs(10),
            history_limit: 200,
        }
    }

    #[must_use]
    pub fn from_text(text: &str) -> Self {
        let parsed = parse(text);
        let initial = parsed.blocks.first().map_or(0, |block| block.content.start);
        Self {
            buffer: TextBuffer::new(text),
            selection: Selection::caret(initial),
            mode: ViewMode::Wysiwyg,
            path: None,
            revision: 0,
            saved_text: text.to_owned(),
            dirty: false,
            undo: Vec::new(),
            redo: Vec::new(),
            parsed,
            parsed_revision: 0,
            last_edit_at: Instant::now() - Duration::from_secs(10),
            history_limit: 200,
        }
    }

    /// Open a Markdown file. UTF-8 with an optional BOM is accepted.
    pub fn open(path: impl Into<PathBuf>) -> std::io::Result<Self> {
        let path = path.into();
        let text = std::fs::read_to_string(&path)?;
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text).to_owned();
        let mut doc = Self::from_text(&text);
        doc.path = Some(path);
        Ok(doc)
    }

    /// Save to the current path, or return `false` when this is untitled.
    pub fn save(&mut self) -> std::io::Result<bool> {
        let Some(path) = self.path.clone() else {
            return Ok(false);
        };
        self.save_to(&path)?;
        Ok(true)
    }

    pub fn save_to(&mut self, path: impl Into<PathBuf>) -> std::io::Result<()> {
        let path = path.into();
        let text = self.buffer.as_str();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, &text)?;
        self.path = Some(path);
        self.saved_text = text;
        self.dirty = false;
        Ok(())
    }

    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    #[must_use]
    pub fn file_name(&self) -> Option<String> {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
    }

    #[must_use]
    pub fn title(&self) -> String {
        self.file_name().unwrap_or_else(|| "Untitled".to_owned())
    }

    #[must_use]
    pub fn text(&self) -> String {
        self.buffer.as_str()
    }

    #[must_use]
    pub fn buffer(&self) -> &TextBuffer {
        &self.buffer
    }

    #[must_use]
    pub fn len_bytes(&self) -> usize {
        self.buffer.len_bytes()
    }

    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    #[must_use]
    pub fn selection(&self) -> Selection {
        self.selection
    }

    pub fn set_selection(&mut self, selection: Selection) {
        let text = self.buffer.as_str();
        self.selection = selection.clamped(&text);
    }

    #[must_use]
    pub fn mode(&self) -> ViewMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: ViewMode) {
        self.mode = mode;
    }

    pub fn toggle_mode(&mut self) {
        self.mode = self.mode.toggled();
    }

    #[must_use]
    pub fn dirty(&self) -> bool {
        self.dirty
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    #[must_use]
    pub fn parsed(&mut self) -> &ParsedDocument {
        if self.parsed_revision != self.revision {
            self.parsed = parse(&self.buffer.as_str());
            self.parsed_revision = self.revision;
        }
        &self.parsed
    }

    #[must_use]
    pub fn outline(&mut self) -> Vec<crate::markdown::OutlineItem> {
        self.parsed().outline.clone()
    }

    #[must_use]
    pub fn stats(&self) -> DocumentStats {
        let text = self.buffer.as_str();
        let mut words = 0usize;
        let mut in_word = false;
        for ch in text.chars() {
            if ch.is_whitespace() {
                in_word = false;
            } else if is_cjk(ch) {
                words += 1;
                in_word = false;
            } else if !in_word {
                words += 1;
                in_word = true;
            }
        }
        let chars = text.chars().count();
        DocumentStats {
            words,
            chars,
            lines: self.buffer.len_lines().max(1),
            reading_minutes: words.div_ceil(220).max(1),
        }
    }

    #[must_use]
    pub fn line_col(&self) -> (usize, usize) {
        let (line, col) = self.buffer.line_col(self.selection.head);
        (line + 1, col + 1)
    }

    #[must_use]
    pub fn selection_text(&self) -> String {
        self.buffer.slice(self.selection.range())
    }

    /// Apply a transaction, recording one undo step.
    pub fn apply_transaction(&mut self, transaction: Transaction) -> bool {
        if transaction.is_empty() {
            let text = self.buffer.as_str();
            self.selection = transaction.selection_after.clamped(&text);
            return false;
        }
        let before = self.buffer.as_str();
        let selection_before = self.selection;
        let applied = match apply(&mut self.buffer, &transaction) {
            Ok(applied) => applied,
            Err(err) => {
                eprintln!("rusk: transaction rejected: {err}");
                return false;
            }
        };
        let _: AppliedTransaction = applied;
        let after = self.buffer.as_str();
        let text = after.clone();
        self.selection = transaction.selection_after.clamped(&text);
        self.revision += 1;
        self.dirty = after != self.saved_text;

        let now = Instant::now();
        let can_coalesce = self.undo.last().is_some_and(|last| {
            last.kind == transaction.kind
                && transaction.kind.is_typing()
                && selection_before == last.selection_after
                && now.duration_since(last.at) <= Duration::from_millis(700)
        });
        if can_coalesce {
            if let Some(last) = self.undo.last_mut() {
                last.after = after;
                last.selection_after = self.selection;
                last.at = now;
            }
        } else {
            self.undo.push(HistoryEntry {
                before,
                after,
                selection_before,
                selection_after: self.selection,
                kind: transaction.kind,
                at: now,
            });
            if self.undo.len() > self.history_limit {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.last_edit_at = now;
        true
    }

    /// Insert text over the current selection.
    pub fn insert_text(&mut self, text: &str, kind: EditKind) -> bool {
        let range = self.selection.range();
        let after = range.start + text.len();
        let transaction = Transaction::new(
            vec![Edit::replace(range, text)],
            Selection::caret(after),
            kind,
        );
        self.apply_transaction(transaction)
    }

    /// Replace an explicit source range and place the caret after it.
    pub fn replace_range(
        &mut self,
        range: std::ops::Range<usize>,
        text: &str,
        kind: EditKind,
    ) -> bool {
        let after = range.start + text.len();
        self.apply_transaction(Transaction::new(
            vec![Edit::replace(range, text)],
            Selection::caret(after),
            kind,
        ))
    }

    /// Delete the current selection, or one grapheme to the left.
    pub fn backspace(&mut self) -> bool {
        if !self.selection.is_caret() {
            return self.delete_selection(EditKind::Delete);
        }
        let head = self.selection.head;
        if head == 0 {
            return false;
        }
        let target = self.buffer.prev_grapheme(head);
        self.apply_transaction(Transaction {
            edits: vec![Edit::delete(target..head)],
            selection_after: Selection::caret(target),
            kind: EditKind::Delete,
            label: None,
        })
    }

    /// Delete the current selection, or one grapheme to the right.
    pub fn delete_forward(&mut self) -> bool {
        if !self.selection.is_caret() {
            return self.delete_selection(EditKind::Delete);
        }
        let head = self.selection.head;
        if head >= self.buffer.len_bytes() {
            return false;
        }
        let target = self.buffer.next_grapheme(head);
        self.apply_transaction(Transaction {
            edits: vec![Edit::delete(head..target)],
            selection_after: Selection::caret(head),
            kind: EditKind::Delete,
            label: None,
        })
    }

    pub fn delete_selection(&mut self, kind: EditKind) -> bool {
        if self.selection.is_caret() {
            return false;
        }
        let range = self.selection.range();
        self.apply_transaction(Transaction {
            edits: vec![Edit::delete(range.clone())],
            selection_after: Selection::caret(range.start),
            kind,
            label: None,
        })
    }

    pub fn undo(&mut self) -> bool {
        let Some(entry) = self.undo.pop() else {
            return false;
        };
        self.buffer.reset(&entry.before);
        let text = self.buffer.as_str();
        self.selection = entry.selection_before.clamped(&text);
        self.revision += 1;
        self.dirty = text != self.saved_text;
        self.redo.push(entry);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(entry) = self.redo.pop() else {
            return false;
        };
        self.buffer.reset(&entry.after);
        let text = self.buffer.as_str();
        self.selection = entry.selection_after.clamped(&text);
        self.revision += 1;
        self.dirty = text != self.saved_text;
        self.undo.push(entry);
        true
    }

    /// Replace the entire document, e.g. after a reload. This is one undo step.
    pub fn reset_text(&mut self, text: &str, selection: Selection) {
        let before = self.buffer.as_str();
        let selection_before = self.selection;
        if before == text {
            self.selection = selection.clamped(text);
            return;
        }
        self.buffer.reset(text);
        self.selection = selection.clamped(text);
        self.revision += 1;
        self.dirty = text != self.saved_text;
        self.undo.push(HistoryEntry {
            before,
            after: text.to_owned(),
            selection_before,
            selection_after: self.selection,
            kind: EditKind::Replace,
            at: Instant::now(),
        });
        self.redo.clear();
    }

    /// Find the next occurrence of `needle` after the caret, wrapping.
    #[must_use]
    pub fn find(&self, needle: &str) -> Option<std::ops::Range<usize>> {
        if needle.is_empty() {
            return None;
        }
        let text = self.buffer.as_str();
        let start = self.selection.end().min(text.len());
        if let Some(pos) = text[start..].find(needle) {
            let at = start + pos;
            return Some(at..at + needle.len());
        }
        text.find(needle).map(|at| at..at + needle.len())
    }

    #[must_use]
    pub fn block_index_at_selection(&mut self) -> Option<usize> {
        let head = self.selection.head;
        self.parsed()
            .block_at(head)
            .or_else(|| self.parsed().blocks.len().checked_sub(1))
    }
}

fn is_cjk(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0xAC00..=0xD7AF
    )
}

#[must_use]
pub fn welcome_document() -> String {
    r#"# Welcome to Rusk

Rusk is a native Markdown editor built around one rule: **the Markdown source is the document**.

There is no rich-text shadow model and no lossy export step. Styling is a *projection* of the source, so switching to source mode is instant and always faithful.

## Try it

- **Bold**, *italic*, ~~strikethrough~~, and `inline code` render in place.
- Links like [the CommonMark spec](https://commonmark.org) hide their destination until you edit them.
- Type `/` shortcuts below, or press **Ctrl+/** to switch between Write and Source.
- Press **Ctrl+S** to save, **Ctrl+O** to open a file.

> A quiet page, a canonical source, and nothing in between.
> Move the caret into formatted text to reveal its syntax.

### A small code block

```rust
fn main() {
    println!("Markdown in, Markdown out.");
}
```

### A table

| Feature | Status |
| :--- | :---: |
| Source-first | Yes |
| Native GPU | Yes |
| Round-trip loss | None |

---

Start writing. Everything you type is Markdown, byte for byte.
"#
    .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_coalesces_and_undoes_as_a_group() {
        let mut doc = Document::from_text("ab");
        doc.set_selection(Selection::caret(2));
        doc.insert_text("c", EditKind::Insert);
        doc.insert_text("d", EditKind::Insert);
        assert_eq!(doc.text(), "abcd");
        assert!(doc.undo());
        assert_eq!(doc.text(), "ab");
        assert!(doc.redo());
        assert_eq!(doc.text(), "abcd");
    }

    #[test]
    fn undo_returns_to_saved_state() {
        let mut doc = Document::from_text("hello");
        doc.set_selection(Selection::caret(5));
        doc.insert_text("!", EditKind::Insert);
        assert!(doc.dirty());
        assert!(doc.undo());
        assert!(!doc.dirty());
    }

    #[test]
    fn stats_are_cjk_aware() {
        let doc = Document::from_text("hello 世界");
        let stats = doc.stats();
        assert_eq!(stats.words, 3);
        assert_eq!(stats.chars, 8);
    }

    #[test]
    fn parsing_cache_follows_revision() {
        let mut doc = Document::from_text("# a");
        assert_eq!(doc.parsed().outline.len(), 1);
        doc.set_selection(Selection::caret(3));
        doc.insert_text("\n\npara", EditKind::Insert);
        assert_eq!(doc.parsed().blocks.len(), 2);
    }

    #[test]
    fn initial_caret_is_inside_the_first_block_not_the_marker() {
        let doc = Document::from_text("# Title");
        assert_eq!(doc.selection().head, 2);
    }
}
