//! Small element builders and paint helpers shared by the component trees.

use vespertine::{App, Div, Pixels, Point, ShapedLine, Styled, TextAlign, Window, div};

/// A flex row whose children are centered on the cross axis.
pub(crate) fn centered_row() -> Div {
    div().flex_row().flex().items_center()
}

/// A flex column whose children are centered on the cross axis.
pub(crate) fn centered_column() -> Div {
    div().flex_col().flex().items_center()
}

/// A flex item that takes the space left by its siblings and may shrink below its content width.
pub(crate) fn flexible_cell() -> Div {
    div().min_w_0().flex_1()
}

/// Paints a shaped line left-aligned at `origin`; a line that fails to paint stays blank.
pub(crate) fn paint_left_aligned(
    line: &ShapedLine,
    origin: Point<Pixels>,
    line_height: Pixels,
    window: &mut Window,
    cx: &mut App,
) {
    let _ = line.paint(origin, line_height, TextAlign::Left, None, window, cx);
}
