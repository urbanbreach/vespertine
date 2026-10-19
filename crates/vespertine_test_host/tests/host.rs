//! DEV-006: production-path offscreen visual test host.

use serde_json::json;
use std::{cell::RefCell, rc::Rc, time::Duration};
use vespertine::{
    AnyElement, App, AppContext as _, Bounds, Context, InteractiveElement, IntoElement,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Render,
    ShapedLine, Styled, TextAlign, TextRun, UniformListScrollHandle, Window, black, canvas, div,
    point, px, rgb, uniform_list,
};
use vespertine_test_host::{
    HostError, HostEvents, HostOptions, NativeEvent, PaintLog, RetainedHost, TestHost, TreeNode,
    fixture::{FixtureTree, scenario_batch},
    require_backend,
};

type EventLog = Rc<RefCell<Vec<NativeEvent>>>;

fn fixture_host() -> (TestHost, EventLog) {
    let events: EventLog = Rc::default();
    let mut tree = FixtureTree::new();
    tree.apply_batch(&scenario_batch(None).to_string())
        .expect("the scenario batch is valid");
    let sink = events.clone();
    let host = TestHost::with_tree(
        HostOptions {
            event_dispatcher: Some(Box::new(move |event| sink.borrow_mut().push(event))),
            ..HostOptions::with_size(400.0, 300.0)
        },
        tree,
    )
    .expect("host");
    (host, events)
}

fn by_test_id(host: &mut TestHost, test_id: &str) -> TreeNode {
    let mut nodes = host.elements_by_test_id(test_id).expect("tree");
    assert_eq!(nodes.len(), 1, "test id {test_id}");
    nodes.remove(0)
}

fn center(node: &TreeNode) -> (f64, f64) {
    node.bounds
        .unwrap_or_else(|| panic!("element {} was not painted", node.id))
        .center()
}

