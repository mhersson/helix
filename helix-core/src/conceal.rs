//! Selection and editing helpers for concealed text, see [`Conceal`].

use crate::graphemes::{next_grapheme_boundary, prev_grapheme_boundary};
use crate::movement::Direction;
use crate::text_annotations::Conceal;
use crate::{RopeSlice, Selection, SmallVec};

/// Returns the conceal containing `pos`, if any. `conceals` must be sorted.
pub fn concealed_at(conceals: &[Conceal], pos: usize) -> Option<&Conceal> {
    let i = conceals.partition_point(|conceal| conceal.range.end <= pos);
    conceals.get(i).filter(|conceal| conceal.range.start <= pos)
}

fn snap_pos(text: RopeSlice, mut pos: usize, dir: Direction, conceals: &[Conceal]) -> usize {
    while let Some(conceal) = concealed_at(conceals, pos) {
        pos = match dir {
            Direction::Forward => conceal.range.end.min(text.len_chars()),
            Direction::Backward if conceal.range.start > 0 => {
                prev_grapheme_boundary(text, conceal.range.start)
            }
            // nothing visible before the document start
            Direction::Backward => {
                return snap_pos(text, conceal.range.end, Direction::Forward, conceals)
            }
        };
    }
    pos
}

/// Moves every block cursor of `new` that lies in concealed text to the
/// nearest visible grapheme, in the direction it moved from `old`.
pub fn snap_selection(
    text: RopeSlice,
    old: &Selection,
    new: Selection,
    conceals: &[Conceal],
) -> Selection {
    if conceals.is_empty() {
        return new;
    }
    let same_len = old.len() == new.len();
    let ranges: SmallVec<_> = new
        .ranges()
        .iter()
        .enumerate()
        .map(|(i, &range)| {
            let cursor = range.cursor(text);
            if concealed_at(conceals, cursor).is_none() {
                return range;
            }
            let dir = match old.ranges().get(i) {
                Some(old) if same_len && old.head > range.head => Direction::Backward,
                _ => Direction::Forward,
            };
            let target = snap_pos(text, cursor, dir, conceals);
            let extend = range.to() > next_grapheme_boundary(text, range.from());
            range.put_cursor(text, target, extend)
        })
        .collect();
    Selection::new(ranges, new.primary_index())
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{smallvec, Range, Rope};

    fn snap(text: &str, conceals: &[Conceal], old: Selection, new: Selection) -> Selection {
        let text = Rope::from(text);
        snap_selection(text.slice(..), &old, new, conceals)
    }

    const BOLD: &str = "a**b**c\n";

    fn bold_conceals() -> [Conceal; 2] {
        [Conceal::new(1..3), Conceal::new(4..6)]
    }

    #[test]
    fn snaps_forward() {
        let new = snap(
            BOLD,
            &bold_conceals(),
            Selection::point(0),
            Selection::point(1),
        );
        assert_eq!(new, Selection::point(3));
    }

    #[test]
    fn snaps_backward() {
        let new = snap(
            BOLD,
            &bold_conceals(),
            Selection::point(3),
            Selection::point(2),
        );
        assert_eq!(new, Selection::point(0));
    }

    #[test]
    fn unchanged_cursor_snaps_forward() {
        let new = snap(
            BOLD,
            &bold_conceals(),
            Selection::point(2),
            Selection::point(2),
        );
        assert_eq!(new, Selection::point(3));
    }

    #[test]
    fn visible_cursor_untouched() {
        let new = snap(
            BOLD,
            &bold_conceals(),
            Selection::point(0),
            Selection::point(3),
        );
        assert_eq!(new, Selection::point(3));
    }

    #[test]
    fn adjacent_conceals() {
        let conceals = [Conceal::new(1..2), Conceal::new(2..4)];
        let fwd = snap(
            "a***b\n",
            &conceals,
            Selection::point(0),
            Selection::point(1),
        );
        assert_eq!(fwd, Selection::point(4));
        let back = snap(
            "a***b\n",
            &conceals,
            Selection::point(4),
            Selection::point(3),
        );
        assert_eq!(back, Selection::point(0));
    }

    #[test]
    fn backward_at_document_start_snaps_forward() {
        let new = snap(
            "**a\n",
            &[Conceal::new(0..2)],
            Selection::point(2),
            Selection::point(1),
        );
        assert_eq!(new, Selection::point(2));
    }

    #[test]
    fn forward_at_document_end() {
        let new = snap(
            "a**",
            &[Conceal::new(1..3)],
            Selection::point(0),
            Selection::point(1),
        );
        assert_eq!(new, Selection::point(3));
    }

    #[test]
    fn extended_range_keeps_anchor() {
        let new = snap(
            BOLD,
            &bold_conceals(),
            Selection::single(0, 1),
            Selection::single(0, 2),
        );
        assert_eq!(new, Selection::single(0, 4));
    }

    #[test]
    fn multiple_cursors() {
        let old = Selection::new(smallvec![Range::point(0), Range::point(3)], 0);
        let new = Selection::new(smallvec![Range::point(1), Range::point(4)], 1);
        let snapped = snap(BOLD, &bold_conceals(), old, new);
        assert_eq!(
            snapped,
            Selection::new(smallvec![Range::point(3), Range::point(6)], 1)
        );
    }

    #[test]
    fn no_conceals_is_identity() {
        let new = snap(BOLD, &[], Selection::point(0), Selection::point(1));
        assert_eq!(new, Selection::point(1));
    }
}
