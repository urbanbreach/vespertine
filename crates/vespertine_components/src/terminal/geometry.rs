//! Pure geometry helpers of the terminal surface: metrics (CMP-021 R5), pointer mapping (R15),
//! edge auto-scroll speed (R18), wheel accumulation and the scrollbar mapping (R19) and the
//! dock height clamp (R21).

use super::emulator::Side;

/// Padding around the grid on every side, in logical pixels (R5).
pub const PADDING: f32 = 12.0;
/// Default terminal font size in logical pixels.
pub const DEFAULT_FONT_SIZE: f32 = 13.0;
/// The line height as a multiple of the font size: 18 / 13 (R5).
pub const LINE_HEIGHT_RATIO: f32 = 18.0 / 13.0;
/// Cell width as a multiple of the font size when the font cannot be measured (R5).
pub const FALLBACK_CELL_WIDTH_RATIO: f32 = 0.6;
/// Largest measured grid in either direction (R5).
pub const MAX_MEASURED: usize = 500;
/// Selection drag threshold in logical pixels (R16).
pub const DRAG_THRESHOLD: f32 = 2.0;
/// Minimum dock height in logical pixels (R21).
pub const DOCK_MIN_HEIGHT: f32 = 160.0;
/// Maximum dock height as a fraction of the window height (R21).
pub const DOCK_MAX_FRACTION: f32 = 0.55;
/// Measurement tolerance of the dock in logical pixels (R21).
pub const DOCK_TOLERANCE: f32 = 0.5;

/// The default line height for a font size, keeping the 18 / 13 ratio (R5).
pub fn default_line_height(font_size: f32) -> f32 {
    font_size * LINE_HEIGHT_RATIO
}

/// The grid size that fits `inner_width` x `inner_height` logical pixels:
/// `cols = floor(w / cell) in [2, 500]`, `rows = floor(h / line) in [1, 500]` (R5). Degenerate
/// metrics give the minimum grid.
pub fn grid_dimensions(
    inner_width: f32,
    inner_height: f32,
    cell_width: f32,
    line_height: f32,
) -> (usize, usize) {
    let cols = fit(inner_width, cell_width).clamp(2, MAX_MEASURED);
    let rows = fit(inner_height, line_height).clamp(1, MAX_MEASURED);
    (cols, rows)
}

fn fit(extent: f32, unit: f32) -> usize {
    if !extent.is_finite() || !unit.is_finite() || unit <= 0.0 || extent <= 0.0 {
        return 0;
    }
    (extent / unit).floor() as usize
}

/// The cell under a pointer position (R15).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellHit {
    /// Viewport row.
    pub row: usize,
    /// Column.
    pub col: usize,
    /// The side of the cell.
    pub side: Side,
}

/// Maps a pointer position relative to the grid origin to a cell hit (R15). Positions outside
/// the grid clamp to the nearest cell and overshoot forces the side: past the right edge or
/// below the bottom row the side is right, above the top row it is left. Non-finite coordinates
/// are 0; degenerate metrics return cell (0, 0) left.
pub fn cell_at(
    x: f32,
    y: f32,
    cell_width: f32,
    line_height: f32,
    cols: usize,
    rows: usize,
) -> CellHit {
    let origin = CellHit {
        row: 0,
        col: 0,
        side: Side::Left,
    };
    let degenerate = |value: f32| !value.is_finite() || value <= 0.0;
    if cols == 0 || rows == 0 || degenerate(cell_width) || degenerate(line_height) {
        return origin;
    }
    let x = if x.is_finite() { x } else { 0.0 };
    let y = if y.is_finite() { y } else { 0.0 };

    let raw_col = (x / cell_width).floor();
    let (col, mut side) = if raw_col < 0.0 {
        (0, Side::Left)
    } else if raw_col >= cols as f32 {
        (cols - 1, Side::Right)
    } else {
        let offset = x - raw_col * cell_width;
        let side = if offset <= cell_width / 2.0 {
            Side::Left
        } else {
            Side::Right
        };
        (raw_col as usize, side)
    };

    let raw_row = (y / line_height).floor();
    let row = if raw_row < 0.0 {
        side = Side::Left;
        0
    } else if raw_row >= rows as f32 {
        side = Side::Right;
        rows - 1
    } else {
        raw_row as usize
    };
    CellHit { row, col, side }
}

