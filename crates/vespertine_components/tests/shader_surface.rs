//! CMP-026 at the surface level: A2 (invalid program keeps the previous frame), A3 (replacement),
//! A5 (removal), A6 (pointer inputs), A9 (reduced motion), R5 scheduling, R7 camera defaults.

mod media_support;

use media_support::solid_image;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use vespertine::{
    Context, ElementId, ImageSource, IntoElement, Modifiers, MouseButton, ParentElement, Render,
    RenderImage, Styled, TestAppContext, VisualTestContext, Window, WindowId, div, img, point, px,
};
use vespertine_components::scene3d::{
    Camera, Delivery, Discard, Dispatch, RenderRequest, RenderResult, SceneFrame, SceneSurfaces,
    SurfaceHandle,
};
use vespertine_components::shader::inputs::mouse_table;
use vespertine_components::shader::{ShaderSurface, gpu_available};

const GOOD: &str = "void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    fragColor = vec4(1.0, 0.0, 0.0, 1.0);
}";
const OTHER: &str = "void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    fragColor = vec4(0.0, 0.0, 1.0, 1.0);
}";

#[derive(Clone)]
struct Options {
    program: &'static str,
    buffers: Vec<&'static str>,
    animate: bool,
    show: bool,
    dispatch: Dispatch,
    extra_image: Option<Arc<RenderImage>>,
    errors: Arc<Mutex<Vec<String>>>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            program: GOOD,
            buffers: Vec::new(),
            animate: false,
            show: true,
            dispatch: Dispatch::Manual,
            extra_image: None,
            errors: Arc::default(),
        }
    }
}

struct Host {
    options: Rc<RefCell<Options>>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let o = self.options.borrow().clone();
        let mut root = div().size_full();
        if let Some(image) = o.extra_image {
            root = root.child(img(ImageSource::Render(image)).w(px(8.)).h(px(8.)));
        }
        if o.show {
            let errors = o.errors.clone();
            root = root.child(
                div().w(px(300.)).child(
                    ShaderSurface::new("shader", o.program)
                        .buffers(o.buffers.clone())
                        .animate(o.animate)
                        .height(px(120.))
                        .dispatch(o.dispatch)
                        .on_error(move |message| errors.lock().unwrap().push(message.to_owned())),
                ),
            );
        }
        root
    }
}

struct Fixture {
    options: Rc<RefCell<Options>>,
    window: WindowId,
}

fn setup(cx: &mut TestAppContext, options: Options) -> (Fixture, &mut VisualTestContext) {
    cx.executor().allow_parking();
    let options = Rc::new(RefCell::new(options));
    let host_options = options.clone();
    let (_, vcx) = cx.add_window_view(move |_, _| Host {
        options: host_options,
    });
    let window = vcx.update(|window, _| window.window_handle().window_id());
    redraw(vcx);
    (Fixture { options, window }, vcx)
}

