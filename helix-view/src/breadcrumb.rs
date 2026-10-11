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
}
