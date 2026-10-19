//! A developer overlay that paints frame-time statistics directly into the
//! scene, bypassing layout, text, and view invalidation entirely (to avoid
//! infinitely triggering new frames).

use crate::{
    BorderStyle, Bounds, ContentMask, Corners, Edges, Hsla, Pixels, Quad, ScaledPixels, Scene,
    Size, point, rgba, size, transparent_black,
};
use std::{collections::VecDeque, time::Duration};

#[allow(missing_docs)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum DebugFrameOverlayMode {
    #[default]
    Hidden,
    Minimal,
    Full,
}

impl DebugFrameOverlayMode {
    /// Returns the next mode in the Hidden → FrameTime → Detailed cycle.
    pub fn next(self) -> Self {
        match self {
            Self::Hidden => Self::Minimal,
            Self::Minimal => Self::Full,
            Self::Full => Self::Hidden,
        }
    }
}

/// A snapshot of the frame-time statistics shown by the debug frame overlay,
/// derived from its history of the most recent draw durations.
///
/// Obtain it with [`crate::Window::debug_frame_overlay_stats`].
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct DebugFrameOverlayStats {
    /// The most recently recorded draw duration, in milliseconds.
    pub current_ms: Option<f32>,
    /// The 90th-percentile draw duration over the retained samples, in milliseconds.
    pub p90_ms: Option<f32>,
    /// The 99th-percentile draw duration over the retained samples, in milliseconds.
    pub p99_ms: Option<f32>,
    /// The largest retained draw duration, in milliseconds.
    pub max_ms: Option<f32>,
    /// The total number of frames recorded, including those whose samples were evicted
    /// or cleared by a reset. This is never capped.
    pub frames: u64,
    /// The number of draw durations currently retained.
    pub samples: usize,
}

/// The number of most recent draw durations retained for percentile statistics.
const MAX_SAMPLES: usize = 1000;

const GLYPH_WIDTH: usize = 5;
const GLYPH_HEIGHT: usize = 7;
/// Glyph advance and line advance, in font cells.
const CHAR_ADVANCE: f32 = (GLYPH_WIDTH + 1) as f32;
const LINE_ADVANCE: f32 = (GLYPH_HEIGHT + 2) as f32;
/// Padding between the panel edge and the text, in font cells.
const PANEL_PADDING: f32 = 2.0;
/// Margin between the panel and the window corner, in font cells.
const PANEL_MARGIN: f32 = 4.0;
/// Side of one square font cell, in logical pixels.
const CELL_SIZE: f32 = 2.0;

fn text_color() -> Hsla {
    rgba(0x33ff33ff).into()
}

fn panel_color() -> Hsla {
    rgba(0x000000aa).into()
}

pub(crate) struct DebugFrameOverlay {
    mode: DebugFrameOverlayMode,
    draw_durations: VecDeque<Duration>,
    total_frame_count: u64,
}

impl DebugFrameOverlay {
    pub(crate) fn new() -> Self {
        Self {
            mode: DebugFrameOverlayMode::default(),
            draw_durations: VecDeque::new(),
            total_frame_count: 0,
        }
    }

    pub(crate) fn mode(&self) -> DebugFrameOverlayMode {
        self.mode
    }

    pub(crate) fn set_mode(&mut self, mode: DebugFrameOverlayMode) {
        self.mode = mode;
    }

    /// Clears the draw-duration samples, restarting the percentile window.
    /// The total frame count is left untouched.
    pub(crate) fn reset_stats(&mut self) {
        self.draw_durations.clear();
    }

    pub(crate) fn is_enabled(&self) -> bool {
        self.mode != DebugFrameOverlayMode::Hidden
    }

    pub(crate) fn record_frame(&mut self, draw_duration: Duration) {
        self.total_frame_count += 1;
        if self.draw_durations.len() >= MAX_SAMPLES {
            self.draw_durations.pop_front();
        }
        self.draw_durations.push_back(draw_duration);
    }

