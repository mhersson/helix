//! Draws Markdown h1–h3 headings enlarged with Kitty's text sizing protocol
//! (OSC 66): after a heading's line is drawn, its text cells are rewritten
//! as multicell characters covering the line and the row `HeadingRows`
//! reserves below it.

use helix_core::doc_formatter::FormattedGrapheme;
use helix_core::Position;
use helix_view::graphics::Style;
use helix_view::markdown_render::{draws_scaled, heading_layouts, HeadingLayout, HeadingScale};
use helix_view::{Document, View};
use tui::multicell::{self, MulticellParams};

use crate::ui::document::{LinePos, TextRenderer};
use crate::ui::text_decorations::Decoration;

struct Heading {
    layout: HeadingLayout,
    /// False on the focused cursor line and in revealed blocks.
    scaled: bool,
}

/// One multicell character of a scaled heading.
#[derive(Debug, PartialEq)]
struct Chunk {
    x: u16,
    text: String,
    params: MulticellParams,
    style: Style,
}

/// The multicells drawing `glyphs` (symbol, style and width of each drawn
/// grapheme) at `scale` from column `x` on. `None` unless every chunk ends at
/// or before `right`, fits a cell and is safe to send inside OSC 66: heading
/// text comes from the file, and control characters in it could end the
/// escape early, so such a heading is drawn at normal size instead.
fn chunks(
    glyphs: &[(String, Style, usize)],
    scale: HeadingScale,
    mut x: u16,
    right: u16,
) -> Option<Vec<Chunk>> {
    let (s, n, d, v) = scale.osc_params();
    let mut chunks = Vec::new();
    for chunk in glyphs.chunks(scale.chunk_graphemes()) {
        let text: String = chunk.iter().map(|g| g.0.as_str()).collect();
        if text.len() > tui::buffer::SYMBOL_CAPACITY || !multicell::is_safe_payload(&text) {
            return None;
        }
        let width: usize = chunk.iter().map(|g| g.2).sum();
        let params = MulticellParams {
            scale: s,
            width: scale.chunk_w(chunk.len()),
            numerator: n,
            denominator: d,
            valign: v,
            halign: 0,
        };
        chunks.push(Chunk {
            x,
            text,
            params,
            // the block has one style: take the first visible grapheme's, as
            // whitespace is drawn in a dim colour that would hide the chunk
            style: chunk
                .iter()
                .find(|g| !g.0.chars().all(char::is_whitespace))
                .unwrap_or(&chunk[0])
                .1,
        });
        x = x.checked_add(scale.chunk_cols(chunk.len(), width) as u16)?;
    }
    (x <= right).then_some(chunks)
}

pub struct HeadingDecoration {
    headings: Vec<Heading>,
    /// The heading whose line is being drawn.
    current: Option<usize>,
    /// Visual column and width of each drawn grapheme of `current`.
    cols: Vec<(u16, usize)>,
    /// Virtual text was drawn inside `current` (e.g. an inlay hint).
    virtual_text: bool,
}

impl HeadingDecoration {
    pub fn new(doc: &Document, view: &View, focused: bool) -> Option<Self> {
        let headings: Vec<Heading> = heading_layouts(doc, view)
            .into_iter()
            .map(|layout| Heading {
                scaled: draws_scaled(doc, view, &layout, focused),
                layout,
            })
            .collect();
        (!headings.is_empty()).then(|| Self {
            headings,
            current: None,
            cols: Vec::new(),
            virtual_text: false,
        })
    }

    fn content(&self, i: usize) -> std::ops::Range<usize> {
        let layout = &self.headings[i].layout;
        layout.content_start..layout.content_end
    }

    /// The next char from `pos` on where a heading's text starts or is.
    fn next_hook(&self, pos: usize) -> usize {
        self.headings
            .iter()
            .map(|h| &h.layout)
            .filter(|l| l.content_end > pos)
            .map(|l| l.content_start.max(pos))
            .min()
            .unwrap_or(usize::MAX)
    }

    /// Rewrites the drawn graphemes of heading `i` on screen row `y` as
    /// multicells; draws nothing unless every chunk fits and is safe.
    fn scale(&self, renderer: &mut TextRenderer, i: usize, y: u16) {
        let layout = &self.headings[i].layout;
        let x0 = renderer.viewport.x;
        let right = renderer.viewport.right();
        let surface = renderer.surface_mut();
        let glyphs: Vec<(String, Style, usize)> = self
            .cols
            .iter()
            .map(|&(col, width)| {
                let cell = &surface[(x0 + col, y)];
                (cell.symbol.to_string(), cell.style(), width)
            })
            .collect();
        let Some(chunks) = chunks(&glyphs, layout.scale, x0 + self.cols[0].0, right) else {
            return;
        };
        for chunk in chunks {
            multicell::place(surface, chunk.x, y, &chunk.text, chunk.params, chunk.style);
        }
    }
}

impl Decoration for HeadingDecoration {
    fn reset_pos(&mut self, pos: usize) -> usize {
        self.next_hook(pos)
    }

    fn decorate_line(&mut self, _renderer: &mut TextRenderer, pos: LinePos) {
        if pos.first_visual_line {
            self.current = self
                .headings
                .iter()
                .position(|h| h.layout.line == pos.doc_line);
            self.cols.clear();
            self.virtual_text = false;
        }
    }

