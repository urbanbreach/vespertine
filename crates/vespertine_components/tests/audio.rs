//! CMP-027 on the framework test contexts and the offscreen host: the animated-geometry clock
//! rules, painted waveform and geometry pixels, and the rolling label.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use vespertine::{IntoElement, ParentElement, Styled, TestAppContext, div, hsla, px, rgb};
use vespertine_components::audio::{
    AnimatedGeometry, Bar, Dot, EnvelopeBuffer, Frame, FrameRequest, FrameSource, InkTheme, Line,
    Mode, Phase, WaveformColors, host_ticked, roll_text, rolling, waveform,
};
use vespertine_test_host::{HostOptions, TestHost, require_backend};

struct Counting {
    calls: Rc<Cell<usize>>,
    stops: Rc<Cell<usize>>,
}

impl FrameSource for Counting {
    fn generate(&mut self, request: &FrameRequest, frame: &mut Frame) {
        self.calls.set(self.calls.get() + 1);
        frame.clear();
        frame.dots.push(Dot {
            x: 10.0 + request.time,
            y: 10.0,
            radius: 3.0,
            ink: 0.0,
            alpha: 1.0,
        });
    }

    fn clock_stopped(&mut self) {
        self.stops.set(self.stops.get() + 1);
    }
}

fn geometry(
    cx: &mut TestAppContext,
) -> (
    vespertine::Entity<AnimatedGeometry>,
    &mut vespertine::VisualTestContext,
    Rc<Cell<usize>>,
    Rc<Cell<usize>>,
) {
    let calls = Rc::new(Cell::new(0));
    let stops = Rc::new(Cell::new(0));
    let (c, s) = (calls.clone(), stops.clone());
    let (view, vcx) = cx.add_window_view(move |window, cx| {
        AnimatedGeometry::new(Counting { calls: c, stops: s }, window, cx)
    });
    vcx.update(|window, _| window.activate_window());
    vcx.update(|window, _| window.refresh());
    vcx.run_until_parked();
    (view, vcx, calls, stops)
}

fn redraw(cx: &mut vespertine::VisualTestContext) {
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

#[vespertine::test]
fn a8_one_timer_while_animating_and_none_otherwise(cx: &mut TestAppContext) {
    let (view, cx, calls, stops) = geometry(cx);
    let timer = |cx: &mut vespertine::VisualTestContext| view.read_with(cx, |v, _| v.has_timer());
    assert!(timer(cx));
    assert!(view.read_with(cx, |v, _| v.is_animating()));
    let first = calls.get();
    assert!(first >= 1);

    // A parent re-render between ticks keeps the pending timer and never reruns the producer.
    view.update(cx, |_, cx| cx.notify());
    redraw(cx);
    redraw(cx);
    assert_eq!(calls.get(), first);
    assert!(timer(cx));

    // A tick marks the frame dirty: the producer runs once and exactly one timer is re-armed.
    cx.executor().advance_clock(Duration::from_millis(40));
    cx.run_until_parked();
    assert_eq!(calls.get(), first + 1);
    assert!(timer(cx));

    // Hidden: the pending timer is dropped immediately and the producer's clock is stopped.
    view.update(cx, |v, cx| v.set_visible(false, cx));
    redraw(cx);
    assert!(!timer(cx));
    assert!(!view.read_with(cx, |v, _| v.is_animating()));
    assert!(stops.get() >= 1);
    cx.executor().advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    assert!(!timer(cx), "no timer may fire while hidden");
    view.update(cx, |v, cx| v.set_visible(true, cx));
    redraw(cx);
    assert!(timer(cx));

    // Paused.
    view.update(cx, |v, cx| v.set_paused(true, cx));
    redraw(cx);
    assert!(!timer(cx));
    view.update(cx, |v, cx| v.set_paused(false, cx));
    redraw(cx);
    assert!(timer(cx));

    // Reduced motion (host flag).
    view.update(cx, |v, cx| v.set_reduced_motion(true, cx));
    redraw(cx);
    assert!(!timer(cx));
    view.update(cx, |v, cx| v.set_reduced_motion(false, cx));
    redraw(cx);
    assert!(timer(cx));

    // An inactive window pauses by default; the flag turns that off.
    cx.deactivate_window();
    redraw(cx);
    assert!(!timer(cx));
    view.update(cx, |v, cx| v.set_pause_when_inactive(false, cx));
    redraw(cx);
    assert!(timer(cx));
    view.update(cx, |v, cx| v.set_pause_when_inactive(true, cx));
    redraw(cx);
    assert!(!timer(cx));
}

#[vespertine::test]
fn a8_fps_values_sanitize(cx: &mut TestAppContext) {
    let (view, cx, _, _) = geometry(cx);
    for (given, expected) in [(0.0, 1.0), (240.0, 30.0), (f32::NAN, 30.0), (12.0, 12.0)] {
        view.update(cx, |v, cx| v.set_target_fps(given, cx));
        assert_eq!(view.read_with(cx, |v, _| v.target_fps()), expected);
    }
}

#[vespertine::test]
fn a8_setters_notify_only_on_change(cx: &mut TestAppContext) {
    let (view, cx, calls, _) = geometry(cx);
    let before = calls.get();
    for _ in 0..3 {
        view.update(cx, |v, cx| {
            v.set_state("idle", cx);
            v.set_size("large", cx);
            v.set_scale(1.0, cx);
            v.set_speed(1.0, cx);
            v.set_audio_levels(0.0, 0.0, cx);
            v.set_theme(InkTheme::Auto, cx);
        });
        redraw(cx);
    }
    // The first round changes state and size (one regeneration); repeats change nothing.
    assert_eq!(calls.get(), before + 1);
    view.update(cx, |v, cx| v.set_audio_levels(0.5, 0.0, cx));
    redraw(cx);
    assert_eq!(calls.get(), before + 2);
}

#[vespertine::test]
fn a10_first_value_never_rolls_and_reduced_motion_shows_plain_text(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| EmptyView);
    cx.update(|window, cx| {
        assert!(rolling(window, cx, "a10-key", "Opus 5.5", false).is_none());
        assert!(rolling(window, cx, "a10-key", "Opus 5.1", false).is_some());
        // Reduced motion mid-roll: the plain label immediately.
        assert!(rolling(window, cx, "a10-key", "Opus 5.1", true).is_none());
        assert!(rolling(window, cx, "a10-key", "Opus 5.1", false).is_none());
        // The system preference reduces motion too.
        assert!(rolling(window, cx, "a10-other", "x", false).is_none());
        cx.set_reduce_motion(true);
        assert!(rolling(window, cx, "a10-other", "y", false).is_none());
        cx.set_reduce_motion(false);
    });
}

