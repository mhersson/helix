use helix_term::application::Application;
use helix_view::theme;

use super::*;

/// An app whose config sets `theme`, or no theme at all.
fn app_with_theme(theme: Option<&str>) -> anyhow::Result<Application> {
    let mut config = test_config();
    config.theme = theme.map(|name| theme::Config::Constant(name.into()));
    // themes needing true colour fall back without it; don't depend on COLORTERM
    config.editor.true_color = true;
    AppBuilder::new().with_config(config).build()
}

#[tokio::test(flavor = "multi_thread")]
async fn unset_theme_is_catppuccin_mocha() -> anyhow::Result<()> {
    let app = app_with_theme(None)?;
    assert_eq!(app.editor.theme.name(), "catppuccin_mocha");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn default_theme_is_the_built_in_theme() -> anyhow::Result<()> {
    let app = app_with_theme(Some("default"))?;
    assert_eq!(app.editor.theme.name(), "default");
    assert_eq!(
        app.editor.theme.get("ui.background"),
        theme::DEFAULT_THEME.get("ui.background")
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn explicit_theme_is_unchanged() -> anyhow::Result<()> {
    let app = app_with_theme(Some("solarized_light"))?;
    assert_eq!(app.editor.theme.name(), "solarized_light");
    Ok(())
}

/// The only test that reloads the config: `:config-reload` reads the
/// process-wide config file, which nothing else in this binary sets.
#[tokio::test(flavor = "multi_thread")]
async fn config_reload_without_theme_is_catppuccin_mocha() -> anyhow::Result<()> {
    let config_file = temp_file_with_contents(indoc::indoc! {r#"
        [editor]
        true-color = true

        [editor.lsp]
        enable = false

        [editor.file-watcher]
        enable = false
    "#})?;
    helix_loader::initialize_config_file(Some(config_file.path().to_path_buf()));
    assert_eq!(helix_loader::config_file(), config_file.path());

    let mut app = app_with_theme(Some("solarized_light"))?;
    test_key_sequence(
        &mut app,
        Some(":config-reload<ret>"),
        Some(&|app| {
            assert_status_not_error(&app.editor);
            assert_eq!(app.editor.theme.name(), "catppuccin_mocha");
        }),
        false,
    )
    .await
}
