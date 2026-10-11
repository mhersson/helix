//! Kitty text-sizing (OSC 66) multicell characters in the cell buffer.
//!
//! A multicell character is drawn by its top-left [`Multicell::Origin`] cell
//! and covers a block of [`MulticellParams::rows`] × [`MulticellParams::cols`]
//! cells. The other cells of the block are [`Multicell::Covered`] and are never
//! written to the terminal: text written into a lower row of a multicell makes
//! the terminal skip past it, and text written into its top row erases it.

use crate::buffer::{Buffer, Cell, SYMBOL_CAPACITY};
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

/// Whether `text` can be carried inside an OSC 66 payload: no C0, DEL or C1
/// control characters, which could end the sequence early and inject others.
pub fn is_safe_payload(text: &str) -> bool {
    !text.chars().any(|c| c.is_control())
}

/// The OSC 66 escape drawing `text` with `params`; zero optional keys are
/// omitted. `None` for an unsafe payload (see [`is_safe_payload`]).
pub fn osc66(params: &MulticellParams, text: &str) -> Option<String> {
    if !is_safe_payload(text) {
        return None;
    }
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
    Some(format!("\x1b]66;{meta};{text}\x07"))
}

/// Writes a multicell origin `symbol` (`symbol_width` cells wide at normal
/// size) to `out`: its OSC 66 escape, or for an unsafe payload as many plain
/// spaces as the block has columns, so the cursor tracking stays right and no
/// control character reaches the terminal.
pub fn write_origin(
    out: &mut impl std::io::Write,
    params: &MulticellParams,
    symbol: &str,
    symbol_width: usize,
) -> std::io::Result<()> {
    match osc66(params, symbol) {
        Some(osc) => out.write_all(osc.as_bytes()),
        None => write!(out, "{:width$}", "", width = params.cols(symbol_width)),
    }
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
        || !is_safe_payload(symbol)
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

/// Repairs blocks that later drawing broke, before the buffer is diffed: an
/// origin whose block is not fully covered (a popup drew over part of it) or
/// leaves the buffer becomes a normal cell (blank for packed text), and
/// covered cells without an intact origin become blanks.
pub fn normalize(buffer: &mut Buffer) {
    let width = buffer.area.width as usize;
    let height = buffer.area.height as usize;
    if width == 0 {
        return;
    }
    let mut claimed = vec![false; buffer.content.len()];
    for i in 0..buffer.content.len() {
        let Multicell::Origin(params) = buffer.content[i].multicell else {
            continue;
        };
        let (x, y) = (i % width, i / width);
        let cols = params.cols(buffer.content[i].width());
        let rows = params.rows();
        let intact = x + cols <= width
            && y + rows <= height
            && (y..y + rows).all(|by| {
                (x..x + cols).all(|bx| {
                    let j = by * width + bx;
                    j == i || (buffer.content[j].multicell == Multicell::Covered && !claimed[j])
                })
            });
        if intact {
            for by in y..y + rows {
                for bx in x..x + cols {
                    claimed[by * width + bx] = true;
                }
            }
        } else if params.width == 0 {
            buffer.content[i].multicell = Multicell::None;
        } else {
            buffer.content[i].set_symbol(" ");
        }
    }
    for (cell, claimed) in buffer.content.iter_mut().zip(claimed) {
        if cell.multicell == Multicell::Covered && !claimed {
            cell.set_symbol(" ");
        }
    }
}

/// What to send to the terminal to turn `previous` into `next`.
pub struct BandDiff<'a> {
    /// Rows to erase completely before drawing (sorted).
    pub erase_rows: Vec<u16>,
    /// Cells to draw, row-major; never [`Multicell::Covered`].
    pub updates: Vec<(u16, u16, &'a Cell)>,
}

/// Like [`Buffer::diff`], aware of multicell characters. Rows touched by a
/// multicell in either buffer form bands (origin row plus `scale - 1` rows,
/// overlapping bands merged). A band with any changed cell is erased and
/// redrawn in full; erasing first is required because the terminal blanks or
/// skips over text written into a multicell. Other rows use the plain diff.
pub fn diff<'a>(previous: &Buffer, next: &'a Buffer) -> BandDiff<'a> {
    let width = next.area.width as usize;
    let height = next.area.height as usize;
    if width == 0 || previous.content.len() != next.content.len() {
        return BandDiff {
            erase_rows: Vec::new(),
            updates: previous.diff(next),
        };
    }
    // band_end[y] > y: a band starts at row y and ends before band_end[y]
    let mut band_end = vec![0usize; height];
    let mut in_band = vec![false; height];
    for buffer in [previous, next] {
        for (i, cell) in buffer.content.iter().enumerate() {
            if let Multicell::Origin(params) = cell.multicell {
                let y = i / width;
                let end = (y + params.rows()).min(height);
                band_end[y] = band_end[y].max(end);
                in_band[y..end].iter_mut().for_each(|row| *row = true);
            }
        }
    }
    let differs = |y: usize| {
        previous.content[y * width..(y + 1) * width] != next.content[y * width..(y + 1) * width]
    };
    let mut dirty = vec![false; height];
    loop {
        let mut changed = false;
        for (start, &end) in band_end.iter().enumerate() {
            if end > start && (start..end).any(|row| dirty[row] || differs(row)) {
                for row in &mut dirty[start..end] {
                    changed |= !*row;
                    *row = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    let mut updates: Vec<_> = previous
        .diff(next)
        .into_iter()
        .filter(|&(_, y, _)| !in_band[y as usize])
        .collect();
    let mut erase_rows = Vec::new();
    for (y, _) in dirty.iter().enumerate().filter(|(_, dirty)| **dirty) {
        erase_rows.push(y as u16);
        let mut x = 0;
        while x < width {
            let cell = &next.content[y * width + x];
            x += match cell.multicell {
                Multicell::Covered => 1,
                Multicell::Origin(params) => {
                    updates.push((x as u16, y as u16, cell));
                    params.cols(cell.width())
                }
                Multicell::None => {
                    updates.push((x as u16, y as u16, cell));
                    cell.width().max(1)
                }
            };
        }
    }
    updates.sort_by_key(|&(x, y, _)| (y, x));
    BandDiff {
        erase_rows,
        updates,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::{Buffer, Cell};
    use helix_view::graphics::{Color, Rect, Style};

    #[test]
    fn osc66_omits_zero_keys() {
        assert_eq!(
            osc66(&MulticellParams::scaled(2), "H").as_deref(),
            Some("\x1b]66;s=2;H\x07")
        );
        let packed = MulticellParams {
            scale: 2,
            width: 3,
            numerator: 3,
            denominator: 4,
            valign: 2,
            halign: 0,
        };
        assert_eq!(
            osc66(&packed, "Head").as_deref(),
            Some("\x1b]66;s=2:w=3:n=3:d=4:v=2;Head\x07")
        );
    }

    #[test]
    fn unsafe_payloads_are_refused() {
        for bad in ["a\x1bb", "a\x07b", "a\u{9c}b", "a\x7fb", "a\nb"] {
            assert!(!is_safe_payload(bad), "{bad:?}");
            assert_eq!(osc66(&MulticellParams::scaled(2), bad), None);
            let mut buffer = Buffer::empty(Rect::new(0, 0, 8, 4));
            let before = buffer.clone();
            assert!(!place(
                &mut buffer,
                0,
                0,
                bad,
                MulticellParams::scaled(2),
                Style::default()
            ));
            assert_eq!(buffer, before);
        }
        assert!(is_safe_payload("Head"));
        assert!(is_safe_payload("漢字"));
    }

    #[test]
    fn unsafe_origins_are_written_as_blanks() {
        let packed = MulticellParams {
            scale: 2,
            width: 3,
            numerator: 3,
            denominator: 4,
            valign: 2,
            halign: 0,
        };
        for params in [MulticellParams::scaled(2), packed] {
            for bad in ["a\x1bb", "a\x07b", "a\u{9c}b"] {
                let mut out = Vec::new();
                write_origin(&mut out, &params, bad, bad.width()).unwrap();
                let out = String::from_utf8(out).unwrap();
                assert!(!out.contains("\x1b]66"), "{bad:?}");
                assert!(!out.contains(['\x1b', '\x07', '\u{9c}']), "{bad:?}");
                assert_eq!(out, " ".repeat(params.cols(bad.width())), "{bad:?}");
            }
        }
    }

    #[test]
    fn safe_origins_are_written_as_osc66() {
        let packed = MulticellParams {
            scale: 2,
            width: 3,
            numerator: 3,
            denominator: 4,
            valign: 2,
            halign: 0,
        };
        let mut out = Vec::new();
        write_origin(&mut out, &packed, "Head", 4).unwrap();
        assert_eq!(out, b"\x1b]66;s=2:w=3:n=3:d=4:v=2;Head\x07");
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

    fn s2() -> MulticellParams {
        MulticellParams::scaled(2)
    }

    #[test]
    fn intact_block_is_kept() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 4, 2));
        place(&mut buffer, 0, 0, "a", s2(), Style::default());
        let before = buffer.clone();
        normalize(&mut buffer);
        assert_eq!(buffer, before);
    }

    #[test]
    fn popup_over_lower_row_demotes_origin() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 4, 2));
        place(&mut buffer, 0, 0, "a", s2(), Style::default());
        buffer[(1, 1)].set_symbol("x"); // a popup draws over the block
        normalize(&mut buffer);
        assert_eq!(buffer[(0, 0)].multicell, Multicell::None);
        assert_eq!(
            buffer[(0, 0)].symbol.as_str(),
            "a",
            "an h1 glyph stays, at normal size"
        );
        assert_eq!(buffer[(1, 1)].symbol.as_str(), "x");
        for (x, y) in [(1, 0), (0, 1)] {
            assert_eq!(buffer[(x, y)].multicell, Multicell::None);
            assert_eq!(buffer[(x, y)].symbol.as_str(), " ");
        }
    }

    #[test]
    fn popup_over_origin_blanks_its_block() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 4, 2));
        place(&mut buffer, 0, 0, "a", s2(), Style::default());
        buffer[(0, 0)].set_symbol("p");
        normalize(&mut buffer);
        assert_eq!(buffer[(0, 0)].symbol.as_str(), "p");
        assert!(buffer
            .content
            .iter()
            .all(|c| c.multicell == Multicell::None));
        assert_eq!(buffer[(1, 1)].symbol.as_str(), " ");
    }

    #[test]
    fn origin_cut_by_right_or_bottom_edge_is_demoted() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 3, 2));
        // only possible through direct writes: the block would leave the buffer
        buffer[(2, 0)].set_symbol("a");
        buffer[(2, 0)].multicell = Multicell::Origin(s2());
        buffer[(2, 1)].set_symbol("b");
        buffer[(2, 1)].multicell = Multicell::Origin(s2());
        normalize(&mut buffer);
        assert!(buffer
            .content
            .iter()
            .all(|c| c.multicell == Multicell::None));
    }

    #[test]
    fn demoted_packed_origin_is_blank() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 8, 2));
        let packed = MulticellParams {
            width: 3,
            numerator: 3,
            denominator: 4,
            valign: 2,
            ..s2()
        };
        place(&mut buffer, 0, 0, "Head", packed, Style::default());
        buffer[(5, 1)].set_symbol("x");
        normalize(&mut buffer);
        // several graphemes in one cell would overflow it at normal size
        assert_eq!(buffer[(0, 0)].symbol.as_str(), " ");
    }

    fn grid() -> Buffer {
        Buffer::empty(Rect::new(0, 0, 8, 6))
    }

    #[test]
    fn unchanged_band_is_not_emitted() {
        let mut prev = grid();
        place(&mut prev, 0, 0, "a", s2(), Style::default());
        let next = prev.clone();
        let d = diff(&prev, &next);
        assert!(d.erase_rows.is_empty());
        assert!(d.updates.is_empty());
    }

    #[test]
    fn change_in_lower_row_redraws_the_band() {
        let mut prev = grid();
        place(&mut prev, 0, 0, "a", s2(), Style::default());
        let mut next = prev.clone();
        next[(5, 1)].set_symbol("z");
        let d = diff(&prev, &next);
        assert_eq!(d.erase_rows, [0, 1]);
        let at: Vec<_> = d.updates.iter().map(|&(x, y, _)| (x, y)).collect();
        assert!(at.contains(&(0, 0)), "origin re-emitted");
        assert!(at.contains(&(5, 1)));
        assert!(!at.contains(&(1, 0)) && !at.contains(&(0, 1)) && !at.contains(&(1, 1)));
    }

    #[test]
    fn covered_cells_are_never_emitted() {
        let prev = grid();
        let mut next = grid();
        place(&mut next, 2, 1, "a", s2(), Style::default());
        let d = diff(&prev, &next);
        assert_eq!(d.erase_rows, [1, 2]);
        assert!(d
            .updates
            .iter()
            .all(|(_, _, c)| c.multicell != Multicell::Covered));
        assert!(
            d.updates
                .windows(2)
                .all(|w| (w[0].1, w[0].0) < (w[1].1, w[1].0)),
            "row-major"
        );
    }

    #[test]
    fn removed_multicell_erases_its_rows() {
        let mut prev = grid();
        place(&mut prev, 0, 0, "a", s2(), Style::default());
        let next = grid();
        let d = diff(&prev, &next);
        assert_eq!(d.erase_rows, [0, 1]);
        assert_eq!(d.updates.len(), 16, "both rows redrawn in full");
    }

    #[test]
    fn overlapping_bands_merge() {
        let mut prev = grid();
        place(&mut prev, 0, 0, "a", s2(), Style::default());
        place(&mut prev, 4, 1, "b", s2(), Style::default());
        let mut next = prev.clone();
        next[(7, 2)].set_symbol("z");
        assert_eq!(diff(&prev, &next).erase_rows, [0, 1, 2]);
    }

    #[test]
    fn rows_outside_bands_use_the_plain_diff() {
        let mut prev = grid();
        place(&mut prev, 0, 0, "a", s2(), Style::default());
        let mut next = prev.clone();
        next[(3, 4)].set_symbol("z");
        let d = diff(&prev, &next);
        assert!(d.erase_rows.is_empty());
        let at: Vec<_> = d.updates.iter().map(|&(x, y, _)| (x, y)).collect();
        assert_eq!(at, [(3, 4)]);
    }
}
