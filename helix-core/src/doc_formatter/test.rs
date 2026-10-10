use crate::doc_formatter::{DocumentFormatter, TextFormat};
use crate::text_annotations::{Conceal, InlineAnnotation, Overlay, TextAnnotations};

impl TextFormat {
    fn new_test(softwrap: bool) -> Self {
        TextFormat {
            soft_wrap: softwrap,
            tab_width: 2,
            max_wrap: 3,
            max_indent_retain: 4,
            wrap_indicator: ".".into(),
            wrap_indicator_highlight: None,
            // use a prime number to allow lining up too often with repeat
            viewport_width: 17,
            soft_wrap_at_text_width: false,
        }
    }
}

impl<'t> DocumentFormatter<'t> {
    fn collect_to_str(&mut self) -> String {
        use std::fmt::Write;
        let mut res = String::new();
        let viewport_width = self.text_fmt.viewport_width;
        let soft_wrap_at_text_width = self.text_fmt.soft_wrap_at_text_width;
        let mut line = 0;

        for grapheme in self {
            if grapheme.visual_pos.row != line {
                line += 1;
                assert_eq!(grapheme.visual_pos.row, line);
                write!(res, "\n{}", ".".repeat(grapheme.visual_pos.col)).unwrap();
            }
            if !soft_wrap_at_text_width {
                assert!(
                    grapheme.visual_pos.col <= viewport_width as usize,
                    "softwrapped failed {}<={viewport_width}",
                    grapheme.visual_pos.col
                );
            }
            write!(res, "{}", grapheme.raw).unwrap();
        }

        res
    }
}

fn softwrap_text(text: &str) -> String {
    DocumentFormatter::new_at_prev_checkpoint(
        text.into(),
        &TextFormat::new_test(true),
        &TextAnnotations::default(),
        0,
    )
    .collect_to_str()
}

#[test]
fn basic_softwrap() {
    assert_eq!(
        softwrap_text(&"foo ".repeat(10)),
        "foo foo foo foo \n.foo foo foo foo \n.foo foo  "
    );
    assert_eq!(
        softwrap_text(&"fooo ".repeat(10)),
        "fooo fooo fooo \n.fooo fooo fooo \n.fooo fooo fooo \n.fooo  "
    );

    // check that we don't wrap unnecessarily
    assert_eq!(softwrap_text("\t\txxxx1xxxx2xx\n"), "    xxxx1xxxx2xx \n ");
}

#[test]
fn softwrap_indentation() {
    assert_eq!(
        softwrap_text("\t\tfoo1 foo2 foo3 foo4 foo5 foo6\n"),
        "    foo1 foo2 \n.....foo3 foo4 \n.....foo5 foo6 \n "
    );
    assert_eq!(
        softwrap_text("\t\t\tfoo1 foo2 foo3 foo4 foo5 foo6\n"),
        "      foo1 foo2 \n.foo3 foo4 foo5 \n.foo6 \n "
    );
}

#[test]
fn long_word_softwrap() {
    assert_eq!(
        softwrap_text("\t\txxxx1xxxx2xxxx3xxxx4xxxx5xxxx6xxxx7xxxx8xxxx9xxx\n"),
        "    xxxx1xxxx2xxx\n.....x3xxxx4xxxx5\n.....xxxx6xxxx7xx\n.....xx8xxxx9xxx \n "
    );
    assert_eq!(
        softwrap_text("xxxxxxxx1xxxx2xxx\n"),
        "xxxxxxxx1xxxx2xxx\n. \n "
    );
    assert_eq!(
        softwrap_text("\t\txxxx1xxxx 2xxxx3xxxx4xxxx5xxxx6xxxx7xxxx8xxxx9xxx\n"),
        "    xxxx1xxxx \n.....2xxxx3xxxx4x\n.....xxx5xxxx6xxx\n.....x7xxxx8xxxx9\n.....xxx \n "
    );
    assert_eq!(
        softwrap_text("\t\txxxx1xxx 2xxxx3xxxx4xxxx5xxxx6xxxx7xxxx8xxxx9xxx\n"),
        "    xxxx1xxx 2xxx\n.....x3xxxx4xxxx5\n.....xxxx6xxxx7xx\n.....xx8xxxx9xxx \n "
    );
}

