//! Printable stand-ins for control characters, so text from a document or
//! any other source never reaches the terminal as raw control sequences.

use std::borrow::Cow;

/// The printable stand-in for a control character (`char::is_control`): C0
/// controls become Unicode Control Pictures (`\x1b` → `␛`), DEL becomes `␡`
/// and C1 controls become U+FFFD. Each is one cell wide, like the control
/// itself in `grapheme_width`. `None` for any other char.
pub fn picture(c: char) -> Option<char> {
    match c {
        '\0'..='\x1f' => char::from_u32(0x2400 + c as u32),
        '\x7f' => Some('\u{2421}'),
        '\u{80}'..='\u{9f}' => Some(char::REPLACEMENT_CHARACTER),
        _ => None,
    }
}

/// The [`picture`] of a grapheme that is a control character. A control
/// character always forms a grapheme of its own (UAX #29 GB4, GB5).
pub fn picture_of(grapheme: &str) -> Option<char> {
    let mut chars = grapheme.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => picture(c),
        _ => None,
    }
}

/// `s` with every control character replaced by its [`picture`]; borrowed
/// when there is none.
pub fn sanitize(s: &str) -> Cow<'_, str> {
    if !s.chars().any(char::is_control) {
        return Cow::Borrowed(s);
    }
    Cow::Owned(s.chars().map(|c| picture(c).unwrap_or(c)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c0_controls_become_control_pictures() {
        assert_eq!(picture('\x00'), Some('\u{2400}'));
        assert_eq!(picture('\x1b'), Some('␛'));
        assert_eq!(picture('\x07'), Some('␇'));
        assert_eq!(picture('\t'), Some('␉'));
        assert_eq!(picture('\x1f'), Some('\u{241f}'));
    }

    #[test]
    fn del_and_c1_controls() {
        assert_eq!(picture('\x7f'), Some('␡'));
        assert_eq!(picture('\u{80}'), Some('\u{fffd}'));
        assert_eq!(picture('\u{9b}'), Some('\u{fffd}'), "CSI");
        assert_eq!(picture('\u{9c}'), Some('\u{fffd}'), "ST");
        assert_eq!(picture('\u{9f}'), Some('\u{fffd}'));
    }

    #[test]
    fn printable_chars_have_no_picture() {
        for c in [' ', 'a', '~', '\u{a0}', 'é', '漢', '\u{2400}'] {
            assert_eq!(picture(c), None, "{c:?}");
        }
    }

    #[test]
    fn sanitize_replaces_every_control() {
        assert_eq!(sanitize("\x1b]2;X\x07"), "␛]2;X␇");
        assert_eq!(sanitize("a\u{9b}31m"), "a\u{fffd}31m");
        assert!(matches!(sanitize("plain"), Cow::Borrowed("plain")));
    }

    #[test]
    fn pictures_keep_the_grapheme_width() {
        use crate::graphemes::grapheme_width;
        for c in ['\x01', '\x1b', '\x7f', '\u{85}', '\u{9c}'] {
            let raw = c.to_string();
            let shown = picture(c).unwrap().to_string();
            assert_eq!(grapheme_width(&raw), grapheme_width(&shown), "{c:?}");
        }
    }

    #[test]
    fn picture_of_a_grapheme() {
        assert_eq!(picture_of("\x1b"), Some('␛'));
        assert_eq!(picture_of("\r"), Some('␍'));
        assert_eq!(picture_of("\u{9c}"), Some('\u{fffd}'));
        assert_eq!(picture_of("a"), None);
        assert_eq!(picture_of("e\u{301}"), None);
        assert_eq!(picture_of(""), None);
    }
}
