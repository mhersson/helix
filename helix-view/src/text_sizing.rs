//! Kitty text sizing protocol (OSC 66) support and the `text-sizing` setting.

/// What the terminal does with OSC 66, from the startup probe.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Support {
    /// Not supported, not probed, or the probe got no full answer.
    #[default]
    None,
    /// Only the `w` (width) key: text is not scaled (foot). Treated as
    /// unsupported, since scaled text would be drawn at normal size.
    WidthOnly,
    /// Width and scale (kitty).
    Scale,
}

impl Support {
    /// Classifies the cursor columns reported by the three CPRs of the probe
    /// `CR, CPR, OSC 66 w=2 " ", CPR, OSC 66 s=2 " ", CPR`.
    pub fn from_probe_columns(columns: &[u16]) -> Self {
        match *columns {
            [a, b, c] if b == a + 2 && c == b + 2 => Self::Scale,
            [a, b, _] if b == a + 2 => Self::WidthOnly,
            _ => Self::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_columns() {
        // CR, CPR, w=2 space, CPR, s=2 space, CPR (spike: kitty, foot, Ghostty/tmux)
        assert_eq!(Support::from_probe_columns(&[1, 3, 5]), Support::Scale);
        assert_eq!(Support::from_probe_columns(&[1, 3, 4]), Support::WidthOnly);
        assert_eq!(Support::from_probe_columns(&[1, 1, 1]), Support::None);
    }

    #[test]
    fn partial_probe_is_none() {
        assert_eq!(Support::from_probe_columns(&[]), Support::None);
        assert_eq!(Support::from_probe_columns(&[1, 3]), Support::None);
        assert_eq!(Support::from_probe_columns(&[1, 3, 5, 7]), Support::None);
    }
}
