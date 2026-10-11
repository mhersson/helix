//! In-buffer Markdown rendering: decorations computed from the tree-sitter
//! `markdown` and `markdown.inline` trees, see [`decorate`].

use std::collections::{BTreeMap, HashSet};
use std::ops::Range;
use std::path::Path;

use helix_core::line_ending::line_end_char_index;
use helix_core::syntax::{Layer, Loader, Syntax};
use helix_core::text_annotations::{Conceal, InlineAnnotation, LineAnnotation, Overlay};
use helix_core::tree_sitter::Node;
use helix_core::RopeSlice;

use crate::annotations::diagnostics::DiagnosticFilter;
use crate::document::Mode;
use crate::images::ImageState;
use crate::{Document, Editor, View, ViewId};

/// Decorations of the blocks touching a range, see [`decorate`].
#[derive(Debug, Default, Clone)]
pub struct Decorations {
    /// Sorted, non-overlapping.
    pub blocks: Vec<Block>,
    /// Length of the decorated text: a cursor at the end of the text lies in
    /// the block that ends there.
    len_chars: usize,
}

/// The unit of reveal: a paragraph, heading, table, code block or thematic
/// break, extended to whole lines. Lines with decorations but no such block
/// (an empty list item) form their own block.
#[derive(Debug, Default, Clone)]
pub struct Block {
    pub range: Range<usize>,
    pub conceals: Vec<Conceal>,
    pub overlays: Vec<Overlay>,
    pub padding: Vec<InlineAnnotation>,
    /// Local images on a single line, sorted by line then position.
    pub images: Vec<ImageRef>,
    /// Terminated mermaid fences, sorted by first line.
    pub diagrams: Vec<DiagramRef>,
    /// Lines ending in a hard line break made of spaces, sorted.
    pub hard_breaks: Vec<usize>,
    /// Single-line ATX and setext headings, sorted by line.
    pub headings: Vec<HeadingRef>,
    /// Pipe tables, sorted by first line.
    pub tables: Vec<TableRef>,
}

/// An ATX or setext heading whose text is on one line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadingRef {
    pub line: usize,
    pub level: u8,
    /// Char range of the heading text, trailing whitespace and any ATX
    /// closing sequence excluded.
    pub content_start: usize,
    pub content_end: usize,
}

/// A heading of a view as drawn when its block is decorated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadingRecord {
    pub line: usize,
    pub level: u8,
    /// Char range of the text, trailing whitespace excluded.
    pub content_start: usize,
    pub content_end: usize,
    /// Shown cells before the text (quote bars).
    pub prefix_width: usize,
    /// Shown cells of the text.
    pub content_width: usize,
    /// Shown graphemes of the text.
    pub graphemes: usize,
    /// A shown grapheme is wider than one cell.
    pub has_wide: bool,
    /// Its block is shown raw (insert mode).
    pub revealed: bool,
}

/// A Markdown image `![caption](destination)` that fits on one line.
#[derive(Debug, Clone)]
pub struct ImageRef {
    pub line: usize,
    pub destination: String,
    /// Conceals hiding everything but the caption text; deliberately not part
    /// of [`Block::conceals`].
    pub caption: [Conceal; 2],
}

/// A terminated ```` ```mermaid ```` fenced code block.
#[derive(Debug, Clone)]
pub struct DiagramRef {
    /// Document lines of the block, opening and closing fence included.
    pub lines: Range<usize>,
    /// The text between the fences, without block quote or list markers.
    pub source: String,
    /// One conceal per non-empty line hiding the whole line (line break
    /// excluded); deliberately not part of [`Block::conceals`].
    pub conceals: Vec<Conceal>,
}

/// A pipe table, as needed to draw its top and bottom borders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRef {
    /// Document lines of the rows.
    pub lines: Range<usize>,
    /// Shown before the delimiter row's rule: block quote bars, everything
    /// else as spaces.
    pub prefix: String,
    /// Width of each cell of the delimiter row as drawn.
    pub widths: Vec<usize>,
    /// The delimiter row has a cell before its first pipe (no left edge).
    pub leading: bool,
    /// The delimiter row has a cell after its last pipe (no right edge).
    pub trailing: bool,
}

impl TableRef {
    /// The top (`┌─┬─┐`) or bottom (`└─┴─┘`) border, lined up with the
    /// delimiter row's `├─┼─┤`, without the prefix.
    pub fn border(&self, top: bool) -> String {
        let (left, junction, right) = if top {
            ("┌", "┬", "┐")
        } else {
            ("└", "┴", "┘")
        };
        let mut out = String::new();
        if !self.leading {
            out.push_str(left);
        }
        for (i, &width) in self.widths.iter().enumerate() {
            if i > 0 {
                out.push_str(junction);
            }
            out.push_str(&"─".repeat(width));
        }
        if !self.trailing {
            out.push_str(right);
        }
        out
    }
}

/// Annotation layers ready for a view, each sorted.
#[derive(Debug, Default)]
pub struct Layers {
    pub conceals: Vec<Conceal>,
    pub overlays: Vec<Overlay>,
    pub padding: Vec<InlineAnnotation>,
    pub hard_breaks: Vec<usize>,
}

impl Decorations {
    /// Indices of the blocks containing any of `cursors`.
    pub fn revealed(&self, cursors: &[usize]) -> Vec<usize> {
        self.blocks
            .iter()
            .enumerate()
            .filter(|(_, block)| {
                cursors.iter().any(|&cursor| {
                    block.range.contains(&cursor)
                        || (cursor == self.len_chars && block.range.end == self.len_chars)
                })
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// Start chars of the blocks containing any of `cursors`. Unlike indices
    /// these do not shift when the decorated range changes.
    pub fn revealed_starts(&self, cursors: &[usize]) -> Vec<usize> {
        self.revealed(cursors)
            .into_iter()
            .map(|i| self.blocks[i].range.start)
            .collect()
    }

    /// The layers of every block except `revealed`.
    pub fn layers(&self, revealed: &[usize]) -> Layers {
        let mut layers = Layers::default();
        for (i, block) in self.blocks.iter().enumerate() {
            if revealed.contains(&i) {
                continue;
            }
            layers.conceals.extend_from_slice(&block.conceals);
            layers.overlays.extend_from_slice(&block.overlays);
            layers.padding.extend_from_slice(&block.padding);
            layers.hard_breaks.extend_from_slice(&block.hard_breaks);
        }
        layers
    }
}

fn grammar<'a>(loader: &'a Loader, syntax: &Syntax, layer: Layer) -> &'a str {
    let config = loader.language(syntax.layer(layer).language).config();
    config.grammar.as_deref().unwrap_or(&config.language_id)
}

/// Whether the root layer of `syntax` uses the markdown grammar.
pub fn is_markdown_syntax(syntax: &Syntax, loader: &Loader) -> bool {
    grammar(loader, syntax, syntax.root_layer()) == "markdown"
}

/// The `markdown_inline` layers injected directly into the root layer that
/// overlap the byte range, each once. Layers injected into code blocks
/// (including fenced Markdown) are not descended into.
fn inline_layers(syntax: &Syntax, loader: &Loader, range: &Range<u32>) -> Vec<Layer> {
    let root = syntax.root_layer();
    let mut out: Vec<Layer> = Vec::new();
    for injection in syntax
        .layer(root)
        .injections_at_byte_idx(range.start)
        .take_while(|injection| injection.range.start < range.end)
    {
        // a layer has one injection per range; walk each layer only once
        if !out.contains(&injection.layer)
            && grammar(loader, syntax, injection.layer) == "markdown_inline"
        {
            out.push(injection.layer);
        }
    }
    out
}

/// Computes the decorations of every block touching `range` (chars). Tables
/// touching `range` are decorated in full. `text_width` is the width that
/// thematic breaks span.
pub fn decorate(
    syntax: &Syntax,
    text: RopeSlice,
    range: Range<usize>,
    text_width: usize,
    loader: &Loader,
) -> Decorations {
    if !is_markdown_syntax(syntax, loader) {
        return Decorations::default();
    }
    let bytes = text.char_to_byte(range.start) as u32..text.char_to_byte(range.end) as u32;
    let mut builder = Builder::new(text, text_width);

    let root = syntax.root_layer();
    if let Some(tree) = syntax.layer(root).tree() {
        builder.walk_block(tree.root_node(), &bytes, 0);
    }
    // tables are decorated in full, so their cells need inline conceals even
    // outside `range`
    let mut inline_bytes = bytes.clone();
    for table in &builder.tables {
        inline_bytes.start = inline_bytes.start.min(table.start_byte());
        inline_bytes.end = inline_bytes.end.max(table.end_byte());
    }
    for layer in inline_layers(syntax, loader, &inline_bytes) {
        if let Some(tree) = syntax.layer(layer).tree() {
            builder.walk_inline(tree.root_node(), &inline_bytes);
        }
    }
    builder.finish_tables();
    builder.finish()
}

enum Item {
    Conceal(Conceal),
    Overlay(Overlay),
    Padding(InlineAnnotation),
    /// Char position of a hard line break made of spaces.
    HardBreak(usize),
}

impl Item {
    fn pos(&self) -> usize {
        match self {
            Item::Conceal(conceal) => conceal.range.start,
            Item::Overlay(overlay) => overlay.char_idx,
            Item::Padding(padding) => padding.char_idx,
            Item::HardBreak(pos) => *pos,
        }
    }
}

fn overlaps(node: &Node, range: &Range<u32>) -> bool {
    node.start_byte() < range.end && node.end_byte() > range.start
}

/// The block containing `pos`, created on first use.
fn block_at<'b>(
    text: RopeSlice,
    spans: &[Range<usize>],
    blocks: &'b mut BTreeMap<usize, Block>,
    pos: usize,
) -> &'b mut Block {
    let line = text.char_to_line(pos.min(text.len_chars()));
    let i = spans.partition_point(|span| span.end <= line);
    let lines = match spans.get(i) {
        Some(span) if span.start <= line => span.clone(),
        _ => line..line + 1,
    };
    blocks.entry(lines.start).or_insert_with(|| Block {
        range: text.line_to_char(lines.start)..text.line_to_char(lines.end.min(text.len_lines())),
        ..Block::default()
    })
}

struct Builder<'a> {
    text: RopeSlice<'a>,
    text_width: usize,
    items: Vec<Item>,
    /// Line spans of leaf blocks.
    leaves: Vec<Range<usize>>,
    /// Tables are decorated after inline conceals are known.
    tables: Vec<Node<'a>>,
    /// Images with the char position they start at.
    images: Vec<(usize, ImageRef)>,
    diagrams: Vec<(usize, DiagramRef)>,
    headings: Vec<(usize, HeadingRef)>,
    /// Decorated tables with the char position they start at.
    table_refs: Vec<(usize, TableRef)>,
    next_group: u32,
}

impl<'a> Builder<'a> {
    fn new(text: RopeSlice<'a>, text_width: usize) -> Self {
        Self {
            text,
            text_width,
            items: Vec::new(),
            leaves: Vec::new(),
            tables: Vec::new(),
            images: Vec::new(),
            diagrams: Vec::new(),
            headings: Vec::new(),
            table_refs: Vec::new(),
            next_group: 0,
        }
    }

    fn char(&self, byte: u32) -> usize {
        self.text.byte_to_char(byte as usize)
    }

    fn group(&mut self) -> u32 {
        self.next_group += 1;
        self.next_group
    }

    fn conceal(&mut self, range: Range<usize>, replacement: Option<&str>, group: Option<u32>) {
        if range.start >= range.end {
            return;
        }
        let mut conceal = Conceal::new(range);
        conceal.replacement = replacement.map(Into::into);
        conceal.group = group;
        self.items.push(Item::Conceal(conceal));
    }

    fn conceal_node(&mut self, node: &Node, group: Option<u32>) {
        self.conceal(
            self.char(node.start_byte())..self.char(node.end_byte()),
            None,
            group,
        );
    }

    fn overlay(&mut self, char_idx: usize, grapheme: &str) {
        self.items
            .push(Item::Overlay(Overlay::new(char_idx, grapheme)));
    }

    fn pad(&mut self, char_idx: usize, fill: &str, count: usize) {
        if count > 0 {
            self.items.push(Item::Padding(InlineAnnotation::new(
                char_idx,
                fill.repeat(count),
            )));
        }
    }

    /// Records `node` as a leaf block covering its whole lines.
    fn leaf(&mut self, node: &Node) {
        let first = self.text.char_to_line(self.char(node.start_byte()));
        let end_char = self
            .char(node.end_byte())
            .saturating_sub(1)
            .max(self.char(node.start_byte()));
        let last = self.text.char_to_line(end_char);
        self.leaves.push(first..last + 1);
    }

    fn walk_block(&mut self, node: Node<'a>, range: &Range<u32>, list_depth: usize) {
        if !overlaps(&node, range) {
            return;
        }
        match node.kind() {
            "paragraph" | "indented_code_block" | "html_block" => self.leaf(&node),
            "atx_heading" => {
                self.leaf(&node);
                self.atx_heading(&node);
            }
            "setext_heading" => {
                self.leaf(&node);
                self.setext_heading(&node);
            }
            "fenced_code_block" => {
                self.leaf(&node);
                self.fence(&node);
                self.diagram(&node);
            }
            "thematic_break" => {
                self.leaf(&node);
                self.thematic_break(&node);
            }
            "pipe_table" => {
                self.leaf(&node);
                for child in node.children() {
                    if child.kind() == "block_continuation" {
                        self.quote_bars(&child);
                    }
                }
                self.tables.push(node);
                return; // rows are decorated in `finish_tables`; cells hold no block content
            }
            "list_item" => self.list_item(&node, list_depth),
            "block_quote_marker" | "block_continuation" => self.quote_bars(&node),
            _ => {}
        }
        let depth = list_depth + usize::from(node.kind() == "list");
        for child in node.children() {
            self.walk_block(child, range, depth);
        }
    }

    /// Records a heading whose text runs from `start` to `end` (trailing
    /// whitespace dropped).
    fn heading(&mut self, level: u8, start: usize, end: usize) {
        let line = self.text.char_to_line(start.min(self.text.len_chars()));
        let mut end = end;
        while end > start && self.text.char(end - 1).is_whitespace() {
            end -= 1;
        }
        if end > start {
            self.headings.push((
                start,
                HeadingRef {
                    line,
                    level,
                    content_start: start,
                    content_end: end,
                },
            ));
        }
    }

