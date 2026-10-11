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

#[tokio::test(flavor = "multi_thread")]
async fn thematic_break_does_not_soft_wrap() -> anyhow::Result<()> {
    for wrap_at_text_width in [false, true] {
        let mut config = test_config();
        config.editor.soft_wrap.enable = Some(true);
        config.editor.soft_wrap.wrap_at_text_width = Some(wrap_at_text_width);
        config.editor.text_width = 20;
        let mut app = AppBuilder::new()
            .with_file("render.md", None)
            .with_config(config)
            .with_input_text("a\n\n---\n#[x|]#\n")
            .build()?;
        test_key_sequence(
            &mut app,
            None,
            Some(&|app| assert_eq!(cursor_screen_row(app), 3, "{wrap_at_text_width}")),
            false,
        )
        .await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_reveals_only_in_focused_view() -> anyhow::Result<()> {
    let mut app = markdown_app("**#[b|]#**\n")?;
    let check = |app: &Application| {
        let focus = app.editor.tree.focus;
        let (_, doc) = helix_view::current_ref!(app.editor);
        let views: Vec<_> = app.editor.tree.views().map(|(view, _)| view.id).collect();
        assert_eq!(views.len(), 2);
        for id in views {
            let concealed: Vec<usize> = doc
                .conceals(id)
                .unwrap_or_default()
                .iter()
                .flat_map(|conceal| conceal.range.clone())
                .collect();
            let expected: &[usize] = if id == focus { &[] } else { &[0, 1, 3, 4] };
            assert_eq!(concealed, expected, "focused: {}", id == focus);
        }
    };
    test_key_sequences(
        &mut app,
        vec![
            (Some(":vsplit<ret>i"), Some(&check)),
            // close the split so that the harness can quit
            (Some("<esc>:q!<ret>"), None),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_at_line_start_skips_task_marker() -> anyhow::Result<()> {
    let mut app = markdown_app("- [ ] to#[d|]#o\n")?;
    test_key_sequence(
        &mut app,
        Some("<esc>IX<esc>"),
        Some(&|app| assert_eq!(text_of(app), "- [ ] Xtodo\n")),
        false,
    )
    .await
}

/// The text area of every screen row (no gutter), trailing whitespace
/// trimmed.
fn screen(app: &Application) -> Vec<String> {
    let (view, doc) = helix_view::current_ref!(app.editor);
    let inner = view.inner_area(doc);
    let buffer = app.test_backend().buffer();
    (inner.top()..inner.bottom())
        .map(|y| {
            (inner.left()..inner.right())
                .map(|x| buffer[(x, y)].symbol.as_str())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

/// Sets `[icons]` (a global) and restores the previous icons when dropped.
struct IconsGuard(std::sync::Arc<helix_view::icons::Icons>);

impl IconsGuard {
    fn set(toml: &str) -> Self {
        let icons: helix_view::icons::Icons = toml::from_str(toml).unwrap();
        Self(helix_view::icons::ICONS.swap(std::sync::Arc::new(icons)))
    }
}

impl Drop for IconsGuard {
    fn drop(&mut self) {
        helix_view::icons::ICONS.store(self.0.clone());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn fences_show_language_icons() -> anyhow::Result<()> {
    // the only test that changes `[icons]`; no other test renders a fence
    // with a language that has an icon
    let _icons = IconsGuard::set("[fs]\nenable = true");
    let mut config = test_config();
    // styles are compared: without this the theme depends on COLORTERM
    config.editor.true_color = true;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_file("render.md", None)
        .with_input_text("```rust\nlet x;\n```\n\n#[a|]#fter\n")
        .build()?;
    test_key_sequences(
        &mut app,
        vec![
            (
                None,
                Some(&|app| {
                    let rows = screen(app);
                    assert_eq!(
                        rows[..5],
                        ["\u{e68b} rust", "let x;", "", "", "after"],
                        "{rows:#?}"
                    );
                    let (view, doc) = helix_view::current_ref!(app.editor);
                    let inner = view.inner_area(doc);
                    let buffer = app.test_backend().buffer();
                    let icon = &buffer[(inner.x, inner.y)];
                    assert_eq!(icon.symbol, "\u{e68b}");
                    assert_eq!(icon.fg, helix_view::graphics::Color::Rgb(0xDE, 0xA5, 0x84));
                    // the language keeps its own style
                    let language = &buffer[(inner.x + 2, inner.y)];
                    assert_eq!(language.symbol, "r");
                    assert_ne!(language.fg, icon.fg);

                    // a config reload turning `[icons.fs]` off
                    helix_view::icons::ICONS.store(std::sync::Arc::new(
                        toml::from_str("[fs]\nenable = false").unwrap(),
                    ));
                }),
            ),
            (
                Some("<esc>"),
                Some(&|app| {
                    // brings back the rules
                    let rows = screen(app);
                    assert_eq!(
                        rows[..5],
                        ["───rust", "let x;", "───", "", "after"],
                        "{rows:#?}"
                    );
                }),
            ),
        ],
        false,
    )
    .await
}
