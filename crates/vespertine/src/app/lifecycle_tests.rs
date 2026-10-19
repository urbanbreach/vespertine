//! Tests for PLT-008: window geometry lifecycle, close interception, quit interception and
//! the retained application lifecycle.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

use uuid::Uuid;

use crate::{
    ActivationRequest, AnyWindowHandle, App, AppContext as _, Bounds, Context, DisplayId,
    DisplayWorkArea, IntoElement, Pixels, QuitMode, Render, SavedWindowGeometry, TestActivation,
    TestAppContext, Window, WindowBounds, WindowHandle, WindowMode, WindowOptions, div,
    launch_urls_from_args, point, px, restore_window_geometry, size,
};

struct Blank;

impl Render for Blank {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

fn bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
    Bounds::new(point(px(x), px(y)), size(px(width), px(height)))
}

fn open(cx: &mut TestAppContext, options: WindowOptions) -> WindowHandle<Blank> {
    cx.update(|cx| {
        cx.open_window(options, |_, cx| cx.new(|_| Blank))
            .expect("the test platform opens windows")
    })
}

fn open_windowed(cx: &mut TestAppContext, window_bounds: WindowBounds) -> WindowHandle<Blank> {
    open(
        cx,
        WindowOptions {
            window_bounds: Some(window_bounds),
            ..Default::default()
        },
    )
}

const SECOND_DISPLAY: DisplayId = DisplayId(2);

fn connect_second_display(cx: &TestAppContext) -> Uuid {
    let uuid = Uuid::from_u128(0x5ec0);
    cx.simulate_display_connected(
        SECOND_DISPLAY,
        uuid,
        bounds(1920.0, 0.0, 1280.0, 1024.0),
        bounds(1920.0, 30.0, 1280.0, 994.0),
    );
    uuid
}

fn primary_display_id(cx: &TestAppContext) -> DisplayId {
    cx.update(|cx| cx.primary_display().expect("a primary display").id())
}

// --- A1: relaunch after a display disappeared ------------------------------------------

#[vespertine::test]
fn window_saved_on_a_removed_display_reopens_reachable_on_an_available_one(
    cx: &mut TestAppContext,
) {
    let second_uuid = connect_second_display(cx);
    let window = open(
        cx,
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds(2020.0, 130.0, 800.0, 600.0))),
            display_id: Some(SECOND_DISPLAY),
            ..Default::default()
        },
    );
    let saved = window
        .update(cx, |_, window, cx| {
            let display = window.display(cx);
            SavedWindowGeometry::capture(window.window_bounds(), display.as_deref())
        })
        .unwrap();
    assert_eq!(saved.display_uuid, Some(second_uuid));
    assert_eq!(saved.bounds, bounds(100.0, 130.0, 800.0, 600.0));

    // The display is gone at the next launch.
    cx.simulate_display_removed(SECOND_DISPLAY);
    let (restored, work_area) = cx.update(|cx| {
        let displays = DisplayWorkArea::all(cx);
        let primary = cx.primary_display().unwrap();
        let restored = restore_window_geometry(Some(&saved), &displays, Some(primary.id()), None);
        (restored, primary.visible_bounds())
    });
    assert_eq!(restored.display_id, Some(primary_display_id(cx)));

    let relaunched = open(
        cx,
        WindowOptions {
            window_bounds: Some(restored.window_bounds),
            display_id: restored.display_id,
            ..Default::default()
        },
    );
    let opened = relaunched
        .update(cx, |_, window, _| window.bounds())
        .unwrap();
    assert_eq!(
        work_area.intersect(&opened),
        opened,
        "fully inside the work area"
    );
}

#[vespertine::test]
fn saved_rectangle_outside_every_work_area_is_centered(cx: &mut TestAppContext) {
    let saved = SavedWindowGeometry {
        mode: WindowMode::Windowed,
        bounds: bounds(7000.0, 7000.0, 800.0, 600.0),
        display_uuid: None,
    };
    let restored = cx.update(|cx| {
        let displays = DisplayWorkArea::all(cx);
        restore_window_geometry(
            Some(&saved),
            &displays,
            cx.primary_display().map(|d| d.id()),
            None,
        )
    });
    let window = open(
        cx,
        WindowOptions {
            window_bounds: Some(restored.window_bounds),
            display_id: restored.display_id,
            ..Default::default()
        },
    );
    assert_eq!(
        window.update(cx, |_, window, _| window.bounds()).unwrap(),
        bounds(560.0, 240.0, 800.0, 600.0)
    );
}

