//! Wrap-boundary recomputation for lines containing chips (CMP-010 R5).
//!
//! A chip is wrapped as an unbreakable unit. The algorithm works on plain numbers: the glyph edges
//! of the shaped line and the chip ranges, both supplied by the caller.

use std::ops::Range;

/// The shaped glyphs of one logical line, as the wrap algorithm needs them.
pub trait WrapGlyphs {
    /// The number of glyphs in the line.
    fn len(&self) -> usize;

    /// Whether the line has no glyph.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The left edge of glyph `glyph`; `x(len())` is the right edge of the last glyph, i.e. the
    /// line width.
    fn x(&self, glyph: usize) -> f32;

    /// The position of glyph `glyph` in the space the chip ranges are given in (typically the
    /// source or display byte offset); `index(len())` is the end of the line.
    fn index(&self, glyph: usize) -> usize;

    /// Whether the glyph before `glyph` is whitespace that permits a break after it. Chips use
    /// non-breaking spaces, which are not breaking whitespace.
    fn preceded_by_whitespace(&self, glyph: usize) -> bool;
}

/// A visual row produced by [`wrap_rows`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WrapRow {
    /// The glyphs of the row.
    pub glyphs: Range<usize>,
    /// The index of the row's first glyph.
    pub start: usize,
    /// The index just after the row's last glyph.
    pub end: usize,
}

/// Recomputes the wrap boundaries of a line (R5). `chips` are the chip ranges in the glyphs' index
/// space and `width` is the wrapping width. For each row starting at glyph `r` with
/// `limit = x(r) + width`, the first glyph `i >= r` whose following edge exceeds the limit is the
/// overflow glyph; without one the row is the last. The break is the greatest `i` in
/// `(r, overflow]` that is not strictly inside a chip and is a chip start/end or follows
/// whitespace; else the greatest such `i` not strictly inside a chip; else
/// `max(overflow, r + 1)` (an oversized chip may break).
pub fn wrap_rows(glyphs: &dyn WrapGlyphs, chips: &[Range<usize>], width: f32) -> Vec<WrapRow> {
    let count = glyphs.len();
    let mut rows = Vec::new();
    let mut row_start = 0;
    let row = |from: usize, to: usize| WrapRow {
        glyphs: from..to,
        start: glyphs.index(from),
        end: glyphs.index(to),
    };
    loop {
        let limit = glyphs.x(row_start) + width;
        let overflow = (row_start..count).find(|&i| glyphs.x(i + 1) > limit);
        let Some(overflow) = overflow else {
            rows.push(row(row_start, count));
            return rows;
        };
        let strictly_inside = |i: usize| {
            let index = glyphs.index(i);
            chips
                .iter()
                .any(|chip| chip.start < index && index < chip.end)
        };
        let on_boundary = |i: usize| {
            let index = glyphs.index(i);
            chips
                .iter()
                .any(|chip| chip.start == index || chip.end == index)
        };
        let candidates = || {
            (row_start + 1..=overflow)
                .rev()
                .filter(|&i| !strictly_inside(i))
        };
        let split = candidates()
            .find(|&i| on_boundary(i) || glyphs.preceded_by_whitespace(i))
            .or_else(|| candidates().next())
            .unwrap_or_else(|| overflow.max(row_start + 1));
        rows.push(row(row_start, split));
        if split >= count {
            return rows;
        }
        row_start = split;
    }
}

/// One pill segment of a chip: the part of the chip on one row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChipSegment {
    /// Index of the row in the slice given to [`chip_segments`].
    pub row: usize,
    /// The segment's start, in the chip's index space.
    pub start: usize,
    /// The segment's end, in the chip's index space.
    pub end: usize,
    /// Whether this is the chip's first segment, the only one that carries the icon.
    pub first: bool,
}

/// The pill segments of `chip`: one per row it occupies (R5, R6); only the first carries the icon.
pub fn chip_segments(rows: &[WrapRow], chip: &Range<usize>) -> Vec<ChipSegment> {
    let mut segments: Vec<ChipSegment> = Vec::new();
    for (row, wrap_row) in rows.iter().enumerate() {
        let start = wrap_row.start.max(chip.start);
        let end = wrap_row.end.min(chip.end);
        if start < end {
            let first = segments.is_empty();
            segments.push(ChipSegment {
                row,
                start,
                end,
                first,
            });
        }
    }
    segments
}

/// A [`WrapGlyphs`] over the characters of a text with caller-supplied glyph edges. Whitespace
/// is every Unicode whitespace character except the non-breaking spaces.
pub struct CharWrapLine<'a> {
    text: &'a str,
    xs: Vec<f32>,
    indices: Vec<usize>,
    breakable_before: Vec<bool>,
}

impl<'a> CharWrapLine<'a> {
    /// A line of `text` whose character `i` starts at `xs[i]`; `xs` has one more entry than the
    /// text has characters, the last being the line width. Returns `None` on a length mismatch.
    pub fn new(text: &'a str, xs: Vec<f32>) -> Option<Self> {
        let count = text.chars().count();
        if xs.len() != count + 1 {
            return None;
        }
        let mut indices: Vec<usize> = text.char_indices().map(|(index, _)| index).collect();
        indices.push(text.len());
        let breakable_before =
            std::iter::once(false)
                .chain(text.chars().map(|c| {
                    c.is_whitespace() && !matches!(c, '\u{00A0}' | '\u{2007}' | '\u{202F}')
                }))
                .collect();
        Some(Self {
            text,
            xs,
            indices,
            breakable_before,
        })
    }

