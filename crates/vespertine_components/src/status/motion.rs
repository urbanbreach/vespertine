//! Pure motion and geometry functions of the status kit (CMP-023 R3-R6, R13-R14).

use std::f32::consts::{FRAC_PI_2, TAU};
use std::time::Duration;

/// Period of the skeleton, wave row, glow and the subtle activity pulse.
pub const SLOW_PERIOD: Duration = Duration::from_millis(2400);
/// Period of the matrix spinner and the mini glyph.
pub const FAST_PERIOD: Duration = Duration::from_millis(750);

/// The cosine wave: `0.5 - 0.5 cos(2 pi p)` (0 at phase 0, 1 at 0.5, 0 at 1).
pub fn wave(phase: f32) -> f32 {
    0.5 - 0.5 * (TAU * phase).cos()
}

/// `(clock - index x stagger) mod 1`, in `[0, 1)`.
pub fn staggered_phase(clock_phase: f32, index: usize, stagger: f32) -> f32 {
    let phase = (clock_phase - index as f32 * stagger).rem_euclid(1.0);
    if phase >= 1.0 { 0.0 } else { phase }
}

/// Phase stagger between consecutive skeleton bars.
pub const SKELETON_STAGGER: f32 = 0.08;
/// Lowest skeleton opacity.
pub const SKELETON_BASE: f32 = 0.45;
/// Skeleton opacity swing.
pub const SKELETON_SWING: f32 = 0.4;
/// The skeleton tint: foreground ink at this opacity.
pub const SKELETON_TINT: f32 = 0.07;
/// Default width ladder of skeleton rows (fractions of the container).
pub const WIDTH_LADDER: [f32; 6] = [0.62, 0.48, 0.71, 0.55, 0.39, 0.66];

/// The skeleton bar opacity at `index` of its column for the shared clock phase.
pub fn skeleton_opacity(clock_phase: f32, index: usize) -> f32 {
    SKELETON_BASE + SKELETON_SWING * wave(staggered_phase(clock_phase, index, SKELETON_STAGGER))
}

/// The width fraction of skeleton row `row` (the ladder repeats).
pub fn ladder_width(row: usize) -> f32 {
    WIDTH_LADDER[row % WIDTH_LADDER.len()]
}

/// Wave row stagger: 0.15 s of the 2.4 s period.
pub const WAVE_STAGGER: f32 = 0.15 / 2.4;
/// Resting opacity of a wave-row cell.
pub const WAVE_REST_OPACITY: f32 = 0.08;
/// Resting inner size (fraction of the cell) of a wave-row cell.
pub const WAVE_REST_SIZE: f32 = 0.9;

/// Opacity and inner size fraction of wave-row cell `index` at the clock phase:
/// `0.08 + 0.92 wave(p_i)` and `0.9 + 0.1 wave(p_i)`.
pub fn wave_cell(clock_phase: f32, index: usize) -> (f32, f32) {
    let w = wave(staggered_phase(clock_phase, index, WAVE_STAGGER));
    (0.08 + 0.92 * w, 0.9 + 0.1 * w)
}

/// The dim opacity of the matrix and mini-glyph keyframe.
pub const DIM: f32 = 0.1;

/// The matrix keyframe over `t` in `[0, 1]`: 1 at 0 falling linearly to 0.1 at 0.45, flat until
/// 0.92, then rising linearly to 1 at 1.
pub fn matrix_keyframe(t: f32) -> f32 {
    let t = t.rem_euclid(1.0).min(1.0);
    if t < 0.45 {
        1.0 + (DIM - 1.0) * (t / 0.45)
    } else if t <= 0.92 {
        DIM
    } else {
        DIM + (1.0 - DIM) * ((t - 0.92) / 0.08)
    }
}

/// Cell phase of matrix cell (`row`, `col`): `d / 4` with `d = (2 - row) + |col - 1|`.
pub fn matrix_cell_phase(row: usize, col: usize) -> f32 {
    let d = (2 - row.min(2)) + col.abs_diff(1);
    d as f32 / 4.0
}

