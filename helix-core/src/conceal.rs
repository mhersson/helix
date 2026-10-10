//! Selection and editing helpers for concealed text, see [`Conceal`].

use crate::graphemes::{next_grapheme_boundary, prev_grapheme_boundary};
use crate::movement::Direction;
use crate::text_annotations::Conceal;
use crate::{RopeSlice, Selection, SmallVec};
use std::collections::{BTreeMap, HashSet};
use std::ops::Range;

/// Returns the conceal containing `pos`, if any. `conceals` must be sorted.
pub fn concealed_at(conceals: &[Conceal], pos: usize) -> Option<&Conceal> {
    let i = conceals.partition_point(|conceal| conceal.range.end <= pos);
    conceals.get(i).filter(|conceal| conceal.range.start <= pos)
}

fn snap_pos(text: RopeSlice, mut pos: usize, dir: Direction, conceals: &[Conceal]) -> usize {
    while let Some(conceal) = concealed_at(conceals, pos) {
        pos = match dir {
            // a stale conceal may extend past the end of the text
            Direction::Forward if conceal.range.end >= text.len_chars() => return text.len_chars(),
            Direction::Forward => conceal.range.end,
            Direction::Backward if conceal.range.start > 0 => {
                prev_grapheme_boundary(text, conceal.range.start)
            }
            // nothing visible before the document start
            Direction::Backward => {
                return snap_pos(text, conceal.range.start, Direction::Forward, conceals)
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

/// Conceals overlapping `span`. `conceals` must be sorted.
fn overlapping(conceals: &[Conceal], span: Range<usize>) -> impl Iterator<Item = &Conceal> {
    let start = conceals.partition_point(|conceal| conceal.range.end <= span.start);
    conceals[start..]
        .iter()
        .take_while(move |conceal| conceal.range.start < span.end)
}

/// Whether `range` contains every visible (not concealed) char of `span`.
/// A span without visible chars counts as covered if `range` overlaps it.
fn covers_visible(range: &Range<usize>, span: &Range<usize>, conceals: &[Conceal]) -> bool {
    let mut first_visible = None;
    let mut visible_end = None;
    let mut pos = span.start;
    for conceal in overlapping(conceals, span.clone()) {
        if conceal.range.start > pos {
            first_visible.get_or_insert(pos);
            visible_end = Some(conceal.range.start);
        }
        pos = pos.max(conceal.range.end);
    }
    if pos < span.end {
        first_visible.get_or_insert(pos);
        visible_end = Some(span.end);
    }
    match (first_visible, visible_end) {
        (Some(first), Some(end)) => range.start <= first && range.end >= end,
        _ => range.start < span.end && range.end > span.start,
    }
}

/// The char ranges to delete when deleting `range`, so that what is deleted
/// matches what is visible:
///
/// * a group whose visible content is entirely deleted is deleted with its
///   markers (repeated until no more groups are added, for nested groups),
/// * markers of a group whose content is only partly deleted are kept,
/// * concealed text without a group is deleted like any other text.
pub fn deletion_ranges(range: Range<usize>, conceals: &[Conceal]) -> Vec<Range<usize>> {
    if range.is_empty() || conceals.is_empty() {
        return vec![range];
    }

    let mut spans: BTreeMap<u32, Range<usize>> = BTreeMap::new();
    for conceal in conceals {
        if let Some(group) = conceal.group {
            spans
                .entry(group)
                .and_modify(|span| {
                    span.start = span.start.min(conceal.range.start);
                    span.end = span.end.max(conceal.range.end);
                })
                .or_insert_with(|| conceal.range.clone());
        }
    }

    let mut range = range;
    loop {
        let mut grown = false;
        for span in spans.values() {
            let outside = span.start < range.start || span.end > range.end;
            if outside && covers_visible(&range, span, conceals) {
                range = range.start.min(span.start)..range.end.max(span.end);
                grown = true;
            }
        }
        if !grown {
            break;
        }
    }

    let kept: HashSet<u32> = spans
        .iter()
        .filter(|(_, span)| !covers_visible(&range, span, conceals))
        .map(|(&group, _)| group)
        .collect();

    let mut deletions = Vec::new();
    let mut pos = range.start;
    for conceal in overlapping(conceals, range.clone()) {
        if !conceal.group.is_some_and(|group| kept.contains(&group)) {
            continue;
        }
        let start = conceal.range.start.max(range.start);
        let end = conceal.range.end.min(range.end);
        if start > pos {
            deletions.push(pos..start);
        }
        pos = pos.max(end);
    }
    if pos < range.end {
        deletions.push(pos..range.end);
    }
    deletions
}

#[cfg(test)]
// expected deletions are lists of ranges, often of length one
#[allow(clippy::single_range_in_vec_init)]
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

    // "**bold** rest"
    fn bold_group() -> [Conceal; 2] {
        [
            Conceal::new(0..2).with_group(1),
            Conceal::new(6..8).with_group(1),
        ]
    }

    #[test]
    fn delete_whole_content_removes_markers() {
        assert_eq!(deletion_ranges(2..6, &bold_group()), [0..8]);
    }

    #[test]
    fn delete_partial_keeps_markers() {
        // "old re" -> "**b**st"
        assert_eq!(deletion_ranges(3..11, &bold_group()), [3..6, 8..11]);
    }

    #[test]
    fn delete_past_group_grows_to_group() {
        assert_eq!(deletion_ranges(2..13, &bold_group()), [0..13]);
    }

    #[test]
    fn delete_ungrouped_concealed_text() {
        // "x\n# h\n" with the heading marker concealed
        let conceals = [Conceal::new(2..4)];
        assert_eq!(deletion_ranges(1..5, &conceals), [1..5]);
    }

    #[test]
    fn delete_nested_groups() {
        // "**[b](u)**"
        let conceals = [
            Conceal::new(0..2).with_group(1),
            Conceal::new(2..3).with_group(2),
            Conceal::new(4..8).with_group(2),
            Conceal::new(8..10).with_group(1),
        ];
        assert_eq!(deletion_ranges(3..4, &conceals), [0..10]);
    }

    #[test]
    fn delete_unrelated_text() {
        assert_eq!(deletion_ranges(9..11, &bold_group()), [9..11]);
    }

    #[test]
    fn delete_empty_range() {
        assert_eq!(deletion_ranges(3..3, &bold_group()), [3..3]);
    }

    #[test]
    fn stale_conceal_past_end_terminates() {
        let new = snap(
            "a**",
            &[Conceal::new(1..5)],
            Selection::point(0),
            Selection::point(1),
        );
        assert_eq!(new, Selection::point(3));
    }

    #[test]
    fn stale_conceal_at_end_terminates() {
        let new = snap(
            "a**",
            &[Conceal::new(3..6)],
            Selection::point(0),
            Selection::point(3),
        );
        assert_eq!(new, Selection::point(3));
    }

    #[test]
    fn stale_conceal_backward_fallback_terminates() {
        let new = snap(
            "**",
            &[Conceal::new(0..5)],
            Selection::point(2),
            Selection::point(1),
        );
        assert_eq!(new, Selection::point(2));
    }
}