    pub(crate) fn paint(&self, scene: &mut Scene, viewport_size: Size<Pixels>, scale_factor: f32) {
        if !self.is_enabled() {
            return;
        }

        let lines = self.lines();
        let max_line_chars = lines.iter().map(|line| line.len()).max().unwrap_or(0);
        // Ensure at least one physical pixel per cell so the text stays legible
        // at fractional downscale factors.
        let cell = (CELL_SIZE * scale_factor).max(1.0);

        let panel_width = cell * (max_line_chars as f32 * CHAR_ADVANCE + 2.0 * PANEL_PADDING);
        let panel_height = cell * (lines.len() as f32 * LINE_ADVANCE + 2.0 * PANEL_PADDING);
        let viewport = viewport_size.scale(scale_factor);
        let panel_left = viewport.width.0 - panel_width - cell * PANEL_MARGIN;
        let panel_top = cell * PANEL_MARGIN;

        let content_mask = ContentMask {
            bounds: Bounds {
                origin: point(ScaledPixels(0.), ScaledPixels(0.)),
                size: viewport,
            },
        };

        scene.insert_primitive(solid_quad(
            scaled_bounds(panel_left, panel_top, panel_width, panel_height),
            &content_mask,
            panel_color(),
        ));

        let text_color = text_color();
        for (line_index, line) in lines.iter().enumerate() {
            let line_top = panel_top + cell * (PANEL_PADDING + line_index as f32 * LINE_ADVANCE);
            for (char_index, character) in line.chars().enumerate() {
                let Some(rows) = glyph(character) else {
                    continue;
                };
                let glyph_left =
                    panel_left + cell * (PANEL_PADDING + char_index as f32 * CHAR_ADVANCE);
                for (row_index, row) in rows.iter().enumerate() {
                    let row_top = line_top + cell * row_index as f32;
                    // Merge horizontal runs of lit cells into single quads.
                    let mut column = 0;
                    while column < GLYPH_WIDTH {
                        if row & (1 << (GLYPH_WIDTH - 1 - column)) == 0 {
                            column += 1;
                            continue;
                        }
                        let run_start = column;
                        while column < GLYPH_WIDTH && row & (1 << (GLYPH_WIDTH - 1 - column)) != 0 {
                            column += 1;
                        }
                        scene.insert_primitive(solid_quad(
                            scaled_bounds(
                                glyph_left + cell * run_start as f32,
                                row_top,
                                cell * (column - run_start) as f32,
                                cell,
                            ),
                            &content_mask,
                            text_color,
                        ));
                    }
                }
            }
        }
    }

    /// The snapshot every statistic of the overlay is derived from: the
    /// overlay text and [`crate::Window::debug_frame_overlay_stats`] both read
    /// it, so there is a single source of truth for the history.
    pub(crate) fn stats(&self) -> DebugFrameOverlayStats {
        let mut sorted: Vec<Duration> = self.draw_durations.iter().copied().collect();
        sorted.sort_unstable();
        let percentile = |percent: usize| {
            (!sorted.is_empty()).then(|| duration_ms(sorted[(sorted.len() - 1) * percent / 100]))
        };
        DebugFrameOverlayStats {
            current_ms: self.draw_durations.back().copied().map(duration_ms),
            p90_ms: percentile(90),
            p99_ms: percentile(99),
            max_ms: sorted.last().copied().map(duration_ms),
            frames: self.total_frame_count,
            samples: sorted.len(),
        }
    }

    fn lines(&self) -> Vec<String> {
        match self.mode {
            DebugFrameOverlayMode::Hidden => Vec::new(),
            DebugFrameOverlayMode::Minimal => {
                vec![format_ms(
                    self.draw_durations.back().copied().map(duration_ms),
                )]
            }
            DebugFrameOverlayMode::Full => {
                let stats = self.stats();
                // Past five digits the count would break the column
                // alignment, so it saturates instead.
                let frame_count = if stats.frames > 99_999 {
                    "LOTS".to_string()
                } else {
                    stats.frames.to_string()
                };
                // Labels are padded to a uniform width so the fixed-width
                // durations start in the same column on every line.
                vec![
                    format!("CUR {}", format_ms(stats.current_ms)),
                    format!("1%  {}", format_ms(stats.p99_ms)),
                    format!("10% {}", format_ms(stats.p90_ms)),
                    format!("MAX {}", format_ms(stats.max_ms)),
                    format!("FRAMES {frame_count:>5}"),
                ]
            }
        }
    }
}

