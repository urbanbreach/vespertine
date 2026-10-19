//! Window-level tests for paint-scoped root key listeners (INP-005) and subset-constrained tab
//! traversal (INP-004).

use std::{cell::RefCell, rc::Rc};

use crate::{
    AppContext as _, Context, DispatchPhase, Entity, FocusHandle, FocusId, InteractiveElement as _,
    IntoElement, KeyBinding, KeyDownEvent, KeyUpEvent, Keystroke, ParentElement as _, Render,
    StyleRefinement, Styled as _, TestApp, TestAppWindow, canvas, div, px,
};

crate::actions!(inp005_tests, [Press]);

type Log = Rc<RefCell<Vec<String>>>;

fn phase_name(phase: DispatchPhase) -> &'static str {
    match phase {
        DispatchPhase::Capture => "capture",
        DispatchPhase::Bubble => "bubble",
    }
}

#[derive(Clone, Copy, PartialEq)]
enum StopAt {
    Never,
    RootCapture,
    PathBubble,
    RefreshAndRefocus,
}

/// A canvas that registers root key listeners (down and up) while painting.
fn root_listeners(
    name: &'static str,
    log: Log,
    stop: StopAt,
    other: Option<FocusHandle>,
) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |_, _, window, _| {
            let down_log = log.clone();
            let other = other.clone();
            window.on_root_key_event(move |_: &KeyDownEvent, phase, window, cx| {
                down_log
                    .borrow_mut()
                    .push(format!("{name}-root-{}", phase_name(phase)));
                if phase == DispatchPhase::Capture {
                    match stop {
                        StopAt::RootCapture => cx.stop_propagation(),
                        StopAt::RefreshAndRefocus => {
                            window.refresh();
                            if let Some(other) = &other {
                                other.focus(window, cx);
                            }
                        }
                        _ => {}
                    }
                }
            });
            let up_log = log.clone();
            window.on_root_key_event(move |_: &KeyUpEvent, phase, _, _| {
                up_log
                    .borrow_mut()
                    .push(format!("{name}-root-up-{}", phase_name(phase)));
            });
        },
    )
    .absolute()
    .size(px(1.))
}

struct KeyView {
    focus: FocusHandle,
    other: FocusHandle,
    log: Log,
    stop: StopAt,
    listener_names: Vec<&'static str>,
    cached_child: Option<Entity<ChildView>>,
}

impl Render for KeyView {
    fn render(&mut self, _: &mut crate::Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let capture_log = self.log.clone();
        let bubble_log = self.log.clone();
        let action_log = self.log.clone();
        let stop = self.stop;
        div()
            .size_full()
            .child(
                div()
                    .id("focused")
                    .key_context("keyview")
                    .track_focus(&self.focus)
                    .size(px(10.))
                    .capture_key_down(move |_, _, _| {
                        capture_log.borrow_mut().push("path-capture".into())
                    })
                    .on_key_down(move |_, _, cx| {
                        bubble_log.borrow_mut().push("path-bubble".into());
                        if stop == StopAt::PathBubble {
                            cx.stop_propagation();
                        }
                    })
                    .on_action(move |_: &Press, _, _| {
                        action_log.borrow_mut().push("action".into())
                    }),
            )
            .child(div().id("other").track_focus(&self.other).size(px(10.)))
            .children(self.listener_names.iter().map(|name| {
                root_listeners(
                    name,
                    self.log.clone(),
                    if *name == "a" {
                        self.stop
                    } else {
                        StopAt::Never
                    },
                    Some(self.other.clone()),
                )
            }))
            .children(
                self.cached_child
                    .clone()
                    .map(|child| child.cached(StyleRefinement::default())),
            )
    }
}

struct ChildView {
    log: Log,
}

impl Render for ChildView {
    fn render(&mut self, _: &mut crate::Window, _: &mut Context<Self>) -> impl IntoElement {
        div().child(root_listeners(
            "child",
            self.log.clone(),
            StopAt::Never,
            None,
        ))
    }
}

