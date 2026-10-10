//! In-buffer Markdown rendering: decorations computed from the tree-sitter
//! `markdown` and `markdown.inline` trees, see [`decorate`].

use std::collections::BTreeMap;
use std::ops::Range;

use helix_core::line_ending::line_end_char_index;
use helix_core::syntax::{Layer, Loader, Syntax};
use helix_core::text_annotations::{Conceal, InlineAnnotation, Overlay};
use helix_core::tree_sitter::Node;
use helix_core::RopeSlice;

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

/// All layers overlapping the byte range, root first.
fn layers_overlapping(syntax: &Syntax, layer: Layer, range: &Range<u32>, out: &mut Vec<Layer>) {
    // a layer has one injection per range; walk each layer only once
    if out.contains(&layer) {
        return;
    }
    out.push(layer);
    for injection in syntax
        .layer(layer)
        .injections_at_byte_idx(range.start)
        .take_while(|injection| injection.range.start < range.end)
    {
        layers_overlapping(syntax, injection.layer, range, out);
    }
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

    let mut layers = Vec::new();
    layers_overlapping(syntax, syntax.root_layer(), &bytes, &mut layers);
    for layer in layers {
        let Some(tree) = syntax.layer(layer).tree() else {
            continue;
        };
        match grammar(loader, syntax, layer) {
            "markdown" => builder.walk_block(tree.root_node(), &bytes, 0),
            "markdown_inline" => builder.walk_inline(tree.root_node(), &bytes),
            _ => {}
        }
    }
    builder.finish_tables();
    builder.finish()
}

#[allow(dead_code)] // `Overlay` and `Padding` are produced by Tasks 3-4
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

#[allow(dead_code)] // `text_width` and `tables` are used by Tasks 3-4
struct Builder<'a> {
    text: RopeSlice<'a>,
    text_width: usize,
    items: Vec<Item>,
    /// Line spans of leaf blocks.
    leaves: Vec<Range<usize>>,
    /// Tables are decorated after inline conceals are known (Task 4).
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

    #[allow(dead_code)] // used by thematic breaks and fences (Task 3)
    fn overlay(&mut self, char_idx: usize, grapheme: &str) {
        self.items
            .push(Item::Overlay(Overlay::new(char_idx, grapheme)));
    }

    #[allow(dead_code)] // used by tables (Task 4)
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
        let mut end = self.char(content.map_or(node.end_byte(), |c| c.start_byte()));
        // hide the whitespace between the marker and the title
        while end < self.text.len_chars() && matches!(self.text.char(end), ' ' | '\t') {
            end += 1;
        }
        self.conceal(start..end, None, None);
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

    /// Decorates the tables collected by `walk_block` (Task 4).
    fn finish_tables(&mut self) {}

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
    fn not_markdown() {
        let (text, syntax, loader) = parse("fn main() {}\n", "rust");
        assert!(!is_markdown_syntax(&syntax, &loader));
        assert!(
            decorate(&syntax, text.slice(..), 0..text.len_chars(), 20, &loader)
                .blocks
                .is_empty()
        );
    }
}