fn received(node: &TreeNode) -> Vec<String> {
    node.custom_props
        .get("received")
        .and_then(|events| events.as_array())
        .map(|events| {
            events
                .iter()
                .filter_map(|event| event.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn dev_006_a1_drives_retained_tree_by_coordinates_and_reads_painted_frame() {
    require_backend!();
    let (mut host, events) = fixture_host();
    host.flush();

    // Retained state: the serialized tree carries the painted bounds of the last frame.
    assert_eq!(host.element_count().unwrap(), 6 + 10 + 1);
    let root = host.root_element().unwrap().expect("root");
    assert_eq!(root.id, 1);
    assert_eq!(root.children.len(), 6);
    assert!(root.bounds.is_some());
    assert_eq!(
        host.painted_text().first().map(String::as_str),
        Some("Fixture")
    );

    // Click by coordinates through native hit testing: the button's handlers run and produce
    // events that reach the dispatcher.
    let save = by_test_id(&mut host, "save");
    let (x, y) = center(&save);
    host.click(x, y, 0, "").unwrap();
    let kinds: Vec<_> = events
        .borrow()
        .iter()
        .map(|event| (event.element_id, event.event_type.clone()))
        .collect();
    assert_eq!(
        kinds,
        [
            (4, "mousedown".to_owned()),
            (4, "click".to_owned()),
            (4, "mouseup".to_owned())
        ]
    );
    assert_eq!(
        received(&by_test_id(&mut host, "save")),
        ["mousedown", "click", "mouseup"]
    );

    // Type into the input by clicking it and sending keystrokes: the painted strings show it.
    let name = by_test_id(&mut host, "name");
    let (x, y) = center(&name);
    host.click(x, y, 0, "").unwrap();
    host.focus_element(3).unwrap();
    host.simulate_keystrokes("a d a", None).unwrap();
    assert_eq!(
        by_test_id(&mut host, "name").custom_props["value"],
        json!("ada")
    );
    assert!(
        host.painted_text().contains(&"ada".to_owned()),
        "{:?}",
        host.painted_text()
    );
    assert_eq!(host.focused_element(), Some(3));

    // Wheel input scrolls the list (content down = negative delta); the offset is readable.
    let list = by_test_id(&mut host, "list");
    let (x, y) = center(&list);
    assert_eq!(host.scroll_offset(5).unwrap(), Some((0.0, 0.0)));
    host.scroll_wheel(x, y, 0.0, -40.0, "").unwrap();
    assert_eq!(host.scroll_offset(5).unwrap(), Some((0.0, 40.0)));
    assert_eq!(host.scroll_offset(2).unwrap(), None, "text does not scroll");
    host.scroll_to(5, 0.0, 60.0).unwrap();
    assert_eq!(host.scroll_offset(5).unwrap(), Some((0.0, 60.0)));
    assert!(matches!(
        host.scroll_to(2, 0.0, 1.0),
        Err(HostError::NoSuchElement(2))
    ));

    // Drag: held-button moves reach the drag area; the hit list is topmost first.
    let area = by_test_id(&mut host, "drag");
    let (x, y) = center(&area);
    host.mouse_down(x, y, 0, "").unwrap();
    host.mouse_move(x + 10.0, y, Some(0), "").unwrap();
    host.mouse_up(x + 10.0, y, 0, "").unwrap();
    assert_eq!(
        received(&by_test_id(&mut host, "drag")),
        ["mousedown", "mousemove", "mouseup"]
    );
    let hit = host.elements_at(x, y);
    assert_eq!(
        hit.first(),
        Some(&6),
        "the drag area is the topmost element at its center"
    );
    assert!(hit.contains(&1), "the root is under it: {hit:?}");
}

#[test]
fn dev_006_a2_hidden_and_undisplayed_elements_have_no_painted_bounds() {
    require_backend!();
    let (mut host, _) = fixture_host();
    host.apply_batch(
        &json!([
            ["createElement", 30, "text"],
            ["setStyle", 30, {"display": "none"}],
            ["setText", 30, "gone"],
            ["setCustomProp", 30, "testId", "none"],
            ["appendChild", 1, 30],
        ])
        .to_string(),
    )
    .unwrap();
    host.flush();
    // Visibility hidden: laid out (takes space) but never painted, so no bounds.
    assert_eq!(host.element_bounds(7), None);
    // Display none: not laid out and not painted.
    assert_eq!(host.element_bounds(30), None);
    // Visible elements report theirs.
    assert!(host.element_bounds(4).is_some());
    let tree = host.serialize_tree().unwrap().unwrap();
    let mut seen = Vec::new();
    tree.walk(&mut |node| seen.push((node.id, node.bounds.is_some())));
    assert!(seen.contains(&(7, false)) && seen.contains(&(30, false)) && seen.contains(&(4, true)));
    assert!(!host.painted_text().contains(&"ghost".to_owned()));
    assert!(!host.painted_text().contains(&"gone".to_owned()));
}

struct Rows {
    handle: UniformListScrollHandle,
}

impl Render for Rows {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            uniform_list("rows", 100, |range, _window, _cx| {
                range
                    .map(|row| {
                        div()
                            .id(("row", row))
                            .h(px(20.0))
                            .child(
                                canvas(
                                    |_, _, _| (),
                                    move |bounds, _, _, cx| {
                                        PaintLog::record_bounds(cx, row as u64, bounds)
                                    },
                                )
                                .absolute()
                                .size_full(),
                            )
                            .child(format!("row {row}"))
                    })
                    .collect()
            })
            .track_scroll(&self.handle)
            .h(px(100.0)),
        )
    }
}

#[test]
fn dev_006_a2_virtual_list_rows_beyond_the_visible_window_have_no_bounds() {
    require_backend!();
    let mut host = TestHost::with_view(HostOptions::with_size(200.0, 100.0), |_, cx| {
        cx.new(|_| Rows {
            handle: UniformListScrollHandle::new(),
        })
    })
    .unwrap();
    host.flush();
    assert!(host.element_bounds(0).is_some(), "the first row is painted");
    assert!(
        host.element_bounds(4).is_some(),
        "the last visible row is painted"
    );
    assert_eq!(
        host.element_bounds(60),
        None,
        "a row beyond the window has no bounds"
    );
    let strings = host.painted_text();
    assert!(strings.contains(&"row 0".to_owned()));
    assert!(!strings.contains(&"row 60".to_owned()));
}

#[test]
fn dev_006_a3_screenshot_is_the_gpu_frame_at_viewport_times_scale() {
    require_backend!();
    let (mut host, _) = fixture_host();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("frame.png");
    host.screenshot(&path).unwrap();
    let image = image::open(&path).unwrap().to_rgba8();
    let scale = host.scale_factor();
    assert_eq!(scale, 2.0);
    assert_eq!(
        image.dimensions(),
        (800, 600),
        "viewport 400x300 times scale factor 2"
    );

    // The pixels match the painted tree: the button's background at the button, white elsewhere.
    let save = by_test_id(&mut host, "save").bounds.expect("painted");
    let probe = |x: f64, y: f64| image.get_pixel((x * 2.0) as u32, (y * 2.0) as u32).0;
    let blue = probe(save.x + 3.0, save.y + 3.0);
    for (got, want) in blue[..3].iter().zip([0x33u8, 0x55, 0xff]) {
        assert!(got.abs_diff(want) <= 2, "button pixel {blue:?}");
    }
    assert_eq!(
        probe(395.0, 295.0)[..3],
        [0xff, 0xff, 0xff],
        "root background"
    );
    assert_eq!(blue[3], 0xff, "opaque");

    // The accessibility tree of the last frame is structured data.
    let tree = host
        .accessibility_tree()
        .unwrap()
        .expect("accessibility is enabled");
    assert!(
        tree["nodes"]
            .as_array()
            .is_some_and(|nodes| !nodes.is_empty()),
        "{tree}"
    );

    // Executor time advances separately from the frozen motion clock.
    let paused = host.motion_pause();
    let fired = Rc::new(RefCell::new(false));
    host.app().update(|cx| {
        let fired = fired.clone();
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            *fired.borrow_mut() = true;
        })
        .detach();
    });
    host.advance_executor_time(1000.0).unwrap();
    assert!(*fired.borrow(), "the timer due within 1000 ms ran");
    assert_eq!(host.motion_now(), paused, "the motion clock stayed frozen");
}

