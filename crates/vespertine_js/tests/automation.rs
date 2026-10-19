//! DEV-007 / APP-007 R8 on the JavaScript host: live automation served over byte streams
//! against a renderer whose UI loop runs on the test platform, so the protocol, the window's
//! input path, the retained tree and the paint log are exercised without a display server. The
//! same scenario against a real window and a real process is the `live_automation` test of
//! `vespertine_js_node`.

use std::io::{Read, Write};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, Weak};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};
use vespertine::{AppContext as _, TestAppContext, WindowOptions, px, size};
use vespertine_js::{HostError, HostRenderer, HostWindow, LiveAutomation, UiLink, UiTask};
use vespertine_test_host::automation::{
    Automation, AutomationError, AutomationHost, Capability, ErrorCode, Session, SessionOptions,
};

type Events = Arc<Mutex<Vec<Value>>>;

struct Rig {
    cx: TestAppContext,
    window: HostWindow,
    tasks: async_channel::Receiver<UiTask>,
    renderer: Arc<HostRenderer>,
    events: Events,
}

fn rig() -> Rig {
    let cx = TestAppContext::single();
    let events: Events = Arc::default();
    let sink = {
        let events = events.clone();
        Arc::new(move |event: Result<Value, String>| {
            events.lock().unwrap().push(event.unwrap());
        }) as vespertine_js::EventSink
    };
    let renderer = Arc::new(HostRenderer::new(Some(sink)));
    let (link, tasks) = UiLink::<UiTask>::channel();
    renderer
        .attach_link(link, Arc::new(AtomicBool::new(true)))
        .unwrap();
    let view = renderer.new_view();
    let window = cx.update(|cx| {
        cx.open_window(
            WindowOptions {
                window_bounds: Some(vespertine::WindowBounds::Windowed(vespertine::Bounds {
                    origin: vespertine::point(px(0.), px(0.)),
                    size: size(px(400.), px(300.)),
                })),
                ..Default::default()
            },
            |_, cx| cx.new(|_| view),
        )
        .unwrap()
    });
    // Focus events are delivered only while the window is active, as on a real display.
    cx.update(|cx| {
        window
            .update(cx, |_, window, cx| window.set_active_for_tests(true, cx))
            .unwrap();
    });
    Rig {
        cx,
        window,
        tasks,
        renderer,
        events,
    }
}

impl Rig {
    /// Runs every queued task on this thread, as the UI loop does.
    fn pump(&self) {
        while let Ok(task) = self.tasks.try_recv() {
            self.cx.update(|cx| task(cx, &self.window));
        }
    }

    /// Runs `call` on a helper thread (it blocks on UI results) while this thread serves tasks.
    fn serve<T: Send>(&self, call: impl FnOnce() -> T + Send) -> T {
        thread::scope(|scope| {
            let handle = scope.spawn(call);
            while !handle.is_finished() {
                self.pump();
                thread::yield_now();
            }
            handle.join().unwrap()
        })
    }

    /// Serves the renderer over in-memory pipes and runs `scenario` against the automation
    /// client on the other end, then closes the channel and waits for the server to finish.
    fn live<T: Send>(
        &self,
        scenario: impl FnOnce(&Automation, &Events, &HostRenderer) -> T + Send,
    ) -> T {
        let renderer = self.renderer.clone();
        let events = self.events.clone();
        self.serve(move || {
            let (server_in, mut client_out) = std::io::pipe().unwrap();
            let (mut client_in, server_out) = std::io::pipe().unwrap();
            let serving = renderer.serve_automation_on(server_in, server_out).unwrap();
            let session = Arc::new(Session::over_channel(
                move |bytes| client_out.write_all(bytes),
                SessionOptions::default(),
            ));
            let feed: Weak<Session> = Arc::downgrade(&session);
            let feeder = thread::spawn(move || {
                let mut buffer = [0u8; 4096];
                loop {
                    match client_in.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => match feed.upgrade() {
                            Some(session) => session.feed(&buffer[..read]),
                            None => break,
                        },
                    }
                }
            });
            let app = Automation::connect_session(session).unwrap();
            let outcome = scenario(&app, &events, &renderer);
            drop(app);
            serving.join().unwrap().unwrap();
            feeder.join().unwrap();
            outcome
        })
    }
}