#[vespertine::test]
fn display_work_area_changes_are_observed(cx: &mut TestAppContext) {
    let events = Rc::new(RefCell::new(Vec::new()));
    cx.update(|cx| {
        let events = events.clone();
        cx.observe_displays(move |event, _| events.borrow_mut().push(event))
            .detach();
    });
    let uuid = connect_second_display(cx);
    cx.simulate_display_geometry_change(
        SECOND_DISPLAY,
        uuid,
        bounds(1920.0, 0.0, 1280.0, 1024.0),
        bounds(1920.0, 60.0, 1280.0, 964.0),
    );
    let events = events.borrow();
    assert!(matches!(events[0], crate::DisplayEvent::Added(id) if id == SECOND_DISPLAY));
    assert!(matches!(
        events[1],
        crate::DisplayEvent::Changed { id, changes }
            if id == SECOND_DISPLAY
                && changes == crate::DisplayChanges::VISIBLE_BOUNDS
    ));
}

// --- A3: mode preservation -------------------------------------------------------------

#[vespertine::test]
fn maximize_fullscreen_and_resize_never_change_the_ordinary_bounds(cx: &mut TestAppContext) {
    let ordinary = bounds(200.0, 150.0, 800.0, 600.0);
    let window = open_windowed(cx, WindowBounds::Windowed(ordinary));
    let any: AnyWindowHandle = window.into();

    let observed = Rc::new(RefCell::new(Vec::new()));
    window
        .update(cx, |_, window, _| {
            let observed = observed.clone();
            window
                .observe_window_bounds(move |window, _| {
                    observed.borrow_mut().push(window.window_bounds());
                })
                .detach();
        })
        .unwrap();

    cx.simulate_window_maximize(any, true);
    assert!(
        window
            .update(cx, |_, window, _| window.is_maximized())
            .unwrap()
    );
    cx.simulate_window_fullscreen(any, true);
    assert!(
        window
            .update(cx, |_, window, _| window.is_fullscreen())
            .unwrap()
    );
    cx.simulate_window_resize(any, size(px(1000.0), px(700.0)));
    cx.simulate_window_move(any, point(px(5.0), px(5.0)));

    let observed_in_special_modes = observed.borrow().clone();
    assert!(observed_in_special_modes.len() >= 4);
    for window_bounds in &observed_in_special_modes {
        assert!(
            matches!(
                window_bounds,
                WindowBounds::Maximized(restore) | WindowBounds::Fullscreen(restore)
                    if *restore == ordinary
            ),
            "{window_bounds:?} must carry the unchanged ordinary bounds"
        );
    }
    assert_eq!(cx.window_ordinary_bounds(any), ordinary);

    // Leaving fullscreen returns the window to the ordinary bounds.
    cx.simulate_window_fullscreen(any, false);
    assert_eq!(cx.window_mode(any), WindowMode::Windowed);
    assert_eq!(
        window
            .update(cx, |_, window, _| window.window_bounds())
            .unwrap(),
        WindowBounds::Windowed(ordinary)
    );

    // Ordinary bounds follow resizes and moves made while windowed.
    let resized = size(px(900.0), px(650.0));
    cx.simulate_window_resize(any, resized);
    cx.simulate_window_move(any, point(px(40.0), px(50.0)));
    assert_eq!(
        window
            .update(cx, |_, window, _| window.window_bounds())
            .unwrap(),
        WindowBounds::Windowed(Bounds::new(point(px(40.0), px(50.0)), resized))
    );
}

