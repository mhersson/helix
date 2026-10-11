//! Mermaid diagrams rendered to images with merman and resvg, see [`render`].

use std::borrow::Cow;
use std::cell::Cell;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, OnceLock};

use image::RgbaImage;
use resvg::{tiny_skia, usvg};

use crate::graphics::Color;
use crate::theme::Mode;

/// Mermaid's base font size in pixels: rendered at `cell height / 16`,
/// diagram text is about as tall as editor text.
const BASE_FONT_PX: f32 = 16.0;

/// `source` with a Mermaid theme directive for the editor's appearance,
/// unless the diagram configures itself with `%%{init ...}%%` or YAML
/// frontmatter (which must stay on the first line).
pub fn with_theme(source: &str, dark: bool) -> Cow<'_, str> {
    let first = source
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    if first.starts_with("%%{init") || first == "---" {
        return Cow::Borrowed(source);
    }
    let theme = if dark { "dark" } else { "default" };
    Cow::Owned(format!(
        "%%{{init: {{\"theme\": \"{theme}\"}}}}%%\n{source}"
    ))
}

/// Whether diagrams use Mermaid's dark theme: from the editor background's
/// luminance, else the terminal's reported mode, else dark.
pub fn is_dark(background: Option<Color>, terminal: Option<Mode>) -> bool {
    match background.and_then(rgb) {
        // the luminance at which black and white text contrast equally
        Some(rgb) => luminance(rgb) < 0.179,
        None => terminal != Some(Mode::Light),
    }
}

fn luminance((r, g, b): (u8, u8, u8)) -> f32 {
    let linear = |c: u8| {
        let c = c as f32 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

/// xterm's default palette.
fn rgb(color: Color) -> Option<(u8, u8, u8)> {
    const ANSI: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (205, 0, 0),
        (0, 205, 0),
        (205, 205, 0),
        (0, 0, 238),
        (205, 0, 205),
        (0, 205, 205),
        (229, 229, 229),
        (127, 127, 127),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (92, 92, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];
    let index = match color {
        Color::Reset => return None,
        Color::Rgb(r, g, b) => return Some((r, g, b)),
        Color::Black => 0,
        Color::Red => 1,
        Color::Green => 2,
        Color::Yellow => 3,
        Color::Blue => 4,
        Color::Magenta => 5,
        Color::Cyan => 6,
        Color::LightGray => 7,
        Color::Gray => 8,
        Color::LightRed => 9,
        Color::LightGreen => 10,
        Color::LightYellow => 11,
        Color::LightBlue => 12,
        Color::LightMagenta => 13,
        Color::LightCyan => 14,
        Color::White => 15,
        Color::Indexed(i) => i,
    };
    Some(match index {
        0..=15 => ANSI[index as usize],
        16..=231 => {
            let level = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
            let i = index - 16;
            (level(i / 36), level(i / 6 % 6), level(i % 6))
        }
        _ => {
            let v = 8 + 10 * (index - 232);
            (v, v, v)
        }
    })
}

/// Renders a Mermaid diagram with its text about one cell tall, scaled down
/// (keeping the aspect ratio) to fit `max_px`. The error is one line.
pub fn render(
    source: &str,
    dark: bool,
    cell_px: (u16, u16),
    max_px: (u32, u32),
) -> Result<RgbaImage, String> {
    let source = with_theme(source, dark);
    guarded(|| {
        let svg = svg(&source)?;
        rasterize(&svg, cell_px.1.max(1) as f32 / BASE_FONT_PX, max_px)
    })
}

thread_local! {
    /// Set while this thread runs the diagram renderer, see [`quiet_panic_hook`].
    static RENDERING: Cell<bool> = const { Cell::new(false) };
}

/// Runs the renderer, turning a panic into a one-line error.
fn guarded<T>(render: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    struct Reset(bool);
    impl Drop for Reset {
        fn drop(&mut self) {
            RENDERING.with(|rendering| rendering.set(self.0));
        }
    }
    let _reset = Reset(RENDERING.with(|rendering| rendering.replace(true)));
    std::panic::catch_unwind(AssertUnwindSafe(render))
        .unwrap_or_else(|_| Err("the diagram renderer crashed".to_string()))
}

/// Wraps the current panic hook so that a panic inside the diagram renderer,
/// which [`render`] catches, is only logged. The terminal backend's hook
/// would otherwise restore the terminal while Helix keeps running. Install
/// it after the backend's hook.
pub fn quiet_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if RENDERING.with(Cell::get) {
            log::error!("the diagram renderer panicked: {info}");
        } else {
            previous(info);
        }
    }));
}