/// A 400 x 300 column: a title, a text input (`name`), a button (`save`), a scrolling list of
/// ten items (`list`) and a hidden element (`ghost`), with the listeners the host maps.
fn scene() -> String {
    let mut ops = vec![
        json!(["createElement", 1, "div"]),
        json!(["setStyle", 1, {"width": 400, "height": 300, "flexDirection": "column", "backgroundColor": "#ffffff", "fontSize": 14, "color": "#111111"}]),
        json!(["createElement", 2, "text"]),
        json!(["setText", 2, "Fixture"]),
        json!(["setCustomProp", 2, "testId", "title"]),
        json!(["createElement", 3, "input"]),
        json!(["setStyle", 3, {"width": 200, "height": 28, "backgroundColor": "#eeeeee"}]),
        json!(["setCustomProp", 3, "testId", "name"]),
        json!(["setEventListener", 3, "focus", true]),
        json!(["setEventListener", 3, "blur", true]),
        json!(["setEventListener", 3, "keyDown", true]),
        json!(["setEventListener", 3, "keyUp", true]),
        json!(["createElement", 4, "div"]),
        json!(["setStyle", 4, {"width": 80, "height": 30, "backgroundColor": "#3355ff", "color": "#ffffff"}]),
        json!(["setText", 4, "Save"]),
        json!(["setCustomProp", 4, "testId", "save"]),
        json!(["setEventListener", 4, "click", true]),
        json!(["setEventListener", 4, "pointerDown", true]),
        json!(["setEventListener", 4, "pointerUp", true]),
        json!(["createElement", 5, "div"]),
        json!(["setStyle", 5, {"width": 200, "height": 90, "flexDirection": "column", "overflow": "scroll", "backgroundColor": "#f4f4f4"}]),
        json!(["setCustomProp", 5, "testId", "list"]),
        json!(["setEventListener", 5, "wheel", true]),
    ];
    for item in 0..10u64 {
        let id = 10 + item;
        ops.push(json!(["createElement", id, "text"]));
        ops.push(json!(["setStyle", id, {"height": 30, "width": "100%"}]));
        ops.push(json!(["setText", id, format!("Item {item}")]));
        ops.push(json!(["appendChild", 5, id]));
    }
    ops.extend([
        json!(["createElement", 7, "text"]),
        json!(["setStyle", 7, {"visibility": "hidden"}]),
        json!(["setText", 7, "ghost"]),
        json!(["setCustomProp", 7, "testId", "ghost"]),
    ]);
    for child in [2, 3, 4, 5, 7] {
        ops.push(json!(["appendChild", 1, child]));
    }
    ops.push(json!(["setRoot", 1]));
    Value::Array(ops).to_string()
}

fn of_type(events: &Events, kind: &str) -> Vec<Value> {
    events
        .lock()
        .unwrap()
        .iter()
        .filter(|event| event["type"] == kind)
        .cloned()
        .collect()
}

fn take_events(events: &Events) -> Vec<Value> {
    std::mem::take(&mut *events.lock().unwrap())
}

#[test]
fn dev_007_live_initialize_reports_identity_capabilities_and_viewport() {
    let rig = rig();
    rig.live(|app, _, _| {
        let info = app.info();
        assert_eq!(info.protocol_version, 1);
        assert_eq!(info.pid, u64::from(std::process::id()));
        assert_eq!((info.width, info.height), (400.0, 300.0));
        // Offered: everything a live window can really do. Not offered: screenshots, because no
        // live platform window reads its frame back.
        let mut offered = info.capabilities.clone();
        offered.sort_by_key(|capability| capability.as_str());
        assert_eq!(
            offered,
            [Capability::Clock, Capability::Input, Capability::Tree]
        );
    });
}

