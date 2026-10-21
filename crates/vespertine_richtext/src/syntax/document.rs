//! Highlighted documents: per-line normalized neutral spans (TXT-007 R9, R10, R12, R16).

use super::kind::TokenKind;
use super::language::Language;
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::fmt;
use std::ops::Range;

/// Largest source accepted for highlighting, in bytes (TXT-007 R12).
pub const MAX_SOURCE_BYTES: usize = 524_288;
/// Largest number of raw spans accepted before normalization (TXT-007 R12).
pub const MAX_RAW_SPANS: usize = 200_000;

/// Source and span limits of one highlight request (TXT-007 R12).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Longest accepted source in bytes; exactly this many bytes are allowed.
    pub max_source_bytes: usize,
    /// Most raw spans accepted; the result is rejected as soon as this is exceeded.
    pub max_raw_spans: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_source_bytes: MAX_SOURCE_BYTES,
            max_raw_spans: MAX_RAW_SPANS,
        }
    }
}

/// Why a highlight request produced no document. Every error means "render plain text"; none
/// is ever shown to the user or cached (TXT-007 R12).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HighlightError {
    /// No language resolved, or no grammar was found for it.
    UnknownLanguage,
    /// A span has `start > end` or `end` past the source length.
    InvalidRange,
    /// A span end lies inside a multi-byte character.
    InvalidUtf8Boundary,
    /// The source is longer than the source limit.
    SourceTooLarge,
    /// More raw spans than the span limit.
    TooManySpans,
    /// The tokenizer reported an error.
    Parser(String),
}

impl fmt::Display for HighlightError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HighlightError::UnknownLanguage => write!(f, "unknown language or missing grammar"),
            HighlightError::InvalidRange => write!(f, "span range outside the source"),
            HighlightError::InvalidUtf8Boundary => {
                write!(f, "span end inside a UTF-8 character")
            }
            HighlightError::SourceTooLarge => write!(f, "source exceeds the highlight limit"),
            HighlightError::TooManySpans => write!(f, "span count exceeds the highlight limit"),
            HighlightError::Parser(message) => write!(f, "tokenizer error: {message}"),
        }
    }
}

impl std::error::Error for HighlightError {}

/// One colored span of a line: a half-open byte range relative to the line start, on character
/// boundaries, excluding the line terminator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub range: Range<usize>,
    pub kind: TokenKind,
}

/// A highlighted document: one entry per source line with sorted, non-overlapping, non-empty
/// spans (TXT-007 R9/R10). The document does not retain the source text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HighlightedDocument {
    language: Language,
    lines: Vec<Vec<Span>>,
    /// Raw spans the tokenizer produced, which the span limit is checked against.
    raw_spans: usize,
}

/// Fixed header cost in the estimated weight (TXT-007 R16).
const HEADER_WEIGHT: usize = 32;
/// Cost per line (the per-line vector header) in the estimated weight.
const LINE_WEIGHT: usize = 24;
/// Cost per span record in the estimated weight.
const SPAN_WEIGHT: usize = 24;

impl HighlightedDocument {
    /// Builds a document from absolute spans `(byte range in source, kind)` of a third-party
    /// tokenizer (TXT-007 R9/R10, R12): every range is validated (`start <= end <= len`, both
    /// ends on character boundaries), empty spans are dropped, spans crossing newlines are split
    /// at them, and each line is normalized. Any invalid span rejects the whole result; so does a
    /// source over `limits.max_source_bytes` or more than `limits.max_raw_spans` spans (checked as
    /// soon as the limit is crossed).
    pub fn from_absolute_spans(
        language: Language,
        source: &str,
        spans: impl IntoIterator<Item = (Range<usize>, TokenKind)>,
        limits: Limits,
    ) -> Result<Self, HighlightError> {
        if source.len() > limits.max_source_bytes {
            return Err(HighlightError::SourceTooLarge);
        }
        let line_starts = line_starts(source);
        let mut lines: Vec<Vec<RawSpan>> = vec![Vec::new(); line_starts.len()];
        let mut count = 0usize;
        let mut index = 0u32;
        for (range, kind) in spans {
            count += 1;
            if count > limits.max_raw_spans {
                return Err(HighlightError::TooManySpans);
            }
            if range.start > range.end || range.end > source.len() {
                return Err(HighlightError::InvalidRange);
            }
            if !source.is_char_boundary(range.start) || !source.is_char_boundary(range.end) {
                return Err(HighlightError::InvalidUtf8Boundary);
            }
            if range.is_empty() {
                continue;
            }
            split_span(source, &line_starts, &mut lines, range, kind, index);
            index += 1;
        }
        let lines = lines.into_iter().map(normalize_line).collect();
        Ok(Self {
            language,
            lines,
            raw_spans: count,
        })
    }

    /// A document with `line_count` lines and no spans (a source highlighted without any
    /// classified token).
    pub fn unstyled(language: Language, source: &str) -> Self {
        Self {
            language,
            lines: vec![Vec::new(); line_starts(source).len()],
            raw_spans: 0,
        }
    }

    /// The language the document was highlighted as.
    pub fn language(&self) -> Language {
        self.language
    }

