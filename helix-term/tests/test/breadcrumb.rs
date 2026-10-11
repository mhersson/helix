use helix_lsp::{lsp, OffsetEncoding};
use helix_term::application::Application;

use super::*;

fn setup(breadcrumb: bool, cursorcolumn: bool) -> anyhow::Result<Application> {
    let mut config = test_config();
    config.editor.breadcrumb.enable = breadcrumb;
    config.editor.cursorcolumn = cursorcolumn;
    AppBuilder::new()
        .with_config(config)
        .with_input_text("#[h|]#ello\nworld\n")
        .build()
}

#[tokio::test(flavor = "multi_thread")]
async fn cursorcolumn_skips_the_breadcrumb_bar() -> anyhow::Result<()> {
    let mut app = setup(true, true)?;
    test_key_sequence(
        &mut app,
        None,
        Some(&|app| {
            let (view, doc) = helix_view::current_ref!(app.editor);
            let inner = view.inner_area(doc);
            assert_eq!(inner.y, view.area.y + 1);
            let column = app.editor.theme.get("ui.cursorline.primary").bg;
            assert!(column.is_some());

            let buffer = app.test_backend().buffer();
            assert_ne!(Some(buffer[(inner.x, view.area.y)].bg), column);
            assert_eq!(Some(buffer[(inner.x, inner.y + 1)].bg), column);
        }),
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn disabled_breadcrumbs_are_not_updated() -> anyhow::Result<()> {
    let mut app = setup(false, false)?;
    {
        let doc = helix_view::doc_mut!(app.editor);
        let range = lsp::Range::new(lsp::Position::new(0, 0), lsp::Position::new(2, 0));
        #[allow(deprecated)]
        let symbol = lsp::DocumentSymbol {
            name: "everything".into(),
            detail: None,
            kind: lsp::SymbolKind::MODULE,
            tags: None,
            deprecated: None,
            range,
            selection_range: range,
            children: None,
        };
        doc.set_document_symbols(vec![symbol], OffsetEncoding::Utf8);
    }

    test_key_sequences(
        &mut app,
        vec![
            (
                Some("j"),
                Some(&|app| assert!(helix_view::doc!(app.editor).breadcrumbs.is_empty())),
            ),
            (
                Some("ix<esc>"),
                Some(&|app| assert!(helix_view::doc!(app.editor).breadcrumbs.is_empty())),
            ),
        ],
        false,
    )
    .await
}
