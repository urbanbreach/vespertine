//! CMP-025 A1 to A9.

mod media_support;

use media_support::solid_image;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use vespertine::{
    Context, ElementId, ImageSource, IntoElement, Modifiers, MouseButton, ParentElement,
    PinchEvent, Pixels, Point, Render, RenderImage, ScrollDelta, ScrollWheelEvent, Styled,
    TestAppContext, TouchPhase, VisualTestContext, Window, WindowId, div, img, point, px,
};
use vespertine_components::scene3d::{
    Annotation, Camera, Delivery, Discard, Dispatch, OrbitController, Palette, Quality,
    RenderRequest, RenderResult, RendererKind, SceneFrame, SceneProducer, SceneSurface,
    SceneSurfaces, SurfaceHandle, hovered_annotation, select_quality,
};

/// A producer whose frames are solid images of the requested pixel size.
struct TestProducer {
    renders: AtomicUsize,
    annotations: Mutex<Vec<Annotation>>,
    errors: Mutex<Vec<String>>,
}

impl TestProducer {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            renders: AtomicUsize::new(0),
            annotations: Mutex::new(Vec::new()),
            errors: Mutex::new(Vec::new()),
        })
    }

    fn frame(&self, request: &RenderRequest) -> SceneFrame {
        let (w, h) = request.key.pixel_size();
        SceneFrame {
            image: solid_image(w.max(1), h.max(1)),
            annotations: self.annotations.lock().unwrap().clone(),
            labels: Vec::new(),
        }
    }
}

impl SceneProducer for TestProducer {
    fn render(&self, request: &RenderRequest) -> Result<SceneFrame, String> {
        self.renders.fetch_add(1, Ordering::SeqCst);
        Ok(self.frame(request))
    }

    fn report_error(&self, message: &str) {
        self.errors.lock().unwrap().push(message.to_owned());
    }
}

#[derive(Clone)]
struct Options {
    show: bool,
    width: f32,
    height: f32,
    identity: u64,
    spin: f32,
    animate: bool,
    max_in_flight: usize,
    renderer: RendererKind,
    palette: Palette,
    extra_image: Option<Arc<RenderImage>>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            show: true,
            width: 400.0,
            height: 300.0,
            identity: 0,
            spin: 0.0,
            animate: false,
            max_in_flight: 1,
            renderer: RendererKind::Software,
            palette: Palette::neutral(),
            extra_image: None,
        }
    }
}

struct Host {
    options: Rc<RefCell<Options>>,
    producer: Arc<TestProducer>,
    renders: Rc<Cell<usize>>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        let o = self.options.borrow().clone();
        let mut root = div().size_full();
        if let Some(image) = o.extra_image {
            root = root.child(img(ImageSource::Render(image)).w(px(8.)).h(px(8.)));
        }
        if o.show {
            root = root.child(
                div().w(px(o.width)).child(
                    SceneSurface::from_producer("cube", self.producer.clone())
                        .height(px(o.height))
                        .identity(o.identity)
                        .spin(o.spin)
                        .animate(o.animate)
                        .max_in_flight(o.max_in_flight)
                        .renderer(o.renderer)
                        .palette(o.palette)
                        .dispatch(Dispatch::Manual),
                ),
            );
        }
        root
    }
}

struct Fixture {
    options: Rc<RefCell<Options>>,
    producer: Arc<TestProducer>,
    window: WindowId,
}

fn key() -> ElementId {
    ElementId::from("cube")
}

fn setup(cx: &mut TestAppContext, options: Options) -> (Fixture, &mut VisualTestContext) {
    let options = Rc::new(RefCell::new(options));
    let producer = TestProducer::new();
    let (host_options, host_producer) = (options.clone(), producer.clone());
    let (_, vcx) = cx.add_window_view(move |_, _| Host {
        options: host_options,
        producer: host_producer,
        renders: Rc::new(Cell::new(0)),
    });
    let window = vcx.update(|window, _| window.window_handle().window_id());
    redraw(vcx);
    (
        Fixture {
            options,
            producer,
            window,
        },
        vcx,
    )
}

