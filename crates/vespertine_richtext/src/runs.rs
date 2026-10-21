//! Exact-cover run construction (TXT-006 R1-R3).

use std::hash::{Hash, Hasher};
use std::ops::Range;
use vespertine::{Font, Hsla, Pixels, TextOverflow, TextRun};

/// A validated rich text: a text plus runs that cover it exactly (TXT-006 R1).
#[derive(Clone, Debug)]
pub struct RichSpans {
    text: String,
    runs: Vec<TextRun>,
}

/// Why a run list does not cover its text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CoverError {
    /// The run lengths do not sum to the text length.
    #[error("run lengths sum to {runs} bytes but the text has {text}")]
    Length {
        /// Sum of the run lengths.
        runs: usize,
        /// Length of the text.
        text: usize,
    },
    /// A run boundary falls inside a character.
    #[error("run boundary at byte {0} is not on a character boundary")]
    Boundary(usize),
}

impl RichSpans {
    /// Validates that `runs` cover `text` exactly: lengths sum to the text length and every
    /// boundary is on a character boundary. Zero-length runs are dropped.
    pub fn new(text: impl Into<String>, mut runs: Vec<TextRun>) -> Result<Self, CoverError> {
        let text = text.into();
        runs.retain(|run| run.len > 0);
        let total: usize = runs.iter().map(|run| run.len).sum();
        if total != text.len() {
            return Err(CoverError::Length {
                runs: total,
                text: text.len(),
            });
        }
        let mut offset = 0;
        for run in &runs {
            offset += run.len;
            if !text.is_char_boundary(offset) {
                return Err(CoverError::Boundary(offset));
            }
        }
        Ok(Self { text, runs })
    }

    /// Builds the runs of one line from `(range, color)` spans per [`runs_from_spans`].
    pub fn from_color_spans(
        text: impl Into<String>,
        spans: &[(Range<usize>, Hsla)],
        plain: Hsla,
        font: &Font,
    ) -> Self {
        let text = text.into();
        let runs = runs_from_spans(&text, spans, plain, font);
        Self { text, runs }
    }

    /// The text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The runs, covering the text exactly.
    pub fn runs(&self) -> &[TextRun] {
        &self.runs
    }

    /// Splits into the text and its runs.
    pub fn into_parts(self) -> (String, Vec<TextRun>) {
        (self.text, self.runs)
    }

    /// The text as an element, with these runs.
    pub fn styled_text(&self) -> vespertine::StyledText {
        vespertine::StyledText::new(self.text.clone()).with_runs(self.runs.clone())
    }

    /// Key of everything that influences shaping and line breaking: the text, each run's length,
    /// font (family, weight, style, features) and letter spacing, the size, the wrap width and
    /// the overflow mode.
    /// Colors, backgrounds, underlines and strikethroughs are paint-only and never part of it
    /// (R3).
    pub fn shaping_key(
        &self,
        font_size: Pixels,
        wrap_width: Option<Pixels>,
        overflow: Option<&TextOverflow>,
    ) -> u64 {
        shaping_key(&self.text, &self.runs, font_size, wrap_width, overflow)
    }
}

/// See [`RichSpans::shaping_key`].
pub fn shaping_key(
    text: &str,
    runs: &[TextRun],
    font_size: Pixels,
    wrap_width: Option<Pixels>,
    overflow: Option<&TextOverflow>,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    for run in runs {
        run.len.hash(&mut hasher);
        run.font.hash(&mut hasher);
        f32::from(run.letter_spacing).to_bits().hash(&mut hasher);
    }
    f32::from(font_size).to_bits().hash(&mut hasher);
    wrap_width
        .map(|width| f32::from(width).to_bits())
        .hash(&mut hasher);
    match overflow {
        None => 0u8.hash(&mut hasher),
        Some(TextOverflow::Truncate(ellipsis)) => (1u8, ellipsis).hash(&mut hasher),
        Some(TextOverflow::TruncateStart(ellipsis)) => (2u8, ellipsis).hash(&mut hasher),
        Some(TextOverflow::TruncateMiddle(ellipsis)) => (3u8, ellipsis).hash(&mut hasher),
    }
    hasher.finish()
}

/// Largest character boundary of `text` that is `<= index` (and `<= text.len()`).
pub fn floor_char_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// A plain-style run of `len` bytes with the given font and color and no decorations. It has no
/// letter spacing of its own, so [`crate::RichText`] gives it the inherited text style's.
pub fn plain_run(len: usize, font: &Font, color: Hsla) -> TextRun {
    TextRun {
        color,
        font: font.clone(),
        len,
        strikethrough: None,
        underline: None,
        background_color: None,
        letter_spacing: Pixels::ZERO,
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
