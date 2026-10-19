//! A bounded, parked, application-wide clock for repeating cosmetic animations.
//!
//! Repeating decorative animations (spinners, pulses, activity cells, label rolls) derive
//! their phase from one epoch shared by the whole [`App`], so every instance is phase-locked.
//! Views keep the clock awake only while their animated content is painted: each paint renews a
//! 300 ms *lease* keyed by the painting view. The clock ticks every 33 ms on the foreground
//! executor, drops expired leases, notifies the views whose stride divides the tick counter, and
//! *parks* (schedules nothing) when a tick finds no live lease. The next lease request restarts
//! it.
//!
//! The clock does not drive the display clock: it only decides when views invalidate, and
//! display-frame delivery wakes on that invalidation.

use std::time::Duration;

use smallvec::SmallVec;
use vespertine_scheduler::Instant;

use crate::{App, EntityId, Global, Window};
use vespertine_collections::FxHashMap;

/// Access to a default-constructed global that, unlike [`App::default_global`], does not queue a
/// global-observer notification. The clock and the motion policy are read on every paint.
pub(crate) fn quiet_global_mut<G: Global + Default>(cx: &mut App) -> &mut G {
    cx.globals_by_type
        .entry(std::any::TypeId::of::<G>())
        .or_insert_with(|| Box::<G>::default())
        .downcast_mut::<G>()
        .unwrap()
}

/// Interval between clock ticks (about 30 Hz).
pub const ANIMATION_CLOCK_INTERVAL: Duration = Duration::from_millis(33);

/// How long a lease lasts after its latest renewal.
pub const ANIMATION_LEASE_DURATION: Duration = Duration::from_millis(300);

/// Stride of subscribers that want to be notified on every clock tick (about 30 Hz).
pub const NORMAL_ANIMATION_STRIDE: u32 = 1;

/// Stride of subscribers that want to be notified on every second clock tick (about 15 Hz).
pub const SLOW_ANIMATION_STRIDE: u32 = 2;

/// Period of the activity-indicator animation.
pub const ACTIVITY_INDICATOR_PERIOD: Duration = Duration::from_millis(2400);

/// Marker for "no stride pending": a lease that was just notified and has not been renewed since.
const NO_PENDING_STRIDE: u32 = u32::MAX;

/// The cadence at which a view wants to be notified by the shared clock.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum AnimationCadence {
    /// Every clock tick (stride 1, about 30 Hz).
    Normal,
    /// Every second clock tick (stride 2, about 15 Hz).
    Slow,
}

impl AnimationCadence {
    /// The stride, in clock ticks, of this cadence.
    pub const fn stride(self) -> u32 {
        match self {
            AnimationCadence::Normal => NORMAL_ANIMATION_STRIDE,
            AnimationCadence::Slow => SLOW_ANIMATION_STRIDE,
        }
    }
}

/// Counters describing the work the shared animation clock has caused. They are always
/// available (not only in test builds) so performance scenes can read them.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct AnimationClockStats {
    /// Foreground wakeups: how many times the clock's timer fired and ran a tick.
    pub wakeups: u64,
    /// Ticks that found a live lease and advanced the tick counter.
    pub ticks: u64,
    /// Clock-driven frame requests: view notifications issued by the clock.
    pub notifications: u64,
    /// Lease renewals requested by views.
    pub lease_renewals: u64,
    /// Times the clock was started (the first lease request, or the first after parking).
    pub starts: u64,
    /// Times a tick found no live lease and parked the clock.
    pub parks: u64,
}

