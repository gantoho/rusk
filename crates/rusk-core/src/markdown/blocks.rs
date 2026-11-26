//! Block-level structure. A block is a *view* over a source range, plus the
//! inline runs inside it. Blocks never own or rewrite the text.

use std::ops::Range;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use super::inline::{InlineCollector, handle_inline};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InlineStyle {
    pub bold: bool,
    pub italic: bool,
    pub strikethrough: bool,
    pub code: bool,
    pub link: bool,
    pub image: bool,
    pub math: bool,
    pub footnote: bool,
    pub syntax: bool,
}

impl InlineStyle {
    #[must_use]
    pub fn is_plain(self) -> bool {
        self == Self::default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Text,
    Code,
    Math,
    Image,
    Html,
    Footnote,
}

#[derive(Clone, Debug)]
pub struct InlineToken {
    /// The text as it should appear visually (may differ from source for
    /// entities or trimmed code spans).
    pub text: String,
    /// The exact source bytes this run represents.
    pub source: Range<usize>,
    pub style: InlineStyle,
    pub kind: TokenKind,
    pub url: Option<String>,
    /// True when `text` is not a byte-for-byte copy of `source` (entities,
    /// soft line breaks, trimmed code spans). Mapping then snaps to the run's
    /// boundaries instead of interpolating.
    pub atomic: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkerKind {
    Strong,
    Emphasis,
    Strikethrough,
    Code,
    Link,
    Image,
    Math,
}

#[derive(Clone, Debug)]
pub struct InlineMarker {
    /// The delimiter bytes that can be hidden.
    pub range: Range<usize>,
    /// The complete inline construct, used to decide when to reveal.
    pub outer: Range<usize>,
    pub kind: MarkerKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListKind {
    Bullet(char),
    Ordered(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListMarker {
    pub kind: ListKind,
    pub checked: Option<bool>,
    /// Marker + task box + following spaces.
    pub source: Range<usize>,
    /// Number of nested list levels (1 for a top-level list).
    pub depth: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrefixKind {
    Heading,
    ListMarker,
    Task,
    Fence,
    Quote,
}

#[derive(Clone, Debug)]
pub struct Prefix {
    pub range: Range<usize>,
    pub kind: PrefixKind,
}

#[derive(Clone, Debug)]
pub struct CodeBlockInfo {
    pub language: Option<String>,
    pub fenced: bool,
    pub info: String,
}

#[derive(Clone, Debug)]
pub struct ImageInfo {
    pub alt: String,
    pub url: String,
    pub source: Range<usize>,
}

#[derive(Clone, Debug)]
pub enum BlockKind {
    Paragraph,
    Heading(u8),
    CodeBlock(CodeBlockInfo),
    Rule,
    Table,
    Html,
    Image(ImageInfo),
}

#[derive(Clone, Debug)]
pub struct TableCell {
    pub range: Range<usize>,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct TableRowData {
    pub header: bool,
    pub cells: Vec<TableCell>,
}

#[derive(Clone, Debug)]
pub struct TableData {
    pub alignments: Vec<Alignment>,
    pub rows: Vec<TableRowData>,
}

#[derive(Clone, Debug)]
pub struct Block {
    pub kind: BlockKind,
    /// The complete source range of this leaf block, including trailing
    /// newline and block syntax.
    pub range: Range<usize>,
    /// The editable content range. For block quotes this can contain `> `
    /// gaps on continuation lines; those bytes are simply not projected.
    pub content: Range<usize>,
    pub prefix: Vec<Prefix>,
    pub quote_depth: usize,
    pub list_depth: usize,
    pub list_marker: Option<ListMarker>,
    pub line: usize,
    pub tokens: Vec<InlineToken>,
    pub markers: Vec<InlineMarker>,
    pub table: Option<TableData>,
    pub image: Option<ImageInfo>,
}

impl Block {
    #[must_use]
    pub fn plain_text(&self) -> String {
        self.tokens.iter().map(|t| t.text.as_str()).collect()
    }

    #[must_use]
    pub fn is_empty_paragraph(&self) -> bool {
        matches!(self.kind, BlockKind::Paragraph) && self.tokens.is_empty()
    }

    #[must_use]
    pub fn heading_level(&self) -> Option<u8> {
        if let BlockKind::Heading(level) = self.kind {
            Some(level)
        } else {
            None
        }
    }

    #[must_use]
    pub fn is_code(&self) -> bool {
        matches!(self.kind, BlockKind::CodeBlock(_))
    }

    /// Byte offset just past this block's last source character, excluding
    /// trailing newlines but including a closing fence. A caret at this offset
    /// is still on the block's last line; one past its line newline is not.
    #[must_use]
    pub fn edit_end(&self) -> usize {
        self.prefix
            .iter()
            .filter(|p| p.kind == PrefixKind::Fence)
            .map(|p| p.range.end)
            .fold(self.content.end, usize::max)
    }
}

#[derive(Clone, Debug)]
pub struct OutlineItem {
    pub level: u8,
    pub text: String,
    pub byte: usize,
    pub line: usize,
}

#[derive(Clone, Debug, Default)]
pub struct ParsedDocument {
    pub blocks: Vec<Block>,
    pub outline: Vec<OutlineItem>,
}

impl ParsedDocument {
    /// The block whose range contains `byte`, or the nearest block before it.
    #[must_use]
    pub fn block_at(&self, byte: usize) -> Option<usize> {
        if self.blocks.is_empty() {
            return None;
        }
        let mut result = 0;
        for (i, block) in self.blocks.iter().enumerate() {
            if block.range.start <= byte {
                result = i;
            } else {
                break;
            }
        }
        Some(result)
    }
}

struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    fn new(source: &str) -> Self {
        let mut starts = vec![0];
        for (i, b) in source.bytes().enumerate() {
            if b == b'\n' {
                starts.push(i + 1);
            }
        }
        Self { starts }
    }

    fn line_of(&self, byte: usize) -> usize {
        self.starts
            .partition_point(|&start| start <= byte)
            .saturating_sub(1)
    }
}

#[derive(Clone, Debug)]
struct ItemContext {
    content_start: usize,
    line_range: Range<usize>,
    marker: ListMarker,
    marker_emitted: bool,
}

struct ListContext {
    ordered: bool,
}

struct CellBuilder {
    range: Range<usize>,
    text: String,
}

struct Builder {
    kind: BlockKind,
    range: Range<usize>,
    content_start: usize,
    content_end: usize,
    prefix: Vec<Prefix>,
    quote_depth: usize,
    list_depth: usize,
    list_marker: Option<ListMarker>,
    #[allow(dead_code)]
    item_index: Option<usize>,
    inline: InlineCollector,
    code: String,
    table: Option<TableData>,
    current_row: Option<TableRowData>,
    current_cell: Option<CellBuilder>,
    in_head: bool,
}

impl Builder {
    #[allow(clippy::too_many_arguments)]
    fn new(
        kind: BlockKind,
        range: Range<usize>,
        content_start: usize,
        content_end: usize,
        quote_depth: usize,
        list_depth: usize,
        list_marker: Option<ListMarker>,
        item_index: Option<usize>,
    ) -> Self {
        Self {
            kind,
            range,
            content_start,
            content_end,
            prefix: Vec::new(),
            quote_depth,
            list_depth,
            list_marker,
            item_index,
            inline: InlineCollector::default(),
            code: String::new(),
            table: None,
            current_row: None,
            current_cell: None,
            in_head: false,
        }
    }

    fn expand_to(&mut self, range: &Range<usize>) {
        self.range.start = self.range.start.min(range.start);
        self.range.end = self.range.end.max(range.end);
        self.content_start = self.content_start.min(range.start);
        self.content_end = self.content_end.max(range.end);
    }
}

/// Parse the source into blocks. This is intentionally a full reparse: a
/// typical note parses in well under a millisecond, and a single code path is
/// much easier to keep correct than an incremental parser.
#[must_use]
pub fn parse(source: &str) -> ParsedDocument {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_MATH
        | Options::ENABLE_GFM
        | Options::ENABLE_HEADING_ATTRIBUTES;
    let line_index = LineIndex::new(source);
    let mut blocks: Vec<Block> = Vec::new();
    let mut current: Option<Builder> = None;
    let mut quote_depth = 0usize;
    let mut lists: Vec<ListContext> = Vec::new();
    let mut items: Vec<ItemContext> = Vec::new();
    let mut item_blocks_start = 0usize;

    for (event, range) in Parser::new_ext(source, options).into_offset_iter() {
        match &event {
            Event::Start(Tag::BlockQuote(_)) => {
                if let Some(builder) = current.take() {
                    finish_builder(builder, source, &line_index, &mut blocks);
                }
                quote_depth += 1;
                continue;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                quote_depth = quote_depth.saturating_sub(1);
                continue;
            }
            Event::Start(Tag::List(number)) => {
                if let Some(builder) = current.take() {
                    finish_builder(builder, source, &line_index, &mut blocks);
                }
                lists.push(ListContext {
                    ordered: number.is_some(),
                });
                continue;
            }
            Event::End(TagEnd::List(_)) => {
                lists.pop();
                continue;
            }
            Event::Start(Tag::Item) => {
                if let Some(builder) = current.take() {
                    finish_builder(builder, source, &line_index, &mut blocks);
                }
                let marker =
                    parse_item_marker(source, range.start, lists.last().is_some_and(|l| l.ordered));
                let line_end = source[range.start..]
                    .find('\n')
                    .map_or(source.len(), |offset| range.start + offset);
                let item = ItemContext {
                    content_start: marker.1,
                    line_range: range.start..line_end,
                    marker: marker.0,
                    marker_emitted: false,
                };
                items.push(item);
                item_blocks_start = blocks.len();
                continue;
            }
            Event::End(TagEnd::Item) => {
                if let Some(builder) = current.take() {
                    finish_builder(builder, source, &line_index, &mut blocks);
                }
                if blocks.len() == item_blocks_start
                    && let Some(item) = items.last()
                {
                    let mut marker = item.marker.clone();
                    if item.marker_emitted {
                        // Only an empty item can reach this branch without a
                        // marker having been consumed.
                    } else {
                        marker.checked = item.marker.checked;
                    }
                    let block = Block {
                        kind: BlockKind::Paragraph,
                        range: item.line_range.clone(),
                        content: item.content_start..item.content_start,
                        prefix: vec![Prefix {
                            range: item.marker.source.clone(),
                            kind: if item.marker.checked.is_some() {
                                PrefixKind::Task
                            } else {
                                PrefixKind::ListMarker
                            },
                        }],
                        quote_depth,
                        list_depth: lists.len(),
                        list_marker: Some(marker),
                        line: line_index.line_of(item.content_start),
                        tokens: Vec::new(),
                        markers: Vec::new(),
                        table: None,
                        image: None,
                    };
                    blocks.push(block);
                }
                items.pop();
                continue;
            }
            Event::Start(Tag::Paragraph) => {
                begin_paragraph(&mut current, source, range, quote_depth, &lists, &mut items);
                continue;
            }
            Event::End(TagEnd::Paragraph) => {
                if let Some(builder) = current.take()
                    && matches!(builder.kind, BlockKind::Paragraph)
                {
                    finish_builder(builder, source, &line_index, &mut blocks);
                }
                continue;
            }
            Event::Start(Tag::Heading { level, .. }) => {
                let level = heading_level(*level);
                let (content_start, content_end, prefixes) = heading_content(source, &range);
                let mut builder = Builder::new(
                    BlockKind::Heading(level),
                    range.clone(),
                    content_start,
                    content_end,
                    quote_depth,
                    lists.len(),
                    take_list_marker(&mut items),
                    items.len().checked_sub(1),
                );
                builder.prefix = prefixes;
                current = Some(builder);
                continue;
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some(builder) = current.take() {
                    finish_builder(builder, source, &line_index, &mut blocks);
                }
                continue;
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                let (language, info, fenced) = code_info(kind);
                let (content_start, content_end, prefixes) = fenced_content(source, &range, fenced);
                let mut builder = Builder::new(
                    BlockKind::CodeBlock(CodeBlockInfo {
                        language,
                        fenced,
                        info,
                    }),
                    range.clone(),
                    content_start,
                    content_end,
                    quote_depth,
                    lists.len(),
                    take_list_marker(&mut items),
                    items.len().checked_sub(1),
                );
                builder.prefix = prefixes;
                current = Some(builder);
                continue;
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some(builder) = current.take() {
                    finish_builder(builder, source, &line_index, &mut blocks);
                }
                continue;
            }
            Event::Start(Tag::HtmlBlock) => {
                let (content_start, content_end) = trim_block_newline(source, &range);
                current = Some(Builder::new(
                    BlockKind::Html,
                    range.clone(),
                    content_start,
                    content_end,
                    quote_depth,
                    lists.len(),
                    take_list_marker(&mut items),
                    items.len().checked_sub(1),
                ));
                continue;
            }
            Event::End(TagEnd::HtmlBlock) => {
                if let Some(builder) = current.take() {
                    finish_builder(builder, source, &line_index, &mut blocks);
                }
                continue;
            }
            Event::Start(Tag::Table(alignments)) => {
                let (content_start, content_end) = trim_block_newline(source, &range);
                let mut builder = Builder::new(
                    BlockKind::Table,
                    range.clone(),
                    content_start,
                    content_end,
                    quote_depth,
                    lists.len(),
                    take_list_marker(&mut items),
                    items.len().checked_sub(1),
                );
                builder.table = Some(TableData {
                    alignments: alignments.clone(),
                    rows: Vec::new(),
                });
                current = Some(builder);
                continue;
            }
            Event::End(TagEnd::Table) => {
                if let Some(mut builder) = current.take() {
                    if let Some(row) = builder.current_row.take()
                        && let Some(table) = &mut builder.table
                    {
                        table.rows.push(row);
                    }
                    finish_builder(builder, source, &line_index, &mut blocks);
                }
                continue;
            }
            Event::Start(Tag::TableHead) => {
                if let Some(builder) = current.as_mut() {
                    builder.in_head = true;
                    builder.current_row = Some(TableRowData {
                        header: true,
                        cells: Vec::new(),
                    });
                }
                continue;
            }
            Event::End(TagEnd::TableHead) => {
                if let Some(builder) = current.as_mut() {
                    builder.in_head = false;
                    if let Some(row) = builder.current_row.take()
                        && let Some(table) = &mut builder.table
                    {
                        table.rows.push(row);
                    }
                }
                continue;
            }
            Event::Start(Tag::TableRow) => {
                if let Some(builder) = current.as_mut() {
                    if let Some(row) = builder.current_row.take()
                        && let Some(table) = &mut builder.table
                    {
                        table.rows.push(row);
                    }
                    builder.current_row = Some(TableRowData {
                        header: builder.in_head,
                        cells: Vec::new(),
                    });
                }
                continue;
            }
            Event::End(TagEnd::TableRow) => {
                if let Some(builder) = current.as_mut()
                    && let Some(row) = builder.current_row.take()
                    && let Some(table) = &mut builder.table
                {
                    table.rows.push(row);
                }
                continue;
            }
            Event::Start(Tag::TableCell) => {
                if let Some(builder) = current.as_mut() {
                    builder.current_cell = Some(CellBuilder {
                        range: range.clone(),
                        text: String::new(),
                    });
                }
                continue;
            }
            Event::End(TagEnd::TableCell) => {
                if let Some(builder) = current.as_mut()
                    && let Some(cell) = builder.current_cell.take()
                    && let Some(row) = builder.current_row.as_mut()
                {
                    let trimmed = trim_range(source, &cell.range);
                    row.cells.push(TableCell {
                        range: trimmed,
                        text: cell.text.trim().to_owned(),
                    });
                }
                continue;
            }
            Event::Rule => {
                blocks.push(Block {
                    kind: BlockKind::Rule,
                    range: range.clone(),
                    content: range.start..range.start,
                    prefix: Vec::new(),
                    quote_depth,
                    list_depth: lists.len(),
                    list_marker: None,
                    line: line_index.line_of(range.start),
                    tokens: Vec::new(),
                    markers: Vec::new(),
                    table: None,
                    image: None,
                });
                continue;
            }
            Event::Start(Tag::FootnoteDefinition(_)) => {
                if let Some(builder) = current.take() {
                    finish_builder(builder, source, &line_index, &mut blocks);
                }
                continue;
            }
            Event::End(TagEnd::FootnoteDefinition) => {
                if let Some(builder) = current.take() {
                    finish_builder(builder, source, &line_index, &mut blocks);
                }
                continue;
            }
            _ => {}
        }

        // The code block and table consume their textual content directly.
        if let Some(builder) = current.as_mut() {
            match builder.kind {
                BlockKind::CodeBlock(_) => {
                    if let Event::Text(text) | Event::Html(text) = &event {
                        builder.code.push_str(text);
                        builder.expand_to(&range);
                    }
                    continue;
                }
                BlockKind::Table => {
                    if let Some(cell) = builder.current_cell.as_mut() {
                        match &event {
                            Event::Text(text) | Event::Code(text) | Event::Html(text) => {
                                cell.text.push_str(text)
                            }
                            Event::SoftBreak | Event::HardBreak => cell.text.push(' '),
                            Event::InlineMath(text) | Event::DisplayMath(text) => {
                                cell.text.push_str(text)
                            }
                            _ => {}
                        }
                        cell.range.start = cell.range.start.min(range.start);
                        cell.range.end = cell.range.end.max(range.end);
                    }
                    continue;
                }
                BlockKind::Html => {
                    if let Event::Html(text) = &event {
                        builder.code.push_str(text);
                        builder.expand_to(&range);
                    }
                    continue;
                }
                _ => {}
            }
        }

        // A tight list item has no Paragraph tag, so the first inline event
        // creates the implicit paragraph block.
        if current.is_none()
            && matches!(
                event,
                Event::Text(_)
                    | Event::Code(_)
                    | Event::SoftBreak
                    | Event::HardBreak
                    | Event::Start(_)
            )
        {
            let (content_start, content_end) = match &items.last() {
                Some(item) => (item.content_start, item.content_start),
                None => (range.start, range.end),
            };
            current = Some(Builder::new(
                BlockKind::Paragraph,
                range.clone(),
                content_start,
                content_end,
                quote_depth,
                lists.len(),
                take_list_marker(&mut items),
                items.len().checked_sub(1),
            ));
        }

        let Some(builder) = current.as_mut() else {
            continue;
        };

        if handle_inline(&mut builder.inline, &event, range.clone(), source) {
            builder.expand_to(&range);
        }
    }

    if let Some(builder) = current.take() {
        finish_builder(builder, source, &line_index, &mut blocks);
    }

    blocks.sort_by_key(|block| (block.range.start, block.range.end));
    let outline = blocks
        .iter()
        .filter_map(|block| {
            let level = block.heading_level()?;
            let text = block.plain_text();
            Some(OutlineItem {
                level,
                text,
                byte: block.content.start,
                line: block.line,
            })
        })
        .collect();

    ParsedDocument { blocks, outline }
}

fn begin_paragraph(
    current: &mut Option<Builder>,
    source: &str,
    range: Range<usize>,
    quote_depth: usize,
    lists: &[ListContext],
    items: &mut [ItemContext],
) {
    if current.is_some() {
        return;
    }
    let raw = &source[range.clone()];
    let trimmed = raw.trim_end_matches(['\n', '\r']).len();
    let content_start = range.start;
    let content_end = range.start + trimmed;
    let list_depth = lists.len();
    // A tight list item starts the paragraph at its first inline event, not at
    // the item marker. Include the marker in the block range without moving
    // the editable content start.
    let block_range = if items.last().is_some_and(|item| !item.marker_emitted) {
        items.last().map_or(range.clone(), |item| {
            item.marker.source.start.min(item.line_range.start)..range.end
        })
    } else {
        range
    };
    *current = Some(Builder::new(
        BlockKind::Paragraph,
        block_range,
        content_start,
        content_end,
        quote_depth,
        list_depth,
        take_list_marker(items),
        items.len().checked_sub(1),
    ));
}

fn take_list_marker(items: &mut [ItemContext]) -> Option<ListMarker> {
    let item = items.last_mut()?;
    if item.marker_emitted {
        return None;
    }
    item.marker_emitted = true;
    Some(item.marker.clone())
}

fn finish_builder(builder: Builder, source: &str, line_index: &LineIndex, blocks: &mut Vec<Block>) {
    let Builder {
        kind,
        range,
        content_start,
        content_end,
        mut prefix,
        quote_depth,
        list_depth,
        list_marker,
        inline,
        mut table,
        ..
    } = builder;

    let (tokens, markers, image) = inline.into_parts();
    let mut content_end = content_end.max(content_start);
    for token in &tokens {
        content_end = content_end.max(token.source.end);
    }
    for marker in &markers {
        content_end = content_end.max(marker.range.end);
    }
    let content = content_start.min(source.len())..content_end.min(source.len());

    let mut kind = kind;
    let mut image_info = None;

    if let (Some((image_range, alt, url)), BlockKind::Paragraph) = (&image, &kind) {
        let only_image = !tokens.is_empty()
            && tokens
                .iter()
                .all(|token| token.style.image || token.text.trim().is_empty());
        if only_image {
            image_info = Some(ImageInfo {
                alt: alt.clone(),
                url: url.clone(),
                source: image_range.clone(),
            });
            kind = BlockKind::Image(image_info.clone().unwrap());
        }
    }

    // Fenced code content is derived from the source because pulldown's text
    // event can normalize line endings.
    let content = if let BlockKind::CodeBlock(info) = &kind {
        if info.fenced {
            fenced_content_range(source, &range)
        } else {
            content.clone()
        }
    } else {
        content
    };

    if let BlockKind::CodeBlock(info) = &kind
        && info.fenced
    {
        for p in fenced_prefixes(source, &range) {
            prefix.push(p);
        }
    }

    // A paragraph with no tokens and no item marker is just structural noise.
    if matches!(kind, BlockKind::Paragraph)
        && tokens.is_empty()
        && list_marker.is_none()
        && content.is_empty()
    {
        return;
    }

    blocks.push(Block {
        kind,
        range,
        content,
        prefix,
        quote_depth,
        list_depth,
        list_marker,
        line: line_index.line_of(content_start),
        tokens,
        markers,
        table: table.take(),
        image: image_info,
    });
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn code_info(kind: &CodeBlockKind<'_>) -> (Option<String>, String, bool) {
    match kind {
        CodeBlockKind::Fenced(info) => {
            let info = info.trim().to_owned();
            let language = info
                .split_whitespace()
                .next()
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            (language, info, true)
        }
        CodeBlockKind::Indented => (None, String::new(), false),
    }
}

fn trim_block_newline(source: &str, range: &Range<usize>) -> (usize, usize) {
    let raw = &source[range.clone()];
    let trimmed = raw.trim_end_matches(['\n', '\r']).len();
    (range.start, range.start + trimmed)
}

/// ATX headings need their `# ` prefix stripped; setext headings need their
/// underline line stripped. Both return content bounds and hidden prefixes.
fn heading_content(source: &str, range: &Range<usize>) -> (usize, usize, Vec<Prefix>) {
    let raw = &source[range.clone()];
    let first_line_end = raw.find('\n').unwrap_or(raw.len());
    let first_line = &raw[..first_line_end];
    let no_newline = raw.trim_end_matches(['\n', '\r']);
    let mut prefixes = Vec::new();

    let indent = first_line.len() - first_line.trim_start_matches(' ').len();
    let after_indent = &first_line[indent..];
    if after_indent.starts_with('#') {
        let hashes = after_indent.chars().take_while(|c| *c == '#').count();
        let mut content_start = range.start + indent + hashes;
        let bytes = source.as_bytes();
        while content_start < range.start + first_line_end
            && matches!(bytes.get(content_start), Some(b' ' | b'\t'))
        {
            content_start += 1;
        }
        let mut content_end = range.start + no_newline.len();
        // Optional closing sequence: spaces followed by hashes.
        let line = &source[range.start + indent..content_end];
        if let Some(hash_pos) = line.rfind('#') {
            let before = &line[..hash_pos];
            if before.ends_with(' ') || before.ends_with('\t') {
                let trimmed = before.trim_end();
                content_end = range.start + indent + trimmed.len();
            }
        }
        if range.start < content_start {
            prefixes.push(Prefix {
                range: range.start..content_start,
                kind: PrefixKind::Heading,
            });
        }
        if content_end < range.start + no_newline.len() {
            prefixes.push(Prefix {
                range: content_end..range.start + no_newline.len(),
                kind: PrefixKind::Heading,
            });
        }
        return (content_start, content_end.max(content_start), prefixes);
    }

    // Setext heading: the last non-empty line is the underline.
    if let Some(last_line_start) = no_newline.rfind('\n').map(|i| range.start + i + 1) {
        let underline = source[last_line_start..range.start + no_newline.len()].trim();
        if !underline.is_empty()
            && (underline.chars().all(|c| c == '=') || underline.chars().all(|c| c == '-'))
        {
            let content_end = source[..last_line_start]
                .trim_end_matches(['\n', '\r'])
                .len();
            prefixes.push(Prefix {
                range: last_line_start..range.end,
                kind: PrefixKind::Heading,
            });
            return (range.start, content_end, prefixes);
        }
    }

    (range.start, range.start + no_newline.len(), prefixes)
}

/// The content range and the closing-fence line (when the fence is closed)
/// for a fenced code block.
fn fenced_parts(source: &str, range: &Range<usize>) -> (Range<usize>, Option<Range<usize>>) {
    let raw = &source[range.clone()];
    let Some(first_newline) = raw.find('\n') else {
        return (range.start..range.start, None);
    };
    let content_start = range.start + first_newline + 1;
    let block_end = range.start + raw.trim_end_matches(['\n', '\r']).len();
    if content_start >= block_end {
        return (content_start..content_start, None);
    }
    let last_line_start = source[content_start..block_end]
        .rfind('\n')
        .map_or(content_start, |i| content_start + i + 1);
    let last_line = source[last_line_start..block_end].trim_start();
    if is_fence_line(last_line) {
        let content_end = source[..last_line_start]
            .trim_end_matches(['\n', '\r'])
            .len()
            .max(content_start);
        return (content_start..content_end, Some(last_line_start..range.end));
    }
    (content_start..block_end, None)
}

/// A line that holds nothing but a run of at least three backticks or tildes.
fn is_fence_line(line: &str) -> bool {
    let line = line.trim();
    let mut chars = line.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if first != '`' && first != '~' {
        return false;
    }
    let count = line.chars().take_while(|c| *c == first).count();
    count >= 3 && line[count..].trim().is_empty()
}

/// The content range (between fences) for a fenced code block.
fn fenced_content_range(source: &str, range: &Range<usize>) -> Range<usize> {
    fenced_parts(source, range).0
}

fn fenced_prefixes(source: &str, range: &Range<usize>) -> Vec<Prefix> {
    let mut prefixes = Vec::new();
    let raw = &source[range.clone()];
    if let Some(first_newline) = raw.find('\n') {
        prefixes.push(Prefix {
            range: range.start..range.start + first_newline + 1,
            kind: PrefixKind::Fence,
        });
    }
    if let (_, Some(closing)) = fenced_parts(source, range) {
        prefixes.push(Prefix {
            range: closing,
            kind: PrefixKind::Fence,
        });
    }
    prefixes
}

fn fenced_content(source: &str, range: &Range<usize>, fenced: bool) -> (usize, usize, Vec<Prefix>) {
    if fenced {
        let content = fenced_content_range(source, range);
        (content.start, content.end, Vec::new())
    } else {
        let raw = &source[range.clone()];
        let trimmed = raw.trim_end_matches(['\n', '\r']).len();
        (range.start, range.start + trimmed, Vec::new())
    }
}

fn trim_range(source: &str, range: &Range<usize>) -> Range<usize> {
    let raw = &source[range.clone()];
    let start = raw.len() - raw.trim_start().len();
    let end = raw.trim_end().len();
    (range.start + start)..(range.start + end.max(start))
}

/// Parse the leading `-`, `*`, `1.` and optional `[ ]` from an item line.
/// Returns the marker and the byte where editable content begins.
fn parse_item_marker(source: &str, item_start: usize, ordered: bool) -> (ListMarker, usize) {
    let line_end = source[item_start..]
        .find('\n')
        .map_or(source.len(), |o| item_start + o);
    let line = &source[item_start..line_end];
    let mut index = 0usize;
    while index < line.len() && (line.as_bytes()[index] == b' ' || line.as_bytes()[index] == b'\t')
    {
        index += 1;
    }
    let marker_start = item_start + index;
    let mut marker_end = marker_start;
    let mut kind = ListKind::Bullet('-');
    if index < line.len() {
        let rest = &line[index..];
        let first = rest.chars().next().unwrap_or('-');
        if matches!(first, '-' | '+' | '*') {
            kind = ListKind::Bullet(first);
            marker_end = marker_start + first.len_utf8();
        } else {
            let digits = rest.chars().take_while(char::is_ascii_digit).count();
            if digits > 0 {
                let after = marker_start + digits;
                if after < line_end {
                    let delim = source.as_bytes()[after];
                    if delim == b'.' || delim == b')' {
                        marker_end = after + 1;
                        kind = ListKind::Ordered(source[marker_start..marker_end].to_owned());
                    }
                }
            }
        }
    }
    if marker_end == marker_start {
        marker_end = marker_start;
    }
    let mut content_start = marker_end;
    while content_start < line_end && matches!(source.as_bytes()[content_start], b' ' | b'\t') {
        content_start += 1;
    }

    let mut checked = None;
    if content_start + 2 < line_end
        && source.as_bytes()[content_start] == b'['
        && source.as_bytes()[content_start + 2] == b']'
    {
        let inner = source.as_bytes()[content_start + 1];
        if matches!(inner, b' ' | b'x' | b'X') {
            checked = Some(inner != b' ');
            let mut after = content_start + 3;
            while after < line_end && matches!(source.as_bytes()[after], b' ' | b'\t') {
                after += 1;
            }
            content_start = after;
        }
    }

    let _ = ordered;
    (
        ListMarker {
            kind,
            checked,
            source: marker_start..content_start,
            depth: 1,
        },
        content_start,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_headings_and_inline_ranges() {
        let md = "# Hello **world**\n\nA *small* paragraph with `code`.\n";
        let doc = parse(md);
        assert_eq!(doc.blocks.len(), 2);
        assert_eq!(doc.blocks[0].heading_level(), Some(1));
        assert_eq!(doc.blocks[0].plain_text(), "Hello world");
        assert_eq!(doc.blocks[0].content.start, 2);
        assert!(
            doc.blocks[0]
                .markers
                .iter()
                .any(|m| m.kind == MarkerKind::Strong)
        );
        assert_eq!(doc.blocks[1].plain_text(), "A small paragraph with code.");
        assert_eq!(doc.outline.len(), 1);
    }

    #[test]
    fn list_items_and_tasks() {
        let md = "- one\n- [x] done\n\n1. first\n";
        let doc = parse(md);
        assert_eq!(doc.blocks.len(), 3);
        assert!(doc.blocks[0].list_marker.is_some());
        assert_eq!(
            doc.blocks[1].list_marker.as_ref().unwrap().checked,
            Some(true)
        );
        assert_eq!(
            doc.blocks[2].list_marker.as_ref().unwrap().kind,
            ListKind::Ordered("1.".to_owned())
        );
    }

    #[test]
    fn tight_list_range_covers_all_inline_content() {
        let md = "- **Bold**, *italic*, ~~strikethrough~~, and `inline code` render in place.";
        let doc = parse(md);
        let block = &doc.blocks[0];
        let italic = md.find("italic").expect("italic");
        assert!(
            block.range.start <= italic && italic < block.range.end,
            "block range {:?} must contain italic at {italic}",
            block.range
        );
        assert_eq!(block.range.end, md.len());
    }

    #[test]
    fn quote_continuation_prefixes_are_gaps() {
        let md = "> first **bold**\n> second\n";
        let doc = parse(md);
        assert_eq!(doc.blocks.len(), 1);
        assert_eq!(doc.blocks[0].plain_text(), "first bold\nsecond");
        assert!(
            doc.blocks[0]
                .markers
                .iter()
                .any(|m| m.kind == MarkerKind::Strong)
        );
    }

    #[test]
    fn tight_nested_lists_are_separate_blocks() {
        let md = "- one\n  - two\n";
        let doc = parse(md);
        assert_eq!(doc.blocks.len(), 2);
        assert_eq!(doc.blocks[0].plain_text(), "one");
        assert_eq!(doc.blocks[1].plain_text(), "two");
        assert_eq!(doc.blocks[0].list_depth, 1);
        assert_eq!(doc.blocks[1].list_depth, 2);
    }

    #[test]
    fn fenced_code_content_excludes_fence() {
        let md = "```rust\nfn main() {}\n```\n";
        let doc = parse(md);
        assert_eq!(doc.blocks.len(), 1);
        let BlockKind::CodeBlock(info) = &doc.blocks[0].kind else {
            panic!("expected code");
        };
        assert_eq!(info.language.as_deref(), Some("rust"));
        assert_eq!(&md[doc.blocks[0].content.clone()], "fn main() {}");
    }

    #[test]
    fn empty_fences_do_not_panic_and_have_no_content() {
        for md in ["```", "```\n", "```rust", "```rust\n", "```\n```\n"] {
            let doc = parse(md);
            let block = &doc.blocks[0];
            assert!(block.content.is_empty(), "{md:?} content");
        }
        let md = "```rust\n```";
        let doc = parse(md);
        assert_eq!(doc.blocks[0].content.start, 8);
        assert_eq!(
            doc.blocks[0]
                .prefix
                .iter()
                .filter(|p| p.kind == PrefixKind::Fence)
                .count(),
            2
        );
    }

    #[test]
    fn closing_fence_prefix_starts_on_its_own_line() {
        let md = "```\ncode\n```\n";
        let doc = parse(md);
        let closing = doc.blocks[0]
            .prefix
            .iter()
            .find(|p| p.kind == PrefixKind::Fence && p.range.start > 0)
            .expect("closing fence");
        assert_eq!(&md[closing.range.clone()], "```");
    }

    #[test]
    fn table_rows_are_structured() {
        let md = "| a | b |\n|---|:--:|\n| 1 | 2 |\n";
        let doc = parse(md);
        let table = doc.blocks[0].table.as_ref().unwrap();
        assert_eq!(table.rows.len(), 2);
        assert_eq!(table.rows[0].cells[0].text, "a");
        assert_eq!(table.rows[1].cells[1].text, "2");
    }

    #[test]
    fn edit_end_excludes_the_trailing_newline_but_keeps_closing_fences() {
        let md = "| a | b |\n|---|---|\n| 1 | 2 |\n\nbelow\n";
        let doc = parse(md);
        let table = &doc.blocks[1];
        assert_eq!(table.edit_end(), table.content.end);
        assert!(table.edit_end() < table.range.end);

        let md = "```\ncode\n```\n";
        let doc = parse(md);
        assert_eq!(doc.blocks[0].edit_end(), 12);
    }
}
