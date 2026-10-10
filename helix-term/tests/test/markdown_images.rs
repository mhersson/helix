use helix_term::application::Application;
use helix_view::editor::KittyGraphics;

use super::*;

const PLACEHOLDER: char = '\u{10EEEE}';

fn setup(md: &str) -> anyhow::Result<(tempfile::TempDir, Application)> {
    setup_with(md, KittyGraphics::On)
}

fn setup_with(
    md: &str,
    kitty_graphics: KittyGraphics,
) -> anyhow::Result<(tempfile::TempDir, Application)> {
    let dir = tempfile::tempdir()?;
    image::RgbaImage::from_pixel(40, 60, image::Rgba([0, 128, 255, 255]))
        .save(dir.path().join("pic.png"))?;
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

/// Screen rows (y) that contain placeholder cells.
fn placeholder_rows(app: &Application) -> Vec<u16> {
    let buffer = app.test_backend().buffer();
    let area = buffer.area;
    (area.top()..area.bottom())
        .filter(|&y| {
            (area.left()..area.right()).any(|x| buffer[(x, y)].symbol.starts_with(PLACEHOLDER))
        })
        .collect()
}

/// Whether some screen row contains `text`.
fn screen_contains(app: &Application, text: &str) -> bool {
    let buffer = app.test_backend().buffer();
    let area = buffer.area;
    (area.top()..area.bottom()).any(|y| {
        let row: String = (area.left()..area.right())
            .map(|x| buffer[(x, y)].symbol.as_str())
            .collect();
        row.contains(text)
    })
}

fn transmissions(app: &Application) -> usize {
    String::from_utf8_lossy(app.test_backend().graphics())
        .matches("a=T")
        .count()
}

#[tokio::test(flavor = "multi_thread")]
async fn image_is_drawn_below_caption() -> anyhow::Result<()> {
    let (_dir, mut app) = setup("![a pic](pic.png)\n\nafter\n")?;
    test_key_sequences(
        &mut app,
        vec![
            (Some("j"), None),
            (
                Some("k"),
                Some(&|app| {
                    // 40x60 px at the 10x20 fallback cell size = 4x3 cells
                    assert_eq!(placeholder_rows(app).len(), 3);
                    assert_eq!(transmissions(app), 1);
                    let (view, doc) = helix_view::current_ref!(app.editor);
                    let concealed: Vec<usize> = doc
                        .conceals(view.id)
                        .unwrap_or_default()
                        .iter()
                        .flat_map(|c| c.range.clone())
                        .collect();
                    assert_eq!(concealed, [0, 1, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
                }),
            ),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_reveals_raw_image_text() -> anyhow::Result<()> {
    let (_dir, mut app) = setup("![a pic](pic.png)\n\nafter\n")?;
    test_key_sequences(
        &mut app,
        vec![
            (
                Some("i"),
                Some(&|app| {
                    assert!(placeholder_rows(app).is_empty());
                    assert!(screen_contains(app, "![a pic](pic.png)"));
                }),
            ),
            (
                Some("<esc>"),
                Some(&|app| assert_eq!(placeholder_rows(app).len(), 3)),
            ),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn partially_scrolled_image_draws_remaining_rows() -> anyhow::Result<()> {
    // enough lines below that scrolloff lets the view scroll
    let md = format!("![a pic](pic.png)\n\nafter\n{}", "x\n".repeat(200));
    let (_dir, mut app) = setup(&md)?;
    test_key_sequences(
        &mut app,
        vec![
            // the caption line scrolls off first
            (
                Some("zj"),
                Some(&|app| assert_eq!(placeholder_rows(app), [0, 1, 2])),
            ),
            (
                Some("zj"),
                Some(&|app| assert_eq!(placeholder_rows(app), [0, 1])),
            ),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn split_keeps_both_placements() -> anyhow::Result<()> {
    let (_dir, mut app) = setup("![a pic](pic.png)\n\nafter\n")?;
    test_key_sequences(
        &mut app,
        vec![
            (
                Some("<C-w>v"),
                Some(&|app| {
                    assert!(!placeholder_rows(app).is_empty());
                    // both splits draw the image
                    let buffer = app.test_backend().buffer();
                    let mid = buffer.area.width / 2;
                    let columns: Vec<u16> = buffer
                        .content
                        .iter()
                        .enumerate()
                        .filter(|(_, cell)| cell.symbol.starts_with(PLACEHOLDER))
                        .map(|(i, _)| i as u16 % buffer.area.width)
                        .collect();
                    assert!(columns.iter().any(|&x| x < mid));
                    assert!(columns.iter().any(|&x| x >= mid));
                    assert!(transmissions(app) >= 1);
                    // nothing transmitted is deleted while both views show the image
                    let bytes = String::from_utf8_lossy(app.test_backend().graphics()).to_string();
                    assert!(!bytes.contains("a=d"));
                }),
            ),
            // `:q!` would only close one of the splits
            (Some(":qa!<ret>"), None),
        ],
        true,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn no_graphics_keeps_raw_text() -> anyhow::Result<()> {
    // explicitly off: the test process may itself run inside kitty or Ghostty
    let (_dir, mut app) = setup_with("![a pic](pic.png)\n\nafter\n", KittyGraphics::Off)?;
    test_key_sequence(
        &mut app,
        Some("j"),
        Some(&|app| {
            assert!(placeholder_rows(app).is_empty());
            assert!(app.test_backend().graphics().is_empty());
            assert!(screen_contains(app, "![a pic](pic.png)"));
        }),
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn hidden_document_frees_its_ids() -> anyhow::Result<()> {
    let (_dir, mut app) = setup("![a pic](pic.png)\n\nafter\n")?;
    test_key_sequences(
        &mut app,
        vec![
            (Some("j"), Some(&|app| assert_eq!(transmissions(app), 1))),
            (
                // the view switches to a scratch buffer, doc.md stays open
                Some(":new<ret>"),
                Some(&|app| {
                    let bytes = String::from_utf8_lossy(app.test_backend().graphics()).to_string();
                    assert!(bytes.contains("a=d"));
                }),
            ),
            // switching back sends the kept pixels again
            (
                Some("ga"),
                Some(&|app| {
                    assert_eq!(transmissions(app), 2);
                    assert_eq!(placeholder_rows(app).len(), 3);
                }),
            ),
            (Some(":qa!<ret>"), None),
        ],
        true,
    )
    .await
}
