//! Shaped-layout geometry for carets, link hitboxes, search washes and selection overlays
//! (TXT-006 R4-R7).

use std::ops::Range;
use vespertine::{Bounds, Pixels, Point, TextLayout, point, px, size};

/// Hard guard of the range-rectangle walk: ranges spanning more rows are truncated (R6).
pub const MAX_RANGE_ROWS: usize = 256;

/// Row tolerance for comparing y positions.
const ROW_EPSILON: f32 = 0.01;

/// What the range-rectangle walk needs from a laid-out text.
///
/// Implemented by [`RichLayout`] for real shaped text; tests implement it with a model layout.
pub trait RowGeometry {
    /// Window position of the caret cell at `index`; `None` when outside the text. At a soft-wrap
    /// boundary the index ending a visual row is reported at the end of that row (R5).
    fn position_for_index(&self, index: usize) -> Option<Point<Pixels>>;
    /// Height of one visual row.
    fn line_height(&self) -> Pixels;
    /// Left edge of the layout; continuation rows start here.
    fn left_edge(&self) -> Pixels;
}

fn same_row(a: Pixels, b: Pixels) -> bool {
    (f32::from(a) - f32::from(b)).abs() < ROW_EPSILON
}

fn lower_row(a: Pixels, than: Pixels) -> bool {
    f32::from(a) > f32::from(than) + ROW_EPSILON
}

/// Wrapped range rectangles (R6): one rectangle per visual row the byte range `[start, end)`
/// touches, in top-to-bottom order, each exactly one line high.
///
/// `pad_x` is the horizontal overhang (inline-code pills) and `inset_y` the vertical shrink; both
/// are 0 for washes. Continuation rows start at the layout's left edge. At most
/// [`MAX_RANGE_ROWS`] rows are examined. Segments of zero width (an empty line inside the range)
/// produce no rectangle.
pub fn range_rects<G: RowGeometry + ?Sized>(
    geometry: &G,
    range: Range<usize>,
    pad_x: Pixels,
    inset_y: Pixels,
) -> Vec<Bounds<Pixels>> {
    let mut rects = Vec::new();
    if range.start >= range.end {
        return rects;
    }
    let (end_index, line_height, left) = (range.end, geometry.line_height(), geometry.left_edge());
    let mut cur = range.start;
    for _ in 0..MAX_RANGE_ROWS {
        let Some(mut p1) = geometry.position_for_index(cur) else {
            break;
        };
        // A range that begins exactly at a soft wrap starts on the next row.
        if let Some(next) = geometry.position_for_index(cur + 1)
            && lower_row(next.y, p1.y)
        {
            p1 = point(left, next.y);
        }
        let Some(p_end) = geometry.position_for_index(end_index) else {
            break;
        };
        let (p2, next_cur) = if same_row(p_end.y, p1.y) {
            (p_end, None)
        } else {
            // Greatest index in (cur, end) whose position is on p1's row.
            let (mut lo, mut hi) = (cur + 1, end_index);
            let mut found = None;
            while lo < hi {
                let mid = lo + (hi - lo) / 2;
                match geometry.position_for_index(mid) {
                    Some(p) if same_row(p.y, p1.y) || lower_row(p1.y, p.y) => {
                        found = Some((mid, p));
                        lo = mid + 1;
                    }
                    _ => hi = mid,
                }
            }
            match found {
                Some((index, p)) => (p, Some(index)),
                None => break,
            }
        };
        if p2.x > p1.x {
            rects.push(Bounds {
                origin: point(p1.x - pad_x, p1.y + inset_y),
                size: size(p2.x - p1.x + pad_x * 2.0, line_height - inset_y * 2.0),
            });
        }
        match next_cur {
            Some(index) if index > cur => cur = index,
            _ => break,
        }
    }
    rects
}

/// Result of a position lookup: an exact glyph hit or the nearest index (R4c).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexHit {
    /// The position lies on the glyph at this byte index.
    Exact(usize),
    /// The position is outside the text; this is the nearest index (start/end of the line, 0 above
    /// the text, the last index below it).
    Nearest(usize),
}