#[test]
fn dev_007_live_tree_bounds_and_text_follow_the_applied_batch() {
    let rig = rig();
    rig.live(|app, _, renderer| {
        assert_eq!(app.tree().unwrap(), None, "an empty window has no tree");
        renderer.apply_batch(&scene()).unwrap();

        let tree = app.tree().unwrap().expect("the mounted tree");
        assert_eq!(tree.id, 1);
        assert_eq!(tree.children.len(), 5);
        let root = tree.bounds.expect("the root was painted");
        assert_eq!(
            (root.x, root.y, root.width, root.height),
            (0.0, 0.0, 400.0, 300.0)
        );

        // Bounds are the painted ones: the button is 80 x 30 and sits below the title and input.
        let save = app.get_by_test_id("save").bounds().unwrap();
        assert_eq!((save.width, save.height), (80.0, 30.0));
        assert!(
            save.y >= 28.0,
            "the button is laid out after the input: {save:?}"
        );
        assert_eq!(app.bounds_of(4).unwrap(), Some(save));
        assert_eq!(app.get_by_test_id("list").bounds().unwrap().height, 90.0);

        // A hidden element is retained but has no painted bounds.
        let ghost = app.get_by_test_id("ghost").element().unwrap();
        assert_eq!(ghost.text.as_deref(), Some("ghost"));
        assert_eq!(ghost.bounds, None);
        assert_eq!(app.bounds_of(7).unwrap(), None);

        let texts = app.all_text().unwrap();
        assert_eq!(&texts[..3], ["Fixture", "Save", "Item 0"]);
        assert!(texts.contains(&"ghost".to_owned()));
        let painted = app.painted_text().unwrap();
        for text in ["Fixture", "Save", "Item 0"] {
            assert!(
                painted.iter().any(|line| line == text),
                "{text:?} in {painted:?}"
            );
        }
        assert!(
            !painted.iter().any(|line| line == "ghost"),
            "hidden text is not painted: {painted:?}"
        );

        // A later batch is observed by the next query.
        renderer
            .apply_batch(r#"[["setText",4,"Saved"],["setStyle",4,{"width":120}]]"#)
            .unwrap();
        assert_eq!(
            app.get_by_test_id("save")
                .element()
                .unwrap()
                .text
                .as_deref(),
            Some("Saved")
        );
        assert_eq!(app.get_by_test_id("save").bounds().unwrap().width, 120.0);
        assert!(
            app.painted_text()
                .unwrap()
                .iter()
                .any(|line| line == "Saved")
        );
        assert_eq!(app.selected_text().unwrap(), None);
        app.clear_selection().unwrap();
    });
}

#[test]
fn dev_007_live_click_delivers_pointer_and_click_events_at_the_painted_center() {
    let rig = rig();
    rig.live(|app, events, renderer| {
        renderer.apply_batch(&scene()).unwrap();
        let save = app.get_by_test_id("save");
        let (x, y) = save.center().unwrap();
        take_events(events);

        save.click().unwrap();
        let clicks = of_type(events, "click");
        assert_eq!(clicks.len(), 1, "{:?}", events.lock().unwrap());
        assert_eq!(clicks[0]["id"], 4);
        assert_eq!(
            (clicks[0]["x"].as_f64(), clicks[0]["y"].as_f64()),
            (Some(x), Some(y))
        );
        assert_eq!(clicks[0]["button"], 0);
        assert_eq!(of_type(events, "pointerDown").len(), 1);
        assert_eq!(of_type(events, "pointerUp").len(), 1);

        // Raw gestures and modifiers reach the same path.
        take_events(events);
        app.mouse().down((x, y), 0, "shift").unwrap();
        let down = of_type(events, "pointerDown");
        assert_eq!(down.len(), 1);
        assert_eq!(down[0]["shift"], true);
        app.mouse().up((x, y), 0, "shift").unwrap();
        assert_eq!(of_type(events, "pointerUp").len(), 1);

        // A click away from the button is not delivered to it.
        take_events(events);
        app.mouse().click((390.0, 290.0), 0, "").unwrap();
        assert!(of_type(events, "click").is_empty());
    });
}

#[test]
fn dev_007_live_keystrokes_focus_blur_and_scroll_reach_the_window() {
    let rig = rig();
    rig.live(|app, events, renderer| {
        renderer.apply_batch(&scene()).unwrap();
        let name = app.get_by_test_id("name").element().unwrap().id;
        assert_eq!(name, 3);
        take_events(events);

        // Keystrokes targeted at an element focus it first.
        app.keystrokes("a b", Some(name)).unwrap();
        assert_eq!(renderer.focused_element().unwrap(), Some(name));
        let keys: Vec<Value> = of_type(events, "keyDown")
            .iter()
            .filter(|event| event["id"] == name)
            .map(|event| event["key"].clone())
            .collect();
        assert_eq!(keys, [json!("a"), json!("b")]);
        assert_eq!(of_type(events, "focus").len(), 1);

        take_events(events);
        app.key_down("c", true, None).unwrap();
        let held = of_type(events, "keyDown");
        assert_eq!(held.len(), 1);
        assert_eq!(held[0]["held"], true);
        app.key_up("c", None).unwrap();
        assert_eq!(of_type(events, "keyUp").len(), 1);

        // Blur removes focus; focus puts it back.
        take_events(events);
        app.blur().unwrap();
        assert_eq!(renderer.focused_element().unwrap(), None);
        assert_eq!(of_type(events, "blur").len(), 1);
        app.focus(name).unwrap();
        assert_eq!(renderer.focused_element().unwrap(), Some(name));

        // Scrolling: absolute offsets, wheel input, and no offset for a non-scrolling element.
        assert_eq!(app.scroll_offset(5).unwrap(), Some((0.0, 0.0)));
        app.scroll_to(5, 0.0, 60.0).unwrap();
        assert_eq!(app.scroll_offset(5).unwrap(), Some((0.0, 60.0)));
        app.scroll_to(5, 0.0, 0.0).unwrap();
        take_events(events);
        app.get_by_test_id("list").wheel(0.0, -30.0).unwrap();
        assert_eq!(of_type(events, "wheel").len(), 1);
        assert_eq!(app.scroll_offset(5).unwrap(), Some((0.0, 30.0)));
        assert_eq!(app.scroll_offset(2).unwrap(), None);
    });
}

#[test]
fn dev_007_live_unsupported_and_failing_methods_use_the_protocol_error_codes() {
    let rig = rig();
    rig.live(|app, _, renderer| {
        renderer.apply_batch(&scene()).unwrap();
        let code = |result: Result<Value, AutomationError>| result.unwrap_err().code;

        // No live frame readback: the screenshot method fails explicitly.
        let path = std::env::temp_dir().join("vespertine-live-automation.png");
        assert_eq!(
            code(app.request("screenshot", json!({ "path": path.to_string_lossy() }))),
            ErrorCode::Unsupported
        );
        assert!(!path.exists());
        assert_eq!(
            code(app.request("noSuchMethod", json!({}))),
            ErrorCode::Protocol
        );
        assert_eq!(
            code(app.request("click", json!({ "x": "left", "y": 1 }))),
            ErrorCode::Protocol
        );

        assert_eq!(
            code(app.request("focus", json!({ "elementId": 99 }))),
            ErrorCode::NotFound
        );
        assert_eq!(
            code(app.request("focus", json!({ "elementId": 2 }))),
            ErrorCode::NotFound
        );
        assert_eq!(
            code(app.request("scrollTo", json!({ "elementId": 2, "x": 0, "y": 1 }))),
            ErrorCode::Unsupported
        );
        assert_eq!(
            code(app.request("keystrokes", json!({ "keys": "a-b" }))),
            ErrorCode::Protocol
        );
        assert_eq!(
            code(app.request("clockSet", json!({ "nowMs": -1 }))),
            ErrorCode::Protocol
        );

        // The motion clock is the application's: set, advance, resume.
        let now = |value: Value| value["nowMs"].as_f64().unwrap();
        assert_eq!(
            now(app.request("clockSet", json!({ "nowMs": 2000 })).unwrap()),
            2000.0
        );
        assert_eq!(
            now(app
                .request("clockFastForward", json!({ "deltaMs": 500 }))
                .unwrap()),
            2500.0
        );
        assert_eq!(now(app.request("clockPause", json!({})).unwrap()), 2500.0);
        thread::sleep(Duration::from_millis(30));
        let resumed = now(app.request("clockResume", json!({})).unwrap());
        assert_eq!(resumed, 2500.0, "wall time spent paused is not counted");
    });
}

#[test]
fn dev_007_live_failures_of_the_ui_loop_map_to_timeout_and_closed() {
    let rig = rig();
    let renderer = rig.renderer.clone();
    // A loop that answers nothing: the response timeout elapses.
    let stalled = Arc::new(HostRenderer::new(None));
    let (link, tasks) = UiLink::<UiTask>::channel_with_timeout(Duration::from_millis(50));
    stalled
        .attach_link(link, Arc::new(AtomicBool::new(true)))
        .unwrap();
    let error = LiveAutomation::new(&stalled)
        .err()
        .expect("nobody answers the attach query");
    assert!(matches!(error, HostError::TimedOut { .. }), "{error}");
    drop(tasks);
    assert_eq!(
        LiveAutomation::new(&stalled)
            .err()
            .expect("the loop is gone"),
        HostError::NotRunning
    );

    // Attached to a serving loop, then the loop goes away: operations fail with Closed.
    let mut host = rig.serve(|| LiveAutomation::new(&renderer).unwrap());
    assert_eq!(
        host.capabilities(),
        [Capability::Input, Capability::Tree, Capability::Clock]
    );
    let Rig { tasks, .. } = rig;
    drop(tasks);
    let error = host.click(1.0, 1.0, 0, "").unwrap_err();
    assert_eq!(error.code, ErrorCode::Closed, "{error}");
    assert_eq!(host.tree().unwrap_err().code, ErrorCode::Closed);
    // Without a window to ask, the viewport is unknown rather than invented.
    assert_eq!(host.viewport(), (0.0, 0.0));
}

#[test]
fn dev_007_live_a_window_paints_ordinary_frames_again_once_the_channel_ends() {
    let rig = rig();
    let observing = |renderer: &HostRenderer| {
        renderer
            .query("observation", |cx, _| {
                cx.try_global::<vespertine::PaintObserver>()
                    .is_some_and(|observer| observer.on_frame_start.is_some())
            })
            .unwrap()
    };
    assert!(
        !rig.serve(|| observing(&rig.renderer)),
        "nobody automates the window yet"
    );
    rig.live(|app, _, renderer| {
        renderer.apply_batch(&scene()).unwrap();
        assert!(app.tree().unwrap().is_some());
        assert!(
            observing(renderer),
            "frames are observed while a client is attached"
        );
    });
    assert!(
        !rig.serve(|| observing(&rig.renderer)),
        "the observation ends with the channel"
    );
}

#[test]
fn app_007_r8_serving_automation_needs_a_running_renderer() {
    let renderer = HostRenderer::new(None);
    assert_eq!(
        renderer.serve_automation().unwrap_err(),
        HostError::NotInitialized
    );
    // The failed attempt did not claim standard input and output.
    assert_eq!(
        renderer.serve_automation().unwrap_err(),
        HostError::NotInitialized
    );
    let error = renderer
        .serve_automation_on(std::io::empty(), std::io::sink())
        .unwrap_err();
    assert_eq!(error, HostError::NotInitialized);
}