fn key_window(
    app: &mut TestApp,
    stop: StopAt,
    listener_names: Vec<&'static str>,
    focus_initially: bool,
) -> (TestAppWindow<KeyView>, Log) {
    app.update(|cx| cx.bind_keys([KeyBinding::new("x", Press, Some("keyview"))]));
    let log = Log::default();
    let mut window = app.open_window({
        let log = log.clone();
        move |_, cx| KeyView {
            focus: cx.focus_handle(),
            other: cx.focus_handle(),
            log,
            stop,
            listener_names,
            cached_child: None,
        }
    });
    window.draw();
    if focus_initially {
        window.update(|view, window, cx| view.focus.focus(window, cx));
        window.draw();
    }
    (window, log)
}

fn take(log: &Log) -> Vec<String> {
    std::mem::take(&mut *log.borrow_mut())
}

fn key_down(window: &mut TestAppWindow<KeyView>, key: &str) {
    window.simulate_event(KeyDownEvent {
        keystroke: Keystroke::parse(key).unwrap(),
        is_held: false,
        prefer_character_input: false,
    });
}

#[test]
fn root_listeners_wrap_the_focused_path() {
    let mut app = TestApp::new();
    let (mut window, log) = key_window(&mut app, StopAt::Never, vec!["a"], true);
    key_down(&mut window, "y");
    assert_eq!(
        take(&log),
        [
            "a-root-capture",
            "path-capture",
            "path-bubble",
            "a-root-bubble"
        ]
    );
}

#[test]
fn bound_actions_consume_the_event_before_root_listeners() {
    let mut app = TestApp::new();
    let (mut window, log) = key_window(&mut app, StopAt::Never, vec!["a"], true);
    key_down(&mut window, "x");
    assert_eq!(
        take(&log),
        ["action"],
        "no raw root callback after an action"
    );
    key_down(&mut window, "y");
    assert_eq!(
        take(&log).first().map(String::as_str),
        Some("a-root-capture")
    );
}

#[test]
fn stopping_propagation_skips_every_later_callback() {
    let mut app = TestApp::new();
    let (mut window, log) = key_window(&mut app, StopAt::RootCapture, vec!["a", "b"], true);
    key_down(&mut window, "y");
    assert_eq!(
        take(&log),
        ["a-root-capture"],
        "nothing after the stopped root capture"
    );

    let mut app = TestApp::new();
    let (mut window, log) = key_window(&mut app, StopAt::PathBubble, vec!["a", "b"], true);
    key_down(&mut window, "y");
    assert_eq!(
        take(&log),
        [
            "a-root-capture",
            "b-root-capture",
            "path-capture",
            "path-bubble"
        ],
        "root bubble does not run after a stopped path bubble"
    );
}

#[test]
fn key_up_and_key_down_listeners_are_typed_and_run_without_focus() {
    let mut app = TestApp::new();
    let (mut window, log) = key_window(&mut app, StopAt::Never, vec!["a"], false);
    window.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse("y").unwrap(),
    });
    assert_eq!(take(&log), ["a-root-up-capture", "a-root-up-bubble"]);
    key_down(&mut window, "y");
    assert_eq!(
        take(&log),
        ["a-root-capture", "a-root-bubble"],
        "no focused element"
    );
}

#[test]
fn refreshing_and_refocusing_inside_a_callback_neither_skips_nor_repeats_listeners() {
    let mut app = TestApp::new();
    let (mut window, log) = key_window(&mut app, StopAt::RefreshAndRefocus, vec!["a", "b"], true);
    key_down(&mut window, "y");
    assert_eq!(
        take(&log),
        [
            "a-root-capture",
            "b-root-capture",
            "path-capture",
            "path-bubble",
            "a-root-bubble",
            "b-root-bubble"
        ]
    );
}

