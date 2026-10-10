//! In-buffer Markdown rendering: decorations computed from the tree-sitter
//! `markdown` and `markdown.inline` trees, see [`decorate`].

use std::collections::BTreeMap;
use std::ops::Range;

use helix_core::line_ending::line_end_char_index;
use helix_core::syntax::{Layer, Loader, Syntax};
use helix_core::text_annotations::{Conceal, InlineAnnotation, Overlay};
use helix_core::tree_sitter::Node;
use helix_core::RopeSlice;

use crate::document::Mode;
use crate::{Document, Editor, View, ViewId};

/// Decorations of the blocks touching a range, see [`decorate`].
#[derive(Debug, Default, Clone)]
pub struct Decorations {
    /// Sorted, non-overlapping.
    pub blocks: Vec<Block>,
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
}

/// Annotation layers ready for a view, each sorted.
#[derive(Debug, Default)]
pub struct Layers {
    pub conceals: Vec<Conceal>,
    pub overlays: Vec<Overlay>,
    pub padding: Vec<InlineAnnotation>,
}

impl Decorations {
    /// Indices of the blocks containing any of `cursors`.
    pub fn revealed(&self, cursors: &[usize]) -> Vec<usize> {
        self.blocks
            .iter()
            .enumerate()
            .filter(|(_, block)| cursors.iter().any(|cursor| block.range.contains(cursor)))
            .map(|(i, _)| i)
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
}

impl Item {
    fn pos(&self) -> usize {
        match self {
            Item::Conceal(conceal) => conceal.range.start,
            Item::Overlay(overlay) => overlay.char_idx,
            Item::Padding(padding) => padding.char_idx,
        }
    }
}

fn overlaps(node: &Node, range: &Range<u32>) -> bool {
    node.start_byte() < range.end && node.end_byte() > range.start
}

struct Builder<'a> {
    text: RopeSlice<'a>,
    text_width: usize,
    items: Vec<Item>,
    /// Line spans of leaf blocks.
    leaves: Vec<Range<usize>>,
    /// Tables are decorated after inline conceals are known.
    tables: Vec<Node<'a>>,
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

