//! Edits are transactional. A transaction is a list of non-overlapping
//! replacements in *before* coordinates, plus the selection to install after
//! the edit.
//!
//! The transaction can be applied to a [`TextBuffer`] to produce an inverse
//! edit list, which is what the undo stack stores. This keeps the source
//! authoritative and makes every content mutation replayable.

use std::ops::Range;
use std::time::{Duration, Instant};

use crate::buffer::TextBuffer;
use crate::selection::Selection;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum EditKind {
    #[default]
    Other,
    Insert,
    Delete,
    Replace,
    Paste,
    Ime,
    Structural,
    Format,
}

impl EditKind {
    /// Typing-like edits within this window can be coalesced into one undo step.
    #[must_use]
    pub fn is_typing(self) -> bool {
        matches!(self, Self::Insert | Self::Delete)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    /// Byte range in the document *before* this transaction is applied.
    pub range: Range<usize>,
    pub replacement: String,
}

impl Edit {
    #[must_use]
    pub fn insert(at: usize, text: impl Into<String>) -> Self {
        Self {
            range: at..at,
            replacement: text.into(),
        }
    }

    #[must_use]
    pub fn delete(range: Range<usize>) -> Self {
        Self {
            range,
            replacement: String::new(),
        }
    }

    #[must_use]
    pub fn replace(range: Range<usize>, text: impl Into<String>) -> Self {
        Self {
            range,
            replacement: text.into(),
        }
    }

    #[must_use]
    pub fn removed_len(&self) -> usize {
        self.range.end - self.range.start
    }

    #[must_use]
    pub fn inserted_len(&self) -> usize {
        self.replacement.len()
    }

