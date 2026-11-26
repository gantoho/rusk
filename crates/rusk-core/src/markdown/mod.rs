//! Markdown parsing into a *concrete* view: every block and every inline run
//! keeps its byte range in the canonical source.
//!
//! The parser never builds an alternate document that would need to be
//! serialized back. It only labels source ranges with meaning.

mod blocks;
mod inline;

pub use blocks::{
    Block, BlockKind, CodeBlockInfo, ImageInfo, InlineMarker, InlineStyle, InlineToken, ListKind,
    ListMarker, MarkerKind, OutlineItem, ParsedDocument, Prefix, PrefixKind, TableCell, TableData,
    TableRowData, TokenKind, parse,
};

/// Render canonical Markdown to HTML for export. Inline HTML is passed through
/// by pulldown-cmark, matching the editor's source-first philosophy.
#[must_use]
pub fn to_html(source: &str) -> String {
    use pulldown_cmark::{Options, Parser, html};
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_MATH
        | Options::ENABLE_GFM;
    let parser = Parser::new_ext(source, options);
    let mut output = String::with_capacity(source.len() * 2);
    html::push_html(&mut output, parser);
    output
}