/// Opacity of matrix cell (`row`, `col`) at the 750 ms clock phase.
pub fn matrix_cell_opacity(clock_phase: f32, row: usize, col: usize) -> f32 {
    matrix_keyframe((clock_phase + matrix_cell_phase(row, col)).rem_euclid(1.0))
}

/// Ring position of mini-glyph cell (`row`, `col`), clockwise from the top-left:
/// (0,0)=0, (0,1)=1, (1,1)=2, (2,1)=3, (2,0)=4, (1,0)=5.
pub const RING_POSITIONS: [[usize; 2]; 3] = [[0, 1], [5, 2], [4, 3]];

/// The ring position of a mini-glyph cell.
pub fn ring_position(row: usize, col: usize) -> usize {
    RING_POSITIONS[row.min(2)][col.min(1)]
}

/// Opacity of mini-glyph cell (`row`, `col`) at the 750 ms clock phase (phase `ring / 6`).
pub fn mini_cell_opacity(clock_phase: f32, row: usize, col: usize) -> f32 {
    let cell_phase = ring_position(row, col) as f32 / 6.0;
    matrix_keyframe((clock_phase + cell_phase).rem_euclid(1.0))
}

/// Opacity of the pulse glow: `floor + (peak - floor) x wave(clock)`.
pub fn glow_opacity(clock_phase: f32, floor: f32, peak: f32) -> f32 {
    floor + (peak - floor) * wave(clock_phase)
}

/// The uniform opacity of the activity-indicator subtle pulse: `0.6 + 0.2 x wave(phase)`.
pub fn subtle_pulse(clock_phase: f32) -> f32 {
    0.6 + 0.2 * wave(clock_phase)
}

/// The determinate fraction `clamp(value / max, 0, 1)`; non-finite results (and a non-positive
/// `max`) count as 0.
pub fn fraction(value: f64, max: f64) -> f32 {
    if !(value.is_finite() && max.is_finite()) || max <= 0.0 {
        return 0.0;
    }
    (value / max).clamp(0.0, 1.0) as f32
}

/// The fraction of an integer percentage clamped to 100.
pub fn percent_fraction(percent: u32) -> f32 {
    percent.min(100) as f32 / 100.0
}

/// Number of line segments of an arc of fraction `f`: `max(2, ceil(64 f))`; 64 for a full
/// circle.
pub fn arc_segments(fraction: f32) -> usize {
    ((64.0 * fraction.clamp(0.0, 1.0)).ceil() as usize).max(2)
}

/// Vertices of the progress arc of `fraction` about `(cx, cy)`: it starts at twelve o'clock and
/// runs clockwise, sweeping `f x 360` degrees. Empty when the sweep is not positive.
pub fn arc_points(cx: f32, cy: f32, radius: f32, fraction: f32) -> Vec<(f32, f32)> {
    let fraction = if fraction.is_finite() {
        fraction.clamp(0.0, 1.0)
    } else {
        0.0
    };
    if fraction <= 0.0 {
        return Vec::new();
    }
    let segments = arc_segments(fraction);
    let sweep = TAU * fraction;
    (0..=segments)
        .map(|i| {
            let angle = -FRAC_PI_2 + sweep * i as f32 / segments as f32;
            (cx + radius * angle.cos(), cy + radius * angle.sin())
        })
        .collect()
}