struct Swatch;

impl Render for Swatch {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().bg(rgb(0x336699))
    }
}

#[test]
fn dev_006_a3_screenshot_honors_the_window_scale_factor() {
    require_backend!();
    let mut host = TestHost::with_view(
        HostOptions {
            scale_factor: 1.0,
            ..HostOptions::with_size(120.0, 60.0)
        },
        |_, cx| cx.new(|_| Swatch),
    )
    .unwrap();
    let image = host.screenshot_image().unwrap();
    assert_eq!(image.dimensions(), (120, 60));
    assert_eq!(image.get_pixel(60, 30).0, [0x33, 0x66, 0x99, 0xff]);
}

#[test]
fn dev_006_a4_missing_backend_is_an_explicit_error_never_a_substitute() {
    // A renderer factory that cannot create a renderer: construction fails explicitly.
    let error = TestHost::with_view(
        HostOptions {
            renderer: Some(Box::new(|| {
                Err(anyhow::anyhow!("no adapter could be created"))
            })),
            ..HostOptions::default()
        },
        |_, cx| cx.new(|_| Swatch),
    )
    .err()
    .expect("construction must fail");
    assert!(
        matches!(&error, HostError::BackendUnavailable(reason) if reason.contains("no adapter could be created")),
        "{error}"
    );

    // A platform with no renderer at all (`None`) is unavailable too, never replaced by a mock.
    let error = TestHost::with_view(
        HostOptions {
            renderer: Some(Box::new(|| Ok(None))),
            ..HostOptions::default()
        },
        |_, cx| cx.new(|_| Swatch),
    )
    .err()
    .expect("construction must fail");
    assert!(matches!(error, HostError::BackendUnavailable(_)), "{error}");

    // The availability boolean is queryable without constructing a host and agrees with
    // construction on this machine.
    let available = vespertine_test_host::is_available();
    assert_eq!(
        available,
        vespertine_test_host::unavailable_reason().is_none()
    );
    match TestHost::new(None, None) {
        Ok(_) => assert!(available, "constructed although reported unavailable"),
        Err(error) => {
            assert!(!available, "reported available but failed: {error}");
            assert!(matches!(error, HostError::BackendUnavailable(_)), "{error}");
        }
    }
}