fn redraw(cx: &mut VisualTestContext) {
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn handle(fixture: &Fixture, cx: &mut VisualTestContext) -> SurfaceHandle {
    cx.update(|_, app| SceneSurfaces::get(app, fixture.window, &key()))
        .expect("surface")
}

/// Renders every queued request with the producer and delivers the results in order.
fn serve(
    fixture: &Fixture,
    handle: &SurfaceHandle,
    cx: &mut VisualTestContext,
) -> Vec<Delivery<SceneFrame>> {
    let mut outcomes = Vec::new();
    for request in handle.take_requests() {
        let frame = fixture.producer.frame(&request);
        let result = RenderResult::success(&request, frame, Duration::from_millis(1));
        outcomes.push(cx.update(|_, app| handle.deliver(result, app)));
    }
    redraw(cx);
    outcomes
}

fn wheel(cx: &mut VisualTestContext, at: Point<Pixels>, lines: f32) {
    cx.simulate_event(ScrollWheelEvent {
        position: at,
        delta: ScrollDelta::Lines(point(0., lines)),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
    });
}

#[vespertine::test]
fn a3_drag_sets_yaw_and_pitch_from_the_press_position(cx: &mut TestAppContext) {
    let (fixture, vcx) = setup(cx, Options::default());
    let handle = handle(&fixture, vcx);
    assert_eq!(handle.camera(), Camera::default());
    let none = Modifiers::none();
    vcx.simulate_mouse_down(point(px(100.), px(100.)), MouseButton::Left, none);
    // The pointer is captured: moves outside the surface still orbit.
    vcx.simulate_mouse_move(point(px(200.), px(140.)), MouseButton::Left, none);
    vcx.simulate_mouse_up(point(px(200.), px(140.)), MouseButton::Left, none);
    let camera = handle.camera();
    assert!((camera.yaw - (0.6 - 1.0)).abs() < 1e-5, "{camera:?}");
    assert!((camera.pitch - (0.45 + 0.4)).abs() < 1e-5, "{camera:?}");
    assert_eq!(camera.zoom, 1.0);

    // 500 px down clamps the pitch at 1.5.
    vcx.simulate_mouse_down(point(px(100.), px(100.)), MouseButton::Left, none);
    vcx.simulate_mouse_move(point(px(100.), px(600.)), MouseButton::Left, none);
    assert_eq!(handle.camera().pitch, 1.5);
    vcx.simulate_mouse_move(point(px(100.), px(-600.)), MouseButton::Left, none);
    assert_eq!(handle.camera().pitch, -1.5);
    vcx.simulate_mouse_up(point(px(100.), px(-600.)), MouseButton::Left, none);
    // After the release moves no longer orbit.
    let after = handle.camera();
    vcx.simulate_mouse_move(point(px(300.), px(10.)), None, none);
    assert_eq!(handle.camera(), after);
}

#[vespertine::test]
fn a4_wheel_notches_zoom_multiplicatively_and_clamp(cx: &mut TestAppContext) {
    let (fixture, vcx) = setup(cx, Options::default());
    let handle = handle(&fixture, vcx);
    let at = point(px(100.), px(100.));
    wheel(vcx, at, 1.0);
    assert!((handle.camera().zoom - 1.1).abs() < 1e-5);
    wheel(vcx, at, -1.0);
    assert!((handle.camera().zoom - 1.0).abs() < 1e-5);
    for _ in 0..30 {
        wheel(vcx, at, 1.0);
    }
    assert_eq!(handle.camera().zoom, 5.0);
    for _ in 0..70 {
        wheel(vcx, at, -1.0);
    }
    assert_eq!(handle.camera().zoom, 0.2);
    // The camera orbit and zoom survive re-renders.
    redraw(vcx);
    assert_eq!(handle.camera().zoom, 0.2);
    // A pinch scales the zoom by its cumulative factor.
    for (phase, delta) in [
        (TouchPhase::Started, 0.0),
        (TouchPhase::Moved, 1.0),
        (TouchPhase::Moved, 1.0),
        (TouchPhase::Ended, 0.0),
    ] {
        vcx.simulate_event(PinchEvent {
            position: at,
            delta,
            modifiers: Modifiers::none(),
            phase,
        });
    }
    assert!((handle.camera().zoom - 0.8).abs() < 1e-4, "0.2 x 2 x 2");
}

#[test]
fn a4_wheel_pixel_deltas_count_one_notch_per_120_units() {
    let now = Instant::now();
    let mut controller = OrbitController::new(Camera::default(), now);
    controller.wheel(vespertine_components::scene3d::camera::wheel_notches(
        ScrollDelta::Pixels(point(px(0.), px(120.))),
    ));
    assert!((controller.stored().zoom - 1.1).abs() < 1e-6);
    controller.set_zoom_step(2.0);
    controller.wheel(1.0);
    assert!((controller.stored().zoom - 2.2).abs() < 1e-5);
}

fn ann(x: f32, y: f32, radius: f32, text: &str) -> Annotation {
    Annotation {
        position: point(px(x), px(y)),
        radius,
        text: text.to_owned().into(),
    }
}

#[test]
fn a5_hover_tolerance_is_radius_plus_six_and_the_nearest_wins() {
    let pointer = Some(point(px(100.), px(100.)));
    let radius = 5.0;
    let at_limit = [ann(100. + radius + 6.0, 100., radius, "a")];
    assert_eq!(hovered_annotation(&at_limit, pointer), Some(0));
    let beyond = [ann(100. + radius + 6.5, 100., radius, "a")];
    assert_eq!(hovered_annotation(&beyond, pointer), None);
    // Two qualifying annotations: the nearer wins; a tie resolves to the earliest.
    let two = [ann(108., 100., 5., "far"), ann(103., 100., 5., "near")];
    assert_eq!(hovered_annotation(&two, pointer), Some(1));
    let tie = [ann(95., 100., 5., "first"), ann(105., 100., 5., "second")];
    assert_eq!(hovered_annotation(&tie, pointer), Some(0));
    assert_eq!(hovered_annotation(&two, None), None);
}

#[vespertine::test]
fn a5_the_hovered_annotation_follows_the_pointer_and_clears_on_leave(cx: &mut TestAppContext) {
    let (fixture, vcx) = setup(cx, Options::default());
    fixture
        .producer
        .annotations
        .lock()
        .unwrap()
        .push(ann(100., 100., 4., "vertex"));
    let handle = handle(&fixture, vcx);
    redraw(vcx);
    serve(&fixture, &handle, vcx);
    let none = Modifiers::none();
    vcx.simulate_mouse_move(point(px(110.), px(100.)), None, none);
    assert_eq!(
        handle.hovered().map(|a| a.text.to_string()),
        Some("vertex".to_owned())
    );
    vcx.simulate_mouse_move(point(px(110.5), px(100.)), None, none);
    assert!(handle.hovered().is_none());
    vcx.simulate_mouse_move(point(px(100.), px(100.)), None, none);
    assert!(handle.hovered().is_some());
    // Outside the surface the pointer is cleared.
    vcx.simulate_mouse_move(point(px(700.), px(100.)), None, none);
    assert!(handle.pointer().is_none() && handle.hovered().is_none());
}

#[vespertine::test]
fn a6_each_input_changes_the_key_and_an_older_frame_is_shown_but_not_current(
    cx: &mut TestAppContext,
) {
    let (fixture, vcx) = setup(cx, Options::default());
    let handle = handle(&fixture, vcx);
    assert!(handle.displayed_key().is_none());
    let outcomes = serve(&fixture, &handle, vcx);
    assert!(outcomes.iter().all(Delivery::is_displayed));
    let base = handle.displayed_key().expect("first frame");
    assert!(handle.displayed_image().is_some());
    assert!(
        handle.take_requests().is_empty(),
        "the displayed frame is current"
    );

    let changes: Vec<(
        &str,
        Box<dyn Fn(&Fixture, &SurfaceHandle, &mut VisualTestContext)>,
    )> = vec![
        (
            "yaw",
            Box::new(|_, _, cx| {
                let none = Modifiers::none();
                cx.simulate_mouse_down(point(px(100.), px(100.)), MouseButton::Left, none);
                cx.simulate_mouse_move(point(px(120.), px(100.)), MouseButton::Left, none);
                cx.simulate_mouse_up(point(px(120.), px(100.)), MouseButton::Left, none);
            }),
        ),
        (
            "pitch",
            Box::new(|_, _, cx| {
                let none = Modifiers::none();
                cx.simulate_mouse_down(point(px(100.), px(100.)), MouseButton::Left, none);
                cx.simulate_mouse_move(point(px(100.), px(130.)), MouseButton::Left, none);
                cx.simulate_mouse_up(point(px(100.), px(130.)), MouseButton::Left, none);
            }),
        ),
        (
            "zoom",
            Box::new(|_, _, cx| wheel(cx, point(px(50.), px(50.)), 1.0)),
        ),
        (
            "size",
            Box::new(|f, _, cx| {
                f.options.borrow_mut().width = 350.0;
                redraw(cx);
            }),
        ),
        (
            "height",
            Box::new(|f, _, cx| {
                f.options.borrow_mut().height = 280.0;
                redraw(cx);
            }),
        ),
        (
            "scale",
            Box::new(|_, _, cx| {
                cx.simulate_scale_factor_change(3.0);
                redraw(cx);
            }),
        ),
        (
            "palette",
            Box::new(|f, _, cx| {
                f.options.borrow_mut().palette.accent = vespertine::rgb(0x123456).into();
                redraw(cx);
            }),
        ),
        (
            "generation",
            Box::new(|_, h, cx| {
                cx.update(|_, app| h.bump_generation(app));
                redraw(cx);
            }),
        ),
    ];
    let mut previous = base;
    for (name, change) in changes {
        // Bring the displayed frame up to date, then change one input.
        serve(&fixture, &handle, vcx);
        let current = handle.displayed_key().expect("frame");
        assert!(handle.take_requests().is_empty(), "{name}: settled");
        change(&fixture, &handle, vcx);
        redraw(vcx);
        let requests = handle.take_requests();
        assert_eq!(requests.len(), 1, "{name}: one request for the changed key");
        assert_ne!(requests[0].key, current, "{name}: the key changed");
        assert_eq!(
            handle.displayed_key(),
            Some(current),
            "{name}: the older frame stays displayed"
        );
        assert!(handle.displayed_image().is_some());
        // Deliver and move on.
        let request = &requests[0];
        let result = RenderResult::success(
            request,
            fixture.producer.frame(request),
            Duration::from_millis(1),
        );
        vcx.update(|_, app| handle.deliver(result, app));
        redraw(vcx);
        assert_ne!(handle.displayed_key(), Some(previous), "{name}");
        previous = handle.displayed_key().unwrap();
        let _ = &handle.take_requests();
    }
}

#[vespertine::test]
fn a6_quality_is_part_of_the_key(cx: &mut TestAppContext) {
    let (fixture, vcx) = setup(cx, Options::default());
    let handle = handle(&fixture, vcx);
    let settled = serve(&fixture, &handle, vcx);
    assert!(!settled.is_empty());
    let displayed = handle.displayed_key().unwrap();
    assert_eq!(
        displayed.quality,
        Quality::Supersampled,
        "settled software quality"
    );
    // During a drag the software renderer asks for Draft (the last settled render was slow
    // enough: the test delivers it with 1 ms, which is below the 8 ms adaptive threshold, so
    // report a slow settled render first).
    let slow = RenderResult::success(
        &RenderRequest {
            key: displayed,
            sequence: 99,
            identity: 0,
            time: 0.0,
            palette: Palette::neutral(),
            reduce_motion: false,
            pointer: None,
            press: None,
            targets: Default::default(),
        },
        fixture.producer.frame(&handle_request_stub(displayed)),
        Duration::from_millis(20),
    );
    vcx.update(|_, app| handle.deliver(slow, app));
    let none = Modifiers::none();
    vcx.simulate_mouse_down(point(px(100.), px(100.)), MouseButton::Left, none);
    vcx.simulate_mouse_move(point(px(130.), px(100.)), MouseButton::Left, none);
    redraw(vcx);
    let requests = handle.take_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].key.quality,
        Quality::Draft,
        "interacting software rendering is Draft"
    );
    assert_ne!(requests[0].key, displayed, "quality is part of the key");
    vcx.simulate_mouse_up(point(px(130.), px(100.)), MouseButton::Left, none);
}

