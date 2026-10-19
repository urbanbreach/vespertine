//! Owner-supplied syntax spans for the code editor (CMP-014 R11).
//!
//! A [`SpanHighlighter`] holds absolute byte ranges tagged with a semantic [`TokenKind`]. It is
//! only installed when the text it was computed from equals the editor's current text; edits
//! adjust it without recomputation, and requested ranges are answered with contiguous runs.
//! Colors never change advances: a run only carries a color.

use std::ops::Range;
use vespertine::Hsla;
use vespertine_richtext::syntax::{HighlightedDocument, TokenKind};

/// One edit of the text: `range` (old byte offsets) was replaced by `inserted` bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    /// The replaced range in the text before the edit.
    pub range: Range<usize>,
    /// The number of bytes inserted in its place.
    pub inserted: usize,
}

impl TextEdit {
    /// The edit's change in text length.
    pub fn delta(&self) -> isize {
        self.inserted as isize - (self.range.end - self.range.start) as isize
    }
}

/// A styled run: a byte range and its color.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StyledRun {
    /// The run's byte range.
    pub range_start: usize,
    /// End of the run (exclusive).
    pub range_end: usize,
    /// The run's color.
    pub color: Hsla,
}

impl StyledRun {
    /// The run's byte range.
    pub fn range(&self) -> Range<usize> {
        self.range_start..self.range_end
    }
}

/// The contract between an editor and an owner-supplied highlighter.
pub trait CodeHighlighter {
    /// Adjusts the highlighter to an edit without recomputing it.
    fn update(&mut self, edit: &TextEdit);
    /// Contiguous styled runs covering exactly `range`; unclassified gaps use `default`.
    fn styles(
        &self,
        range: Range<usize>,
        resolver: &dyn Fn(TokenKind) -> Hsla,
        default: Hsla,
    ) -> Vec<StyledRun>;
    /// Foldable ranges; none are required.
    fn fold_ranges(&self) -> Vec<Range<usize>> {
        Vec::new()
    }
}

/// Absolute-range spans with edit adjustment.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpanHighlighter {
    spans: Vec<(Range<usize>, TokenKind)>,
}

impl SpanHighlighter {
    /// A highlighter from spans computed for `source`. Spans must be half-open, in bounds and on
    /// UTF-8 boundaries; invalid or empty ones are dropped, overlapping ones are clipped by
    /// order (earlier start wins).
    pub fn new(source: &str, spans: impl IntoIterator<Item = (Range<usize>, TokenKind)>) -> Self {
        let mut spans: Vec<_> = spans
            .into_iter()
            .filter(|(range, _)| {
                range.start < range.end
                    && range.end <= source.len()
                    && source.is_char_boundary(range.start)
                    && source.is_char_boundary(range.end)
            })
            .collect();
        spans.sort_by_key(|(range, _)| (range.start, range.end));
        let mut cleaned: Vec<(Range<usize>, TokenKind)> = Vec::with_capacity(spans.len());
        for (mut range, kind) in spans {
            if let Some((last, _)) = cleaned.last() {
                range.start = range.start.max(last.end);
            }
            if range.start < range.end {
                cleaned.push((range, kind));
            }
        }
        Self { spans: cleaned }
    }

    /// A highlighter from a cached coloration (TXT-007) of `source`: the per-line spans become
    /// absolute ranges.
    pub fn from_document(source: &str, document: &HighlightedDocument) -> Self {
        let mut spans = Vec::with_capacity(document.span_count());
        let mut line_start = 0;
        for (index, line) in source.split('\n').enumerate() {
            if let Some(line_spans) = document.lines().get(index) {
                spans.extend(line_spans.iter().map(|span| {
                    (
                        line_start + span.range.start..line_start + span.range.end,
                        span.kind,
                    )
                }));
            }
            line_start += line.len() + 1;
        }
        Self::new(source, spans)
    }

    /// The spans in order.
    pub fn spans(&self) -> &[(Range<usize>, TokenKind)] {
        &self.spans
    }
}

impl CodeHighlighter for SpanHighlighter {
    fn update(&mut self, edit: &TextEdit) {
        let delta = edit.delta();
        self.spans.retain_mut(|(range, _)| {
            if range.end <= edit.range.start {
                true
            } else if range.start >= edit.range.end {
                range.start = (range.start as isize + delta) as usize;
                range.end = (range.end as isize + delta) as usize;
                true
            } else {
                false
            }
        });
    }