    /// The per-line spans, one entry per source line.
    pub fn lines(&self) -> &[Vec<Span>] {
        &self.lines
    }

    /// Number of lines (an empty source has exactly one).
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Total number of spans over all lines.
    pub fn span_count(&self) -> usize {
        self.lines.iter().map(Vec::len).sum()
    }

    /// Number of raw spans the document was built from, before line splitting and merging:
    /// what [`Limits::max_raw_spans`] bounds.
    pub fn raw_span_count(&self) -> usize {
        self.raw_spans
    }

    /// Estimated retained weight: 32 + 24 per line + 24 per span, in bytes (TXT-007 R16).
    pub fn weight(&self) -> usize {
        HEADER_WEIGHT + LINE_WEIGHT * self.lines.len() + SPAN_WEIGHT * self.span_count()
    }
}

/// A raw span restricted to one line, with the index of its original absolute span.
#[derive(Clone, Copy)]
struct RawSpan {
    start: usize,
    end: usize,
    kind: TokenKind,
    index: u32,
}

/// Byte offsets of the start of every line: an empty source has one line, a trailing newline
/// does not create a phantom last line (TXT-007 R9).
pub(crate) fn line_starts(source: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (offset, byte) in source.bytes().enumerate() {
        if byte == b'\n' && offset + 1 < source.len() {
            starts.push(offset + 1);
        }
    }
    starts
}

/// End of the content of the line `start..next` (exclusive of its `\n` or `\r\n` terminator).
fn content_end(bytes: &[u8], start: usize, next: usize) -> usize {
    let mut end = next;
    if end > start && bytes[end - 1] == b'\n' {
        end -= 1;
        if end > start && bytes[end - 1] == b'\r' {
            end -= 1;
        }
    }
    end
}

/// Splits an absolute span at line boundaries and pushes the line-relative pieces, dropping the
/// bytes of line terminators.
fn split_span(
    source: &str,
    line_starts: &[usize],
    lines: &mut [Vec<RawSpan>],
    range: Range<usize>,
    kind: TokenKind,
    index: u32,
) {
    let bytes = source.as_bytes();
    let mut line = line_starts.partition_point(|&start| start <= range.start) - 1;
    while line < line_starts.len() {
        let start = line_starts[line];
        if start >= range.end {
            break;
        }
        let next = line_starts.get(line + 1).copied().unwrap_or(source.len());
        let content_end = content_end(bytes, start, next);
        let from = range.start.max(start);
        let to = range.end.min(content_end);
        if from < to {
            lines[line].push(RawSpan {
                start: from - start,
                end: to - start,
                kind,
                index,
            });
        }
        line += 1;
    }
}

/// Flattens the raw spans of one line into sorted, non-overlapping spans where, per byte, the
/// highest precedence wins and the later original span wins ties; touching equal kinds merge
/// (TXT-007 R10).
fn normalize_line(mut raw: Vec<RawSpan>) -> Vec<Span> {
    if raw.is_empty() {
        return Vec::new();
    }
    let ordered = raw.windows(2).all(|pair| pair[0].end <= pair[1].start);
    let mut out: Vec<Span> = Vec::with_capacity(raw.len());
    let push = |out: &mut Vec<Span>, start: usize, end: usize, kind: TokenKind| {
        if let Some(last) = out.last_mut() {
            if last.kind == kind && last.range.end == start {
                last.range.end = end;
                return;
            }
        }
        out.push(Span {
            range: start..end,
            kind,
        });
    };
    if ordered {
        for span in &raw {
            push(&mut out, span.start, span.end, span.kind);
        }
        return out;
    }

    raw.sort_by_key(|span| (span.start, span.index));
    let mut boundaries: Vec<usize> = raw.iter().flat_map(|s| [s.start, s.end]).collect();
    boundaries.sort_unstable();
    boundaries.dedup();
    // Max-heap on (precedence, original index): the winner of an elementary segment is the top
    // after discarding spans that ended at or before the segment start.
    let mut active: BinaryHeap<(u8, u32, Reverse<usize>, u8)> = BinaryHeap::new();
    let mut next = 0;
    for window in boundaries.windows(2) {
        let (from, to) = (window[0], window[1]);
        while next < raw.len() && raw[next].start <= from {
            let span = &raw[next];
            active.push((
                span.kind.precedence(),
                span.index,
                Reverse(span.end),
                span.kind as u8,
            ));
            next += 1;
        }
        while active
            .peek()
            .is_some_and(|&(_, _, Reverse(end), _)| end <= from)
        {
            active.pop();
        }
        if let Some(&(_, _, _, kind)) = active.peek() {
            push(&mut out, from, to, TokenKind::ALL[kind as usize]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_starts_follow_the_spec_rules() {
        assert_eq!(line_starts(""), vec![0]);
        assert_eq!(line_starts("a"), vec![0]);
        assert_eq!(line_starts("a\n"), vec![0]);
        assert_eq!(line_starts("a\nb"), vec![0, 2]);
        assert_eq!(line_starts("a\n\n"), vec![0, 2]);
        assert_eq!(line_starts("\n"), vec![0]);
    }
}
