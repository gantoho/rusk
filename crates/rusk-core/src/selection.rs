//! Selections are expressed in UTF-8 byte offsets into the canonical source.

use std::ops::Range;

/// A text selection with a fixed anchor and a moving head.
///
/// A caret is a selection whose `anchor == head`. Keeping the direction is
/// important for `Shift`-arrow behaviour and for restoring a selection exactly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub anchor: usize,
    pub head: usize,
}

impl Selection {
    #[must_use]
    pub const fn caret(pos: usize) -> Self {
        Self {
            anchor: pos,
            head: pos,
        }
    }

    #[must_use]
    pub const fn new(anchor: usize, head: usize) -> Self {
        Self { anchor, head }
    }

    #[must_use]
    pub fn is_caret(self) -> bool {
        self.anchor == self.head
    }

    /// The smaller byte offset.
    #[must_use]
    pub fn start(self) -> usize {
        self.anchor.min(self.head)
    }

    /// The larger byte offset.
    #[must_use]
    pub fn end(self) -> usize {
        self.anchor.max(self.head)
    }

    #[must_use]
    pub fn range(self) -> Range<usize> {
        self.start()..self.end()
    }

    #[must_use]
    pub fn len(self) -> usize {
        self.end() - self.start()
    }

    #[must_use]
    pub fn is_empty(self) -> bool {
        self.is_caret()
    }

    /// Normalize a raw anchor/head pair against a document length and snap both
    /// ends to UTF-8 character boundaries.
    #[must_use]
    pub fn clamped(self, text: &str) -> Self {
        Self {
            anchor: floor_char_boundary(text, self.anchor.min(text.len())),
            head: floor_char_boundary(text, self.head.min(text.len())),
        }
    }

    #[must_use]
    pub fn with_head(self, head: usize) -> Self {
        Self {
            anchor: self.anchor,
            head,
        }
    }

    #[must_use]
    pub fn collapsed_to(self, pos: usize) -> Self {
        Self::caret(pos)
    }

    /// Shift both ends by a delta, useful when an edit happens before the
    /// selection. The result is clamped to `[0, text_len]` by the caller.
    #[must_use]
    pub fn shifted(self, delta: isize) -> Self {
        let shift = |v: usize| -> usize {
            if delta >= 0 {
                v.saturating_add(delta as usize)
            } else {
                v.saturating_sub(delta.unsigned_abs())
            }
        };
        Self {
            anchor: shift(self.anchor),
            head: shift(self.head),
        }
    }

    /// True when `pos` is inside the selection (inclusive at both visible
    /// boundaries, useful for deciding whether a syntax marker is revealed).
    #[must_use]
    pub fn touches(self, pos: usize) -> bool {
        self.start() <= pos && pos <= self.end()
    }

    #[must_use]
    pub fn intersects(self, range: &Range<usize>) -> bool {
        self.start() < range.end && range.start < self.end()
    }
}

/// Round `index` down to the nearest UTF-8 boundary.
#[must_use]
pub fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// Round `index` up to the nearest UTF-8 boundary.
#[must_use]
pub fn ceil_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_direction_and_range() {
        let s = Selection::new(10, 4);
        assert_eq!(s.start(), 4);
        assert_eq!(s.end(), 10);
        assert_eq!(s.range(), 4..10);
        assert!(!s.is_caret());
    }

    #[test]
    fn clamps_to_char_boundary() {
        let s = Selection::caret(1).clamped("é"); // é is two bytes
        assert_eq!(s.head, 0);
        let s = Selection::caret(2).clamped("é");
        assert_eq!(s.head, 2);
    }
}