#[derive(Copy, Clone, Debug)]
struct Lease {
    expires: Instant,
    /// Minimum stride requested since the last notification, or [`NO_PENDING_STRIDE`].
    pending_stride: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TickOutcome {
    /// No live lease remained; the clock must stop scheduling ticks.
    Park,
    /// The clock keeps running; these views are to be notified.
    Notify(SmallVec<[EntityId; 4]>),
}

/// The shared clock's state. Stored as an application global.
#[derive(Default)]
pub(crate) struct AnimationClock {
    leases: FxHashMap<EntityId, Lease>,
    tick: u64,
    running: bool,
    stats: AnimationClockStats,
}

impl Global for AnimationClock {}

impl AnimationClock {
    /// Records a lease renewal. Returns whether the clock must be started.
    fn renew(&mut self, view: EntityId, stride: u32, now: Instant) -> bool {
        let stride = stride.max(1);
        self.stats.lease_renewals += 1;
        let lease = self.leases.entry(view).or_insert(Lease {
            expires: now,
            pending_stride: NO_PENDING_STRIDE,
        });
        lease.expires = now + ANIMATION_LEASE_DURATION;
        lease.pending_stride = lease.pending_stride.min(stride);
        if self.running {
            false
        } else {
            self.running = true;
            self.stats.starts += 1;
            true
        }
    }

    /// Runs one clock tick: drops expired leases, then parks or advances and selects the views to
    /// notify. A notified lease's pending stride resets.
    fn tick(&mut self, now: Instant) -> TickOutcome {
        self.stats.wakeups += 1;
        self.leases.retain(|_, lease| lease.expires > now);
        if self.leases.is_empty() {
            self.running = false;
            self.stats.parks += 1;
            return TickOutcome::Park;
        }
        self.tick += 1;
        self.stats.ticks += 1;
        let mut views = SmallVec::new();
        for (view, lease) in self.leases.iter_mut() {
            if lease.pending_stride != NO_PENDING_STRIDE
                && self.tick.is_multiple_of(lease.pending_stride as u64)
            {
                lease.pending_stride = NO_PENDING_STRIDE;
                views.push(*view);
            }
        }
        self.stats.notifications += views.len() as u64;
        TickOutcome::Notify(views)
    }
}

/// The normalized phase in `[0, 1)` of a repeating animation of the given `period` at `now`:
/// the fractional part of the time since `epoch` divided by `period`. A zero period has phase 0.
pub fn animation_phase_at(now: Instant, epoch: Instant, period: Duration) -> f32 {
    if period.is_zero() {
        return 0.0;
    }
    let elapsed = now.saturating_duration_since(epoch);
    // Reduce modulo the period in integer nanoseconds before converting to float, which would
    // lose sub-second precision for long-running applications.
    let remainder = Duration::from_nanos((elapsed.as_nanos() % period.as_nanos()) as u64);
    // The f32 quotient can round up to exactly 1.0 right below a period boundary.
    let phase = remainder.as_secs_f32() / period.as_secs_f32();
    if phase >= 1.0 { 0.0 } else { phase }
}

/// The smooth wave used by activity cues: `0.5 - 0.5 * cos(2 * pi * p)`, which is 0 at phase 0,
/// 1 at phase 0.5 and returns to 0 at phase 1.
pub fn animation_wave(phase: f32) -> f32 {
    0.5 - 0.5 * (std::f32::consts::TAU * phase).cos()
}

/// The uniform opacity of the activity-indicator gentle cue at `phase`: `0.6 + 0.2 * wave(phase)`,
/// so 0.6 at phase 0 and 0.8 at phase 0.5.
pub fn gentle_cue_opacity(phase: f32) -> f32 {
    0.6 + 0.2 * animation_wave(phase)
}

/// What an activity indicator should draw. See [`Window::activity_indicator`].
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ActivityIndicator {
    /// Normalized phase in `[0, 1)` over [`ACTIVITY_INDICATOR_PERIOD`]. Zero when frozen.
    pub phase: f32,
    /// Whether the indicator is under system-only reduced motion and must show only the gentle
    /// uniform brightness cue: every cell animates in phase, with [`ActivityIndicator::opacity`]
    /// applied uniformly. The indicator MUST NOT move or scale glyphs or cells in this mode.
    pub gentle_cue: bool,
    /// The uniform opacity multiplier to apply: `0.6 + 0.2 * wave(phase)` for the gentle cue and
    /// `1.0` otherwise.
    pub opacity: f32,
}

impl ActivityIndicator {
    const fn frozen() -> Self {
        Self {
            phase: 0.0,
            gentle_cue: false,
            opacity: 1.0,
        }
    }
}

impl App {
    /// Renews the animation lease of `view` with the given stride (in clock ticks), starting the
    /// shared clock if it is parked. The clock notifies `view` at the fastest stride requested
    /// since its last notification. A renewal records the minimum of the stride already pending
    /// and `stride`. The lease lapses 300 ms after its latest renewal, so views simply stop
    /// renewing when they are no longer painted.
    pub fn renew_animation_lease(&mut self, view: EntityId, stride: u32) {
        let now = self.background_executor().now();
        let start = quiet_global_mut::<AnimationClock>(self).renew(view, stride, now);
        if start {
            self.spawn(async move |cx| {
                loop {
                    cx.background_executor()
                        .timer(ANIMATION_CLOCK_INTERVAL)
                        .await;
                    if !cx.update(run_clock_tick) {
                        break;
                    }
                }
            })
            .detach();
        }
    }

