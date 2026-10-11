//! Kitty text sizing protocol (OSC 66) support and the `text-sizing` setting.

use serde::{Deserialize, Serialize};

use crate::{editor::Config, Editor};

/// `[editor] text-sizing`: draw large Markdown headings with OSC 66.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Setting {
    /// On when the startup probe reports scale support.
    #[default]
    Auto,
    On,
    Off,
}

/// The probe result and whether text sizing is on.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct State {
    pub support: Support,
    pub enabled: bool,
}

pub fn resolve(setting: Setting, support: Support) -> bool {
    match setting {
        Setting::Auto => support == Support::Scale,
        Setting::On => true,
        Setting::Off => false,
    }
}

/// Stores the probed `support` and resolves the editor's flag from the
/// `text-sizing` setting.
pub fn apply(editor: &mut Editor, support: Support) {
    let state = State {
        support,
        enabled: resolve(editor.config().text_sizing, support),
    };
    if editor.text_sizing != state {
        editor.text_sizing = state;
        helix_event::request_redraw();
    }
}

/// Resolves the flag again after a config reload changed `text-sizing`.
pub fn reconfigure(editor: &mut Editor, old: &Config) {
    if editor.config().text_sizing != old.text_sizing {
        let support = editor.text_sizing.support;
        apply(editor, support);
    }
}

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

    #[test]
    fn resolve_setting_with_support() {
        assert!(resolve(Setting::Auto, Support::Scale));
        assert!(!resolve(Setting::Auto, Support::WidthOnly));
        assert!(!resolve(Setting::Auto, Support::None));
        assert!(resolve(Setting::On, Support::None));
        assert!(!resolve(Setting::Off, Support::Scale));
    }

    #[test]
    fn setting_from_config() {
        let config: crate::editor::Config = toml::from_str("text-sizing = \"on\"").unwrap();
        assert_eq!(config.text_sizing, Setting::On);
        assert_eq!(crate::editor::Config::default().text_sizing, Setting::Auto);
        assert!(toml::from_str::<crate::editor::Config>("text-sizing = \"big\"").is_err());
    }
}
