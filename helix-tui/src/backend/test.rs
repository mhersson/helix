use crate::{
    backend::Backend,
    buffer::{Buffer, Cell},
    terminal::Config,
};
use helix_core::unicode::width::UnicodeWidthStr;
use helix_view::graphics::{CursorKind, Rect};
use std::{fmt::Write, io};

/// A backend used for the integration tests.
#[derive(Debug)]
pub struct TestBackend {
    width: u16,
    buffer: Buffer,
    height: u16,
    cursor: bool,
    pos: (u16, u16),
    graphics: Vec<u8>,
    cell_px: Option<(u16, u16)>,
}

/// Returns a string representation of the given buffer for debugging purpose.
fn buffer_view(buffer: &Buffer) -> String {
    let mut view = String::with_capacity(buffer.content.len() + buffer.area.height as usize * 3);
    for cells in buffer.content.chunks(buffer.area.width as usize) {
        let mut overwritten = vec![];
        let mut skip: usize = 0;
        view.push('"');
        for (x, c) in cells.iter().enumerate() {
            if skip == 0 {
                view.push_str(&c.symbol);
            } else {
                overwritten.push((x, &c.symbol))
            }
            skip = std::cmp::max(skip, c.symbol.width()).saturating_sub(1);
        }
        view.push('"');
        if !overwritten.is_empty() {
            write!(
                &mut view,
                " Hidden by multi-width symbols: {:?}",
                overwritten
            )
            .unwrap();
        }
        view.push('\n');
    }
    view
}

impl TestBackend {
    pub fn new(width: u16, height: u16) -> TestBackend {
        TestBackend {
            width,
            height,
            buffer: Buffer::empty(Rect::new(0, 0, width, height)),
            cursor: false,
            pos: (0, 0),
            graphics: Vec::new(),
            cell_px: None,
        }
    }

    /// All bytes passed to `write_graphics`.
    pub fn graphics(&self) -> &[u8] {
        &self.graphics
    }

    pub fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    pub fn resize(&mut self, width: u16, height: u16) {
        self.buffer.resize(Rect::new(0, 0, width, height));
        self.width = width;
        self.height = height;
    }

    pub fn assert_buffer(&self, expected: &Buffer) {
        assert_eq!(expected.area, self.buffer.area);
        let diff = expected.diff(&self.buffer);
        if diff.is_empty() {
            return;
        }

        let mut debug_info = String::from("Buffers are not equal");
        debug_info.push('\n');
        debug_info.push_str("Expected:");
        debug_info.push('\n');
        let expected_view = buffer_view(expected);
        debug_info.push_str(&expected_view);
        debug_info.push('\n');
        debug_info.push_str("Got:");
        debug_info.push('\n');
        let view = buffer_view(&self.buffer);
        debug_info.push_str(&view);
        debug_info.push('\n');

        debug_info.push_str("Diff:");
        debug_info.push('\n');
        let nice_diff = diff
            .iter()
            .enumerate()
            .map(|(i, (x, y, cell))| {
                let expected_cell = expected.get(*x, *y);
                format!(
                    "{}: at ({}, {}) expected {:?} got {:?}",
                    i, x, y, expected_cell, cell
                )
            })
            .collect::<Vec<String>>()
            .join("\n");
        debug_info.push_str(&nice_diff);
        panic!("{}", debug_info);
    }
}

impl Backend for TestBackend {
    fn claim(&mut self) -> Result<(), io::Error> {
        Ok(())
    }

    fn reconfigure(&mut self, _config: Config) -> Result<(), io::Error> {
        Ok(())
    }

    fn restore(&mut self) -> Result<(), io::Error> {
        Ok(())
    }