fn handle_request_stub(key: vespertine_components::scene3d::RenderKey) -> RenderRequest {
    RenderRequest {
        key,
        sequence: 0,
        identity: 0,
        time: 0.0,
        palette: Palette::neutral(),
        reduce_motion: false,
        pointer: None,
        press: None,
        targets: Default::default(),
    }
}

#[vespertine::test]
fn a1_results_delivered_in_reverse_order_never_move_the_displayed_frame_backwards(
    cx: &mut TestAppContext,
) {
    let (fixture, vcx) = setup(
        cx,
        Options {
            max_in_flight: 3,
            ..Options::default()
        },
    );
    let handle = handle(&fixture, vcx);
    let mut requests = handle.take_requests();
    for step in 0..2 {
        wheel(
            vcx,
            point(px(50.), px(50.)),
            if step == 0 { 1.0 } else { 2.0 },
        );
        redraw(vcx);
        requests.extend(handle.take_requests());
    }
    assert_eq!(requests.len(), 3, "three frames in flight");
    assert_eq!(handle.in_flight(), 3);
    let sequences: Vec<u64> = requests.iter().map(|r| r.sequence).collect();
    assert!(
        sequences.windows(2).all(|w| w[0] < w[1]),
        "monotonic: {sequences:?}"
    );
    let mut displayed_sequences = Vec::new();
    for request in requests.iter().rev() {
        let result =
            RenderResult::success(request, fixture.producer.frame(request), Duration::ZERO);
        let delivery = vcx.update(|_, app| handle.deliver(result, app));
        displayed_sequences.push(handle.displayed_sequence());
        if request.sequence != *sequences.last().unwrap() {
            assert!(matches!(
                delivery,
                Delivery::Discarded {
                    reason: Discard::Older,
                    ..
                }
            ));
        }
    }
    assert!(
        displayed_sequences.windows(2).all(|w| w[0] <= w[1]),
        "{displayed_sequences:?}"
    );
    assert_eq!(handle.displayed_sequence(), sequences.last().copied());
    assert_eq!(
        handle.displayed_key(),
        Some(requests.last().unwrap().key),
        "the final frame carries the final key"
    );
    assert_eq!(handle.in_flight(), 0);
    assert!(
        (handle.camera().zoom - 1.1f32 * 1.1 * 1.1).abs() < 1e-4,
        "the camera was retained"
    );
    redraw(vcx);
    assert!(
        handle.take_requests().is_empty(),
        "the final frame is current"
    );
}

