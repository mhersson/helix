use helix_term::application::Application;
use helix_view::{doc, editor::Severity, input::parse_macro};
use tokio_stream::wrappers::UnboundedReceiverStream;

#[cfg(windows)]
use crossterm::event::{Event, KeyEvent};
#[cfg(not(windows))]
use termina::event::{Event, KeyEvent};

use super::*;

#[cfg(windows)]
const FOCUS_GAINED: Event = Event::FocusGained;
#[cfg(not(windows))]
const FOCUS_GAINED: Event = Event::FocusIn;

/// Feed `keys` and then a terminal focus-gained event to the app and run it until idle. Temp files
/// live outside the workspace, so they are not watched and focus-gained is what re-checks them.
async fn send_keys_then_focus(app: &mut Application, keys: &str) -> anyhow::Result<()> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);
    for key_event in parse_macro(keys)? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    tx.send(Ok(FOCUS_GAINED))?;
    app.event_loop_until_idle(&mut rx_stream).await;
    Ok(())
}

/// Finish the test: quit (discarding changes) and close the app.
async fn quit(mut app: Application) -> anyhow::Result<()> {
    test_key_sequence(&mut app, Some("<esc>"), None, false).await
}

fn write_externally(file: &tempfile::NamedTempFile, content: &str) -> anyhow::Result<()> {
    let mtime = file.as_file().metadata()?.modified()?;
    std::fs::write(file.path(), content)?;
    // make sure the external write has a different mtime from the one seen on open, however
    // coarse the file system's timestamps are
    let mtime = filetime::FileTime::from_system_time(mtime + std::time::Duration::from_secs(2));
    filetime::set_file_mtime(file.path(), mtime)?;
    Ok(())
}

fn auto_reload_config(prompt_if_modified: bool) -> Config {
    let mut config = test_config();
    config.editor.auto_reload.prompt_if_modified = prompt_if_modified;
    config
}

#[tokio::test(flavor = "multi_thread")]
async fn reloads_unmodified_buffer_on_focus() -> anyhow::Result<()> {
    let file = temp_file_with_contents("one\n")?;
    let mut app = AppBuilder::new()
        .with_file(file.path(), None)
        .with_config(auto_reload_config(true))
        .build()?;

    write_externally(&file, "two\n")?;
    send_keys_then_focus(&mut app, "").await?;

    let doc = doc!(app.editor);
    assert_eq!(doc.text().to_string(), "two\n");
    assert!(!doc.is_modified());
    let (status, severity) = app.editor.get_status().expect("a status message");
    assert!(status.contains("reloaded"), "status: {status}");
    assert_eq!(*severity, Severity::Info);

    quit(app).await
}

#[tokio::test(flavor = "multi_thread")]
async fn warns_instead_of_reloading_modified_buffer() -> anyhow::Result<()> {
    let file = temp_file_with_contents("one\n")?;
    let mut app = AppBuilder::new()
        .with_file(file.path(), None)
        .with_config(auto_reload_config(false))
        .build()?;

    write_externally(&file, "two\n")?;
    send_keys_then_focus(&mut app, "iedit <esc>").await?;

    let doc = doc!(app.editor);
    assert_eq!(doc.text().to_string(), "edit one\n");
    assert!(doc.is_modified());
    let (status, severity) = app.editor.get_status().expect("a status message");
    assert!(status.contains("changed externally"), "status: {status}");
    assert_eq!(*severity, Severity::Warning);
    quit(app).await?;

    // the buffer is left alone and the file on disk keeps the external change
    assert_eq!(std::fs::read_to_string(file.path())?, "two\n");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn prompts_before_reloading_modified_buffer() -> anyhow::Result<()> {
    let file = temp_file_with_contents("one\n")?;
    let mut app = AppBuilder::new()
        .with_file(file.path(), None)
        .with_config(auto_reload_config(true))
        .build()?;

    write_externally(&file, "two\n")?;
    send_keys_then_focus(&mut app, "iedit <esc>").await?;

    // the prompt is open, nothing was reloaded yet
    assert_eq!(doc!(app.editor).text().to_string(), "edit one\n");

    test_key_sequence(
        &mut app,
        // accept the prompt
        Some("<ret>"),
        Some(&|app| {
            let doc = doc!(app.editor);
            assert_eq!(doc.text().to_string(), "two\n");
            assert!(!doc.is_modified());
        }),
        false,
    )
    .await
}
