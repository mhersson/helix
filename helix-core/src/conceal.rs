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

/// Skips concealed text starting exactly at `pos`, so that text inserted at
/// the returned position lands after hidden closing markers.
pub fn skip_conceals_at(conceals: &[Conceal], mut pos: usize) -> usize {
    loop {
        let i = conceals.partition_point(|conceal| conceal.range.start < pos);
        match conceals.get(i) {
            Some(conceal) if conceal.range.start == pos => pos = conceal.range.end,
            _ => return pos,
        }
    }
}

/// Conceals overlapping `span`. `conceals` must be sorted.
fn overlapping(conceals: &[Conceal], span: Range<usize>) -> impl Iterator<Item = &Conceal> {
    let start = conceals.partition_point(|conceal| conceal.range.end <= span.start);
    conceals[start..]
        .iter()
        .take_while(move |conceal| conceal.range.start < span.end)
}

/// Sorts `intervals` and merges overlapping or adjacent ones.
fn merge(mut intervals: Vec<Range<usize>>) -> Vec<Range<usize>> {
    intervals.sort_unstable_by_key(|interval| interval.start);
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(intervals.len());
    for interval in intervals {
        match merged.last_mut() {
            Some(last) if interval.start <= last.end => last.end = last.end.max(interval.end),
            _ => merged.push(interval),
        }
    }
    merged
}

/// Whether one interval of the merged `union` contains `span`.
fn union_contains(union: &[Range<usize>], span: &Range<usize>) -> bool {
    let i = union.partition_point(|interval| interval.end < span.end);
    union
        .get(i)
        .is_some_and(|interval| interval.start <= span.start)
}

/// Whether the merged `union` contains every visible (not concealed) char of
/// `span`. A span without visible chars counts as covered if `union` overlaps
/// it.
fn covers_visible(union: &[Range<usize>], span: &Range<usize>, conceals: &[Conceal]) -> bool {
    let mut any_visible = false;
    let mut pos = span.start;
    for conceal in overlapping(conceals, span.clone()) {
        if conceal.range.start > pos {
            any_visible = true;
            if !union_contains(union, &(pos..conceal.range.start)) {
                return false;
            }
        }
        pos = pos.max(conceal.range.end);
    }
    if pos < span.end {
        any_visible = true;
        if !union_contains(union, &(pos..span.end)) {
            return false;
        }
    }
    if any_visible {
        return true;
    }
    let i = union.partition_point(|interval| interval.end <= span.start);
    union
        .get(i)
        .is_some_and(|interval| interval.start < span.end)
}