#[test]
fn dev_006_a5_window_size_validation_and_defaults() {
    require_backend!();
    for bad in [
        0.0,
        -1.0,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        2147483648.0,
        1e300,
    ] {
        for (width, height) in [
            (Some(bad), None),
            (None, Some(bad)),
            (Some(bad), Some(100.0)),
        ] {
            let error = TestHost::new(width, height).err().unwrap_or_else(|| {
                panic!("size {width:?} x {height:?} must be rejected at construction")
            });
            assert!(matches!(error, HostError::InvalidSize { .. }), "{error}");
        }
    }
    let mut host = TestHost::new(None, None).unwrap();
    let viewport = host.viewport();
    assert_eq!((viewport.width, viewport.height), (1280.0, 800.0));
    let mut host = TestHost::new(Some(320.0), Some(200.0)).unwrap();
    let viewport = host.viewport();
    assert_eq!((viewport.width, viewport.height), (320.0, 200.0));
    // The largest accepted value is a valid size (nothing is rendered at that size until asked).
    TestHost::new(Some(2147483647.0), Some(1.0)).expect("2147483647 fits signed 32-bit geometry");
}

#[test]
fn dev_006_a6_batch_returns_every_destroyed_identity_including_text_nodes() {
    require_backend!();
    let (mut host, _) = fixture_host();
    let destroyed = host
        .apply_batch(&json!([["destroyElement", 5], ["destroyElement", 5]]).to_string())
        .unwrap();
    assert_eq!(destroyed, [5, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19]);
    assert_eq!(host.element_count().unwrap(), 6);
    host.flush();
    assert!(
        host.elements_by_type("text")
            .unwrap()
            .iter()
            .all(|node| node.id < 10)
    );
    // A rejected batch reports an error and changes nothing.
    let error = host
        .apply_batch(&json!([["createElement", 90, "div"], ["explode", 1]]).to_string())
        .err()
        .expect("rejected");
    assert!(
        matches!(&error, HostError::Batch(message) if message.contains("operation 1")),
        "{error}"
    );
    assert_eq!(host.element_count().unwrap(), 6);
}

#[test]
fn dev_006_a7_executor_time_is_validated_and_independent_of_the_motion_clock() {
    require_backend!();
    let mut host = TestHost::new(Some(100.0), Some(100.0)).unwrap();
    for bad in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(
            matches!(
                host.advance_executor_time(bad),
                Err(HostError::InvalidDuration(_))
            ),
            "{bad}"
        );
    }
    assert_eq!(host.motion_set(123.0).unwrap(), 123.0);
    let ran = Rc::new(RefCell::new(Vec::new()));
    host.app().update(|cx| {
        for delay in [100u64, 300] {
            let ran = ran.clone();
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(delay))
                    .await;
                ran.borrow_mut().push(delay);
            })
            .detach();
        }
    });
    host.advance_executor_time(150.0).unwrap();
    assert_eq!(*ran.borrow(), [100], "only the timer due within 150 ms ran");
    assert_eq!(host.motion_now(), 123.0, "the motion clock is untouched");
    host.advance_executor_time(200.0).unwrap();
    assert_eq!(*ran.borrow(), [100, 300]);
    assert_eq!(host.motion_now(), 123.0);
    // Motion operations validate too.
    assert!(matches!(
        host.motion_set(-5.0),
        Err(HostError::InvalidDuration(_))
    ));
    assert!(matches!(
        host.motion_fast_forward(f64::NAN),
        Err(HostError::InvalidDuration(_))
    ));
    assert_eq!(host.motion_fast_forward(27.0).unwrap(), 150.0);
}

#[derive(Default)]
struct Selection {
    text: String,
    line: Option<ShapedLine>,
    bounds: Bounds<Pixels>,
    anchor: Option<f32>,
    focus: Option<f32>,
    selecting: bool,
}

/// A retained "document" with one text run and drag selection. Its selection listeners are
/// installed during paint, like selectable text elements are.
struct SelectableRun {
    state: Rc<RefCell<Selection>>,
}