/// Lines to scroll per auto-scroll tick for a pointer at window `pointer_y` over a grid whose
/// top edge is `grid_top` (R18): positive into history above the top band, negative toward the
/// live bottom below the bottom band, 0 in the middle. The band is `min(line_height,
/// grid_height / 3)`; with `t = clamp(penetration / band, 0, 1)` the speed is
/// `round(1 + 2 t^2)` lines (1, 2 or 3).
pub fn edge_scroll_lines(pointer_y: f32, grid_top: f32, line_height: f32, rows: usize) -> i32 {
    if !pointer_y.is_finite() || !grid_top.is_finite() || !line_height.is_finite() {
        return 0;
    }
    let height = line_height * rows as f32;
    if height <= 0.0 {
        return 0;
    }
    let band = line_height.min(height / 3.0);
    let bottom = grid_top + height;
    let top_penetration = grid_top + band - pointer_y;
    let bottom_penetration = pointer_y - (bottom - band);
    let (penetration, sign) = if top_penetration > 0.0 {
        (top_penetration, 1)
    } else if bottom_penetration > 0.0 {
        (bottom_penetration, -1)
    } else {
        return 0;
    };
    let t = (penetration / band).clamp(0.0, 1.0);
    sign * (1.0 + 2.0 * t * t).round() as i32
}

/// Fractional wheel lines accumulated per tab (R19).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WheelAccumulator {
    remainder: f32,
}

impl WheelAccumulator {
    /// Adds `lines` (positive scrolls up into history) and returns the whole-line part,
    /// truncated toward zero, keeping the fraction.
    pub fn push(&mut self, lines: f32) -> i32 {
        if !lines.is_finite() {
            return 0;
        }
        let total = self.remainder + lines;
        let whole = total.trunc();
        self.remainder = total - whole;
        whole as i32
    }

    /// Drops the accumulated fraction (a touch or gesture phase started).
    pub fn reset(&mut self) {
        self.remainder = 0.0;
    }

    /// The accumulated fraction.
    pub fn remainder(&self) -> f32 {
        self.remainder
    }
}

/// The scrollback scrollbar mapping (R19), in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollbarMetrics {
    /// `rows x lineHeight`.
    pub viewport: f32,
    /// `(history + rows) x lineHeight`.
    pub content: f32,
    /// `(history - displayOffset) x lineHeight`: the emulator offset counts from the bottom,
    /// so it is mirrored.
    pub offset: f32,
}

/// Maps the scrollback state to scrollbar metrics (R19).
pub fn scrollbar_metrics(
    rows: usize,
    history: usize,
    display_offset: usize,
    line_height: f32,
) -> ScrollbarMetrics {
    let offset = history.saturating_sub(display_offset);
    ScrollbarMetrics {
        viewport: rows as f32 * line_height,
        content: (history + rows) as f32 * line_height,
        offset: offset as f32 * line_height,
    }
}

