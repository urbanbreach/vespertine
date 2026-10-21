//! The `CodeLine` value: one syntax-colored source line (TXT-007 R10, R13).
//!
//! The run list of a line comes from [`runs_from_spans`] with ONE shared font, so changing colors
//! (or receiving a late highlight result) never changes shaping. Hosts paint the line as a native
//! [`crate::RichText`] run.

use crate::runs::runs_from_spans;
use std::ops::Range;
use vespertine::{Font, Hsla, SharedString, TextRun};

/// One line of a code view: its text and the runs built from the line's spans with the view's
/// single font.
pub struct CodeLine {
    index: u32,
    text: SharedString,
    runs: Vec<TextRun>,
}

impl CodeLine {
    /// Builds a line from its text and `(byte range, color)` spans sorted by start; every run
    /// carries `font`, gaps get `plain` (TXT-006 R2/R3).
    pub fn new(
        index: usize,
        text: &str,
        spans: impl IntoIterator<Item = (Range<usize>, Hsla)>,
        plain: Hsla,
        font: &Font,
    ) -> Self {
        let spans: Vec<(Range<usize>, Hsla)> = spans.into_iter().collect();
        Self {
            index: index as u32,
            runs: runs_from_spans(text, &spans, plain, font),
            text: SharedString::from(text.to_owned()),
        }
    }

    /// The index of the line in the source.
    pub fn index(&self) -> usize {
        self.index as usize
    }

    /// The text of the line (without its terminator).
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The runs covering the text exactly.
    pub fn runs(&self) -> &[TextRun] {
        &self.runs
    }
}