impl IndexHit {
    /// The byte index regardless of how it was found.
    pub fn index(self) -> usize {
        match self {
            IndexHit::Exact(index) | IndexHit::Nearest(index) => index,
        }
    }

    /// Whether the position hit a glyph.
    pub fn is_exact(self) -> bool {
        matches!(self, IndexHit::Exact(_))
    }
}

/// Metrics of one visual (wrapped) row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RowInfo {
    /// Byte index of the first character of the row.
    pub start: usize,
    /// Top of the row in window coordinates.
    pub top: Pixels,
    /// Font ascent of the row's line.
    pub ascent: Pixels,
    /// Font descent of the row's line.
    pub descent: Pixels,
}

impl RowInfo {
    /// Baseline offset from the row top: `(line_height - (ascent + descent)) / 2 + ascent` (R4d).
    pub fn baseline_offset(&self, line_height: Pixels) -> Pixels {
        (line_height - (self.ascent + self.descent)) / 2.0 + self.ascent
    }
}

/// A shaped, laid-out text with the geometry queries of TXT-006 R4.
///
/// Wraps the layout handle of a [`vespertine::StyledText`]; every query requires that the element
/// has been measured and prepainted this frame.
#[derive(Clone)]
pub struct RichLayout {
    layout: TextLayout,
}

impl RichLayout {
    /// Wraps a text layout handle.
    pub fn new(layout: TextLayout) -> Self {
        Self { layout }
    }

    /// The wrapped layout handle.
    pub fn text_layout(&self) -> &TextLayout {
        &self.layout
    }

    /// Overall bounds of the laid-out text.
    pub fn bounds(&self) -> Bounds<Pixels> {
        self.layout.bounds()
    }

    /// Height of one visual row.
    pub fn line_height(&self) -> Pixels {
        self.layout.line_height()
    }

    /// Length of the text in bytes.
    pub fn len(&self) -> usize {
        self.layout.len()
    }

    /// Whether the text is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Every visual row: starting byte index, top and the line's ascent/descent (R4b, R4d).
    pub fn rows(&self) -> Vec<RowInfo> {
        let line_height = self.line_height();
        let mut top = self.bounds().origin.y;
        let mut base = 0;
        let mut rows = Vec::new();
        for line in self.layout.line_layouts() {
            let (ascent, descent) = (line.ascent(), line.descent());
            let mut push = |start: usize, top: Pixels| {
                rows.push(RowInfo {
                    start,
                    top,
                    ascent,
                    descent,
                })
            };
            push(base, top);
            for boundary in line.wrap_boundaries() {
                top += line_height;
                let glyph = &line.runs()[boundary.run_ix].glyphs[boundary.glyph_ix];
                push(base + glyph.index, top);
            }
            top += line_height;
            base += line.len() + 1;
        }
        rows
    }

    /// Starting byte index of every visual row.
    pub fn line_starts(&self) -> Vec<usize> {
        self.rows().iter().map(|row| row.start).collect()
    }

    /// Width of the text without wrapping: the widest hard line shaped on one row (R4e).
    pub fn unwrapped_width(&self) -> Pixels {
        self.layout
            .line_layouts()
            .iter()
            .map(|line| line.unwrapped_layout.width)
            .fold(px(0.), Pixels::max)
    }

    /// Window position (top-left of the caret cell) of a byte index (R4c).
    pub fn position_for_index(&self, index: usize) -> Option<Point<Pixels>> {
        self.layout.position_for_index(index)
    }

    /// Byte index at a window position, distinguishing an exact hit from the nearest index (R4c).
    pub fn index_for_position(&self, position: Point<Pixels>) -> IndexHit {
        match self.layout.index_for_position(position) {
            Ok(index) => IndexHit::Exact(index),
            Err(index) => IndexHit::Nearest(index),
        }
    }