    /// Drops every lease. Views that still animate renew theirs when they next repaint.
    pub(crate) fn clear_animation_leases(&mut self) {
        if let Some(clock) = self
            .globals_by_type
            .get_mut(&std::any::TypeId::of::<AnimationClock>())
        {
            clock
                .downcast_mut::<AnimationClock>()
                .unwrap()
                .leases
                .clear();
        }
    }

    /// The phase in `[0, 1)` of a repeating animation of the given period right now, derived from
    /// the shared epoch. Returns zero under reduced motion. Does not renew a lease; see
    /// [`Window::animation_phase`].
    pub fn animation_phase(&self, period: Duration) -> f32 {
        if self.reduce_motion() {
            return 0.0;
        }
        animation_phase_at(
            self.background_executor().now(),
            self.synced_animation_epoch,
            period,
        )
    }

    /// A snapshot of the shared animation clock's counters.
    pub fn animation_clock_stats(&self) -> AnimationClockStats {
        self.try_global::<AnimationClock>()
            .map(|clock| clock.stats)
            .unwrap_or_default()
    }

    /// Resets the shared animation clock's counters to zero (leases and the running state are
    /// unaffected).
    pub fn reset_animation_clock_stats(&mut self) {
        quiet_global_mut::<AnimationClock>(self).stats = AnimationClockStats::default();
    }

    /// Whether the shared animation clock currently has a scheduled tick (it is not parked).
    pub fn animation_clock_is_running(&self) -> bool {
        self.try_global::<AnimationClock>()
            .is_some_and(|clock| clock.running)
    }

    /// Number of views currently holding a (possibly not yet expired) animation lease.
    pub fn animation_lease_count(&self) -> usize {
        self.try_global::<AnimationClock>()
            .map_or(0, |clock| clock.leases.len())
    }
}

/// One foreground clock tick. Returns whether the clock keeps running.
fn run_clock_tick(cx: &mut App) -> bool {
    let now = cx.background_executor().now();
    match quiet_global_mut::<AnimationClock>(cx).tick(now) {
        TickOutcome::Park => false,
        TickOutcome::Notify(views) => {
            for view in views {
                cx.notify(view);
            }
            true
        }
    }
}

impl Window {
    /// Renews the animation lease of the view being painted with the given stride. Call this only
    /// while the view's animated content is actually painted and motion is not reduced. See
    /// [`App::renew_animation_lease`].
    pub fn renew_animation_lease(&mut self, cx: &mut App, stride: u32) {
        cx.renew_animation_lease(self.current_view(), stride);
    }