/// The terminal height at the bottom of a window (R21): clamped to `[160, max(55% of
/// window_height, 160)]`; a non-finite request becomes 160.
pub fn dock_height(request: f32, window_height: f32) -> f32 {
    if !request.is_finite() {
        return DOCK_MIN_HEIGHT;
    }
    let window = if window_height.is_finite() {
        window_height
    } else {
        0.0
    };
    let limit = (window * DOCK_MAX_FRACTION).max(DOCK_MIN_HEIGHT);
    request.clamp(DOCK_MIN_HEIGHT, limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(x: f32, y: f32) -> (usize, usize, Side) {
        let hit = cell_at(x, y, 10.0, 20.0, 8, 4);
        (hit.row, hit.col, hit.side)
    }

    #[test]
    fn pointer_mapping_landmarks() {
        assert_eq!(hit(0.0, 0.0), (0, 0, Side::Left));
        assert_eq!(hit(25.0, 45.0), (2, 2, Side::Left));
        assert_eq!(hit(26.0, 45.0), (2, 2, Side::Right));
        assert_eq!(hit(9999.0, 10.0), (0, 7, Side::Right));
        assert_eq!(hit(1.0, 9999.0), (3, 0, Side::Right));
        assert_eq!(hit(75.0, -50.0).2, Side::Left);
    }

    #[test]
    fn degenerate_inputs_do_not_panic() {
        let origin = CellHit {
            row: 0,
            col: 0,
            side: Side::Left,
        };
        assert_eq!(cell_at(5.0, 5.0, 0.0, 20.0, 8, 4), origin);
        assert_eq!(cell_at(5.0, 5.0, 10.0, 20.0, 0, 4), origin);
        assert_eq!(cell_at(5.0, 5.0, 10.0, 20.0, 8, 0), origin);
        assert_eq!(cell_at(5.0, 5.0, f32::NAN, 20.0, 8, 4), origin);
        assert_eq!(cell_at(5.0, 5.0, 10.0, -1.0, 8, 4), origin);
        assert_eq!(cell_at(f32::NAN, f32::INFINITY, 10.0, 20.0, 8, 4), origin);
    }

    #[test]
    fn edge_scroll_speed_follows_penetration() {
        // Grid top 20, line height 20, 10 rows: bottom 220, band 20.
        assert_eq!(edge_scroll_lines(28.0, 20.0, 20.0, 10), 2);
        assert_eq!(edge_scroll_lines(100.0, 20.0, 20.0, 10), 0);
        assert_eq!(edge_scroll_lines(208.0, 20.0, 20.0, 10), -1);
        assert_eq!(edge_scroll_lines(-500.0, 20.0, 20.0, 10), 3);
        assert_eq!(edge_scroll_lines(900.0, 20.0, 20.0, 10), -3);
        for step in 0..=400 {
            let y = step as f32;
            let lines = edge_scroll_lines(y, 20.0, 20.0, 10);
            if !(40.0..=200.0).contains(&y) {
                assert!((1..=3).contains(&lines.abs()), "y = {y}: {lines}");
            } else {
                assert_eq!(lines, 0, "y = {y}");
            }
        }
        assert_eq!(edge_scroll_lines(f32::NAN, 20.0, 20.0, 10), 0);
    }

    #[test]
    fn edge_band_shrinks_with_a_short_grid() {
        // Two rows of 30 px: grid height 60, band min(30, 20) = 20.
        assert_eq!(edge_scroll_lines(10.0, 10.0, 30.0, 2), 3);
        assert_eq!(edge_scroll_lines(15.0, 10.0, 30.0, 2), 2);
        assert_eq!(edge_scroll_lines(40.0, 10.0, 30.0, 2), 0);
    }

    #[test]
    fn wheel_remainder_accumulates_and_resets() {
        let mut wheel = WheelAccumulator::default();
        let line = |px: f32| px / 20.0;
        assert_eq!(wheel.push(line(5.0)), 0);
        assert_eq!(wheel.push(line(5.0)), 0);
        assert_eq!(wheel.push(line(5.0)), 0);
        assert_eq!(wheel.push(line(5.0)), 1);

        let mut wheel = WheelAccumulator::default();
        assert_eq!(wheel.push(0.25), 0);
        assert_eq!(wheel.push(0.75), 1);

        let mut wheel = WheelAccumulator::default();
        assert_eq!(wheel.push(0.5), 0);
        wheel.reset();
        assert_eq!(wheel.push(0.5), 0);

        let mut wheel = WheelAccumulator::default();
        assert_eq!(wheel.push(0.75), 0);
        assert_eq!(wheel.push(-1.0), 0);
        assert_eq!(wheel.push(-0.75), -1);
    }

    #[test]
    fn scrollbar_mapping_mirrors_the_offset() {
        let metrics = scrollbar_metrics(10, 90, 30, 20.0);
        assert_eq!(metrics.viewport, 200.0);
        assert_eq!(metrics.content, 2000.0);
        assert_eq!(metrics.offset, 1200.0);
        assert_eq!(scrollbar_metrics(10, 90, 0, 20.0).offset, 1800.0);
    }

    #[test]
    fn dock_height_clamps() {
        assert_eq!(dock_height(10.0, 1000.0), 160.0);
        assert_eq!(dock_height(f32::NAN, 1000.0), 160.0);
        assert_eq!(dock_height(f32::INFINITY, 1000.0), 160.0);
        assert_eq!(dock_height(900.0, 1000.0), 550.0);
        assert_eq!(dock_height(300.0, 1000.0), 300.0);
        assert_eq!(dock_height(900.0, 290.0), 160.0);
        assert_eq!(dock_height(200.0, 200.0), 160.0);
    }

    #[test]
    fn grid_dimensions_clamp() {
        assert_eq!(grid_dimensions(800.0, 400.0, 8.0, 18.0), (100, 22));
        assert_eq!(grid_dimensions(1.0, 1.0, 8.0, 18.0), (2, 1));
        assert_eq!(grid_dimensions(1.0e6, 1.0e6, 8.0, 18.0), (500, 500));
        assert_eq!(grid_dimensions(100.0, 100.0, 0.0, 18.0), (2, 5));
    }
}