#[vespertine::test]
fn a2_removing_the_surface_discards_a_late_result_and_releases_every_image(
    cx: &mut TestAppContext,
) {
    let late_image = solid_image(16, 16);
    let (fixture, vcx) = setup(
        cx,
        Options {
            extra_image: Some(late_image.clone()),
            ..Options::default()
        },
    );
    let handle = handle(&fixture, vcx);
    serve(&fixture, &handle, vcx);
    let shown = handle.displayed_image().expect("displayed");
    vcx.update(|window, _| {
        assert!(
            window.has_image_atlas_entry(&shown),
            "the displayed frame is in the atlas"
        );
        assert!(window.has_image_atlas_entry(&late_image));
    });
    // A new request is in flight when the surface goes away.
    wheel(vcx, point(px(50.), px(50.)), 1.0);
    redraw(vcx);
    let pending = handle.take_requests();
    assert_eq!(pending.len(), 1);
    assert_eq!(handle.in_flight(), 1);

    {
        let mut o = fixture.options.borrow_mut();
        o.show = false;
    }
    redraw(vcx);
    redraw(vcx);
    assert!(
        handle.is_removed(),
        "the framework dropped the surface's state"
    );
    vcx.update(|window, _| {
        assert!(
            !window.has_image_atlas_entry(&shown),
            "the displayed image was released"
        );
    });

    // The late result is discarded and its image released (it sits in the atlas through the
    // extra image element until the delivery evicts it).
    let late = RenderResult::success(
        &pending[0],
        SceneFrame {
            image: late_image.clone(),
            annotations: Vec::new(),
            labels: Vec::new(),
        },
        Duration::ZERO,
    );
    let outcome = vcx.update(|_, app| handle.deliver(late, app));
    assert!(matches!(
        outcome,
        Delivery::Discarded {
            reason: Discard::Removed,
            ..
        }
    ));
    assert!(handle.displayed_image().is_none());
    vcx.run_until_parked();
    // The registry holds weak references: drop the test's own handle before asking it.
    drop(handle);
    vcx.update(|window, app| {
        assert!(
            !window.has_image_atlas_entry(&late_image),
            "the late image was released"
        );
        assert!(SceneSurfaces::get(app, fixture.window, &key()).is_none());
    });
}