    /// Wrapped range rectangles for `[start, end)` (R6); offsets past the text are clamped.
    pub fn range_rects(
        &self,
        range: Range<usize>,
        pad_x: Pixels,
        inset_y: Pixels,
    ) -> Vec<Bounds<Pixels>> {
        let len = self.len();
        range_rects(
            self,
            range.start.min(len)..range.end.min(len),
            pad_x,
            inset_y,
        )
    }
}

impl RowGeometry for RichLayout {
    fn position_for_index(&self, index: usize) -> Option<Point<Pixels>> {
        RichLayout::position_for_index(self, index)
    }

    fn line_height(&self) -> Pixels {
        RichLayout::line_height(self)
    }

    fn left_edge(&self) -> Pixels {
        self.bounds().origin.x
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Model layout: rows of `row_len` bytes, 10 px per glyph; the index ending a row is reported
    /// at the end of that row (upstream affinity), the first index of the next row after its
    /// first glyph's start... exactly as the specification's A4 model.
    struct Model {
        row_len: usize,
        rows: usize,
        left: f32,
    }

    impl RowGeometry for Model {
        fn position_for_index(&self, index: usize) -> Option<Point<Pixels>> {
            if index > self.row_len * self.rows {
                return None;
            }
            // The boundary index belongs to the end of the row it closes.
            let row = if index == 0 {
                0
            } else {
                (index - 1) / self.row_len
            };
            let column = index - row * self.row_len;
            Some(point(
                px(self.left + column as f32 * 10.0),
                px(row as f32 * 20.0),
            ))
        }

        fn line_height(&self) -> Pixels {
            px(20.0)
        }

        fn left_edge(&self) -> Pixels {
            px(self.left)
        }
    }

    fn model() -> Model {
        Model {
            row_len: 5,
            rows: 3,
            left: 0.0,
        }
    }

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds {
            origin: point(px(x), px(y)),
            size: size(px(w), px(h)),
        }
    }

    #[test]
    fn range_starting_at_a_wrap_boundary_starts_on_the_next_row() {
        let m = model();
        assert_eq!(m.position_for_index(5), Some(point(px(50.), px(0.))));
        assert_eq!(m.position_for_index(6), Some(point(px(10.), px(20.))));
        let rects = range_rects(&m, 5..9, px(0.), px(0.));
        assert_eq!(rects, vec![rect(0., 20., 40., 20.)]);
    }

    #[test]
    fn range_over_a_wrap_yields_a_rectangle_per_row() {
        let m = model();
        let rects = range_rects(&m, 2..9, px(0.), px(0.));
        assert_eq!(
            rects,
            vec![rect(20., 0., 30., 20.), rect(0., 20., 40., 20.)]
        );
    }

    #[test]
    fn empty_and_reversed_ranges_yield_nothing() {
        let m = model();
        assert!(range_rects(&m, 3..3, px(0.), px(0.)).is_empty());
        let reversed = Range { start: 4, end: 2 };
        assert!(range_rects(&m, reversed, px(0.), px(0.)).is_empty());
    }

    #[test]
    fn padding_and_inset_apply_to_every_rectangle() {
        let m = model();
        let rects = range_rects(&m, 1..3, px(2.), px(3.));
        assert_eq!(rects, vec![rect(8., 3., 24., 14.)]);
    }

    #[test]
    fn walk_is_guarded_at_256_rows() {
        let m = Model {
            row_len: 2,
            rows: 300,
            left: 0.0,
        };
        let rects = range_rects(&m, 0..600, px(0.), px(0.));
        assert!(rects.len() <= MAX_RANGE_ROWS, "{}", rects.len());
        assert!(rects.len() > 250);
        // Rows are contiguous: no vertical gaps.
        for pair in rects.windows(2) {
            assert_eq!(pair[0].bottom(), pair[1].top());
        }
    }

    #[test]
    fn range_past_the_text_does_not_panic() {
        let m = model();
        assert!(range_rects(&m, 14..99, px(0.), px(0.)).len() <= 1);
    }
}