    /// Start of the ATX closing sequence (spaces, a run of `#`, spaces up to
    /// the line end) of heading text running from `start` to `line_end`.
    /// The run must start the text or follow a space or tab (`# C#` has none).
    fn atx_closing(&self, start: usize, line_end: usize) -> Option<usize> {
        let blank = |c: char| matches!(c, ' ' | '\t');
        let mut end = line_end;
        while end > start && blank(self.text.char(end - 1)) {
            end -= 1;
        }
        let mut hashes = end;
        while hashes > start && self.text.char(hashes - 1) == '#' {
            hashes -= 1;
        }
        if hashes == end || (hashes > start && !blank(self.text.char(hashes - 1))) {
            return None;
        }
        let mut close = hashes;
        while close > start && blank(self.text.char(close - 1)) {
            close -= 1;
        }
        Some(close)
    }

    fn atx_heading(&mut self, node: &Node) {
        let content = node.children().find(|child| child.kind() == "inline");
        let has_content = content.is_some();
        let start = self.char(node.start_byte());
        let line_end = line_end_char_index(&self.text, self.text.char_to_line(start));
        let mut end = self
            .char(content.map_or(node.end_byte(), |c| c.start_byte()))
            .min(line_end);
        // hide the whitespace between the marker and the title
        while end < line_end && matches!(self.text.char(end), ' ' | '\t') {
            end += 1;
        }
        self.conceal(start..end, None, None);
        // the closing sequence is concealed like the opening marker; both are
        // shown again when the block is revealed
        let content_end = match self.atx_closing(end, line_end) {
            Some(close) => {
                self.conceal(close..line_end, None, None);
                close
            }
            None => line_end,
        };
        let level = node.children().find_map(|child| {
            child
                .kind()
                .strip_prefix("atx_h")?
                .strip_suffix("_marker")?
                .parse::<u8>()
                .ok()
        });
        if let (Some(level), true) = (level, has_content) {
            self.heading(level, end, content_end);
        }
    }

    const BULLETS: [&'static str; 3] = ["•", "◦", "▪"];

    /// `end` clamped so that it stays on the line of `start`.
    fn clamp_to_line(&self, start: usize, end: usize) -> usize {
        let line = self.text.char_to_line(start.min(self.text.len_chars()));
        end.min(line_end_char_index(&self.text, line))
    }

    fn list_item(&mut self, node: &Node, depth: usize) {
        let Some(marker) = node.child(0) else { return };
        let task = node.children().find(|c| {
            matches!(
                c.kind(),
                "task_list_marker_checked" | "task_list_marker_unchecked"
            )
        });
        let start = self.char(marker.start_byte());
        match task {
            Some(task) => {
                let checked = task.kind() == "task_list_marker_checked";
                let mut end = self.clamp_to_line(start, self.char(task.end_byte()));
                // conceal through the following space so that `I` lands on
                // the item text; the replacement shows the space again
                let glyph = if end < self.clamp_to_line(start, end + 1)
                    && matches!(self.text.char(end), ' ' | '\t')
                {
                    end += 1;
                    if checked {
                        "☑ "
                    } else {
                        "☐ "
                    }
                } else if checked {
                    "☑"
                } else {
                    "☐"
                };
                self.conceal(start..end, Some(glyph), None);
            }
            None if matches!(
                marker.kind(),
                "list_marker_minus" | "list_marker_plus" | "list_marker_star"
            ) =>
            {
                // the marker node may include indentation or trailing whitespace
                let text = self.text;
                let end = self.clamp_to_line(start, self.char(marker.end_byte()));
                let pos = (start..end)
                    .find(|&i| !text.char(i).is_whitespace())
                    .unwrap_or(start);
                self.overlay(pos, Self::BULLETS[depth.saturating_sub(1) % 3]);
            }
            None => {}
        }
    }

    fn quote_bars(&mut self, node: &Node) {
        let start = self.char(node.start_byte());
        let end = self.clamp_to_line(start, self.char(node.end_byte()));
        for i in start..end {
            if self.text.char(i) == '>' {
                self.overlay(i, "▎");
            }
        }
    }

    /// The last line of the document when it closes the fence opened by
    /// `opening`, which the parser leaves unterminated because the line has
    /// no line ending.
    fn closing_fence_without_line_ending(&self, node: &Node, opening: &Node) -> Option<usize> {
        let last = self.text.len_lines().checked_sub(1)?;
        let first = self.text.char_to_line(self.char(node.start_byte()));
        if last <= first || self.char(node.end_byte()) != self.text.len_chars() {
            return None;
        }
        let open: String = self
            .text
            .slice(self.char(opening.start_byte())..self.char(opening.end_byte()))
            .chars()
            .collect();
        let fence = open.chars().next()?;
        let line: String = self.text.line(last).chars().collect();
        let line = line.trim_start_matches(|c: char| c == '>' || c.is_whitespace());
        let line = line.trim_end();
        (line.chars().count() >= open.chars().count() && line.chars().all(|c| c == fence))
            .then_some(last)
    }

    /// Records a terminated mermaid fence as a [`DiagramRef`].
    fn diagram(&mut self, node: &Node) {
        let language = node
            .children()
            .find(|c| c.kind() == "info_string")
            .and_then(|info| info.children().find(|c| c.kind() == "language"));
        let is_mermaid = language.is_some_and(|language| {
            self.text
                .slice(self.char(language.start_byte())..self.char(language.end_byte()))
                .to_string()
                .eq_ignore_ascii_case("mermaid")
        });
        let delimiters: Vec<_> = node
            .children()
            .filter(|c| c.kind() == "fenced_code_block_delimiter")
            .collect();
        if !is_mermaid || delimiters.is_empty() {
            return;
        }
        // the parser takes a closing fence on the document's last line
        // without a line ending for content
        let unterminated_end = (delimiters.len() < 2)
            .then(|| self.closing_fence_without_line_ending(node, &delimiters[0]))
            .flatten();
        if delimiters.len() < 2 && unterminated_end.is_none() {
            return;
        }
        let content_end = unterminated_end.map(|line| self.text.line_to_char(line));
        let mut source = String::new();
        if let Some(content) = node.children().find(|c| c.kind() == "code_fence_content") {
            let end = content_end.unwrap_or_else(|| self.char(content.end_byte()));
            let mut pos = self.char(content.start_byte());
            for marker in content
                .children()
                .filter(|c| c.kind() == "block_continuation")
            {
                let marker_start = self.char(marker.start_byte());
                if marker_start >= end {
                    break;
                }
                source.extend(self.text.slice(pos..marker_start).chunks());
                pos = self.char(marker.end_byte());
            }
            if pos < end {
                source.extend(self.text.slice(pos..end).chunks());
            }
        }
        let start = self.char(node.start_byte());
        let first = self.text.char_to_line(start);
        let last = match unterminated_end {
            Some(line) => line,
            None => {
                let closing = &delimiters[delimiters.len() - 1];
                self.text.char_to_line(self.char(closing.start_byte()))
            }
        };
        let conceals = (first..last + 1)
            .filter_map(|line| {
                let range = self.text.line_to_char(line)..line_end_char_index(&self.text, line);
                (range.start < range.end).then(|| Conceal::new(range))
            })
            .collect();
        self.diagrams.push((
            start,
            DiagramRef {
                lines: first..last + 1,
                source,
                conceals,
            },
        ));
    }

    fn fence(&mut self, node: &Node) {
        for delimiter in node
            .children()
            .filter(|c| c.kind() == "fenced_code_block_delimiter")
        {
            let start = self.char(delimiter.start_byte());
            let end = self.clamp_to_line(start, self.char(delimiter.end_byte()));
            for i in start..end {
                self.overlay(i, "─");
            }
        }
    }

    fn setext_heading(&mut self, node: &Node) {
        for underline in node
            .children()
            .filter(|c| matches!(c.kind(), "setext_h1_underline" | "setext_h2_underline"))
        {
            let start = self.char(underline.start_byte());
            let end = self.clamp_to_line(start, self.char(underline.end_byte()));
            for i in start..end {
                if !self.text.char(i).is_whitespace() {
                    self.overlay(i, "━");
                }
            }
        }
        let level = node.children().find_map(|child| match child.kind() {
            "setext_h1_underline" => Some(1),
            "setext_h2_underline" => Some(2),
            _ => None,
        });
        if let (Some(level), Some(content)) = (
            level,
            node.children().find(|child| child.kind() == "paragraph"),
        ) {
            let start = self.char(content.start_byte());
            let last = self.char(content.end_byte()).saturating_sub(1).max(start);
            // only single-line text: the reserved row goes right below it
            let line = self.text.char_to_line(start);
            if line == self.text.char_to_line(last) {
                self.heading(level, start, line_end_char_index(&self.text, line));
            }
        }
    }

    fn thematic_break(&mut self, node: &Node) {
        let start = self.char(node.start_byte());
        let line = self.text.char_to_line(start);
        let line_end = line_end_char_index(&self.text, line);
        let mut end = line_end;
        while end > start && self.text.char(end - 1).is_whitespace() {
            end -= 1;
        }
        for i in start..end {
            self.overlay(i, "─");
        }
        let width = end - start;
        self.pad(line_end, "─", self.text_width.saturating_sub(width));
    }

    fn walk_inline(&mut self, node: Node<'a>, range: &Range<u32>) {
        if !overlaps(&node, range) {
            return;
        }
        match node.kind() {
            "emphasis" | "strong_emphasis" | "strikethrough" => {
                let group = self.group();
                for child in node.children() {
                    if child.kind() == "emphasis_delimiter" || !child.is_named() {
                        self.conceal_node(&child, Some(group));
                    }
                }
            }
            "code_span" => {
                let group = self.group();
                for child in node.children() {
                    if child.kind() == "code_span_delimiter" {
                        self.conceal_node(&child, Some(group));
                    }
                }
                return;
            }
            "inline_link" | "full_reference_link" | "collapsed_reference_link" => {
                if let Some(link_text) = node.children().find(|c| c.kind() == "link_text") {
                    let group = Some(self.group());
                    let (start, end) = (self.char(node.start_byte()), self.char(node.end_byte()));
                    let (text_start, text_end) = (
                        self.char(link_text.start_byte()),
                        self.char(link_text.end_byte()),
                    );
                    self.conceal(start..text_start, None, group);
                    self.conceal(text_end..end, None, group);
                    self.walk_inline(link_text, range);
                }
                return;
            }
            "uri_autolink" => {
                let (start, end) = (self.char(node.start_byte()), self.char(node.end_byte()));
                if end > start + 1 && self.text.char(start) == '<' {
                    let group = Some(self.group());
                    self.conceal(start..start + 1, None, group);
                    self.conceal(end - 1..end, None, group);
                }
                return;
            }
            "backslash_escape" => {
                let start = self.char(node.start_byte());
                self.conceal(start..start + 1, None, None);
                return;
            }
            "image" => {
                self.image(&node);
                return;
            }
            "hard_line_break" => {
                // trailing spaces, not a backslash: these look like trailing
                // whitespace in the raw text
                let start = self.char(node.start_byte());
                if self.text.char(start) == ' ' {
                    self.items.push(Item::HardBreak(start));
                }
                return;
            }
            _ => {}
        }
        for child in node.children() {
            self.walk_inline(child, range);
        }
    }

    fn image(&mut self, node: &Node) {
        let (start_byte, end_byte) = (node.start_byte(), node.end_byte());
        if self
            .tables
            .iter()
            .any(|table| table.start_byte() <= start_byte && end_byte <= table.end_byte())
        {
            return;
        }
        let (start, end) = (self.char(start_byte), self.char(end_byte));
        let line = self.text.char_to_line(start);
        if self.text.char_to_line(end.saturating_sub(1).max(start)) != line {
            return;
        }
        let children: Vec<_> = node.children().collect();
        if !children.iter().any(|c| c.kind() == "(") {
            return; // reference-style image
        }
        let Some(destination) = children.iter().find(|c| c.kind() == "link_destination") else {
            return;
        };
        let raw = self
            .text
            .slice(self.char(destination.start_byte())..self.char(destination.end_byte()))
            .to_string();
        let destination = raw
            .trim_start_matches('<')
            .trim_end_matches('>')
            .to_string();
        if destination.is_empty() || destination.contains("://") {
            return;
        }
        let (text_start, text_end) = match children.iter().find(|c| c.kind() == "image_description")
        {
            Some(d) => (self.char(d.start_byte()), self.char(d.end_byte())),
            None => (start + 2, start + 2),
        };
        let group = Some(self.group());
        let mut opening = Conceal::new(start..text_start);
        opening.group = group;
        let mut closing = Conceal::new(text_end..end);
        closing.group = group;
        self.images.push((
            start,
            ImageRef {
                line,
                destination,
                caption: [opening, closing],
            },
        ));
    }

    fn finish_tables(&mut self) {
        let tables = std::mem::take(&mut self.tables);
        for table in tables {
            self.table(&table);
        }
    }

    /// Display width of `range` as rendered: concealed chars are skipped,
    /// replacements are counted. `conceals` are sorted and non-overlapping.
    fn visible_width(&self, range: Range<usize>, conceals: &[Conceal]) -> usize {
        use helix_core::unicode::width::{UnicodeWidthChar, UnicodeWidthStr};
        let mut width = 0;
        let mut next = conceals.partition_point(|c| c.range.end <= range.start);
        let mut chars = self.text.slice(range.clone()).chars();
        let mut pos = range.start;
        while pos < range.end {
            match conceals.get(next) {
                Some(conceal) if conceal.range.start <= pos => {
                    width += conceal.replacement.as_deref().map_or(0, |r| r.width());
                    let end = conceal.range.end.min(range.end);
                    for _ in pos..end {
                        chars.next();
                    }
                    pos = end;
                    next += 1;
                }
                _ => {
                    width += chars.next().and_then(|c| c.width()).unwrap_or(0);
                    pos += 1;
                }
            }
        }
        width
    }

    fn table(&mut self, node: &Node) {
        #[derive(Clone, Copy, PartialEq)]
        enum Align {
            Left,
            Right,
            Center,
        }
        struct Row {
            start: usize,
            delimiter: bool,
            pipes: Vec<usize>,
            cells: Vec<Range<usize>>,
            leading: bool,
            trailing: bool,
        }

        let mut rows = Vec::new();
        for row in node.children().filter(|c| {
            matches!(
                c.kind(),
                "pipe_table_header" | "pipe_table_delimiter_row" | "pipe_table_row"
            )
        }) {
            let start = self.char(row.start_byte());
            let end = self.char(row.end_byte()).min(line_end_char_index(
                &self.text,
                self.text.char_to_line(start),
            ));
            let pipes: Vec<usize> = row
                .children()
                .filter(|c| c.kind() == "|")
                .map(|c| self.char(c.start_byte()))
                .collect();
            let text = self.text;
            let has_content = |r: &Range<usize>| r.clone().any(|i| !text.char(i).is_whitespace());
            let mut cells = Vec::new();
            let leading = pipes.first().is_some_and(|&p| has_content(&(start..p)));
            if leading {
                cells.push(start..pipes[0]);
            } else if pipes.is_empty() {
                cells.push(start..end);
            }
            for pair in pipes.windows(2) {
                cells.push(pair[0] + 1..pair[1]);
            }
            let trailing = pipes.last().is_some_and(|&p| has_content(&(p + 1..end)));
            if trailing {
                cells.push(pipes[pipes.len() - 1] + 1..end);
            }
            rows.push(Row {
                start,
                delimiter: row.kind() == "pipe_table_delimiter_row",
                pipes,
                cells,
                leading,
                trailing,
            });
        }

        let table_range = self.char(node.start_byte())..self.char(node.end_byte());
        let mut conceals: Vec<Conceal> = self
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Conceal(c)
                    if c.range.start < table_range.end && c.range.end > table_range.start =>
                {
                    Some(c.clone())
                }
                _ => None,
            })
            .collect();
        // as in `finish`: sorted, without duplicates or overlaps
        conceals.sort_by_key(|conceal| conceal.range.start);
        let mut end = 0;
        conceals.retain(|conceal| {
            let keep = conceal.range.start >= end;
            if keep {
                end = conceal.range.end;
            }
            keep
        });