    fn atx_heading(&mut self, node: &Node) {
        let content = node.children().find(|child| child.kind() == "inline");
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
                let glyph = if task.kind() == "task_list_marker_checked" {
                    "☑"
                } else {
                    "☐"
                };
                let end = self.clamp_to_line(start, self.char(task.end_byte()));
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
            "image" => return,
            _ => {}
        }
        for child in node.children() {
            self.walk_inline(child, range);
        }
    }

    fn finish_tables(&mut self) {
        let tables = std::mem::take(&mut self.tables);
        for table in tables {
            self.table(&table);
        }
    }

    /// Display width of `range` as rendered: concealed chars are skipped,
    /// replacements are counted.
    fn visible_width(&self, range: Range<usize>, conceals: &[&Conceal]) -> usize {
        use helix_core::unicode::width::UnicodeWidthStr;
        let mut width = 0;
        let mut pos = range.start;
        while pos < range.end {
            if let Some(conceal) = conceals.iter().find(|c| c.range.contains(&pos)) {
                width += conceal.replacement.as_deref().map_or(0, |r| r.width());
                pos = conceal.range.end;
            } else {
                width += self.text.char(pos).to_string().width();
                pos += 1;
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
                delimiter: row.kind() == "pipe_table_delimiter_row",
                pipes,
                cells,
                leading,
                trailing,
            });
        }

        let table_range = self.char(node.start_byte())..self.char(node.end_byte());
        let conceals: Vec<Conceal> = self
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
        let conceals: Vec<&Conceal> = conceals.iter().collect();

        let columns = rows.iter().map(|r| r.cells.len()).max().unwrap_or(0);
        let mut widths = vec![0; columns];
        let mut aligns = vec![Align::Left; columns];
        for row in &rows {
            for (col, cell) in row.cells.iter().enumerate() {
                let width = if row.delimiter {
                    cell.len()
                } else {
                    self.visible_width(cell.clone(), &conceals)
                };
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
        }

        for row in &rows {
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
                let pad = widths[col] - self.visible_width(cell.clone(), &conceals);
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
                let line = self
                    .text
                    .char_to_line(item.pos().min(self.text.len_chars()));
                let i = spans.partition_point(|span| span.end <= line);
                let lines = match spans.get(i) {
                    Some(span) if span.start <= line => span.clone(),
                    _ => line..line + 1,
                };
                let text = self.text;
                let block = blocks.entry(lines.start).or_insert_with(|| Block {
                    range: text.line_to_char(lines.start)
                        ..text.line_to_char(lines.end.min(text.len_lines())),
                    ..Block::default()
                });
                match item {
                    Item::Conceal(conceal) => block.conceals.push(conceal),
                    Item::Overlay(overlay) => block.overlays.push(overlay),
                    Item::Padding(padding) => block.padding.push(padding),
                }
            }
        }

        for block in blocks.values_mut() {
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
        }
        Decorations {
            blocks: blocks.into_values().collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Key {
    version: i32,
    range: Range<usize>,
    text_width: usize,
}

/// Markdown rendering state of one view of a document.
#[derive(Debug, Default)]
pub struct ViewState {
    key: Option<Key>,
    decorations: Decorations,
    revealed: Vec<usize>,
    pub(crate) overlays: Vec<Overlay>,
    pub(crate) padding: Vec<InlineAnnotation>,
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

/// The lines to decorate: the viewport and one viewport height around it, or
/// around the primary cursor when it lies outside the viewport.
fn decoration_range(view: &View, doc: &Document) -> Range<usize> {
    let text = doc.text().slice(..);
    let height = view.inner_height().max(1);
    let anchor = doc.view_offset(view.id).anchor.min(text.len_chars());
    let anchor_line = text.char_to_line(anchor);
    let cursor_line = doc.selection(view.id).primary().cursor_line(text);
    let (first, last) = if (anchor_line..anchor_line + height).contains(&cursor_line) {
        (anchor_line.saturating_sub(height), anchor_line + 2 * height)
    } else {
        (cursor_line.saturating_sub(height), cursor_line + height)
    };
    let last = last.min(text.len_lines());
    text.line_to_char(first)..text.line_to_char(last)
}

/// Recomputes the markdown layers of `view_id` if the text, the decorated
/// range, the text width or the revealed blocks changed. In insert mode the
/// blocks containing a cursor are revealed.
pub fn refresh(editor: &mut Editor, view_id: ViewId) {
    if !editor.tree.contains(view_id) {
        return;
    }
    let loader = editor.syn_loader.load();
    let insert = editor.mode == Mode::Insert;
    let view = editor.tree.get(view_id);
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
        text_width: view.inner_width(doc) as usize,
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
    if cached.is_some_and(|state| state.decorations.revealed(&cursors) == state.revealed) {
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
    let revealed = decorations.revealed(&cursors);
    let layers = decorations.layers(&revealed);
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
    doc.set_conceals(view_id, layers.conceals);
    doc.markdown_views.insert(
        view_id,
        ViewState {
            key: Some(key),
            decorations,
            revealed,
            overlays: layers.overlays,
            padding: layers.padding,
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

    /// Renders `md` with every block decorated except those containing `cursors`.
    pub(super) fn render_with(md: &str, text_width: usize, cursors: &[usize]) -> String {
        let (text, syntax, loader) = parse(md, "markdown");
        let decorations = decorate(
            &syntax,
            text.slice(..),
            0..text.len_chars(),
            text_width,
            &loader,
        );
        let layers = decorations.layers(&decorations.revealed(cursors));
        assert_well_formed(&layers, text.slice(..));
        let mut annotations = TextAnnotations::default();
        annotations
            .set_conceals(&layers.conceals, None)
            .add_overlay(&layers.overlays, None)
            .add_inline_annotations(&layers.padding, None);
        let text_fmt = TextFormat {
            soft_wrap: false,
            ..TextFormat::default()
        };
        let mut out = String::new();
        for grapheme in
            DocumentFormatter::new_at_prev_checkpoint(text.slice(..), &text_fmt, &annotations, 0)
        {
            if grapheme.raw == Grapheme::Newline {
                out.push('\n');
            } else {
                out.push_str(&grapheme.raw.to_string());
            }
        }
        out.trim_end().to_string()
    }

    pub(super) fn render(md: &str) -> String {
        render_with(md, 20, &[])
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
            "│ a   │ bb │\n├─────┼────┤\n│ ccc │  d │"
        );
    }

    #[test]
    fn table_centre() {
        assert_eq!(
            render("|a|b|\n|:-:|-|\n|xxxxx|y|\n"),
            "│  a  │b│\n├─────┼─┤\n│xxxxx│y│"
        );
    }

    #[test]
    fn table_wide_chars() {
        assert_eq!(
            render("|日本|a|\n|-|-|\n|x|b|\n"),
            "│日本│a│\n├────┼─┤\n│x   │b│"
        );
    }

    #[test]
    fn table_counts_visible_width() {
        assert_eq!(
            render("| **a** | b |\n|-|-|\n| ccc | d |\n"),
            "│ a   │ b │\n├─────┼───┤\n│ ccc │ d │"
        );
    }

    #[test]
    fn table_uneven_rows() {
        assert_eq!(render("|a|b|\n|-|-|\n|c|\n"), "│a│b│\n├─┼─┤\n│c│");
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
            "│a  │bb│\n├───┼──┤\n│ccc│d │"
        );
        assert_eq!(render("a|b\n-|-\nccc|d\n"), "a  │b\n───┼─\nccc│d");
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
        assert_eq!(render("> |a|\n> |-|\n> |b|\n"), "▎ │a│\n▎ ├─┤\n▎ │b│");
    }
}
