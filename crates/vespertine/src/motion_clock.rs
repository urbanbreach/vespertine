//! The motion clock: one elapsed-time source for declarative motion and for automation.
//!
//! Motion time is the number of milliseconds elapsed since a renderer-local origin. It runs
//! with the wall clock until it is frozen. Automation and test hosts (DEV-006, DEV-007) freeze,
//! set, advance and resume it so that animations can be sampled deterministically; animation
//! code reads it through [`MotionClock::now_ms`] so it follows the same instant.
//!
//! - [`MotionClock::pause`] freezes the clock at the current instant and returns it.
//! - [`MotionClock::set`] freezes the clock at an absolute elapsed time.
//! - [`MotionClock::fast_forward`] adds a delta to the current instant (frozen or running) and
//!   freezes there.
//! - [`MotionClock::resume`] rebases the origin so that the elapsed time continues from the
//!   frozen value; wall time spent paused is not included.
//!
//! Every operation requests a new frame in every window so animations repaint at the new instant.

use crate::{App, Global};
use std::{
    sync::LazyLock,
    time::{Duration, Instant},
};

/// The shared motion clock, stored as an application global.
#[derive(Clone, Copy, Debug)]
pub struct MotionClock {
    origin: Instant,
    frozen_ms: Option<f64>,
}

impl Global for MotionClock {}

impl Default for MotionClock {
    fn default() -> Self {
        Self {
            origin: Instant::now(),
            frozen_ms: None,
        }
    }
}

static PROCESS_ORIGIN: LazyLock<Instant> = LazyLock::new(Instant::now);

fn process_origin() -> Instant {
    *PROCESS_ORIGIN
}

impl MotionClock {
    /// The elapsed motion time in milliseconds: the frozen value while the clock is frozen,
    /// otherwise wall time since the origin.
    pub fn elapsed_ms(&self) -> f64 {
        self.frozen_ms
            .unwrap_or_else(|| self.origin.elapsed().as_secs_f64() * 1000.0)
    }

    /// Whether the clock is frozen.
    pub fn is_paused(&self) -> bool {
        self.frozen_ms.is_some()
    }

    /// The current motion time of the application, in milliseconds. Applications that never
    /// operate the clock share one process-wide wall-clock origin.
    pub fn now_ms(cx: &App) -> f64 {
        match cx.try_global::<MotionClock>() {
            Some(clock) => clock.elapsed_ms(),
            None => process_origin().elapsed().as_secs_f64() * 1000.0,
        }
    }

    /// Whether the application's motion clock is frozen.
    pub fn paused(cx: &App) -> bool {
        cx.try_global::<MotionClock>()
            .is_some_and(MotionClock::is_paused)
    }

    fn operate(cx: &mut App, operation: impl FnOnce(&mut MotionClock) -> f64) -> f64 {
        if !cx.has_global::<MotionClock>() {
            // Share the process origin so reads before and after the first operation agree.
            cx.set_global(MotionClock {
                origin: process_origin(),
                frozen_ms: None,
            });
        }
        let now = operation(cx.global_mut::<MotionClock>());
        cx.refresh_windows();
        now
    }

    /// Freezes the clock at the current instant and returns it in milliseconds. Pausing a
    /// frozen clock keeps its value.
    pub fn pause(cx: &mut App) -> f64 {
        Self::operate(cx, |clock| {
            let now = clock.elapsed_ms();
            clock.frozen_ms = Some(now);
            now
        })
    }

    /// Freezes the clock at the absolute elapsed time `ms` and returns it. `ms` must be a finite,
    /// nonnegative number; callers validate it (see the automation protocol).
    pub fn set(cx: &mut App, ms: f64) -> f64 {
        debug_assert!(
            ms.is_finite() && ms >= 0.0,
            "motion time must be finite and nonnegative"
        );
        Self::operate(cx, |clock| {
            clock.frozen_ms = Some(ms);
            ms
        })
    }

    /// Adds `delta_ms` to the current instant (frozen or running), freezes the clock there and
    /// returns the new reading.
    pub fn fast_forward(cx: &mut App, delta_ms: f64) -> f64 {
        debug_assert!(
            delta_ms.is_finite() && delta_ms >= 0.0,
            "motion delta must be finite and nonnegative"
        );
        Self::operate(cx, |clock| {
            let now = clock.elapsed_ms() + delta_ms;
            clock.frozen_ms = Some(now);
            now
        })
    }

    /// Resumes a frozen clock so that the elapsed time continues from the frozen value, and
    /// returns that value. Resuming a running clock leaves it running.
    pub fn resume(cx: &mut App) -> f64 {
        Self::operate(cx, |clock| {
            let now = clock.elapsed_ms();
            if let Some(frozen) = clock.frozen_ms.take() {
                // A frozen value too large to rebase onto the monotonic clock leaves the origin
                // at the current instant, i.e. the clock restarts from zero.
                clock.origin = Duration::try_from_secs_f64(frozen / 1000.0)
                    .ok()
                    .and_then(|elapsed| Instant::now().checked_sub(elapsed))
                    .unwrap_or_else(Instant::now);
            }
            now
        })
    }
}