        let columns = rows.iter().map(|r| r.cells.len()).max().unwrap_or(0);
        let mut widths = vec![0; columns];
        let mut aligns = vec![Align::Left; columns];
        let mut cell_widths = Vec::with_capacity(rows.len());
        for row in &rows {
            let mut row_widths = Vec::with_capacity(row.cells.len());
            for (col, cell) in row.cells.iter().enumerate() {
                let width = if row.delimiter {
                    cell.len()
                } else {
                    self.visible_width(cell.clone(), &conceals)
                };
                row_widths.push(width);
                widths[col] = widths[col].max(width);
                if row.delimiter {
                    let spec = self.text.slice(cell.clone()).to_string();
                    let spec = spec.trim();
                    aligns[col] = match (spec.starts_with(':'), spec.ends_with(':')) {
                        (true, true) if spec.len() > 1 => Align::Center,
                        (_, true) => Align::Right,
                        _ => Align::Left,
                    };
                }
            }
            cell_widths.push(row_widths);
        }

        for (row, row_widths) in rows.iter().zip(&cell_widths) {
            let last = row.pipes.len().saturating_sub(1);
            for (i, &pipe) in row.pipes.iter().enumerate() {
                let glyph = match (row.delimiter, i) {
                    (false, _) => "│",
                    (true, 0) if !row.leading => "├",
                    (true, i) if i == last && !row.trailing => "┤",
                    (true, _) => "┼",
                };
                self.overlay(pipe, glyph);
            }
            for (col, cell) in row.cells.iter().enumerate() {
                if row.delimiter {
                    for i in cell.clone() {
                        self.overlay(i, "─");
                    }
                    self.pad(cell.end, "─", widths[col] - cell.len());
                    continue;
                }
                let pad = widths[col] - row_widths[col];
                match aligns[col] {
                    Align::Left => self.pad(cell.end, " ", pad),
                    Align::Right => self.pad(cell.start, " ", pad),
                    Align::Center => {
                        self.pad(cell.start, " ", pad / 2);
                        self.pad(cell.end, " ", pad - pad / 2);
                    }
                }
            }
        }

        if let (Some(first), Some(last), Some(delimiter)) = (
            rows.first(),
            rows.last(),
            rows.iter().find(|row| row.delimiter),
        ) {
            // a row without pipes has no edges
            let (leading, trailing) = match delimiter.pipes.first() {
                Some(_) => (delimiter.leading, delimiter.trailing),
                None => (true, true),
            };
            let rule_start = match delimiter.pipes.first() {
                Some(&pipe) if !leading => pipe,
                _ => delimiter.start,
            };
            let line = self.text.char_to_line(delimiter.start);
            let prefix = self.border_prefix(self.text.line_to_char(line)..rule_start);
            let table = TableRef {
                lines: self.text.char_to_line(first.start)..self.text.char_to_line(last.start) + 1,
                prefix,
                widths: (0..delimiter.cells.len()).map(|col| widths[col]).collect(),
                leading,
                trailing,
            };
            self.table_refs.push((table_range.start, table));
        }
    }

    /// `range` of a line as shown before a table border: block quote bars
    /// kept, every other grapheme replaced by spaces of its width.
    fn border_prefix(&self, range: Range<usize>) -> String {
        use helix_core::unicode::width::UnicodeWidthChar;
        range
            .map(|i| {
                let bar = self.items.iter().any(|item| {
                    matches!(item, Item::Overlay(o) if o.char_idx == i && &*o.grapheme == "▎")
                });
                if bar {
                    "▎".to_string()
                } else {
                    " ".repeat(self.text.char(i).width().unwrap_or(1))
                }
            })
            .collect()
    }

    /// Splits `conceal` at line breaks; the line break itself stays visible.
    fn split_at_lines(&self, conceal: Conceal, out: &mut Vec<Conceal>) {
        let mut start = conceal.range.start;
        let mut first = true;
        while start < conceal.range.end {
            let line = self.text.char_to_line(start);
            let end = conceal.range.end.min(line_end_char_index(&self.text, line));
            if start < end {
                out.push(Conceal {
                    range: start..end,
                    replacement: if first {
                        conceal.replacement.clone()
                    } else {
                        None
                    },
                    group: conceal.group,
                });
                first = false;
            }
            if line + 1 >= self.text.len_lines() {
                break;
            }
            start = self.text.line_to_char(line + 1);
        }
    }

    fn finish(mut self) -> Decorations {
        self.leaves.sort_by_key(|lines| lines.start);
        let mut spans: Vec<Range<usize>> = Vec::new();
        for lines in self.leaves.drain(..) {
            match spans.last_mut() {
                Some(last) if lines.start < last.end => last.end = last.end.max(lines.end),
                _ => spans.push(lines),
            }
        }

        let items = std::mem::take(&mut self.items);
        let mut split = Vec::new();
        let mut blocks: BTreeMap<usize, Block> = BTreeMap::new();
        for item in items {
            let parts = match item {
                Item::Conceal(conceal) => {
                    split.clear();
                    self.split_at_lines(conceal, &mut split);
                    split.drain(..).map(Item::Conceal).collect()
                }
                item => vec![item],
            };
            for item in parts {
                let block = block_at(self.text, &spans, &mut blocks, item.pos());
                match item {
                    Item::Conceal(conceal) => block.conceals.push(conceal),
                    Item::Overlay(overlay) => block.overlays.push(overlay),
                    Item::Padding(padding) => block.padding.push(padding),
                    Item::HardBreak(pos) => block.hard_breaks.push(self.text.char_to_line(pos)),
                }
            }
        }

        for (pos, image) in std::mem::take(&mut self.images) {
            block_at(self.text, &spans, &mut blocks, pos)
                .images
                .push(image);
        }
        for (pos, diagram) in std::mem::take(&mut self.diagrams) {
            block_at(self.text, &spans, &mut blocks, pos)
                .diagrams
                .push(diagram);
        }

        for (pos, heading) in std::mem::take(&mut self.headings) {
            block_at(self.text, &spans, &mut blocks, pos)
                .headings
                .push(heading);
        }

        for (pos, table) in std::mem::take(&mut self.table_refs) {
            block_at(self.text, &spans, &mut blocks, pos)
                .tables
                .push(table);
        }

        for block in blocks.values_mut() {
            block.headings.sort_by_key(|h| h.line);
            block.tables.sort_by_key(|t| t.lines.start);
            block
                .images
                .sort_by_key(|image| (image.line, image.caption[0].range.start));
            block.diagrams.sort_by_key(|diagram| diagram.lines.start);
            block.conceals.sort_by_key(|conceal| conceal.range.start);
            block.conceals.dedup_by(|b, a| a.range == b.range);
            // nested constructs can produce overlapping conceals; keep the first
            let mut end = 0;
            block.conceals.retain(|conceal| {
                let keep = conceal.range.start >= end;
                if keep {
                    end = conceal.range.end;
                }
                keep
            });
            block.overlays.sort_by_key(|overlay| overlay.char_idx);
            block.padding.sort_by_key(|padding| padding.char_idx);
            block.hard_breaks.sort_unstable();
        }
        Decorations {
            blocks: blocks.into_values().collect(),
            len_chars: self.text.len_chars(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Key {
    version: i32,
    range: Range<usize>,
    text_width: usize,
    images_generation: u64,
    cell_px: (u16, u16),
    max_image_rows: u16,
    dark: bool,
    max_diagram_rows: u16,
    text_sizing: bool,
}

/// An image shown by a view below the line `line`, or a diagram drawn over
/// `covers` lines from `line` on and continued below them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImagePlacement {
    pub line: usize,
    pub id: u32,
    pub cols: u16,
    pub rows: u16,
    /// Number of document lines, from `line` on, the image is drawn over
    /// (0 for images below their caption line).
    pub covers: usize,
}

impl ImagePlacement {
    /// The line after which the rows not drawn over lines are reserved.
    pub fn last_line(&self) -> usize {
        self.line + self.covers.saturating_sub(1)
    }

    /// Rows reserved below [`Self::last_line`].
    pub fn extra_rows(&self) -> usize {
        (self.rows as usize).saturating_sub(self.covers)
    }
}

/// Images are at most half the view height.
fn max_image_rows(view_height: usize) -> u16 {
    (view_height / 2).clamp(1, u16::MAX as usize) as u16
}

/// Diagrams are at most the view height minus two rows.
fn max_diagram_rows(view_height: usize) -> u16 {
    view_height.saturating_sub(2).clamp(1, u16::MAX as usize) as u16
}

fn is_covered(covered: &[Range<usize>], char_idx: usize) -> bool {
    covered.iter().any(|range| range.contains(&char_idx))
}

/// Whether `line` ends with a line ending: the formatter reserves no rows
/// below the last line of a text without one.
fn has_line_end(text: RopeSlice, line: usize) -> bool {
    line + 1 < text.len_lines()
}

/// Reserves rows below lines that show images.
pub struct ImageRows<'a> {
    text: RopeSlice<'a>,
    placements: &'a [ImagePlacement],
}

impl<'a> ImageRows<'a> {
    pub fn new(text: RopeSlice<'a>, placements: &'a [ImagePlacement]) -> Self {
        Self { text, placements }
    }
}

impl LineAnnotation for ImageRows<'_> {
    fn insert_virtual_lines(
        &mut self,
        line_end_char_idx: usize,
        _line_end_visual_pos: helix_core::Position,
        doc_line: usize,
    ) -> helix_core::Position {
        // only at the real end of the document line, not at soft-wrap points
        let line_end = if doc_line + 1 < self.text.len_lines() {
            self.text.line_to_char(doc_line + 1)
        } else {
            self.text.len_chars()
        };
        if line_end_char_idx != line_end {
            return helix_core::Position::new(0, 0);
        }
        let rows: usize = self
            .placements
            .iter()
            .filter(|p| p.last_line() == doc_line)
            .map(ImagePlacement::extra_rows)
            .sum();
        helix_core::Position::new(rows, 0)
    }
}

/// Shown width, shown grapheme count and whether a shown grapheme is wider
/// than one cell, of `range` with `conceals` and `overlays` applied.
fn shown_width(
    text: RopeSlice,
    range: Range<usize>,
    conceals: &[Conceal],
    overlays: &[Overlay],
) -> (usize, usize, bool) {
    use helix_core::graphemes::grapheme_width;
    use helix_core::unicode::segmentation::UnicodeSegmentation;
    let (mut width, mut count, mut wide) = (0, 0, false);
    let mut add = |shown: &str| {
        for g in shown.graphemes(true) {
            let w = grapheme_width(g);
            width += w;
            count += 1;
            wide |= w > 1;
        }
    };
    let mut pos = range.start;
    let raw = text.slice(range).to_string();
    for g in raw.graphemes(true) {
        let at = pos;
        pos += g.chars().count();
        if let Some(conceal) = conceals.iter().find(|c| c.range.contains(&at)) {
            if conceal.range.start == at {
                if let Some(replacement) = &conceal.replacement {
                    add(replacement);
                }
            }
            continue;
        }
        match overlays.iter().find(|o| o.char_idx == at) {
            Some(overlay) => add(&overlay.grapheme),
            None => add(g),
        }
    }
    (width, count, wide)
}

/// The headings of `decorations` measured as drawn with their block's
/// layers; `revealed` are the indices of blocks shown raw.
fn heading_records_of(
    text: RopeSlice,
    decorations: &Decorations,
    revealed: &[usize],
) -> Vec<HeadingRecord> {
    let mut out = Vec::new();
    for (i, block) in decorations.blocks.iter().enumerate() {
        for heading in &block.headings {
            let line_start = text.line_to_char(heading.line);
            let (prefix_width, _, _) = shown_width(
                text,
                line_start..heading.content_start,
                &block.conceals,
                &block.overlays,
            );
            let (content_width, graphemes, has_wide) = shown_width(
                text,
                heading.content_start..heading.content_end,
                &block.conceals,
                &block.overlays,
            );
            out.push(HeadingRecord {
                line: heading.line,
                level: heading.level,
                content_start: heading.content_start,
                content_end: heading.content_end,
                prefix_width,
                content_width,
                graphemes,
                has_wide,
                revealed: revealed.contains(&i),
            });
        }
    }
    out
}

/// Markdown rendering state of one view of a document.
#[derive(Debug, Default)]
pub struct ViewState {
    key: Option<Key>,
    decorations: Decorations,
    /// Start chars of the revealed blocks.
    revealed: Vec<usize>,
    pub(crate) overlays: Vec<Overlay>,
    pub(crate) padding: Vec<InlineAnnotation>,
    pub(crate) images: Vec<ImagePlacement>,
    pub(crate) headings: Vec<HeadingRecord>,
    pub(crate) diagram_errors: Vec<InlineAnnotation>,
    hard_breaks: Vec<usize>,
    borders: Vec<BorderRow>,
}

/// Whether `doc` is a markdown document (its root syntax layer uses the
/// markdown grammar).
pub fn is_markdown(doc: &Document, loader: &Loader) -> bool {
    doc.syntax()
        .is_some_and(|syntax| is_markdown_syntax(syntax, loader))
}