#[vespertine::test]
fn persisted_triple_is_restored_on_a_fresh_window(cx: &mut TestAppContext) {
    let second_uuid = connect_second_display(cx);
    let ordinary = bounds(2000.0, 100.0, 700.0, 500.0);

    for (mode, expect_work_area) in [
        (WindowMode::Maximized, true),
        (WindowMode::Fullscreen, false),
        (WindowMode::Windowed, false),
    ] {
        // The application persisted (state, ordinary bounds, display uuid).
        let saved = SavedWindowGeometry {
            mode,
            bounds: bounds(80.0, 100.0, 700.0, 500.0),
            display_uuid: Some(second_uuid),
        };
        let restored = cx.update(|cx| {
            let displays = DisplayWorkArea::all(cx);
            restore_window_geometry(
                Some(&saved),
                &displays,
                cx.primary_display().map(|d| d.id()),
                None,
            )
        });
        assert_eq!(restored.display_id, Some(SECOND_DISPLAY));
        assert_eq!(restored.window_bounds, mode.with_bounds(ordinary));

        let window = open(
            cx,
            WindowOptions {
                window_bounds: Some(restored.window_bounds),
                display_id: restored.display_id,
                ..Default::default()
            },
        );
        let any: AnyWindowHandle = window.into();
        assert_eq!(cx.window_mode(any), mode);
        assert_eq!(cx.window_ordinary_bounds(any), ordinary);
        let (window_bounds, current) = window
            .update(cx, |_, window, _| (window.window_bounds(), window.bounds()))
            .unwrap();
        assert_eq!(window_bounds, mode.with_bounds(ordinary));
        if expect_work_area {
            assert_eq!(current, bounds(1920.0, 30.0, 1280.0, 994.0));
        }

        // Leaving the state returns to the ordinary bounds.
        cx.simulate_window_maximize(any, false);
        cx.simulate_window_fullscreen(any, false);
        assert_eq!(
            window.update(cx, |_, window, _| window.bounds()).unwrap(),
            ordinary
        );
    }
}

// --- A4: close interception -------------------------------------------------------------

#[vespertine::test]
fn vetoed_close_keeps_the_window_intact_until_a_bypassing_close_after_a_timer_save(
    cx: &mut TestAppContext,
) {
    let window = open_windowed(
        cx,
        WindowBounds::Windowed(bounds(100.0, 100.0, 640.0, 480.0)),
    );
    let any: AnyWindowHandle = window.into();
    let saved = Rc::new(Cell::new(false));
    let consulted = Rc::new(Cell::new(0));
    window
        .update(cx, |_, window, cx| {
            let (saved, consulted) = (saved.clone(), consulted.clone());
            window.on_window_should_close(cx, move |window, cx| {
                consulted.set(consulted.get() + 1);
                // Display information comes from the window and cached values only.
                let _ = window.display(cx);
                let _ = window.bounds();
                saved.get()
            });
        })
        .unwrap();
    let before = window
        .update(cx, |_, window, _| {
            (
                window.bounds(),
                window.viewport_size(),
                window.window_bounds(),
            )
        })
        .unwrap();

    // The first close request is vetoed and starts a save that takes one second.
    assert!(!cx.simulate_window_close_request(any));
    assert_eq!(consulted.get(), 1);
    assert_eq!(cx.windows(), vec![any]);
    let after = window
        .update(cx, |_, window, _| {
            (
                window.bounds(),
                window.viewport_size(),
                window.window_bounds(),
            )
        })
        .unwrap();
    assert_eq!(before, after, "size and state are intact");

    cx.update(|cx| {
        let saved = saved.clone();
        let timer = cx.background_executor().timer(Duration::from_secs(1));
        cx.spawn(async move |cx| {
            timer.await;
            saved.set(true);
            // The save finished: issue the second close request.
            cx.update_window(any, |_, window, cx| window.request_close(cx))
                .unwrap();
        })
        .detach();
    });
    cx.run_until_parked();
    assert_eq!(cx.windows(), vec![any], "the save is still running");
    cx.background_executor.advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    assert_eq!(
        consulted.get(),
        2,
        "the second request consulted the decision"
    );
    assert!(
        cx.windows().is_empty(),
        "the bypassing close removed the window"
    );
}

#[vespertine::test]
fn cancelled_save_leaves_the_window_open(cx: &mut TestAppContext) {
    let window = open_windowed(cx, WindowBounds::Windowed(bounds(0.0, 0.0, 400.0, 300.0)));
    let any: AnyWindowHandle = window.into();
    window
        .update(cx, |_, window, cx| {
            window.on_window_should_close(cx, |_, _| false);
        })
        .unwrap();
    assert!(!cx.simulate_window_close_request(any));
    // The user cancels the save prompt: nothing further is issued.
    cx.run_until_parked();
    cx.background_executor
        .advance_clock(Duration::from_secs(10));
    cx.run_until_parked();
    assert_eq!(cx.windows(), vec![any]);
}

