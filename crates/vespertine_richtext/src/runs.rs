//! Exact-cover run construction (TXT-006 R1-R3).

use std::ops::Range;
use vespertine::{Font, Hsla, TextRun};

/// Largest character boundary of `text` that is `<= index` (and `<= text.len()`).
pub fn floor_char_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// A plain-style run of `len` bytes with the given font and color and no decorations.
pub fn plain_run(len: usize, font: &Font, color: Hsla) -> TextRun {
    TextRun {
        len,
        font: font.clone(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    }
}

/// Builds the run list of one line of `text` from `(byte range, color)` spans sorted by start.
///
/// The runs cover `text` exactly (TXT-006 R2): a cursor starts at 0; each span has its start
/// clamped up to the cursor and its end clamped down to the text length (both snapped down to
/// character boundaries so a stale span can never split a character); spans that end up empty are
/// skipped, so overlapping spans never double count and stale spans past a shortened text are
/// discarded. Gaps get `plain`. Every run carries the same `font`; only the color differs, so
/// recoloring can never change shaping (R3). Empty text produces no runs.
pub fn runs_from_spans(
    text: &str,
    spans: &[(Range<usize>, Hsla)],
    plain: Hsla,
    font: &Font,
) -> Vec<TextRun> {
    let len = text.len();
    let mut runs = Vec::with_capacity(spans.len() * 2 + 1);
    let mut cursor = 0;
    for (range, color) in spans {
        let start = floor_char_boundary(text, range.start.max(cursor));
        let end = floor_char_boundary(text, range.end.min(len));
        if end <= start {
            continue;
        }
        if start > cursor {
            runs.push(plain_run(start - cursor, font, plain));
        }
        runs.push(plain_run(end - start, font, *color));
        cursor = end;
    }
    if cursor < len {
        runs.push(plain_run(len - cursor, font, plain));
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;
    use vespertine::{font, rgb};

    fn lens(runs: &[TextRun]) -> Vec<usize> {
        runs.iter().map(|run| run.len).collect()
    }

    #[test]
    fn spans_leave_plain_gaps() {
        let f = font("Noto Sans");
        let red: Hsla = rgb(0xff0000).into();
        let plain: Hsla = rgb(0xeeeeee).into();
        let runs = runs_from_spans("let x = 1;", &[(0..3, red), (8..9, red)], plain, &f);
        assert_eq!(lens(&runs), vec![3, 5, 1, 1]);
        assert_eq!(runs.iter().map(|r| r.len).sum::<usize>(), 10);
        assert!(runs.iter().all(|run| run.font == f));
        assert_eq!(runs[0].color, red);
        assert_eq!(runs[1].color, plain);
    }

    #[test]
    fn overlapping_spans_never_double_count() {
        let f = font("Noto Sans");
        let c: Hsla = rgb(0xff0000).into();
        let runs = runs_from_spans("abcdef", &[(0..4, c), (2..6, c)], c, &f);
        assert_eq!(lens(&runs), vec![4, 2]);
        assert_eq!(runs.iter().map(|r| r.len).sum::<usize>(), 6);
    }

    #[test]
    fn stale_spans_are_clipped_or_discarded() {
        let f = font("Noto Sans");
        let c: Hsla = rgb(0xff0000).into();
        let runs = runs_from_spans("ab", &[(0..99, c)], c, &f);
        assert_eq!(lens(&runs), vec![2]);
        let runs = runs_from_spans("ab", &[(5..9, c)], c, &f);
        assert_eq!(lens(&runs), vec![2]);
        assert!(runs_from_spans("", &[(0..3, c)], c, &f).is_empty());
    }

    #[test]
    fn spans_inside_characters_snap_down() {
        let f = font("Noto Sans");
        let c: Hsla = rgb(0xff0000).into();
        // "é" is two bytes; a span ending inside it is cut before it.
        let runs = runs_from_spans("aéb", &[(0..2, c)], c, &f);
        assert_eq!(runs.iter().map(|r| r.len).sum::<usize>(), 4);
        assert_eq!(runs[0].len, 1);
    }
}