/// Turns rendering on or off for `doc`, clearing its layers when off.
pub fn set_enabled(doc: &mut Document, enabled: bool) {
    doc.markdown_render = enabled;
    if !enabled {
        let views: Vec<ViewId> = doc.markdown_views.keys().copied().collect();
        for view_id in views {
            clear(doc, view_id);
        }
    }
}

fn clear(doc: &mut Document, view_id: ViewId) {
    if doc.markdown_views.remove(&view_id).is_some() {
        doc.remove_conceals(view_id);
    }
}

/// The images shown by `view_id`.
pub fn placements(doc: &Document, view_id: ViewId) -> &[ImagePlacement] {
    doc.markdown_views
        .get(&view_id)
        .map_or(&[], |state| &state.images)
}

/// The headings of `view_id` when text sizing is on, else empty.
pub fn heading_records(doc: &Document, view_id: ViewId) -> &[HeadingRecord] {
    doc.markdown_views
        .get(&view_id)
        .map_or(&[], |state| &state.headings)
}

/// The lines of `view_id` that end in a rendered hard line break made of
/// spaces, sorted. Their trailing spaces are not marked as trailing whitespace.
pub fn hard_breaks(doc: &Document, view_id: ViewId) -> &[usize] {
    doc.markdown_views
        .get(&view_id)
        .map_or(&[], |state| &state.hard_breaks)
}

/// Kitty image ids shown by any view. A document a view switched away from
/// keeps its state for that view, but its ids are not live.
pub fn live_image_ids(editor: &Editor) -> HashSet<u32> {
    editor
        .tree
        .views()
        .filter_map(|(view, _)| editor.document(view.doc)?.markdown_views.get(&view.id))
        .flat_map(|state| state.images.iter().map(|p| p.id))
        .collect()
}

/// The lines to decorate: the viewport and one viewport height around it, or
/// around the primary cursor when it lies outside the viewport, widened to
/// multiples of half the viewport height.
fn decoration_range(view: &View, doc: &Document) -> Range<usize> {
    let text = doc.text().slice(..);
    let height = view.inner_height(doc).max(1);
    let anchor = doc.view_offset(view.id).anchor.min(text.len_chars());
    let anchor_line = text.char_to_line(anchor);
    let cursor_line = doc.selection(view.id).primary().cursor_line(text);
    let (first, last) = if (anchor_line..anchor_line + height).contains(&cursor_line) {
        (anchor_line.saturating_sub(height), anchor_line + 2 * height)
    } else {
        (cursor_line.saturating_sub(height), cursor_line + height)
    };
    // snapped outwards so that scrolling by a few lines reuses the cache
    let step = (height / 2).max(1);
    let first = first / step * step;
    let last = last
        .div_ceil(step)
        .saturating_mul(step)
        .min(text.len_lines());
    text.line_to_char(first)..text.line_to_char(last)
}

/// Recomputes the markdown layers of `view_id` if the text, the decorated
/// range, the text width or the revealed blocks changed. In insert mode the
/// blocks containing a cursor of the focused view are revealed.
pub fn refresh(editor: &mut Editor, view_id: ViewId) {
    if !editor.tree.contains(view_id) {
        return;
    }
    let loader = editor.syn_loader.load();
    let insert = editor.mode == Mode::Insert && editor.tree.focus == view_id;
    let text_sizing = editor.text_sizing.enabled;
    let view = editor.tree.get(view_id);
    let dark = crate::images::diagram::is_dark(
        editor.theme.get("ui.background").bg,
        editor.images.theme_mode(),
    );
    let images = &mut editor.images;
    let Some(doc) = editor.documents.get_mut(&view.doc) else {
        return;
    };
    if !doc.markdown_render || !is_markdown(doc, &loader) {
        clear(doc, view_id);
        return;
    }

    let text = doc.text().slice(..);
    let key = Key {
        version: doc.version(),
        range: decoration_range(view, doc),
        // one less than the formatter's width: soft-wrap breaks a line whose
        // last word ends exactly at the edge
        text_width: (doc.text_format(view.inner_width(doc), None).viewport_width as usize)
            .saturating_sub(1),
        // read before `get`/`placement` below: a bump there recomputes once more
        images_generation: images.generation(),
        cell_px: images.caps().cell_px,
        max_image_rows: max_image_rows(view.inner_height(doc)),
        dark,
        max_diagram_rows: max_diagram_rows(view.inner_height(doc)),
        text_sizing,
    };
    let cursors: Vec<usize> = if insert {
        doc.selection(view_id)
            .iter()
            .map(|range| range.cursor(text))
            .collect()
    } else {
        Vec::new()
    };
    let cached = doc
        .markdown_views
        .get(&view_id)
        .filter(|state| state.key.as_ref() == Some(&key));
    if cached.is_some_and(|state| state.decorations.revealed_starts(&cursors) == state.revealed) {
        return;
    }

    let decorations = match cached {
        Some(state) => state.decorations.clone(),
        None => decorate(
            doc.syntax().expect("checked by is_markdown"),
            text,
            key.range.clone(),
            key.text_width,
            &loader,
        ),
    };
    let revealed_indices = decorations.revealed(&cursors);
    let headings = if key.text_sizing {
        heading_records_of(text, &decorations, &revealed_indices)
    } else {
        Vec::new()
    };
    let borders = border_rows_of(text, &decorations, &revealed_indices);
    let layers = decorations.layers(&revealed_indices);
    let mut conceals = layers.conceals;
    let mut placements = Vec::new();
    let mut covered: Vec<Range<usize>> = Vec::new();
    let mut diagram_errors = Vec::new();
    if images.caps().supported {
        let base_dir = doc
            .path()
            .and_then(|path| path.parent())
            .map(Path::to_path_buf);
        let max_cols = key.text_width as u16;
        let max_rows = key.max_image_rows;
        let mut diagram_conceals = Vec::new();
        for (i, block) in decorations.blocks.iter().enumerate() {
            if revealed_indices.contains(&i) {
                continue;
            }
            for image in &block.images {
                if !has_line_end(text, image.line) {
                    continue;
                }
                let destination = Path::new(&image.destination);
                let path = if destination.is_absolute() {
                    destination.to_path_buf()
                } else if let Some(base) = &base_dir {
                    base.join(destination)
                } else {
                    continue;
                };
                let source = crate::images::ImageSource::File(path);
                let ImageState::Ready { width, height } = images.get(&source) else {
                    continue;
                };
                let caps = images.caps();
                let (cols, rows) =
                    crate::images::layout::fit((width, height), caps.cell_px, max_cols, max_rows);
                if let Some(id) = images.placement(&source, cols, rows) {
                    conceals.extend(
                        image
                            .caption
                            .iter()
                            .filter(|c| c.range.start < c.range.end)
                            .cloned(),
                    );
                    placements.push(ImagePlacement {
                        line: image.line,
                        id,
                        cols,
                        rows,
                        covers: 0,
                    });
                }
            }
            for diagram in &block.diagrams {
                let source = images.diagram(&diagram.source, key.dark);
                match images.get(&source) {
                    ImageState::Ready { width, height } => {
                        let covers = diagram.lines.len();
                        let last = diagram.lines.end - 1;
                        // the formatter reserves no rows below a last line
                        // without a line ending: fit the diagram to its lines
                        let max_rows = if has_line_end(text, last) {
                            key.max_diagram_rows
                        } else {
                            covers.min(u16::MAX as usize) as u16
                        };
                        let caps = images.caps();
                        let (cols, rows) = crate::images::layout::fit(
                            (width, height),
                            caps.cell_px,
                            max_cols,
                            max_rows,
                        );
                        if let Some(id) = images.placement(&source, cols, rows) {
                            covered.push(
                                text.line_to_char(diagram.lines.start)
                                    ..line_end_char_index(&text, last),
                            );
                            diagram_conceals.extend(diagram.conceals.iter().cloned());
                            placements.push(ImagePlacement {
                                line: diagram.lines.start,
                                id,
                                cols,
                                rows,
                                covers,
                            });
                        }
                    }
                    ImageState::Error(message) => {
                        diagram_errors.push(InlineAnnotation::new(
                            line_end_char_index(&text, diagram.lines.start),
                            format!("  ⚠ {message}"),
                        ));
                    }
                    ImageState::Loading | ImageState::Unavailable => {}
                }
            }
        }
        // covered lines show only the diagram
        conceals.retain(|c| {
            !covered
                .iter()
                .any(|r| c.range.start < r.end && r.start < c.range.end)
        });
        conceals.extend(diagram_conceals);
        conceals.sort_by_key(|conceal| conceal.range.start);
    }
    let revealed = decorations.revealed_starts(&cursors);
    // measured with the old layers, restored once the new ones are in place
    let reveal_changed = doc
        .markdown_views
        .get(&view_id)
        .is_some_and(|state| state.revealed != revealed);
    let cursor = doc.selection(view_id).primary().cursor(text);
    let row = if reveal_changed {
        cursor_row(view, doc, cursor)
    } else {
        None
    };
    doc.set_conceals(view_id, conceals);
    doc.markdown_views.insert(
        view_id,
        ViewState {
            key: Some(key),
            decorations,
            revealed,
            headings,
            overlays: layers
                .overlays
                .into_iter()
                .filter(|o| !is_covered(&covered, o.char_idx))
                .collect(),
            padding: layers
                .padding
                .into_iter()
                .filter(|p| !is_covered(&covered, p.char_idx))
                .collect(),
            images: placements,
            diagram_errors,
            hard_breaks: layers.hard_breaks,
            borders,
        },
    );
    if let Some(row) = row {
        keep_cursor_row(view, doc, cursor, row);
    }
}

/// Visual row of `pos` relative to the top of the view.
fn cursor_row(view: &View, doc: &Document, pos: usize) -> Option<usize> {
    let text = doc.text().slice(..);
    let offset = doc.view_offset(view.id);
    let text_fmt = doc.text_format(view.inner_width(doc), None);
    let annotations = view.text_annotations(doc, None);
    let (visual, _) = helix_core::visual_offset_from_anchor(
        text,
        offset.anchor,
        pos,
        &text_fmt,
        &annotations,
        // unbounded; `usize::MAX` would overflow when added to the anchor row
        usize::MAX / 2,
    )
    .ok()?;
    visual.row.checked_sub(offset.vertical_offset)
}

/// Scrolls so that `pos` is shown at `row` again.
fn keep_cursor_row(view: &View, doc: &mut Document, pos: usize, row: usize) {
    let Some(now) = cursor_row(view, doc, pos) else {
        return;
    };
    let delta = now as isize - row as isize;
    if delta == 0 {
        return;
    }
    let text = doc.text().slice(..);
    let text_fmt = doc.text_format(view.inner_width(doc), None);
    let mut offset = doc.view_offset(view.id);
    let annotations = view.text_annotations(doc, None);
    (offset.anchor, offset.vertical_offset) = helix_core::char_idx_at_visual_offset(
        text,
        offset.anchor,
        offset.vertical_offset as isize + delta,
        0,
        &text_fmt,
        &annotations,
    );
    drop(annotations);
    doc.set_view_offset(view.id, offset);
}

/// How a heading level is enlarged (spec table: h1 2×, h2 1.5×, h3 1.25×).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadingScale {
    /// h1: `s=2`, one multicell per grapheme.
    Double,
    /// h2: `s=2:n=3:d=4:v=2`, 4 graphemes packed per `w=3` chunk.
    ThreeHalves,
    /// h3: `s=2:n=5:d=8:v=2`, 8 graphemes packed per `w=5` chunk.
    FiveQuarters,
}

impl HeadingScale {
    pub fn for_level(level: u8) -> Option<Self> {
        match level {
            1 => Some(Self::Double),
            2 => Some(Self::ThreeHalves),
            3 => Some(Self::FiveQuarters),
            _ => None,
        }
    }

    /// OSC 66 `(s, n, d, v)`.
    pub fn osc_params(self) -> (u8, u8, u8, u8) {
        match self {
            Self::Double => (2, 0, 0, 0),
            Self::ThreeHalves => (2, 3, 4, 2),
            Self::FiveQuarters => (2, 5, 8, 2),
        }
    }

    pub fn chunk_graphemes(self) -> usize {
        match self {
            Self::Double => 1,
            Self::ThreeHalves => 4,
            Self::FiveQuarters => 8,
        }
    }

    /// OSC 66 `w` of a chunk of `len` width-1 graphemes; 0 (the grapheme's
    /// own width) for [`Self::Double`].
    pub fn chunk_w(self, len: usize) -> u8 {
        match self {
            Self::Double => 0,
            Self::ThreeHalves => (3 * len).div_ceil(4) as u8,
            Self::FiveQuarters => (5 * len).div_ceil(8) as u8,
        }
    }

    /// Cells of a chunk of `len` graphemes `width` cells wide at normal size.
    pub fn chunk_cols(self, len: usize, width: usize) -> usize {
        match self {
            Self::Double => 2 * width,
            _ => 2 * self.chunk_w(len) as usize,
        }
    }

    pub fn scaled_width(self, content_width: usize, graphemes: usize) -> usize {
        match self {
            Self::Double => 2 * content_width,
            _ => {
                let n = self.chunk_graphemes();
                (0..graphemes)
                    .step_by(n)
                    .map(|start| self.chunk_cols((graphemes - start).min(n), 0))
                    .sum()
            }
        }
    }

    /// The formatter column, relative to the heading text, of the grapheme
    /// drawn at screen column `rel` relative to the scaled text.
    pub fn unscale_column(self, rel: usize, content_width: usize, graphemes: usize) -> usize {
        match self {
            Self::Double => (rel / 2).min(content_width.saturating_sub(1)),
            _ => {
                let n = self.chunk_graphemes();
                let (mut rel, mut base) = (rel, 0);
                while base < graphemes {
                    let len = (graphemes - base).min(n);
                    let cols = self.chunk_cols(len, 0);
                    if rel < cols {
                        return (base + rel * len / cols).min(graphemes - 1);
                    }
                    rel -= cols;
                    base += len;
                }
                graphemes.saturating_sub(1)
            }
        }
    }
}

/// A heading drawn enlarged in a view, with one row reserved below it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadingLayout {
    pub line: usize,
    pub scale: HeadingScale,
    pub content_start: usize,
    pub content_end: usize,
    pub prefix_width: usize,
    pub content_width: usize,
    pub graphemes: usize,
    pub scaled_width: usize,
    pub revealed: bool,
}

/// The state of a view that decides whether a heading fits.
struct FitContext {
    inner_width: usize,
    /// The soft-wrap width when soft-wrap is on.
    wrap_width: Option<usize>,
    horizontal_offset: usize,
    has_line_end: bool,
    eol_virtual_text: bool,
}

