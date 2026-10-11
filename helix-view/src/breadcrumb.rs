/// The part of an LSP symbol name the breadcrumb bar shows: its first
/// non-blank line, trimmed. The bar is a single row, and marksman names a
/// setext heading by its source text, underline included (`"Title\n====="`).
pub fn symbol_name(name: String) -> Box<str> {
    let line = name
        .split(['\n', '\r'])
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    if line.len() == name.len() {
        name.into_boxed_str()
    } else {
        line.into()
    }
}

#[cfg(test)]
mod tests {
    use super::symbol_name;

    #[test]
    fn single_line_names_are_kept() {
        assert_eq!(&*symbol_name("ATX Heading".into()), "ATX Heading");
        assert_eq!(&*symbol_name("impl Foo for Bar".into()), "impl Foo for Bar");
    }

    #[test]
    fn setext_underlines_are_dropped() {
        assert_eq!(&*symbol_name("My Title\n========".into()), "My Title");
        assert_eq!(&*symbol_name("Sub Heading\r\n---".into()), "Sub Heading");
        assert_eq!(&*symbol_name("Two\nlines\n===".into()), "Two");
    }

    #[test]
    fn blank_lines_and_padding_are_skipped() {
        assert_eq!(&*symbol_name("\n  Title  \n===".into()), "Title");
        assert_eq!(&*symbol_name("Old Mac\rline".into()), "Old Mac");
        assert_eq!(&*symbol_name(" \n ".into()), "");
    }

    mod position {
        use std::sync::Arc;

        use arc_swap::ArcSwap;
        use helix_core::{syntax, Rope, Selection};
        use helix_lsp::{lsp, OffsetEncoding};

        use crate::{editor::Config, Document, ViewId};

        #[allow(deprecated)]
        fn symbol(
            name: &str,
            start: (u32, u32),
            end: (u32, u32),
            children: Vec<lsp::DocumentSymbol>,
        ) -> lsp::DocumentSymbol {
            let range = lsp::Range::new(
                lsp::Position::new(start.0, start.1),
                lsp::Position::new(end.0, end.1),
            );
            lsp::DocumentSymbol {
                name: name.into(),
                detail: None,
                kind: lsp::SymbolKind::STRING,
                tags: None,
                deprecated: None,
                range,
                selection_range: range,
                children: Some(children),
            }
        }

        /// The breadcrumb for a cursor on `(line, character)` of four
        /// two-letter lines.
        fn crumbs_at(
            symbols: Vec<lsp::DocumentSymbol>,
            line: usize,
            character: usize,
        ) -> Vec<String> {
            let mut doc = Document::from(
                Rope::from("ab\ncd\nef\ngh\n"),
                None,
                Arc::new(ArcSwap::new(Arc::new(Config::default()))),
                Arc::new(ArcSwap::from_pointee(syntax::Loader::default())),
            );
            let view = ViewId::default();
            doc.set_selection(view, Selection::point(line * 3 + character));
            doc.set_document_symbols(symbols, OffsetEncoding::Utf8);
            doc.update_breadcrumbs_for_view(view);
            doc.breadcrumbs[&view]
                .iter()
                .map(|crumb| crumb.name.to_string())
                .collect()
        }

        /// marksman ends a section where the next heading starts.
        #[test]
        fn adjacent_sections_meet_at_the_next_start() {
            let sections = || {
                vec![
                    symbol("A", (0, 0), (2, 0), vec![]),
                    symbol("B", (2, 0), (4, 0), vec![]),
                ]
            };
            assert_eq!(crumbs_at(sections(), 1, 2), ["A"]);
            assert_eq!(crumbs_at(sections(), 2, 0), ["B"]);
        }

        #[test]
        fn nested_sections_meet_at_the_next_start() {
            let sections = || {
                vec![symbol(
                    "P",
                    (0, 0),
                    (4, 0),
                    vec![
                        symbol("C1", (0, 0), (2, 0), vec![]),
                        symbol("C2", (2, 0), (4, 0), vec![]),
                    ],
                )]
            };
            assert_eq!(crumbs_at(sections(), 1, 0), ["P", "C1"]);
            assert_eq!(crumbs_at(sections(), 2, 0), ["P", "C2"]);
        }

        /// gopls ends a function right after its closing brace.
        #[test]
        fn the_end_position_is_outside_the_symbol() {
            let function = || vec![symbol("F", (0, 0), (1, 1), vec![])];
            assert_eq!(crumbs_at(function(), 1, 0), ["F"]);
            assert!(crumbs_at(function(), 1, 1).is_empty());
        }

        #[test]
        fn empty_ranges_hold_their_own_position() {
            let symbols = || {
                vec![symbol(
                    "P",
                    (0, 0),
                    (4, 0),
                    vec![symbol("Z", (1, 1), (1, 1), vec![])],
                )]
            };
            assert_eq!(crumbs_at(symbols(), 1, 0), ["P"]);
            assert_eq!(crumbs_at(symbols(), 1, 1), ["P", "Z"]);
            assert_eq!(crumbs_at(symbols(), 1, 2), ["P"]);
        }
    }
}
