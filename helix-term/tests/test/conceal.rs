use helix_core::text_annotations::Conceal;
use helix_term::application::Application;

use super::*;

/// Runs `keys` on `input` (selection markup as in other integration tests)
/// with `conceals` set on the focused view, then checks text and selection.
async fn test_concealed(
    input: &str,
    conceals: Vec<Conceal>,
    keys: &str,
    expected: &str,
) -> anyhow::Result<()> {
    let mut app = AppBuilder::new().with_input_text(input).build()?;
    {
        let (view, doc) = helix_view::current!(app.editor);
        doc.set_conceals(view.id, conceals);
    }
    let expected = expected.to_string();
    let check = move |app: &Application| {
        let (view, doc) = helix_view::current_ref!(app.editor);
        let actual = helix_core::test::plain(doc.text().slice(..), doc.selection(view.id));
        assert_eq!(expected, actual);
    };
    test_key_sequence(&mut app, Some(keys), Some(&check), false).await
}

fn bold() -> Vec<Conceal> {
    // "a**b**c"
    vec![
        Conceal::new(1..3).with_group(1),
        Conceal::new(4..6).with_group(1),
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn move_right_skips_concealed() -> anyhow::Result<()> {
    test_concealed("#[a|]#**b**c\n", bold(), "l", "a**#[b|]#**c\n").await?;
    test_concealed("#[a|]#**b**c\n", bold(), "ll", "a**b**#[c|]#\n").await
}

#[tokio::test(flavor = "multi_thread")]
async fn move_left_skips_concealed() -> anyhow::Result<()> {
    test_concealed("a**b**#[c|]#\n", bold(), "h", "a**#[b|]#**c\n").await?;
    test_concealed("a**#[b|]#**c\n", bold(), "h", "#[a|]#**b**c\n").await
}

#[tokio::test(flavor = "multi_thread")]
async fn move_up_uses_visible_columns() -> anyhow::Result<()> {
    test_concealed("a**b**c\nxy#[z|]#\n", bold(), "k", "a**b**#[c|]#\nxyz\n").await
}

#[tokio::test(flavor = "multi_thread")]
async fn till_char_snaps_forward() -> anyhow::Result<()> {
    test_concealed("#[a|]#**b**c\n", bold(), "tc", "#[a**b**c|]#\n").await
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_word_removes_markers() -> anyhow::Result<()> {
    test_concealed("a**#[b|]#**c\n", bold(), "d", "a#[c|]#\n").await
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_partial_keeps_markers() -> anyhow::Result<()> {
    let conceals = vec![
        Conceal::new(0..2).with_group(1),
        Conceal::new(6..8).with_group(1),
    ];
    test_concealed("**b#[old** re|]#st\n", conceals, "d", "**b**#[s|]#t\n").await
}

#[tokio::test(flavor = "multi_thread")]
async fn change_word_removes_markers() -> anyhow::Result<()> {
    test_concealed("a**#[b|]#**c\n", bold(), "cX<esc>", "aX#[c|]#\n").await
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_multi_cursor() -> anyhow::Result<()> {
    // "**b** **d**"
    let conceals = vec![
        Conceal::new(0..2).with_group(1),
        Conceal::new(3..5).with_group(1),
        Conceal::new(6..8).with_group(2),
        Conceal::new(9..11).with_group(2),
    ];
    test_concealed("**#[b|]#** **#(d|)#**\n", conceals, "d", "#[ #(|]#\n|)#").await
}

#[tokio::test(flavor = "multi_thread")]
async fn change_partial_inserts_inside_markers() -> anyhow::Result<()> {
    let conceals = vec![
        Conceal::new(0..2).with_group(1),
        Conceal::new(6..8).with_group(1),
    ];
    test_concealed(
        "**b#[old|]#** rest\n",
        conceals,
        "cX<esc>",
        "**bX**#[ |]#rest\n",
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_two_cursors_in_one_group() -> anyhow::Result<()> {
    // "**ab**": together the cursors cover all of the group's visible text
    let conceals = vec![
        Conceal::new(0..2).with_group(1),
        Conceal::new(4..6).with_group(1),
    ];
    test_concealed("**#[a|]##(b|)#**\n", conceals, "d", "#[\n|]#").await
}

#[tokio::test(flavor = "multi_thread")]
async fn move_down_uses_visible_columns() -> anyhow::Result<()> {
    test_concealed(
        "x#[y|]#z\na**b**c\n",
        bold_line_two(),
        "j",
        "xyz\na**#[b|]#**c\n",
    )
    .await?;
    test_concealed(
        "xy#[z|]#\na**b**c\n",
        bold_line_two(),
        "j",
        "xyz\na**b**#[c|]#\n",
    )
    .await
}

fn bold_line_two() -> Vec<Conceal> {
    // "xyz\na**b**c"
    vec![
        Conceal::new(5..7).with_group(1),
        Conceal::new(8..10).with_group(1),
    ]
}

// Pins current behaviour: `w` is not Markdown-aware. It stops on the
// concealed `**` as a word of its own, and snapping then moves the cursor
// past it, so the second press visibly selects just `b`. Markdown-aware word
// motion is deferred to patch 2.
#[tokio::test(flavor = "multi_thread")]
async fn word_motion_over_concealed_markers() -> anyhow::Result<()> {
    // "word **bold** rest"
    let conceals = || {
        vec![
            Conceal::new(5..7).with_group(1),
            Conceal::new(11..13).with_group(1),
        ]
    };
    let input = "#[w|]#ord **bold** rest\n";
    test_concealed(input, conceals(), "w", "#[word |]#**bold** rest\n").await?;
    test_concealed(input, conceals(), "ww", "word #[**b|]#old** rest\n").await?;
    test_concealed(input, conceals(), "www", "word **#[bold|]#** rest\n").await?;
    test_concealed(input, conceals(), "wwww", "word **bold#[** |]#rest\n").await
}
