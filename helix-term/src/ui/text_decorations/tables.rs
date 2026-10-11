//! Draws the top and bottom borders of rendered Markdown tables in the rows
//! `TableRows` reserves.

use helix_core::graphemes::grapheme_width;
use helix_core::unicode::segmentation::UnicodeSegmentation;
use helix_core::Position;
use helix_view::graphics::Style;
use helix_view::markdown_render::{rows_after, BorderRow};
use helix_view::Theme;

use crate::ui::document::{LinePos, TextRenderer};
use crate::ui::text_decorations::Decoration;

/// Must be added after every other decoration that draws virtual rows, as
/// `TableRows` is the last line annotation.
pub struct TableDecoration<'a> {
    rows: &'a [BorderRow],
    /// The highlights of the delimiter row: `punctuation.special`, inside a
    /// `markup.quote` in a block quote.
    rule: Style,
    quote: Style,
}

impl<'a> TableDecoration<'a> {
    pub fn new(rows: &'a [BorderRow], theme: &Theme) -> Self {
        Self {
            rows,
            rule: theme.get("punctuation.special"),
            quote: theme.get("markup.quote"),
        }
    }

    /// Draws `row` on visual row `y`, shifted by the horizontal scroll and
    /// cut at the view's edge.
    fn draw(&self, renderer: &mut TextRenderer, y: u16, row: &BorderRow) {
        let top = renderer.offset.row;
        if (y as usize) < top || y as usize - top >= renderer.viewport.height as usize {
            return;
        }
        let mut base = renderer.text_style;
        if row.prefix.contains('▎') {
            base = base.patch(self.quote);
        }
        let rule = base.patch(self.rule);
        let mut col = 0;
        for grapheme in row.prefix.graphemes(true).chain(row.border.graphemes(true)) {
            let width = grapheme_width(grapheme);
            if renderer.column_in_bounds(col, width) {
                let x = renderer.viewport.x + (col - renderer.offset.col) as u16;
                let style = if grapheme == " " { base } else { rule };
                renderer.set_string(x, y, grapheme, style);
            }
            col += width;
        }
    }
}

impl Decoration for TableDecoration<'_> {
    fn render_virt_lines(
        &mut self,
        renderer: &mut TextRenderer,
        pos: LinePos,
        virt_off: Position,
    ) -> Position {
        // `TableRows` reserves rows after the last visual line only
        if !pos.last_visual_line {
            return Position::new(0, 0);
        }
        let rows = rows_after(self.rows, pos.doc_line);
        for (k, row) in rows.iter().enumerate() {
            // a revealed table keeps its rows, drawn blank
            if !row.revealed {
                let y = pos.visual_line + (virt_off.row + k) as u16;
                self.draw(renderer, y, row);
            }
        }
        Position::new(rows.len(), 0)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn border(line: usize, top: bool, prefix: &str, border: &str) -> BorderRow {
        BorderRow {
            line,
            top,
            prefix: prefix.into(),
            border: border.into(),
            revealed: false,
        }
    }

    /// Renders `text` 8 columns wide and `height` rows high with `borders`
    /// and the view scrolled `col` columns to the right, as screen rows.
    fn render(text: &str, borders: &[BorderRow], col: usize, height: u16) -> Vec<String> {
        use std::sync::Arc;

        use arc_swap::ArcSwap;
        use helix_core::text_annotations::TextAnnotations;
        use helix_core::{syntax, Rope};
        use helix_view::graphics::Rect;
        use helix_view::{editor::Config, theme, Document};
        use tui::buffer::Buffer as Surface;

        use crate::ui::document::render_text;
        use crate::ui::text_decorations::DecorationManager;

        let mut config = Config::default();
        config.soft_wrap.enable = Some(false);
        let doc = Document::from(
            Rope::from(text),
            None,
            Arc::new(ArcSwap::new(Arc::new(config))),
            Arc::new(ArcSwap::from_pointee(syntax::Loader::default())),
        );
        let text = doc.text().slice(..);
        let viewport = Rect::new(0, 0, 8, height);
        let theme = theme::DEFAULT_THEME.clone();
        let text_fmt = doc.text_format(viewport.width, None);
        let mut annotations = TextAnnotations::default();
        annotations.add_line_annotation(Box::new(helix_view::markdown_render::TableRows::new(
            text, borders,
        )));
        let mut surface = Surface::empty(viewport);
        let mut renderer =
            TextRenderer::new(&mut surface, &doc, &theme, Position::new(0, col), viewport);
        let mut decorations = DecorationManager::default();
        decorations.add_decoration(TableDecoration::new(borders, &theme));
        render_text(
            &mut renderer,
            text,
            0,
            &text_fmt,
            &annotations,
            None,
            Vec::new(),
            &theme,
            decorations,
        );
        (0..viewport.height)
            .map(|y| {
                (0..viewport.width)
                    .map(|x| surface[(x, y)].symbol.to_string())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn draws_borders_in_the_reserved_rows() {
        let borders = [border(0, true, "", "┌─┐"), border(1, false, "", "└─┘")];
        assert_eq!(
            render("x\n|a|\ny\n", &borders, 0, 5),
            ["x", "┌─┐", "|a|", "└─┘", "y"]
        );
    }

    #[test]
    fn bottom_then_top_border_on_the_same_line() {
        let borders = [border(0, false, "▎ ", "└─┘"), border(0, true, "", "┌─┐")];
        assert_eq!(
            render("|a|\n|b|\n", &borders, 0, 4),
            ["|a|", "▎ └─┘", "┌─┐", "|b|"]
        );
    }

    #[test]
    fn revealed_border_rows_stay_blank() {
        let mut top = border(0, true, "", "┌─┐");
        top.revealed = true;
        assert_eq!(render("x\n|a|\n", &[top], 0, 3), ["x", "", "|a|"]);
    }

    #[test]
    fn too_wide_border_is_cut_at_the_view_edge() {
        let borders = [border(0, true, "▎ ", "┌──────────┐")];
        assert_eq!(render("x\n|a|\n", &borders, 0, 3), ["x", "▎ ┌─────", "|a|"]);
    }

    #[test]
    fn border_scrolls_with_the_text() {
        let borders = [border(0, true, "", "┌─┬───┐")];
        assert_eq!(
            render("xy\n|a|bcd|\n", &borders, 2, 3),
            ["", "┬───┐", "|bcd|"]
        );
    }

    #[test]
    fn rows_below_the_view_are_not_drawn() {
        let borders = [border(0, true, "", "┌─┐")];
        // must not panic drawing past the bottom
        assert_eq!(render("x\n|a|\n", &borders, 0, 1), ["x"]);
    }
}
