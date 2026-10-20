//! The waveform painter (CMP-027 R6-R11): scrolling rounded bars and a baseline in one canvas.

use std::sync::LazyLock;
use std::time::Instant;

use vespertine::{
    Bounds, Corners, Hsla, IntoElement, Pixels, Rgba, Styled, Window, canvas, fill, point, px, size,
};

use super::envelope::Bar;

/// Bar width.
pub const BAR_WIDTH: f32 = 3.0;
/// Gap between bars.
pub const BAR_GAP: f32 = 3.0;
/// Column pitch.
pub const PITCH: f32 = BAR_WIDTH + BAR_GAP;
/// Corner radius of a bar.
pub const BAR_RADIUS: f32 = 1.5;
/// Width over which bars dissolve at the leading (left) edge.
pub const EDGE_FADE: f32 = 28.0;
/// Lowest amplitude (the baseline dot).
pub const MIN_AMPLITUDE: f32 = 0.08;
/// Shortest bar.
pub const MIN_HEIGHT: f32 = 3.0;
/// Breath period of the waiting baseline, in seconds.
pub const BREATH_PERIOD: f32 = 1.4;
/// Sweep period of the processing highlight, in seconds.
pub const SWEEP_PERIOD: f32 = 1.6;
/// Half-width of the processing highlight band (fraction of the width).
pub const SWEEP_BAND: f32 = 0.18;
/// Baseline swell amplitude.
pub const SWELL_AMPLITUDE: f32 = 0.1;
/// Phase spacing of the breath between columns, in cycles.
pub const BREATH_SPACING: f32 = 1.0 / 18.0;

/// What the waveform shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Input is opening: a quiet baseline breathes.
    Waiting,
    /// Bars follow the input.
    Live,
    /// Capture ended: bars hold still while a highlight sweeps across them.
    Processing,
}

/// Colors of the waveform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaveformColors {
    /// Bars in the live state.
    pub ink: Hsla,
    /// The baseline and quiet bars.
    pub quiet: Hsla,
}

/// Entrance and exit phases, each in `[0, 1]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Phase {
    /// Entrance: the baseline draws in from the trailing edge.
    pub intro: f32,
    /// Exit: bars settle back into the baseline.
    pub collapse: f32,
}

impl Phase {
    /// Fully entered, not collapsing.
    pub const STEADY: Phase = Phase {
        intro: 1.0,
        collapse: 0.0,
    };
}

/// One painted bar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BarQuad {
    /// Column index from the right.
    pub column: usize,
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub width: f32,
    /// Height.
    pub height: f32,
    /// Fill, with the column alpha applied.
    pub color: Hsla,
    /// The column alpha.
    pub alpha: f32,
    /// Normalized amplitude that produced the height.
    pub amplitude: f32,
    /// Whether the column is a baseline dot rather than a bar.
    pub baseline: bool,
}

/// `easeOut(t) = 1 - (1 - clamp(t, 0, 1))^3`.
pub fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

fn mix(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let (a, b): (Rgba, Rgba) = (a.into(), b.into());
    let blend = |from: f32, to: f32| from + (to - from) * t;
    let alpha = blend(a.a, b.a);
    let blue = blend(a.b, b.b);
    Rgba {
        a: alpha,
        b: blue,
        g: blend(a.g, b.g),
        r: blend(a.r, b.r),
    }
    .into()
}

static PROCESS_CLOCK: LazyLock<Instant> = LazyLock::new(Instant::now);

/// Seconds on the process-wide monotonic clock, so repeated renders stay phase-continuous.
pub fn process_time() -> f32 {
    PROCESS_CLOCK.elapsed().as_secs_f32()
}