#[test]
fn softwrap_multichar_grapheme() {
    assert_eq!(
        softwrap_text("xxxx xxxx xxx a\u{0301}bc\n"),
        "xxxx xxxx xxx \n.ábc \n "
    )
}

fn softwrap_text_at_text_width(text: &str) -> String {
    let mut text_fmt = TextFormat::new_test(true);
    text_fmt.soft_wrap_at_text_width = true;
    let annotations = TextAnnotations::default();
    let mut formatter =
        DocumentFormatter::new_at_prev_checkpoint(text.into(), &text_fmt, &annotations, 0);
    formatter.collect_to_str()
}
#[test]
fn long_word_softwrap_text_width() {
    assert_eq!(
        softwrap_text_at_text_width("xxxxxxxx1xxxx2xxx\nxxxxxxxx1xxxx2xxx"),
        "xxxxxxxx1xxxx2xxx \nxxxxxxxx1xxxx2xxx "
    );
}

fn overlay_text(text: &str, char_pos: usize, softwrap: bool, overlays: &[Overlay]) -> String {
    DocumentFormatter::new_at_prev_checkpoint(
        text.into(),
        &TextFormat::new_test(softwrap),
        TextAnnotations::default().add_overlay(overlays, None),
        char_pos,
    )
    .collect_to_str()
}

#[test]
fn overlay() {
    assert_eq!(
        overlay_text(
            "foobar",
            0,
            false,
            &[Overlay::new(0, "X"), Overlay::new(2, "\t")],
        ),
        "Xo  bar "
    );
    assert_eq!(
        overlay_text(
            &"foo ".repeat(10),
            0,
            true,
            &[
                Overlay::new(2, "\t"),
                Overlay::new(5, "\t"),
                Overlay::new(16, "X"),
            ]
        ),
        "fo   f  o foo \n.foo Xoo foo foo \n.foo foo foo  "
    );
}

fn annotate_text(text: &str, softwrap: bool, annotations: &[InlineAnnotation]) -> String {
    DocumentFormatter::new_at_prev_checkpoint(
        text.into(),
        &TextFormat::new_test(softwrap),
        TextAnnotations::default().add_inline_annotations(annotations, None),
        0,
    )
    .collect_to_str()
}

#[test]
fn annotation() {
    assert_eq!(
        annotate_text("bar", false, &[InlineAnnotation::new(0, "foo")]),
        "foobar "
    );
    assert_eq!(
        annotate_text(
            &"foo ".repeat(10),
            true,
            &[InlineAnnotation::new(0, "foo ")]
        ),
        "foo foo foo foo \n.foo foo foo foo \n.foo foo foo  "
    );
}

#[test]
fn annotation_and_overlay() {
    let annotations = [InlineAnnotation {
        char_idx: 0,
        text: "fooo".into(),
    }];
    let overlay = [Overlay {
        char_idx: 0,
        grapheme: "\t".into(),
    }];
    assert_eq!(
        DocumentFormatter::new_at_prev_checkpoint(
            "bbar".into(),
            &TextFormat::new_test(false),
            TextAnnotations::default()
                .add_inline_annotations(annotations.as_slice(), None)
                .add_overlay(overlay.as_slice(), None),
            0,
        )
        .collect_to_str(),
        "fooo  bar "
    );
}

fn conceal_text(text: &str, softwrap: bool, conceals: &[Conceal]) -> String {
    let mut annotations = TextAnnotations::default();
    annotations.set_conceals(conceals, None);
    let text_fmt = TextFormat::new_test(softwrap);
    let formatted =
        DocumentFormatter::new_at_prev_checkpoint(text.into(), &text_fmt, &annotations, 0)
            .collect_to_str();
    formatted
}

