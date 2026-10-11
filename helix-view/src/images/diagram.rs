//! Mermaid diagrams rendered to images with merman and resvg, see [`render`].

use std::borrow::Cow;
use std::sync::{Arc, OnceLock};

use image::RgbaImage;
use resvg::{tiny_skia, usvg};

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

/// Renders a Mermaid diagram with its text about one cell tall, scaled down
/// (keeping the aspect ratio) to fit `max_px`. The error is one line.
pub fn render(
    source: &str,
    dark: bool,
    cell_px: (u16, u16),
    max_px: (u32, u32),
) -> Result<RgbaImage, String> {
    let source = with_theme(source, dark);
    let svg = std::panic::catch_unwind(|| svg(&source))
        .unwrap_or_else(|_| Err("the diagram renderer crashed".to_string()))?;
    rasterize(&svg, cell_px.1.max(1) as f32 / BASE_FONT_PX, max_px)
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
}