/// Lays out every visible column of the waveform in `bounds` (`left`, `top`, `width`,
/// `height`). `bars` are newest first. `time` is seconds (the breath and sweep clock); pass 0
/// when motion is disabled, which also stops bars from growing in (`motion = false`).
#[allow(clippy::too_many_arguments)]
pub fn layout(
    bounds: (f32, f32, f32, f32),
    bars: &[Bar],
    mode: Mode,
    colors: WaveformColors,
    phase: Phase,
    motion: bool,
    time: f32,
) -> Vec<BarQuad> {
    let (left, top, width, height) = bounds;
    let right = left + width - BAR_WIDTH;
    let span = (right - left).max(1.0);
    let mid = top + height / 2.0;
    let count = ((span + BAR_WIDTH) / PITCH).ceil() as usize + 1;
    let time = if motion { time } else { 0.0 };
    let offset = match (mode, bars.first()) {
        (Mode::Waiting, _) | (_, None) => 0.0,
        (_, Some(newest)) => newest.age.fract(),
    };
    let intro = phase.intro.clamp(0.0, 1.0);
    let settle = 1.0 - ease_out(phase.collapse);
    let mut quads = Vec::with_capacity(count);
    for k in 0..count {
        let x = right - (k as f32 + offset) * PITCH;
        let fade = ((x - left) / EDGE_FADE).clamp(0.0, 1.0);
        let distance = 1.0 - (x - left) / span;
        let reveal = ease_out((intro - 0.5 * distance) / 0.5);
        let alpha = fade * reveal;
        if alpha <= 0.0 {
            continue;
        }
        let (amplitude, color, baseline) = match (mode, bars.get(k)) {
            (Mode::Waiting, _) | (_, None) => {
                let swell = if mode == Mode::Waiting {
                    let phase = time / BREATH_PERIOD - k as f32 * BREATH_SPACING;
                    (0.5 + 0.5 * (std::f32::consts::TAU * phase).sin()).powi(3)
                } else {
                    0.0
                };
                (MIN_AMPLITUDE + SWELL_AMPLITUDE * swell, colors.quiet, true)
            }
            (_, Some(bar)) => {
                let grow = if motion { ease_out(bar.age) } else { 1.0 };
                let base = if mode == Mode::Processing {
                    let sweep = (time / SWEEP_PERIOD).fract();
                    let position = 1.0 - (x - left) / span;
                    let distance = (position - (sweep * 1.4 - 0.2)).abs();
                    let glow = (1.0 - distance / SWEEP_BAND).clamp(0.0, 1.0);
                    mix(colors.quiet, colors.ink, ease_out(glow))
                } else {
                    colors.ink
                };
                (
                    MIN_AMPLITUDE + (1.0 - MIN_AMPLITUDE) * bar.amplitude * grow * settle,
                    mix(colors.quiet, base, settle),
                    false,
                )
            }
        };
        let h =
            (amplitude.clamp(MIN_AMPLITUDE, 1.0) * height * (0.4 + 0.6 * reveal)).max(MIN_HEIGHT);
        let mut color = color;
        color.a *= alpha;
        quads.push(BarQuad {
            column: k,
            x,
            y: mid - h / 2.0,
            width: BAR_WIDTH,
            height: h,
            color,
            alpha,
            amplitude,
            baseline,
        });
    }
    quads
}

/// The waveform as an element filling the available width and height: one canvas paints every
/// bar and the baseline.
pub fn waveform(
    bars: Vec<Bar>,
    mode: Mode,
    colors: WaveformColors,
    phase: Phase,
    motion: bool,
) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds: Bounds<Pixels>, _, window: &mut Window, _| {
            let time = if motion { process_time() } else { 0.0 };
            let quads = layout(
                (
                    f32::from(bounds.origin.x),
                    f32::from(bounds.origin.y),
                    f32::from(bounds.size.width),
                    f32::from(bounds.size.height),
                ),
                &bars,
                mode,
                colors,
                phase,
                motion,
                time,
            );
            for quad in quads {
                window.paint_quad(
                    fill(
                        Bounds {
                            origin: point(px(quad.x), px(quad.y)),
                            size: size(px(quad.width), px(quad.height)),
                        },
                        quad.color,
                    )
                    .corner_radii(Corners::all(px(BAR_RADIUS))),
                );
            }
        },
    )
    .size_full()
}

#[cfg(test)]
mod tests {
    use super::*;
    use vespertine::hsla;

    fn colors() -> WaveformColors {
        WaveformColors {
            ink: hsla(0.0, 0.0, 1.0, 1.0),
            quiet: hsla(0.0, 0.0, 0.4, 1.0),
        }
    }

    const BOUNDS: (f32, f32, f32, f32) = (10.0, 20.0, 120.0, 40.0);

    fn bar(age: f32, amplitude: f32) -> Bar {
        Bar { age, amplitude }
    }

    #[test]
    fn a7_bar_positions_minimum_height_and_edge_fade() {
        let bars = [bar(1.0, 1.0), bar(2.0, 0.0), bar(3.0, 0.5)];
        let quads = layout(
            BOUNDS,
            &bars,
            Mode::Live,
            colors(),
            Phase::STEADY,
            true,
            0.0,
        );
        let right = 10.0 + 120.0 - 3.0;
        assert_eq!(quads[0].column, 0);
        assert_eq!(quads[0].x, right);
        assert_eq!(quads[1].x, right - 6.0);
        // Full-amplitude bar is the full height at full reveal; the quiet bar is >= 3 px.
        assert!((quads[0].height - 40.0).abs() < 1e-3);
        assert!(quads[1].height >= MIN_HEIGHT);
        assert!((quads[1].y + quads[1].height / 2.0 - 40.0).abs() < 1e-3);
        // Columns at x <= left are absent (alpha 0) and the edge fades over 28 px.
        assert!(quads.iter().all(|q| q.x > 10.0));
        let faded = quads.iter().find(|q| q.x < 10.0 + 28.0).unwrap();
        assert!((faded.alpha - (faded.x - 10.0) / 28.0).abs() < 1e-3);
        let count = ((117.0f32 + 3.0) / 6.0).ceil() as usize + 1;
        assert!(quads.len() < count);
    }

