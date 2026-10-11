//! The fork's default theme: `catppuccin_mocha` when the config sets no
//! `theme`. `theme = "default"` still names helix's built-in theme.

use crate::theme::{Config, Loader, Mode, Theme};

/// The theme used when the config has no `theme` key.
pub const NAME: &str = "catppuccin_mocha";

/// The theme for the config's `theme` key, or [`NAME`] when it is unset.
///
/// A theme that fails to load, or that needs true colour without
/// `true_color`, falls back to the built-in default (`base16_default`
/// without true colour).
pub fn configured(
    loader: &Loader,
    config: Option<&Config>,
    mode: Option<Mode>,
    true_color: bool,
) -> Theme {
    let name = config.map_or(NAME, |config| config.choose(mode));
    loader
        .load(name)
        .map_err(|err| log::warn!("failed to load theme `{name}` - {err}"))
        .ok()
        .filter(|theme| {
            let colors_ok = true_color || theme.is_16_color();
            if !colors_ok {
                log::warn!(
                    "loaded theme `{name}` but cannot use it because true color \
                    support is not enabled"
                );
            }
            colors_ok
        })
        .unwrap_or_else(|| loader.default_theme(true_color))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;

    fn loader() -> Loader {
        Loader::new(&[Path::new(env!("CARGO_MANIFEST_DIR")).join("../runtime")])
    }

    fn constant(name: &str) -> Config {
        Config::Constant(name.into())
    }

    #[test]
    fn unset_is_catppuccin_mocha() {
        assert_eq!(configured(&loader(), None, None, true).name(), NAME);
    }

    #[test]
    fn default_is_the_built_in_theme() {
        let theme = configured(&loader(), Some(&constant("default")), None, true);
        assert_eq!(theme.name(), "default");
        assert_eq!(
            theme.get("ui.background"),
            crate::theme::DEFAULT_THEME.get("ui.background")
        );
    }

    #[test]
    fn explicit_theme_is_unchanged() {
        let theme = configured(&loader(), Some(&constant("solarized_light")), None, true);
        assert_eq!(theme.name(), "solarized_light");
    }

    #[test]
    fn adaptive_theme_follows_the_mode() {
        let config = Config::Adaptive {
            light: "solarized_light".into(),
            dark: "gruvbox".into(),
            fallback: None,
        };
        let theme = configured(&loader(), Some(&config), Some(Mode::Light), true);
        assert_eq!(theme.name(), "solarized_light");
    }

    #[test]
    fn unset_without_true_color_is_base16_default() {
        assert_eq!(
            configured(&loader(), None, None, false).name(),
            "base16_default"
        );
    }

    #[test]
    fn unset_without_runtime_is_the_built_in_theme() {
        let loader = Loader::new(&[PathBuf::from("/nonexistent-helix-runtime")]);
        assert_eq!(configured(&loader, None, None, true).name(), "default");
    }

    #[test]
    fn missing_explicit_theme_is_the_built_in_theme() {
        let theme = configured(&loader(), Some(&constant("no_such_theme")), None, true);
        assert_eq!(theme.name(), "default");
    }
}