#[vespertine::test]
fn request_close_without_a_decision_removes_the_window_and_remove_window_bypasses_it(
    cx: &mut TestAppContext,
) {
    let vetoing = open_windowed(cx, WindowBounds::Windowed(bounds(0.0, 0.0, 400.0, 300.0)));
    let plain = open_windowed(cx, WindowBounds::Windowed(bounds(0.0, 0.0, 400.0, 300.0)));
    vetoing
        .update(cx, |_, window, cx| {
            window.on_window_should_close(cx, |_, _| false)
        })
        .unwrap();
    assert!(
        !vetoing
            .update(cx, |_, window, cx| window.request_close(cx))
            .unwrap()
    );
    assert_eq!(cx.windows().len(), 2);
    vetoing
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
    assert!(
        plain
            .update(cx, |_, window, cx| window.request_close(cx))
            .unwrap()
    );
    assert!(cx.windows().is_empty());
}

#[vespertine::test]
fn allowed_close_removes_the_window_without_enumerating_displays(cx: &mut TestAppContext) {
    let window = open_windowed(cx, WindowBounds::Windowed(bounds(0.0, 0.0, 400.0, 300.0)));
    let any: AnyWindowHandle = window.into();
    let removed = Rc::new(Cell::new(0));
    cx.update(|cx| {
        let removed = removed.clone();
        cx.on_window_closed(move |_, _| removed.set(removed.get() + 1))
            .detach();
    });
    window
        .update(cx, |_, window, cx| {
            window.on_window_should_close(cx, |window, cx| {
                window.display(cx);
                true
            })
        })
        .unwrap();
    assert!(cx.simulate_window_close_request(any));
    assert_eq!(removed.get(), 1);
    assert!(cx.windows().is_empty());
}

/// Runs `body`, which must panic with a message containing `expected`.
fn expect_panic(expected: &str, body: impl FnOnce()) {
    let payload = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body))
        .expect_err("the test double must fail the test");
    let message = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or_default();
    assert!(
        message.contains(expected),
        "unexpected panic message: {message}"
    );
}

#[vespertine::test]
fn enumerating_displays_inside_the_close_callback_fails_the_test(cx: &mut TestAppContext) {
    let window = open_windowed(cx, WindowBounds::Windowed(bounds(0.0, 0.0, 400.0, 300.0)));
    window
        .update(cx, |_, window, cx| {
            window.on_window_should_close(cx, |_, cx| {
                cx.displays();
                true
            })
        })
        .unwrap();
    expect_panic(
        "displays were enumerated while a window close request",
        || {
            cx.simulate_window_close_request(window.into());
        },
    );
}

#[vespertine::test]
fn enumerating_displays_in_the_removal_path_fails_the_test(cx: &mut TestAppContext) {
    let window = open_windowed(cx, WindowBounds::Windowed(bounds(0.0, 0.0, 400.0, 300.0)));
    cx.update(|cx| {
        cx.on_window_closed(|cx, _| {
            cx.primary_display();
        })
        .detach();
    });
    expect_panic(
        "displays were enumerated while a window close request",
        || {
            cx.simulate_window_close_request(window.into());
        },
    );
}

// --- A5: quit interception --------------------------------------------------------------

/// Registers a quit decision that answers `answer` after `delay`; returns how often it was
/// consulted.
fn register_quit_decision(
    cx: &mut TestAppContext,
    delay: Duration,
    answer: bool,
) -> Rc<Cell<usize>> {
    let consulted = Rc::new(Cell::new(0));
    cx.update(|cx| {
        let consulted = consulted.clone();
        cx.on_quit_request(move |cx: &mut App| {
            consulted.set(consulted.get() + 1);
            let timer = cx.background_executor().timer(delay);
            async move {
                timer.await;
                answer
            }
        })
        .detach();
    });
    consulted
}

