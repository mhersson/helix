use helix_core::Position;
use helix_view::graphics::Color;
use helix_view::images::placeholder::{self, id_color};
use helix_view::markdown_render::ImagePlacement;

use crate::ui::document::{LinePos, TextRenderer};
use crate::ui::text_decorations::Decoration;

/// Fills the rows reserved below image lines with Kitty placeholder cells.
pub struct ImageDecoration<'a> {
    placements: &'a [ImagePlacement],
}

impl<'a> ImageDecoration<'a> {
    pub fn new(placements: &'a [ImagePlacement]) -> Self {
        Self { placements }
    }
}

/// `(id, image row, row offset below the line)` for every image row of `doc_line`.
fn rows_for(placements: &[ImagePlacement], doc_line: usize) -> Vec<(u32, u16, u16)> {
    let mut out = Vec::new();
    let mut offset = 0;
    for placement in placements.iter().filter(|p| p.line == doc_line) {
        for row in 0..placement.rows {
            out.push((placement.id, row, offset));
            offset += 1;
        }
    }
    out
}

fn cell(row: u16, col: u16, id: u32) -> (String, Color) {
    (placeholder::placeholder(row, col), id_color(id))
}

impl Decoration for ImageDecoration<'_> {
    fn render_virt_lines(
        &mut self,
        renderer: &mut TextRenderer,
        pos: LinePos,
        virt_off: Position,
    ) -> Position {
        // `ImageRows` reserves the rows after the last visual line only
        if !pos.last_visual_line {
            return Position::new(0, 0);
        }
        let rows = rows_for(self.placements, pos.doc_line);
        for &(id, image_row, offset) in &rows {
            let y = pos.visual_line + virt_off.row as u16 + offset;
            let cols = self
                .placements
                .iter()
                .find(|p| p.id == id && p.line == pos.doc_line)
                .map_or(0, |p| p.cols);
            for col in 0..cols.min(renderer.viewport.width) {
                let (symbol, fg) = cell(image_row, col, id);
                renderer.set_cell(renderer.viewport.x + col, y, &symbol, fg);
            }
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
            },
            ImagePlacement {
                line: 3,
                id: 8,
                cols: 1,
                rows: 2,
            },
            ImagePlacement {
                line: 4,
                id: 9,
                cols: 1,
                rows: 1,
            },
        ];
        // (id, image row, screen row offset) for doc line 3
        assert_eq!(rows_for(&placements, 3), [(7, 0, 0), (8, 0, 1), (8, 1, 2)]);
        assert!(rows_for(&placements, 5).is_empty());
    }

    /// Renders `text` with a 3-row, 4-column image (id 1) below line 0, scrolled down by
    /// `scroll` visual rows the way the view scrolls, and returns the screen rows.
    fn render(text: &str, scroll: isize, soft_wrap: bool) -> Vec<String> {
        use std::sync::Arc;

        use arc_swap::ArcSwap;
        use helix_core::text_annotations::TextAnnotations;
        use helix_core::{char_idx_at_visual_offset, syntax, Rope};
        use helix_view::graphics::Rect;
        use helix_view::{editor::Config, theme, Document};
        use tui::buffer::Buffer as Surface;

        use crate::ui::document::render_text;
        use crate::ui::text_decorations::DecorationManager;

        let placements = [ImagePlacement {
            line: 0,
            id: 1,
            cols: 4,
            rows: 3,
        }];
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
        let viewport = Rect::new(0, 0, 12, 4);
        let theme = theme::DEFAULT_THEME.clone();
        let text_fmt = doc.text_format(viewport.width, None);
        let mut annotations = TextAnnotations::default();
        annotations.add_line_annotation(Box::new(helix_view::markdown_render::ImageRows::new(
            text,
            &placements,
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
        decorations.add_decoration(ImageDecoration::new(&placements));
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
