use helix_core::doc_formatter::FormattedGrapheme;
use helix_core::graphemes::Grapheme;
use helix_core::str_utils::char_to_byte_idx;
use helix_core::{Position, RopeSlice};
use helix_view::editor::{
    WhitespaceFeature, WhitespacePalette, WhitespaceRender, WhitespaceRenderValue,
};
use helix_view::theme::Style;

use crate::ui::document::{LinePos, TextRenderer};
use crate::ui::text_decorations::Decoration;

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum WhitespaceKind {
    Space,
    NonBreakingSpace,
    NarrowNonBreakingSpace,
    Tab,
}

impl WhitespaceKind {
    /// The symbol drawn in the `cell`th cell of a grapheme of this kind.
    pub fn to_str(self, palette: &WhitespacePalette, cell: usize) -> &str {
        match self {
            WhitespaceKind::Space => &palette.space,
            WhitespaceKind::NonBreakingSpace => &palette.nbsp,
            WhitespaceKind::NarrowNonBreakingSpace => &palette.nnbsp,
            WhitespaceKind::Tab => {
                let start = char_to_byte_idx(&palette.tab, cell);
                let end = char_to_byte_idx(&palette.tab, cell + 1);
                &palette.tab[start..end]
            }
        }
    }
}

/// Consecutive whitespace graphemes of one kind on one visual row.
#[derive(Debug)]
struct Run {
    kind: WhitespaceKind,
    /// Visual position of the first cell
    pos: Position,
    /// Width in cells
    width: usize,
    /// Overlay style (selections, cursors) of the run's cells
    overlay_style: Style,
}

/// Collects the whitespace at the end of a document line while it is
/// rendered, so it can be redrawn with the trailing whitespace palette once
/// the line end is reached.
#[derive(Debug)]
pub struct TrailingWhitespaceTracker {
    enabled: bool,
    render: WhitespaceRender,
    palette: WhitespacePalette,
    runs: Vec<Run>,
    /// Char index after the last tracked document grapheme
    next_char_idx: usize,
    /// Whether the current line ends in a rendered Markdown hard line break,
    /// see [`HardBreaks`]
    hard_break: bool,
}

impl TrailingWhitespaceTracker {
    pub fn new(render: WhitespaceRender, palette: WhitespacePalette) -> Self {
        Self {
            enabled: render.any(WhitespaceRenderValue::Trailing),
            render,
            palette,
            runs: Vec::new(),
            next_char_idx: 0,
            hard_break: false,
        }
    }

    fn renders(&self, kind: WhitespaceKind) -> bool {
        let value = match kind {
            WhitespaceKind::Space => self.render.space(),
            WhitespaceKind::NonBreakingSpace => self.render.nbsp(),
            WhitespaceKind::NarrowNonBreakingSpace => self.render.nnbsp(),
            WhitespaceKind::Tab => self.render.tab(),
        };
        WhitespaceFeature::Trailing.is_enabled(value)
    }

    /// Tracks a rendered grapheme of `text` and returns whether [`Self::render`]
    /// should be called right after because a line end was reached.
    ///
    /// Virtual text is ignored: whitespace in front of an inlay hint at the end
    /// of a line is still trailing. Concealed text ends the whitespace in front
    /// of it unless it is whitespace itself.
    pub fn track(
        &mut self,
        text: RopeSlice,
        grapheme: &FormattedGrapheme,
        overlay_style: Style,
    ) -> bool {
        if !self.enabled || grapheme.is_virtual() {
            return false;
        }
        if grapheme.char_idx > self.next_char_idx
            && !self.runs.is_empty()
            && text
                .slice(self.next_char_idx..grapheme.char_idx)
                .chars()
                .any(|c| !c.is_whitespace())
        {
            self.runs.clear();
        }
        self.next_char_idx = grapheme.char_idx + grapheme.doc_chars();

        let kind = match grapheme.raw {
            _ if grapheme.source.is_eof() => return true,
            Grapheme::Newline => {
                if self.hard_break {
                    // the trailing spaces of a hard line break are part of the
                    // rendered Markdown
                    while self
                        .runs
                        .last()
                        .is_some_and(|run| run.kind == WhitespaceKind::Space)
                    {
                        self.runs.pop();
                    }
                }
                return true;
            }
            Grapheme::Tab { .. } => WhitespaceKind::Tab,
            Grapheme::Other { ref g } if g == " " => WhitespaceKind::Space,
            Grapheme::Other { ref g } if g == "\u{00A0}" => WhitespaceKind::NonBreakingSpace,
            Grapheme::Other { ref g } if g == "\u{202F}" => WhitespaceKind::NarrowNonBreakingSpace,
            // other whitespace is drawn as is but doesn't end the trailing whitespace
            Grapheme::Other { .. } if grapheme.is_whitespace() => return false,
            Grapheme::Other { .. } => {
                self.runs.clear();
                return false;
            }
        };
        if !self.renders(kind) {
            return false;
        }

        let pos = grapheme.visual_pos;
        let width = grapheme.width();
        match self.runs.last_mut() {
            // every cell of a tab is drawn differently, so tabs are never merged
            Some(run)
                if kind != WhitespaceKind::Tab
                    && run.kind == kind
                    && run.overlay_style == overlay_style
                    && run.pos.row == pos.row
                    && run.pos.col + run.width == pos.col =>
            {
                run.width += width
            }
            _ => self.runs.push(Run {
                kind,
                pos,
                width,
                overlay_style,
            }),
        }
        false
    }