#[test]
fn cached_view_reuses_its_root_listeners_exactly_once() {
    let mut app = TestApp::new();
    let (mut window, log) = key_window(&mut app, StopAt::Never, vec![], true);
    let child = window.update(|_, _, cx| cx.new(|_| ChildView { log: log.clone() }));
    window.update(|view, _, cx| {
        view.cached_child = Some(child.clone());
        cx.notify();
    });
    window.draw();

    key_down(&mut window, "y");
    let first: Vec<String> = take(&log)
        .into_iter()
        .filter(|entry| entry.starts_with("child"))
        .collect();
    assert_eq!(first, ["child-root-capture", "child-root-bubble"]);

    // Repaint only the parent: the child view is reused, never re-rendered.
    window.update(|_, _, cx| cx.notify());
    window.draw();
    key_down(&mut window, "y");
    let second: Vec<String> = take(&log)
        .into_iter()
        .filter(|entry| entry.starts_with("child"))
        .collect();
    assert_eq!(second, ["child-root-capture", "child-root-bubble"]);
}

// ---- INP-004 at window level ----------------------------------------------------------------

struct TabView {
    handles: Vec<FocusHandle>,
    visible: Vec<bool>,
    wrap_tracks_same_handle: bool,
    cached_child: Option<Entity<DuplicateChild>>,
}

impl Render for TabView {
    fn render(&mut self, _: &mut crate::Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .children(self.handles.iter().enumerate().filter_map(|(ix, handle)| {
                self.visible[ix].then(|| {
                    let stop = div().id(("stop", ix)).track_focus(handle).size(px(10.));
                    if self.wrap_tracks_same_handle && ix == 2 {
                        div()
                            .id("wrapper")
                            .track_focus(handle)
                            .child(stop)
                            .into_any_element()
                    } else {
                        stop.into_any_element()
                    }
                })
            }))
            .children(
                self.cached_child
                    .clone()
                    .map(|child| child.cached(StyleRefinement::default())),
            )
    }
}

struct DuplicateChild {
    handle: FocusHandle,
}

impl Render for DuplicateChild {
    fn render(&mut self, _: &mut crate::Window, _: &mut Context<Self>) -> impl IntoElement {
        div().id("dup-wrapper").track_focus(&self.handle).child(
            div()
                .id("dup-child")
                .track_focus(&self.handle)
                .size(px(10.)),
        )
    }
}

fn tab_window(app: &mut TestApp, count: usize) -> TestAppWindow<TabView> {
    let mut window = app.open_window(|_, cx| TabView {
        handles: (0..count)
            .map(|_| cx.focus_handle().tab_stop(true))
            .collect(),
        visible: vec![true; count],
        wrap_tracks_same_handle: false,
        cached_child: None,
    });
    window.draw();
    window
}

fn focused_index(window: &mut TestAppWindow<TabView>) -> Option<usize> {
    window.update(|view, window, cx| {
        let focused = window.focused(cx)?;
        view.handles
            .iter()
            .position(|handle| handle.id() == focused.id())
    })
}

fn ids(window: &mut TestAppWindow<TabView>, positions: &[usize]) -> Vec<FocusId> {
    window.update(|view, _, _| positions.iter().map(|ix| view.handles[*ix].id()).collect())
}

fn step(window: &mut TestAppWindow<TabView>, allowed: &[FocusId], forward: bool) {
    window.update(|_, window, cx| {
        if forward {
            window.focus_next_among(cx, |id, _| allowed.contains(&id));
        } else {
            window.focus_prev_among(cx, |id, _| allowed.contains(&id));
        }
    });
    window.draw();
}