impl RetainedHost for SelectableRun {
    fn apply_batch(&mut self, _batch: &str) -> Result<Vec<u64>, String> {
        Ok(Vec::new())
    }

    fn render(&mut self, _window: &mut Window, _cx: &mut App) -> AnyElement {
        let state = self.state.clone();
        canvas(
            |bounds, _, _| bounds,
            move |bounds, _, window, cx| {
                let text = state.borrow().text.clone();
                let font = window.text_style().font();
                let line = window.text_system().shape_line(
                    text.clone().into(),
                    px(16.0),
                    &[TextRun {
                        len: text.len(),
                        font,
                        color: black(),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    }],
                    None,
                );
                line.paint(bounds.origin, px(20.0), TextAlign::Left, None, window, cx)
                    .unwrap();
                {
                    let mut selection = state.borrow_mut();
                    selection.line = Some(line);
                    selection.bounds = bounds;
                }
                let st = state.clone();
                window.on_mouse_event(move |event: &MouseDownEvent, _phase, _window, _cx| {
                    let mut selection = st.borrow_mut();
                    if selection.bounds.contains(&event.position) {
                        let x = f32::from(event.position.x - selection.bounds.origin.x);
                        selection.anchor = Some(x);
                        selection.focus = Some(x);
                        selection.selecting = true;
                        // No invalidation: the move/up listeners only appear once a frame is
                        // painted, which the host's drag helper does between pointer events.
                    }
                });
                // Move and up listeners exist only in frames painted while selecting.
                if state.borrow().selecting {
                    let st = state.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, _phase, window, _cx| {
                        let mut selection = st.borrow_mut();
                        selection.focus =
                            Some(f32::from(event.position.x - selection.bounds.origin.x));
                        window.refresh();
                    });
                    let st = state;
                    window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                        if !phase.bubble() {
                            return;
                        }
                        let mut selection = st.borrow_mut();
                        selection.focus =
                            Some(f32::from(event.position.x - selection.bounds.origin.x));
                        selection.selecting = false;
                        drop(selection);
                        HostEvents::notify_selection_changed(cx);
                        window.refresh();
                    });
                }
            },
        )
        .size_full()
        .into_any_element()
    }

    fn root(&self) -> Option<u64> {
        Some(1)
    }

    fn element_count(&self) -> usize {
        1
    }

    fn snapshot(&self) -> Option<TreeNode> {
        Some(TreeNode {
            id: 1,
            element_type: "document".to_owned(),
            text: Some(self.state.borrow().text.clone()),
            ..TreeNode::default()
        })
    }

    fn selected_text(&self, _cx: &App) -> Option<String> {
        let selection = self.state.borrow();
        let (line, anchor, focus) = (
            selection.line.as_ref()?,
            selection.anchor?,
            selection.focus?,
        );
        let a = line.closest_index_for_x(px(anchor));
        let b = line.closest_index_for_x(px(focus));
        let (start, end) = (a.min(b), a.max(b));
        (start < end).then(|| selection.text[start..end].to_owned())
    }

    fn clear_selection(
        &mut self,
        window: &mut Window,
        _cx: &mut App,
    ) -> vespertine_test_host::HostResult<()> {
        let mut selection = self.state.borrow_mut();
        selection.anchor = None;
        selection.focus = None;
        window.refresh();
        Ok(())
    }
}

fn raw_mouse(host: &mut TestHost, input: vespertine::PlatformInput) {
    let window = host.window();
    host.app().simulate_platform_input(window, input).unwrap();
}