fn redraw(cx: &mut VisualTestContext) {
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn handle(fixture: &Fixture, cx: &mut VisualTestContext) -> SurfaceHandle {
    cx.update(|_, app| SceneSurfaces::get(app, fixture.window, &ElementId::from("shader")))
        .expect("surface")
}

fn deliver(
    handle: &SurfaceHandle,
    request: &RenderRequest,
    cx: &mut VisualTestContext,
) -> Delivery<SceneFrame> {
    let (w, h) = request.key.pixel_size();
    let frame = SceneFrame {
        image: solid_image(w.max(1), h.max(1)),
        annotations: Vec::new(),
        labels: Vec::new(),
    };
    let result = RenderResult::success(request, frame, Duration::from_millis(1));
    cx.update(|_, app| handle.deliver(result, app))
}

#[vespertine::test]
fn shader_surfaces_default_to_a_camera_of_0_0_1_with_orbit_disabled(cx: &mut TestAppContext) {
    let (fixture, vcx) = setup(cx, Options::default());
    let handle = handle(&fixture, vcx);
    assert_eq!(handle.camera(), Camera::new(0.0, 0.0, 1.0));
    let none = Modifiers::none();
    // Dragging does not orbit (orbit defaults to off) but the pointer is still tracked.
    vcx.simulate_mouse_down(point(px(50.), px(50.)), MouseButton::Left, none);
    vcx.simulate_mouse_move(point(px(120.), px(80.)), MouseButton::Left, none);
    assert_eq!(handle.camera(), Camera::new(0.0, 0.0, 1.0));
    vcx.simulate_mouse_up(point(px(120.), px(80.)), MouseButton::Left, none);
}

#[vespertine::test]
fn a6_the_surface_tracks_the_pointer_and_the_press_origin_across_the_capture(
    cx: &mut TestAppContext,
) {
    let (fixture, vcx) = setup(cx, Options::default());
    let handle = handle(&fixture, vcx);
    let none = Modifiers::none();
    assert!(handle.pointer().is_none() && handle.press_origin().is_none());
    vcx.simulate_mouse_move(point(px(10.), px(20.)), None, none);
    assert_eq!(handle.pointer(), Some(point(px(10.), px(20.))));
    assert!(handle.press_origin().is_none());
    vcx.simulate_mouse_down(point(px(10.), px(20.)), MouseButton::Left, none);
    assert_eq!(handle.press_origin(), Some(point(px(10.), px(20.))));
    // Moves outside the surface keep reporting during the press (the pointer is captured).
    vcx.simulate_mouse_move(point(px(700.), px(400.)), MouseButton::Left, none);
    assert_eq!(handle.pointer(), Some(point(px(700.), px(400.))));
    assert_eq!(handle.press_origin(), Some(point(px(10.), px(20.))));
    // The press origin is cleared on release anywhere.
    vcx.simulate_mouse_up(point(px(700.), px(400.)), MouseButton::Left, none);
    assert!(handle.press_origin().is_none());
    // Not hovered: the pointer is cleared.
    vcx.simulate_mouse_move(point(px(700.), px(400.)), None, none);
    assert!(handle.pointer().is_none());
    // The request carries them; the mouse table follows R3 with the y axis from the bottom.
    redraw(vcx);
    let first = handle.take_requests();
    if let Some(first) = first.first() {
        deliver(&handle, first, vcx);
    }
    vcx.simulate_mouse_move(point(px(10.), px(20.)), None, none);
    vcx.simulate_mouse_down(point(px(10.), px(20.)), MouseButton::Left, none);
    redraw(vcx);
    let requests = handle.take_requests();
    let last = requests.last().expect("a request");
    assert_eq!(last.pointer, Some(point(px(10.), px(20.))));
    assert_eq!(last.press, Some(point(px(10.), px(20.))));
    let (w, h) = last.key.pixel_size();
    assert_eq!(
        mouse_table(last.pointer, last.press, h as f32, last.key.scale),
        [
            10.0 * last.key.scale,
            h as f32 - 20.0 * last.key.scale,
            10.0 * last.key.scale,
            h as f32 - 20.0 * last.key.scale
        ]
    );
    assert!(w > 0);
}

#[vespertine::test]
fn r5_a_changed_pointer_triggers_a_render_and_unchanged_inputs_do_not(cx: &mut TestAppContext) {
    let (fixture, vcx) = setup(cx, Options::default());
    let handle = handle(&fixture, vcx);
    let first = handle.take_requests();
    assert_eq!(first.len(), 1);
    deliver(&handle, &first[0], vcx);
    redraw(vcx);
    assert!(
        handle.take_requests().is_empty(),
        "nothing changed: nothing is rendered"
    );
    vcx.simulate_mouse_move(point(px(40.), px(40.)), None, Modifiers::none());
    redraw(vcx);
    assert_eq!(handle.take_requests().len(), 1, "the mouse changed");
}

#[vespertine::test]
fn a9_animation_frames_follow_animate_feedback_and_reduced_motion(cx: &mut TestAppContext) {
    // Idle: neither animated nor buffers: no frames.
    let (fixture, vcx) = setup(cx, Options::default());
    let idle = handle(&fixture, vcx);
    for _ in 0..3 {
        redraw(vcx);
    }
    assert_eq!(idle.animation_frames(), 0);

    // Animated: frames while motion is permitted; none under reduced motion.
    let (fixture, vcx) = setup(
        cx,
        Options {
            animate: true,
            ..Options::default()
        },
    );
    let animated = handle(&fixture, vcx);
    redraw(vcx);
    assert!(animated.animation_frames() > 0);
    vcx.update(|_, app| app.set_reduce_motion(true));
    let frames = animated.animation_frames();
    for _ in 0..3 {
        redraw(vcx);
    }
    assert_eq!(
        animated.animation_frames(),
        frames,
        "reduced motion stops animation frames"
    );
    vcx.update(|_, app| app.set_reduce_motion(false));

    // A surface with feedback buffers is always changed while motion is permitted.
    let (fixture, vcx) = setup(
        cx,
        Options {
            buffers: vec![GOOD],
            ..Options::default()
        },
    );
    let feedback = handle(&fixture, vcx);
    let first = feedback.take_requests();
    deliver(&feedback, &first[0], vcx);
    redraw(vcx);
    redraw(vcx);
    assert!(feedback.animation_frames() > 0);
    let forced = feedback.take_requests();
    assert!(
        !forced.is_empty(),
        "a new frame is started although no input moved"
    );
    for request in &forced {
        deliver(&feedback, request, vcx);
    }
    vcx.update(|_, app| app.set_reduce_motion(true));
    let frames = feedback.animation_frames();
    redraw(vcx);
    redraw(vcx);
    assert_eq!(feedback.animation_frames(), frames);
    // A static frame is still rendered once and on every input change.
    let pending = feedback.take_requests();
    assert!(pending.len() <= 1);
    for request in &pending {
        deliver(&feedback, request, vcx);
    }
    redraw(vcx);
    // The static frame is rendered once (its key no longer includes time), then nothing.
    for request in &feedback.take_requests() {
        deliver(&feedback, request, vcx);
    }
    redraw(vcx);
    assert!(
        feedback.take_requests().is_empty(),
        "nothing forces frames any more"
    );
    vcx.simulate_mouse_move(point(px(30.), px(30.)), None, Modifiers::none());
    redraw(vcx);
    assert_eq!(
        feedback.take_requests().len(),
        1,
        "an input change still renders"
    );
}

#[vespertine::test]
fn a3_changing_the_program_replaces_in_place_the_old_frame_stays_and_old_results_are_discarded(
    cx: &mut TestAppContext,
) {
    let (fixture, vcx) = setup(cx, Options::default());
    let handle = handle(&fixture, vcx);
    let request = handle.take_requests().remove(0);
    deliver(&handle, &request, vcx);
    let shown = handle.displayed_image().expect("a frame");
    // Retained targets (feedback state) exist.
    handle.targets().with((0, 0), || 7u32, |_| ());
    assert!(handle.targets().is_allocated());
    // Another request is in flight when the program changes.
    vcx.simulate_mouse_move(point(px(40.), px(40.)), None, Modifiers::none());
    redraw(vcx);
    let in_flight = handle.take_requests().remove(0);

    fixture.options.borrow_mut().program = OTHER;
    redraw(vcx);
    assert!(
        Arc::ptr_eq(&handle.displayed_image().unwrap(), &shown),
        "the previous frame stays visible until the first frame of the new program lands"
    );
    assert!(!handle.targets().is_allocated(), "feedback state was reset");
    // The late result of the old program is discarded.
    let late = deliver(&handle, &in_flight, vcx);
    assert!(matches!(
        late,
        Delivery::Discarded {
            reason: Discard::Replaced,
            ..
        }
    ));
    assert!(Arc::ptr_eq(&handle.displayed_image().unwrap(), &shown));
    // The new program's request lands and replaces the frame.
    let fresh = handle.take_requests();
    assert_eq!(fresh.len(), 1, "one request for the new program");
    assert!(deliver(&handle, &fresh[0], vcx).is_displayed());
    assert!(!Arc::ptr_eq(&handle.displayed_image().unwrap(), &shown));
}

#[vespertine::test]
fn a5_removing_the_surface_discards_the_pending_result_and_releases_every_image(
    cx: &mut TestAppContext,
) {
    let late_image = solid_image(12, 12);
    let (fixture, vcx) = setup(
        cx,
        Options {
            extra_image: Some(late_image.clone()),
            ..Options::default()
        },
    );
    let handle = handle(&fixture, vcx);
    let request = handle.take_requests().remove(0);
    deliver(&handle, &request, vcx);
    let shown = handle.displayed_image().unwrap();
    vcx.simulate_mouse_move(point(px(40.), px(40.)), None, Modifiers::none());
    redraw(vcx);
    let pending = handle.take_requests().remove(0);
    vcx.update(|window, _| {
        assert!(window.has_image_atlas_entry(&shown));
        assert!(window.has_image_atlas_entry(&late_image));
    });

    fixture.options.borrow_mut().show = false;
    redraw(vcx);
    redraw(vcx);
    assert!(handle.is_removed());
    vcx.update(|window, _| {
        assert!(
            !window.has_image_atlas_entry(&shown),
            "the displayed image was released"
        )
    });
    let result = RenderResult::success(
        &pending,
        SceneFrame {
            image: late_image.clone(),
            annotations: Vec::new(),
            labels: Vec::new(),
        },
        Duration::ZERO,
    );
    let outcome = vcx.update(|_, app| handle.deliver(result, app));
    assert!(matches!(
        outcome,
        Delivery::Discarded {
            reason: Discard::Removed,
            ..
        }
    ));
    vcx.run_until_parked();
    vcx.update(|window, _| {
        assert!(
            !window.has_image_atlas_entry(&late_image),
            "released, never displayed"
        )
    });
}

#[vespertine::test]
fn a2_an_invalid_program_reports_a_line_numbered_error_and_keeps_the_previous_frame(
    cx: &mut TestAppContext,
) {
    if !gpu_available() {
        eprintln!("skipping: no GPU adapter");
        return;
    }
    let errors: Arc<Mutex<Vec<String>>> = Arc::default();
    let (fixture, vcx) = setup(
        cx,
        Options {
            dispatch: Dispatch::Background,
            errors: errors.clone(),
            ..Options::default()
        },
    );
    let handle = handle(&fixture, vcx);
    let wait = |vcx: &mut VisualTestContext, condition: &dyn Fn() -> bool| {
        for _ in 0..200 {
            vcx.run_until_parked();
            if condition() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(25));
            redraw(vcx);
        }
        false
    };
    assert!(
        wait(vcx, &|| handle.displayed_image().is_some()),
        "the good program lands"
    );
    let shown = handle.displayed_image().unwrap();

    fixture.options.borrow_mut().program = "void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    float a = 1.0;
    fragColor = vec4(undefined_name, a, 0.0, 1.0);
}";
    redraw(vcx);
    assert!(
        wait(vcx, &|| handle.last_error().is_some()),
        "the error is reported"
    );
    let message = handle.last_error().unwrap();
    assert!(message.starts_with("line 3:"), "{message}");
    assert_eq!(
        errors.lock().unwrap().last().map(String::as_str),
        Some(message.as_str())
    );
    assert!(
        Arc::ptr_eq(&handle.displayed_image().unwrap(), &shown),
        "the previous good frame stays"
    );

    // Fixing the program clears the error with the next good frame.
    fixture.options.borrow_mut().program = OTHER;
    redraw(vcx);
    assert!(wait(vcx, &|| handle.last_error().is_none()
        && !Arc::ptr_eq(&handle.displayed_image().unwrap(), &shown)));
    let _ = Cell::new(0);
}