    fn decorate_grapheme(
        &mut self,
        _renderer: &mut TextRenderer,
        grapheme: &FormattedGrapheme,
    ) -> usize {
        let pos = grapheme.char_idx;
        if self.current.is_some_and(|i| self.content(i).contains(&pos)) {
            if grapheme.is_virtual() {
                self.virtual_text = true;
            } else {
                self.cols
                    .push((grapheme.visual_pos.col as u16, grapheme.raw.width()));
            }
        }
        // never return `pos` itself: the manager would call again forever
        self.next_hook(pos + 1)
    }

    fn render_virt_lines(
        &mut self,
        renderer: &mut TextRenderer,
        pos: LinePos,
        virt_off: Position,
    ) -> Position {
        if !pos.last_visual_line {
            return Position::new(0, 0);
        }
        let Some(i) = self
            .headings
            .iter()
            .position(|h| h.layout.line == pos.doc_line)
        else {
            return Position::new(0, 0);
        };
        let heading = &self.headings[i];
        let offset_row = renderer.offset.row;
        let visible_row = (pos.visual_line as usize)
            .checked_sub(offset_row)
            .filter(|&row| row + 1 < renderer.viewport.height as usize);
        if let Some(row) = visible_row.filter(|_| {
            heading.scaled
                && pos.first_visual_line
                && virt_off.row == 1
                && self.current == Some(i)
                && !self.virtual_text
                && self.cols.len() == heading.layout.graphemes
        }) {
            let y = renderer.viewport.y + row as u16;
            self.scale(renderer, i, y);
        }
        self.current = None;
        // `HeadingRows` reserved one row below every heading listed here
        Position::new(1, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glyphs(text: &str) -> Vec<(String, Style, usize)> {
        text.chars()
            .map(|c| (c.to_string(), Style::default(), 1))
            .collect()
    }

    fn texts(chunks: &[Chunk]) -> Vec<(u16, &str)> {
        chunks.iter().map(|c| (c.x, c.text.as_str())).collect()
    }

    #[test]
    fn h1_is_one_chunk_per_grapheme_two_cells_apart() {
        let chunks = chunks(&glyphs("One"), HeadingScale::Double, 3, 120).unwrap();
        assert_eq!(texts(&chunks), [(3, "O"), (5, "n"), (7, "e")]);
        assert_eq!(chunks[0].params, MulticellParams::scaled(2));
    }

    #[test]
    fn h2_packs_four_graphemes_per_chunk() {
        let chunks = chunks(&glyphs("Headings"), HeadingScale::ThreeHalves, 0, 120).unwrap();
        assert_eq!(texts(&chunks), [(0, "Head"), (6, "ings")]);
        assert_eq!(chunks[0].params.width, 3);
    }

    fn styled_glyphs(text: &str) -> Vec<(String, Style, usize)> {
        let dim = Style::default().fg(helix_view::graphics::Color::Gray);
        let bright = Style::default().fg(helix_view::graphics::Color::Red);
        text.chars()
            .map(|c| (c.to_string(), if c == ' ' { dim } else { bright }, 1))
            .collect()
    }

    #[test]
    fn chunk_takes_the_style_of_its_first_visible_grapheme() {
        let dim = Style::default().fg(helix_view::graphics::Color::Gray);
        let bright = Style::default().fg(helix_view::graphics::Color::Red);
        let chunks = chunks(
            &styled_glyphs("Second level heading"),
            HeadingScale::ThreeHalves,
            0,
            120,
        )
        .unwrap();
        assert_eq!(chunks[3].text, " hea");
        assert_eq!(chunks[3].style, bright);
        assert!(chunks.iter().all(|c| c.style == bright));

        // all whitespace: the first grapheme's style
        let chunks = chunks_of_spaces(dim);
        assert_eq!(chunks[0].style, dim);
    }

    fn chunks_of_spaces(dim: Style) -> Vec<Chunk> {
        let glyphs: Vec<_> = (0..4).map(|_| (" ".to_string(), dim, 1)).collect();
        chunks(&glyphs, HeadingScale::ThreeHalves, 0, 120).unwrap()
    }

    #[test]
    fn too_wide_falls_back() {
        assert!(chunks(&glyphs("One"), HeadingScale::Double, 0, 6).is_some());
        assert!(chunks(&glyphs("One"), HeadingScale::Double, 0, 5).is_none());
    }

    #[test]
    fn too_long_chunk_falls_back() {
        // 8 graphemes of 4 bytes = 32 bytes in one h3 chunk
        let glyphs: Vec<_> = (0..8)
            .map(|_| ("😀".to_string(), Style::default(), 1))
            .collect();
        assert!(chunks(&glyphs, HeadingScale::FiveQuarters, 0, 120).is_none());
    }

    #[test]
    fn control_chars_make_the_whole_heading_fall_back() {
        for control in ["\x1b", "\x07", "\u{9c}"] {
            let text = format!("Title{control}]0;pwned");
            for scale in [
                HeadingScale::Double,
                HeadingScale::ThreeHalves,
                HeadingScale::FiveQuarters,
            ] {
                assert!(
                    chunks(&glyphs(&text), scale, 0, 120).is_none(),
                    "{control:?} {scale:?}"
                );
            }
        }
        // the escape as its own grapheme and inside a packed chunk alike
        assert!(chunks(&glyphs("A\x1bB"), HeadingScale::Double, 0, 120).is_none());
        assert!(chunks(&glyphs("ABCDEFG\x07"), HeadingScale::FiveQuarters, 0, 120).is_none());
    }
}
