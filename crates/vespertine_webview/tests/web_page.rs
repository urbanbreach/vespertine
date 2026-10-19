//! UI-level tests of the embedded page: pointer, wheel, focus, presentation gating, overlay
//! precedence, text-input handler and frames, against an in-process scripted helper.

use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use vespertine::Bounds;
use vespertine::{
    AppContext, Context, Entity, EntityInputHandler, FocusHandle, InteractiveElement, IntoElement,
    Modifiers, MouseButton, ParentElement, Pixels, Render, ScrollDelta, ScrollWheelEvent, Styled,
    TestAppContext, TouchPhase, VisualTestContext, Window, div, point, px, size,
};
use vespertine_webview::protocol::{Command, encode_inline_frame, kind};
use vespertine_webview::testing::{Peer, wait_until};
use vespertine_webview::{
    BrowserContext, Presentation, WebPage, WebPageEvent, frame_placement, page_element,
};

struct Root {
    page: Entity<WebPage>,
    other: FocusHandle,
    overlay: bool,
    size: (f32, f32),
}

impl Render for Root {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut root = div().size_full().child(
            div()
                .absolute()
                .left(px(40.))
                .top(px(80.))
                .w(px(self.size.0))
                .h(px(self.size.1))
                .child(page_element(&self.page, cx)),
        );
        root = root.child(div().id("other").track_focus(&self.other).size(px(1.)));
        if self.overlay {
            root = root.child(
                div()
                    .id("overlay")
                    .absolute()
                    .left(px(200.))
                    .top(px(100.))
                    .w(px(100.))
                    .h(px(100.))
                    .occlude(),
            );
        }
        root
    }
}

struct Fixture {
    root: Entity<Root>,
    page: Entity<WebPage>,
    peer: Peer,
    _helper: Arc<vespertine_webview::Helper>,
}

fn setup(cx: &mut TestAppContext, presentation: Presentation) -> (Fixture, &mut VisualTestContext) {
    cx.executor().allow_parking();
    let (helper, peer) = Peer::start();
    let native = helper.new_page().unwrap();
    let (root, vcx) = cx.add_window_view(|window, cx| {
        let page = cx.new(|cx| WebPage::new(BrowserContext::ephemeral(), window, cx));
        Root {
            page,
            other: cx.focus_handle(),
            overlay: false,
            size: (300., 200.),
        }
    });
    let page = root.read_with(vcx, |root, _| root.page.clone());
    vcx.update(|window, cx| {
        page.update(cx, |page, cx| {
            page.adopt(native, window, cx);
            page.set_presentation(presentation, window, cx);
        });
    });
    vcx.run_until_parked();
    vcx.update(|window, _| {
        window.activate_window();
        window.refresh();
    });
    vcx.run_until_parked();
    (
        Fixture {
            root,
            page,
            peer,
            _helper: helper,
        },
        vcx,
    )
}

fn names(peer: &Peer, wanted: &[&str]) -> Vec<Value> {
    peer.commands()
        .into_iter()
        .map(|(_, command)| command)
        .filter(|command| wanted.iter().any(|name| command["cmd"] == *name))
        .collect()
}

/// Runs the foreground executor until `condition` holds (events from the helper arrive through
/// tasks) or five seconds passed.
fn pump_until(cx: &mut VisualTestContext, mut condition: impl FnMut() -> bool) -> bool {
    wait_until(Duration::from_secs(5), || {
        cx.run_until_parked();
        condition()
    })
}

