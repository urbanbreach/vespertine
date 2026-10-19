//! Per-window frame statistics (DEV-003 R1).
//!
//! [`FrameStats`] accumulates what drawing a window did: phase times, layout-engine work, shaping,
//! retained-view reuse and scroll-layer reuse. Counters are plain increments that never alter
//! invalidation, caching or output. Timers are off until
//! [`Window::set_frame_timing_enabled`] turns them on; a disabled timer records zero time.
//!
//! The counters are compiled into every build (they are cheap increments) so that rendering code
//! never needs a feature check to record work; the public window methods that read, reset and
//! configure them exist only with `test-support`.

use crate::Window;
use std::time::{Duration, Instant};

/// Statistics accumulated by one window since creation or the last
/// [`Window::reset_frame_stats`].
///
/// Fields documented as "layout engine" are read from the layout engine's own counters when the
/// statistics are requested; every other field is incremented where the work happens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameStats {
    /// Draws that ran to completion.
    pub draws: u64,
    /// Time spent in complete draws (timer; zero unless timing is enabled).
    pub frame_time: Duration,
    /// Time spent building elements: view renders and layout requests of the root, excluding
    /// layout computation (timer).
    pub build_time: Duration,
    /// Time spent in the prepaint phase, excluding layout computation (timer).
    pub prepaint_time: Duration,
    /// Time spent in the paint phase (timer).
    pub paint_time: Duration,
    /// Time spent computing layout (timer).
    pub layout_time: Duration,
    /// Layout nodes created (layout engine).
    pub nodes_created: u64,
    /// Requests served by a persistent layout node of an earlier frame (layout engine).
    pub nodes_reused: u64,
    /// Style writes to the layout engine (layout engine).
    pub style_writes: u64,
    /// Measure-context writes to the layout engine, installs and in-place replacements
    /// (layout engine).
    pub context_writes: u64,
    /// Invocations of measuring functions, in layout and in replay (layout engine).
    pub measurements: u64,
    /// Measure rebinds: a new measuring function replaced the installed one in place because
    /// replaying the recorded probes gave the same answers, so nothing was dirtied
    /// (layout engine).
    pub rebinds: u64,
    /// Lines shaped: lines that missed the frame cache and the recent-shape store.
    pub lines_shaped: u64,
    /// Wall time spent shaping lines (timer; zero unless timing is enabled).
    pub shaping_time: Duration,
    /// Views whose render ran.
    pub views_built: u64,
    /// Views whose recorded output was replayed instead of rebuilt.
    pub views_reused: u64,
    /// Parents replayed around freshly rebuilt children.
    pub views_spliced: u64,
    /// Splice attempts that fell back to full reconstruction.
    pub splice_failures: u64,
    /// Scroll-layer composites drawn from the retained layer (PRF-004).
    pub layer_composites: u64,
    /// Scroll layers whose content was repainted (PRF-004).
    pub layer_repaints: u64,
    /// Layer tiles marked dirty (PRF-004).
    pub tiles_dirty: u64,
    /// Layer tiles rasterized (PRF-004).
    pub tiles_rasterized: u64,
    /// Layer rebuilds triggered by an input change (pointer, modifiers) rather than by a model
    /// change (PRF-004).
    pub input_layer_rebuilds: u64,
    /// List rows whose element subtree was built (PRF-005).
    pub rows_built: u64,
    /// List rows whose recorded output was replayed (PRF-005).
    pub rows_replayed: u64,
    /// List rows measured (PRF-005).
    pub rows_measured: u64,
}

/// A timer that records nothing unless it was started while timing was enabled, so a disabled
/// timer reports zero.
pub(crate) struct PhaseClock(Option<Instant>);

impl PhaseClock {
    pub(crate) fn start(enabled: bool) -> Self {
        Self(enabled.then(Instant::now))
    }

    pub(crate) fn elapsed(&self) -> Duration {
        self.0.map_or(Duration::ZERO, |start| start.elapsed())
    }
}

impl Window {
    /// The accumulating statistics, for counting work where it happens. Crate-internal: the
    /// public reads go through [`Window::frame_stats`].
    pub(crate) fn frame_stats_mut(&mut self) -> &mut FrameStats {
        &mut self.frame_stats
    }

    /// Starts a timer that reads zero unless frame timing is enabled.
    pub(crate) fn phase_clock(&self) -> PhaseClock {
        PhaseClock::start(self.frame_timing)
    }

    /// The statistics accumulated since creation or the last [`Window::reset_frame_stats`]:
    /// per-window counters plus the layout engine's work. Test support only.
    #[cfg(any(test, feature = "test-support"))]
    pub fn frame_stats(&self) -> FrameStats {
        let layout = self.layout_stats();
        FrameStats {
            nodes_created: layout.nodes_created,
            nodes_reused: layout.nodes_reused,
            style_writes: layout.style_writes,
            context_writes: layout.measure_writes,
            measurements: layout.measure_calls,
            rebinds: layout.replays_succeeded,
            ..self.frame_stats
        }
    }

