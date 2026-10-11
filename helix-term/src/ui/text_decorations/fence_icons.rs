//! Styles the language icons that replace the opening fences of rendered
//! Markdown code blocks. A conceal replacement is drawn with the conceal
//! layer's one highlight, so each icon's own colour is patched onto its cells
//! once its line is drawn.

use std::path::Path;

use helix_core::doc_formatter::FormattedGrapheme;
use helix_core::Position;
use helix_view::graphics::{Rect, Style};
use helix_view::icons::ICONS;
use helix_view::markdown_render::FenceIconRef;
use helix_view::Theme;

use crate::ui::document::{LinePos, TextRenderer};
use crate::ui::text_decorations::Decoration;

pub struct FenceIconDecoration<'a> {
    icons: &'a [FenceIconRef],
    theme: &'a Theme,
    /// The next icon to look for.
    idx: usize,
    /// Icons drawn on the current visual line: position, width and style.
    drawn: Vec<(Position, usize, Style)>,
}

impl<'a> FenceIconDecoration<'a> {
    pub fn new(icons: &'a [FenceIconRef], theme: &'a Theme) -> Self {
        Self {
            icons,
            theme,
            idx: 0,
            drawn: Vec::new(),
        }
    }

    /// The style of `icon`, as in the file picker: the icon's colour, or the
    /// theme's `icons.file.<extension>`.
    fn style(&self, icon: &FenceIconRef) -> Option<Style> {
        let icons = ICONS.load();
        let file = icons.fs().file()?;
        let path = format!("_.{}", icon.extension);
        Some(
            file.get_with_style_or_default(Path::new(&path), self.theme)
                .style(),
        )
    }

    fn next_char_idx(&self) -> usize {
        self.icons
            .get(self.idx)
            .map_or(usize::MAX, |icon| icon.char_idx)
    }
}

impl Decoration for FenceIconDecoration<'_> {
    fn reset_pos(&mut self, pos: usize) -> usize {
        self.idx = self.icons.partition_point(|icon| icon.char_idx < pos);
        self.next_char_idx()
    }

    fn decorate_grapheme(
        &mut self,
        _renderer: &mut TextRenderer,
        grapheme: &FormattedGrapheme,
    ) -> usize {
        let Some(icon) = self.icons.get(self.idx) else {
            return usize::MAX;
        };
        // the conceal replacement is the first grapheme at the icon's char
        if grapheme.is_virtual() && grapheme.raw.to_string() == icon.glyph {
            if let Some(style) = self.style(icon) {
                self.drawn
                    .push((grapheme.visual_pos, grapheme.width(), style));
            }
        }
        self.idx += 1;
        self.next_char_idx()
    }

    fn render_virt_lines(
        &mut self,
        renderer: &mut TextRenderer,
        pos: LinePos,
        _virt_off: Position,
    ) -> Position {
        // called once the line is drawn
        for (at, width, style) in self.drawn.drain(..) {
            if at.row == pos.visual_line as usize && renderer.column_in_bounds(at.col, width) {
                let x = renderer.viewport.x + (at.col - renderer.offset.col) as u16;
                renderer.set_style(Rect::new(x, at.row as u16, width as u16, 1), style);
            }
        }
        Position::new(0, 0)
    }
}
