//! Wrapped geometry of the multiline editor (CMP-008 R3-R5, R11-R13).
//!
//! Every logical line is shaped separately, wrapped to the width minus its continuation indent
//! and split into visual rows. All positions here are in DISPLAY bytes (the projected text) and
//! content coordinates (origin at the top-left of the content box, before scrolling).

use crate::text::{common::closest_boundary, core::Affinity};
use std::{ops::Range, rc::Rc};
use vespertine::{
    Bounds, Pixels, Point, ShapedLine, SharedString, TextRun, Window, point, px, size,
};

/// Minimum shaping width of a line (R3).
pub const MIN_SHAPE_WIDTH: f32 = 20.0;

/// One shaped logical line split into visual rows.
pub struct LineShape {
    /// The shaped rows, each rebased to its own origin.
    pub rows: Vec<ShapedLine>,
    /// Start offset of every row within the line text.
    pub starts: Vec<usize>,
    /// Length of the line text (without a trailing carriage return).
    pub len: usize,
    /// Indent of the continuation rows.
    pub indent: Pixels,
}

impl LineShape {
    /// Shapes `text` with `runs`, wrapping to `wrap_width` (never below 20 px) and splitting
    /// into rows; `breaks` overrides the default wrap points with explicit row starts.
    pub fn shape(
        window: &mut Window,
        text: &str,
        runs: &[TextRun],
        font_size: Pixels,
        wrap_width: Pixels,
        indent: Pixels,
        breaks: Option<Vec<usize>>,
    ) -> Self {
        let text_system = window.text_system().clone();
        let shared = SharedString::from(text.to_owned());
        let full = text_system.shape_line(shared.clone(), font_size, runs, None);
        let mut starts = vec![0];
        if !text.is_empty() {
            let boundaries = match breaks {
                Some(breaks) => breaks,
                None => {
                    let width = wrap_width.max(px(MIN_SHAPE_WIDTH));
                    match text_system.shape_text(shared, font_size, runs, Some(width), None) {
                        Ok(lines) => lines
                            .first()
                            .map(|line| {
                                line.wrap_boundaries()
                                    .iter()
                                    .map(|boundary| {
                                        line.unwrapped_layout.runs[boundary.run_ix].glyphs
                                            [boundary.glyph_ix]
                                            .index
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                        Err(_) => Vec::new(),
                    }
                }
            };
            for boundary in boundaries {
                if boundary > *starts.last().unwrap_or(&0)
                    && boundary < text.len()
                    && text.is_char_boundary(boundary)
                {
                    starts.push(boundary);
                }
            }
        }
        let mut rows = Vec::with_capacity(starts.len());
        if starts.len() == 1 {
            rows.push(full);
        } else {
            let mut cursor = full.cursor();
            for next in starts.iter().skip(1) {
                rows.push(cursor.take_until(*next));
            }
            rows.push(cursor.take_until(text.len()));
        }
        Self {
            rows,
            starts,
            len: text.len(),
            indent,
        }
    }

    /// Number of visual rows.
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// End offset of a row within the line.
    pub fn row_end(&self, row: usize) -> usize {
        self.starts.get(row + 1).copied().unwrap_or(self.len)
    }

    /// The row holding `rel` with the given affinity: a boundary offset belongs to the lower row
    /// downstream and to the upper row upstream.
    pub fn row_for(&self, rel: usize, affinity: Affinity) -> usize {
        let rel = rel.min(self.len);
        let below = self.starts.partition_point(|start| *start <= rel);
        let mut row = below.saturating_sub(1);
        if affinity == Affinity::Upstream && row > 0 && self.starts[row] == rel {
            row -= 1;
        }
        row
    }

    fn row_origin(&self, row: usize) -> Pixels {
        if row == 0 { px(0.0) } else { self.indent }
    }

    /// The x of `rel` on `row` (content coordinates, indent included).
    pub fn x_for(&self, rel: usize, row: usize) -> Pixels {
        let start = self.starts[row];
        let local = rel.clamp(start, self.row_end(row)) - start;
        self.row_origin(row) + self.rows[row].x_for_index(local)
    }

    /// The offset and affinity at `x` on `row`: the nearest boundary, upstream when it is the
    /// end of a row that is followed by another one (R4).
    pub fn hit(&self, row: usize, x: Pixels) -> (usize, Affinity) {
        let local = x - self.row_origin(row);
        let rel = self.starts[row] + closest_boundary(&self.rows[row], local);
        let end = self.row_end(row);
        let rel = rel.min(end);
        if rel == end && row + 1 < self.rows.len() {
            (rel, Affinity::Upstream)
        } else {
            (rel, Affinity::Downstream)
        }
    }

    /// The width of the widest row.
    pub fn max_width(&self) -> Pixels {
        self.rows
            .iter()
            .enumerate()
            .map(|(row, line)| self.row_origin(row) + line.width())
            .fold(px(0.0), Pixels::max)
    }
}

/// A line placed in the document.
pub struct LineLayout {
    /// The shaped rows.
    pub shape: Rc<LineShape>,
    /// Offset of the line start in the display text.
    pub display_start: usize,
    /// Length including a trailing carriage return.
    pub full_len: usize,
    /// Content-space top of the line.
    pub top: Pixels,
}

/// The part of a display range on one visual row (content coordinates, one line tall).
#[derive(Clone, Debug, PartialEq)]
pub struct RangeSegment {
    /// The segment's box.
    pub bounds: Bounds<Pixels>,
    /// The display range on this row.
    pub range: Range<usize>,
    /// Whether the segment holds the first character of the whole range.
    pub starts: bool,
    /// Whether the segment holds the last character of the whole range.
    pub ends: bool,
}

/// A visual row of the document.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VisualRow {
    /// Index of the logical line.
    pub line: usize,
    /// Row index within the line.
    pub row: usize,
    /// Content-space top.
    pub top: Pixels,
    /// Start offset in the display text.
    pub start: usize,
    /// End offset in the display text (excluding a trailing carriage return).
    pub end: usize,
}

/// The wrapped geometry of the whole display text.
pub struct AreaLayout {
    /// The lines.
    pub lines: Vec<LineLayout>,
    /// Row height.
    pub line_height: Pixels,
    /// Height of all rows, at least one line.
    pub content_height: Pixels,
    /// Length of the display text.
    pub display_len: usize,
    /// Shaping width used.
    pub width: Pixels,
}

impl AreaLayout {
    /// Places the shaped lines; `full_lens` holds each line's length including a trailing CR.
    pub fn new(shapes: Vec<(Rc<LineShape>, usize)>, line_height: Pixels, width: Pixels) -> Self {
        let mut lines = Vec::with_capacity(shapes.len());
        let mut display_start = 0;
        let mut top = px(0.0);
        for (shape, full_len) in shapes {
            let rows = shape.row_count();
            lines.push(LineLayout {
                shape,
                display_start,
                full_len,
                top,
            });
            display_start += full_len + 1;
            top += line_height * rows;
        }
        let display_len = display_start.saturating_sub(1);
        Self {
            lines,
            line_height,
            content_height: top.max(line_height),
            display_len,
            width,
        }
    }

    /// Total number of visual rows.
    pub fn row_count(&self) -> usize {
        self.lines.iter().map(|line| line.shape.row_count()).sum()
    }

    fn line_index(&self, offset: usize) -> usize {
        self.lines
            .partition_point(|line| line.display_start <= offset)
            .saturating_sub(1)
    }

    /// The visual row holding `offset` with the given affinity.
    pub fn row_at(&self, offset: usize, affinity: Affinity) -> VisualRow {
        let line_ix = self.line_index(offset.min(self.display_len));
        let line = &self.lines[line_ix];
        let rel = offset
            .saturating_sub(line.display_start)
            .min(line.shape.len);
        let row = line.shape.row_for(rel, affinity);
        self.visual_row(line_ix, row)
    }

    fn visual_row(&self, line_ix: usize, row: usize) -> VisualRow {
        let line = &self.lines[line_ix];
        VisualRow {
            line: line_ix,
            row,
            top: line.top + self.line_height * row,
            start: line.display_start + line.shape.starts[row],
            end: line.display_start + line.shape.row_end(row),
        }
    }

    /// The previous visual row.
    pub fn row_above(&self, row: VisualRow) -> Option<VisualRow> {
        if row.row > 0 {
            Some(self.visual_row(row.line, row.row - 1))
        } else if row.line > 0 {
            let line = row.line - 1;
            Some(self.visual_row(line, self.lines[line].shape.row_count() - 1))
        } else {
            None
        }
    }

    /// The next visual row.
    pub fn row_below(&self, row: VisualRow) -> Option<VisualRow> {
        if row.row + 1 < self.lines[row.line].shape.row_count() {
            Some(self.visual_row(row.line, row.row + 1))
        } else if row.line + 1 < self.lines.len() {
            Some(self.visual_row(row.line + 1, 0))
        } else {
            None
        }
    }

    /// Every visual row in order.
    pub fn rows(&self) -> Vec<VisualRow> {
        let mut out = Vec::with_capacity(self.row_count());
        for (line_ix, line) in self.lines.iter().enumerate() {
            for row in 0..line.shape.row_count() {
                out.push(self.visual_row(line_ix, row));
            }
        }
        out
    }

    /// The content-space position of the caret cell at a display offset (top-left of the cell).
    pub fn point_for(&self, offset: usize, affinity: Affinity) -> Point<Pixels> {
        let row = self.row_at(offset, affinity);
        let line = &self.lines[row.line];
        let rel = offset.saturating_sub(line.display_start);
        point(line.shape.x_for(rel, row.row), row.top)
    }

    /// The display offset and affinity at a content-space point (R4): above the content maps to
    /// offset 0 (downstream), below the last row to the end (upstream).
    pub fn hit(&self, at: Point<Pixels>) -> (usize, Affinity) {
        if at.y < px(0.0) {
            return (0, Affinity::Downstream);
        }
        let last = self.lines.len() - 1;
        let last_line = &self.lines[last];
        if at.y >= last_line.top + self.line_height * last_line.shape.row_count() {
            return (self.display_len, Affinity::Upstream);
        }
        let line_ix = self
            .lines
            .partition_point(|line| line.top <= at.y)
            .saturating_sub(1);
        let line = &self.lines[line_ix];
        let row = (((at.y - line.top) / self.line_height) as usize).min(line.shape.row_count() - 1);
        let (rel, affinity) = line.shape.hit(row, at.x);
        (line.display_start + rel, affinity)
    }

    /// The offset reached on `row` at `x` (vertical movement, R6).
    pub fn offset_on_row(&self, row: VisualRow, x: Pixels) -> (usize, Affinity) {
        let line = &self.lines[row.line];
        let (rel, affinity) = line.shape.hit(row.row, x);
        (line.display_start + rel, affinity)
    }

    /// Selection rectangles for a display range (R5): a single rectangle on one row; otherwise
    /// the first row from the start to the right edge of the content box, full-width middle
    /// rows (none when the selection spans exactly two rows) and the last row from the left
    /// edge to the end point. Zero-width pieces on a single row produce nothing.
    pub fn selection_rects(
        &self,
        range: Range<usize>,
        content_width: Pixels,
    ) -> Vec<Bounds<Pixels>> {
        let mut rects = Vec::new();
        if range.start >= range.end {
            return rects;
        }
        let start_row = self.row_at(range.start, Affinity::Downstream);
        let end_row = self.row_at(range.end, Affinity::Upstream);
        let start = self.point_for(range.start, Affinity::Downstream);
        let end = self.point_for(range.end, Affinity::Upstream);
        let lh = self.line_height;
        let rect = |left: Pixels, top: Pixels, right: Pixels| Bounds {
            origin: point(left, top),
            size: size((right - left).max(px(0.0)), lh),
        };
        if start_row.line == end_row.line && start_row.row == end_row.row {
            if end.x > start.x {
                rects.push(rect(start.x, start_row.top, end.x));
            }
            return rects;
        }
        rects.push(rect(start.x, start_row.top, content_width.max(start.x)));
        let mut cursor = self.row_below(start_row);
        while let Some(row) = cursor {
            if row.line == end_row.line && row.row == end_row.row {
                break;
            }
            rects.push(rect(px(0.0), row.top, content_width));
            cursor = self.row_below(row);
        }
        let left = if end_row.row == 0 {
            px(0.0)
        } else {
            self.lines[end_row.line].shape.indent
        };
        if end.x > left {
            rects.push(rect(left, end_row.top, end.x));
        }
        rects
    }

    /// The per-row segments of a display range in content coordinates, one line-height tall: what
    /// washes and pills are painted over. Rows without any character of the range produce
    /// nothing; `starts`/`ends` tell whether a segment holds the range's first/last character.
    pub fn range_segments(&self, range: Range<usize>) -> Vec<RangeSegment> {
        let mut segments = Vec::new();
        if range.start >= range.end {
            return segments;
        }
        let mut cursor = Some(self.row_at(range.start, Affinity::Downstream));
        while let Some(row) = cursor {
            if row.start >= range.end {
                break;
            }
            let start = range.start.max(row.start);
            let end = range.end.min(row.end);
            if start < end {
                let line = &self.lines[row.line];
                let left = line.shape.x_for(start - line.display_start, row.row);
                let right = line.shape.x_for(end - line.display_start, row.row);
                segments.push(RangeSegment {
                    bounds: Bounds {
                        origin: point(left, row.top),
                        size: size((right - left).max(px(0.0)), self.line_height),
                    },
                    range: start..end,
                    starts: start == range.start,
                    ends: end == range.end,
                });
            }
            cursor = self.row_below(row);
        }
        segments
    }
}

/// The height of a box holding `content` plus padding, clamped to the bounds (R11).
pub fn auto_height(content: Pixels, padding: Pixels, min: Pixels, max: Pixels) -> Pixels {
    (content + padding).max(min).min(max.max(min))
}

/// The row bounds of an editor: `min_rows` at least 1, `max_rows` at least `min_rows` (R11).
pub fn row_bounds(min_rows: usize, max_rows: usize, line_height: Pixels) -> (Pixels, Pixels) {
    let min_rows = min_rows.max(1);
    let max_rows = max_rows.max(min_rows);
    (line_height * min_rows, line_height * max_rows)
}

/// Clamps a vertical scroll to `[0, max(0, content - viewport)]`.
pub fn clamp_scroll(scroll: Pixels, content: Pixels, viewport: Pixels) -> Pixels {
    scroll.max(px(0.0)).min((content - viewport).max(px(0.0)))
}

/// The minimal scroll that makes the caret row fully visible (R12).
pub fn follow_scroll(
    scroll: Pixels,
    caret_top: Pixels,
    caret_height: Pixels,
    content: Pixels,
    viewport: Pixels,
) -> Pixels {
    let scroll = if caret_top < scroll {
        caret_top
    } else if caret_top + caret_height > scroll + viewport {
        caret_top + caret_height - viewport
    } else {
        scroll
    };
    clamp_scroll(scroll, content, viewport)
}

/// The painted height while the box animates between heights: whole rows only (R13).
pub fn painted_height(scroll: Pixels, visible: Pixels, line_height: Pixels) -> Pixels {
    let lh = f32::from(line_height).max(1.0);
    let row_end = ((f32::from(scroll) + f32::from(visible) + 0.001) / lh).floor() * lh;
    px((row_end - f32::from(scroll)).clamp(0.0, f32::from(visible)))
}

/// Whether content is hidden past the top or bottom edge of the SETTLED viewport (R13).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OverflowEdges {
    /// Content is hidden above the viewport.
    pub top_hidden: bool,
    /// Content is hidden below the viewport.
    pub bottom_hidden: bool,
}

/// The overflow edge flags: both false when the maximum scroll at the settled height is at most
/// 1 px; otherwise the top edge is hidden when the scroll exceeds 1 px and the bottom edge
/// when it is more than 1 px short of the maximum.
pub fn overflow_edges(scroll: Pixels, content: Pixels, settled_viewport: Pixels) -> OverflowEdges {
    let max = f32::from((content - settled_viewport).max(px(0.0)));
    if max <= 1.0 {
        return OverflowEdges::default();
    }
    let scroll = f32::from(scroll);
    OverflowEdges {
        top_hidden: scroll > 1.0,
        bottom_hidden: scroll < max - 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_height_clamps_between_the_row_bounds() {
        let (min, max) = row_bounds(1, 10, px(20.0));
        let height =
            |rows: usize| f32::from(auto_height(px(20.0 * rows as f32), px(0.0), min, max));
        assert_eq!(height(1), 20.0);
        assert_eq!(height(5), 100.0);
        assert_eq!(height(10), 200.0);
        assert_eq!(height(25), 200.0);
        let (min, max) = row_bounds(0, 0, px(20.0));
        assert_eq!((f32::from(min), f32::from(max)), (20.0, 20.0));
        let (_, max) = row_bounds(3, 1, px(20.0));
        assert_eq!(f32::from(max), 60.0);
    }

    #[test]
    fn scroll_is_clamped_and_follows_with_the_minimal_adjustment() {
        let (content, viewport, lh) = (px(500.0), px(200.0), px(20.0));
        assert_eq!(f32::from(clamp_scroll(px(-5.0), content, viewport)), 0.0);
        assert_eq!(f32::from(clamp_scroll(px(999.0), content, viewport)), 300.0);
        assert_eq!(f32::from(clamp_scroll(px(10.0), px(100.0), viewport)), 0.0);
        // Moving the caret from the top to the last row and back.
        let down = follow_scroll(px(0.0), px(480.0), lh, content, viewport);
        assert_eq!(f32::from(down), 300.0);
        let inside = follow_scroll(down, px(400.0), lh, content, viewport);
        assert_eq!(f32::from(inside), 300.0);
        let up = follow_scroll(down, px(280.0), lh, content, viewport);
        assert_eq!(f32::from(up), 280.0);
        let top = follow_scroll(up, px(0.0), lh, content, viewport);
        assert_eq!(f32::from(top), 0.0);
    }

    #[test]
    fn painted_height_is_a_whole_number_of_rows_while_resizing() {
        assert_eq!(f32::from(painted_height(px(0.0), px(70.0), px(20.0))), 60.0);
        assert_eq!(f32::from(painted_height(px(0.0), px(80.0), px(20.0))), 80.0);
        assert_eq!(f32::from(painted_height(px(5.0), px(70.0), px(20.0))), 55.0);
        assert_eq!(f32::from(painted_height(px(0.0), px(10.0), px(20.0))), 0.0);
    }

    #[test]
    fn overflow_edges_use_the_settled_overflow_with_a_one_pixel_tolerance() {
        let settled = px(120.0);
        assert_eq!(
            overflow_edges(px(0.0), px(121.0), settled),
            OverflowEdges::default()
        );
        assert_eq!(
            overflow_edges(px(0.0), px(200.0), settled),
            OverflowEdges {
                top_hidden: false,
                bottom_hidden: true
            }
        );
        assert_eq!(
            overflow_edges(px(40.0), px(200.0), settled),
            OverflowEdges {
                top_hidden: true,
                bottom_hidden: true
            }
        );
        assert_eq!(
            overflow_edges(px(80.0), px(200.0), settled),
            OverflowEdges {
                top_hidden: true,
                bottom_hidden: false
            }
        );
    }
}