    #[test]
    fn a7_waiting_draws_only_baseline_columns_with_the_breath_range() {
        let bars = [bar(0.0, 1.0)];
        for step in 0..20 {
            let quads = layout(
                BOUNDS,
                &bars,
                Mode::Waiting,
                colors(),
                Phase::STEADY,
                true,
                step as f32 * 0.17,
            );
            assert!(!quads.is_empty());
            for quad in &quads {
                assert!(quad.baseline);
                assert!((0.08..=0.18 + 1e-6).contains(&quad.amplitude));
            }
        }
        // The breath moves with time.
        let a = layout(
            BOUNDS,
            &[],
            Mode::Waiting,
            colors(),
            Phase::STEADY,
            true,
            0.0,
        );
        let b = layout(
            BOUNDS,
            &[],
            Mode::Waiting,
            colors(),
            Phase::STEADY,
            true,
            0.35,
        );
        assert_ne!(a[0].amplitude, b[0].amplitude);
        // Motion off freezes breath and sweep.
        let c = layout(
            BOUNDS,
            &[],
            Mode::Waiting,
            colors(),
            Phase::STEADY,
            false,
            0.35,
        );
        let d = layout(
            BOUNDS,
            &[],
            Mode::Waiting,
            colors(),
            Phase::STEADY,
            false,
            9.0,
        );
        assert_eq!(c, d);
        let bars = [bar(0.5, 0.8), bar(1.5, 0.8), bar(2.5, 0.8)];
        let e = layout(
            BOUNDS,
            &bars,
            Mode::Processing,
            colors(),
            Phase::STEADY,
            false,
            0.3,
        );
        let f = layout(
            BOUNDS,
            &bars,
            Mode::Processing,
            colors(),
            Phase::STEADY,
            false,
            1.1,
        );
        assert_eq!(e, f);
    }

    #[test]
    fn scroll_offset_is_the_fraction_of_the_newest_age() {
        let bars = [bar(3.25, 0.5)];
        let quads = layout(
            BOUNDS,
            &bars,
            Mode::Live,
            colors(),
            Phase::STEADY,
            true,
            0.0,
        );
        assert!((quads[0].x - (127.0 - 0.25 * PITCH)).abs() < 1e-4);
        let waiting = layout(
            BOUNDS,
            &bars,
            Mode::Waiting,
            colors(),
            Phase::STEADY,
            true,
            0.0,
        );
        assert_eq!(waiting[0].x, 127.0);
    }

    #[test]
    fn processing_highlight_lights_the_columns_under_the_sweep() {
        let bars: Vec<Bar> = (0..20).map(|i| bar(i as f32, 0.5)).collect();
        let quads = layout(
            BOUNDS,
            &bars,
            Mode::Processing,
            colors(),
            Phase::STEADY,
            true,
            0.8,
        );
        // Sweep at 0.5 of the period: highlight centre at position 0.5 of the width.
        let brightness = |q: &BarQuad| Rgba::from(q.color).r;
        let lit = quads
            .iter()
            .max_by(|a, b| brightness(a).total_cmp(&brightness(b)))
            .unwrap();
        let position = 1.0 - (lit.x - 10.0) / 117.0;
        assert!((position - 0.5).abs() < 0.1, "{position}");
    }

    #[test]
    fn collapse_settles_bars_into_the_baseline_and_intro_reveals_from_the_trailing_edge() {
        let bars = [bar(2.0, 1.0)];
        let collapsed = layout(
            BOUNDS,
            &bars,
            Mode::Live,
            colors(),
            Phase {
                intro: 1.0,
                collapse: 1.0,
            },
            true,
            0.0,
        );
        assert!((collapsed[0].amplitude - MIN_AMPLITUDE).abs() < 1e-5);
        let entering = layout(
            BOUNDS,
            &[],
            Mode::Waiting,
            colors(),
            Phase {
                intro: 0.3,
                collapse: 0.0,
            },
            true,
            0.0,
        );
        let full = layout(
            BOUNDS,
            &[],
            Mode::Waiting,
            colors(),
            Phase::STEADY,
            true,
            0.0,
        );
        assert!(entering.len() < full.len());
        assert_eq!(entering[0].column, 0);
        assert!(entering[0].alpha > entering.last().unwrap().alpha);
    }
}