struct EmptyView;

impl vespertine::Render for EmptyView {
    fn render(
        &mut self,
        _window: &mut vespertine::Window,
        _cx: &mut vespertine::Context<Self>,
    ) -> impl IntoElement {
        div()
    }
}

fn host(
    size: (f64, f64),
    element: impl Fn(&mut vespertine::Window, &mut vespertine::App) -> vespertine::AnyElement + 'static,
) -> TestHost {
    TestHost::with_element(HostOptions::with_size(size.0, size.1), element).expect("host")
}

fn bar_pixel(image: &image::RgbaImage, scale: f32, x: f32, y: f32) -> [u8; 4] {
    image.get_pixel((x * scale) as u32, (y * scale) as u32).0
}

#[test]
fn waveform_paints_bars_in_one_canvas_and_motion_off_is_static() {
    require_backend!();
    let ink = hsla(0.0, 0.0, 1.0, 1.0);
    let quiet = hsla(0.0, 0.0, 0.4, 1.0);
    let bars: Vec<Bar> = (0..30)
        .map(|i| Bar {
            age: i as f32 + 1.0,
            amplitude: 0.9,
        })
        .collect();
    let draw = move |motion: bool| {
        let bars = bars.clone();
        move |_: &mut vespertine::Window, _: &mut vespertine::App| {
            div()
                .size_full()
                .bg(rgb(0x101114))
                .child(div().w(px(200.0)).h(px(60.0)).child(waveform(
                    bars.clone(),
                    Mode::Live,
                    WaveformColors { ink, quiet },
                    Phase::STEADY,
                    motion,
                )))
                .into_any_element()
        }
    };
    let mut host = host((240.0, 80.0), draw(false));
    host.flush();
    let scale = host.scale_factor();
    let image = host.screenshot_image().unwrap();
    // The newest bar: x = 200 - 3 .. 200, centered vertically in the 60 px canvas, 54 px tall.
    let lit = bar_pixel(&image, scale, 198.5, 30.0);
    assert!(lit[0] > 200, "{lit:?}");
    let gap = bar_pixel(&image, scale, 195.0, 30.0);
    assert!(gap[0] < 40, "{gap:?}");
    // Motion off: a later frame is pixel-identical.
    host.advance_executor_time(900.0).ok();
    host.flush();
    assert_eq!(image.as_raw(), host.screenshot_image().unwrap().as_raw());
}