fn first_line(message: &str) -> String {
    message.lines().next().unwrap_or("").trim().to_string()
}

fn svg(source: &str) -> Result<String, String> {
    use merman::{OperationControl, RenderOutput, RenderRequest, Renderer, SvgRequest};

    let request = SvgRequest {
        pipeline: Some(merman::svg::SvgPipeline::resvg_safe()),
        ..SvgRequest::default()
    };
    match Renderer::new().render(RenderRequest::svg(source, OperationControl::new(), request)) {
        Ok(RenderOutput::Svg(Some(output))) => Ok(output.into_parts().0),
        Ok(_) => Err("no Mermaid diagram detected".to_string()),
        Err(err) => Err(first_line(&err.to_string())),
    }
}

/// System fonts, loaded once on first use and shared by all renders.
fn fontdb() -> Arc<usvg::fontdb::Database> {
    static FONTDB: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    FONTDB
        .get_or_init(|| {
            let mut db = usvg::fontdb::Database::new();
            db.load_system_fonts();
            Arc::new(db)
        })
        .clone()
}

fn rasterize(svg: &str, scale: f32, max_px: (u32, u32)) -> Result<RgbaImage, String> {
    let options = usvg::Options {
        fontdb: fontdb(),
        font_family: "sans-serif".to_string(),
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_str(svg, &options).map_err(|err| first_line(&err.to_string()))?;
    let size = tree.size();
    let (max_w, max_h) = (max_px.0.max(1) as f32, max_px.1.max(1) as f32);
    let scale = scale.min(max_w / size.width()).min(max_h / size.height());
    let width = ((size.width() * scale).round() as u32).clamp(1, max_px.0.max(1));
    let height = ((size.height() * scale).round() as u32).clamp(1, max_px.1.max(1));
    let mut pixmap =
        tiny_skia::Pixmap::new(width, height).ok_or_else(|| "diagram too large".to_string())?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    // tiny-skia is premultiplied; Kitty expects straight alpha
    let rgba = pixmap
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let color = pixel.demultiply();
            [color.red(), color.green(), color.blue(), color.alpha()]
        })
        .collect();
    RgbaImage::from_raw(width, height, rgba).ok_or_else(|| "invalid diagram size".to_string())
}

#[cfg(test)]
mod test {
    use super::*;

    const FLOWCHART: &str = "flowchart TD\n  A[Start] --> B{Ok?}\n  B -->|Yes| C[Done]\n";
    /// Rejected by merman: an edge without a target.
    const INVALID: &str = "flowchart TD\n  A -->\n";

    fn big() -> (u32, u32) {
        (4000, 4000)
    }

    #[test]
    fn injects_the_editor_theme() {
        assert_eq!(
            with_theme("flowchart TD\n", true),
            "%%{init: {\"theme\": \"dark\"}}%%\nflowchart TD\n"
        );
        assert_eq!(
            with_theme("flowchart TD\n", false),
            "%%{init: {\"theme\": \"default\"}}%%\nflowchart TD\n"
        );
    }

    #[test]
    fn own_init_is_respected() {
        let source = "\n  %%{init: {\"theme\": \"forest\"}}%%\nflowchart TD\n";
        assert_eq!(with_theme(source, true), source);
    }

    #[test]
    fn a_panic_is_an_error_and_clears_the_flag() {
        let result: Result<(), String> = guarded(|| panic!("boom"));
        assert_eq!(result.unwrap_err(), "the diagram renderer crashed");
        assert!(!RENDERING.with(Cell::get));
        assert_eq!(guarded(|| Ok(RENDERING.with(Cell::get))), Ok(true));
        assert!(!RENDERING.with(Cell::get));
    }

