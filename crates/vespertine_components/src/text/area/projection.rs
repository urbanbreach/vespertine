//! The source/display projection of the multiline editor (CMP-008 R16).
//!
//! A [`Projection`] is an ordered, non-overlapping list of [`Replacement`]s, each mapping a source
//! byte range to a display string. Layout, painting and hit testing use the display text; editing
//! and selection use source offsets. This module is pure: it maps offsets and normalizes carets
//! and selections around atomic replacements (chips); the editor applies the results.

use std::ops::Range;

/// One replacement of the projection: a source range shown as `display`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Replacement {
    /// The replaced source byte range.
    pub range: Range<usize>,
    /// The display string; may be empty (hidden markers) but never contains a line feed.
    pub display: String,
    /// Whether the range behaves as one unit for caret and selection (a chip).
    pub atomic: bool,
}

/// A validated projection: sorted, non-overlapping replacements with precomputed display starts.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Projection {
    replacements: Vec<Replacement>,
    /// `display_starts[i]` is the display offset where replacement `i` starts.
    display_starts: Vec<usize>,
}

impl Projection {
    /// A projection without replacements: display equals source.
    pub fn identity() -> Self {
        Self::default()
    }

    /// Builds a projection from `replacements` for `source`. The list is sorted by start (stable);
    /// entries that are invalid are dropped: an empty or out-of-bounds range, a range not on char
    /// boundaries of `source`, a range or display containing a line feed, and any entry that
    /// overlaps an earlier kept one.
    pub fn new(source: &str, mut replacements: Vec<Replacement>) -> Self {
        replacements.sort_by_key(|replacement| replacement.range.start);
        let mut kept: Vec<Replacement> = Vec::with_capacity(replacements.len());
        let mut end_of_previous = 0;
        for replacement in replacements {
            let range = &replacement.range;
            let valid = range.start < range.end
                && range.start >= end_of_previous
                && range.end <= source.len()
                && source.is_char_boundary(range.start)
                && source.is_char_boundary(range.end)
                && !source[range.clone()].contains('\n')
                && !replacement.display.contains('\n');
            if valid {
                end_of_previous = range.end;
                kept.push(replacement);
            }
        }
        let mut display_starts = Vec::with_capacity(kept.len());
        let mut source_cursor = 0;
        let mut display_cursor = 0;
        for replacement in &kept {
            display_cursor += replacement.range.start - source_cursor;
            display_starts.push(display_cursor);
            display_cursor += replacement.display.len();
            source_cursor = replacement.range.end;
        }
        Self {
            replacements: kept,
            display_starts,
        }
    }

    /// Whether the projection has no replacement.
    pub fn is_identity(&self) -> bool {
        self.replacements.is_empty()
    }

    /// The validated replacements, sorted and non-overlapping.
    pub fn replacements(&self) -> &[Replacement] {
        &self.replacements
    }

    /// The display range of replacement `index`.
    pub fn display_range(&self, index: usize) -> Range<usize> {
        let start = self.display_starts[index];
        start..start + self.replacements[index].display.len()
    }

    /// The display text of `source`: unreplaced source text with every replacement's display.
    pub fn display_text(&self, source: &str) -> String {
        let mut out = String::with_capacity(source.len());
        let mut cursor = 0;
        for replacement in &self.replacements {
            out.push_str(&source[cursor..replacement.range.start]);
            out.push_str(&replacement.display);
            cursor = replacement.range.end;
        }
        out.push_str(&source[cursor..]);
        out
    }

    /// The length of the display text of a source of `source_len` bytes.
    pub fn display_len(&self, source_len: usize) -> usize {
        self.source_to_display(source_len)
    }

    /// Maps a source offset to a display offset (R16): find the first replacement whose source
    /// end is greater than `s`; if `s` is strictly after its start the result is its display
    /// start; otherwise it is the display end of the previous replacement (or 0) plus `s` minus
    /// the previous source end (or 0).
    pub fn source_to_display(&self, s: usize) -> usize {
        let found = self
            .replacements
            .partition_point(|replacement| replacement.range.end <= s);
        if let Some(replacement) = self.replacements.get(found)
            && s > replacement.range.start
        {
            return self.display_starts[found];
        }
        let (display_end, source_end) = self.previous_ends(found);
        display_end + (s - source_end)
    }