#[vespertine::test]
fn quit_decision_answered_after_a_delay_keeps_windows_open_until_answered(cx: &mut TestAppContext) {
    open_windowed(cx, WindowBounds::Windowed(bounds(0.0, 0.0, 400.0, 300.0)));
    open_windowed(cx, WindowBounds::Windowed(bounds(0.0, 0.0, 400.0, 300.0)));
    let consulted = register_quit_decision(cx, Duration::from_secs(2), true);

    cx.update(|cx| cx.quit());
    cx.run_until_parked();
    assert_eq!(consulted.get(), 1);
    assert_eq!(
        cx.windows().len(),
        2,
        "windows stay open while the decision is pending"
    );
    assert_eq!(cx.platform_quit_count(), 0);

    cx.background_executor.advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    assert_eq!(cx.windows().len(), 2);
    assert_eq!(cx.platform_quit_count(), 0);

    cx.background_executor.advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    assert!(cx.windows().is_empty());
    assert_eq!(
        cx.platform_quit_count(),
        1,
        "the event loop is stopped once"
    );
}

#[vespertine::test]
fn denied_quit_leaves_everything_untouched(cx: &mut TestAppContext) {
    open_windowed(cx, WindowBounds::Windowed(bounds(0.0, 0.0, 400.0, 300.0)));
    let handlers_ran = Rc::new(Cell::new(0));
    let _quit_handler = cx.update(|cx| {
        let handlers_ran = handlers_ran.clone();
        cx.on_app_quit(move |_| {
            handlers_ran.set(handlers_ran.get() + 1);
            async {}
        })
    });
    let consulted = register_quit_decision(cx, Duration::from_millis(500), false);

    cx.update(|cx| cx.quit());
    cx.background_executor
        .advance_clock(Duration::from_millis(500));
    cx.run_until_parked();
    assert_eq!(consulted.get(), 1);
    assert_eq!(cx.windows().len(), 1);
    assert_eq!(handlers_ran.get(), 0);
    assert_eq!(cx.platform_quit_count(), 0);

    // A denied quit can be requested again.
    cx.update(|cx| cx.quit());
    cx.background_executor
        .advance_clock(Duration::from_millis(500));
    cx.run_until_parked();
    assert_eq!(consulted.get(), 2);
}

#[vespertine::test]
fn quit_requests_while_a_decision_is_pending_are_coalesced(cx: &mut TestAppContext) {
    let consulted = register_quit_decision(cx, Duration::from_secs(1), false);
    cx.update(|cx| cx.quit());
    cx.update(|cx| cx.quit());
    cx.run_until_parked();
    cx.update(|cx| cx.quit());
    cx.background_executor.advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    assert_eq!(consulted.get(), 1, "three requests produced one decision");
}

#[vespertine::test]
fn allowed_quit_runs_app_quit_handlers_once_then_stops_the_loop(cx: &mut TestAppContext) {
    open_windowed(cx, WindowBounds::Windowed(bounds(0.0, 0.0, 400.0, 300.0)));
    let handlers_ran = Rc::new(Cell::new(0));
    let _quit_handler = cx.update(|cx| {
        let handlers_ran = handlers_ran.clone();
        cx.on_app_quit(move |_| {
            handlers_ran.set(handlers_ran.get() + 1);
            async {}
        })
    });
    register_quit_decision(cx, Duration::ZERO, true);
    cx.update(|cx| cx.quit());
    cx.update(|cx| cx.quit());
    cx.run_until_parked();
    assert_eq!(handlers_ran.get(), 1);
    assert_eq!(cx.platform_quit_count(), 1);
    // The platform's own exit path does not run them again.
    cx.update(|cx| cx.shutdown());
    assert_eq!(handlers_ran.get(), 1);
}

#[vespertine::test]
fn forced_quit_skips_the_decision(cx: &mut TestAppContext) {
    let consulted = register_quit_decision(cx, Duration::from_secs(1), false);
    cx.update(|cx| cx.force_quit());
    cx.run_until_parked();
    assert_eq!(consulted.get(), 0);
    assert_eq!(cx.platform_quit_count(), 1);
}

#[vespertine::test]
fn quit_without_a_decision_goes_straight_to_the_platform(cx: &mut TestAppContext) {
    cx.update(|cx| cx.quit());
    assert_eq!(cx.platform_quit_count(), 1);
}