fn fit(record: &HeadingRecord, ctx: &FitContext) -> Option<HeadingLayout> {
    let scale = HeadingScale::for_level(record.level)?;
    let shown = record.prefix_width + record.content_width;
    if record.graphemes == 0
        || !ctx.has_line_end
        || ctx.horizontal_offset > 0
        || ctx.eol_virtual_text
        || (record.has_wide && scale != HeadingScale::Double)
        || ctx.wrap_width.is_some_and(|wrap| shown > wrap)
    {
        return None;
    }
    let scaled_width = scale.scaled_width(record.content_width, record.graphemes);
    (record.prefix_width + scaled_width <= ctx.inner_width).then_some(HeadingLayout {
        line: record.line,
        scale,
        content_start: record.content_start,
        content_end: record.content_end,
        prefix_width: record.prefix_width,
        content_width: record.content_width,
        graphemes: record.graphemes,
        scaled_width,
        revealed: record.revealed,
    })
}

/// Whether end-of-line virtual text that can show on lines other than the
/// cursor's is on `line`: inline blame on all lines, or an end-of-line
/// diagnostic. Cursor-line-only text (blame `cursor-line`, ghost text) is
/// ignored: the cursor's line is drawn at normal size anyway. `diagnostics`
/// are sorted by line, as [`Document::diagnostics`] is, and `line_severity`
/// gives a diagnostic's line and severity.
fn eol_text_forces_normal<D>(
    line: usize,
    blame: crate::editor::InlineBlameShow,
    eol_diagnostics: DiagnosticFilter,
    diagnostics: &[D],
    line_severity: impl Fn(&D) -> (usize, helix_core::diagnostic::Severity),
) -> bool {
    if blame == crate::editor::InlineBlameShow::AllLines {
        return true;
    }
    match eol_diagnostics {
        DiagnosticFilter::Disable => false,
        DiagnosticFilter::Enable(min) => {
            let first = diagnostics.partition_point(|d| line_severity(d).0 < line);
            diagnostics[first..]
                .iter()
                .map(line_severity)
                .take_while(|&(l, _)| l == line)
                .any(|(_, severity)| severity >= min)
        }
    }
}

/// The headings `view` draws enlarged, each with a reserved row.
pub fn heading_layouts(doc: &Document, view: &View) -> Vec<HeadingLayout> {
    let records = heading_records(doc, view.id);
    if records.is_empty() {
        return Vec::new();
    }
    let text = doc.text().slice(..);
    let inner_width = view.inner_width(doc);
    let fmt = doc.text_format(inner_width, None);
    let config = doc.config.load();
    let horizontal_offset = doc.view_offset(view.id).horizontal_offset;
    records
        .iter()
        .filter_map(|record| {
            let ctx = FitContext {
                inner_width: inner_width as usize,
                wrap_width: fmt.soft_wrap.then_some(fmt.viewport_width as usize),
                horizontal_offset,
                has_line_end: has_line_end(text, record.line),
                eol_virtual_text: eol_text_forces_normal(
                    record.line,
                    config.inline_blame.show,
                    config.end_of_line_diagnostics,
                    doc.diagnostics(),
                    |d| (d.line, d.severity()),
                ),
            };
            fit(record, &ctx)
        })
        .collect()
}

/// Maps a click at screen `(row, column)` on an enlarged heading in `view`,
/// on its text row or its reserved row, to where the clicked grapheme is at
/// normal size, which is what `View::pos_at_screen_coords` expects. Other
/// clicks are returned unchanged.
pub fn map_click(editor: &Editor, view: &View, row: u16, column: u16) -> (u16, u16) {
    // clicks in other views are theirs to map
    let area = view.area;
    if column < area.left() || column >= area.right() || row < area.top() || row >= area.bottom() {
        return (row, column);
    }
    let Some(doc) = editor.documents.get(&view.doc) else {
        return (row, column);
    };
    let layouts = heading_layouts(doc, view);
    if layouts.is_empty() {
        return (row, column);
    }
    let inner = view.inner_area(doc);
    let text = doc.text().slice(..);
    let line_at = |r: u16| {
        view.pos_at_screen_coords(doc, r, inner.x, true)
            .map(|pos| text.char_to_line(pos))
    };
    let target = match line_at(row) {
        Some(line) => Some((row, line)),
        // the reserved row is virtual: the heading is on the row above
        None => row
            .checked_sub(1)
            .filter(|&above| above >= inner.y)
            .and_then(|above| Some((above, line_at(above)?))),
    };
    let focused = editor.tree.focus == view.id;
    let Some((heading_row, layout)) = target.and_then(|(r, line)| {
        let layout = layouts
            .iter()
            .find(|l| l.line == line && draws_scaled(doc, view, l, focused))?;
        Some((r, layout))
    }) else {
        return (row, column);
    };
    let x0 = inner.x as usize + layout.prefix_width;
    let Some(rel) = (column as usize).checked_sub(x0) else {
        return (heading_row, column);
    };
    let col = if rel >= layout.scaled_width {
        x0 + layout.content_width
    } else {
        x0 + layout
            .scale
            .unscale_column(rel, layout.content_width, layout.graphemes)
    };
    (heading_row, col.min(u16::MAX as usize) as u16)
}

/// Whether `layout` is drawn enlarged now: not while its block is revealed
/// or the primary cursor is on its line in the focused view, or in any view
/// while virtual text drawn on every view's cursor line can show (inline
/// blame `cursor-line`, inline completion ghost text). Its row stays
/// reserved, so nothing moves.
pub fn draws_scaled(doc: &Document, view: &View, layout: &HeadingLayout, focused: bool) -> bool {
    let text = doc.text().slice(..);
    let cursor_line_text = || {
        doc.config.load().inline_blame.show == crate::editor::InlineBlameShow::CursorLine
            || doc.inline_completions.current().is_some()
    };
    let on_cursor_line = doc.selection(view.id).primary().cursor_line(text) == layout.line
        && (focused || cursor_line_text());
    !(layout.revealed || on_cursor_line)
}

/// Reserves the second row of every enlarged heading.
pub struct HeadingRows<'a> {
    text: RopeSlice<'a>,
    lines: Vec<usize>,
}

impl<'a> HeadingRows<'a> {
    pub fn new(text: RopeSlice<'a>, lines: Vec<usize>) -> Self {
        Self { text, lines }
    }
}

impl LineAnnotation for HeadingRows<'_> {
    fn insert_virtual_lines(
        &mut self,
        line_end_char_idx: usize,
        _line_end_visual_pos: helix_core::Position,
        doc_line: usize,
    ) -> helix_core::Position {
        // only at the real end of the document line, not at soft-wrap points
        let line_end = if doc_line + 1 < self.text.len_lines() {
            self.text.line_to_char(doc_line + 1)
        } else {
            self.text.len_chars()
        };
        let reserved = line_end_char_idx == line_end && self.lines.binary_search(&doc_line).is_ok();
        helix_core::Position::new(usize::from(reserved), 0)
    }
}

/// The row reservations of `view`'s enlarged headings, if any.
pub fn heading_rows<'a>(doc: &'a Document, view: &View) -> Option<HeadingRows<'a>> {
    let lines: Vec<usize> = heading_layouts(doc, view).iter().map(|l| l.line).collect();
    (!lines.is_empty()).then(|| HeadingRows::new(doc.text().slice(..), lines))
}

/// A table border drawn in a row reserved below a document line: the top
/// border below the line before the table, the bottom border below its last
/// line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BorderRow {
    /// The row is reserved below this line.
    pub line: usize,
    pub top: bool,
    /// Block quote bars and spaces before the border, see [`TableRef::prefix`].
    pub prefix: String,
    pub border: String,
    /// The table is shown raw (insert mode): the row stays reserved but is
    /// drawn blank, so nothing moves.
    pub revealed: bool,
}

/// The border rows of the tables of `decorations`, sorted by line with a
/// bottom border before a top border on the same line; `revealed` are the
/// indices of blocks shown raw. A table on the first line has no top border
/// and one on the last line without a line ending no bottom border: the
/// formatter reserves no rows there.
fn border_rows_of(
    text: RopeSlice,
    decorations: &Decorations,
    revealed: &[usize],
) -> Vec<BorderRow> {
    let mut out = Vec::new();
    for (i, block) in decorations.blocks.iter().enumerate() {
        for table in &block.tables {
            let row = |line, top| BorderRow {
                line,
                top,
                prefix: table.prefix.clone(),
                border: table.border(top),
                revealed: revealed.contains(&i),
            };
            if let Some(line) = table.lines.start.checked_sub(1) {
                out.push(row(line, true));
            }
            let last = table.lines.end - 1;
            if has_line_end(text, last) {
                out.push(row(last, false));
            }
        }
    }
    out.sort_by_key(|row| (row.line, row.top));
    out
}

/// The table border rows of `view_id`, see [`BorderRow`].
pub fn table_borders(doc: &Document, view_id: ViewId) -> &[BorderRow] {
    doc.markdown_views
        .get(&view_id)
        .map_or(&[], |state| &state.borders)
}

/// Reserves the rows of table borders. Added after every other line
/// annotation (inline diagnostics included), so that a top border is the
/// row right above its table.
pub struct TableRows<'a> {
    text: RopeSlice<'a>,
    rows: &'a [BorderRow],
}

impl<'a> TableRows<'a> {
    pub fn new(text: RopeSlice<'a>, rows: &'a [BorderRow]) -> Self {
        Self { text, rows }
    }
}

impl LineAnnotation for TableRows<'_> {
    fn insert_virtual_lines(
        &mut self,
        line_end_char_idx: usize,
        _line_end_visual_pos: helix_core::Position,
        doc_line: usize,
    ) -> helix_core::Position {
        // only at the real end of the document line, not at soft-wrap points
        let line_end = if doc_line + 1 < self.text.len_lines() {
            self.text.line_to_char(doc_line + 1)
        } else {
            self.text.len_chars()
        };
        if line_end_char_idx != line_end {
            return helix_core::Position::new(0, 0);
        }
        helix_core::Position::new(rows_after(self.rows, doc_line).len(), 0)
    }
}

/// The border rows reserved below `line`, in drawing order.
pub fn rows_after(rows: &[BorderRow], line: usize) -> &[BorderRow] {
    let start = rows.partition_point(|row| row.line < line);
    let end = rows.partition_point(|row| row.line <= line);
    &rows[start..end]
}

#[cfg(test)]
mod test {
    use super::*;
    use helix_core::doc_formatter::{DocumentFormatter, TextFormat};
    use helix_core::graphemes::Grapheme;
    use helix_core::syntax::Loader;
    use helix_core::text_annotations::TextAnnotations;
    use helix_core::Rope;

    fn parse(md: &str, language: &str) -> (Rope, Syntax, Loader) {
        let loader = helix_core::config::default_lang_loader();
        let text = Rope::from(md);
        let language = loader.language_for_name(language).unwrap();
        let syntax = Syntax::new(text.slice(..), language, &loader).unwrap();
        (text, syntax, loader)
    }

    pub(super) fn assert_well_formed(layers: &Layers, text: RopeSlice) {
        for pair in layers.conceals.windows(2) {
            assert!(pair[0].range.end <= pair[1].range.start, "overlap {pair:?}");
        }
        for conceal in &layers.conceals {
            assert!(conceal.range.start < conceal.range.end);
            let hidden = text.slice(conceal.range.clone()).to_string();
            assert!(
                !hidden.contains('\n') && !hidden.contains('\r'),
                "hides a line break: {conceal:?}"
            );
        }
        assert!(layers
            .overlays
            .windows(2)
            .all(|w| w[0].char_idx <= w[1].char_idx));
        assert!(layers
            .padding
            .windows(2)
            .all(|w| w[0].char_idx <= w[1].char_idx));
    }

    /// Renders `md` with every block decorated except those containing
    /// `cursors`, with table borders in their reserved rows (blank when
    /// revealed).
    pub(super) fn render_with(md: &str, text_width: usize, cursors: &[usize]) -> String {
        let (text, syntax, loader) = parse(md, "markdown");
        let decorations = decorate(
            &syntax,
            text.slice(..),
            0..text.len_chars(),
            text_width,
            &loader,
        );
        let revealed = decorations.revealed(cursors);
        let layers = decorations.layers(&revealed);
        assert_well_formed(&layers, text.slice(..));
        let borders = border_rows_of(text.slice(..), &decorations, &revealed);
        let mut annotations = TextAnnotations::default();
        annotations
            .set_conceals(&layers.conceals, None)
            .add_overlay(&layers.overlays, None)
            .add_inline_annotations(&layers.padding, None)
            .add_line_annotation(Box::new(TableRows::new(text.slice(..), &borders)));
        let text_fmt = TextFormat {
            soft_wrap: false,
            ..TextFormat::default()
        };
        let mut rows: Vec<Option<String>> = Vec::new();
        // the last visual row of every document line
        let mut last_rows: BTreeMap<usize, usize> = BTreeMap::new();
        for grapheme in
            DocumentFormatter::new_at_prev_checkpoint(text.slice(..), &text_fmt, &annotations, 0)
        {
            let row = grapheme.visual_pos.row;
            if rows.len() <= row {
                rows.resize(row + 1, None);
            }
            let out = rows[row].get_or_insert_with(String::new);
            if grapheme.raw != Grapheme::Newline {
                out.push_str(&grapheme.raw.to_string());
            }
            last_rows.insert(grapheme.line_idx, row);
        }
        drop(annotations);
        let mut k = 0;
        for (i, border) in borders.iter().enumerate() {
            k = if i > 0 && borders[i - 1].line == border.line {
                k + 1
            } else {
                1
            };
            let row = last_rows[&border.line] + k;
            if rows.len() <= row {
                rows.resize(row + 1, None);
            }
            assert!(rows[row].is_none(), "row {row} is not reserved: {border:?}");
            rows[row] = Some(if border.revealed {
                String::new()
            } else {
                format!("{}{}", border.prefix, border.border)
            });
        }
        let rows: Vec<String> = rows
            .into_iter()
            .enumerate()
            .map(|(row, r)| r.unwrap_or_else(|| panic!("row {row} reserved but not drawn")))
            .collect();
        rows.join("\n").trim_end().to_string()
    }

    pub(super) fn render(md: &str) -> String {
        render_with(md, 20, &[])
    }