#[vespertine::test]
fn a2_replacing_the_identity_releases_the_old_state_and_discards_old_results(
    cx: &mut TestAppContext,
) {
    let (fixture, vcx) = setup(cx, Options::default());
    let old = handle(&fixture, vcx);
    serve(&fixture, &old, vcx);
    let shown = old.displayed_image().unwrap();
    wheel(vcx, point(px(50.), px(50.)), 2.0);
    redraw(vcx);
    let in_flight = old.take_requests();
    assert_eq!(in_flight.len(), 1);

    fixture.options.borrow_mut().identity = 7;
    redraw(vcx);
    redraw(vcx);
    let new = handle(&fixture, vcx);
    assert_eq!(
        new.camera(),
        Camera::default(),
        "camera state is dropped with the old identity"
    );
    assert!(new.displayed_image().is_none());
    vcx.update(|window, _| assert!(!window.has_image_atlas_entry(&shown)));
    // The old identity's result never displays.
    let late = RenderResult::success(
        &in_flight[0],
        fixture.producer.frame(&in_flight[0]),
        Duration::ZERO,
    );
    let outcome = vcx.update(|_, app| new.deliver(late, app));
    assert!(matches!(
        outcome,
        Delivery::Discarded {
            reason: Discard::Replaced,
            ..
        }
    ));
    assert!(new.displayed_image().is_none());
}

