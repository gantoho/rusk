//! Rusk core: the Markdown source is the document.
//!
//! Everything visual is a *projection* of the source; there is no second
//! document model to serialize back.

pub mod buffer;
pub mod document;
pub mod markdown;
pub mod projection;
pub mod selection;
pub mod transaction;

pub use buffer::TextBuffer;
pub use document::{Document, DocumentStats, ViewMode};
pub use markdown::to_html;
pub use selection::Selection;
pub use transaction::{Edit, EditKind, Transaction};