    fn heading_refs(md: &str) -> Vec<(usize, u8, String)> {
        let (text, syntax, loader) = parse(md, "markdown");
        let decorations = decorate(&syntax, text.slice(..), 0..text.len_chars(), 80, &loader);
        decorations
            .blocks
            .iter()
            .flat_map(|b| b.headings.iter())
            .map(|h| {
                (
                    h.line,
                    h.level,
                    text.slice(h.content_start..h.content_end).to_string(),
                )
            })
            .collect()
    }

    #[test]
    fn records_atx_and_setext_headings() {
        assert_eq!(
            heading_refs("# A\n\n## B\n\n### C\n\n#### D\n"),
            [
                (0, 1, "A".into()),
                (2, 2, "B".into()),
                (4, 3, "C".into()),
                (6, 4, "D".into())
            ]
        );
        assert_eq!(
            heading_refs("T\n===\n\nU\n---\n"),
            [(0, 1, "T".into()), (3, 2, "U".into())]
        );
    }

    #[test]
    fn heading_edge_cases() {
        assert!(heading_refs("A\nB\n===\n").is_empty(), "multi-line setext");
        assert!(heading_refs("#\n").is_empty(), "empty heading");
        assert_eq!(heading_refs("> # Q\n"), [(0, 1, "Q".into())]);
        // a document of just "# End" parses as an ERROR node in the grammar
        assert_eq!(
            heading_refs("a\n\n# End"),
            [(2, 1, "End".into())],
            "last line, no newline"
        );
        assert_eq!(
            heading_refs("# T  \n"),
            [(0, 1, "T".into())],
            "trailing spaces"
        );
    }

    #[test]
    fn atx_closing_sequence_is_concealed() {
        assert_eq!(render("# Title ##\n"), "Title");
        assert_eq!(render("# Title #####   \n"), "Title");
        assert_eq!(render("## Two #\n"), "Two");
        // not a closing sequence: no space before the run
        assert_eq!(render("# C# and F#\n"), "C# and F#");
        // only a closing sequence: an empty heading
        assert_eq!(render("### ###\n"), "");
        // shown again when the block is revealed (insert mode)
        assert_eq!(render_with("# Title ##\n", 20, &[0]), "# Title ##");
        assert_eq!(heading_refs("# Title ##\n"), [(0, 1, "Title".into())]);
        assert!(heading_refs("### ###\n").is_empty());
    }

    fn records(md: &str) -> Vec<HeadingRecord> {
        let (text, syntax, loader) = parse(md, "markdown");
        let decorations = decorate(&syntax, text.slice(..), 0..text.len_chars(), 80, &loader);
        heading_records_of(text.slice(..), &decorations, &[])
    }

    #[test]
    fn measures_shown_width() {
        let r = &records("# A **b**\n")[0];
        assert_eq!(
            (r.prefix_width, r.content_width, r.graphemes, r.has_wide),
            (0, 3, 3, false)
        );
        let r = &records("# 漢字\n")[0];
        assert_eq!((r.content_width, r.graphemes, r.has_wide), (4, 2, true));
        // the quote prefix is drawn before the text
        let r = &records("> # Q\n")[0];
        assert_eq!(
            r.prefix_width + r.content_width,
            render("> # Q\n").chars().count()
        );
        assert!(r.prefix_width > 0);
    }

    #[test]
    fn headings() {
        assert_eq!(render("# Title\n\n## Sub\n"), "Title\n\nSub");
    }

    #[test]
    fn emphasis_and_code() {
        assert_eq!(render("a **b** *c* _d_ ~~e~~ `f`\n"), "a b c d e f");
    }

    #[test]
    fn links() {
        assert_eq!(
            render("[x](http://u) [y][r] <http://v>\n\n[r]: http://w\n"),
            "x y http://v\n\n[r]: http://w"
        );
    }

    #[test]
    fn escapes() {
        assert_eq!(render("\\*a\\*\n"), "*a*");
    }

    #[test]
    fn nested_constructs_render() {
        assert_eq!(render("**[x](u\\_v)** ***y*** [`c` *d*](e)\n"), "x y c d");
        assert_eq!(render("# A **b**\n"), "A b");
    }

    #[test]
    fn emphasis_across_lines() {
        assert_eq!(render("**a\nb** [c\nd](e)\n"), "a\nb c\nd");
    }

    #[test]
    fn reveal_only_cursor_block() {
        // cursor at char 10 is inside the second paragraph
        assert_eq!(render_with("**a**\n\n**b**\n", 20, &[10]), "a\n\n**b**");
    }

    #[test]
    fn cursor_at_eof_reveals_last_block() {
        let md = "**a**\n\n**b**";
        assert_eq!(render_with(md, 20, &[md.len()]), "a\n\n**b**");
    }

    #[test]
    fn revealed_starts_survive_range_changes() {
        let (text, syntax, loader) = parse("**a**\n\n**b**\n\n**c**\n", "markdown");
        let decorate = |range| decorate(&syntax, text.slice(..), range, 20, &loader);
        let full = decorate(0..text.len_chars());
        let tail = decorate(text.line_to_char(2)..text.len_chars());
        let cursors = [8]; // in `**b**`
                           // block indices shift with the decorated range, block starts do not
        assert_ne!(full.revealed(&cursors), tail.revealed(&cursors));
        assert_eq!(full.revealed_starts(&cursors), [7]);
        assert_eq!(tail.revealed_starts(&cursors), [7]);
    }

    #[test]
    fn range_limits_blocks() {
        let (text, syntax, loader) = parse("**a**\n\n**b**\n\n**c**\n", "markdown");
        let line = text.line_to_char(2)..text.line_to_char(3);
        let decorations = decorate(&syntax, text.slice(..), line, 20, &loader);
        assert_eq!(decorations.blocks.len(), 1);
        assert_eq!(decorations.blocks[0].range, 7..13);
    }

    #[test]
    fn multi_range_layers_are_walked_once() {
        for md in ["> **a**\n> **b**\n", "- **a**\n  **b**\n"] {
            let (text, syntax, loader) = parse(md, "markdown");
            let decorations = decorate(&syntax, text.slice(..), 0..text.len_chars(), 20, &loader);
            let layers = decorations.layers(&[]);
            assert_well_formed(&layers, text.slice(..));
            // two strong emphases, each with 2 + 2 single-char `*` delimiters
            assert_eq!(layers.conceals.len(), 8, "{md:?}: {:?}", layers.conceals);
            let groups: std::collections::BTreeSet<_> =
                layers.conceals.iter().map(|c| c.group).collect();
            assert_eq!(groups.len(), 2, "layer walked twice: {md:?}");
        }
    }

    #[test]
    fn inline_layers_have_no_duplicates() {
        for md in ["> **a**\n> **b**\n", "- **a**\n  **b**\n"] {
            let (text, syntax, loader) = parse(md, "markdown");
            let range = 0..text.len_bytes() as u32;
            let layers = inline_layers(&syntax, &loader, &range);
            let mut unique = layers.clone();
            unique.sort_by_key(|layer| format!("{layer:?}"));
            unique.dedup();
            assert_eq!(
                unique.len(),
                layers.len(),
                "{md:?}: duplicates in {layers:?}"
            );
            assert_eq!(layers.len(), 1, "{md:?}: {layers:?}");
        }
    }

    #[test]
    fn not_markdown() {
        let (text, syntax, loader) = parse("fn main() {}\n", "rust");
        assert!(!is_markdown_syntax(&syntax, &loader));
        assert!(
            decorate(&syntax, text.slice(..), 0..text.len_chars(), 20, &loader)
                .blocks
                .is_empty()
        );
    }

    #[test]
    fn bullets_by_depth() {
        assert_eq!(
            render("- a\n  - b\n    - c\n      - d\n"),
            "• a\n  ◦ b\n    ▪ c\n      • d"
        );
        assert_eq!(render("* a\n+ b\n"), "• a\n• b");
        assert_eq!(render("1. a\n2. b\n"), "1. a\n2. b");
    }

    #[test]
    fn task_items() {
        assert_eq!(render("- [ ] t\n- [x] d\n"), "☐ t\n☑ d");
    }

    #[test]
    fn list_items_reveal_separately() {
        // cursor at char 5 is on the second item
        assert_eq!(render_with("- a\n- b\n", 20, &[5]), "• a\n- b");
    }

    #[test]
    fn block_quotes() {
        assert_eq!(render("> q\n>\n> > n\n"), "▎ q\n▎\n▎ ▎ n");
    }

    #[test]
    fn code_fences() {
        assert_eq!(render("```rust\nlet x;\n```\n"), "───rust\nlet x;\n───");
        assert_eq!(render("~~~\nx\n~~~\n"), "───\nx\n───");
    }

    #[test]
    fn fenced_markdown_stays_raw() {
        assert_eq!(
            render("```md\n# Raw **heading**\n- item\n```\n"),
            "───md\n# Raw **heading**\n- item\n───"
        );
    }

    #[test]
    fn fenced_rustdoc_stays_raw() {
        assert_eq!(
            render("```rust\n/// # Example **b**\nfn f() {}\n```\n"),
            "───rust\n/// # Example **b**\nfn f() {}\n───"
        );
    }

    #[test]
    fn setext_headings() {
        assert_eq!(render("T\n===\n\nU\n---\n"), "T\n━━━\n\nU\n━━━");
    }

    #[test]
    fn thematic_break_spans_text_width() {
        assert_eq!(render_with("a\n\n***\n", 6, &[]), "a\n\n──────");
        assert_eq!(render_with("a\n\n- - -\n", 6, &[]), "a\n\n──────");
    }

    #[test]
    fn degenerate_documents() {
        assert_eq!(render(""), "");
        assert_eq!(render("#\n"), "");
        assert_eq!(render("```\ncode\n"), "───\ncode");
        assert_eq!(render("-\n"), "•");
    }

    #[test]
    fn crlf_document() {
        assert_eq!(render_with("# T\r\n\r\n***\r\n", 4, &[]), "T\n\n────");
    }

    #[test]
    fn empty_heading_keeps_next_line_indentation() {
        assert_eq!(render("#\n    code\n"), "\n    code");
        assert_eq!(render("# \n\n    code\n"), "\n\n    code");
    }

    #[test]
    fn table_left_and_right() {
        assert_eq!(
            render("| a | bb |\n|---|---:|\n| ccc | d |\n"),
            "│ a   │ bb │\n├─────┼────┤\n│ ccc │  d │\n└─────┴────┘"
        );
    }

    #[test]
    fn table_centre() {
        assert_eq!(
            render("|a|b|\n|:-:|-|\n|xxxxx|y|\n"),
            "│  a  │b│\n├─────┼─┤\n│xxxxx│y│\n└─────┴─┘"
        );
    }

    #[test]
    fn table_wide_chars() {
        assert_eq!(
            render("|日本|a|\n|-|-|\n|x|b|\n"),
            "│日本│a│\n├────┼─┤\n│x   │b│\n└────┴─┘"
        );
    }

    #[test]
    fn table_counts_visible_width() {
        assert_eq!(
            render("| **a** | b |\n|-|-|\n| ccc | d |\n"),
            "│ a   │ b │\n├─────┼───┤\n│ ccc │ d │\n└─────┴───┘"
        );
    }

    #[test]
    fn table_uneven_rows() {
        assert_eq!(render("|a|b|\n|-|-|\n|c|\n"), "│a│b│\n├─┼─┤\n│c│\n└─┴─┘");
    }

    #[test]
    fn table_is_one_block() {
        // cursor in the last row reveals the whole table
        let md = "|a|b|\n|-|-|\n|c|d|\n";
        assert_eq!(render_with(md, 20, &[13]), md.trim_end());
    }

    #[test]
    fn table_crlf_and_no_outer_pipes() {
        assert_eq!(
            render_with("|a|bb|\r\n|-|-|\r\n|ccc|d|\r\n", 20, &[]),
            "│a  │bb│\n├───┼──┤\n│ccc│d │\n└───┴──┘"
        );
        assert_eq!(render("a|b\n-|-\nccc|d\n"), "a  │b\n───┼─\nccc│d\n───┴─");
    }

    #[test]
    fn table_partial_range_matches_full() {
        let md = "| **a** | b |\n|-|-|\n| ccc | d |\n";
        let (text, syntax, loader) = parse(md, "markdown");
        let layers = |range: Range<usize>| {
            let d = decorate(&syntax, text.slice(..), range, 20, &loader);
            let l = d.layers(&[]);
            (
                l.conceals
                    .iter()
                    .map(|c| c.range.clone())
                    .collect::<Vec<_>>(),
                l.overlays
                    .iter()
                    .map(|o| (o.char_idx, o.grapheme.to_string()))
                    .collect::<Vec<_>>(),
                l.padding
                    .iter()
                    .map(|p| (p.char_idx, p.text.to_string()))
                    .collect::<Vec<_>>(),
            )
        };
        let last = text.line_to_char(2)..text.len_chars();
        assert_eq!(layers(last), layers(0..text.len_chars()));
    }

    #[test]
    fn table_in_block_quote_keeps_bars() {
        assert_eq!(
            render("> |a|\n> |-|\n> |b|\n"),
            "▎ │a│\n▎ ├─┤\n▎ │b│\n▎ └─┘"
        );
    }

    #[test]
    fn table_borders_with_outer_pipes() {
        assert_eq!(
            render("x\n\n| a | bb |\n|---|---:|\n| ccc | d |\n\ny\n"),
            "x\n\n┌─────┬────┐\n│ a   │ bb │\n├─────┼────┤\n│ ccc │  d │\n└─────┴────┘\n\ny"
        );
    }

    #[test]
    fn table_borders_without_outer_pipes() {
        assert_eq!(
            render("x\n\na|b\n-|-\nccc|d\n"),
            "x\n\n───┬─\na  │b\n───┼─\nccc│d\n───┴─"
        );
        // only one side open
        assert_eq!(
            render("x\n\n|a|b\n|-|-\n|c|d\n"),
            "x\n\n┌─┬─\n│a│b\n├─┼─\n│c│d\n└─┴─"
        );
    }

    #[test]
    fn table_borders_follow_the_delimiter_rule() {
        // uneven rows
        assert_eq!(
            render("x\n\n|a|b|\n|-|-|\n|c|\n"),
            "x\n\n┌─┬─┐\n│a│b│\n├─┼─┤\n│c│\n└─┴─┘"
        );
        // wide chars
        assert_eq!(
            render("x\n\n|日本|a|\n|-|-|\n|x|b|\n"),
            "x\n\n┌────┬─┐\n│日本│a│\n├────┼─┤\n│x   │b│\n└────┴─┘"
        );
        // alignment
        assert_eq!(
            render("x\n\n|a|b|c|\n|:-:|-:|:-|\n|xxxxx|y|z|\n"),
            "x\n\n┌─────┬──┬──┐\n│  a  │ b│c │\n├─────┼──┼──┤\n│xxxxx│ y│z │\n└─────┴──┴──┘"
        );
        // rendered cell width, not raw
        assert_eq!(
            render("x\n\n| **a** | b |\n|-|-|\n| ccc | d |\n"),
            "x\n\n┌─────┬───┐\n│ a   │ b │\n├─────┼───┤\n│ ccc │ d │\n└─────┴───┘"
        );
    }