    /// The normalized phase in `[0, 1)` of a repeating animation of the given `period`, derived
    /// from the application's shared epoch so that all instances are phase-locked. The painting
    /// view's lease is renewed at the cadence's stride so that it repaints.
    ///
    /// Under reduced motion this returns a static `0.0` and renews no lease.
    pub fn animation_phase(
        &mut self,
        cx: &mut App,
        period: Duration,
        cadence: AnimationCadence,
    ) -> f32 {
        if cx.reduce_motion() || period.is_zero() {
            return 0.0;
        }
        self.renew_animation_lease(cx, cadence.stride());
        cx.animation_phase(period)
    }

    /// [`Window::animation_phase`] at [`AnimationCadence::Normal`] (every tick, about 30 Hz).
    pub fn animation_phase_normal(&mut self, cx: &mut App, period: Duration) -> f32 {
        self.animation_phase(cx, period, AnimationCadence::Normal)
    }

    /// [`Window::animation_phase`] at [`AnimationCadence::Slow`] (every second tick, about 15 Hz).
    pub fn animation_phase_slow(&mut self, cx: &mut App, period: Duration) -> f32 {
        self.animation_phase(cx, period, AnimationCadence::Slow)
    }

    /// Phase and gentle-cue flag for an activity indicator.
    ///
    /// - Normal motion: the phase over [`ACTIVITY_INDICATOR_PERIOD`] at the normal stride, no
    ///   gentle cue.
    /// - System-only reduced motion (preference follows the system, the system reports reduced
    ///   motion, and the window is not paused by the background setting): all cells in phase at
    ///   the slow stride with a uniform opacity of `0.6 + 0.2 * wave(phase)`.
    /// - Forced reduction or background pause: frozen (phase zero, full opacity, no lease).
    pub fn activity_indicator(&mut self, cx: &mut App) -> ActivityIndicator {
        if !cx.reduce_motion() {
            let phase = self.animation_phase_normal(cx, ACTIVITY_INDICATOR_PERIOD);
            return ActivityIndicator {
                phase,
                gentle_cue: false,
                opacity: 1.0,
            };
        }
        if !cx.motion_policy().system_only_reduction() {
            return ActivityIndicator::frozen();
        }
        self.renew_animation_lease(cx, SLOW_ANIMATION_STRIDE);
        let phase = animation_phase_at(
            cx.background_executor().now(),
            cx.synced_animation_epoch,
            ACTIVITY_INDICATOR_PERIOD,
        );
        ActivityIndicator {
            phase,
            gentle_cue: true,
            opacity: gentle_cue_opacity(phase),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Context, IntoElement, Render, TestAppContext, WindowHandle, div, prelude::*, px, size,
    };

    /// One clock interval: each advance of the test clock runs exactly one tick.
    const STEP: Duration = ANIMATION_CLOCK_INTERVAL;

    struct ClockView {
        cadences: Vec<AnimationCadence>,
        renders: usize,
        phase: f32,
    }

    impl ClockView {
        fn new(cadences: &[AnimationCadence]) -> Self {
            Self {
                cadences: cadences.to_vec(),
                renders: 0,
                phase: 0.0,
            }
        }
    }

    impl Render for ClockView {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.renders += 1;
            for cadence in self.cadences.clone() {
                self.phase = window.animation_phase(cx, Duration::from_secs(1), cadence);
            }
            div()
        }
    }

    struct Root {
        children: Vec<crate::Entity<ClockView>>,
    }

    impl Render for Root {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            // Cached, so that only a notified view re-renders (and renews its lease).
            div().children(self.children.iter().map(|child| {
                child
                    .clone()
                    .cached(crate::StyleRefinement::default().size_full())
            }))
        }
    }

    fn open(
        cx: &mut TestAppContext,
        views: &[&[AnimationCadence]],
    ) -> (WindowHandle<Root>, Vec<crate::Entity<ClockView>>) {
        let children = std::cell::RefCell::new(Vec::new());
        let window = cx.open_window(size(px(100.), px(100.)), |_, cx| {
            let entities: Vec<_> = views
                .iter()
                .map(|cadences| cx.new(|_| ClockView::new(cadences)))
                .collect();
            *children.borrow_mut() = entities.clone();
            Root { children: entities }
        });
        cx.run_until_parked();
        (window, children.into_inner())
    }

