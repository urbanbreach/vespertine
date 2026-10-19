//! PERF-ANM-004: one hundred cosmetic cells driven by the shared parked animation clock.
//!
//! Run in the runner's idle mode: the scene drives itself from the shared clock. After the warmup
//! (about two seconds of 60 Hz frames) the cells run 10 s at stride 1 (30 Hz), 10 s at stride 2
//! (15 Hz) and 10 s hidden (not painted), measured from the first rendered frame.

use std::time::{Duration, Instant};

use crate::common::hex;
use crate::scene::{Scene, register_scene};
use vespertine::{
    AnimationCadence, AppContext as _, AnyElement, App, Context, Entity, IntoElement, ParentElement, Render,
    StyleRefinement, Styled, Window, animation_wave, div, px,
};

const COLUMNS: usize = 10;
const CELLS: usize = 100;
const PERIOD: Duration = Duration::from_millis(1200);
/// Idle mode renders 120 warmup frames at 60 Hz before the measured interval starts.
const WARMUP: Duration = Duration::from_secs(2);
const STAGE: Duration = Duration::from_secs(10);

struct Cells {
    started: Instant,
}

impl Render for Cells {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let elapsed = self.started.elapsed();
        let cadence = if elapsed < WARMUP + 2 * STAGE {
            if elapsed < WARMUP + STAGE {
                Some(AnimationCadence::Normal)
            } else {
                Some(AnimationCadence::Slow)
            }
        } else {
            None
        };
        let Some(cadence) = cadence else {
            // Hidden: nothing is painted and no lease is renewed, so the clock parks.
            return div().size_full().into_any_element();
        };
        let phase = window.animation_phase(cx, PERIOD, cadence);
        div()
            .size_full()
            .children((0..CELLS).map(|i| {
                let cell_phase = (phase + i as f32 / CELLS as f32).fract();
                let opacity = 0.08 + 0.92 * animation_wave(cell_phase);
                let (row, column) = (i / COLUMNS, i % COLUMNS);
                div()
                    .absolute()
                    .left(px(20.0 + 12.0 * column as f32))
                    .top(px(20.0 + 12.0 * row as f32))
                    .size(px(8.0))
                    .rounded(px(2.0))
                    .bg(hex(0x3366cc))
                    .opacity(opacity)
            }))
            .into_any_element()
    }
}

struct ClockCells {
    cells: Option<Entity<Cells>>,
}

impl Scene for ClockCells {
    fn size(&self) -> (f32, f32) {
        (800.0, 480.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, cx: &mut App) -> AnyElement {
        let cells = self
            .cells
            .get_or_insert_with(|| {
                cx.new(|_| Cells {
                    started: Instant::now(),
                })
            })
            .clone();
        div()
            .size_full()
            .bg(hex(0xffffff))
            .child(cells.cached(StyleRefinement::default().size_full()))
            .into_any_element()
    }
}

register_scene!("PERF-ANM-004", || ClockCells { cells: None });
