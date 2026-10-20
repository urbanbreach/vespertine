//! Touch fling demonstration (INP-003).
//!
//! Opens a scrollable list and replays two scripted touch gestures through the
//! window's ordinary touch input path, exactly as a touch screen would deliver
//! them:
//!
//! 1. a quick flick that eases off and wobbles slightly at the very end (a
//!    plain quadratic fit of such a tail would cancel the flick), which must
//!    keep scrolling with momentum after the finger lifts;
//! 2. a slow drag, which must stop the moment the finger lifts.
//!
//! The example prints how far each gesture scrolled before and after release
//! and exits. Pass `--interactive` to keep the window open for use with a real
//! touch screen afterwards.

#[path = "example_support/fonts.rs"]
mod example_support;

use std::time::Duration;

use vespertine::{
    App, AppContext as _, AsyncApp, Bounds, Context, InputEvent as _, Pixels, ScrollHandle,
    TouchEvent, TouchId, TouchPhase, Window, WindowBounds, WindowHandle, WindowOptions, div, point,
    prelude::*, px, size,
};
use vespertine_platform::application;

struct TouchFling {
    scroll: ScrollHandle,
}

impl Render for TouchFling {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let rows = (0..400).map(|row| {
            div()
                .h(px(32.))
                .px_4()
                .border_b_1()
                .border_color(vespertine::black().opacity(0.1))
                .child(format!("Row {row}"))
        });
        div()
            .id("list")
            .bg(vespertine::white())
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .children(rows)
    }
}

async fn send(
    window: WindowHandle<TouchFling>,
    cx: &mut AsyncApp,
    phase: TouchPhase,
    position: (f32, f32),
) {
    window
        .update(cx, |_, window, cx| {
            window.dispatch_event(
                TouchEvent {
                    id: TouchId(1),
                    phase,
                    position: point(px(position.0), px(position.1)),
                    predicted_position: None,
                    force: None,
                }
                .to_platform_input(),
                cx,
            );
        })
        .expect("window is open");
}

async fn sleep(cx: &AsyncApp, duration: Duration) {
    cx.background_executor().timer(duration).await;
}

fn offset(window: WindowHandle<TouchFling>, cx: &mut AsyncApp) -> Pixels {
    window
        .read_with(cx, |view, _| view.scroll.offset().y)
        .expect("window is open")
}

/// Replays `samples` (milliseconds since the touch started, y position) with
/// real-time spacing, lifts the finger at the last sample's position and
/// returns the scroll offsets at release and a while after it.
async fn gesture(
    window: WindowHandle<TouchFling>,
    cx: &mut AsyncApp,
    samples: &[(u64, f32)],
) -> (Pixels, Pixels) {
    let x = 200.;
    let mut previous = 0;
    for (index, &(millis, y)) in samples.iter().enumerate() {
        sleep(cx, Duration::from_millis(millis - previous)).await;
        previous = millis;
        let phase = if index == 0 {
            TouchPhase::Started
        } else {
            TouchPhase::Moved
        };
        send(window, cx, phase, (x, y)).await;
    }
    let last = samples.last().expect("samples").1;
    send(window, cx, TouchPhase::Ended, (x, last)).await;
    let at_release = offset(window, cx);
    sleep(cx, Duration::from_millis(1500)).await;
    (at_release, offset(window, cx))
}

fn run_example() {
    let interactive = std::env::args().any(|argument| argument == "--interactive");
    application().run(move |cx: &mut App| {
        if !example_support::load_fonts(cx) {
            return;
        }
        let bounds = Bounds::centered(None, size(px(400.), px(500.)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |_, cx| {
                    cx.new(|_| TouchFling {
                        scroll: ScrollHandle::new(),
                    })
                },
            )
            .unwrap();
        cx.activate(true);

        cx.spawn(async move |cx| {
            sleep(cx, Duration::from_millis(500)).await;

            // Upward flick: ~2 px/ms, easing off and wobbling back at the end.
            let flick = [
                (0, 450.),
                (16, 410.),
                (32, 370.),
                (48, 330.),
                (64, 300.),
                (80, 302.),
                (96, 303.),
            ];
            let (at_release, settled) = gesture(window, cx, &flick).await;
            println!(
                "flick: offset at release {:.1}px, after momentum {:.1}px ({})",
                f32::from(at_release),
                f32::from(settled),
                if settled < at_release {
                    "momentum continued in the direction of travel"
                } else {
                    "NO MOMENTUM"
                }
            );

            // Slow drag: 15 px to pass the touch slop, then 0.3 px every 30 ms.
            let mut slow = vec![(0, 450.), (30, 435.)];
            for step in 1..=6u64 {
                slow.push((30 + step * 30, 435. - step as f32 * 0.3));
            }
            let (at_release, settled) = gesture(window, cx, &slow).await;
            println!(
                "slow drag: offset at release {:.1}px, afterwards {:.1}px ({})",
                f32::from(at_release),
                f32::from(settled),
                if settled == at_release {
                    "stopped without a fling"
                } else {
                    "UNEXPECTED FLING"
                }
            );

            if !interactive {
                cx.update(|cx| cx.quit());
            }
        })
        .detach();
    });
}

fn main() {
    run_example();
}