    #[test]
    fn table_borders_in_block_quote() {
        assert_eq!(
            render("> x\n>\n> |a|\n> |-|\n> |b|\n"),
            "▎ x\n▎\n▎ ┌─┐\n▎ │a│\n▎ ├─┤\n▎ │b│\n▎ └─┘"
        );
        // the quote starts with the table: the top border goes below the
        // blank line before it, with the table's bars
        assert_eq!(
            render("x\n\n> |a|\n> |-|\n> |b|\n"),
            "x\n\n▎ ┌─┐\n▎ │a│\n▎ ├─┤\n▎ │b│\n▎ └─┘"
        );
    }

    #[test]
    fn table_at_document_start_has_no_top_border() {
        assert_eq!(render("|a|\n|-|\n|b|\n"), "│a│\n├─┤\n│b│\n└─┘");
        // nor a bottom border without a line ending after the last row
        assert_eq!(render("x\n\n|a|\n|-|\n|b|"), "x\n\n┌─┐\n│a│\n├─┤\n│b│");
    }

    #[test]
    fn revealed_table_keeps_blank_border_rows() {
        let md = "x\n\n|a|\n|-|\n|b|\n\ny\n";
        assert_eq!(render_with(md, 20, &[4]), "x\n\n\n|a|\n|-|\n|b|\n\n\ny");
    }

    #[test]
    fn table_records() {
        let md = "x\n\n> | a | b\n> |---|:-:\n> | ccc | d\n\ny\n";
        let (text, syntax, loader) = parse(md, "markdown");
        let decorations = decorate(&syntax, text.slice(..), 0..text.len_chars(), 20, &loader);
        let tables: Vec<&TableRef> = decorations
            .blocks
            .iter()
            .flat_map(|b| b.tables.iter())
            .collect();
        assert_eq!(
            tables,
            [&TableRef {
                lines: 2..5,
                prefix: "▎ ".into(),
                widths: vec![5, 3],
                leading: false,
                trailing: true,
            }]
        );
        // tables are decorated in full: a range in the last row records it too
        let last_row = text.line_to_char(4)..text.line_to_char(5);
        let partial = decorate(&syntax, text.slice(..), last_row, 20, &loader);
        let partial: Vec<&TableRef> = partial
            .blocks
            .iter()
            .flat_map(|b| b.tables.iter())
            .collect();
        assert_eq!(partial, tables);
        assert_eq!(
            border_rows_of(text.slice(..), &decorations, &[]),
            [
                BorderRow {
                    line: 1,
                    top: true,
                    prefix: "▎ ".into(),
                    border: "┌─────┬───".into(),
                    revealed: false,
                },
                BorderRow {
                    line: 4,
                    top: false,
                    prefix: "▎ ".into(),
                    border: "└─────┴───".into(),
                    revealed: false,
                },
            ]
        );
    }

    fn diagrams(md: &str) -> Vec<DiagramRef> {
        let (text, syntax, loader) = parse(md, "markdown");
        let decorations = decorate(&syntax, text.slice(..), 0..text.len_chars(), 20, &loader);
        decorations
            .blocks
            .iter()
            .flat_map(|block| block.diagrams.iter().cloned())
            .collect()
    }

    #[test]
    fn finds_mermaid_fences() {
        let md = "text\n\n```mermaid\nflowchart TD\n  A --> B\n```\n";
        let found = diagrams(md);
        assert_eq!(found.len(), 1);
        let diagram = &found[0];
        assert_eq!(diagram.lines, 2..6);
        assert_eq!(diagram.source, "flowchart TD\n  A --> B\n");
        let text = Rope::from(md);
        let hidden: Vec<String> = diagram
            .conceals
            .iter()
            .map(|c| text.slice(c.range.clone()).to_string())
            .collect();
        assert_eq!(hidden, ["```mermaid", "flowchart TD", "  A --> B", "```"]);
    }

    #[test]
    fn mermaid_language_is_case_insensitive() {
        assert_eq!(diagrams("```Mermaid\ngraph TD\n```\n").len(), 1);
        assert_eq!(diagrams("~~~mermaid\ngraph TD\n~~~\n").len(), 1);
    }

    #[test]
    fn other_fences_are_not_diagrams() {
        assert!(diagrams("```rust\nfn f() {}\n```\n").is_empty());
        assert!(diagrams("```mermaidx\ngraph TD\n```\n").is_empty());
        assert!(diagrams("```\ngraph TD\n```\n").is_empty());
        // unterminated
        assert!(diagrams("```mermaid\ngraph TD\n").is_empty());
    }