    /// The text of the line.
    pub fn text(&self) -> &str {
        self.text
    }
}

impl WrapGlyphs for CharWrapLine<'_> {
    fn len(&self) -> usize {
        self.indices.len() - 1
    }

    fn x(&self, glyph: usize) -> f32 {
        self.xs[glyph]
    }

    fn index(&self, glyph: usize) -> usize {
        self.indices[glyph]
    }

    fn preceded_by_whitespace(&self, glyph: usize) -> bool {
        self.breakable_before[glyph]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line of ASCII text where every character is 10 px wide.
    fn line(text: &str) -> CharWrapLine<'_> {
        let xs = (0..=text.len()).map(|i| i as f32 * 10.0).collect();
        CharWrapLine::new(text, xs).unwrap()
    }

    fn row_texts<'a>(text: &'a str, rows: &[WrapRow]) -> Vec<&'a str> {
        rows.iter().map(|r| &text[r.start..r.end]).collect()
    }

    #[test]
    fn line_that_fits_is_one_row() {
        let text = "aaaa CCCCCCCC bbbb";
        let rows = wrap_rows(&line(text), &[5..13], 180.0);
        assert_eq!(row_texts(text, &rows), vec![text]);
    }

    #[test]
    fn chip_that_fits_on_the_next_row_never_breaks() {
        let text = "aaaa CCCCCCCC bbbb";
        let chips = [5..13];
        let rows = wrap_rows(&line(text), &chips, 120.0);
        assert_eq!(row_texts(text, &rows), vec!["aaaa ", "CCCCCCCC ", "bbbb"]);
        // The chip is entirely on the second row: one segment, with the icon.
        let segments = chip_segments(&rows, &chips[0]);
        assert_eq!(
            segments,
            vec![ChipSegment {
                row: 1,
                start: 5,
                end: 13,
                first: true
            }]
        );
    }

    #[test]
    fn plain_words_break_after_whitespace() {
        let text = "aaaa bbbb cccc";
        let rows = wrap_rows(&line(text), &[], 100.0);
        assert_eq!(row_texts(text, &rows), vec!["aaaa bbbb ", "cccc"]);
    }

    #[test]
    fn unbreakable_text_breaks_at_the_overflow_glyph() {
        let text = "abcdefghijkl";
        let rows = wrap_rows(&line(text), &[], 50.0);
        assert_eq!(row_texts(text, &rows), vec!["abcde", "fghij", "kl"]);
    }

    #[test]
    fn oversized_chip_wraps_and_paints_one_segment_per_row() {
        let text = "aa CCCCCCCCCCCCCCC";
        let chips = [3..18];
        let rows = wrap_rows(&line(text), &chips, 120.0);
        assert_eq!(row_texts(text, &rows), vec!["aa ", "CCCCCCCCCCCC", "CCC"]);
        let segments = chip_segments(&rows, &chips[0]);
        assert_eq!(segments.len(), 2);
        assert_eq!(
            (segments[0].row, segments[0].start, segments[0].end),
            (1, 3, 15)
        );
        assert_eq!(
            (segments[1].row, segments[1].start, segments[1].end),
            (2, 15, 18)
        );
        assert!(segments[0].first);
        assert!(!segments[1].first);
    }

    #[test]
    fn oversized_chip_starting_a_row_breaks_at_the_overflow() {
        let text = "CCCCCCCCCCCCCCC";
        let rows = wrap_rows(&line(text), &[0..15], 100.0);
        assert_eq!(row_texts(text, &rows), vec!["CCCCCCCCCC", "CCCCC"]);
    }

    #[test]
    fn glyph_wider_than_the_width_still_makes_progress() {
        let text = "abc";
        let rows = wrap_rows(&line(text), &[], 5.0);
        assert_eq!(row_texts(text, &rows), vec!["a", "b", "c"]);
    }

    #[test]
    fn chip_end_is_a_break_opportunity_without_whitespace() {
        // Overflow is glyph 10; the greatest i in (0, 10] that is a chip start/end is 6.
        let text = "CCCCCCbbbbbb";
        let rows = wrap_rows(&line(text), &[0..6], 100.0);
        assert_eq!(row_texts(text, &rows), vec!["CCCCCC", "bbbbbb"]);
    }

    #[test]
    fn chip_boundary_is_chosen_when_the_overflow_is_inside_the_chip() {
        // The overflow glyph 12 is inside the chip 9..17; the break goes to the chip start 9.
        let text = "aaa bbbb CCCCCCCC";
        let rows = wrap_rows(&line(text), &[9..17], 120.0);
        assert_eq!(row_texts(text, &rows), vec!["aaa bbbb ", "CCCCCCCC"]);
    }

    #[test]
    fn nbsp_is_not_a_break_opportunity() {
        let text = "ab\u{00A0}cdefgh";
        let xs = (0..=text.chars().count())
            .map(|i| i as f32 * 10.0)
            .collect();
        let line = CharWrapLine::new(text, xs).unwrap();
        assert!(!line.preceded_by_whitespace(3));
    }

    #[test]
    fn mismatched_edges_are_rejected() {
        assert!(CharWrapLine::new("abc", vec![0.0, 1.0]).is_none());
    }

    #[test]
    fn empty_line_is_one_empty_row() {
        let rows = wrap_rows(&line(""), &[], 100.0);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].glyphs, 0..0);
    }
}
