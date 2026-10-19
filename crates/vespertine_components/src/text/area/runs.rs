//! Style spans and shaping runs of the multiline editor (CMP-008 R16, CMP-009 R2/R3).
//!
//! Spans are expressed over SOURCE bytes by their producers (Markdown faces, syntax colors, an
//! owner's highlighter) and mapped through the projection to display bytes; the runs of every
//! logical line merge them at every boundary.

use std::ops::Range;
use vespertine::{
    FontStyle, FontWeight, Hsla, SharedString, StrikethroughStyle, TextRun, UnderlineStyle, px,
};

/// The visual style of a span; overlapping spans combine (bold and italic add up, the last
/// color wins).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SpanStyle {
    /// Text color override.
    pub color: Option<Hsla>,
    /// Bold weight.
    pub bold: bool,
    /// Italic style.
    pub italic: bool,
    /// A 1 px line through the text in the text color.
    pub strikethrough: bool,
    /// A 1 px underline in the text color.
    pub underline: bool,
    /// The monospace family of the theme (code).
    pub monospace: bool,
    /// A wash painted behind the run.
    pub background: Option<Hsla>,
}

impl SpanStyle {
    /// Bold.
    pub fn bold() -> Self {
        Self {
            bold: true,
            ..Self::default()
        }
    }

    /// Italic.
    pub fn italic() -> Self {
        Self {
            italic: true,
            ..Self::default()
        }
    }

    /// A color.
    pub fn color(color: Hsla) -> Self {
        Self {
            color: Some(color),
            ..Self::default()
        }
    }

    fn combine(self, other: SpanStyle) -> SpanStyle {
        SpanStyle {
            color: other.color.or(self.color),
            bold: self.bold || other.bold,
            italic: self.italic || other.italic,
            strikethrough: self.strikethrough || other.strikethrough,
            underline: self.underline || other.underline,
            monospace: self.monospace || other.monospace,
            background: other.background.or(self.background),
        }
    }
}

/// The base appearance every run starts from.
#[derive(Clone, Debug)]
pub struct BaseStyle {
    /// The body font.
    pub font: vespertine::Font,
    /// The family used by monospace spans.
    pub monospace_family: SharedString,
    /// The text color.
    pub color: Hsla,
}

/// Maps a span style over `base` to a text run of `len` bytes.
pub fn run_for(base: &BaseStyle, style: SpanStyle, len: usize, marked: bool) -> TextRun {
    let mut font = base.font.clone();
    if style.bold {
        font.weight = FontWeight::BOLD;
    }
    if style.italic {
        font.style = FontStyle::Italic;
    }
    if style.monospace {
        font.family = base.monospace_family.clone();
    }
    let color = style.color.unwrap_or(base.color);
    TextRun {
        len,
        font,
        color,
        // A span's wash is painted by the element as a rounded quad per row segment
        // (CMP-009 R2), never as the rectangle of the shaped run.
        background_color: None,
        underline: (style.underline || marked).then_some(UnderlineStyle {
            color: Some(base.color),
            thickness: px(1.0),
            wavy: false,
        }),
        strikethrough: style.strikethrough.then_some(StrikethroughStyle {
            color: Some(base.color),
            thickness: px(1.0),
        }),
    }
}

/// An incremental selection of the spans that overlap consecutive lines: spans are sorted by
/// start and visited once, so building the runs of a whole document is linear.
pub struct SpanSweep<'a> {
    spans: &'a [(Range<usize>, SpanStyle)],
    next: usize,
    active: Vec<usize>,
}

impl<'a> SpanSweep<'a> {
    /// A sweep over spans sorted by start offset.
    pub fn new(spans: &'a [(Range<usize>, SpanStyle)]) -> Self {
        Self {
            spans,
            next: 0,
            active: Vec::new(),
        }
    }

    /// The spans overlapping `line` (display bytes). Lines must be visited in increasing order.
    pub fn for_line(&mut self, line: Range<usize>) -> Vec<(Range<usize>, SpanStyle)> {
        self.active
            .retain(|&index| self.spans[index].0.end > line.start);
        while self.next < self.spans.len() && self.spans[self.next].0.start < line.end {
            if self.spans[self.next].0.end > line.start {
                self.active.push(self.next);
            }
            self.next += 1;
        }
        let mut found: Vec<_> = self
            .active
            .iter()
            .map(|&index| self.spans[index].clone())
            .filter(|(range, _)| range.start < line.end && range.end > line.start)
            .collect();
        found.sort_by_key(|(range, _)| range.start);
        found
    }
}