    /// Maps a display offset to a source offset (R16): find the first replacement whose display
    /// end is greater than `d` (zero-width replacements at `d` are skipped); if `d` is strictly
    /// after its display start the result is its source start when `d - display start` is less
    /// than half the display length (integer division) and its source end otherwise; else it is
    /// the source end of the previous replacement (or 0) plus `d` minus its display end (or 0).
    /// The result is clamped to the source length and floored to a char boundary of `source`.
    pub fn display_to_source(&self, source: &str, d: usize) -> usize {
        let found = (0..self.replacements.len())
            .find(|&index| self.display_range(index).end > d)
            .unwrap_or(self.replacements.len());
        let mapped = 'mapped: {
            if let Some(replacement) = self.replacements.get(found) {
                let display_start = self.display_starts[found];
                if d > display_start {
                    break 'mapped if d - display_start < replacement.display.len() / 2 {
                        replacement.range.start
                    } else {
                        replacement.range.end
                    };
                }
            }
            let (display_end, source_end) = self.previous_ends(found);
            source_end + (d - display_end)
        };
        let mut result = mapped.min(source.len());
        while !source.is_char_boundary(result) {
            result -= 1;
        }
        result
    }

    /// Maps a source range to a display range.
    pub fn source_range_to_display(&self, range: Range<usize>) -> Range<usize> {
        self.source_to_display(range.start)..self.source_to_display(range.end)
    }

    /// Maps a display range to a source range.
    pub fn display_range_to_source(&self, source: &str, range: Range<usize>) -> Range<usize> {
        self.display_to_source(source, range.start)..self.display_to_source(source, range.end)
    }

    /// `(display end, source end)` of the replacement before index `found`, or `(0, 0)`.
    fn previous_ends(&self, found: usize) -> (usize, usize) {
        match found.checked_sub(1) {
            Some(previous) => (
                self.display_range(previous).end,
                self.replacements[previous].range.end,
            ),
            None => (0, 0),
        }
    }

    /// The source ranges of the atomic replacements, in order.
    pub fn atomic_ranges(&self) -> Vec<Range<usize>> {
        self.replacements
            .iter()
            .filter(|replacement| replacement.atomic)
            .map(|replacement| replacement.range.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn replacement(range: Range<usize>, display: &str, atomic: bool) -> Replacement {
        Replacement {
            range,
            display: display.to_owned(),
            atomic,
        }
    }

    /// The A9 projection: source 2..6 is shown as "X" (display 2..3).
    fn a9() -> (String, Projection) {
        let source = "abcdefghijkl".to_owned();
        let projection = Projection::new(&source, vec![replacement(2..6, "X", false)]);
        (source, projection)
    }

    #[test]
    fn source_to_display_matches_the_a9_numbers() {
        let (_, projection) = a9();
        for (source, display) in [(0, 0), (2, 2), (4, 2), (6, 3), (9, 6)] {
            assert_eq!(projection.source_to_display(source), display, "s={source}");
        }
        assert_eq!(projection.display_range(0), 2..3);
    }

    #[test]
    fn display_to_source_matches_the_a9_numbers() {
        let (source, projection) = a9();
        for (display, expected) in [(2, 2), (3, 6), (5, 8)] {
            assert_eq!(
                projection.display_to_source(&source, display),
                expected,
                "d={display}"
            );
        }
    }

    #[test]
    fn display_text_applies_the_replacements() {
        let (source, projection) = a9();
        assert_eq!(projection.display_text(&source), "abXghijkl");
        assert_eq!(projection.display_len(source.len()), 9);
    }

    #[test]
    fn identity_maps_offsets_unchanged() {
        let projection = Projection::identity();
        assert!(projection.is_identity());
        assert_eq!(projection.display_text("héllo"), "héllo");
        assert_eq!(projection.source_to_display(3), 3);
        assert_eq!(projection.display_to_source("héllo", 3), 3);
        assert!(projection.atomic_ranges().is_empty());
    }

    #[test]
    fn display_to_source_inside_a_replacement_snaps_by_the_midpoint() {
        let source = "ab123456cd";
        let projection = Projection::new(source, vec![replacement(2..8, "wxyz", false)]);
        // display "abwxyzcd": the replacement is display 2..6, length 4, half 2.
        assert_eq!(projection.display_to_source(source, 3), 2);
        assert_eq!(projection.display_to_source(source, 4), 8);
        assert_eq!(projection.display_to_source(source, 5), 8);
        assert_eq!(projection.display_to_source(source, 6), 8);
        assert_eq!(projection.display_to_source(source, 8), 10);
    }

    #[test]
    fn zero_width_replacements_are_skipped_for_hit_points() {
        // Hidden opening marker "**" at 0..2 and closing marker at 6..8.
        let source = "**bold**!";
        let projection = Projection::new(
            source,
            vec![replacement(0..2, "", false), replacement(6..8, "", false)],
        );
        assert_eq!(projection.display_text(source), "bold!");
        // Display 0 lands after the hidden opening marker.
        assert_eq!(projection.display_to_source(source, 0), 2);
        assert_eq!(projection.display_to_source(source, 4), 8);
        assert_eq!(projection.display_to_source(source, 5), 9);
        assert_eq!(projection.source_to_display(0), 0);
        assert_eq!(projection.source_to_display(2), 0);
        assert_eq!(projection.source_to_display(1), 0);
        assert_eq!(projection.source_to_display(7), 4);
        assert_eq!(projection.source_to_display(9), 5);
    }

    #[test]
    fn multibyte_text_maps_on_char_boundaries() {
        let source = "é😀 chip é";
        let chip = source.find("chip").unwrap();
        let projection = Projection::new(source, vec![replacement(chip..chip + 4, "ç", false)]);
        let display = projection.display_text(source);
        assert_eq!(display, "é😀 ç é");
        let after = projection.source_to_display(chip + 4);
        assert_eq!(&display[after..], " é");
        assert_eq!(projection.display_to_source(source, after), chip + 4);
        // A display offset inside a multi-byte char floors to a source char boundary.
        for d in 0..=display.len() {
            let s = projection.display_to_source(source, d);
            assert!(source.is_char_boundary(s), "d={d} s={s}");
        }
    }

    #[test]
    fn results_are_clamped_to_the_source() {
        let (source, projection) = a9();
        assert_eq!(projection.display_to_source(&source, 500), source.len());
    }

    #[test]
    fn invalid_replacements_are_dropped() {
        let source = "ab\ncdefghij";
        let projection = Projection::new(
            source,
            vec![
                replacement(8..10, "late", false),
                replacement(9..11, "overlap", false),
                replacement(4..6, "ok", false),
                replacement(1..5, "newline in source", false),
                replacement(6..7, "new\nline", false),
                replacement(20..30, "outside", false),
                replacement(7..7, "empty", false),
            ],
        );
        let kept: Vec<Range<usize>> = projection
            .replacements()
            .iter()
            .map(|replacement| replacement.range.clone())
            .collect();
        assert_eq!(kept, vec![4..6, 8..10]);
    }

    #[test]
    fn non_boundary_ranges_are_dropped_and_unsorted_input_is_sorted() {
        let source = "éa b";
        let projection = Projection::new(
            source,
            vec![
                replacement(4..5, "B", false),
                replacement(1..2, "split", false),
                replacement(0..2, "E", false),
            ],
        );
        let kept: Vec<Range<usize>> = projection
            .replacements()
            .iter()
            .map(|replacement| replacement.range.clone())
            .collect();
        assert_eq!(kept, vec![0..2, 4..5]);
        assert_eq!(projection.display_text(source), "Ea B");
    }

    #[test]
    fn display_ranges_accumulate_across_replacements() {
        let source = "0123456789";
        let projection = Projection::new(
            source,
            vec![
                replacement(1..3, "ab", false),
                replacement(5..9, "c", false),
            ],
        );
        assert_eq!(projection.display_range(0), 1..3);
        assert_eq!(projection.display_range(1), 5..6);
        assert_eq!(projection.display_text(source), "0ab34c9");
        assert_eq!(projection.source_to_display(10), 7);
        assert_eq!(projection.source_range_to_display(4..10), 4..7);
        assert_eq!(projection.display_range_to_source(source, 4..7), 4..10);
    }

    #[test]
    fn atomic_ranges_are_listed() {
        let source = "a0123b4567c";
        let projection = Projection::new(
            source,
            vec![replacement(1..5, "A", true), replacement(6..10, "B", false)],
        );
        assert_eq!(projection.atomic_ranges(), vec![1..5]);
    }
}