/// Draws a frame so focus changes reach their listeners.
fn redraw(cx: &mut VisualTestContext) {
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn settle(cx: &mut VisualTestContext) {
    std::thread::sleep(Duration::from_millis(100));
    cx.run_until_parked();
}

#[vespertine::test]
fn a_press_over_the_page_sends_element_relative_coordinates(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx, Presentation::Live);
    // Element at (40, 80): a press at (130, 90) is at (90, 10) in page coordinates; the helper
    // multiplies by the page scale (the test window's scale factor).
    cx.simulate_mouse_down(
        point(px(130.), px(90.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    let downs = fixture.peer.wait_named("down", 1);
    assert_eq!(downs[0]["x"], 90.0);
    assert_eq!(downs[0]["y"], 10.0);
    assert_eq!(downs[0]["button"], 1);
    assert_eq!(downs[0]["mods"], 0);
    cx.simulate_mouse_up(
        point(px(130.), px(90.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    fixture.peer.wait_named("up", 1);
    // Pressing on the page focuses it and notifies the page.
    let focused = fixture
        .page
        .read_with(cx, |page, _| page.focus_handle().clone());
    cx.update(|window, _| assert!(focused.is_focused(window)));
    redraw(cx);
    assert!(pump_until(cx, || !fixture.peer.named("focus").is_empty()));
    assert_eq!(fixture.peer.named("focus")[0]["value"], 1);
}

#[vespertine::test]
fn wheel_events_are_converted_to_smooth_scroll_units(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx, Presentation::Live);
    let position = point(px(130.), px(90.));
    cx.simulate_event(ScrollWheelEvent {
        position,
        delta: ScrollDelta::Pixels(point(px(0.), px(-250.))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
    });
    cx.simulate_event(ScrollWheelEvent {
        position,
        delta: ScrollDelta::Lines(point(0., -1.)),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
    });
    let scrolls = fixture.peer.wait_named("scroll", 2);
    assert_eq!(scrolls[0]["dy"], 6.25);
    assert_eq!(scrolls[0]["dx"], 0.0);
    assert_eq!(scrolls[0]["x"], 90.0);
    assert_eq!(scrolls[1]["dy"], 0.4);
}

#[vespertine::test]
fn press_ownership_and_capture(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx, Presentation::Live);
    let inside = point(px(130.), px(90.));
    let outside = point(px(700.), px(500.));
    cx.simulate_mouse_down(inside, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_down(inside, MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(inside, MouseButton::Right, Modifiers::none());
    cx.simulate_mouse_up(inside, MouseButton::Left, Modifiers::none());
    // An up without a matching down is dropped.
    cx.simulate_mouse_up(inside, MouseButton::Middle, Modifiers::none());
    settle(cx);
    let buttons: Vec<(String, i64)> = names(&fixture.peer, &["down", "up"])
        .into_iter()
        .map(|c| {
            (
                c["cmd"].as_str().unwrap().to_owned(),
                c["button"].as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        buttons,
        [
            ("down".into(), 1),
            ("down".into(), 3),
            ("up".into(), 3),
            ("up".into(), 1)
        ]
    );
    // A back-button press is consumed and sends nothing.
    cx.simulate_mouse_down(
        inside,
        MouseButton::Navigate(vespertine::NavigationDirection::Back),
        Modifiers::none(),
    );
    settle(cx);
    assert_eq!(fixture.peer.named("down").len(), 2);
    // Moves outside without a held button are not forwarded; with a held button they are.
    cx.simulate_mouse_move(outside, None, Modifiers::none());
    settle(cx);
    assert!(fixture.peer.named("move").is_empty());
    cx.simulate_mouse_down(inside, MouseButton::Left, Modifiers::none());
    cx.simulate_mouse_move(outside, Some(MouseButton::Left), Modifiers::none());
    let moves = fixture.peer.wait_named("move", 1);
    assert_eq!(moves[0]["mods"], 256);
    assert_eq!(moves[0]["x"], 660.0);
    assert_eq!(moves[0]["y"], 420.0);
}

#[vespertine::test]
fn passthrough_and_hidden_forward_nothing(cx: &mut TestAppContext) {
    for presentation in [Presentation::Passthrough, Presentation::Hidden] {
        let (fixture, cx) = setup(cx, presentation);
        let inside = point(px(130.), px(90.));
        cx.simulate_mouse_down(inside, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_move(inside, None, Modifiers::none());
        cx.simulate_event(ScrollWheelEvent {
            position: inside,
            delta: ScrollDelta::Pixels(point(px(0.), px(-10.))),
            modifiers: Modifiers::none(),
            touch_phase: TouchPhase::Moved,
        });
        cx.simulate_keystrokes("a");
        cx.simulate_mouse_up(inside, MouseButton::Left, Modifiers::none());
        settle(cx);
        let forwarded = names(
            &fixture.peer,
            &[
                "move", "down", "up", "scroll", "leave", "key_down", "key_up", "commit", "text",
            ],
        );
        assert!(forwarded.is_empty(), "{presentation:?}: {forwarded:?}");
    }
}

#[vespertine::test]
fn leaving_live_while_a_button_is_held_releases_it(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx, Presentation::Live);
    let inside = point(px(130.), px(90.));
    cx.simulate_mouse_down(inside, MouseButton::Left, Modifiers::none());
    fixture.peer.wait_named("down", 1);
    let visible_before = fixture.peer.named("visible").len();
    cx.update(|window, app| {
        fixture.page.update(app, |page, cx| {
            page.set_presentation(Presentation::Hidden, window, cx)
        })
    });
    let ups = fixture.peer.wait_named("up", 1);
    assert_eq!(ups[0]["button"], 1);
    fixture.peer.wait_named("leave", 1);
    fixture.peer.assert_no_more("up", 1);
    fixture.peer.assert_no_more("leave", 1);
    // "visible 0" was sent exactly once for the change.
    let visible = fixture.peer.named("visible");
    assert_eq!(visible.len(), visible_before + 1);
    assert_eq!(visible.last().unwrap()["value"], 0);
}

#[vespertine::test]
fn a_blocking_overlay_prevents_events_and_ends_the_hover(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx, Presentation::Live);
    fixture.root.update(cx, |root, cx| {
        root.overlay = true;
        cx.notify();
    });
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    // Hover the page first, then enter the overlay (page area 40..340 x 80..280; overlay
    // 200..300 x 100..200).
    cx.simulate_mouse_move(point(px(100.), px(100.)), None, Modifiers::none());
    fixture.peer.wait_named("move", 1);
    cx.simulate_mouse_move(point(px(250.), px(150.)), None, Modifiers::none());
    fixture.peer.wait_named("leave", 1);
    cx.simulate_mouse_down(
        point(px(250.), px(150.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    cx.simulate_event(ScrollWheelEvent {
        position: point(px(250.), px(150.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(-10.))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
    });
    settle(cx);
    assert!(fixture.peer.named("down").is_empty());
    assert!(fixture.peer.named("scroll").is_empty());
    assert_eq!(fixture.peer.named("move").len(), 1);
    // Uncovered parts of the page still receive input.
    cx.simulate_mouse_down(
        point(px(60.), px(260.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    fixture.peer.wait_named("down", 1);
}

#[vespertine::test]
fn focus_moves_notify_the_page(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx, Presentation::Live);
    let page_handle = fixture
        .page
        .read_with(cx, |page, _| page.focus_handle().clone());
    let other = fixture.root.read_with(cx, |root, _| root.other.clone());
    cx.update(|window, app| window.focus(&page_handle, app));
    redraw(cx);
    assert_eq!(fixture.peer.wait_named("focus", 1)[0]["value"], 1);
    // Composition in progress is cancelled when the page loses focus.
    cx.update(|window, app| {
        fixture.page.update(app, |page, cx| {
            page.replace_and_mark_text_in_range(None, "に", None, window, cx)
        })
    });
    cx.update(|window, app| window.focus(&other, app));
    redraw(cx);
    let focus = fixture.peer.wait_named("focus", 2);
    assert_eq!(focus[1]["value"], 0);
    fixture.peer.wait_named("unmark", 1);
    // Keys are no longer forwarded while the page is not focused.
    cx.simulate_keystrokes("b");
    settle(cx);
    assert!(fixture.peer.named("key_down").is_empty());
    // Pressing on the page returns focus to it.
    cx.update(|window, app| window.focus(&page_handle, app));
    redraw(cx);
    fixture.peer.wait_named("focus", 3);
    cx.simulate_keystrokes("b");
    fixture.peer.wait_named("key_down", 1);
}

#[vespertine::test]
fn keys_are_forwarded_with_names_and_modifiers(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx, Presentation::Live);
    let page_handle = fixture
        .page
        .read_with(cx, |page, _| page.focus_handle().clone());
    cx.update(|window, app| window.focus(&page_handle, app));
    cx.simulate_keystrokes("enter f5 ctrl-shift-left");
    let downs = fixture.peer.wait_named("key_down", 3);
    assert_eq!(downs[0]["key"], "Return");
    assert_eq!(downs[1]["key"], "F5");
    assert_eq!(downs[2]["key"], "Left");
    assert_eq!(downs[2]["mods"], 1 + 4);
    cx.simulate_keystrokes("a");
    let downs = fixture.peer.wait_named("key_down", 4);
    assert_eq!(downs[3]["key"], "a");
    assert_eq!(downs[3]["text"], "a");
}

#[vespertine::test]
fn typed_text_is_committed_when_the_page_has_an_editable_focus(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx, Presentation::Live);
    let page_handle = fixture
        .page
        .read_with(cx, |page, _| page.focus_handle().clone());
    cx.update(|window, app| window.focus(&page_handle, app));
    fixture
        .peer
        .send_json(kind::INPUT_METHOD, 1, json!({"focused": true}));
    assert!(wait_until(Duration::from_secs(5), || {
        cx.run_until_parked();
        fixture.page.read_with(cx, |page, _| {
            page.native_page().unwrap().input_state().focused
        })
    }));
    cx.simulate_keystrokes("h");
    let commits = fixture.peer.wait_named("commit", 1);
    assert_eq!(commits[0]["text"], "h");
    assert!(fixture.peer.named("key_down").is_empty());
}

#[vespertine::test]
fn the_text_input_handler_answers_from_the_stored_state(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx, Presentation::Live);
    // Accepts text input only after a focused message.
    let accepts = |cx: &mut VisualTestContext, page: &Entity<WebPage>| {
        cx.update(|window, app| page.update(app, |page, cx| page.accepts_text_input(window, cx)))
    };
    assert!(!accepts(cx, &fixture.page));
    fixture.peer.send_json(
        kind::INPUT_METHOD,
        1,
        json!({"focused": true, "caret": [20, 40, 2, 18], "text": "aé日", "cursor": 3, "selection": 3}),
    );
    assert!(wait_until(Duration::from_secs(5), || {
        cx.run_until_parked();
        accepts(cx, &fixture.page)
    }));
    let (selected, text, adjusted, bounds, index) = cx.update(|window, app| {
        fixture.page.update(app, |page, cx| {
            let selected = page.selected_text_range(true, window, cx).unwrap();
            let mut adjusted = None;
            let text = page.text_for_range(1..3, &mut adjusted, window, cx);
            let mut clamped = None;
            let _ = page.text_for_range(1..300, &mut clamped, window, cx);
            let bounds = page.bounds_for_range(
                0..0,
                vespertine::Bounds {
                    origin: point(px(10.), px(100.)),
                    size: size(px(300.), px(200.)),
                },
                window,
                cx,
            );
            let index = page.character_index_for_point(point(px(1.), px(1.)), window, cx);
            (selected, text, clamped, bounds, index)
        })
    });
    assert_eq!((selected.range, selected.reversed), (2..2, false));
    assert_eq!(text.as_deref(), Some("é日"));
    assert_eq!(adjusted, Some(1..3));
    assert_eq!(index, None);
    // The caret is in device pixels of the page: divided by the page scale.
    let scale = cx.update(|window, _| window.scale_factor());
    let bounds = bounds.unwrap();
    assert_eq!(
        bounds.origin,
        point(px(10. + 20. / scale), px(100. + 40. / scale))
    );
    assert_eq!(bounds.size, size(px(2. / scale), px(18. / scale)));
}

#[vespertine::test]
fn composition_marks_text_and_commit_clears_it(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx, Presentation::Live);
    cx.update(|window, app| {
        fixture.page.update(app, |page, cx| {
            page.replace_and_mark_text_in_range(None, "にほ", None, window, cx);
            page.replace_and_mark_text_in_range(None, "日本語", Some(0..0), window, cx);
            assert_eq!(page.marked_text_range(window, cx), Some(0..3));
        })
    });
    let preedits = fixture.peer.wait_named("preedit", 2);
    assert_eq!(preedits[0]["text"], "にほ");
    assert_eq!(preedits[1]["text"], "日本語");
    cx.update(|window, app| {
        fixture.page.update(app, |page, cx| {
            page.replace_text_in_range(None, "日本語", window, cx);
            assert_eq!(page.marked_text_range(window, cx), None);
            page.replace_and_mark_text_in_range(None, "x", None, window, cx);
            page.unmark_text(window, cx);
            assert_eq!(page.marked_text_range(window, cx), None);
        })
    });
    assert_eq!(fixture.peer.wait_named("commit", 1)[0]["text"], "日本語");
    fixture.peer.wait_named("unmark", 1);
}

#[vespertine::test]
fn frames_are_retained_latest_wins_and_painted(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx, Presentation::Live);
    // 100 frames while the UI is not looking leave only the last one.
    for index in 0..100u32 {
        let pixels: Vec<u8> = [index as u8, 0, 0, 255]
            .into_iter()
            .cycle()
            .take(4 * 4 * 4)
            .collect();
        fixture
            .peer
            .send(kind::FRAME, 1, &encode_inline_frame(4, 4, 1.5, &pixels));
    }
    assert!(wait_until(Duration::from_secs(5), || {
        cx.run_until_parked();
        fixture
            .page
            .read_with(cx, |page, _| page.frame_backing() == Some([0, 0, 99]))
    }));
    let (width, height, scale) = fixture
        .page
        .read_with(cx, |page, _| page.frame_size().unwrap());
    assert_eq!((width, height, scale), (4, 4, 1.5));
    // Invalid frames never replace the retained one.
    fixture.peer.send(
        kind::FRAME,
        1,
        &encode_inline_frame(4, 4, f32::NAN, &[0; 64]),
    );
    fixture.peer.send(kind::FRAME, 1, &[0; 11]);
    settle(cx);
    assert_eq!(
        fixture.page.read_with(cx, |page, _| page.frame_backing()),
        Some([0, 0, 99])
    );
}

#[vespertine::test]
fn painting_resizes_once_per_device_size(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx, Presentation::Live);
    let scale = cx.update(|window, _| window.scale_factor());
    // 300 x 200 logical at the window scale.
    let resizes = fixture.peer.wait_named("resize", 1);
    assert_eq!(resizes[0]["width"], (300.0 * scale).round());
    assert_eq!(resizes[0]["height"], (200.0 * scale).round());
    assert_eq!(resizes[0]["scale"], f64::from(scale));
    for _ in 0..3 {
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
    }
    fixture.peer.assert_no_more("resize", 1);
    let _ = (Pixels::ZERO, Command::Create);
}

#[vespertine::test]
fn page_menus_are_shown_only_while_live(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx, Presentation::Live);
    let received = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let sink = received.clone();
    let _subscription = cx.update(|_, app| {
        app.subscribe(&fixture.page, move |_, event: &WebPageEvent, _| {
            sink.lock().push(event.clone());
        })
    });
    let menu = json!({"x": 5.0, "y": 6.0, "items": [{"label": "Copy", "action": "copy", "enabled": true}]});
    fixture.peer.send_json(kind::MENU, 1, menu.clone());
    assert!(wait_until(Duration::from_secs(5), || {
        cx.run_until_parked();
        received
            .lock()
            .iter()
            .any(|event| matches!(event, WebPageEvent::Menu(_)))
    }));
    // Not live: the menu is dismissed immediately and nothing is shown.
    cx.update(|window, app| {
        fixture.page.update(app, |page, cx| {
            page.set_presentation(Presentation::Passthrough, window, cx)
        })
    });
    received.lock().clear();
    fixture.peer.send_json(kind::MENU, 1, menu);
    assert!(pump_until(cx, || !fixture
        .peer
        .named("dismiss-menu")
        .is_empty()));
    settle(cx);
    assert!(
        !received
            .lock()
            .iter()
            .any(|event| matches!(event, WebPageEvent::Menu(_)))
    );
    // New-tab requests are ignored when not live, accepted when live.
    fixture.peer.send(kind::NEW_TAB, 1, b"http://example.test/");
    settle(cx);
    assert!(
        !received
            .lock()
            .iter()
            .any(|event| matches!(event, WebPageEvent::NewTab(_)))
    );
    cx.update(|window, app| {
        fixture.page.update(app, |page, cx| {
            page.set_presentation(Presentation::Live, window, cx)
        })
    });
    fixture.peer.send(kind::NEW_TAB, 1, b"http://example.test/");
    assert!(wait_until(Duration::from_secs(5), || {
        cx.run_until_parked();
        received.lock().iter().any(
            |event| matches!(event, WebPageEvent::NewTab(url) if url == "http://example.test/"),
        )
    }));
}

#[vespertine::test]
fn clipboard_messages_write_the_system_clipboard_unless_empty(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx, Presentation::Live);
    cx.update(|_, app| {
        app.write_to_clipboard(vespertine::ClipboardItem::new_string("before".into()))
    });
    fixture.peer.send(kind::CLIPBOARD, 1, b"");
    settle(cx);
    let text = cx.update(|_, app| app.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(text.as_deref(), Some("before"));
    fixture
        .peer
        .send(kind::CLIPBOARD, 1, "café 日本語".as_bytes());
    assert!(wait_until(Duration::from_secs(5), || {
        cx.run_until_parked();
        cx.update(|_, app| app.read_from_clipboard().and_then(|item| item.text()))
            .as_deref()
            == Some("café 日本語")
    }));
}

#[vespertine::test]
fn paste_sends_clipboard_text_and_nothing_without_it(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx, Presentation::Live);
    let page_handle = fixture
        .page
        .read_with(cx, |page, _| page.focus_handle().clone());
    cx.update(|window, app| window.focus(&page_handle, app));
    cx.update(|_, app| {
        app.write_to_clipboard(vespertine::ClipboardItem::new_string(
            " — café 日本語".into(),
        ))
    });
    cx.simulate_keystrokes("ctrl-v");
    let texts = fixture.peer.wait_named("text", 1);
    assert_eq!(texts[0]["text"], " — café 日本語");
    assert!(fixture.peer.named("key_down").is_empty());
    cx.simulate_keystrokes("ctrl-c ctrl-x");
    fixture.peer.wait_named("copy", 1);
    fixture.peer.wait_named("cut", 1);
}

#[vespertine::test]
fn a_frame_keeps_its_physical_size_across_a_scale_change(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx, Presentation::Live);
    cx.simulate_scale_factor_change(1.5);
    redraw(cx);
    fixture.root.update(cx, |root, cx| {
        root.size = (640., 360.);
        cx.notify();
    });
    redraw(cx);
    // A 960 x 540 frame captured at scale 1.5.
    fixture.peer.send(
        kind::FRAME,
        1,
        &encode_inline_frame(960, 540, 1.5, &solid(960, 540, [30, 20, 10, 255])),
    );
    let page = fixture.page.clone();
    assert!(wait_until(Duration::from_secs(5), || {
        cx.run_until_parked();
        page.read_with(cx, |page, _| page.has_frame())
    }));
    redraw(cx);
    let element = Bounds {
        origin: point(px(40.), px(80.)),
        size: size(px(640.), px(360.)),
    };
    assert_eq!(
        frame_placement(element, 960, 540, 1.5).size,
        size(px(640.), px(360.))
    );
    // The backing fill is the colour of the frame's top-left pixel, over the element bounds.
    let scale = cx.update(|window, _| window.scale_factor());
    assert_eq!(scale, 1.5);
    let backing = cx.update(|window, _| window.painted_quads());
    let quad = backing
        .iter()
        .find(|quad| quad.bounds.size.width.as_f32() == 640. * 1.5)
        .expect("a backing quad over the element bounds");
    let colour = vespertine::Rgba::from(quad.background.as_solid().unwrap());
    assert_eq!(
        [
            (colour.r * 255.0).round() as u8,
            (colour.g * 255.0).round() as u8,
            (colour.b * 255.0).round() as u8
        ],
        [10, 20, 30]
    );
    // After the window scale becomes 2 and before a new frame arrives the old frame still covers
    // 640 x 360 logical pixels; one resize with 1280 x 720 at scale 2 was sent.
    cx.simulate_scale_factor_change(2.0);
    redraw(cx);
    let resizes = fixture.peer.wait_named("resize", 1);
    assert!(
        resizes
            .iter()
            .any(|r| r["width"] == 1280 && r["height"] == 720 && r["scale"] == 2.0)
    );
    let count_2x = resizes
        .iter()
        .filter(|r| r["width"] == 1280 && r["scale"] == 2.0)
        .count();
    assert_eq!(count_2x, 1);
    assert_eq!(
        fixture.page.read_with(cx, |page, _| page.frame_size()),
        Some((960, 540, 1.5))
    );
    assert_eq!(
        frame_placement(element, 960, 540, 1.5).size,
        size(px(640.), px(360.))
    );
    // The same frame at its own scale is never stretched to a different element size.
    let smaller = Bounds {
        size: size(px(100.), px(100.)),
        ..element
    };
    assert_eq!(
        frame_placement(smaller, 960, 540, 1.5).size,
        size(px(640.), px(360.))
    );
}

fn solid(width: u32, height: u32, bgra: [u8; 4]) -> Vec<u8> {
    bgra.into_iter()
        .cycle()
        .take(width as usize * height as usize * 4)
        .collect()
}
