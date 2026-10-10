use helix_term::application::Application;

use super::*;

fn concealed_chars(app: &Application) -> Vec<usize> {
    let (view, doc) = helix_view::current_ref!(app.editor);
    doc.conceals(view.id)
        .unwrap_or_default()
        .iter()
        .flat_map(|conceal| conceal.range.clone())
        .collect()
}

fn markdown_app(input: &str) -> anyhow::Result<Application> {
    AppBuilder::new()
        .with_file("render.md", None)
        .with_input_text(input)
        .build()
}

#[tokio::test(flavor = "multi_thread")]
async fn renders_on_open() -> anyhow::Result<()> {
    let mut app = markdown_app("**#[b|]#old**\n")?;
    test_key_sequence(
        &mut app,
        Some("l"),
        Some(&|app| assert_eq!(concealed_chars(app), [0, 1, 6, 7])),
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_reveals_cursor_block() -> anyhow::Result<()> {
    let mut app = markdown_app("**a**\n\n**#[b|]#**\n")?;
    test_key_sequences(
        &mut app,
        vec![
            (
                Some("i"),
                Some(&|app| assert_eq!(concealed_chars(app), [0, 1, 3, 4])),
            ),
            (
                Some("<esc>"),
                Some(&|app| assert_eq!(concealed_chars(app), [0, 1, 3, 4, 7, 8, 10, 11])),
            ),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn goto_snaps_to_visible() -> anyhow::Result<()> {
    let mut app = markdown_app("#[x|]#\n## Title\n")?;
    test_key_sequence(
        &mut app,
        Some(":goto 2<ret>"),
        Some(&|app| {
            let (view, doc) = helix_view::current_ref!(app.editor);
            assert_eq!(
                helix_core::test::plain(doc.text().slice(..), doc.selection(view.id)),
                "x\n## #[T|]#itle\n"
            );
        }),
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn jump_far_snaps_to_visible() -> anyhow::Result<()> {
    let mut input = String::from("#[s|]#tart\n");
    for i in 0..300 {
        input.push_str(&format!("line {i}\n\n"));
    }
    input.push_str("## End\n");
    let mut app = markdown_app(&input)?;
    test_key_sequence(
        &mut app,
        Some("ge"),
        Some(&|app| {
            let (view, doc) = helix_view::current_ref!(app.editor);
            let text = doc.text().slice(..);
            let cursor = doc.selection(view.id).primary().cursor(text);
            assert_eq!(text.char(cursor), 'E');
        }),
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_bold_word() -> anyhow::Result<()> {
    let mut app = markdown_app("a **#[b|]#** c\n")?;
    test_key_sequence(
        &mut app,
        Some("d"),
        Some(&|app| {
            let (view, doc) = helix_view::current_ref!(app.editor);
            assert_eq!(
                helix_core::test::plain(doc.text().slice(..), doc.selection(view.id)),
                "a #[ |]#c\n"
            );
        }),
        false,
    )
    .await
}

fn cursor_screen_row(app: &Application) -> usize {
    let (view, doc) = helix_view::current_ref!(app.editor);
    let text = doc.text().slice(..);
    let cursor = doc.selection(view.id).primary().cursor(text);
    view.screen_coords_at_pos(doc, text, cursor).unwrap().row
}

#[tokio::test(flavor = "multi_thread")]
async fn reveal_keeps_cursor_row() -> anyhow::Result<()> {
    // one paragraph whose raw lines soft-wrap but whose rendered lines do not
    let line = "**aaaa** ".repeat(14);
    let mut input = String::new();
    for _ in 0..5 {
        input.push_str(line.trim_end());
        input.push('\n');
    }
    input.push_str("#[e|]#nd\n");

    let mut config = test_config();
    config.editor.soft_wrap.enable = Some(true);
    let mut app = AppBuilder::new()
        .with_file("render.md", None)
        .with_config(config)
        .with_input_text(&input)
        .build()?;

    let row = std::cell::Cell::new(0);
    test_key_sequences(
        &mut app,
        vec![
            (None, Some(&|app| row.set(cursor_screen_row(app)))),
            (
                Some("i"),
                Some(&|app| assert_eq!(cursor_screen_row(app), row.get())),
            ),
            (
                Some("<esc>"),
                Some(&|app| assert_eq!(cursor_screen_row(app), row.get())),
            ),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn toggle_command() -> anyhow::Result<()> {
    let mut app = markdown_app("**#[b|]#**\n")?;
    test_key_sequences(
        &mut app,
        vec![
            (
                Some(":markdown-render off<ret>"),
                Some(&|app| assert!(concealed_chars(app).is_empty())),
            ),
            (
                Some(":markdown-render<ret>"),
                Some(&|app| assert_eq!(concealed_chars(app), [0, 1, 3, 4])),
            ),
            (
                Some(":markdown-render bogus<ret>"),
                Some(&|app| assert!(app.editor.is_err())),
            ),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn toggle_rejects_non_markdown() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().with_file("x.rs", None).build()?;
    test_key_sequence(
        &mut app,
        Some(":markdown-render<ret>"),
        Some(&|app| assert!(app.editor.is_err())),
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn off_survives_save_as() -> anyhow::Result<()> {
    let mut app = markdown_app("**#[b|]#**\n")?;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("saved.md");
    let save = format!(":markdown-render off<ret>:w {}<ret>", path.display());
    test_key_sequence(
        &mut app,
        Some(&save),
        Some(&|app| assert!(concealed_chars(app).is_empty())),
        false,
    )
    .await
}

fn text_of(app: &Application) -> String {
    let (_, doc) = helix_view::current_ref!(app.editor);
    doc.text().to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn append_lands_after_closing_markers() -> anyhow::Result<()> {
    let mut app = markdown_app("**bol#[d|]#** x\n")?;
    test_key_sequence(
        &mut app,
        Some("<esc>aX<esc>"),
        Some(&|app| assert_eq!(text_of(app), "**bold**X x\n")),
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_at_line_start_skips_heading_marker() -> anyhow::Result<()> {
    let mut app = markdown_app("## Ti#[t|]#le\n")?;
    test_key_sequence(
        &mut app,
        Some("<esc>IX<esc>"),
        Some(&|app| assert_eq!(text_of(app), "## XTitle\n")),
        false,
    )
    .await
}
