use super::GraphicsCaps;
use crate::editor::KittyGraphics;

/// The capabilities of the terminal Helix runs in, detected from the
/// environment. `cell_px` falls back to 10x20 when the platform reports none.
pub fn caps(
    setting: KittyGraphics,
    true_color: bool,
    cell_px: Option<(u16, u16)>,
    term_cells: (u16, u16),
) -> GraphicsCaps {
    GraphicsCaps {
        supported: supported(
            setting,
            true_color,
            &|key| std::env::var(key).ok(),
            &tmux_client_termname,
        ),
        tmux: std::env::var_os("TMUX").is_some(),
        cell_px: cell_px.unwrap_or(GraphicsCaps::default().cell_px),
        term_cells,
    }
}

/// Whether the terminal shows Kitty graphics. `env` reads environment
/// variables; `tmux_client_termname` is only called inside tmux.
pub fn supported(
    setting: KittyGraphics,
    true_color: bool,
    env: &dyn Fn(&str) -> Option<String>,
    tmux_client_termname: &dyn Fn() -> Option<String>,
) -> bool {
    match setting {
        KittyGraphics::Off => false,
        KittyGraphics::On => true,
        KittyGraphics::Auto if !true_color => false,
        KittyGraphics::Auto if env("TMUX").is_some() => tmux_client_termname()
            .is_some_and(|name| name.contains("kitty") || name.contains("ghostty")),
        KittyGraphics::Auto => {
            let term = env("TERM").unwrap_or_default();
            // GNU screen shows APC sequences as its hardstatus line
            !term.starts_with("screen")
                && (term == "xterm-kitty"
                    || term == "xterm-ghostty"
                    || env("KITTY_WINDOW_ID").is_some()
                    || env("TERM_PROGRAM").as_deref() == Some("ghostty")
                    || env("GHOSTTY_RESOURCES_DIR").is_some())
        }
    }
}

/// The outer terminal's name as seen by the tmux client.
pub fn tmux_client_termname() -> Option<String> {
    let output = std::process::Command::new("tmux")
        .args(["display", "-p", "#{client_termname}"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::editor::KittyGraphics;

    fn env(vars: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |key| {
            vars.iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.to_string())
        }
    }
    fn no_tmux() -> Option<String> {
        panic!("tmux must not be queried outside tmux")
    }

    #[test]
    fn kitty_and_ghostty() {
        let cases: [&'static [(&'static str, &'static str)]; 5] = [
            &[("TERM", "xterm-kitty")],
            &[("KITTY_WINDOW_ID", "1")],
            &[("TERM_PROGRAM", "ghostty")],
            &[("TERM", "xterm-ghostty")],
            &[("GHOSTTY_RESOURCES_DIR", "/usr/share/ghostty")],
        ];
        for vars in cases {
            assert!(
                supported(KittyGraphics::Auto, true, &env(vars), &no_tmux),
                "{vars:?}"
            );
        }
        assert!(!supported(
            KittyGraphics::Auto,
            true,
            &env(&[("TERM", "xterm-256color")]),
            &no_tmux
        ));
    }

    #[test]
    fn gnu_screen_is_unsupported() {
        // inherited from a kitty shell; screen would show APC as hardstatus
        let vars = &[("TERM", "screen.xterm-256color"), ("KITTY_WINDOW_ID", "1")];
        assert!(!supported(KittyGraphics::Auto, true, &env(vars), &no_tmux));
        assert!(supported(KittyGraphics::On, true, &env(vars), &no_tmux));
    }

    #[test]
    fn requires_true_colour() {
        assert!(!supported(
            KittyGraphics::Auto,
            false,
            &env(&[("TERM", "xterm-kitty")]),
            &no_tmux
        ));
    }

    #[test]
    fn tmux_uses_client_termname() {
        let vars = &[
            ("TMUX", "/tmp/tmux-1000/default,1,0"),
            ("KITTY_WINDOW_ID", "1"),
        ];
        assert!(supported(KittyGraphics::Auto, true, &env(vars), &|| Some(
            "xterm-ghostty".into()
        )));
        // a stale KITTY_WINDOW_ID from the tmux server environment is ignored
        assert!(!supported(KittyGraphics::Auto, true, &env(vars), &|| Some(
            "xterm-256color".into()
        )));
        assert!(!supported(KittyGraphics::Auto, true, &env(vars), &|| None));
    }

    #[test]
    fn overrides() {
        assert!(supported(KittyGraphics::On, false, &env(&[]), &no_tmux));
        assert!(!supported(
            KittyGraphics::Off,
            true,
            &env(&[("TERM", "xterm-kitty")]),
            &no_tmux
        ));
    }
}
