use helix_term::application::Application;

use super::*;

const TEXT: &str = "safe line\n\x1b]2;PWNED\x07 a\u{9b}b\x7f\n";

fn screen_rows(app: &Application) -> Vec<String> {
    let buffer = app.test_backend().buffer();
    let area = buffer.area;
    (area.top()..area.bottom())
        .map(|y| {
            (area.left()..area.right())
                .map(|x| buffer[(x, y)].symbol.as_str())
                .collect()
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn control_characters_are_drawn_as_pictures() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("ctl.txt");
    std::fs::write(&path, TEXT)?;
    let mut app = AppBuilder::new()
        .with_config(test_config())
        .with_file(path, None)
        .build()?;
    test_key_sequence(
        &mut app,
        None,
        Some(&|app| {
            let rows = screen_rows(app);
            assert!(
                rows.iter()
                    .any(|row| row.contains("␛]2;PWNED␇ a\u{fffd}b␡")),
                "{rows:#?}"
            );
            // nothing the backend draws from cells can start an escape sequence
            for row in &rows {
                assert!(!row.contains(char::is_control), "{row:?}");
            }
            let safe = rows
                .iter()
                .position(|row| row.contains("safe line"))
                .unwrap();
            let col = |row: &str, s: &str| row[..row.find(s).unwrap()].chars().count();
            // one cell per control character: the layout is unchanged
            assert_eq!(
                col(&rows[safe + 1], "␛"),
                col(&rows[safe], "safe line"),
                "same text column"
            );
        }),
        false,
    )
    .await
}