    #[must_use]
    pub fn is_noop(&self) -> bool {
        self.range.is_empty() && self.replacement.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transaction {
    pub edits: Vec<Edit>,
    pub selection_after: Selection,
    pub kind: EditKind,
    pub label: Option<String>,
}

impl Transaction {
    #[must_use]
    pub fn new(edits: Vec<Edit>, selection_after: Selection, kind: EditKind) -> Self {
        Self {
            edits,
            selection_after,
            kind,
            label: None,
        }
    }

    #[must_use]
    pub fn insert(at: usize, text: impl Into<String>, kind: EditKind) -> Self {
        let text = text.into();
        let after = at + text.len();
        Self::new(vec![Edit::insert(at, text)], Selection::caret(after), kind)
    }

    #[must_use]
    pub fn replace(range: Range<usize>, text: impl Into<String>, kind: EditKind) -> Self {
        let text = text.into();
        let after = range.start + text.len();
        Self::new(
            vec![Edit::replace(range, text)],
            Selection::caret(after),
            kind,
        )
    }

    #[must_use]
    pub fn delete(range: Range<usize>) -> Self {
        let at = range.start;
        Self::new(
            vec![Edit::delete(range)],
            Selection::caret(at),
            EditKind::Delete,
        )
    }

    #[must_use]
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.edits.iter().all(Edit::is_noop)
    }

    /// Number of bytes removed and inserted, used to decide whether a
    /// transaction is cheap enough to merge into the previous undo step.
    #[must_use]
    pub fn weight(&self) -> usize {
        self.edits
            .iter()
            .map(|e| e.removed_len() + e.inserted_len())
            .sum()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AppliedTransaction {
    pub transaction: Transaction,
    /// Edits that undo this transaction, expressed in *after* coordinates.
    pub inverse: Vec<Edit>,
}

/// Validate and sort edits, returning them in ascending order.
fn normalized(mut edits: Vec<Edit>) -> Result<Vec<Edit>, TransactionError> {
    edits.retain(|e| !e.is_noop());
    edits.sort_by_key(|e| (e.range.start, e.range.end));
    let mut last_end = 0;
    for e in &edits {
        if e.range.start > e.range.end {
            return Err(TransactionError::BackwardsRange(e.range.clone()));
        }
        if e.range.start < last_end {
            return Err(TransactionError::OverlappingEdits);
        }
        last_end = e.range.end;
    }
    Ok(edits)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransactionError {
    BackwardsRange(Range<usize>),
    OverlappingEdits,
    OutOfBounds { offset: usize, len: usize },
}

impl std::fmt::Display for TransactionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BackwardsRange(r) => write!(f, "backwards edit range {r:?}"),
            Self::OverlappingEdits => write!(f, "transaction contains overlapping edits"),
            Self::OutOfBounds { offset, len } => {
                write!(f, "edit offset {offset} is outside document of {len} bytes")
            }
        }
    }
}

impl std::error::Error for TransactionError {}

/// Apply a transaction to a buffer and compute its inverse.
pub fn apply(
    buffer: &mut TextBuffer,
    transaction: &Transaction,
) -> Result<AppliedTransaction, TransactionError> {
    let len = buffer.len_bytes();
    for edit in &transaction.edits {
        if edit.range.end > len {
            return Err(TransactionError::OutOfBounds {
                offset: edit.range.end,
                len,
            });
        }
    }

    let edits = normalized(transaction.edits.clone())?;
    let mut inverse = Vec::with_capacity(edits.len());
    let mut delta: isize = 0;

    for edit in &edits {
        let final_start = (edit.range.start as isize + delta).max(0) as usize;
        let removed = buffer.slice(edit.range.clone());
        inverse.push(Edit::replace(
            final_start..final_start + edit.inserted_len(),
            removed,
        ));
        delta += edit.inserted_len() as isize - edit.removed_len() as isize;
    }

    // Apply from the end: earlier offsets remain valid while we mutate.
    for edit in edits.iter().rev() {
        buffer.replace(edit.range.clone(), &edit.replacement);
    }

    Ok(AppliedTransaction {
        transaction: transaction.clone(),
        inverse,
    })
}

/// Metadata used by the history stack to merge typing bursts.
#[derive(Clone, Copy, Debug)]
pub struct Coalesce {
    pub kind: EditKind,
    pub at: Instant,
    pub window: Duration,
}

impl Default for Coalesce {
    fn default() -> Self {
        Self {
            kind: EditKind::Other,
            at: Instant::now(),
            window: Duration::from_millis(650),
        }
    }
}

impl Coalesce {
    #[must_use]
    pub fn can_merge(&self, kind: EditKind, now: Instant) -> bool {
        kind.is_typing() && self.kind == kind && now.duration_since(self.at) <= self.window
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverse_restores_exact_text() {
        let mut buf = TextBuffer::new("hello world");
        let tx = Transaction::new(
            vec![Edit::replace(0..5, "hi"), Edit::insert(11, "!")],
            Selection::caret(9),
            EditKind::Replace,
        );
        let applied = apply(&mut buf, &tx).unwrap();
        assert_eq!(buf.as_str(), "hi world!");
        let undo = Transaction::new(applied.inverse, Selection::caret(0), EditKind::Other);
        apply(&mut buf, &undo).unwrap();
        assert_eq!(buf.as_str(), "hello world");
    }

    #[test]
    fn inverse_offsets_account_for_prior_edits() {
        let mut buf = TextBuffer::new("abcdef");
        let tx = Transaction::new(
            vec![Edit::insert(1, "XX"), Edit::delete(4..5)],
            Selection::caret(5),
            EditKind::Other,
        );
        let applied = apply(&mut buf, &tx).unwrap();
        assert_eq!(buf.as_str(), "aXXbcdf");
        let undo = Transaction::new(applied.inverse, Selection::caret(0), EditKind::Other);
        apply(&mut buf, &undo).unwrap();
        assert_eq!(buf.as_str(), "abcdef");
    }

    #[test]
    fn overlapping_edits_are_rejected() {
        let mut buf = TextBuffer::new("abcdef");
        let tx = Transaction::new(
            vec![Edit::delete(1..4), Edit::delete(2..3)],
            Selection::caret(1),
            EditKind::Other,
        );
        assert_eq!(
            apply(&mut buf, &tx),
            Err(TransactionError::OverlappingEdits)
        );
    }
}