    #[test]
    fn quiet_hook_only_silences_renders() {
        thread_local! {
            static CALLS: Cell<u32> = const { Cell::new(0) };
        }
        let original = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| CALLS.with(|c| c.set(c.get() + 1))));
        quiet_panic_hook();
        let _ = guarded::<()>(|| panic!("inside a render"));
        let inside = CALLS.with(Cell::get);
        let _ = std::panic::catch_unwind(|| panic!("outside a render"));
        let outside = CALLS.with(Cell::get);
        drop(std::panic::take_hook());
        std::panic::set_hook(original);
        assert_eq!((inside, outside), (0, 1));
    }

    #[test]
    fn frontmatter_is_not_touched() {
        let source = "---\nconfig:\n  theme: forest\n---\nflowchart TD\n  A --> B\n";
        assert_eq!(with_theme(source, true), source);
    }

    #[test]
    fn renders_a_flowchart() {
        let image = render(FLOWCHART, true, (10, 20), big()).unwrap();
        assert!(image.width() > 10 && image.height() > 10);
        // something was drawn
        assert!(image.pixels().any(|p| p.0[3] > 0));
    }

    #[test]
    fn dark_and_light_differ() {
        let dark = render(FLOWCHART, true, (10, 20), big()).unwrap();
        let light = render(FLOWCHART, false, (10, 20), big()).unwrap();
        assert_ne!(dark.as_raw(), light.as_raw());
    }

    #[test]
    fn frontmatter_theme_wins() {
        let source = format!("---\nconfig:\n  theme: forest\n---\n{FLOWCHART}");
        let dark = render(&source, true, (10, 20), big()).unwrap();
        let light = render(&source, false, (10, 20), big()).unwrap();
        assert_eq!(dark.as_raw(), light.as_raw());
    }

    #[test]
    fn invalid_syntax_is_an_error() {
        let err = render(INVALID, true, (10, 20), big()).unwrap_err();
        assert!(!err.is_empty());
        assert!(!err.contains('\n'), "{err:?}");
    }

    #[test]
    fn empty_source_is_an_error() {
        assert!(render("", true, (10, 20), big()).is_err());
        assert!(render("%%{init: {\"theme\": \"dark\"}}%%\n", true, (10, 20), big()).is_err());
    }

    #[test]
    fn scales_with_the_cell_height() {
        let small = render(FLOWCHART, true, (8, 16), big()).unwrap();
        let large = render(FLOWCHART, true, (16, 32), big()).unwrap();
        assert!(large.height() > small.height() * 3 / 2);
    }

    #[test]
    fn stays_within_max_px() {
        let image = render(FLOWCHART, true, (16, 32), (60, 50)).unwrap();
        assert!(
            image.width() <= 60 && image.height() <= 50,
            "{:?}",
            image.dimensions()
        );
    }

    /// Writes sample PNGs for a manual look:
    /// `cargo test -p helix-view diagram::test::write_samples -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn write_samples() {
        let samples = [
            ("flowchart", FLOWCHART),
            (
                "sequence",
                "sequenceDiagram\n  participant C as Client\n  participant A as API\n  C->>A: POST /orders\n  alt ok\n    A-->>C: 201\n  else failed\n    A-->>C: 402\n  end\n",
            ),
            (
                "class",
                "classDiagram\n  class Animal {\n    +String name\n    +speak() String\n  }\n  Animal <|-- Dog\n  Animal <|-- Cat\n",
            ),
            (
                "state",
                "stateDiagram-v2\n  [*] --> Idle\n  Idle --> Running: start\n  Running --> Done: finish\n  Done --> [*]\n",
            ),
        ];
        for (name, source) in samples {
            for dark in [true, false] {
                let image = render(source, dark, (10, 20), big()).unwrap();
                let path = std::env::temp_dir().join(format!("helix-mermaid-{name}-{dark}.png"));
                image.save(&path).unwrap();
                println!("{}", path.display());
            }
        }
    }

    use crate::graphics::Color;
    use crate::theme::Mode;

    #[test]
    fn background_luminance_decides() {
        assert!(is_dark(
            Some(Color::Rgb(0x28, 0x2c, 0x34)),
            Some(Mode::Light)
        ));
        assert!(!is_dark(
            Some(Color::Rgb(0xfd, 0xf6, 0xe3)),
            Some(Mode::Dark)
        ));
    }

    #[test]
    fn named_indexed_and_grey_backgrounds() {
        // mid grey: black text is far more readable than white
        assert!(!is_dark(Some(Color::Rgb(0xa0, 0xa0, 0xa0)), None));
        assert!(is_dark(Some(Color::Rgb(0x60, 0x60, 0x60)), None));
        assert!(is_dark(Some(Color::Black), None));
        assert!(is_dark(Some(Color::Blue), None));
        assert!(!is_dark(Some(Color::White), None));
        assert!(!is_dark(Some(Color::LightGray), None));
        assert!(is_dark(Some(Color::Indexed(234)), None)); // grey ramp, dark
        assert!(!is_dark(Some(Color::Indexed(231)), None)); // cube white
        assert!(!is_dark(Some(Color::Indexed(15)), None));
    }

    #[test]
    fn terminal_mode_then_dark_default() {
        assert!(!is_dark(None, Some(Mode::Light)));
        assert!(is_dark(None, Some(Mode::Dark)));
        assert!(is_dark(Some(Color::Reset), None));
        assert!(is_dark(None, None));
    }
}