#[test]
fn geometry_primitives_cull_below_the_alpha_threshold_and_follow_the_ink_direction() {
    require_backend!();
    let buffer = Rc::new(RefCell::new(Frame::new()));
    struct Fixed;
    impl FrameSource for Fixed {
        fn generate(&mut self, _: &FrameRequest, frame: &mut Frame) {
            frame.clear();
            frame.lines.push(Line {
                x1: 10.0,
                y1: 40.0,
                x2: 90.0,
                y2: 40.0,
                width: 4.0,
                ink: 0.2,
                alpha: 1.0,
            });
            frame.dots.push(Dot {
                x: 30.0,
                y: 15.0,
                radius: 6.0,
                ink: 0.0,
                alpha: 1.0,
            });
            // Alpha 0.019 is culled; a dot below r_min uses r_min.
            frame.dots.push(Dot {
                x: 60.0,
                y: 15.0,
                radius: 6.0,
                ink: 0.0,
                alpha: 0.019,
            });
            frame.dots.push(Dot {
                x: 80.0,
                y: 15.0,
                radius: 0.1,
                ink: 0.0,
                alpha: 1.0,
            });
        }
    }
    let draw = move |dark: bool| {
        let buffer = buffer.clone();
        move |_: &mut vespertine::Window, _: &mut vespertine::App| {
            let element = host_ticked(
                &mut Fixed,
                &buffer,
                "idle".into(),
                "small".into(),
                0.0,
                false,
                1.0,
                3.0,
                dark,
            );
            div()
                .size_full()
                .bg(rgb(0x808080))
                .child(div().w(px(100.0)).h(px(60.0)).child(element))
                .into_any_element()
        }
    };
    let mut light = host((120.0, 80.0), draw(false));
    light.flush();
    let scale = light.scale_factor();
    let image = light.screenshot_image().unwrap();
    // Ink 0.2 on light themes: lightness 0.2 (dark pixels over the 0x80 background).
    let line = bar_pixel(&image, scale, 50.0, 40.0);
    assert!(line[0] < 0x60, "{line:?}");
    let dot = bar_pixel(&image, scale, 30.0, 15.0);
    assert!(dot[0] < 0x20, "{dot:?}");
    // The culled dot paints nothing; the tiny dot is raised to r_min = 3 px.
    let culled = bar_pixel(&image, scale, 60.0, 15.0);
    assert!(culled[0].abs_diff(0x80) < 6, "{culled:?}");
    let small = bar_pixel(&image, scale, 82.5, 15.0);
    assert!(small[0] < 0x20, "{small:?}");
    let mut dark = host((120.0, 80.0), draw(true));
    dark.flush();
    let image = dark.screenshot_image().unwrap();
    // Dark themes use 1 - w: the line becomes light.
    let line = bar_pixel(&image, scale, 50.0, 40.0);
    assert!(line[0] > 0xC0, "{line:?}");
}

#[test]
fn rolling_label_settles_to_the_plain_label() {
    require_backend!();
    let mut host = host((200.0, 40.0), |window, cx| {
        let label = roll_text(window, cx, "settle-key", "Opus 5.5", false);
        div()
            .size_full()
            .bg(rgb(0x101114))
            .text_color(rgb(0xFFFFFF))
            .child(label)
            .into_any_element()
    });
    host.flush();
    assert!(host.painted_text().contains(&"Opus 5.5".to_owned()));
    // Observing a changed value starts a roll, which no longer paints the plain string; after
    // the total duration the plain label is back with the new text.
    let mut host = host_with_changing_label();
    host.flush();
    assert!(host.painted_text().contains(&"before".to_owned()));
    CHANGED.with(|changed| changed.set(true));
    host.app().update(|cx| cx.refresh_windows());
    host.flush();
    let mid = host.painted_text();
    assert!(
        !mid.contains(&"after".to_owned()) && !mid.contains(&"before".to_owned()),
        "a running roll paints glyphs, not the plain label: {mid:?}"
    );
    host.advance_executor_time(900.0).ok();
    host.app().update(|cx| cx.refresh_windows());
    host.flush();
    assert!(
        host.painted_text().contains(&"after".to_owned()),
        "{:?}",
        host.painted_text()
    );
}

thread_local! {
    static CHANGED: Cell<bool> = const { Cell::new(false) };
}

fn host_with_changing_label() -> TestHost {
    host((200.0, 40.0), |window, cx| {
        let text = if CHANGED.with(Cell::get) {
            "after"
        } else {
            "before"
        };
        let label = roll_text(window, cx, "changing-key", text, false);
        div()
            .size_full()
            .bg(rgb(0x101114))
            .text_color(rgb(0xFFFFFF))
            .child(label)
            .into_any_element()
    })
}

#[test]
fn envelope_example_values_drive_the_waveform_input() {
    // The deterministic samples of the example: irregular polls fill consecutive slots.
    let base = Instant::now();
    let mut buffer = EnvelopeBuffer::new();
    buffer.start(base);
    let mut stored = 0;
    for step in 0..50u64 {
        let raw = 0.01 + 0.2 * ((step % 7) as f32 / 7.0);
        if buffer.record(base + Duration::from_millis(step * 37), raw) {
            stored += 1;
        }
    }
    assert!(stored > 20);
    let bars = buffer.bars(base + Duration::from_millis(50 * 37 + 100), true);
    assert!(!bars.is_empty());
    assert!(bars.iter().all(|bar| (0.0..=1.0).contains(&bar.amplitude)));
}
