//! Demonstrates the shared, parked animation clock (ANM-004) on a real window with real timers.
//!
//! Mounts one, then one hundred phase-aligned activity indicators, hides all of them, and
//! checks that the clock parks within the 300 ms lease plus at most one 33 ms clock interval
//! (with some slack for real timers) and then schedules nothing. Prints the counts, then `PASS`
//! and exits 0 after a few seconds; any violated expectation prints `FAIL` and exits nonzero.
//!
//! Run: cargo run -p vespertine --example animation_clock

use std::time::{Duration, Instant};

use vespertine::{
    ACTIVITY_INDICATOR_PERIOD, ANIMATION_CLOCK_INTERVAL, ANIMATION_LEASE_DURATION,
    ActivityIndicator, App, Bounds, Context, Entity, IntoElement, MotionPreference, Render,
    StyleRefinement, Window, WindowBounds, WindowOptions, div, prelude::*, px, size,
};
use vespertine_platform::application;

/// Slack for real timers and scheduling latency on a loaded machine.
const SLACK: Duration = Duration::from_millis(120);

struct Indicator {
    state: ActivityIndicator,
}

impl Render for Indicator {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.state = window.activity_indicator(cx);
        div()
            .size_1()
            .bg(vespertine::black().opacity(self.state.opacity))
    }
}

struct Root {
    indicators: Vec<Entity<Indicator>>,
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // Cached, so only the views the clock notifies re-render (and renew their lease).
        div()
            .flex()
            .flex_wrap()
            .children(self.indicators.iter().map(|indicator| {
                indicator
                    .clone()
                    .cached(StyleRefinement::default().size_1())
            }))
    }
}

fn fail(message: &str) -> ! {
    eprintln!("FAIL: {message}");
    std::process::exit(1);
}

fn check(condition: bool, message: &str) {
    if !condition {
        fail(message);
    }
}

fn main() {
    application().run(|cx: &mut App| {
        // Indicators must animate at normal motion whatever the desktop's setting is.
        cx.set_motion_preference(MotionPreference::Off);
        let bounds = Bounds::centered(None, size(px(200.), px(120.)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |_, cx| cx.new(|_| Root { indicators: Vec::new() }),
            )
            .unwrap();
        let root = window.update(cx, |_, _, cx| cx.entity()).unwrap();
        cx.activate(true);

        // Watchdog: a stuck run must fail rather than hang.
        cx.spawn(async move |cx| {
            cx.background_executor().timer(Duration::from_secs(10)).await;
            fail("timed out");
        })
        .detach();

        cx.spawn(async move |cx| {
            let timer = |duration| cx.background_executor().timer(duration);
            let stats = |cx: &vespertine::AsyncApp| cx.update(|cx| cx.animation_clock_stats());
            let mount = |cx: &vespertine::AsyncApp, count: usize| {
                cx.update(|cx| {
                    root.update(cx, |root, cx| {
                        root.indicators = (0..count)
                            .map(|_| {
                                cx.new(|_| Indicator {
                                    state: ActivityIndicator {
                                        phase: 0.0,
                                        gentle_cue: false,
                                        opacity: 1.0,
                                    },
                                })
                            })
                            .collect();
                        cx.notify();
                    })
                });
            };

            // 1. One indicator.
            cx.update(|cx| cx.reset_animation_clock_stats());
            mount(cx, 1);
            timer(Duration::from_millis(700)).await;
            let one = stats(cx);
            println!(
                "1 indicator, 0.7 s: {} notifications, {} clock wakeups",
                one.notifications, one.wakeups
            );
            check(one.notifications >= 4, "a mounted indicator is repainted by the clock");
            check(one.notifications <= one.wakeups + 1, "one notification per tick at most");

            // 2. One hundred indicators, all phase-aligned on the shared epoch.
            cx.update(|cx| cx.reset_animation_clock_stats());
            mount(cx, 100);
            timer(Duration::from_millis(700)).await;
            let hundred = stats(cx);
            println!(
                "100 indicators, 0.7 s: {} notifications, {} clock wakeups",
                hundred.notifications, hundred.wakeups
            );
            // One clock: wakeups are bounded by the interval, not by the number of views.
            check(
                hundred.wakeups <= 700 / ANIMATION_CLOCK_INTERVAL.as_millis() as u64 + 2,
                "the clock wakes at most once per interval however many views subscribe",
            );
            check(
                hundred.notifications >= 50 * hundred.wakeups / 2,
                "every indicator is repainted on most ticks",
            );
            let phases: Vec<f32> = cx.update(|cx| {
                root.read(cx)
                    .indicators
                    .iter()
                    .map(|indicator| indicator.read(cx).state.phase)
                    .collect()
            });
            let reference = phases[0];
            let period = ACTIVITY_INDICATOR_PERIOD.as_secs_f32();
            let max_skew = phases
                .iter()
                .map(|phase| {
                    let delta = (phase - reference).abs();
                    delta.min(1.0 - delta) * period
                })
                .fold(0.0, f32::max);
            println!("all 100 indicators within {:.1} ms of phase {reference:.3}", max_skew * 1000.0);
            check(max_skew < 0.05, "indicators are phase-aligned");

            // 3. Hide everything; the clock must park within lease + one interval.
            let before = stats(cx);
            mount(cx, 0);
            let hidden_at = Instant::now();
            let mut seen = stats(cx).notifications;
            let mut last_notification = Duration::ZERO;
            let budget = ANIMATION_LEASE_DURATION + ANIMATION_CLOCK_INTERVAL + SLACK;
            while cx.update(|cx| cx.animation_clock_is_running()) {
                timer(Duration::from_millis(5)).await;
                let now = stats(cx).notifications;
                if now != seen {
                    seen = now;
                    last_notification = hidden_at.elapsed();
                }
                check(hidden_at.elapsed() <= budget, "the clock failed to park in time");
            }
            let parked_after = hidden_at.elapsed();
            println!(
                "hidden: last notification {last_notification:?} after hiding, clock parked after {parked_after:?}"
            );
            check(
                last_notification <= ANIMATION_LEASE_DURATION + ANIMATION_CLOCK_INTERVAL + SLACK,
                "no notifications after the lease lapsed",
            );
            check(parked_after >= ANIMATION_LEASE_DURATION - Duration::from_millis(40), "parked only after the lease");

            let parked = stats(cx);
            check(parked.wakeups > before.wakeups, "the clock ran while views were mounted");
            timer(Duration::from_millis(1500)).await;
            let later = stats(cx);
            println!(
                "1.5 s later: {} further wakeups, {} further notifications",
                later.wakeups - parked.wakeups,
                later.notifications - parked.notifications
            );
            check(later == parked, "a parked clock schedules nothing");
            println!("PASS");
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
}