/// The runs of one line: `line` is the line's display byte range, `spans` the spans overlapping
/// it (display bytes) and `marked` the marked range in display bytes. The runs always cover the
/// line exactly.
pub fn line_runs(
    base: &BaseStyle,
    line: Range<usize>,
    spans: &[(Range<usize>, SpanStyle)],
    marked: Option<&Range<usize>>,
) -> Vec<TextRun> {
    if line.is_empty() {
        return Vec::new();
    }
    let mut points = vec![line.start, line.end];
    for (range, _) in spans {
        points.push(range.start.clamp(line.start, line.end));
        points.push(range.end.clamp(line.start, line.end));
    }
    if let Some(marked) = marked {
        points.push(marked.start.clamp(line.start, line.end));
        points.push(marked.end.clamp(line.start, line.end));
    }
    points.sort_unstable();
    points.dedup();
    let mut runs: Vec<TextRun> = Vec::new();
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let style = spans
            .iter()
            .filter(|(range, _)| range.start <= a && range.end >= b && range.start < range.end)
            .fold(SpanStyle::default(), |acc, (_, style)| acc.combine(*style));
        let is_marked = marked.is_some_and(|m| m.start <= a && m.end >= b && m.start < m.end);
        let run = run_for(base, style, b - a, is_marked);
        match runs.last_mut() {
            Some(last) if same_look(last, &run) => last.len += run.len,
            _ => runs.push(run),
        }
    }
    runs
}

fn same_look(a: &TextRun, b: &TextRun) -> bool {
    let mut b = b.clone();
    b.len = a.len;
    *a == b
}

#[cfg(test)]
mod tests {
    use super::*;
    use vespertine::{font, hsla};

    fn base() -> BaseStyle {
        BaseStyle {
            font: font("Test Sans"),
            monospace_family: "Test Mono".into(),
            color: hsla(0.0, 0.0, 1.0, 1.0),
        }
    }

    #[test]
    fn faces_combine_and_runs_cover_the_line_exactly() {
        let spans = vec![
            (0..10, SpanStyle::bold()),
            (4..8, SpanStyle::italic()),
            (
                6..9,
                SpanStyle {
                    monospace: true,
                    ..SpanStyle::default()
                },
            ),
        ];
        let runs = line_runs(&base(), 0..12, &spans, None);
        assert_eq!(runs.iter().map(|run| run.len).sum::<usize>(), 12);
        let lens: Vec<usize> = runs.iter().map(|run| run.len).collect();
        assert_eq!(lens, [4, 2, 2, 1, 1, 2]);
        assert_eq!(runs[0].font.weight, FontWeight::BOLD);
        assert_eq!(runs[1].font.style, FontStyle::Italic);
        assert_eq!(runs[1].font.weight, FontWeight::BOLD);
        assert_eq!(runs[2].font.family.as_ref(), "Test Mono");
        assert_eq!(runs[5].font.weight, FontWeight::NORMAL);
    }

    #[test]
    fn marked_text_is_underlined_and_adjacent_equal_runs_merge() {
        let runs = line_runs(&base(), 0..6, &[], Some(&(2..4)));
        let lens: Vec<usize> = runs.iter().map(|run| run.len).collect();
        assert_eq!(lens, [2, 2, 2]);
        assert!(runs[1].underline.is_some());
        assert!(runs[0].underline.is_none());
        let runs = line_runs(&base(), 0..6, &[(0..3, SpanStyle::default())], None);
        assert_eq!(runs.len(), 1);
    }

    #[test]
    fn sweep_returns_only_the_spans_of_each_line_in_order() {
        let spans = vec![
            (0..3, SpanStyle::bold()),
            (2..14, SpanStyle::italic()),
            (12..13, SpanStyle::bold()),
        ];
        let mut sweep = SpanSweep::new(&spans);
        assert_eq!(sweep.for_line(0..5).len(), 2);
        assert_eq!(sweep.for_line(6..11).len(), 1);
        assert_eq!(sweep.for_line(12..13).len(), 2);
        assert_eq!(sweep.for_line(14..20).len(), 0);
    }
}