#[vespertine::test]
fn session_termination_goes_through_the_quit_decision(cx: &mut TestAppContext) {
    open_windowed(cx, WindowBounds::Windowed(bounds(0.0, 0.0, 400.0, 300.0)));
    let consulted = register_quit_decision(cx, Duration::from_secs(1), true);
    cx.simulate_quit_request();
    cx.simulate_quit_request();
    cx.run_until_parked();
    assert_eq!(consulted.get(), 1);
    assert_eq!(cx.windows().len(), 1);
    cx.background_executor.advance_clock(Duration::from_secs(1));
    cx.run_until_parked();
    assert!(cx.windows().is_empty());
    assert_eq!(cx.platform_quit_count(), 1);
}

// --- A6: retained lifecycle -------------------------------------------------------------

#[vespertine::test]
fn with_zero_windows_the_loop_keeps_running_until_an_explicit_quit(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_quit_mode(QuitMode::Explicit));
    let window = open_windowed(cx, WindowBounds::Windowed(bounds(0.0, 0.0, 400.0, 300.0)));
    assert!(cx.simulate_window_close_request(window.into()));
    assert!(cx.windows().is_empty());
    assert_eq!(
        cx.platform_quit_count(),
        0,
        "the loop keeps running without windows"
    );
    cx.update(|cx| cx.quit());
    assert_eq!(cx.platform_quit_count(), 1);
}

#[vespertine::test]
fn last_window_closed_mode_still_quits_when_requested(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_quit_mode(QuitMode::LastWindowClosed));
    let window = open_windowed(cx, WindowBounds::Windowed(bounds(0.0, 0.0, 400.0, 300.0)));
    cx.simulate_window_close_request(window.into());
    assert_eq!(cx.platform_quit_count(), 1);
}

struct Retained(u32);

impl crate::Global for Retained {}

#[vespertine::test]
fn activate_recreates_the_main_window_while_retained_state_is_unchanged(cx: &mut TestAppContext) {
    cx.update(|cx| {
        cx.set_quit_mode(QuitMode::Explicit);
        cx.set_global(Retained(41));
        cx.on_reopen(|cx| {
            if cx.windows().is_empty() {
                cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| Blank))
                    .unwrap();
            }
        });
    });
    let window = open(cx, WindowOptions::default());
    assert!(cx.simulate_window_close_request(window.into()));
    assert!(cx.windows().is_empty());

    cx.simulate_reopen();
    assert_eq!(cx.windows().len(), 1, "the main window was recreated");
    assert_eq!(cx.read(|cx| cx.global::<Retained>().0), 41);

    // Activating a running instance with a window does not open another one.
    cx.simulate_reopen();
    assert_eq!(cx.windows().len(), 1);
    assert_eq!(cx.platform_quit_count(), 0);
}

#[vespertine::test]
fn open_delivers_the_exact_url_strings_once_in_order(cx: &mut TestAppContext) {
    let received = Rc::new(RefCell::new(Vec::new()));
    cx.update(|cx| {
        let received = received.clone();
        cx.on_open_urls(move |urls, _| received.borrow_mut().push(urls));
    });
    let urls = vec![
        "HTTPS://Example.com/a b?q=%7e#frag".to_owned(),
        "myapp:open?id=7".to_owned(),
    ];
    cx.simulate_open_urls(urls.clone());
    assert_eq!(*received.borrow(), vec![urls]);

    cx.simulate_open_urls(vec!["second://one".to_owned()]);
    assert_eq!(received.borrow().len(), 2);
    assert_eq!(received.borrow()[1], vec!["second://one".to_owned()]);
}

#[vespertine::test]
fn launch_urls_wait_for_the_handler_and_are_delivered_exactly_once(cx: &mut TestAppContext) {
    // What the first launch queues from the command line, before any handler exists.
    cx.update(|cx| cx.deliver_open_urls(vec!["first://launch".into(), "second://launch".into()]));
    cx.simulate_open_urls(vec!["early://runtime".into()]);

    let received = Rc::new(RefCell::new(Vec::new()));
    assert!(
        received.borrow().is_empty(),
        "nothing is delivered before registration"
    );
    cx.update(|cx| {
        let received = received.clone();
        cx.on_open_urls(move |urls, _| received.borrow_mut().push(urls));
    });
    assert_eq!(
        *received.borrow(),
        vec![
            vec!["first://launch".to_owned(), "second://launch".to_owned()],
            vec!["early://runtime".to_owned()],
        ]
    );

    // Registering again does not redeliver them.
    cx.update(|cx| {
        let received = received.clone();
        cx.on_open_urls(move |urls, _| received.borrow_mut().push(urls));
    });
    assert_eq!(received.borrow().len(), 2);
}

