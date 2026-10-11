use helix_term::application::Application;
use helix_view::editor::KittyGraphics;
use helix_view::text_sizing::Setting;

use super::*;

const PLACEHOLDER: char = '\u{10EEEE}';
const TABLE: &str = "x\n\n| a | bb |\n|---|---:|\n| ccc | d |\n\nafter\n";

fn setup_with(
    md: &str,
    edit: impl FnOnce(&mut helix_view::editor::Config),
) -> anyhow::Result<(tempfile::TempDir, Application)> {
    let dir = tempfile::tempdir()?;
    image::RgbaImage::from_pixel(40, 60, image::Rgba([0, 128, 255, 255]))
        .save(dir.path().join("pic.png"))?;
    let doc = dir.path().join("doc.md");
    std::fs::write(&doc, md)?;
    let mut config = test_config();
    // styles are compared: without this the theme depends on COLORTERM
    config.editor.true_color = true;
    config.editor.kitty_graphics = KittyGraphics::Off;
    config.editor.text_sizing = Setting::Off;
    edit(&mut config.editor);
    let app = AppBuilder::new()
        .with_config(config)
        .with_file(doc, None)
        .build()?;
    Ok((dir, app))
}

fn setup(md: &str) -> anyhow::Result<(tempfile::TempDir, Application)> {
    setup_with(md, |_| {})
}