#[test]
fn dev_006_a8_drag_selection_returns_the_run_and_needs_a_painted_frame_between_events() {
    require_backend!();
    let state = Rc::new(RefCell::new(Selection {
        text: "Selectable run of text".to_owned(),
        ..Selection::default()
    }));
    let mut host = TestHost::with_tree(
        HostOptions::with_size(400.0, 100.0),
        SelectableRun { state },
    )
    .unwrap();
    host.flush();
    let changes = Rc::new(RefCell::new(0));
    let _subscription = host.on_selection_change({
        let changes = changes.clone();
        move || *changes.borrow_mut() += 1
    });

    // The run is painted from x = 0; drag from its start to far past its end.
    let frames_before = host.paint_log().frames();
    let selected = host.drag_select((0.5, 10.0), (390.0, 10.0)).unwrap();
    assert_eq!(selected.as_deref(), Some("Selectable run of text"));
    assert!(
        host.painted_text()
            .contains(&"Selectable run of text".to_owned()),
        "text painted by a custom element is in the painted strings"
    );
    assert_eq!(
        host.selected_text().unwrap().as_deref(),
        Some("Selectable run of text")
    );
    assert!(
        host.paint_log().frames() >= frames_before + 3,
        "a frame is painted between pointer-down, pointer-move and pointer-up"
    );
    assert_eq!(
        *changes.borrow(),
        1,
        "the selection listener fired once, at pointer-up"
    );

    // Without a frame between events the move/up listeners do not exist yet: the raw events
    // never reach them.
    host.clear_selection().unwrap();
    raw_mouse(
        &mut host,
        vespertine::PlatformInput::MouseDown(MouseDownEvent {
            position: point(px(0.5), px(10.0)),
            modifiers: Default::default(),
            button: MouseButton::Left,
            click_count: 1,
            first_mouse: false,
        }),
    );
    let move_event = || {
        vespertine::PlatformInput::MouseMove(MouseMoveEvent {
            position: point(px(390.0), px(10.0)),
            pressed_button: Some(MouseButton::Left),
            modifiers: Default::default(),
        })
    };
    raw_mouse(&mut host, move_event());
    assert_eq!(
        host.selected_text().unwrap(),
        None,
        "no frame was painted since pointer-down"
    );
    host.paint_frame();
    raw_mouse(&mut host, move_event());
    assert_eq!(
        host.selected_text().unwrap().as_deref(),
        Some("Selectable run of text"),
        "after a painted frame the listener is installed"
    );
}

struct Reentrant;

impl Render for Reentrant {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().on_mouse_down(
            MouseButton::Left,
            cx.listener(|_this, _event, _window, cx| {
                // Updating the view that is being dispatched to is invalid in a live window.
                let this = cx.entity();
                this.update(cx, |_, _| {});
            }),
        )
    }
}

#[test]
fn dev_006_a9_reentering_the_root_view_during_dispatch_fails_as_in_a_live_window() {
    require_backend!();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut host = TestHost::with_view(HostOptions::with_size(100.0, 100.0), |_, cx| {
            cx.new(|_| Reentrant)
        })
        .unwrap();
        host.click(10.0, 10.0, 0, "").unwrap();
    }));
    let payload = outcome.expect_err("re-entering the leased root view must fail");
    let message = payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|text| text.to_string()))
        .unwrap_or_default();
    assert!(
        message.contains("while it is already being updated"),
        "unexpected failure: {message}"
    );
}

#[test]
fn dev_006_input_helpers_reject_invalid_arguments_before_dispatching() {
    require_backend!();
    let (mut host, events) = fixture_host();
    assert!(matches!(
        host.click(10.0, 10.0, 3, ""),
        Err(HostError::InvalidButton(3))
    ));
    assert!(matches!(
        host.simulate_keystrokes("a not-a-key- b", None),
        Err(HostError::InvalidKeystroke { .. })
    ));
    assert!(events.borrow().is_empty());
    // Modifier strings: unknown words are ignored.
    let modifiers = vespertine_test_host::parse_modifiers("ctrl-shift-bogus");
    assert!(modifiers.control && modifiers.shift && !modifiers.alt && !modifiers.platform);
    let modifiers = vespertine_test_host::parse_modifiers("cmd-alt");
    assert!(modifiers.platform && modifiers.alt);
    assert!(vespertine_test_host::parse_modifiers("meta").platform);
    assert!(vespertine_test_host::parse_modifiers("control").control);
}