#[vespertine::test]
fn a7_reduced_motion_stops_spin_animation_and_idle_surfaces_schedule_nothing(
    cx: &mut TestAppContext,
) {
    // Idle: no spin, no animation.
    let (fixture, vcx) = setup(cx, Options::default());
    let handle_idle = handle(&fixture, vcx);
    serve(&fixture, &handle_idle, vcx);
    for _ in 0..3 {
        redraw(vcx);
    }
    assert_eq!(
        handle_idle.animation_frames(),
        0,
        "an idle surface schedules no frames"
    );

    // A turntable asks for frames while motion is permitted.
    let (fixture, vcx) = setup(
        cx,
        Options {
            spin: 1.0,
            animate: true,
            ..Options::default()
        },
    );
    let spinning = handle(&fixture, vcx);
    redraw(vcx);
    assert!(spinning.animation_frames() > 0);
    let generation = spinning.generation();
    assert!(
        generation > 0,
        "an animating scene is asked for a new drawing at every paint"
    );

    // Reduced motion: no frames, no spin, no new drawings.
    vcx.update(|_, app| app.set_reduce_motion(true));
    let (frames, generation) = (spinning.animation_frames(), spinning.generation());
    for _ in 0..3 {
        redraw(vcx);
    }
    assert_eq!(spinning.animation_frames(), frames);
    assert_eq!(spinning.generation(), generation);
    let later = Instant::now() + Duration::from_secs(10);
    assert_eq!(spinning.camera_at(later, true), spinning.camera());
}

