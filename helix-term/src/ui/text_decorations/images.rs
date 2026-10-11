use helix_core::Position;
use helix_view::graphics::Color;
use helix_view::images::placeholder::{self, id_color};
use helix_view::markdown_render::ImagePlacement;

use crate::ui::document::{LinePos, TextRenderer};
use crate::ui::text_decorations::Decoration;

/// Draws Kitty placeholder cells for images below their caption line and for
/// diagrams over their fenced lines.
pub struct ImageDecoration<'a> {
    placements: &'a [ImagePlacement],
}

impl<'a> ImageDecoration<'a> {
    pub fn new(placements: &'a [ImagePlacement]) -> Self {
        Self { placements }
    }
}

/// `(id, image row, row offset below the line, cols)` for every image row
/// drawn in the virtual rows after `doc_line`.
fn rows_for(placements: &[ImagePlacement], doc_line: usize) -> Vec<(u32, u16, u16, u16)> {
    let mut out = Vec::new();
    let mut offset = 0;
    for placement in placements.iter().filter(|p| p.last_line() == doc_line) {
        let first = placement.covers.min(u16::MAX as usize) as u16;
        for row in first..placement.rows {
            out.push((placement.id, row, offset, placement.cols));
            offset += 1;
        }
    }
    out
}

/// `(id, image row, cols)` drawn over `doc_line` itself, which a diagram covers.
fn covered_row(placements: &[ImagePlacement], doc_line: usize) -> Option<(u32, u16, u16)> {
    placements.iter().find_map(|p| {
        let k = doc_line.checked_sub(p.line)?;
        (k < p.covers && k < p.rows as usize).then_some((p.id, k as u16, p.cols))
    })
}

fn cell(row: u16, col: u16, id: u32) -> (String, Color) {
    (placeholder::placeholder(row, col), id_color(id))
}

impl ImageDecoration<'_> {
    fn draw_row(renderer: &mut TextRenderer, y: u16, id: u32, image_row: u16, cols: u16) {
        for col in 0..cols.min(renderer.viewport.width) {
            let (symbol, fg) = cell(image_row, col, id);
            renderer.set_cell(renderer.viewport.x + col, y, &symbol, fg);
        }
    }
}