#[test]
fn dev_006_focus_traversal_and_identity() {
    require_backend!();
    let (mut host, _) = fixture_host();
    host.flush();
    assert_eq!(host.focused_element(), None);
    host.focus_element(3).unwrap();
    assert_eq!(host.focused_element(), Some(3));
    assert!(matches!(
        host.focus_element(999),
        Err(HostError::NoSuchElement(999))
    ));
    host.blur().unwrap();
    assert_eq!(host.focused_element(), None);
    // Within-subtree traversal is an optional capability of the retained tree.
    assert!(matches!(
        host.focus_next_within(1),
        Err(HostError::Unsupported(_))
    ));
    // Key events are observable at window level.
    let seen = Rc::new(RefCell::new(Vec::new()));
    let _subscription = host.on_key_event({
        let seen = seen.clone();
        move |event| seen.borrow_mut().push(event.keystroke.unparse())
    });
    host.simulate_keystrokes("x", Some(3)).unwrap();
    host.key_down("y", true, None).unwrap();
    host.key_up("y", None).unwrap();
    assert_eq!(*seen.borrow(), ["x", "y"]);
    assert_eq!(
        by_test_id(&mut host, "name").custom_props["value"],
        json!("xy")
    );
}

#[test]
fn dev_006_file_drop_is_an_enter_then_a_submit() {
    require_backend!();
    struct DropTarget(Rc<RefCell<Vec<String>>>);
    impl Render for DropTarget {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let drops = self.0.clone();
            div().id("drop-target").size_full().on_drop(
                move |paths: &vespertine::ExternalPaths, _window, _cx| {
                    drops
                        .borrow_mut()
                        .extend(paths.paths().iter().map(|path| path.display().to_string()));
                },
            )
        }
    }
    let drops = Rc::new(RefCell::new(Vec::new()));
    let mut host = TestHost::with_view(HostOptions::with_size(100.0, 100.0), {
        let drops = drops.clone();
        move |_, cx| cx.new(|_| DropTarget(drops))
    })
    .unwrap();
    host.file_drop(50.0, 50.0, &["/tmp/a.txt".into(), "/tmp/b.txt".into()])
        .unwrap();
    assert_eq!(*drops.borrow(), ["/tmp/a.txt", "/tmp/b.txt"]);
}

#[test]
fn dev_006_register_font_rejects_invalid_bytes_and_accepts_a_font() {
    require_backend!();
    let mut host = TestHost::new(Some(50.0), Some(50.0)).unwrap();
    assert!(matches!(
        host.register_font(vec![1, 2, 3]),
        Err(HostError::InvalidFont(_))
    ));
    // Any installed font file is a valid font to register.
    let font = ["/usr/share/fonts", "/usr/local/share/fonts"]
        .iter()
        .flat_map(|root| walk(std::path::Path::new(root)))
        .find(|path| path.extension().is_some_and(|extension| extension == "ttf"));
    if let Some(font) = font {
        host.register_font(std::fs::read(font).unwrap()).unwrap();
    }
}

fn walk(directory: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(directory) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(walk(&path));
            } else {
                files.push(path);
            }
        }
    }
    files
}

#[test]
fn dev_006_tree_less_host_reports_missing_capabilities_explicitly() {
    require_backend!();
    let mut host = TestHost::with_view(HostOptions::with_size(80.0, 60.0), |_, cx| {
        cx.new(|_| Swatch)
    })
    .unwrap();
    assert!(matches!(
        host.serialize_tree(),
        Err(HostError::Unsupported(_))
    ));
    assert!(matches!(
        host.apply_batch("[]"),
        Err(HostError::Unsupported(_))
    ));
    assert!(matches!(
        host.syntax_cache_counters(),
        Err(HostError::Unsupported(_))
    ));
    assert!(matches!(
        host.set_image_bytes(1, &[]),
        Err(HostError::Unsupported(_))
    ));
    // Painted-frame inspection needs no tree.
    assert!(host.painted_text().is_empty());
    let image = host.screenshot_image().unwrap();
    assert_eq!(image.dimensions(), (160, 120));
}

#[test]
fn dev_006_teardown_releases_entities_before_the_application() {
    require_backend!();
    struct Probe(Rc<RefCell<bool>>);
    impl Drop for Probe {
        fn drop(&mut self) {
            *self.0.borrow_mut() = true;
        }
    }
    impl Render for Probe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }
    let released = Rc::new(RefCell::new(false));
    let host = TestHost::with_view(HostOptions::with_size(50.0, 50.0), {
        let released = released.clone();
        move |_, cx| cx.new(|_| Probe(released))
    })
    .unwrap();
    assert!(!*released.borrow());
    drop(host);
    assert!(
        *released.borrow(),
        "the view-owned entity was released on teardown"
    );
}