    fn styles(
        &self,
        range: Range<usize>,
        resolver: &dyn Fn(TokenKind) -> Hsla,
        default: Hsla,
    ) -> Vec<StyledRun> {
        let mut runs = Vec::new();
        let mut cursor = range.start;
        let first = self
            .spans
            .partition_point(|(span, _)| span.end <= range.start);
        for (span, kind) in &self.spans[first..] {
            if span.start >= range.end {
                break;
            }
            let start = span.start.max(range.start);
            let end = span.end.min(range.end);
            if start > cursor {
                runs.push(run(cursor, start, default));
            }
            runs.push(run(start, end, resolver(*kind)));
            cursor = end;
        }
        if cursor < range.end {
            runs.push(run(cursor, range.end, default));
        }
        runs
    }
}

fn run(start: usize, end: usize, color: Hsla) -> StyledRun {
    StyledRun {
        range_start: start,
        range_end: end,
        color,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vespertine::{black, white};

    fn highlighter() -> SpanHighlighter {
        // "let x = 1;" with `let` keyword (0..3), `1` number (8..9).
        SpanHighlighter::new(
            "let x = 1;",
            [(0..3, TokenKind::Keyword), (8..9, TokenKind::Number)],
        )
    }

    #[test]
    fn spans_before_an_edit_are_unchanged_and_after_are_shifted() {
        let mut h = highlighter();
        h.update(&TextEdit {
            range: 4..5,
            inserted: 3,
        });
        assert_eq!(h.spans()[0].0, 0..3);
        assert_eq!(h.spans()[1].0, 10..11);
        h.update(&TextEdit {
            range: 0..0,
            inserted: 0,
        });
        let mut h = highlighter();
        h.update(&TextEdit {
            range: 5..7,
            inserted: 0,
        });
        assert_eq!(h.spans()[1].0, 6..7);
    }

    #[test]
    fn overlapping_spans_are_removed() {
        let mut h = highlighter();
        h.update(&TextEdit {
            range: 1..2,
            inserted: 1,
        });
        assert_eq!(h.spans().len(), 1);
        assert_eq!(h.spans()[0].1, TokenKind::Number);
        // An insertion strictly inside a span also removes it.
        let mut h = highlighter();
        h.update(&TextEdit {
            range: 2..2,
            inserted: 4,
        });
        assert_eq!(h.spans().len(), 1);
    }

    #[test]
    fn touching_edits_keep_both_neighbours() {
        let mut h = highlighter();
        // Insert exactly at the end of `let` and exactly at the start of `1`.
        h.update(&TextEdit {
            range: 3..3,
            inserted: 2,
        });
        assert_eq!(h.spans()[0].0, 0..3);
        assert_eq!(h.spans()[1].0, 10..11);
    }

    #[test]
    fn styles_cover_the_whole_range_contiguously() {
        let h = highlighter();
        let keyword = white();
        let default = black();
        let runs = h.styles(
            0..10,
            &|kind| {
                if kind == TokenKind::Keyword {
                    keyword
                } else {
                    white()
                }
            },
            default,
        );
        let mut cursor = 0;
        for r in &runs {
            assert_eq!(r.range_start, cursor);
            cursor = r.range_end;
        }
        assert_eq!(cursor, 10);
        assert_eq!(runs[0].range(), 0..3);
        assert_eq!(runs[1].color, default);
        // A sub-range clips spans.
        let runs = h.styles(1..9, &|_| white(), default);
        assert_eq!(runs.first().unwrap().range_start, 1);
        assert_eq!(runs.last().unwrap().range_end, 9);
    }

    #[test]
    fn a_cached_document_becomes_absolute_spans() {
        use vespertine_richtext::syntax::HighlightCache;
        let source = "fn a() {}\nlet x = 1;\n";
        let cache = HighlightCache::new();
        let document = vespertine_richtext::syntax::HighlighterAdapter::document_for(
            &cache,
            source,
            vespertine_richtext::syntax::LanguageHint {
                fence_label: Some("rust"),
                path: None,
            },
        )
        .expect("rust is supported");
        let highlighter = SpanHighlighter::from_document(source, &document);
        assert!(!highlighter.spans().is_empty());
        let keyword = highlighter
            .spans()
            .iter()
            .find(|(range, _)| &source[range.clone()] == "let")
            .expect("`let` on the second line is classified");
        assert_eq!(keyword.0.start, "fn a() {}\n".len());
    }

    #[test]
    fn invalid_spans_are_dropped_on_construction() {
        let h = SpanHighlighter::new(
            "é",
            [
                (0..1, TokenKind::Keyword),
                (0..2, TokenKind::Keyword),
                (0..9, TokenKind::Keyword),
                (2..2, TokenKind::Keyword),
            ],
        );
        assert_eq!(h.spans().len(), 1);
    }
}
