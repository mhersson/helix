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

#[cfg(not(windows))]
#[tokio::test(flavor = "multi_thread")]
async fn click_on_scaled_heading_lands_on_the_grapheme() -> anyhow::Result<()> {
    use termina::event::{
        Event, KeyCode, KeyEvent, Modifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    let (_dir, mut app) = setup("top\n\n# Head\n\nafter\n")?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
    // a key press makes the app render once
    tx.send(Ok(Event::Key(KeyEvent::new(
        KeyCode::Escape,
        Modifiers::NONE,
    ))))?;
    app.event_loop_until_idle(&mut stream).await;
    let (x, y, _) = origins(&app)
        .into_iter()
        .find(|(_, _, s)| s == "a")
        .expect("scaled 'a'");
    // the lower-right cell of the block, on the reserved row
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        tx.send(Ok(Event::Mouse(MouseEvent {
            kind,
            column: x + 1,
            row: y + 1,
            modifiers: Modifiers::NONE,
        })))?;
    }
    app.event_loop_until_idle(&mut stream).await;
    let (view, doc) = helix_view::current_ref!(app.editor);
    let text = doc.text().slice(..);
    let cursor = doc.selection(view.id).primary().cursor(text);
    assert_eq!((text.char_to_line(cursor), text.char(cursor)), (2, 'a'));
    let errs = app.close().await;
    assert!(errs.is_empty(), "{errs:?}");
    Ok(())
}

