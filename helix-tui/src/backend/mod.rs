//! Provides interface for controlling the terminal

use std::io;

use crate::{buffer::Cell, terminal::Config};

use helix_view::{
    graphics::{CursorKind, Rect},
    theme::Color,
};

/// Writes a cell's symbol with every control character replaced by a
/// printable stand-in: symbols come from documents, file names, language
/// servers and more, and a raw ESC or BEL would let them inject terminal
/// escape sequences.
pub(crate) fn write_symbol(writer: &mut impl io::Write, symbol: &str) -> io::Result<()> {
    writer.write_all(helix_core::control_chars::sanitize(symbol).as_bytes())
}

#[cfg(all(feature = "termina", not(windows)))]
mod termina;
#[cfg(all(feature = "termina", not(windows)))]
pub use self::termina::TerminaBackend;

#[cfg(all(feature = "termina", windows))]
mod crossterm;
#[cfg(all(feature = "termina", windows))]
pub use self::crossterm::CrosstermBackend;

mod test;
pub use self::test::TestBackend;

/// Representation of a terminal backend.
pub trait Backend {
    /// Claims the terminal for TUI use.
    fn claim(&mut self) -> Result<(), io::Error>;
    /// Update terminal configuration.
    fn reconfigure(&mut self, config: Config) -> Result<(), io::Error>;
    /// Restores the terminal to a normal state, undoes `claim`
    fn restore(&mut self) -> Result<(), io::Error>;
    /// Draws styled text to the terminal
    fn draw<'a, I>(&mut self, content: I) -> Result<(), io::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>;
    /// Hides the cursor
    fn hide_cursor(&mut self) -> Result<(), io::Error>;
    /// Sets the cursor to the given shape
    fn show_cursor(&mut self, kind: CursorKind) -> Result<(), io::Error>;
    /// Sets the cursor to the given position
    fn set_cursor(&mut self, x: u16, y: u16) -> Result<(), io::Error>;
    /// Clears the terminal
    fn clear(&mut self) -> Result<(), io::Error>;
    /// Erases the given zero-based rows completely; every multicell character
    /// intersecting an erased row disappears with it.
    fn erase_rows(&mut self, rows: &[u16]) -> Result<(), io::Error>;
    /// Begins a synchronized-output frame (if the terminal supports it), so the
    /// draw and cursor updates between `start_sync` and `end_sync` present as one
    /// frame instead of flickering.
    fn start_sync(&mut self) -> Result<(), io::Error>;
    /// Ends the synchronized-output frame opened by `start_sync`.
    fn end_sync(&mut self) -> Result<(), io::Error>;
    /// Gets the size of the terminal in cells
    fn size(&self) -> Result<Rect, io::Error>;
    /// Flushes the terminal buffer
    fn flush(&mut self) -> Result<(), io::Error>;
    fn supports_true_color(&self) -> bool;
    fn get_theme_mode(&self) -> Option<helix_view::theme::Mode>;
    fn set_background_color(&mut self, color: Option<Color>) -> io::Result<()>;
    /// Writes raw terminal graphics escape sequences (Kitty graphics protocol).
    fn write_graphics(&mut self, _bytes: &[u8]) -> Result<(), io::Error> {
        Ok(())
    }
    /// Size of one cell in pixels, if the platform reports it.
    fn cell_pixel_size(&self) -> Option<(u16, u16)> {
        None
    }
    /// Kitty text sizing (OSC 66) support detected when claiming the terminal.
    fn text_sizing(&self) -> helix_view::text_sizing::Support {
        helix_view::text_sizing::Support::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_symbol_never_writes_raw_control_characters() {
        let mut out = Vec::new();
        for symbol in [
            "\x1b", "]", "2", ";", "X", "\x07", "\u{9c}", "\u{9b}", "\x7f", "é",
        ] {
            write_symbol(&mut out, symbol).unwrap();
        }
        let out = String::from_utf8(out).unwrap();
        assert_eq!(out, "␛]2;X␇\u{fffd}\u{fffd}␡é");
        assert!(!out.contains(char::is_control));
    }
}