impl Decoration for ImageDecoration<'_> {
    fn render_virt_lines(
        &mut self,
        renderer: &mut TextRenderer,
        pos: LinePos,
        virt_off: Position,
    ) -> Position {
        // `ImageRows` reserves the rows after the last visual line only; a
        // covered line is concealed to a single (last) visual line
        if !pos.last_visual_line {
            return Position::new(0, 0);
        }
        // drawn after the line's text, which renders its newline as a space
        if let Some((id, image_row, cols)) = covered_row(self.placements, pos.doc_line) {
            Self::draw_row(renderer, pos.visual_line, id, image_row, cols);
        }
        let rows = rows_for(self.placements, pos.doc_line);
        for &(id, image_row, offset, cols) in &rows {
            let y = pos.visual_line + virt_off.row as u16 + offset;
            Self::draw_row(renderer, y, id, image_row, cols);
        }
        Position::new(rows.len(), 0)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use helix_view::graphics::Color;
    use helix_view::markdown_render::ImagePlacement;

    #[test]
    fn stacks_placements_of_a_line() {
        let placements = [
            ImagePlacement {
                line: 3,
                id: 7,
                cols: 2,
                rows: 1,
                covers: 0,
            },
            ImagePlacement {
                line: 3,
                id: 8,
                cols: 1,
                rows: 2,
                covers: 0,
            },
            ImagePlacement {
                line: 4,
                id: 9,
                cols: 1,
                rows: 1,
                covers: 0,
            },
        ];
        // (id, image row, screen row offset, cols) for doc line 3
        assert_eq!(
            rows_for(&placements, 3),
            [(7, 0, 0, 2), (8, 0, 1, 1), (8, 1, 2, 1)]
        );
        assert!(rows_for(&placements, 5).is_empty());
    }

    /// Renders `text` with a 3-row, 4-column image (id 1) below line 0, scrolled down by
    /// `scroll` visual rows the way the view scrolls, and returns the screen rows.
    fn render(text: &str, scroll: isize, soft_wrap: bool) -> Vec<String> {
        let placements = [ImagePlacement {
            line: 0,
            id: 1,
            cols: 4,
            rows: 3,
            covers: 0,
        }];
        render_with(text, &placements, scroll, soft_wrap, 4)
    }

    fn render_with(
        text: &str,
        placements: &[ImagePlacement],
        scroll: isize,
        soft_wrap: bool,
        height: u16,
    ) -> Vec<String> {
        use std::sync::Arc;

        use arc_swap::ArcSwap;
        use helix_core::text_annotations::TextAnnotations;
        use helix_core::{char_idx_at_visual_offset, syntax, Rope};
        use helix_view::graphics::Rect;
        use helix_view::{editor::Config, theme, Document};
        use tui::buffer::Buffer as Surface;

        use crate::ui::document::render_text;
        use crate::ui::text_decorations::DecorationManager;

        let mut config = Config::default();
        config.soft_wrap.enable = Some(soft_wrap);
        config.soft_wrap.wrap_indicator = Some(String::new());
        let doc = Document::from(
            Rope::from(text),
            None,
            Arc::new(ArcSwap::new(Arc::new(config))),
            Arc::new(ArcSwap::from_pointee(syntax::Loader::default())),
        );
        let text = doc.text().slice(..);
        let viewport = Rect::new(0, 0, 12, height);
        let theme = theme::DEFAULT_THEME.clone();
        let text_fmt = doc.text_format(viewport.width, None);
        let rope = doc.text();
        let conceals: Vec<helix_core::text_annotations::Conceal> = placements
            .iter()
            .flat_map(|p| p.line..p.line + p.covers)
            .filter_map(|line| {
                let start = rope.line_to_char(line);
                let end = helix_core::line_ending::line_end_char_index(&rope.slice(..), line);
                (start < end).then(|| helix_core::text_annotations::Conceal::new(start..end))
            })
            .collect();
        let mut annotations = TextAnnotations::default();
        annotations.set_conceals(&conceals, None);
        annotations.add_line_annotation(Box::new(helix_view::markdown_render::ImageRows::new(
            text, placements,
        )));
        let (anchor, vertical_offset) =
            char_idx_at_visual_offset(text, 0, scroll, 0, &text_fmt, &annotations);

        let mut surface = Surface::empty(viewport);
        let mut renderer = TextRenderer::new(
            &mut surface,
            &doc,
            &theme,
            Position::new(vertical_offset, 0),
            viewport,
        );
        let mut decorations = DecorationManager::default();
        decorations.add_decoration(ImageDecoration::new(placements));
        render_text(
            &mut renderer,
            text,
            anchor,
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
                    .map(|x| match surface[(x, y)].symbol.as_str() {
                        s if s.starts_with('\u{10EEEE}') => {
                            let row = placeholder::DIACRITICS
                                .iter()
                                .position(|&d| s.chars().nth(1) == Some(d))
                                .unwrap();
                            char::from_digit(row as u32, 10).unwrap()
                        }
                        s => s.chars().next().unwrap(),
                    })
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn draws_rows_below_the_image_line() {
        assert_eq!(
            render("cap\nafter\n", 0, false),
            ["cap", "0000", "1111", "2222"]
        );
    }

    #[test]
    fn partially_scrolled_image_draws_remaining_rows() {
        // the caption and the first image row are above the viewport
        assert_eq!(
            render("cap\nafter\nmore\n", 2, false),
            ["1111", "2222", "after", "more"]
        );
        assert_eq!(
            render("cap\nafter\nmore\n", 3, false),
            ["2222", "after", "more", ""]
        );
        assert_eq!(
            render("cap\nafter\nmore\n", 4, false),
            ["after", "more", "", ""]
        );
    }

    #[test]
    fn soft_wrapped_line_draws_after_its_last_visual_line() {
        assert_eq!(
            render("abcdefghijklmn\n", 0, true),
            ["abcdefghijkl", "mn", "0000", "1111"]
        );
        assert_eq!(
            render("abcdefghijklmn\nafter\n", 2, true),
            ["0000", "1111", "2222", "after"]
        );
        assert_eq!(
            render("abcdefghijklmn\nafter\n", 3, true),
            ["1111", "2222", "after", ""]
        );
    }

    fn diagram(rows: u16) -> [ImagePlacement; 1] {
        [ImagePlacement {
            line: 1,
            id: 1,
            cols: 4,
            rows,
            covers: 3,
        }]
    }

    const BLOCK: &str = "a\n```mermaid\nx\n```\nb\n";

    #[test]
    fn diagram_draws_over_its_lines_then_below() {
        assert_eq!(
            render_with(BLOCK, &diagram(5), 0, false, 7),
            ["a", "0000", "1111", "2222", "3333", "4444", "b"]
        );
    }

    #[test]
    fn short_diagram_leaves_the_rest_of_its_lines_empty() {
        assert_eq!(
            render_with(BLOCK, &diagram(2), 0, false, 5),
            ["a", "0000", "1111", "", "b"]
        );
    }

    #[test]
    fn scrolled_diagram_draws_its_visible_rows() {
        assert_eq!(
            render_with(BLOCK, &diagram(5), 2, false, 4),
            ["1111", "2222", "3333", "4444"]
        );
        assert_eq!(
            render_with(BLOCK, &diagram(5), 4, false, 4),
            ["3333", "4444", "b", ""]
        );
    }

    #[test]
    fn covered_rows_win_over_the_line_text() {
        // the renderer draws each (empty) covered line's newline as a space
        // at column 0 after decorate_line; the placeholder must survive it
        let rows = render_with(BLOCK, &diagram(3), 0, false, 5);
        assert_eq!(&rows[1..4], ["0000", "1111", "2222"]);
    }

    #[test]
    fn stacks_extra_rows_after_the_last_covered_line() {
        let placements = [ImagePlacement {
            line: 2,
            id: 9,
            cols: 1,
            rows: 4,
            covers: 2,
        }];
        assert!(rows_for(&placements, 2).is_empty());
        assert_eq!(rows_for(&placements, 3), [(9, 2, 0, 1), (9, 3, 1, 1)]);
        assert_eq!(covered_row(&placements, 2), Some((9, 0, 1)));
        assert_eq!(covered_row(&placements, 3), Some((9, 1, 1)));
        assert_eq!(covered_row(&placements, 4), None);
    }

    #[test]
    fn placeholder_cells() {
        assert_eq!(
            cell(1, 2, 0x01_02_03),
            (
                "\u{10EEEE}\u{030D}\u{030E}".to_string(),
                Color::Rgb(1, 2, 3)
            )
        );
    }
}