    /// Calls `draw` with the visual position, symbol and overlay style of every
    /// tracked cell and stops tracking them.
    pub fn render(&mut self, mut draw: impl FnMut(Position, &str, Style)) {
        for run in self.runs.drain(..) {
            for cell in 0..run.width {
                let pos = Position::new(run.pos.row, run.pos.col + cell);
                draw(pos, run.kind.to_str(&self.palette, cell), run.overlay_style);
            }
        }
    }
}

/// Tells the tracker which lines end in a rendered Markdown hard line break
/// (`foo␠␠`), whose trailing spaces are not marked.
pub struct HardBreaks<'a> {
    /// Sorted document lines
    lines: &'a [usize],
}

impl<'a> HardBreaks<'a> {
    pub fn new(lines: &'a [usize]) -> Self {
        Self { lines }
    }
}

impl Decoration for HardBreaks<'_> {
    fn decorate_line(&mut self, renderer: &mut TextRenderer, pos: LinePos) {
        renderer.trailing_whitespace_tracker.hard_break =
            self.lines.binary_search(&pos.doc_line).is_ok();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arc_swap::ArcSwap;
    use helix_core::text_annotations::{Conceal, TextAnnotations};
    use helix_core::{syntax, Rope};
    use helix_view::editor::Config;
    use helix_view::graphics::Rect;
    use helix_view::{theme, Document};
    use tui::buffer::Buffer as Surface;

    use crate::ui::document::{render_text, TextRenderer};
    use crate::ui::text_decorations::DecorationManager;

    use super::*;

    struct Render<'a> {
        text: &'a str,
        render: WhitespaceRender,
        width: u16,
        col_offset: usize,
        soft_wrap: bool,
        conceals: Vec<Conceal>,
        /// Char index of a `ui.cursor` overlay
        cursor: Option<usize>,
        /// Lines ending in a rendered Markdown hard line break
        hard_breaks: Vec<usize>,
    }

    impl<'a> Render<'a> {
        fn new(text: &'a str) -> Self {
            Self {
                text,
                render: WhitespaceRender::Basic(WhitespaceRenderValue::Trailing),
                width: 12,
                col_offset: 0,
                soft_wrap: false,
                conceals: Vec::new(),
                cursor: None,
                hard_breaks: Vec::new(),
            }
        }

        /// Renders the text as Markdown with rendering on.
        fn markdown(mut self) -> Self {
            let loader = helix_core::config::default_lang_loader();
            let text = Rope::from(self.text);
            let language = loader.language_for_name("markdown").unwrap();
            let syntax = syntax::Syntax::new(text.slice(..), language, &loader).unwrap();
            let decorations = helix_view::markdown_render::decorate(
                &syntax,
                text.slice(..),
                0..text.len_chars(),
                self.width as usize,
                &loader,
            );
            self.hard_breaks = decorations.layers(&[]).hard_breaks;
            self
        }

        fn surface(&self, theme: &theme::Theme) -> Surface {
            let mut config = Config::default();
            config.whitespace.render = self.render;
            config.soft_wrap.enable = Some(self.soft_wrap);
            config.soft_wrap.wrap_indicator = Some(String::new());
            let doc = Document::from(
                Rope::from(self.text),
                None,
                Arc::new(ArcSwap::new(Arc::new(config))),
                Arc::new(ArcSwap::from_pointee(syntax::Loader::default())),
            );
            let text = doc.text().slice(..);
            let viewport = Rect::new(0, 0, self.width, 4);
            let text_fmt = doc.text_format(viewport.width, None);
            let mut annotations = TextAnnotations::default();
            annotations.set_conceals(&self.conceals, None);

            let mut surface = Surface::empty(viewport);
            let mut decorations = DecorationManager::default();
            decorations.add_decoration(HardBreaks::new(&self.hard_breaks));
            let mut renderer = TextRenderer::new(
                &mut surface,
                &doc,
                theme,
                Position::new(0, self.col_offset),
                viewport,
            );
            render_text(
                &mut renderer,
                text,
                0,
                &text_fmt,
                &annotations,
                None,
                self.cursor
                    .map(|cursor| syntax::OverlayHighlights::Heterogenous {
                        highlights: vec![(
                            theme.find_highlight_exact("ui.cursor").unwrap(),
                            cursor..cursor + 1,
                        )],
                    })
                    .into_iter()
                    .collect(),
                theme,
                decorations,
            );
            surface
        }

        fn rows(&self) -> Vec<String> {
            let surface = self.surface(&theme::DEFAULT_THEME);
            let area = surface.area;
            (area.top()..area.bottom())
                .map(|y| {
                    (area.left()..area.right())
                        .map(|x| surface[(x, y)].symbol.as_str())
                        .collect::<String>()
                        .trim_end()
                        .to_string()
                })
                .collect()
        }
    }

    #[test]
    fn marks_only_trailing_whitespace() {
        assert_eq!(
            Render::new("a b  \n  c\n \u{a0}\u{202f}\n").rows(),
            ["a b··", "  c", "·⍽␣", ""]
        );
    }

    #[test]
    fn marks_whitespace_before_the_end_of_file() {
        assert_eq!(Render::new("a\nb  ").rows(), ["a", "b··", "", ""]);
    }

    #[test]
    fn marks_every_cell_of_trailing_tabs() {
        // the first tab is 3 cells wide, the second one 4
        assert_eq!(Render::new("a\t\t\nb\n").rows(), ["a→  →", "b", "", ""]);
    }

    #[test]
    fn leaves_unconfigured_whitespace_kinds_alone() {
        let mut render = Render::new("a \t\n");
        render.render = WhitespaceRender::Specific {
            default: None,
            space: Some(WhitespaceRenderValue::Trailing),
            nbsp: None,
            nnbsp: None,
            tab: None,
            newline: None,
        };
        assert_eq!(render.rows(), ["a·", "", "", ""]);
    }

    #[test]
    fn all_draws_the_regular_palette() {
        let mut render = Render::new("a b \n");
        render.render = WhitespaceRender::Basic(WhitespaceRenderValue::All);
        assert_eq!(render.rows(), ["a·b·⏎", "", "", ""]);
    }

    #[test]
    fn whitespace_cut_off_by_the_viewport_is_not_trailing_unless_it_is() {
        let mut render = Render::new("ab  cd\nab    \n");
        render.width = 4;
        assert_eq!(render.rows(), ["ab", "ab··", "", ""]);
        render.col_offset = 3;
        assert_eq!(render.rows(), [" cd", "···", "", ""]);
    }

    #[test]
    fn soft_wrapped_trailing_whitespace_is_marked_on_every_row() {
        let text = format!("abcdefgh{}\n", " ".repeat(20));
        let mut render = Render::new(&text);
        render.soft_wrap = true;
        assert_eq!(render.rows(), ["abcdefgh····", "············", "····", ""]);
    }

    #[test]
    fn whitespace_at_a_soft_wrap_is_not_trailing() {
        let mut render = Render::new("abcd efgh  ijklmn  \n");
        render.soft_wrap = true;
        assert_eq!(render.rows(), ["abcd efgh", "ijklmn··", "", ""]);
    }

    #[test]
    fn concealed_text_ends_trailing_whitespace() {
        // `[a  ](b)` with `](b)` concealed: the spaces are not trailing
        let mut render = Render::new("[a  ](b)\nc  \n");
        render.conceals = vec![Conceal::new(0..1), Conceal::new(4..8)];
        assert_eq!(render.rows(), ["a", "c··", "", ""]);
    }

    #[test]
    fn concealed_trailing_whitespace_is_not_drawn() {
        // hiding only part of the trailing whitespace marks the visible rest
        let mut render = Render::new("ab   \n");
        render.conceals = vec![Conceal::new(3..5)];
        assert_eq!(render.rows(), ["ab·", "", "", ""]);
    }

    #[test]
    fn rendered_markdown_hard_breaks_are_not_marked() {
        let text = "a  \nb \nc\t\n";
        assert_eq!(
            Render::new(text).rows(),
            ["a··", "b·", "c→", ""],
            "raw Markdown"
        );
        // a single space and a tab are not hard breaks
        assert_eq!(
            Render::new(text).markdown().rows(),
            ["a", "b·", "c→", ""],
            "rendered Markdown"
        );
        // code is shown raw
        assert_eq!(
            Render::new("```\nd  \ne\n```\n").markdown().rows(),
            ["```", "d··", "e", "```"],
            "fenced code"
        );
    }

    #[test]
    fn trailing_whitespace_style_stays_below_cursors() {
        use helix_view::graphics::Color;

        let theme: theme::Theme = toml::from_str(
            r#"
            "ui.virtual.trailing-whitespace" = { fg = "red", bg = "red" }
            "ui.cursor" = { bg = "blue" }
            "#,
        )
        .unwrap();
        let mut render = Render::new("ab  \n");
        render.cursor = Some(3);
        let surface = render.surface(&theme);
        let cells: Vec<_> = (2..4)
            .map(|x| {
                let cell = &surface[(x, 0)];
                (cell.symbol.as_str(), cell.fg, cell.bg)
            })
            .collect();
        assert_eq!(
            cells,
            [
                ("·", Color::Red, Color::Red),
                ("·", Color::Red, Color::Blue)
            ]
        );
    }
}