#[test]
fn dev_006_key_down_repeat_flag_key_up_and_mouse_exit_reach_window_listeners() {
    require_backend!();
    #[derive(Default)]
    struct Seen {
        downs: Vec<(String, bool)>,
        ups: Vec<String>,
        exits: usize,
    }
    struct Listeners {
        seen: Rc<RefCell<Seen>>,
        focus: vespertine::FocusHandle,
    }
    impl Render for Listeners {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let (down, up, exit) = (self.seen.clone(), self.seen.clone(), self.seen.clone());
            div()
                .id("listeners")
                .size_full()
                .track_focus(&self.focus)
                .on_key_down(move |event, _, _| {
                    down.borrow_mut()
                        .downs
                        .push((event.keystroke.key.clone(), event.is_held));
                })
                .on_key_up(move |event, _, _| {
                    up.borrow_mut().ups.push(event.keystroke.key.clone());
                })
                .child(
                    canvas(
                        |_, _, _| (),
                        move |_, _, window, _| {
                            window.on_mouse_event(
                                move |_: &vespertine::MouseExitEvent, phase, _, _| {
                                    if phase.bubble() {
                                        exit.borrow_mut().exits += 1;
                                    }
                                },
                            );
                        },
                    )
                    .size_full(),
                )
        }
    }
    let seen = Rc::new(RefCell::new(Seen::default()));
    let mut host = TestHost::with_view(HostOptions::with_size(100.0, 100.0), {
        let seen = seen.clone();
        move |window, cx| {
            let focus = cx.focus_handle();
            window.focus(&focus, cx);
            cx.new(|_| Listeners { seen, focus })
        }
    })
    .unwrap();
    host.flush();
    host.key_down("a", false, None).unwrap();
    host.key_down("a", true, None).unwrap();
    host.key_up("a", None).unwrap();
    host.mouse_exit(120.0, 50.0).unwrap();
    let seen = seen.borrow();
    assert_eq!(
        seen.downs,
        [("a".to_owned(), false), ("a".to_owned(), true)],
        "the repeat flag is carried"
    );
    assert_eq!(seen.ups, ["a"]);
    assert_eq!(seen.exits, 1);
}

#[test]
fn dev_006_painted_washes_are_a_per_frame_log_cleared_once_per_frame() {
    require_backend!();
    use vespertine_test_host::{PaintedWash, Rect};
    struct Washes;
    impl Render for Washes {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            canvas(
                |_, _, _| (),
                |bounds, _, window, cx| {
                    PaintLog::record_wash(
                        cx,
                        PaintedWash {
                            element_id: 9,
                            run_index: 1,
                            text: "Find red".to_owned(),
                            start_utf16: 5,
                            end_utf16: 8,
                            active: true,
                            rects: vec![Rect::from(bounds)],
                        },
                    );
                    PaintLog::record_text(cx, "generated chrome");
                    let _ = window;
                },
            )
            .size_full()
        }
    }
    let mut host = TestHost::with_view(HostOptions::with_size(60.0, 40.0), |_, cx| {
        cx.new(|_| Washes)
    })
    .unwrap();
    host.flush();
    let washes = host.painted_washes();
    assert_eq!(
        washes.len(),
        1,
        "one wash per frame, not accumulated: {washes:?}"
    );
    assert_eq!(washes[0].element_id, 9);
    assert_eq!((washes[0].start_utf16, washes[0].end_utf16), (5, 8));
    assert_eq!(
        washes[0].rects,
        [Rect {
            x: 0.0,
            y: 0.0,
            width: 60.0,
            height: 40.0
        }]
    );
    assert_eq!(host.painted_text(), ["generated chrome"]);
    let frames = host.paint_log().frames();
    host.paint_frame();
    assert!(host.paint_log().frames() > frames);
    assert_eq!(host.painted_washes().len(), 1);
    assert_eq!(host.painted_text(), ["generated chrome"]);
}