    fn tick(cx: &mut TestAppContext) {
        cx.executor().advance_clock(STEP);
        cx.run_until_parked();
    }

    fn renders(cx: &mut TestAppContext, view: &crate::Entity<ClockView>) -> usize {
        view.read_with(cx, |view, _| view.renders)
    }

    fn set_cadences(
        cx: &mut TestAppContext,
        view: &crate::Entity<ClockView>,
        cadences: &[AnimationCadence],
    ) {
        view.update(cx, |view, cx| {
            view.cadences = cadences.to_vec();
            cx.notify();
        });
        cx.run_until_parked();
    }

    fn stats(cx: &mut TestAppContext) -> AnimationClockStats {
        cx.update(|cx| cx.animation_clock_stats())
    }

    #[test]
    fn phase_is_fractional_time_since_epoch_over_period() {
        let epoch = Instant::now();
        let period = Duration::from_millis(1200);
        assert_eq!(animation_phase_at(epoch, epoch, period), 0.0);
        let quarter = animation_phase_at(epoch + Duration::from_millis(300), epoch, period);
        assert!((quarter - 0.25).abs() < 1e-6);
        let wrapped = animation_phase_at(epoch + Duration::from_millis(1500), epoch, period);
        assert!((wrapped - 0.25).abs() < 1e-6);
        assert_eq!(animation_phase_at(epoch, epoch, Duration::ZERO), 0.0);
        assert!(animation_phase_at(epoch + period - Duration::from_nanos(1), epoch, period) < 1.0);
    }

    #[test]
    fn wave_and_gentle_cue_opacity_follow_the_cosine() {
        assert!(animation_wave(0.0).abs() < 1e-6);
        assert!((animation_wave(0.5) - 1.0).abs() < 1e-6);
        assert!((gentle_cue_opacity(0.0) - 0.6).abs() < 1e-6);
        assert!((gentle_cue_opacity(0.5) - 0.8).abs() < 1e-6);
        assert!((gentle_cue_opacity(0.25) - 0.7).abs() < 1e-6);
    }

    #[vespertine::test]
    fn stride_resets_after_notification_and_renewal_keeps_the_minimum(cx: &mut TestAppContext) {
        let views: Vec<_> = (0..2).map(|_| cx.new(|_| ClockView::new(&[]))).collect();
        let (a, b) = (views[0].entity_id(), views[1].entity_id());
        let mut clock = AnimationClock::default();
        let t0 = Instant::now();

        // Renewals before a notification record the minimum stride, whatever the order.
        assert!(clock.renew(a, 2, t0));
        assert!(!clock.renew(a, 1, t0));
        assert!(!clock.renew(a, 2, t0));
        assert!(!clock.renew(b, 2, t0));

        // Tick 1: only `a` (stride 1 pending) is notified, and its stride resets.
        assert_eq!(
            clock.tick(t0 + Duration::from_millis(33)),
            TickOutcome::Notify(SmallVec::from_slice(&[a]))
        );
        // Tick 2: `b` (stride 2) is due; `a` has no pending stride until it renews again.
        assert_eq!(
            clock.tick(t0 + Duration::from_millis(66)),
            TickOutcome::Notify(SmallVec::from_slice(&[b]))
        );
        // `a` renews with stride 2 only: it is notified on the next even tick (4), not on 3.
        clock.renew(a, 2, t0 + Duration::from_millis(66));
        assert_eq!(
            clock.tick(t0 + Duration::from_millis(99)),
            TickOutcome::Notify(SmallVec::new())
        );
        assert_eq!(
            clock.tick(t0 + Duration::from_millis(132)),
            TickOutcome::Notify(SmallVec::from_slice(&[a]))
        );
    }