/// The text area of every screen row (no gutter), trailing whitespace
/// trimmed.
fn screen(app: &Application) -> Vec<String> {
    let (view, doc) = helix_view::current_ref!(app.editor);
    let inner = view.inner_area(doc);
    let buffer = app.test_backend().buffer();
    let area = buffer.area;
    (area.top()..area.bottom())
        .map(|y| {
            (inner.left()..inner.right())
                .map(|x| buffer[(x, y)].symbol.as_str())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

/// The screen row of the first row containing `text`.
fn row_of(app: &Application, text: &str) -> Option<u16> {
    screen(app)
        .iter()
        .position(|row| row.contains(text))
        .map(|y| y as u16)
}

/// The cell showing the first `symbol` on screen.
fn cell_of<'a>(app: &'a Application, symbol: &str) -> Option<&'a tui::buffer::Cell> {
    app.test_backend()
        .buffer()
        .content
        .iter()
        .find(|cell| cell.symbol == symbol)
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

#[tokio::test(flavor = "multi_thread")]
async fn borders_are_drawn_above_and_below_the_table() -> anyhow::Result<()> {
    let (_dir, mut app) = setup(TABLE)?;
    test_key_sequence(
        &mut app,
        None,
        Some(&|app| {
            let x = row_of(app, "x").unwrap() as usize;
            let rows = screen(app);
            assert_eq!(
                rows[x..x + 9],
                [
                    "x",
                    "",
                    "┌─────┬────┐",
                    "│ a   │ bb │",
                    "├─────┼────┤",
                    "│ ccc │  d │",
                    "└─────┴────┘",
                    "",
                    "after",
                ],
                "{rows:#?}"
            );
            // the same style as the delimiter rule
            let rule = cell_of(app, "├").unwrap();
            for symbol in ["┌", "┬", "┐", "└", "┴", "┘"] {
                let cell = cell_of(app, symbol).unwrap();
                assert_eq!(cell.style(), rule.style(), "{symbol}");
            }
            let line = cell_of(app, "─").unwrap();
            assert_eq!(line.style(), rule.style());
        }),
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_mode_reveal_keeps_the_layout() -> anyhow::Result<()> {
    let (_dir, mut app) = setup(TABLE)?;
    let rows = std::cell::Cell::new((0, 0));
    test_key_sequences(
        &mut app,
        vec![
            (
                None,
                Some(&|app: &Application| {
                    let header = row_of(app, "│ a   │ bb │").unwrap();
                    rows.set((header, row_of(app, "after").unwrap()));
                }),
            ),
            (
                Some("3ggi"),
                Some(&|app: &Application| {
                    let (header, after) = rows.get();
                    assert_eq!(
                        row_of(app, "| a | bb |"),
                        Some(header),
                        "shown raw in place"
                    );
                    assert_eq!(row_of(app, "after"), Some(after), "nothing moved");
                    let screen = screen(app);
                    assert!(
                        !screen
                            .iter()
                            .any(|row| row.contains('┌') || row.contains('└')),
                        "{screen:#?}"
                    );
                    assert_eq!(screen[header as usize - 1], "", "blank top border row");
                    assert_eq!(screen[header as usize + 3], "", "blank bottom border row");
                }),
            ),
            (
                Some("<esc>"),
                Some(&|app: &Application| {
                    let (header, after) = rows.get();
                    assert_eq!(row_of(app, "┌─────┬────┐"), Some(header - 1));
                    assert_eq!(row_of(app, "after"), Some(after));
                }),
            ),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn heading_row_comes_before_the_top_border() -> anyhow::Result<()> {
    let md = "x\n\n# One\n| a |\n|---|\n| b |\n\nafter\n";
    let (_dir, mut app) = setup_with(md, |editor| editor.text_sizing = Setting::On)?;
    test_key_sequence(
        &mut app,
        None,
        Some(&|app| {
            let x = row_of(app, "x").unwrap();
            // x, blank, heading, its reserved row, top border, table
            assert_eq!(row_of(app, "┌───┐"), Some(x + 4), "{:#?}", screen(app));
            assert_eq!(row_of(app, "│ a │"), Some(x + 5));
            assert_eq!(row_of(app, "└───┘"), Some(x + 8));
        }),
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn image_rows_come_before_the_top_border() -> anyhow::Result<()> {
    // a table may follow a paragraph directly
    let md = "![a pic](pic.png)\n| a |\n|---|\n| b |\n\nafter\n";
    let (_dir, mut app) = setup_with(md, |editor| editor.kitty_graphics = KittyGraphics::On)?;
    test_key_sequences(
        &mut app,
        vec![
            (Some("j"), None),
            (
                Some("k"),
                Some(&|app| {
                    let caption = row_of(app, "a pic").unwrap();
                    // 40x60 px at the 10x20 fallback cell size = 4x3 cells
                    assert_eq!(
                        placeholder_rows(app),
                        [caption + 1, caption + 2, caption + 3]
                    );
                    assert_eq!(
                        row_of(app, "┌───┐"),
                        Some(caption + 4),
                        "{:#?}",
                        screen(app)
                    );
                    assert_eq!(row_of(app, "│ a │"), Some(caption + 5));
                }),
            ),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn diagram_rows_come_before_the_top_border() -> anyhow::Result<()> {
    let md = "```mermaid\nflowchart TD\n  A[Start] --> B[Done]\n```\n| a |\n|---|\n| b |\n";
    let (_dir, mut app) = setup_with(md, |editor| editor.kitty_graphics = KittyGraphics::On)?;
    test_key_sequence(
        &mut app,
        Some("ge"),
        Some(&|app| {
            let rows = placeholder_rows(app);
            assert!(!rows.is_empty());
            let header = row_of(app, "│ a │").unwrap();
            assert_eq!(row_of(app, "┌───┐"), Some(header - 1), "{:#?}", screen(app));
            assert!(rows.iter().all(|&row| row < header - 1), "{rows:?}");
        }),
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn rendering_off_draws_the_raw_table() -> anyhow::Result<()> {
    let (_dir, mut app) = setup(TABLE)?;
    test_key_sequence(
        &mut app,
        Some(":markdown-render off<ret>"),
        Some(&|app| {
            let x = row_of(app, "x").unwrap() as usize;
            let rows = screen(app);
            assert_eq!(
                rows[x..x + 7],
                [
                    "x",
                    "",
                    "| a | bb |",
                    "|---|---:|",
                    "| ccc | d |",
                    "",
                    "after",
                ],
                "{rows:#?}"
            );
        }),
        false,
    )
    .await
}