#[test]
fn launch_urls_are_the_url_shaped_command_line_arguments_exactly_as_given() {
    let args = [
        "--flag",
        "myapp://open/thing",
        "plain-name",
        "C:/not-a-url-scheme-of-one-letter",
        "module::path",
        "mailto:someone@example.com",
        "https://example.com/a%20b",
        "--url=https://flag.example",
    ];
    assert_eq!(
        launch_urls_from_args(args),
        [
            "myapp://open/thing",
            "mailto:someone@example.com",
            "https://example.com/a%20b"
        ]
    );
}

// --- A7: activation tokens --------------------------------------------------------------

#[vespertine::test]
fn activation_request_with_a_token_raises_the_window_with_that_token(cx: &mut TestAppContext) {
    let window = open_windowed(cx, WindowBounds::Windowed(bounds(0.0, 0.0, 400.0, 300.0)));
    let any: AnyWindowHandle = window.into();
    cx.simulate_window_minimize(any);
    assert!(cx.is_window_minimized(any));

    cx.simulate_activation_request(ActivationRequest {
        token: Some("xdg-token-1".into()),
    });
    assert_eq!(
        cx.window_activations(any),
        [TestActivation::WithToken("xdg-token-1".into())]
    );
    assert!(!cx.is_window_minimized(any), "activation also un-minimizes");
}

#[vespertine::test]
fn activation_request_without_a_token_requests_attention_and_does_not_force_focus(
    cx: &mut TestAppContext,
) {
    let window = open_windowed(cx, WindowBounds::Windowed(bounds(0.0, 0.0, 400.0, 300.0)));
    let any: AnyWindowHandle = window.into();
    cx.simulate_activation_request(ActivationRequest { token: None });
    assert_eq!(cx.window_activations(any), [TestActivation::Attention]);
    assert_eq!(
        cx.update(|cx| cx.active_window()),
        None,
        "focus was not taken"
    );
}

#[vespertine::test]
fn activation_targets_the_most_recently_opened_window_when_none_is_active(cx: &mut TestAppContext) {
    let first: AnyWindowHandle =
        open_windowed(cx, WindowBounds::Windowed(bounds(0.0, 0.0, 400.0, 300.0))).into();
    let second: AnyWindowHandle =
        open_windowed(cx, WindowBounds::Windowed(bounds(0.0, 0.0, 400.0, 300.0))).into();
    cx.simulate_activation_request(ActivationRequest {
        token: Some("t".into()),
    });
    assert!(cx.window_activations(first).is_empty());
    assert_eq!(
        cx.window_activations(second),
        [TestActivation::WithToken("t".into())]
    );
}

#[vespertine::test]
fn activation_request_without_windows_does_nothing(cx: &mut TestAppContext) {
    let activated = cx.update(|cx| cx.activate_app(&ActivationRequest::default()));
    assert!(!activated);
}

#[vespertine::test]
fn activated_action_reaches_its_handler_with_the_name(cx: &mut TestAppContext) {
    let names = Rc::new(RefCell::new(Vec::new()));
    cx.update(|cx| {
        let names = names.clone();
        cx.on_activate_action(move |name, _| names.borrow_mut().push(name.to_owned()));
    });
    cx.simulate_activate_action("new-window");
    assert_eq!(*names.borrow(), ["new-window"]);
}

#[vespertine::test]
fn registering_a_reopen_handler_retains_the_app_under_the_default_quit_mode(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| cx.on_reopen(|_| {}));
    let window = open_windowed(cx, WindowBounds::Windowed(bounds(0.0, 0.0, 400.0, 300.0)));
    cx.simulate_window_close_request(window.into());
    assert!(cx.windows().is_empty());
    assert_eq!(cx.platform_quit_count(), 0);
}