    fn draw<'a, I>(&mut self, content: I) -> Result<(), io::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        use crate::multicell::Multicell;
        for (x, y, c) in content {
            self.buffer[(x, y)] = c.clone();
            if let Multicell::Origin(params) = c.multicell {
                let area = self.buffer.area;
                let right = (x + params.cols(c.width()) as u16).min(area.right());
                let bottom = (y + params.rows() as u16).min(area.bottom());
                for by in y..bottom {
                    for bx in x..right {
                        if (bx, by) != (x, y) {
                            let cell = &mut self.buffer[(bx, by)];
                            cell.reset();
                            cell.set_style(c.style());
                            cell.multicell = Multicell::Covered;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn erase_rows(&mut self, rows: &[u16]) -> Result<(), io::Error> {
        use crate::multicell::Multicell;
        let width = self.buffer.area.width as usize;
        // blocks intersecting an erased row disappear entirely
        let origins: Vec<(usize, usize, usize, usize)> = self
            .buffer
            .content
            .iter()
            .enumerate()
            .filter_map(|(i, c)| match c.multicell {
                Multicell::Origin(p) => Some((i % width, i / width, p.cols(c.width()), p.rows())),
                _ => None,
            })
            .collect();
        for (x, y, cols, rows_n) in origins {
            if rows
                .iter()
                .any(|&r| (y..y + rows_n).contains(&(r as usize)))
            {
                for by in y..(y + rows_n).min(self.buffer.area.height as usize) {
                    for bx in x..(x + cols).min(width) {
                        self.buffer.content[by * width + bx].reset();
                    }
                }
            }
        }
        for &row in rows {
            for x in 0..self.buffer.area.width {
                self.buffer[(x, row)].reset();
            }
        }
        Ok(())
    }

    fn hide_cursor(&mut self) -> Result<(), io::Error> {
        self.cursor = false;
        Ok(())
    }

    fn show_cursor(&mut self, _kind: CursorKind) -> Result<(), io::Error> {
        self.cursor = true;
        Ok(())
    }

    fn set_cursor(&mut self, x: u16, y: u16) -> Result<(), io::Error> {
        self.pos = (x, y);
        Ok(())
    }

    fn clear(&mut self) -> Result<(), io::Error> {
        self.buffer.reset();
        Ok(())
    }

    fn start_sync(&mut self) -> Result<(), io::Error> {
        Ok(())
    }

    fn end_sync(&mut self) -> Result<(), io::Error> {
        Ok(())
    }

    fn size(&self) -> Result<Rect, io::Error> {
        Ok(Rect::new(0, 0, self.width, self.height))
    }

    fn flush(&mut self) -> Result<(), io::Error> {
        Ok(())
    }

    fn supports_true_color(&self) -> bool {
        false
    }

    fn get_theme_mode(&self) -> Option<helix_view::theme::Mode> {
        None
    }

    fn set_background_color(&mut self, _color: Option<helix_view::theme::Color>) -> io::Result<()> {
        Ok(())
    }

    fn write_graphics(&mut self, bytes: &[u8]) -> Result<(), io::Error> {
        self.graphics.extend_from_slice(bytes);
        Ok(())
    }

    fn cell_pixel_size(&self) -> Option<(u16, u16)> {
        self.cell_px
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multicell::{place, Multicell, MulticellParams};
    use crate::terminal::Terminal;
    use helix_view::graphics::Style;

    #[test]
    fn draw_marks_the_block_of_an_origin() {
        let mut backend = TestBackend::new(6, 3);
        let mut cell = Cell::default();
        cell.set_symbol("a");
        cell.multicell = Multicell::Origin(MulticellParams::scaled(2));
        backend.draw(std::iter::once((1, 0, &cell))).unwrap();
        let buffer = backend.buffer();
        assert_eq!(buffer[(1, 0)].symbol.as_str(), "a");
        for (x, y) in [(2, 0), (1, 1), (2, 1)] {
            assert_eq!(buffer[(x, y)].multicell, Multicell::Covered);
        }
    }

    #[test]
    fn erase_rows_removes_intersecting_blocks() {
        let mut backend = TestBackend::new(6, 3);
        let mut cell = Cell::default();
        cell.set_symbol("a");
        cell.multicell = Multicell::Origin(MulticellParams::scaled(2));
        backend.draw(std::iter::once((0, 0, &cell))).unwrap();
        backend.erase_rows(&[1]).unwrap();
        assert!(backend
            .buffer()
            .content
            .iter()
            .all(|c| c.multicell == Multicell::None));
        assert_eq!(backend.buffer()[(0, 0)].symbol.as_str(), " ");
    }

    #[test]
    fn flush_erases_before_drawing() {
        let mut terminal = Terminal::new(TestBackend::new(8, 4)).unwrap();
        place(
            terminal.current_buffer_mut(),
            0,
            0,
            "a",
            MulticellParams::scaled(2),
            Style::default(),
        );
        terminal
            .draw(None, helix_view::graphics::CursorKind::Hidden)
            .unwrap();
        // next frame: same heading, one changed cell in its lower row
        place(
            terminal.current_buffer_mut(),
            0,
            0,
            "a",
            MulticellParams::scaled(2),
            Style::default(),
        );
        terminal.current_buffer_mut()[(6, 1)].set_symbol("z");
        terminal
            .draw(None, helix_view::graphics::CursorKind::Hidden)
            .unwrap();
        let buffer = terminal.backend().buffer();
        // drawing before erasing would have wiped the origin again
        assert_eq!(buffer[(0, 0)].symbol.as_str(), "a");
        assert!(matches!(buffer[(0, 0)].multicell, Multicell::Origin(_)));
        assert_eq!(buffer[(6, 1)].symbol.as_str(), "z");
    }
}