    #[vespertine::test]
    fn lease_expires_300ms_after_renewal_and_a_tick_without_leases_parks(cx: &mut TestAppContext) {
        let view = cx.new(|_| ClockView::new(&[]));
        let id = view.entity_id();
        let mut clock = AnimationClock::default();
        let t0 = Instant::now();
        assert!(clock.renew(id, 1, t0));
        // Still live just before 300 ms.
        assert!(matches!(
            clock.tick(t0 + Duration::from_millis(299)),
            TickOutcome::Notify(_)
        ));
        // Lapsed at 300 ms: parks and stays parked until the next request.
        assert_eq!(
            clock.tick(t0 + Duration::from_millis(300)),
            TickOutcome::Park
        );
        assert!(!clock.running);
        assert!(clock.leases.is_empty());
        assert!(clock.renew(id, 1, t0 + Duration::from_millis(400)));
        assert!(clock.running);
    }

    #[vespertine::test]
    fn clock_parks_after_the_lease_lapses_and_restarts_on_the_next_request(
        cx: &mut TestAppContext,
    ) {
        let (_window, views) = open(cx, &[&[AnimationCadence::Normal]]);
        let view = &views[0];
        assert!(cx.update(|cx| cx.animation_clock_is_running()));
        assert_eq!(stats(cx).starts, 1);

        // The view is repainted on every tick while it renews.
        for expected in 2..=5 {
            tick(cx);
            assert_eq!(renders(cx, view), expected);
        }

        // Hide the view: it stops renewing, so within lease + one interval the clock parks.
        set_cadences(cx, view, &[]);
        let hidden_at = cx.executor().now();
        let mut elapsed = Duration::ZERO;
        while cx.update(|cx| cx.animation_clock_is_running()) {
            tick(cx);
            elapsed = cx.executor().now() - hidden_at;
            assert!(
                elapsed <= ANIMATION_LEASE_DURATION + ANIMATION_CLOCK_INTERVAL + STEP,
                "clock still running {elapsed:?} after the last renewal"
            );
        }
        assert!(elapsed >= ANIMATION_LEASE_DURATION);
        let parked = stats(cx);
        assert_eq!(parked.parks, 1);
        let renders_when_parked = renders(cx, view);

        // Parked: nothing is scheduled however long we wait.
        cx.executor().advance_clock(Duration::from_secs(5));
        cx.run_until_parked();
        assert_eq!(stats(cx), parked);
        assert_eq!(renders(cx, view), renders_when_parked);

        // The next lease request restarts the clock.
        set_cadences(cx, view, &[AnimationCadence::Normal]);
        assert!(cx.update(|cx| cx.animation_clock_is_running()));
        assert_eq!(stats(cx).starts, 2);
        let before = renders(cx, view);
        tick(cx);
        assert_eq!(renders(cx, view), before + 1);
    }

    #[vespertine::test]
    fn removing_a_fast_subscriber_returns_slow_views_to_every_second_tick(cx: &mut TestAppContext) {
        let (_window, views) = open(
            cx,
            &[&[AnimationCadence::Normal], &[AnimationCadence::Slow]],
        );
        let (fast, slow) = (&views[0], &views[1]);
        let (fast0, slow0) = (renders(cx, fast), renders(cx, slow));
        for _ in 0..6 {
            tick(cx);
        }
        assert_eq!(renders(cx, fast) - fast0, 6);
        assert_eq!(renders(cx, slow) - slow0, 3);

        // Retire the fast subscriber; the slow one keeps its lower cadence.
        set_cadences(cx, fast, &[]);
        let slow1 = renders(cx, slow);
        for _ in 0..6 {
            tick(cx);
        }
        assert_eq!(renders(cx, slow) - slow1, 3);
        // The retired view's lease lapses without any explicit unsubscription.
        for _ in 0..4 {
            tick(cx);
        }
        assert_eq!(cx.update(|cx| cx.animation_lease_count()), 1);
    }

