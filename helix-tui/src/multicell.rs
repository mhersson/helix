//! Kitty text-sizing (OSC 66) multicell characters in the cell buffer.
//!
//! A multicell character is drawn by its top-left [`Multicell::Origin`] cell
//! and covers a block of [`MulticellParams::rows`] × [`MulticellParams::cols`]
//! cells. The other cells of the block are [`Multicell::Covered`] and are never
//! written to the terminal: text written into a lower row of a multicell makes
//! the terminal skip past it, and text written into its top row erases it.

use crate::buffer::{Buffer, SYMBOL_CAPACITY};
use helix_core::unicode::width::UnicodeWidthStr;
use helix_view::graphics::Style;

/// The OSC 66 metadata of a multicell character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MulticellParams {
    /// `s`: overall scale, 1–7.
    pub scale: u8,
    /// `w`: width in scaled cells; 0 lets the terminal use the symbol's width.
    pub width: u8,
    /// `n`: numerator of the fractional scale (0 = none).
    pub numerator: u8,
    /// `d`: denominator of the fractional scale (0 = none).
    pub denominator: u8,
    /// `v`: vertical alignment of fractional text: 0 top, 1 bottom, 2 centre.
    pub valign: u8,
    /// `h`: horizontal alignment of fractional text: 0 left, 1 right, 2 centre.
    pub halign: u8,
}

impl MulticellParams {
    /// Integer scale `scale`, the symbol's own width, no fraction.
    pub const fn scaled(scale: u8) -> Self {
        Self {
            scale,
            width: 0,
            numerator: 0,
            denominator: 0,
            valign: 0,
            halign: 0,
        }
    }

    /// Columns of the block for a symbol `symbol_width` cells wide.
    pub fn cols(&self, symbol_width: usize) -> usize {
        let width = if self.width == 0 {
            symbol_width.max(1)
        } else {
            self.width as usize
        };
        self.scale.max(1) as usize * width
    }

    /// Rows of the block.
    pub fn rows(&self) -> usize {
        self.scale.max(1) as usize
    }
}

/// Part of a multicell character, or not.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Multicell {
    #[default]
    None,
    /// Top-left cell: its symbol is drawn with these parameters.
    Origin(MulticellParams),
    /// Covered by an origin up or left of it; never drawn.
    Covered,
}

/// The OSC 66 escape drawing `text` with `params`; zero optional keys are
/// omitted.
pub fn osc66(params: &MulticellParams, text: &str) -> String {
    let mut meta = format!("s={}", params.scale.max(1));
    for (key, value) in [
        ("w", params.width),
        ("n", params.numerator),
        ("d", params.denominator),
        ("v", params.valign),
        ("h", params.halign),
    ] {
        if value != 0 {
            meta.push_str(&format!(":{key}={value}"));
        }
    }
    format!("\x1b]66;{meta};{text}\x07")
}

/// Writes `symbol` as a multicell character with its origin at `(x, y)`: every
/// cell of the block gets `style`; the origin holds `symbol`, the others are
/// [`Multicell::Covered`] blanks. Writes nothing and returns false when the
/// block leaves the buffer or `symbol` exceeds the cell capacity.
pub fn place(
    buffer: &mut Buffer,
    x: u16,
    y: u16,
    symbol: &str,
    params: MulticellParams,
    style: Style,
) -> bool {
    let area = buffer.area;
    let cols = params.cols(symbol.width()) as u16;
    let rows = params.rows() as u16;
    if symbol.len() > SYMBOL_CAPACITY
        || x < area.left()
        || y < area.top()
        || x + cols > area.right()
        || y + rows > area.bottom()
    {
        return false;
    }
    for by in y..y + rows {
        for bx in x..x + cols {
            let cell = &mut buffer[(bx, by)];
            cell.reset();
            cell.set_style(style);
            if (bx, by) == (x, y) {
                cell.set_symbol(symbol);
                cell.multicell = Multicell::Origin(params);
            } else {
                cell.set_symbol(" ");
                cell.multicell = Multicell::Covered;
            }
        }
    }
    true
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::{Buffer, Cell};
    use helix_view::graphics::{Color, Rect, Style};

    #[test]
    fn osc66_omits_zero_keys() {
        assert_eq!(osc66(&MulticellParams::scaled(2), "H"), "\x1b]66;s=2;H\x07");
        let packed = MulticellParams {
            scale: 2,
            width: 3,
            numerator: 3,
            denominator: 4,
            valign: 2,
            halign: 0,
        };
        assert_eq!(
            osc66(&packed, "Head"),
            "\x1b]66;s=2:w=3:n=3:d=4:v=2;Head\x07"
        );
    }

    #[test]
    fn block_size() {
        let s2 = MulticellParams::scaled(2);
        assert_eq!((s2.cols(1), s2.rows()), (2, 2));
        assert_eq!(s2.cols(2), 4, "a wide grapheme is twice as wide");
        let packed = MulticellParams { width: 3, ..s2 };
        assert_eq!(packed.cols(4), 6, "w wins over the symbol width");
    }

    #[test]
    fn writing_a_symbol_resets_the_marker() {
        let mut cell = Cell::default();
        cell.multicell = Multicell::Covered;
        cell.set_symbol("x");
        assert_eq!(cell.multicell, Multicell::None);
        cell.multicell = Multicell::Covered;
        cell.set_char('y');
        assert_eq!(cell.multicell, Multicell::None);
    }

    #[test]
    fn place_writes_origin_and_covered_cells() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 6, 3));
        let style = Style::default().fg(Color::Red);
        assert!(place(
            &mut buffer,
            1,
            0,
            "a",
            MulticellParams::scaled(2),
            style
        ));
        let origin = &buffer[(1, 0)];
        assert_eq!(origin.symbol.as_str(), "a");
        assert_eq!(
            origin.multicell,
            Multicell::Origin(MulticellParams::scaled(2))
        );
        assert_eq!(origin.fg, Color::Red);
        for (x, y) in [(2, 0), (1, 1), (2, 1)] {
            assert_eq!(buffer[(x, y)].multicell, Multicell::Covered, "({x}, {y})");
            assert_eq!(buffer[(x, y)].fg, Color::Red);
        }
        assert_eq!(buffer[(0, 0)].multicell, Multicell::None);
    }

    #[test]
    fn place_refuses_blocks_that_do_not_fit() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 4, 2));
        assert!(!place(
            &mut buffer,
            3,
            0,
            "a",
            MulticellParams::scaled(2),
            Style::default()
        ));
        assert!(!place(
            &mut buffer,
            0,
            1,
            "a",
            MulticellParams::scaled(2),
            Style::default()
        ));
        let long = "x".repeat(crate::buffer::SYMBOL_CAPACITY + 1);
        let packed = MulticellParams {
            width: 1,
            ..MulticellParams::scaled(2)
        };
        assert!(!place(&mut buffer, 0, 0, &long, packed, Style::default()));
        assert!(buffer
            .content
            .iter()
            .all(|c| c.multicell == Multicell::None));
    }
}