    #[test]
    fn empty_mermaid_fence_is_a_diagram() {
        let found = diagrams("```mermaid\n```\n");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].lines, 0..2);
        assert_eq!(found[0].source, "");
    }

    #[test]
    fn closing_fence_without_line_ending_is_terminated() {
        let found = diagrams("```mermaid\ngraph TD\n  A --> B\n```");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].lines, 0..4);
        assert_eq!(found[0].source, "graph TD\n  A --> B\n");
        assert_eq!(found[0].conceals.len(), 4);

        let found = diagrams("> ```mermaid\n> graph TD\n> ```");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].lines, 0..3);
        assert_eq!(found[0].source, "graph TD\n");

        assert!(diagrams("````mermaid\ngraph TD\n```").is_empty());
        assert!(diagrams("```mermaid\ngraph TD\nx").is_empty());
    }

    #[test]
    fn quoted_fence_source_has_no_markers() {
        let found = diagrams("> ```mermaid\n> graph TD\n>   A --> B\n> ```\n");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].source, "graph TD\n  A --> B\n");
        assert_eq!(found[0].conceals.len(), 4);
    }

    #[test]
    fn list_item_fence_source_has_no_indentation() {
        let found = diagrams("- item\n\n  ```mermaid\n  graph TD\n    A --> B\n  ```\n");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].source, "graph TD\n  A --> B\n");
    }

    #[test]
    fn diagrams_do_not_change_patch2_output() {
        assert_eq!(
            render("```mermaid\ngraph TD\n```\n"),
            "───mermaid\ngraph TD\n───"
        );
    }

    fn images(md: &str) -> Vec<(usize, String, String)> {
        let (text, syntax, loader) = parse(md, "markdown");
        let decorations = decorate(&syntax, text.slice(..), 0..text.len_chars(), 20, &loader);
        decorations
            .blocks
            .iter()
            .flat_map(|block| block.images.iter())
            .map(|image| {
                let caption = render_caption(&text, image);
                (image.line, image.destination.clone(), caption)
            })
            .collect()
    }

    /// The image's line with only its caption conceals applied.
    fn render_caption(text: &Rope, image: &ImageRef) -> String {
        let line = text.line(image.line).to_string();
        let start = text.line_to_char(image.line);
        line.chars()
            .enumerate()
            .filter(|(i, _)| !image.caption.iter().any(|c| c.range.contains(&(start + i))))
            .map(|(_, c)| c)
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    #[test]
    fn finds_local_images() {
        assert_eq!(
            images("text\n\n![a cat](img/cat.png \"Cat\")\n"),
            [(2, "img/cat.png".into(), "a cat".into())]
        );
        assert_eq!(
            images("![x](<a b.png>)\n"),
            [(0, "a b.png".into(), "x".into())]
        );
        assert_eq!(
            images("see ![x](/abs/p.jpg) here\n"),
            [(0, "/abs/p.jpg".into(), "see x here".into())]
        );
    }

    #[test]
    fn skips_remote_reference_and_table_images() {
        assert!(images("![x](https://example.com/a.png)\n").is_empty());
        assert!(images("![x][ref]\n\n[ref]: a.png\n").is_empty());
        assert!(images("| ![x](a.png) | b |\n|-|-|\n| c | d |\n").is_empty());
        assert!(images("![x](\na.png)\n").is_empty());
    }

    #[test]
    fn images_do_not_change_patch2_output() {
        // without graphics the line renders as raw text (caption conceals are not in Block::conceals)
        assert_eq!(render("![a](b.png)\n"), "![a](b.png)");
    }

    use helix_core::text_annotations::LineAnnotation;

    fn diagram_placement(line: usize, covers: usize, rows: u16) -> [ImagePlacement; 1] {
        [ImagePlacement {
            line,
            id: 1,
            cols: 4,
            rows,
            covers,
        }]
    }

    const FENCE: &str = "a\n```mermaid\nx\n```\nnext\n";

    #[test]
    fn taller_diagram_reserves_the_extra_rows_after_its_block() {
        // lines 1..4 covered, 5 rows -> 2 extra rows after line 3
        let rows = rows_by_line(FENCE, &diagram_placement(1, 3, 5), None);
        assert_eq!(rows, [(0, 0), (1, 1), (2, 2), (3, 3), (4, 6)]);
    }

    #[test]
    fn diagram_as_tall_as_its_block_reserves_nothing() {
        let rows = rows_by_line(FENCE, &diagram_placement(1, 3, 3), None);
        assert_eq!(rows.last(), Some(&(4, 4)));
    }

    #[test]
    fn shorter_diagram_reserves_nothing() {
        let rows = rows_by_line(FENCE, &diagram_placement(1, 3, 1), None);
        assert_eq!(rows.last(), Some(&(4, 4)));
    }

    #[test]
    fn max_diagram_rows_leaves_two_rows() {
        assert_eq!(max_diagram_rows(40), 38);
        assert_eq!(max_diagram_rows(2), 1);
        assert_eq!(max_diagram_rows(0), 1);
    }

    #[test]
    #[allow(clippy::single_range_in_vec_init)]
    fn covered_ranges_drop_overlapping_layers() {
        let covered = [2..5];
        assert!(is_covered(&covered, 3));
        assert!(is_covered(&covered, 2));
        assert!(!is_covered(&covered, 5));
        assert!(!is_covered(&covered, 1));
    }

    fn rows_by_line(
        md: &str,
        placements: &[ImagePlacement],
        softwrap_width: Option<u16>,
    ) -> Vec<(usize, usize)> {
        let text = Rope::from(md);
        let mut annotations = TextAnnotations::default();
        annotations.add_line_annotation(Box::new(ImageRows::new(text.slice(..), placements)));
        let text_fmt = TextFormat {
            soft_wrap: softwrap_width.is_some(),
            viewport_width: softwrap_width.unwrap_or(80),
            ..TextFormat::default()
        };
        // (doc line, visual row) of the first grapheme of every visual line
        let mut out = Vec::new();
        let mut last_row = usize::MAX;
        for g in
            DocumentFormatter::new_at_prev_checkpoint(text.slice(..), &text_fmt, &annotations, 0)
        {
            if g.visual_pos.row != last_row && !g.source.is_eof() {
                out.push((g.line_idx, g.visual_pos.row));
                last_row = g.visual_pos.row;
            }
        }
        out
    }

    #[test]
    fn image_rows_reserved_after_image_line() {
        let placements = [ImagePlacement {
            line: 0,
            id: 1,
            cols: 4,
            rows: 3,
            covers: 0,
        }];
        assert_eq!(
            rows_by_line("img\nnext\n", &placements, None),
            [(0, 0), (1, 4)]
        );
    }

    #[test]
    fn image_rows_only_after_last_visual_line() {
        let placements = [ImagePlacement {
            line: 0,
            id: 1,
            cols: 4,
            rows: 2,
            covers: 0,
        }];
        let md = "aaaa bbbb cccc dddd eeee\nnext\n";
        // soft-wrapped into several rows; the 2 image rows come after the last one
        let rows = rows_by_line(md, &placements, Some(10));
        let next = rows.iter().find(|(line, _)| *line == 1).unwrap().1;
        let last_wrap = rows
            .iter()
            .filter(|(line, _)| *line == 0)
            .map(|r| r.1)
            .max()
            .unwrap();
        assert_eq!(next, last_wrap + 1 + 2);
    }

    #[test]
    fn stacked_images_add_up() {
        let placements = [
            ImagePlacement {
                line: 0,
                id: 1,
                cols: 4,
                rows: 2,
                covers: 0,
            },
            ImagePlacement {
                line: 0,
                id: 2,
                cols: 4,
                rows: 3,
                covers: 0,
            },
        ];
        assert_eq!(
            rows_by_line("img\nnext\n", &placements, None),
            [(0, 0), (1, 6)]
        );
    }

    #[test]
    fn image_rows_add_to_other_line_annotations() {
        struct OneRow;
        impl LineAnnotation for OneRow {
            fn insert_virtual_lines(
                &mut self,
                _: usize,
                _: helix_core::Position,
                _: usize,
            ) -> helix_core::Position {
                helix_core::Position::new(1, 0)
            }
        }
        let text = Rope::from("img\nnext\n");
        let placements = [ImagePlacement {
            line: 0,
            id: 1,
            cols: 4,
            rows: 2,
            covers: 0,
        }];
        let mut annotations = TextAnnotations::default();
        annotations.add_line_annotation(Box::new(ImageRows::new(text.slice(..), &placements)));
        annotations.add_line_annotation(Box::new(OneRow));
        let text_fmt = TextFormat::default();
        let next =
            DocumentFormatter::new_at_prev_checkpoint(text.slice(..), &text_fmt, &annotations, 0)
                .find(|g| g.line_idx == 1)
                .unwrap();
        assert_eq!(next.visual_pos.row, 1 + 2 + 1);
    }

    #[test]
    fn image_rows_need_a_line_ending() {
        // the formatter never calls line annotations at the end of the text
        let text = Rope::from("img\nnext\n");
        assert!(has_line_end(text.slice(..), 0));
        assert!(has_line_end(text.slice(..), 1));
        let text = Rope::from("img\nlast");
        assert!(has_line_end(text.slice(..), 0));
        assert!(!has_line_end(text.slice(..), 1));
        let text = Rope::from("img");
        assert!(!has_line_end(text.slice(..), 0));
        let placements = [ImagePlacement {
            line: 0,
            id: 1,
            cols: 4,
            rows: 3,
            covers: 0,
        }];
        assert_eq!(rows_by_line("img", &placements, None), [(0, 0)]);
    }

    #[test]
    fn hard_breaks() {
        let md = "a  \nb\n\nc \nd\n\ne\\\nf\n\n```\ng  \nh\n```\n\n> i  \n> j\n\nk  \n";
        let (text, syntax, loader) = parse(md, "markdown");
        let decorations = decorate(&syntax, text.slice(..), 0..text.len_chars(), 20, &loader);
        // a single space, a backslash, code and the end of a paragraph are not
        assert_eq!(decorations.layers(&[]).hard_breaks, [0, 14]);
        // revealed blocks are raw
        let revealed = decorations.revealed(&[1]);
        assert_eq!(decorations.layers(&revealed).hard_breaks, [14]);
    }

    #[test]
    fn max_image_rows_is_half_the_view() {
        assert_eq!(max_image_rows(40), 20);
        assert_eq!(max_image_rows(1), 1);
        assert_eq!(max_image_rows(0), 1);
    }

    #[test]
    fn heading_scale_math() {
        use HeadingScale::*;
        assert_eq!(HeadingScale::for_level(4), None);
        assert_eq!(Double.scaled_width(5, 5), 10);
        assert_eq!(
            Double.scaled_width(4, 2),
            8,
            "wide graphemes scale by width"
        );
        assert_eq!((ThreeHalves.chunk_w(4), ThreeHalves.chunk_w(1)), (3, 1));
        assert_eq!(ThreeHalves.scaled_width(5, 5), 6 + 2);
        assert_eq!((FiveQuarters.chunk_w(8), FiveQuarters.chunk_w(3)), (5, 2));
        assert_eq!(FiveQuarters.scaled_width(9, 9), 10 + 2);
        // screen column relative to the scaled text → formatter column
        assert_eq!(Double.unscale_column(5, 4, 4), 2);
        assert_eq!(ThreeHalves.unscale_column(0, 5, 5), 0);
        assert_eq!(ThreeHalves.unscale_column(5, 5, 5), 3);
        assert_eq!(ThreeHalves.unscale_column(6, 5, 5), 4, "second chunk");
        assert_eq!(FiveQuarters.unscale_column(9, 8, 8), 7);
        assert_eq!(ThreeHalves.unscale_column(11, 5, 5), 4);
    }

    fn record(level: u8, prefix: usize, width: usize, wide: bool) -> HeadingRecord {
        HeadingRecord {
            line: 0,
            level,
            content_start: 2,
            content_end: 2 + width,
            prefix_width: prefix,
            content_width: width,
            graphemes: width,
            has_wide: wide,
            revealed: false,
        }
    }

    fn ctx() -> FitContext {
        FitContext {
            inner_width: 20,
            wrap_width: None,
            horizontal_offset: 0,
            has_line_end: true,
            eol_virtual_text: false,
        }
    }

    #[test]
    fn fit_rules() {
        assert!(
            fit(&record(1, 0, 10, false), &ctx()).is_some(),
            "2×10 fits 20"
        );
        assert!(
            fit(&record(1, 1, 10, false), &ctx()).is_none(),
            "prefix pushes it over"
        );
        assert!(
            fit(&record(4, 0, 3, false), &ctx()).is_none(),
            "h4 is not scaled"
        );
        assert!(fit(
            &record(1, 0, 3, false),
            &FitContext {
                horizontal_offset: 1,
                ..ctx()
            }
        )
        .is_none());
        assert!(fit(
            &record(1, 0, 3, false),
            &FitContext {
                has_line_end: false,
                ..ctx()
            }
        )
        .is_none());
        assert!(fit(
            &record(1, 0, 3, false),
            &FitContext {
                eol_virtual_text: true,
                ..ctx()
            }
        )
        .is_none());
        assert!(fit(
            &record(1, 0, 8, false),
            &FitContext {
                wrap_width: Some(6),
                ..ctx()
            }
        )
        .is_none());
    }

    #[test]
    fn wide_graphemes_only_scale_h1() {
        assert!(fit(&record(1, 0, 4, true), &ctx()).is_some());
        assert!(fit(&record(2, 0, 4, true), &ctx()).is_none());
        assert!(fit(&record(3, 0, 4, true), &ctx()).is_none());
    }

    #[test]
    fn eol_virtual_text_rules() {
        use crate::editor::InlineBlameShow;
        use helix_core::diagnostic::Severity;
        let key = |&d: &(usize, Severity)| d;
        let none: [(usize, Severity); 0] = [];
        let off = DiagnosticFilter::Disable;
        assert!(eol_text_forces_normal(
            3,
            InlineBlameShow::AllLines,
            off,
            &none,
            key
        ));
        assert!(!eol_text_forces_normal(
            3,
            InlineBlameShow::CursorLine,
            off,
            &none,
            key
        ));
        let warn = DiagnosticFilter::Enable(Severity::Warning);
        let diags = [
            (1, Severity::Error),
            (3, Severity::Hint),
            (3, Severity::Error),
            (4, Severity::Hint),
            (6, Severity::Error),
        ];
        assert!(eol_text_forces_normal(
            3,
            InlineBlameShow::Never,
            warn,
            &diags,
            key
        ));
        assert!(!eol_text_forces_normal(
            4,
            InlineBlameShow::Never,
            warn,
            &diags,
            key
        ));
        assert!(
            !eol_text_forces_normal(5, InlineBlameShow::Never, warn, &diags, key),
            "no diagnostic on the line"
        );
        assert!(eol_text_forces_normal(
            6,
            InlineBlameShow::Never,
            warn,
            &diags,
            key
        ));
        assert!(!eol_text_forces_normal(
            3,
            InlineBlameShow::Never,
            off,
            &diags,
            key
        ));
    }

    #[test]
    fn heading_rows_reserved_after_heading_line() {
        let text = Rope::from("# H\nnext\n");
        let mut annotations = TextAnnotations::default();
        annotations.add_line_annotation(Box::new(HeadingRows::new(text.slice(..), vec![0])));
        let text_fmt = TextFormat {
            soft_wrap: false,
            ..TextFormat::default()
        };
        let rows: Vec<(usize, usize)> =
            DocumentFormatter::new_at_prev_checkpoint(text.slice(..), &text_fmt, &annotations, 0)
                .filter(|g| g.visual_pos.col == 0 && !g.source.is_eof())
                .map(|g| (g.line_idx, g.visual_pos.row))
                .collect();
        assert_eq!(rows, [(0, 0), (1, 2)]);
    }

    /// `(annotation, doc line, visual row the rows go below, rows)`.
    type Reservation = (&'static str, usize, usize, usize);

    /// Records the visual row each line annotation is asked to fill rows
    /// after, which is where its decoration draws them.
    struct Spy<A> {
        inner: A,
        name: &'static str,
        log: std::rc::Rc<std::cell::RefCell<Vec<Reservation>>>,
    }

    impl<A: LineAnnotation> LineAnnotation for Spy<A> {
        fn insert_virtual_lines(
            &mut self,
            line_end_char_idx: usize,
            line_end_visual_pos: helix_core::Position,
            doc_line: usize,
        ) -> helix_core::Position {
            let rows =
                self.inner
                    .insert_virtual_lines(line_end_char_idx, line_end_visual_pos, doc_line);
            if rows.row > 0 {
                self.log.borrow_mut().push((
                    self.name,
                    doc_line,
                    line_end_visual_pos.row,
                    rows.row,
                ));
            }
            rows
        }
    }

    /// A diagnostic-like annotation: `rows` rows below `line`.
    struct RowsAfter {
        line: usize,
        rows: usize,
    }

    impl LineAnnotation for RowsAfter {
        fn insert_virtual_lines(
            &mut self,
            _: usize,
            _: helix_core::Position,
            doc_line: usize,
        ) -> helix_core::Position {
            helix_core::Position::new(if doc_line == self.line { self.rows } else { 0 }, 0)
        }
    }

    /// `(annotation, doc line, first row, rows)` of every reservation and the
    /// `(doc line, visual row)` of every line's first grapheme, with heading
    /// rows on `heading_line`, a 3-row image below `image_line` and two
    /// diagnostic rows below `diagnostic_line`, added in the order of
    /// `View::text_annotations` and the editor.
    fn heading_image_diagnostic_rows(
        md: &str,
        heading_line: usize,
        image_line: usize,
        diagnostic_line: usize,
    ) -> (Vec<Reservation>, Vec<(usize, usize)>) {
        ordered_rows(md, heading_line, image_line, diagnostic_line, &[])
    }

    /// As [`heading_image_diagnostic_rows`], with `borders` reserved last.
    fn ordered_rows(
        md: &str,
        heading_line: usize,
        image_line: usize,
        diagnostic_line: usize,
        borders: &[BorderRow],
    ) -> (Vec<Reservation>, Vec<(usize, usize)>) {
        let text = Rope::from(md);
        let placements = [ImagePlacement {
            line: image_line,
            id: 1,
            cols: 4,
            rows: 3,
            covers: 0,
        }];
        let log = std::rc::Rc::default();
        let mut annotations = TextAnnotations::default();
        annotations.add_line_annotation(Box::new(Spy {
            inner: HeadingRows::new(text.slice(..), vec![heading_line]),
            name: "heading",
            log: std::rc::Rc::clone(&log),
        }));
        annotations.add_line_annotation(Box::new(Spy {
            inner: ImageRows::new(text.slice(..), &placements),
            name: "image",
            log: std::rc::Rc::clone(&log),
        }));
        annotations.add_line_annotation(Box::new(Spy {
            inner: RowsAfter {
                line: diagnostic_line,
                rows: 2,
            },
            name: "diagnostic",
            log: std::rc::Rc::clone(&log),
        }));
        annotations.add_line_annotation(Box::new(Spy {
            inner: TableRows::new(text.slice(..), borders),
            name: "table",
            log: std::rc::Rc::clone(&log),
        }));
        let text_fmt = TextFormat {
            soft_wrap: false,
            ..TextFormat::default()
        };
        let lines: Vec<(usize, usize)> =
            DocumentFormatter::new_at_prev_checkpoint(text.slice(..), &text_fmt, &annotations, 0)
                .filter(|g| g.visual_pos.col == 0 && !g.source.is_eof())
                .map(|g| (g.line_idx, g.visual_pos.row))
                .collect();
        drop(annotations);
        let log = log.borrow().clone();
        (log, lines)
    }

    fn border(line: usize, top: bool) -> BorderRow {
        BorderRow {
            line,
            top,
            prefix: String::new(),
            border: if top { "┌─┐" } else { "└─┘" }.into(),
            revealed: false,
        }
    }

    #[test]
    fn table_rows_reserved_after_the_line_before_and_the_last_line() {
        // a table on lines 1..=3 after an empty line annotation set
        let borders = [border(0, true), border(3, false)];
        let (log, lines) = ordered_rows("x\n|a|\n|-|\n|b|\ny\n", 9, 9, 9, &borders);
        assert_eq!(log, [("table", 0, 0, 1), ("table", 3, 4, 1)]);
        assert_eq!(lines, [(0, 0), (1, 2), (2, 3), (3, 4), (4, 6)]);
    }

    #[test]
    fn table_rows_only_at_the_real_end_of_a_line() {
        let text = Rope::from("aaaa bbbb cccc\n|a|\n|-|\n|b|\n");
        let borders = [border(0, true)];
        let mut annotations = TextAnnotations::default();
        annotations.add_line_annotation(Box::new(TableRows::new(text.slice(..), &borders)));
        let text_fmt = TextFormat {
            soft_wrap: true,
            viewport_width: 6,
            ..TextFormat::default()
        };
        let rows: Vec<(usize, usize)> =
            DocumentFormatter::new_at_prev_checkpoint(text.slice(..), &text_fmt, &annotations, 0)
                .filter(|g| g.visual_pos.col == 0 && !g.source.is_eof())
                .map(|g| (g.line_idx, g.visual_pos.row))
                .collect();
        // line 0 wraps to rows 0..=2, the border row is 3
        assert_eq!(rows, [(0, 0), (0, 1), (0, 2), (1, 4), (2, 5), (3, 6)]);
    }

    #[test]
    fn top_border_comes_after_heading_image_and_diagnostic_rows() {
        // all on the line before the table: the top border is reserved last,
        // right above the table
        let borders = [border(0, true), border(3, false)];
        let (log, lines) = ordered_rows("# H\n|a|\n|-|\n|b|\ny\n", 0, 0, 0, &borders);
        assert_eq!(
            log,
            [
                ("heading", 0, 0, 1),
                ("image", 0, 1, 3),
                ("diagnostic", 0, 4, 2),
                ("table", 0, 6, 1),
                ("table", 3, 10, 1),
            ]
        );
        // heading 0, reserved 1, image 2..5, diagnostic 5..7, top border 7
        assert_eq!(lines, [(0, 0), (1, 8), (2, 9), (3, 10), (4, 12)]);
    }

    #[test]
    fn bottom_border_comes_before_the_next_tables_top_border() {
        // a table in a quote, then a table right after it
        let md = "> |a|\n> |-|\n> |b|\n|c|\n|-|\n|d|\n";
        let (text, syntax, loader) = parse(md, "markdown");
        let decorations = decorate(&syntax, text.slice(..), 0..text.len_chars(), 20, &loader);
        let borders = border_rows_of(text.slice(..), &decorations, &[]);
        let tops: Vec<(usize, bool)> = borders.iter().map(|b| (b.line, b.top)).collect();
        assert_eq!(tops, [(2, false), (2, true), (5, false)]);
        assert_eq!(
            render(md),
            "▎ │a│\n▎ ├─┤\n▎ │b│\n▎ └─┘\n┌─┐\n│c│\n├─┤\n│d│\n└─┘"
        );
    }

    #[test]
    fn heading_rows_come_before_image_and_diagnostic_rows() {
        // a heading directly above an image line with a diagnostic
        let (log, lines) = heading_image_diagnostic_rows("# H\n![i](p)\nnext\n", 0, 1, 1);
        assert_eq!(
            log,
            [
                ("heading", 0, 0, 1),
                ("image", 1, 2, 3),
                ("diagnostic", 1, 5, 2)
            ]
        );
        // heading 0, reserved 1, image line 2, image 3..6, diagnostic 6..8
        assert_eq!(lines, [(0, 0), (1, 2), (2, 8)]);

        // all on the heading's line: the reserved row comes first, right
        // below the heading, then the image, then the diagnostic
        let (log, lines) = heading_image_diagnostic_rows("# H\nnext\n", 0, 0, 0);
        assert_eq!(
            log,
            [
                ("heading", 0, 0, 1),
                ("image", 0, 1, 3),
                ("diagnostic", 0, 4, 2)
            ]
        );
        assert_eq!(lines, [(0, 0), (1, 7)]);
    }
}