    #[vespertine::test]
    fn a_view_with_fast_and_slow_animations_is_notified_every_tick_until_the_fast_one_retires(
        cx: &mut TestAppContext,
    ) {
        let (_window, views) = open(cx, &[&[AnimationCadence::Slow, AnimationCadence::Normal]]);
        let view = &views[0];
        let r0 = renders(cx, view);
        for _ in 0..4 {
            tick(cx);
        }
        assert_eq!(renders(cx, view) - r0, 4);

        // The retired fast animation no longer pulls the view up: the next notification still
        // honors the pending fast stride and then resets it, so only the slow request is
        // renewed afterwards.
        set_cadences(cx, view, &[AnimationCadence::Slow]);
        tick(cx);
        let r1 = renders(cx, view);
        for _ in 0..6 {
            tick(cx);
        }
        assert_eq!(renders(cx, view) - r1, 3);
    }

    #[vespertine::test]
    fn reduced_motion_phase_is_zero_and_renews_no_lease(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let (_window, views) = open(cx, &[&[AnimationCadence::Normal]]);
        let view = &views[0];
        cx.executor().advance_clock(Duration::from_millis(777));
        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |view, _| view.phase), 0.0);
        assert_eq!(stats(cx), AnimationClockStats::default());
        assert!(!cx.update(|cx| cx.animation_clock_is_running()));
        assert_eq!(cx.update(|cx| cx.animation_lease_count()), 0);
    }

    #[vespertine::test]
    fn enabling_reduced_motion_settles_an_in_flight_animation_without_more_frames(
        cx: &mut TestAppContext,
    ) {
        let (_window, views) = open(cx, &[&[AnimationCadence::Normal]]);
        let view = &views[0];
        tick(cx);
        tick(cx);
        assert!(view.read_with(cx, |view, _| view.phase) > 0.0);

        cx.update(|cx| cx.set_reduce_motion(true));
        cx.run_until_parked();
        assert_eq!(view.read_with(cx, |view, _| view.phase), 0.0);
        let settled = renders(cx, view);

        // No further frames are requested; the leftover lease lapses and the clock parks.
        for _ in 0..12 {
            tick(cx);
        }
        assert_eq!(renders(cx, view), settled);
        assert!(!cx.update(|cx| cx.animation_clock_is_running()));
    }

    #[vespertine::test]
    fn all_views_share_one_phase_regardless_of_when_they_subscribed(cx: &mut TestAppContext) {
        let (window, views) = open(cx, &[&[AnimationCadence::Normal], &[]]);
        // The second view subscribes 130 ms (and an arbitrary fraction of a period) later.
        for _ in 0..3 {
            tick(cx);
        }
        set_cadences(cx, &views[1], &[AnimationCadence::Slow]);
        let (first, second) = (views[0].clone(), views[1].clone());
        let _ = window;
        for _ in 0..4 {
            tick(cx);
        }
        // Force both to render in one frame, at one instant.
        first.update(cx, |_, cx| cx.notify());
        second.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        let a = first.read_with(cx, |view, _| view.phase);
        let b = second.read_with(cx, |view, _| view.phase);
        assert!(a > 0.0);
        assert!((a - b).abs() < 1e-3, "phases differ: {a} vs {b}");
        let expected = cx.update(|cx| cx.animation_phase(Duration::from_secs(1)));
        assert!((a - expected).abs() < 1e-3);
    }

    #[vespertine::test]
    fn synced_repeating_animations_and_the_clock_share_the_epoch(cx: &mut TestAppContext) {
        cx.executor().advance_clock(Duration::from_millis(1250));
        let phase = cx.update(|cx| cx.animation_phase(Duration::from_secs(1)));
        let direct = cx.update(|cx| {
            animation_phase_at(
                cx.background_executor().now(),
                cx.synced_animation_epoch,
                Duration::from_secs(1),
            )
        });
        assert!((phase - direct).abs() < 1e-3);
        assert!((phase - 0.25).abs() < 1e-2);
    }
}
