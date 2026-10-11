//! Terminal images via the Kitty graphics protocol with Unicode placeholders.

pub mod detect;
pub mod diagram;
pub mod layout;
pub mod placeholder;
pub mod protocol;

/// What the terminal supports, see [`detect`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphicsCaps {
    pub supported: bool,
    pub tmux: bool,
    /// Size of one cell in pixels.
    pub cell_px: (u16, u16),
    /// Size of the terminal in cells.
    pub term_cells: (u16, u16),
}

impl Default for GraphicsCaps {
    fn default() -> Self {
        Self {
            supported: false,
            tmux: false,
            cell_px: (10, 20),
            term_cells: (80, 24),
        }
    }
}

pub mod store;
pub use store::{ImageState, ImageStore};