#[test]
fn conceal_hides_text() {
    let conceals = [Conceal::new(1..3), Conceal::new(4..6)];
    assert_eq!(conceal_text("a**b**c", false, &conceals), "abc ");
}

#[test]
fn conceal_replacement() {
    let conceals = [
        Conceal::new(1..3).with_replacement("<"),
        Conceal::new(4..6).with_replacement(">"),
    ];
    assert_eq!(conceal_text("a**b**c", false, &conceals), "a<b>c ");
}

#[test]
fn conceal_adjacent_and_line_bounds() {
    assert_eq!(
        conceal_text("a***b", false, &[Conceal::new(1..2), Conceal::new(2..4)]),
        "ab "
    );
    assert_eq!(conceal_text("**ab", false, &[Conceal::new(0..2)]), "ab ");
    assert_eq!(conceal_text("ab**", false, &[Conceal::new(2..4)]), "ab ");
}

#[test]
fn conceal_stops_at_newline() {
    assert_eq!(
        conceal_text("a**\nb", false, &[Conceal::new(1..4)]),
        "a \nb "
    );
}

#[test]
fn conceal_stops_at_crlf() {
    assert_eq!(
        conceal_text("a**\r\nb", false, &[Conceal::new(1..5)]),
        "a \nb "
    );
}

#[test]
fn conceal_past_eof() {
    assert_eq!(conceal_text("ab", false, &[Conceal::new(1..5)]), "a ");
}

#[test]
fn conceal_softwrap_uses_visible_width() {
    let conceals = [Conceal::new(0..2), Conceal::new(6..8)];
    // 18 raw chars would wrap at width 17, 14 visible chars do not
    assert_eq!(
        conceal_text("**aaaa** bbbb cccc\n", true, &conceals),
        "aaaa bbbb cccc \n "
    );
    assert_eq!(
        conceal_text("**aaaa** bbbb cccc\n", false, &conceals),
        "aaaa bbbb cccc \n "
    );
}

#[test]
fn conceal_drops_annotations_inside() {
    let conceals = [Conceal::new(1..3)];
    let overlays = [Overlay::new(2, "X"), Overlay::new(3, "Y")];
    let mut annotations = TextAnnotations::default();
    annotations
        .set_conceals(&conceals, None)
        .add_overlay(&overlays, None);
    let text_fmt = TextFormat::new_test(false);
    assert_eq!(
        DocumentFormatter::new_at_prev_checkpoint("a**b".into(), &text_fmt, &annotations, 0)
            .collect_to_str(),
        "aY "
    );

    let inline = [InlineAnnotation::new(2, "I"), InlineAnnotation::new(3, "J")];
    let mut annotations = TextAnnotations::default();
    annotations
        .set_conceals(&conceals, None)
        .add_inline_annotations(&inline, None);
    assert_eq!(
        DocumentFormatter::new_at_prev_checkpoint("a**b".into(), &text_fmt, &annotations, 0)
            .collect_to_str(),
        "aJb "
    );
}

#[test]
fn conceal_char_idx_accounting() {
    let conceals = [Conceal::new(1..3).with_replacement("<"), Conceal::new(4..6)];
    let mut annotations = TextAnnotations::default();
    annotations.set_conceals(&conceals, None);
    let text_fmt = TextFormat::new_test(false);
    let graphemes: Vec<_> =
        DocumentFormatter::new_at_prev_checkpoint("a**b**c".into(), &text_fmt, &annotations, 0)
            .map(|g| (g.char_idx, g.is_virtual(), g.visual_pos.col))
            .collect();
    assert_eq!(
        graphemes,
        [
            (0, false, 0), // a
            (1, true, 1),  // <
            (3, false, 2), // b
            (6, false, 3), // c
            (7, false, 4), // EOF
        ]
    );
}