/// The char ranges to delete when deleting all of `ranges` at once, one list
/// per input range, so that what is deleted matches what is visible:
///
/// * a group whose visible content is entirely deleted (by any of the ranges
///   together) is deleted with its markers (repeated until no more groups are
///   added, for nested groups),
/// * markers of a group whose content is only partly deleted are kept,
/// * concealed text without a group is deleted like any other text.
///
/// The returned ranges are disjoint across all lists. Text deleted on behalf
/// of several input ranges (such as the markers of a group they cover
/// together) is listed once, so a list may be empty.
pub fn deletion_ranges(ranges: &[Range<usize>], conceals: &[Conceal]) -> Vec<Vec<Range<usize>>> {
    let mut deletions: Vec<Vec<Range<usize>>> = ranges
        .iter()
        .map(|range| {
            if range.is_empty() || conceals.is_empty() {
                vec![range.clone()]
            } else {
                Vec::new()
            }
        })
        .collect();
    if conceals.is_empty() {
        return deletions;
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

    let mut union = merge(
        ranges
            .iter()
            .filter(|range| !range.is_empty())
            .cloned()
            .collect(),
    );
    loop {
        let grown: Vec<_> = spans
            .values()
            .filter(|span| !union_contains(&union, span) && covers_visible(&union, span, conceals))
            .cloned()
            .collect();
        if grown.is_empty() {
            break;
        }
        union.extend(grown);
        union = merge(union);
    }

    let kept: HashSet<u32> = spans
        .iter()
        .filter(|(_, span)| !covers_visible(&union, span, conceals))
        .map(|(&group, _)| group)
        .collect();

    // non-empty input ranges by start, each within exactly one union interval
    let mut order: Vec<usize> = (0..ranges.len())
        .filter(|&i| !ranges[i].is_empty())
        .collect();
    order.sort_by_key(|&i| ranges[i].start);
    let mut order = order.as_slice();

    for interval in &union {
        let members = order.partition_point(|&i| ranges[i].start < interval.end);
        let (inside, rest) = order.split_at(members);
        order = rest;
        // each piece goes to the last range starting before its end
        let mut push = |piece: Range<usize>| {
            let owner = inside
                .iter()
                .rev()
                .find(|&&i| ranges[i].start < piece.end)
                .or(inside.first());
            if let Some(&owner) = owner {
                deletions[owner].push(piece);
            }
        };

        let mut pos = interval.start;
        for conceal in overlapping(conceals, interval.clone()) {
            if !conceal.group.is_some_and(|group| kept.contains(&group)) {
                continue;
            }
            let start = conceal.range.start.max(interval.start);
            let end = conceal.range.end.min(interval.end);
            if start > pos {
                push(pos..start);
            }
            pos = pos.max(end);
        }
        if pos < interval.end {
            push(pos..interval.end);
        }
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
        assert_eq!(deletion_ranges(&[2..6], &bold_group()), [[0..8]]);
    }

    #[test]
    fn delete_partial_keeps_markers() {
        // "old re" -> "**b**st"
        assert_eq!(deletion_ranges(&[3..11], &bold_group()), [[3..6, 8..11]]);
    }

    #[test]
    fn delete_past_group_grows_to_group() {
        assert_eq!(deletion_ranges(&[2..13], &bold_group()), [[0..13]]);
    }

    #[test]
    fn delete_ungrouped_concealed_text() {
        // "x\n# h\n" with the heading marker concealed
        let conceals = [Conceal::new(2..4)];
        assert_eq!(deletion_ranges(&[1..5], &conceals), [[1..5]]);
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
        assert_eq!(deletion_ranges(&[3..4], &conceals), [[0..10]]);
    }

    #[test]
    fn delete_unrelated_text() {
        assert_eq!(deletion_ranges(&[9..11], &bold_group()), [[9..11]]);
    }

    #[test]
    fn delete_empty_range() {
        assert_eq!(deletion_ranges(&[3..3], &bold_group()), [[3..3]]);
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

    #[test]
    fn delete_group_covered_by_two_ranges() {
        // "**ab**"
        let conceals = [
            Conceal::new(0..2).with_group(1),
            Conceal::new(4..6).with_group(1),
        ];
        assert_eq!(
            deletion_ranges(&[2..3, 3..4], &conceals),
            [vec![], vec![0..6]]
        );
    }

    #[test]
    fn delete_group_partly_covered_by_two_ranges() {
        // "o" and "d" of "**bold** rest"
        assert_eq!(
            deletion_ranges(&[3..4, 5..6], &bold_group()),
            [[3..4], [5..6]]
        );
    }

    #[test]
    fn delete_two_ranges_around_kept_marker() {
        // "old" and "** r" of "**bold** rest"
        assert_eq!(
            deletion_ranges(&[3..6, 6..10], &bold_group()),
            [[3..6], [8..10]]
        );
    }

    #[test]
    fn delete_two_groups() {
        // "**b** **d**"
        let conceals = [
            Conceal::new(0..2).with_group(1),
            Conceal::new(3..5).with_group(1),
            Conceal::new(6..8).with_group(2),
            Conceal::new(9..11).with_group(2),
        ];
        assert_eq!(deletion_ranges(&[2..3, 8..9], &conceals), [[0..5], [6..11]]);
    }

    #[test]
    fn delete_empty_range_among_others() {
        assert_eq!(
            deletion_ranges(&[2..6, 9..9], &bold_group()),
            [[0..8], [9..9]]
        );
    }

    #[test]
    fn skip_conceals_at_start() {
        let conceals = [Conceal::new(2..4), Conceal::new(4..6), Conceal::new(8..9)];
        assert_eq!(skip_conceals_at(&conceals, 2), 6);
        assert_eq!(skip_conceals_at(&conceals, 3), 3);
        assert_eq!(skip_conceals_at(&conceals, 7), 7);
        assert_eq!(skip_conceals_at(&conceals, 8), 9);
        assert_eq!(skip_conceals_at(&[], 5), 5);
    }
}
