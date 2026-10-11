use helix_term::application::Application;
use helix_view::editor::KittyGraphics;

use super::*;

const PLACEHOLDER: char = '\u{10EEEE}';
const FLOWCHART: &str = "```mermaid\nflowchart TD\n  A[Start] --> B[Done]\n```\n\nafter\n";
const INVALID: &str = "```mermaid\nflowchart TD\n  A -->\n```\n\nafter\n";

fn setup_with(
    md: &str,
    kitty_graphics: KittyGraphics,
) -> anyhow::Result<(tempfile::TempDir, Application)> {
    let dir = tempfile::tempdir()?;
    let doc = dir.path().join("doc.md");
    std::fs::write(&doc, md)?;
    let mut config = test_config();
    config.editor.kitty_graphics = kitty_graphics;
    let app = AppBuilder::new()
        .with_config(config)
        .with_file(doc, None)
        .build()?;
    Ok((dir, app))
}

fn setup(md: &str) -> anyhow::Result<(tempfile::TempDir, Application)> {
    setup_with(md, KittyGraphics::On)
}

fn placeholder_rows(app: &Application) -> Vec<u16> {
    let buffer = app.test_backend().buffer();
    let area = buffer.area;
    (area.top()..area.bottom())
        .filter(|&y| {
            (area.left()..area.right()).any(|x| buffer[(x, y)].symbol.starts_with(PLACEHOLDER))
        })
        .collect()
}

/// The screen row of the first cell showing `text`.
fn row_of(app: &Application, text: &str) -> Option<u16> {
    let buffer = app.test_backend().buffer();
    let area = buffer.area;
    (area.top()..area.bottom()).find(|&y| {
        let row: String = (area.left()..area.right())
            .map(|x| buffer[(x, y)].symbol.as_str())
            .collect();
        row.contains(text)
    })
}

fn placement(app: &Application) -> helix_view::markdown_render::ImagePlacement {
    let (view, doc) = helix_view::current_ref!(app.editor);
    let placements = helix_view::markdown_render::placements(doc, view.id);
    assert_eq!(placements.len(), 1, "{placements:?}");
    placements[0]
}

#[tokio::test(flavor = "multi_thread")]
async fn diagram_replaces_its_block() -> anyhow::Result<()> {
    let (_dir, mut app) = setup(FLOWCHART)?;
    test_key_sequence(
        &mut app,
        Some("ge"),
        Some(&|app| {
            let p = placement(app);
            assert_eq!((p.line, p.covers), (0, 4));
            let rows = placeholder_rows(app);
            assert_eq!(rows.len(), p.rows as usize);
            assert_eq!(rows[0], 0);
            assert!(rows.windows(2).all(|w| w[1] == w[0] + 1), "{rows:?}");
            assert_eq!(row_of(app, "flowchart"), None);
            // the blank line after the block, then "after"
            let after = row_of(app, "after").unwrap();
            assert_eq!(after as usize, (p.rows as usize).max(p.covers) + 1);
        }),
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_reveals_the_source() -> anyhow::Result<()> {
    let (_dir, mut app) = setup(FLOWCHART)?;
    test_key_sequences(
        &mut app,
        vec![
            (
                Some("ggji"),
                Some(&|app| {
                    assert!(placeholder_rows(app).is_empty());
                    assert!(row_of(app, "flowchart TD").is_some());
                }),
            ),
            (
                Some("<esc>"),
                Some(&|app| assert!(!placeholder_rows(app).is_empty())),
            ),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn cursor_on_covered_line_keeps_the_diagram() -> anyhow::Result<()> {
    let (_dir, mut app) = setup(FLOWCHART)?;
    test_key_sequences(
        &mut app,
        vec![
            (Some("gg"), None),
            (
                Some("jj"),
                Some(&|app| {
                    let p = placement(app);
                    assert_eq!(placeholder_rows(app).len(), p.rows as usize);
                    let (view, doc) = helix_view::current_ref!(app.editor);
                    let text = doc.text().slice(..);
                    let line = doc.selection(view.id).primary().cursor_line(text);
                    assert_eq!(line, 2);
                    // the first text column (after the gutter, where the cursor
                    // is) of every diagram row is a placeholder
                    let buffer = app.test_backend().buffer();
                    let area = buffer.area;
                    let rows = placeholder_rows(app);
                    let x = (area.left()..area.right())
                        .find(|&x| buffer[(x, rows[0])].symbol.starts_with(PLACEHOLDER))
                        .unwrap();
                    for y in rows {
                        assert!(buffer[(x, y)].symbol.starts_with(PLACEHOLDER), "row {y}");
                    }
                }),
            ),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn syntax_error_shows_a_warning() -> anyhow::Result<()> {
    let (_dir, mut app) = setup(INVALID)?;
    test_key_sequence(
        &mut app,
        Some("ge"),
        Some(&|app| {
            assert!(placeholder_rows(app).is_empty());
            assert_eq!(row_of(app, "⚠"), Some(0));
            assert!(row_of(app, "flowchart TD").is_some());
        }),
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn no_graphics_renders_a_code_block() -> anyhow::Result<()> {
    let (_dir, mut app) = setup_with(FLOWCHART, KittyGraphics::Off)?;
    test_key_sequence(
        &mut app,
        Some("ge"),
        Some(&|app| {
            assert!(placeholder_rows(app).is_empty());
            assert!(app.test_backend().graphics().is_empty());
            assert_eq!(row_of(app, "───mermaid"), Some(0));
            assert!(row_of(app, "flowchart TD").is_some());
        }),
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn diagram_at_end_without_newline_fits_its_lines() -> anyhow::Result<()> {
    let (_dir, mut app) = setup("```mermaid\nflowchart TD\n  A[Start] --> B[Done]\n```")?;
    test_key_sequence(
        &mut app,
        Some("gg"),
        Some(&|app| {
            let p = placement(app);
            assert!(p.rows as usize <= p.covers, "{p:?}");
            assert_eq!(placeholder_rows(app).len(), p.rows as usize);
        }),
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn tall_block_with_small_diagram_reserves_nothing() -> anyhow::Result<()> {
    let md = format!(
        "```mermaid\nflowchart TD\n  A\n{}```\n\nafter\n",
        "\n".repeat(20)
    );
    let (_dir, mut app) = setup(&md)?;
    test_key_sequence(
        &mut app,
        Some("gg"),
        Some(&|app| {
            let p = placement(app);
            assert!((p.rows as usize) < p.covers, "{p:?}");
            assert_eq!(row_of(app, "after"), Some(p.covers as u16 + 1));
        }),
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn quoted_diagram_renders() -> anyhow::Result<()> {
    let (_dir, mut app) = setup("> ```mermaid\n> flowchart TD\n>   A --> B\n> ```\n\nafter\n")?;
    test_key_sequence(
        &mut app,
        Some("ge"),
        Some(&|app| {
            assert_eq!(placement(app).covers, 4);
            assert!(!placeholder_rows(app).is_empty());
            assert_eq!(row_of(app, "▎"), None);
        }),
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn theme_switch_renders_the_other_variant() -> anyhow::Result<()> {
    let (_dir, mut app) = setup(FLOWCHART)?;
    test_key_sequences(
        &mut app,
        vec![
            (Some("ge"), None),
            (
                Some(":theme solarized_light<ret>"),
                Some(&|app| {
                    let sent = String::from_utf8_lossy(app.test_backend().graphics())
                        .matches("a=T")
                        .count();
                    assert_eq!(sent, 2);
                    assert!(!placeholder_rows(app).is_empty());
                }),
            ),
        ],
        false,
    )
    .await
}
