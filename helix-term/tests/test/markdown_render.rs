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