#[test]
fn focus_next_and_prev_among_walk_the_accepted_subset() {
    let mut app = TestApp::new();
    let mut window = tab_window(&mut app, 5);
    let allowed = ids(&mut window, &[1, 3]);

    // No current focus: forward selects position 2 (index 1), backward position 4 (index 3).
    step(&mut window, &allowed, true);
    assert_eq!(focused_index(&mut window), Some(1));
    step(&mut window, &allowed, true);
    assert_eq!(focused_index(&mut window), Some(3));
    step(&mut window, &allowed, true);
    assert_eq!(focused_index(&mut window), Some(1));

    step(&mut window, &allowed, false);
    assert_eq!(
        focused_index(&mut window),
        Some(3),
        "backward wraps from the first accepted"
    );
    step(&mut window, &allowed, false);
    assert_eq!(focused_index(&mut window), Some(1));

    // Rejected current focus (position 3, index 2).
    window.update(|view, window, cx| view.handles[2].focus(window, cx));
    window.draw();
    step(&mut window, &allowed, true);
    assert_eq!(focused_index(&mut window), Some(1));
    window.update(|view, window, cx| view.handles[2].focus(window, cx));
    window.draw();
    step(&mut window, &allowed, false);
    assert_eq!(focused_index(&mut window), Some(3));

    // Empty accepted set: focus does not change.
    step(&mut window, &[], true);
    assert_eq!(focused_index(&mut window), Some(3));
    step(&mut window, &[], false);
    assert_eq!(focused_index(&mut window), Some(3));

    // Disabled window focus: no change.
    window.update(|_, window, _| window.focus_enabled = false);
    step(&mut window, &allowed, true);
    assert_eq!(focused_index(&mut window), Some(3));
}

#[test]
fn hidden_handles_are_absent_from_subset_traversal() {
    let mut app = TestApp::new();
    let mut window = tab_window(&mut app, 4);
    window.update(|view, _, cx| {
        view.visible[1] = false;
        cx.notify();
    });
    window.draw();
    let allowed = ids(&mut window, &[1, 2]);
    step(&mut window, &allowed, true);
    assert_eq!(
        focused_index(&mut window),
        Some(2),
        "the hidden handle is never selected"
    );
    step(&mut window, &allowed, true);
    assert_eq!(focused_index(&mut window), Some(2));
}

#[test]
fn one_handle_tracked_twice_is_one_stop_at_its_first_position() {
    let mut app = TestApp::new();
    let mut window = tab_window(&mut app, 4);
    window.update(|view, _, cx| {
        view.wrap_tracks_same_handle = true;
        cx.notify();
    });
    window.draw();
    let count = window.update(|_, window, _| window.rendered_frame.tab_stops.tab_stop_count());
    assert_eq!(count, 4);

    // Ordinary traversal: backward from the stop after the duplicate lands on it and backward
    // from the duplicate lands on the stop before it.
    window.update(|view, window, cx| view.handles[3].focus(window, cx));
    window.draw();
    window.update(|_, window, cx| window.focus_prev(cx));
    assert_eq!(focused_index(&mut window), Some(2));
    window.update(|_, window, cx| window.focus_prev(cx));
    assert_eq!(focused_index(&mut window), Some(1));
}

#[test]
fn cached_view_replay_keeps_duplicate_handles_unique() {
    let mut app = TestApp::new();
    let mut window = tab_window(&mut app, 2);
    let child = window.update(|view, _, cx| {
        let handle = cx.focus_handle().tab_stop(true);
        let _ = view;
        cx.new(|_| DuplicateChild { handle })
    });
    window.update(|view, _, cx| {
        view.cached_child = Some(child.clone());
        cx.notify();
    });
    window.draw();
    let first = window.update(|_, window, _| window.rendered_frame.tab_stops.tab_stop_count());
    assert_eq!(first, 3);

    // Repaint only the parent: the child's recorded tab-stop operations are replayed.
    window.update(|_, _, cx| cx.notify());
    window.draw();
    let replayed = window.update(|_, window, _| window.rendered_frame.tab_stops.tab_stop_count());
    assert_eq!(replayed, 3);
}
