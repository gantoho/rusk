//! A thin, panic-resistant wrapper around [`ropey::Rope`].
//!
//! All public offsets are byte offsets, because Markdown parsers and the
//! projection layer both speak bytes. Conversions to character indices happen
//! at the rope boundary.

use std::ops::Range;

use ropey::Rope;

#[derive(Clone, Debug, Default)]
pub struct TextBuffer {
    rope: Rope,
}

impl TextBuffer {
    #[must_use]
    pub fn new(text: &str) -> Self {
        Self {
            rope: Rope::from_str(text),
        }
    }

    #[must_use]
    pub fn from_rope(rope: Rope) -> Self {
        Self { rope }
    }

    #[must_use]
    pub fn as_str(&self) -> String {
        self.rope.to_string()
    }

    #[must_use]
    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    #[must_use]
    pub fn len_bytes(&self) -> usize {
        self.rope.len_bytes()
    }

    #[must_use]
    pub fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }

    #[must_use]
    pub fn len_lines(&self) -> usize {
        self.rope.len_lines()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rope.len_bytes() == 0
    }

    /// Clamp an arbitrary byte offset to `0..=len` and to a char boundary.
    #[must_use]
    pub fn snap_byte(&self, byte: usize) -> usize {
        let byte = byte.min(self.len_bytes());
        let char_idx = self.rope.byte_to_char(byte);
        self.rope.char_to_byte(char_idx)
    }

    #[must_use]
    pub fn char_to_byte(&self, char_idx: usize) -> usize {
        let char_idx = char_idx.min(self.len_chars());
        self.rope.char_to_byte(char_idx)
    }

    #[must_use]
    pub fn byte_to_char(&self, byte: usize) -> usize {
        self.rope.byte_to_char(self.snap_byte(byte))
    }

    /// The byte offset of the first character on `line` (0-based).
    #[must_use]
    pub fn line_start_byte(&self, line: usize) -> usize {
        if line >= self.len_lines() {
            return self.len_bytes();
        }
        self.rope.char_to_byte(self.rope.line_to_char(line))
    }

    /// The byte offset just past the last character of `line`, including its
    /// newline when one is present.
    #[must_use]
    pub fn line_end_byte_inclusive(&self, line: usize) -> usize {
        if line + 1 >= self.len_lines() {
            return self.len_bytes();
        }
        self.line_start_byte(line + 1)
    }

    /// The byte offset of the line terminator (or EOF for the last line).
    #[must_use]
    pub fn line_end_byte_exclusive(&self, line: usize) -> usize {
        let end = self.line_end_byte_inclusive(line);
        let slice = self
            .rope
            .byte_slice(self.line_start_byte(line)..end)
            .to_string();
        let trimmed = slice.trim_end_matches(['\n', '\r']).len();
        self.line_start_byte(line) + trimmed
    }

    #[must_use]
    pub fn line_of_byte(&self, byte: usize) -> usize {
        self.rope.char_to_line(self.byte_to_char(byte))
    }

    /// 0-based character column, not display column.
    #[must_use]
    pub fn column_of_byte(&self, byte: usize) -> usize {
        let byte = self.snap_byte(byte);
        let char_idx = self.rope.byte_to_char(byte);
        char_idx - self.rope.line_to_char(self.rope.char_to_line(char_idx))
    }

    #[must_use]
    pub fn line_text(&self, line: usize) -> String {
        if line >= self.len_lines() {
            return String::new();
        }
        self.rope.line(line).to_string()
    }

    #[must_use]
    pub fn slice(&self, range: Range<usize>) -> String {
        let start = self.snap_byte(range.start);
        let end = self.snap_byte(range.end.max(start));
        self.rope.byte_slice(start..end).to_string()
    }

    #[must_use]
    pub fn char_at_byte(&self, byte: usize) -> Option<char> {
        let byte = self.snap_byte(byte);
        if byte >= self.len_bytes() {
            return None;
        }
        self.rope.byte_slice(byte..self.len_bytes()).chars().next()
    }

    /// Insert `text` at `byte`. Returns the number of bytes inserted.
    pub fn insert(&mut self, byte: usize, text: &str) -> usize {
        let byte = self.snap_byte(byte);
        let char_idx = self.rope.byte_to_char(byte);
        self.rope.insert(char_idx, text);
        text.len()
    }

    /// Remove `range`. Returns the removed text.
    pub fn remove(&mut self, range: Range<usize>) -> String {
        let start = self.snap_byte(range.start);
        let end = self.snap_byte(range.end.max(start));
        if start == end {
            return String::new();
        }
        let start_char = self.rope.byte_to_char(start);
        let end_char = self.rope.byte_to_char(end);
        let removed = self.rope.slice(start_char..end_char).to_string();
        self.rope.remove(start_char..end_char);
        removed
    }

    /// Replace `range` with `text`. Returns the removed text.
    pub fn replace(&mut self, range: Range<usize>, text: &str) -> String {
        let start = self.snap_byte(range.start);
        let end = self.snap_byte(range.end.max(start));
        let removed = if start == end {
            String::new()
        } else {
            let start_char = self.rope.byte_to_char(start);
            let end_char = self.rope.byte_to_char(end);
            let removed = self.rope.slice(start_char..end_char).to_string();
            self.rope.remove(start_char..end_char);
            removed
        };
        if !text.is_empty() {
            let char_idx = self.rope.byte_to_char(start);
            self.rope.insert(char_idx, text);
        }
        removed
    }

    /// Rebuild the buffer from a string, preserving the rope abstraction.
    pub fn reset(&mut self, text: &str) {
        self.rope = Rope::from_str(text);
    }

    /// The text of the line containing `byte`, including its trailing newline.
    #[must_use]
    pub fn line_text_at(&self, byte: usize) -> String {
        self.line_text(self.line_of_byte(byte))
    }

    /// Return `(line, column)` for a byte offset. Both zero-based.
    #[must_use]
    pub fn line_col(&self, byte: usize) -> (usize, usize) {
        (self.line_of_byte(byte), self.column_of_byte(byte))
    }

    /// Move `byte` left by one grapheme cluster, stopping at the previous line.
    #[must_use]
    pub fn prev_grapheme(&self, byte: usize) -> usize {
        let text = self.as_str();
        let byte = self.snap_byte(byte);
        if byte == 0 {
            return 0;
        }
        use unicode_segmentation::UnicodeSegmentation;
        text[..byte]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(i, _)| i)
    }

    /// Move `byte` right by one grapheme cluster, stopping at the next line.
    #[must_use]
    pub fn next_grapheme(&self, byte: usize) -> usize {
        let text = self.as_str();
        let byte = self.snap_byte(byte);
        if byte >= text.len() {
            return text.len();
        }
        use unicode_segmentation::UnicodeSegmentation;
        text[byte..]
            .graphemes(true)
            .next()
            .map_or(text.len(), |g| byte + g.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_editing_round_trips() {
        let mut b = TextBuffer::new("héllo 世界");
        let end = b.len_bytes();
        b.insert(end, "!");
        assert_eq!(b.as_str(), "héllo 世界!");
        let removed = b.remove(0..3);
        assert_eq!(removed, "hé");
        assert_eq!(b.as_str(), "llo 世界!");
    }

    #[test]
    fn line_queries_are_byte_accurate() {
        let b = TextBuffer::new("one\n二\nthree");
        assert_eq!(b.len_lines(), 3);
        assert_eq!(b.line_start_byte(1), 4);
        assert_eq!(b.line_end_byte_exclusive(1), 7);
        assert_eq!(b.line_col(8), (2, 0));
    }

    #[test]
    fn grapheme_navigation_handles_emoji() {
        let b = TextBuffer::new("a👨‍👩‍👧‍👦b");
        assert_eq!(b.next_grapheme(1), "a👨‍👩‍👧‍👦".len());
        assert_eq!(b.prev_grapheme("a👨‍👩‍👧‍👦".len()), 1);
    }
}