/// Vertices of the full-circle track (64 segments, closed).
pub fn circle_points(cx: f32, cy: f32, radius: f32) -> Vec<(f32, f32)> {
    arc_points(cx, cy, radius, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn a3_wave_and_staggers() {
        assert!(close(wave(0.0), 0.0));
        assert!(close(wave(0.5), 1.0));
        assert!(close(wave(1.0), 0.0));
        assert!(close(wave_cell(0.0, 0).0, 0.08));
        assert!(close(wave_cell(0.5, 0).0, 1.0));
        assert!(close(wave_cell(0.0, 0).1, 0.9));
        assert!(close(wave_cell(0.5, 0).1, 1.0));
        assert!(close(staggered_phase(0.0, 1, WAVE_STAGGER), 1.0 - 0.0625));
        assert!(close(skeleton_opacity(0.0, 0), 0.45));
        assert!(close(skeleton_opacity(0.5, 0), 0.85));
        // Static frame: later bars follow wave((-0.08 index) mod 1).
        let expected = 0.45 + 0.4 * wave((-0.08f32 * 3.0).rem_euclid(1.0));
        assert!(close(skeleton_opacity(0.0, 3), expected));
        assert_eq!(ladder_width(0), 0.62);
        assert_eq!(ladder_width(6), 0.62);
        assert_eq!(ladder_width(4), 0.39);
    }

    #[test]
    fn a3_matrix_keyframe_and_cell_phases() {
        assert!(close(matrix_keyframe(0.0), 1.0));
        assert!(close(matrix_keyframe(0.45), 0.1));
        assert!(close(matrix_keyframe(0.7), 0.1));
        assert!(close(matrix_keyframe(1.0), 1.0));
        assert!(close(matrix_keyframe(0.225), 0.55));
        assert!(close(matrix_keyframe(0.96), 0.55));
        // Middle column: 0.5, 0.25, 0 for rows 0, 1, 2; bottom row corners 0.25.
        assert_eq!(matrix_cell_phase(0, 1), 0.5);
        assert_eq!(matrix_cell_phase(1, 1), 0.25);
        assert_eq!(matrix_cell_phase(2, 1), 0.0);
        assert_eq!(matrix_cell_phase(2, 0), 0.25);
        assert_eq!(matrix_cell_phase(2, 2), 0.25);
        assert_eq!(matrix_cell_phase(0, 0), 0.75);
    }

    #[test]
    fn mini_glyph_ring_positions_run_clockwise_from_the_top_left() {
        assert_eq!(ring_position(0, 0), 0);
        assert_eq!(ring_position(0, 1), 1);
        assert_eq!(ring_position(1, 1), 2);
        assert_eq!(ring_position(2, 1), 3);
        assert_eq!(ring_position(2, 0), 4);
        assert_eq!(ring_position(1, 0), 5);
        assert!(close(mini_cell_opacity(0.0, 0, 0), 1.0));
        assert!(close(
            mini_cell_opacity(0.0, 0, 1),
            matrix_keyframe(1.0 / 6.0)
        ));
    }

    #[test]
    fn a4_fraction_and_arc() {
        assert_eq!(fraction(-1.0, 1.0), 0.0);
        assert_eq!(fraction(0.5, 1.0), 0.5);
        assert_eq!(fraction(1.7, 1.0), 1.0);
        assert_eq!(fraction(f64::NAN, 1.0), 0.0);
        assert_eq!(fraction(5.0, 0.0), 0.0);
        assert_eq!(arc_segments(percent_fraction(50)), 32);
        assert_eq!(arc_segments(percent_fraction(1)), 2);
        assert_eq!(arc_segments(percent_fraction(100)), 64);
        assert_eq!(percent_fraction(250), 1.0);
        assert!(arc_points(0.0, 0.0, 5.0, 0.0).is_empty());
        assert_eq!(arc_points(0.0, 0.0, 5.0, 0.5).len(), 33);
        let arc = arc_points(10.0, 20.0, 6.0, 0.25);
        // The first vertex is directly above the center, and the arc runs clockwise: the
        // quarter arc ends at three o'clock.
        assert!(close(arc[0].0, 10.0) && close(arc[0].1, 14.0));
        let last = arc.last().unwrap();
        assert!(close(last.0, 16.0) && close(last.1, 20.0));
        assert!(arc[1].0 > 10.0);
    }

    #[test]
    fn glow_and_subtle_pulse() {
        assert!(close(glow_opacity(0.0, 0.2, 0.9), 0.2));
        assert!(close(glow_opacity(0.5, 0.2, 0.9), 0.9));
        assert!(close(subtle_pulse(0.0), 0.6));
        assert!(close(subtle_pulse(0.5), 0.8));
    }
}