    /// Resets every statistic to zero. Timing stays as configured. Test support only.
    #[cfg(any(test, feature = "test-support"))]
    pub fn reset_frame_stats(&mut self) {
        self.frame_stats = FrameStats::default();
        self.reset_layout_stats();
    }

    /// Enables or disables the phase and shaping timers (disabled by default). Counters are
    /// recorded either way and timing never changes what is drawn. The shaping timer is shared
    /// by every window of the application, which shares one text system. Test support only.
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_frame_timing_enabled(&mut self, enabled: bool) {
        self.frame_timing = enabled;
        self.text_system().set_shaping_timing(enabled);
    }

    /// Whether the timers are enabled. Test support only.
    #[cfg(any(test, feature = "test-support"))]
    pub fn frame_timing_enabled(&self) -> bool {
        self.frame_timing
    }
}

#[cfg(test)]
mod tests {
    use super::FrameStats;
    use crate::{
        AnyWindowHandle, AppContext as _, Context, IntoElement, ParentElement as _, Render,
        Styled as _, TestAppContext, Window, div, px,
    };
    use std::time::Duration;

    struct Rows;

    impl Render for Rows {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .flex_col()
                .flex()
                .children((0..6).map(|row| div().h(px(20.)).child(format!("row {row}"))))
                .size_full()
        }
    }

    fn drawn_window(cx: &mut TestAppContext, timing: bool) -> (AnyWindowHandle, FrameStats) {
        let window: AnyWindowHandle = cx.add_window(|_, _| Rows).into();
        cx.update_window(window, |_, window, _| {
            window.set_frame_timing_enabled(timing);
        })
        .unwrap();
        cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        let stats = cx
            .update_window(window, |_, window, _| window.frame_stats())
            .unwrap();
        (window, stats)
    }

    fn without_times(mut stats: FrameStats) -> FrameStats {
        stats.frame_time = Duration::ZERO;
        stats.build_time = Duration::ZERO;
        stats.prepaint_time = Duration::ZERO;
        stats.paint_time = Duration::ZERO;
        stats.layout_time = Duration::ZERO;
        stats.shaping_time = Duration::ZERO;
        // Both windows of a test share one text system, whose recent-shape store serves the
        // second window's lines; the number of lines actually shaped is not window-local.
        stats.lines_shaped = 0;
        stats
    }

    #[crate::test]
    fn reset_clears_every_statistic(cx: &mut TestAppContext) {
        let (window, stats) = drawn_window(cx, true);
        assert!(stats.draws >= 1, "{stats:?}");
        assert!(stats.nodes_created > 0, "{stats:?}");
        assert!(stats.views_built > 0, "{stats:?}");
        assert!(stats.lines_shaped > 0, "{stats:?}");
        assert!(stats.frame_time > Duration::ZERO, "{stats:?}");

        let after_reset = cx
            .update_window(window, |_, window, _| {
                window.reset_frame_stats();
                (window.frame_stats(), window.frame_timing_enabled())
            })
            .unwrap();
        assert_eq!(after_reset.0, FrameStats::default());
        assert!(after_reset.1, "reset keeps the timing setting");

        cx.update_window(window, |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx)
        })
        .unwrap();
        let stats = cx
            .update_window(window, |_, window, _| window.frame_stats())
            .unwrap();
        assert_eq!(stats.draws, 1, "accumulation restarts after a reset");
    }

    #[crate::test]
    fn disabled_timers_record_zero_without_altering_counters(cx: &mut TestAppContext) {
        let (timed_window, timed) = drawn_window(cx, true);
        let (plain_window, plain) = drawn_window(cx, false);

        assert!(
            !cx.update_window(plain_window, |_, window, _| window.frame_timing_enabled())
                .unwrap()
        );
        for time in [
            plain.frame_time,
            plain.build_time,
            plain.prepaint_time,
            plain.paint_time,
            plain.layout_time,
            plain.shaping_time,
        ] {
            assert_eq!(time, Duration::ZERO, "{plain:?}");
        }
        assert!(timed.frame_time > Duration::ZERO, "{timed:?}");
        assert!(timed.paint_time > Duration::ZERO, "{timed:?}");

        assert_eq!(without_times(timed), without_times(plain));

        let timed_output = cx
            .update_window(timed_window, |_, window, _| window.painted_primitives())
            .unwrap();
        let plain_output = cx
            .update_window(plain_window, |_, window, _| window.painted_primitives())
            .unwrap();
        assert!(!plain_output.is_empty());
        assert_eq!(timed_output, plain_output, "timing never changes output");
    }
}