#[cfg(not(windows))]
#[tokio::test(flavor = "multi_thread")]
async fn click_in_the_neighbouring_view_is_not_taken_by_a_scaled_heading() -> anyhow::Result<()> {
    use termina::event::{
        Event, KeyCode, KeyEvent, Modifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    let (_dir, mut app) = setup("top\n\n# Head\n\nafter\n")?;
    // the new view is on the right; focus the left one
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
    // the new view is on the right; focus the left one
    for key in helix_view::input::parse_macro(":vsplit<ret><C-w>h")? {
        tx.send(Ok(Event::Key(KeyEvent::from(key))))?;
    }
    app.event_loop_until_idle(&mut stream).await;
    let (left, right) = {
        let mut views: Vec<_> = app
            .editor
            .tree
            .views()
            .map(|(v, _)| (v.area, v.id))
            .collect();
        views.sort_by_key(|(a, _)| a.x);
        (views[0], views[1])
    };
    assert_eq!(app.editor.tree.focus, left.1);
    tx.send(Ok(Event::Key(KeyEvent::new(
        KeyCode::Escape,
        Modifiers::NONE,
    ))))?;
    app.event_loop_until_idle(&mut stream).await;
    let (_, y, _) = origins(&app)
        .into_iter()
        .find(|(x, _, s)| s == "H" && *x < right.0.x)
        .expect("scaled 'H' in the left view");
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        tx.send(Ok(Event::Mouse(MouseEvent {
            kind,
            column: right.0.x + right.0.width - 1,
            row: y,
            modifiers: Modifiers::NONE,
        })))?;
    }
    app.event_loop_until_idle(&mut stream).await;
    assert_eq!(
        app.editor.tree.focus, right.1,
        "the click focuses the right view"
    );
    let errs = app.close().await;
    assert!(errs.is_empty(), "{errs:?}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn packed_chunk_starting_with_a_space_keeps_the_heading_colour() -> anyhow::Result<()> {
    let (_dir, mut app) = setup("top\n\n## Second level heading\n")?;
    test_key_sequence(
        &mut app,
        None,
        Some(&|app| {
            let o = origins(app);
            let buffer = app.test_backend().buffer();
            let fg = |symbol: &str| {
                let (x, y, _) = o
                    .iter()
                    .find(|(_, _, s)| s == symbol)
                    .unwrap_or_else(|| panic!("{symbol:?} in {o:?}"));
                buffer[(*x, *y)].fg
            };
            assert_eq!(fg(" hea"), fg("Seco"), "{o:?}");
        }),
        false,
    )
    .await
}

/// The text of screen row `y`, trimmed at the end.
fn row_text(app: &Application, y: u16) -> String {
    let buffer = app.test_backend().buffer();
    let area = buffer.area;
    let row: String = (area.left()..area.right())
        .map(|x| buffer[(x, y)].symbol.as_str())
        .collect();
    row.trim_end().to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn heading_rows_come_before_image_and_diagnostic_rows() -> anyhow::Result<()> {
    use helix_core::diagnostic::{Diagnostic, DiagnosticProvider, Range, Severity};
    use helix_view::annotations::diagnostics::DiagnosticFilter;
    let md = "top\n\n# One\n![a pic](pic.png)\n\nafter\n";
    let (dir, mut app) = setup_with(md, Setting::On, |editor| {
        editor.kitty_graphics = helix_view::editor::KittyGraphics::On;
        // the diagnostic is drawn in rows below its line, not at its end
        editor.end_of_line_diagnostics = DiagnosticFilter::Disable;
        editor.inline_diagnostics.other_lines = DiagnosticFilter::Enable(Severity::Hint);
    })?;
    image::RgbaImage::from_pixel(40, 60, image::Rgba([0, 128, 255, 255]))
        .save(dir.path().join("pic.png"))?;
    {
        let doc = helix_view::doc_mut!(app.editor);
        // on the heading's line "# One"
        let start = doc.text().line_to_char(2) + 2;
        doc.replace_diagnostics(
            [Diagnostic {
                range: Range {
                    start,
                    end: start + 3,
                },
                ends_at_word: true,
                starts_at_word: true,
                zero_width: false,
                line: 2,
                message: "heading diagnostic".to_string(),
                severity: Some(Severity::Error),
                code: None,
                provider: DiagnosticProvider::Lsp {
                    server_id: Default::default(),
                    identifier: None,
                },
                tags: Vec::new(),
                source: None,
                data: None,
            }],
            &[],
            None,
        );
    }
    test_key_sequence(
        &mut app,
        None,
        Some(&|app| {
            let screen: Vec<String> = (0..16).map(|y| row_text(app, y)).collect();
            let o = origins(app);
            let symbols: Vec<&str> = o.iter().map(|(_, _, s)| s.as_str()).collect();
            assert_eq!(symbols, ["O", "n", "e"], "{screen:#?}");
            let heading = row_of(app, "top").unwrap() + 2;
            assert!(o.iter().all(|&(_, y, _)| y == heading), "{o:?}");
            // the reserved row is the heading's lower half: nothing else on it
            let buffer = app.test_backend().buffer();
            assert!(
                (buffer.area.left()..buffer.area.right()).all(|x| {
                    let cell = &buffer[(x, heading + 1)];
                    cell.multicell == Multicell::Covered || cell.symbol.trim().is_empty()
                }),
                "{screen:#?}"
            );
            let diagnostic = row_of(app, "heading diagnostic").unwrap();
            assert_eq!(diagnostic, heading + 2, "{screen:#?}");
            let caption = row_of(app, "a pic").unwrap();
            assert!(caption > diagnostic, "{screen:#?}");
            let placeholders: Vec<u16> = (buffer.area.top()..buffer.area.bottom())
                .filter(|&y| {
                    (buffer.area.left()..buffer.area.right())
                        .any(|x| buffer[(x, y)].symbol.starts_with('\u{10EEEE}'))
                })
                .collect();
            assert_eq!(
                placeholders,
                [caption + 1, caption + 2, caption + 3],
                "{screen:#?}"
            );
            assert_eq!(row_of(app, "after"), Some(caption + 5), "{screen:#?}");
        }),
        false,
    )
    .await
}

/// The scaled symbols of the left and the right view of a vertical split.
fn origins_by_view(app: &Application) -> (Vec<String>, Vec<String>) {
    let right_x = app
        .editor
        .tree
        .views()
        .map(|(v, _)| v.area.x)
        .max()
        .unwrap();
    let (left, right): (Vec<_>, Vec<_>) = origins(app).into_iter().partition(|o| o.0 < right_x);
    let symbols = |o: Vec<(u16, u16, String)>| o.into_iter().map(|(_, _, s)| s).collect();
    (symbols(left), symbols(right))
}

/// Puts the heading on the cursor line of the left, unfocused view only.
const UNFOCUSED_CURSOR_ON_HEADING: &str = "jj:vsplit<ret>gg";

#[tokio::test(flavor = "multi_thread")]
async fn unfocused_cursor_line_heading_is_scaled() -> anyhow::Result<()> {
    let (_dir, mut app) = setup("top\n\n# Head\n\nafter\n")?;
    test_key_sequences(
        &mut app,
        vec![
            (
                Some(UNFOCUSED_CURSOR_ON_HEADING),
                Some(&|app: &Application| {
                    assert_eq!(app.editor.tree.views().count(), 2);
                    let (left, right) = origins_by_view(app);
                    assert_eq!(left, ["H", "e", "a", "d"]);
                    assert_eq!(right, ["H", "e", "a", "d"]);
                }),
            ),
            // one view left, so the helper's final :q! exits
            (Some("<C-w>o"), None),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn unfocused_cursor_line_heading_is_normal_size_under_cursor_line_blame() -> anyhow::Result<()>
{
    // blame `cursor-line` is drawn on every view's cursor line, focused or not
    let (_dir, mut app) = setup_with("top\n\n# Head\n\nafter\n", Setting::On, |editor| {
        editor.inline_blame.show = helix_view::editor::InlineBlameShow::CursorLine;
    })?;
    test_key_sequences(
        &mut app,
        vec![
            (
                Some(UNFOCUSED_CURSOR_ON_HEADING),
                Some(&|app: &Application| {
                    assert_eq!(app.editor.tree.views().count(), 2);
                    let (left, right) = origins_by_view(app);
                    assert!(left.is_empty(), "{left:?}");
                    assert_eq!(right, ["H", "e", "a", "d"], "cursor elsewhere");
                    // the row stays reserved in both views
                    let buffer = app.test_backend().buffer();
                    let rows: Vec<u16> = (buffer.area.top()..buffer.area.bottom())
                        .filter(|&y| {
                            let row: String = (buffer.area.left()..buffer.area.right())
                                .map(|x| buffer[(x, y)].symbol.as_str())
                                .collect();
                            row.contains("after")
                        })
                        .collect();
                    assert_eq!(rows.len(), 1, "both views draw 'after' on one row");
                }),
            ),
            // one view left, so the helper's final :q! exits
            (Some("<C-w>o"), None),
        ],
        false,
    )
    .await
}

#[cfg(not(windows))]
#[tokio::test(flavor = "multi_thread")]
async fn unfocused_cursor_line_heading_is_normal_size_with_inline_completion() -> anyhow::Result<()>
{
    use termina::event::{Event, KeyCode, KeyEvent, Modifiers};
    // ghost text is drawn on every view's cursor line, focused or not
    let (_dir, mut app) = setup("top\n\n# Head\n\nafter\n")?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut stream = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
    for key in helix_view::input::parse_macro(UNFOCUSED_CURSOR_ON_HEADING)? {
        tx.send(Ok(Event::Key(KeyEvent::from(key))))?;
    }
    app.event_loop_until_idle(&mut stream).await;
    assert_eq!(origins_by_view(&app).0, ["H", "e", "a", "d"]);
    helix_view::doc_mut!(app.editor).inline_completions.push(
        helix_view::document::InlineCompletion {
            ghost_text: "ghost".to_string(),
            replace_range: helix_core::Range::point(0),
            cursor_char_idx: 0,
            overlays: Vec::new(),
            overflow_text: None,
            eol_ghost_text: Some("ghost".to_string()),
            additional_lines: Vec::new(),
        },
    );
    // a key that leaves the selection alone makes the app render again
    tx.send(Ok(Event::Key(KeyEvent::new(
        KeyCode::Escape,
        Modifiers::NONE,
    ))))?;
    app.event_loop_until_idle(&mut stream).await;
    assert!(
        helix_view::doc!(app.editor)
            .inline_completions
            .current()
            .is_some(),
        "the completion is still there"
    );
    let (left, right) = origins_by_view(&app);
    assert!(left.is_empty(), "{left:?}");
    assert_eq!(right, ["H", "e", "a", "d"], "cursor elsewhere");
    let errs = app.close().await;
    assert!(errs.is_empty(), "{errs:?}");
    Ok(())
}