/// Formats as `abc.d MS`, right-aligned in room for three integer digits and
/// one decimal (padded with spaces, not zeroes), so stacked readouts align.
fn format_ms(milliseconds: Option<f32>) -> String {
    match milliseconds {
        Some(ms) => format!("{ms:>5.1} MS"),
        None => "   -- MS".into(),
    }
}

/// Seconds multiplied by 1000 into single-precision milliseconds.
fn duration_ms(duration: Duration) -> f32 {
    duration.as_secs_f32() * 1000.0
}

fn scaled_bounds(left: f32, top: f32, width: f32, height: f32) -> Bounds<ScaledPixels> {
    Bounds {
        origin: point(ScaledPixels(left), ScaledPixels(top)),
        size: size(ScaledPixels(width), ScaledPixels(height)),
    }
}

fn solid_quad(
    bounds: Bounds<ScaledPixels>,
    content_mask: &ContentMask<ScaledPixels>,
    color: Hsla,
) -> Quad {
    Quad {
        order: 0,
        border_style: BorderStyle::Solid,
        bounds,
        content_mask: *content_mask,
        background: color.into(),
        border_color: transparent_black(),
        corner_radii: Corners::default(),
        border_widths: Edges::default(),
        scope: Default::default(),
    }
}

/// Returns the 5x7 bitmap for the given character, one `u8` of column bits per
/// row with the most significant of the 5 bits leftmost. Only the characters
/// used by the overlay's readouts are defined.
fn glyph(character: char) -> Option<[u8; GLYPH_HEIGHT]> {
    Some(match character {
        '0' => [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
        '1' => [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        '2' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        '3' => [
            0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110,
        ],
        '4' => [
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ],
        '5' => [
            0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110,
        ],
        '6' => [
            0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
        ],
        '7' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ],
        '8' => [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
        '9' => [
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100,
        ],
        '.' => [
            0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b01100, 0b01100,
        ],
        '-' => [
            0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000,
        ],
        '%' => [
            0b11001, 0b11001, 0b00010, 0b00100, 0b01000, 0b10011, 0b10011,
        ],
        'A' => [
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'C' => [
            0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110,
        ],
        'E' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
        'F' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'L' => [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
        'M' => [
            0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001,
        ],
        'N' => [
            0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001,
        ],
        'O' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'R' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ],
        'S' => [
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        'T' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'U' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'X' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001,
        ],
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every character the readouts can produce must have a glyph, or it
    /// would silently render as blank space.
    #[test]
    fn all_rendered_characters_have_glyphs() {
        let mut overlay = DebugFrameOverlay::new();
        overlay.set_mode(DebugFrameOverlayMode::Full);

        let mut lines = Vec::new();
        for duration in [
            Duration::ZERO,
            Duration::from_micros(1),
            Duration::from_micros(8_333),
            Duration::from_millis(123),
            Duration::from_secs(2),
        ] {
            overlay.record_frame(duration);
            lines.extend(overlay.lines());
        }
        // Counts beyond five digits render as "LOTS".
        overlay.total_frame_count = 100_000;
        lines.extend(overlay.lines());

        // An enabled overlay with no samples yet renders placeholders.
        let mut empty = DebugFrameOverlay::new();
        empty.set_mode(DebugFrameOverlayMode::Full);
        lines.extend(empty.lines());

        for line in lines {
            for character in line.chars() {
                assert!(
                    character == ' ' || glyph(character).is_some(),
                    "no glyph for {character:?} in line {line:?}"
                );
            }
        }
    }

    #[test]
    fn percentile_lows_are_reported_as_times() {
        let mut overlay = DebugFrameOverlay::new();
        overlay.set_mode(DebugFrameOverlayMode::Full);
        for milliseconds in 1..=100 {
            overlay.record_frame(Duration::from_millis(milliseconds));
        }
        let lines = overlay.lines();
        assert_eq!(lines[0], "CUR 100.0 MS");
        assert_eq!(lines[1], "1%   99.0 MS");
        assert_eq!(lines[2], "10%  90.0 MS");
        assert_eq!(lines[3], "MAX 100.0 MS");
        assert_eq!(lines[4], "FRAMES   100");
    }

    #[test]
    fn reset_clears_durations_but_keeps_frame_count() {
        let mut overlay = DebugFrameOverlay::new();
        overlay.set_mode(DebugFrameOverlayMode::Full);
        for _ in 0..10 {
            overlay.record_frame(Duration::from_millis(10));
        }
        overlay.reset_stats();
        let lines = overlay.lines();
        assert_eq!(lines[0], "CUR    -- MS");
        assert_eq!(lines[3], "MAX    -- MS");
        assert_eq!(lines[4], "FRAMES    10");
        overlay.record_frame(Duration::from_millis(20));
        let lines = overlay.lines();
        assert_eq!(lines[0], "CUR  20.0 MS");
        assert_eq!(lines[4], "FRAMES    11");
    }

    #[test]
    fn frame_count_accumulates_across_mode_changes() {
        let mut overlay = DebugFrameOverlay::new();
        for _ in 0..3 {
            overlay.record_frame(Duration::from_millis(10));
        }
        overlay.set_mode(DebugFrameOverlayMode::Minimal);
        assert_eq!(overlay.lines(), vec![" 10.0 MS".to_string()]);
        overlay.set_mode(DebugFrameOverlayMode::Full);
        overlay.record_frame(Duration::from_millis(10));
        assert_eq!(overlay.lines()[4], "FRAMES     4");
        overlay.set_mode(DebugFrameOverlayMode::Hidden);
        overlay.record_frame(Duration::from_millis(10));
        overlay.set_mode(DebugFrameOverlayMode::Full);
        assert_eq!(overlay.lines()[4], "FRAMES     5");
    }

    #[test]
    fn frame_count_is_right_aligned_and_saturates() {
        let mut overlay = DebugFrameOverlay::new();
        overlay.set_mode(DebugFrameOverlayMode::Full);
        overlay.record_frame(Duration::from_millis(10));
        assert_eq!(overlay.lines()[4], "FRAMES     1");
        overlay.total_frame_count = 99_999;
        assert_eq!(overlay.lines()[4], "FRAMES 99999");
        overlay.total_frame_count = 100_000;
        assert_eq!(overlay.lines()[4], "FRAMES  LOTS");
    }

    #[test]
    fn toggling_on_shows_previous_frame_immediately() {
        let mut overlay = DebugFrameOverlay::new();
        overlay.record_frame(Duration::from_millis(10));
        overlay.set_mode(DebugFrameOverlayMode::Minimal);
        assert_eq!(overlay.lines(), vec![" 10.0 MS".to_string()]);
    }

    fn record_millis(overlay: &mut DebugFrameOverlay, milliseconds: impl IntoIterator<Item = u64>) {
        for ms in milliseconds {
            overlay.record_frame(Duration::from_millis(ms));
        }
    }

    fn assert_close(actual: Option<f32>, expected: f32) {
        let actual = actual.expect("value is present");
        assert!(
            (actual - expected).abs() < 1e-3,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn stats_select_floor_indexed_percentiles() {
        let mut overlay = DebugFrameOverlay::new();
        record_millis(&mut overlay, 1..=100);
        let stats = overlay.stats();
        assert_close(stats.current_ms, 100.0);
        assert_close(stats.p90_ms, 90.0);
        assert_close(stats.p99_ms, 99.0);
        assert_close(stats.max_ms, 100.0);
        assert_eq!(stats.frames, 100);
        assert_eq!(stats.samples, 100);
    }

    #[test]
    fn stats_current_is_latest_sample_not_maximum() {
        let mut overlay = DebugFrameOverlay::new();
        record_millis(&mut overlay, [50, 10]);
        let stats = overlay.stats();
        assert_close(stats.current_ms, 10.0);
        assert_close(stats.max_ms, 50.0);
    }

    #[test]
    fn history_keeps_latest_thousand_samples_and_counts_every_frame() {
        let mut overlay = DebugFrameOverlay::new();
        record_millis(&mut overlay, 1..=1001);
        let stats = overlay.stats();
        assert_eq!(stats.samples, 1000);
        assert_eq!(stats.frames, 1001);
        // The oldest sample (1 ms) was evicted, so the smallest retained is 2 ms
        // and the newest (1001 ms) is still present.
        assert_close(stats.current_ms, 1001.0);
        assert_close(stats.max_ms, 1001.0);
        let retained: Vec<Duration> = overlay.draw_durations.iter().copied().collect();
        assert_eq!(retained.first(), Some(&Duration::from_millis(2)));
        assert_eq!(retained.last(), Some(&Duration::from_millis(1001)));
        // Index floor(999 * 90 / 100) = 899 of the sorted 2..=1001 range.
        assert_close(stats.p90_ms, 901.0);
        assert_close(stats.p99_ms, 991.0);
    }

    #[test]
    fn empty_stats_have_no_timings() {
        let stats = DebugFrameOverlay::new().stats();
        assert_eq!(stats.current_ms, None);
        assert_eq!(stats.p90_ms, None);
        assert_eq!(stats.p99_ms, None);
        assert_eq!(stats.max_ms, None);
        assert_eq!(stats.frames, 0);
        assert_eq!(stats.samples, 0);
    }

    #[test]
    fn single_sample_stats_all_equal_the_sample() {
        let mut overlay = DebugFrameOverlay::new();
        record_millis(&mut overlay, [7]);
        let stats = overlay.stats();
        for value in [stats.current_ms, stats.p90_ms, stats.p99_ms, stats.max_ms] {
            assert_close(value, 7.0);
        }
        assert_eq!(stats.frames, 1);
        assert_eq!(stats.samples, 1);
    }

    /// The overlay text is formatted from the same snapshot the programmatic
    /// API returns, and polling the snapshot changes nothing.
    #[test]
    fn overlay_text_matches_snapshot_and_polling_is_pure() {
        let mut overlay = DebugFrameOverlay::new();
        overlay.set_mode(DebugFrameOverlayMode::Full);
        record_millis(&mut overlay, (1..=100).map(|ms| ms * 3 / 2 + 1));

        let before = overlay.stats();
        let lines = overlay.lines();
        let expected = |value: Option<f32>| format!("{:>5.1} MS", value.unwrap());
        assert_eq!(lines[0], format!("CUR {}", expected(before.current_ms)));
        assert_eq!(lines[1], format!("1%  {}", expected(before.p99_ms)));
        assert_eq!(lines[2], format!("10% {}", expected(before.p90_ms)));
        assert_eq!(lines[3], format!("MAX {}", expected(before.max_ms)));
        assert_eq!(lines[4], format!("FRAMES {:>5}", before.frames));

        for _ in 0..5 {
            assert_eq!(overlay.stats(), before);
        }
        assert_eq!(overlay.mode(), DebugFrameOverlayMode::Full);
        assert_eq!(overlay.lines(), lines);

        overlay.total_frame_count = 100_000;
        assert_eq!(overlay.lines()[4], "FRAMES  LOTS");
        assert_eq!(overlay.stats().frames, 100_000);
        overlay.total_frame_count = 123_456_789;
        assert_eq!(overlay.stats().frames, 123_456_789);
    }

    #[test]
    fn reset_clears_samples_in_stats_and_keeps_frames() {
        let mut overlay = DebugFrameOverlay::new();
        record_millis(&mut overlay, [5, 6, 7]);
        overlay.reset_stats();
        let stats = overlay.stats();
        assert_eq!(stats.samples, 0);
        assert_eq!(stats.frames, 3);
        assert_eq!(stats.current_ms, None);
        assert_eq!(stats.max_ms, None);
        record_millis(&mut overlay, [9]);
        let stats = overlay.stats();
        assert_eq!(stats.samples, 1);
        assert_eq!(stats.frames, 4);
    }
}