#[test]
fn a7_the_turntable_adds_spin_times_elapsed_and_folds_at_the_press() {
    let t0 = Instant::now();
    let mut c = OrbitController::new(Camera::default(), t0);
    c.set_spin(1.0, t0);
    let t2 = t0 + Duration::from_secs(2);
    assert!((c.displayed(t2, false).yaw - 2.6).abs() < 1e-5);
    assert_eq!(
        c.displayed(t2, true).yaw,
        0.6,
        "reduced motion disables the turntable"
    );
    // Pressing folds the turntable yaw into the stored yaw.
    assert!(c.press(point(px(0.), px(0.)), t2, false));
    assert!((c.stored().yaw - 2.6).abs() < 1e-5);
    // While dragging the turntable is paused.
    assert!((c.displayed(t2 + Duration::from_secs(5), false).yaw - 2.6).abs() < 1e-5);
    // The epoch restarts at the release.
    let t9 = t2 + Duration::from_secs(5);
    assert!(c.release(t9));
    assert!((c.displayed(t9 + Duration::from_secs(1), false).yaw - 3.6).abs() < 1e-5);
    // Spin is clamped to +-3 rad/s.
    c.set_spin(100.0, t9);
    assert_eq!(c.spin(), 3.0);
    c.set_spin(-100.0, t9);
    assert_eq!(c.spin(), -3.0);
    // Orbit disabled: a press does not start a drag.
    c.set_orbit(false);
    assert!(!c.press(point(px(0.), px(0.)), t9, false));
}

#[vespertine::test]
fn a8_a_producer_error_keeps_the_last_good_image_and_the_next_success_clears_it(
    cx: &mut TestAppContext,
) {
    let (fixture, vcx) = setup(cx, Options::default());
    let handle = handle(&fixture, vcx);
    serve(&fixture, &handle, vcx);
    let good = handle.displayed_image().expect("good image");
    assert!(handle.last_error().is_none());

    wheel(vcx, point(px(50.), px(50.)), 1.0);
    redraw(vcx);
    let request = handle.take_requests().remove(0);
    let failure = RenderResult::failure(&request, "gpu lost", Duration::ZERO);
    vcx.update(|_, app| handle.deliver(failure, app));
    assert_eq!(handle.last_error().as_deref(), Some("gpu lost"));
    assert!(
        Arc::ptr_eq(&handle.displayed_image().unwrap(), &good),
        "the last good image stays visible"
    );
    assert_eq!(
        fixture.producer.errors.lock().unwrap().as_slice(),
        ["gpu lost"]
    );
    assert_eq!(handle.in_flight(), 0);

    // A failed key is not retried until something changes; the next successful frame clears
    // the error.
    redraw(vcx);
    assert!(
        handle.take_requests().is_empty(),
        "no request storm after a failure"
    );
    wheel(vcx, point(px(50.), px(50.)), 1.0);
    redraw(vcx);
    let request = handle.take_requests().remove(0);
    let ok = RenderResult::success(&request, fixture.producer.frame(&request), Duration::ZERO);
    vcx.update(|_, app| handle.deliver(ok, app));
    assert!(handle.last_error().is_none());
    assert!(!Arc::ptr_eq(&handle.displayed_image().unwrap(), &good));
}

#[test]
fn a9_quality_selection() {
    use Quality::{Draft, Supersampled};
    let fast = Some(Duration::from_millis(7));
    let slow = Some(Duration::from_millis(8));
    // A GPU renderer always uses the settled quality, interacting or not.
    for interacting in [false, true] {
        for last in [None, fast, slow] {
            assert_eq!(
                select_quality(RendererKind::Gpu, interacting, last, Draft, Supersampled),
                Supersampled
            );
        }
    }
    // The software renderer: Draft while interacting unless the last settled render took < 8 ms.
    let soft = |interacting, last| {
        select_quality(
            RendererKind::Software,
            interacting,
            last,
            Draft,
            Supersampled,
        )
    };
    assert_eq!(soft(true, None), Draft);
    assert_eq!(soft(true, slow), Draft);
    assert_eq!(soft(true, fast), Supersampled);
    assert_eq!(soft(false, None), Supersampled);
    assert_eq!(soft(false, slow), Supersampled);
    // Settled supersampling: 2 x 2 up to device w x h x 4 <= 6,000,000, otherwise one sample.
    assert_eq!(Supersampled.samples_per_pixel((1500, 1000)), 4);
    assert_eq!(Supersampled.samples_per_pixel((1501, 1000)), 1);
    assert_eq!(Draft.samples_per_pixel((10, 10)), 1);
}
