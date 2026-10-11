use helix_term::application::Application;
use helix_view::text_sizing::Setting;
use tui::multicell::Multicell;

use super::*;

fn setup_with(
    md: &str,
    setting: Setting,
    edit: impl FnOnce(&mut helix_view::editor::Config),
) -> anyhow::Result<(tempfile::TempDir, Application)> {
    let dir = tempfile::tempdir()?;
    let doc = dir.path().join("doc.md");
    std::fs::write(&doc, md)?;
    let mut config = test_config();
    config.editor.text_sizing = setting;
    config.editor.true_color = true;
    edit(&mut config.editor);
    let app = AppBuilder::new()
        .with_config(config)
        .with_file(doc, None)
        .build()?;
    Ok((dir, app))
}

fn setup(md: &str) -> anyhow::Result<(tempfile::TempDir, Application)> {
    setup_with(md, Setting::On, |_| {})
}

/// `(x, y, symbol)` of every multicell origin on screen, row-major.
fn origins(app: &Application) -> Vec<(u16, u16, String)> {
    let buffer = app.test_backend().buffer();
    let width = buffer.area.width as usize;
    buffer
        .content
        .iter()
        .enumerate()
        .filter(|(_, c)| matches!(c.multicell, Multicell::Origin(_)))
        .map(|(i, c)| ((i % width) as u16, (i / width) as u16, c.symbol.to_string()))
        .collect()
}

/// The screen row of the first normal-size cell run showing `text`.
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

const LEVELS: &str = "top\n\n# One\n\n## Two\n\n### Three\n\n#### Four\n";

#[tokio::test(flavor = "multi_thread")]
async fn h1_h2_h3_are_scaled_and_h4_is_not() -> anyhow::Result<()> {
    let (_dir, mut app) = setup(LEVELS)?;
    test_key_sequence(
        &mut app,
        None,
        Some(&|app| {
            let o = origins(app);
            let symbols: Vec<&str> = o.iter().map(|(_, _, s)| s.as_str()).collect();
            // h1 one origin per grapheme, h2/h3 packed chunks
            assert_eq!(symbols, ["O", "n", "e", "Two", "Three"], "{o:?}");
            let (one_row, two_row, three_row) = (o[0].1, o[3].1, o[4].1);
            let top = row_of(app, "top").unwrap();
            assert_eq!(one_row, top + 2);
            // heading row + reserved row + blank line
            assert_eq!(two_row, one_row + 3);
            assert_eq!(three_row, two_row + 3);
            assert_eq!(row_of(app, "Four"), Some(three_row + 3));
            // h1 graphemes are 2 cells apart
            assert_eq!((o[1].0 - o[0].0, o[2].0 - o[1].0), (2, 2));
        }),
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn cursor_line_heading_is_normal_size_and_keeps_its_row() -> anyhow::Result<()> {
    let (_dir, mut app) = setup("# One\n\nafter\n")?;
    test_key_sequence(
        &mut app,
        None,
        Some(&|app| {
            assert!(origins(app).is_empty());
            let one = row_of(app, "One").unwrap();
            assert_eq!(row_of(app, "after"), Some(one + 3));
        }),
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_mode_on_heading_keeps_layout() -> anyhow::Result<()> {
    let (_dir, mut app) = setup("x\n\n# One\n\nafter\n")?;
    let after = std::cell::Cell::new(0);
    test_key_sequences(
        &mut app,
        vec![
            (
                None,
                Some(&|app: &Application| {
                    assert_eq!(origins(app).len(), 3);
                    after.set(row_of(app, "after").unwrap());
                }),
            ),
            (
                Some("3ggi"),
                Some(&|app: &Application| {
                    assert!(origins(app).is_empty(), "revealed and on the cursor line");
                    assert!(row_of(app, "# One").is_some(), "shown raw");
                    assert_eq!(row_of(app, "after"), Some(after.get()), "nothing moved");
                }),
            ),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn eol_virtual_text_forces_normal_size() -> anyhow::Result<()> {
    let (_dir, mut app) = setup_with("x\n\n# One\n\nafter\n", Setting::On, |editor| {
        editor.inline_blame.show = helix_view::editor::InlineBlameShow::AllLines;
    })?;
    test_key_sequence(
        &mut app,
        None,
        Some(&|app| {
            assert!(origins(app).is_empty());
            let one = row_of(app, "One").unwrap();
            assert_eq!(row_of(app, "after"), Some(one + 2), "no row reserved");
        }),
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn too_wide_heading_falls_back() -> anyhow::Result<()> {
    let md = format!("x\n\n# {}\n\nafter\n", "W".repeat(70));
    let (_dir, mut app) = setup(&md)?;
    test_key_sequence(
        &mut app,
        None,
        Some(&|app| {
            assert!(origins(app).is_empty());
            let heading = row_of(app, "WWWW").unwrap();
            assert_eq!(row_of(app, "after"), Some(heading + 2));
        }),
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn feature_off_draws_like_today() -> anyhow::Result<()> {
    let (_dir, mut app) = setup_with("x\n\n# One\n\nafter\n", Setting::Off, |_| {})?;
    test_key_sequence(
        &mut app,
        None,
        Some(&|app| {
            assert!(origins(app).is_empty());
            let one = row_of(app, "One").unwrap();
            assert_eq!(row_of(app, "after"), Some(one + 2));
        }),
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn control_chars_in_heading_never_reach_a_multicell() -> anyhow::Result<()> {
    let (_dir, mut app) = setup("x\n\n# A\x1bB\x07C\u{9c}D\n\nafter\n")?;
    test_key_sequence(
        &mut app,
        None,
        Some(&|app| {
            let o = origins(app);
            assert!(
                o.iter().all(|(_, _, s)| !s.chars().any(char::is_control)),
                "{o:?}"
            );
            // helix draws the raw control chars into the cells, so the whole
            // heading falls back to normal size but keeps its reserved row
            assert!(o.is_empty(), "{o:?}");
            let heading = row_of(app, "A\x1bB\x07C\u{9c}D").expect("drawn raw");
            assert_eq!(row_of(app, "after"), Some(heading + 3));
        }),
        false,
    )
    .await
}
