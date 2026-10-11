#[cfg(feature = "integration")]
mod test {
    mod helpers;

    use helix_core::{syntax::config::AutoPairConfig, Selection};
    use helix_term::config::Config;

    use indoc::indoc;

    use self::helpers::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn hello_world() -> anyhow::Result<()> {
        test(("#[\n|]#", "ihello world<esc>", "hello world#[|\n]#")).await?;
        Ok(())
    }

    mod auto_pairs;
    mod auto_reload;
    mod breadcrumb;
    mod command_line;
    mod commands;
    mod conceal;
    mod control_chars;
    mod markdown_diagrams;
    mod markdown_headings;
    mod markdown_images;
    mod markdown_render;
    mod markdown_tables;
    mod movement;
    mod splits;
    mod theme;
}
