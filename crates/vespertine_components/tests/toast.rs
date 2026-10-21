//! CMP-018: the toast host inside windows: countdown gating, stack geometry, swipe, scheduling,
//! deferred error publication, semantics and placement.

use vespertine::{
    Bounds, Context, IntoElement, Modifiers, MotionClock, MouseButton, ParentElement, Pixels,
    Point, Render, Styled, TestAppContext, VisualTestContext, Window, div, point, px, size,
};
use vespertine_components::toast::{
    self, ToastKind, live_messages, render_toaster, scheduled_wakeup, take_error, update_queue,
    with_queue,
};

struct Root {
    pending_error: Option<String>,
    take_in_render: bool,
}

impl Render for Root {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.take_in_render {
            take_error(&mut self.pending_error, cx);
        }
        div().size_full().children(render_toaster(window, cx))
    }
}

fn setup(cx: &mut TestAppContext, take_in_render: bool) -> &mut VisualTestContext {
    let (_, vcx) = cx.add_window_view(move |_, _| Root {
        pending_error: None,
        take_in_render,
    });
    vcx.update(|window, app| {
        window.activate_window();
        MotionClock::set(app, 1000.0);
    });
    redraw(vcx);
    vcx
}

fn redraw(cx: &mut VisualTestContext) {
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn set_time(cx: &mut VisualTestContext, ms: f64) {
    cx.update(|_, app| {
        MotionClock::set(app, ms);
    });
    redraw(cx);
}

/// Several frames at the given motion times, so measurements and glides settle.
fn settle(cx: &mut VisualTestContext, from: f64, to: f64) {
    let mut now = from;
    while now < to {
        set_time(cx, now);
        now += 100.0;
    }
    set_time(cx, to);
    set_time(cx, to);
}

fn live(cx: &mut VisualTestContext) -> Vec<String> {
    cx.update(|_, app| live_messages(app).iter().map(ToString::to_string).collect())
}

fn publish(cx: &mut VisualTestContext, kind: ToastKind, message: &str) {
    cx.update(|_, app| toast::show(app, kind, message.to_owned()));
    redraw(cx);
}

fn card_ids(cx: &mut VisualTestContext) -> Vec<u64> {
    cx.update(|_, app| {
        with_queue(app, |queue| {
            queue.toasts().iter().map(|toast| toast.id).collect()
        })
        .unwrap_or_default()
    })
}

fn card(cx: &mut VisualTestContext, id: u64) -> Option<Bounds<Pixels>> {
    cx.debug_bounds(&format!("toast:{id}"))
}

fn closing(cx: &mut VisualTestContext, id: u64) -> bool {
    cx.update(|_, app| {
        with_queue(app, |queue| {
            queue.get(id).is_some_and(|toast| toast.is_closing())
        })
        .unwrap_or(false)
    })
}

fn resize(cx: &mut VisualTestContext, width: f32, height: f32) {
    cx.simulate_resize(size(px(width), px(height)));
    redraw(cx);
    redraw(cx);
}

#[vespertine::test]
fn an_empty_queue_renders_no_element_and_publishing_shows_the_host(cx: &mut TestAppContext) {
    let cx = setup(cx, false);
    assert!(cx.debug_bounds("toast-stack").is_none());
    publish(cx, ToastKind::Info, "  hello  ");
    redraw(cx);
    assert!(cx.debug_bounds("toast-stack").is_some());
    assert_eq!(live(cx), ["hello"]);
    // Whitespace only: nothing is queued.
    publish(cx, ToastKind::Error, " \n ");
    assert_eq!(live(cx), ["hello"]);
}

#[vespertine::test]
fn the_countdown_pauses_while_the_stack_is_hovered_and_while_the_window_is_inactive(
    cx: &mut TestAppContext,
) {
    let cx = setup(cx, false);
    resize(cx, 1000.0, 800.0);
    publish(cx, ToastKind::Info, "timed");
    settle(cx, 1000.0, 2000.0);
    // Hover the stack after 1 s: the pointer is over the stack for 30 s.
    let stack = cx.debug_bounds("toast-stack").expect("the stack");
    cx.simulate_mouse_move(stack.center(), None, Modifiers::default());
    redraw(cx);
    assert!(cx.update(|_, app| with_queue(app, |q| q.is_hovered()).unwrap()));
    settle(cx, 2000.0, 32_000.0);
    assert_eq!(live(cx), ["timed"]);
    // Leaving releases hover; an inactive window keeps the countdown paused.
    cx.update(|window, app| window.set_active_for_tests(false, app));
    cx.simulate_mouse_move(point(px(5.), px(5.)), None, Modifiers::default());
    redraw(cx);
    assert!(!cx.update(|_, app| with_queue(app, |q| q.is_hovered()).unwrap()));
    settle(cx, 32_000.0, 61_000.0);
    assert_eq!(live(cx), ["timed"]);
    // Active again: 3000 ms remain.
    cx.update(|window, app| window.set_active_for_tests(true, app));
    set_time(cx, 61_000.0);
    set_time(cx, 63_900.0);
    let id = card_ids(cx)[0];
    assert!(!closing(cx, id));
    set_time(cx, 64_100.0);
    assert!(closing(cx, id));
    set_time(cx, 64_400.0);
    assert!(live(cx).is_empty());
    assert!(cx.debug_bounds("toast-stack").is_none());
}

#[vespertine::test]
fn the_stack_is_anchored_bottom_right_and_narrows_with_the_viewport(cx: &mut TestAppContext) {
    let cx = setup(cx, false);
    resize(cx, 1000.0, 800.0);
    publish(cx, ToastKind::Info, "placed");
    settle(cx, 1000.0, 2000.0);
    let stack = cx.debug_bounds("toast-stack").expect("the stack");
    assert_eq!(stack.size.width, px(356.));
    assert_eq!(stack.origin.x, px(1000. - 24. - 356.));
    assert_eq!(stack.origin.y + stack.size.height, px(800. - 24.));
    resize(cx, 300.0, 800.0);
    settle(cx, 2000.0, 2100.0);
    let narrow = cx.debug_bounds("toast-stack").expect("the stack");
    assert_eq!(narrow.size.width, px(252.));
    assert_eq!(narrow.origin.x, px(24.));
}

#[vespertine::test]
fn five_cards_collapse_to_three_and_fan_out_when_hovered(cx: &mut TestAppContext) {
    let cx = setup(cx, false);
    resize(cx, 1000.0, 800.0);
    for ix in 0..5 {
        publish(cx, ToastKind::Info, &format!("card number {ix}"));
    }
    settle(cx, 1000.0, 2200.0);
    let ids = card_ids(cx);
    assert_eq!(ids.len(), 5);
    // Newest first: the last id is the front card.
    let front = card(cx, ids[4]).expect("front card");
    let second = card(cx, ids[3]).expect("second card");
    let third = card(cx, ids[2]).expect("third card");
    assert!(
        card(cx, ids[1]).is_none() && card(cx, ids[0]).is_none(),
        "depths 3 and 4 are hidden"
    );
    // Offsets 0, 14 and 28 px; every clip height equals the front card's; insets 0, 8.9, 17.8.
    assert_eq!(front.bottom() - second.bottom(), px(14.));
    assert_eq!(second.bottom() - third.bottom(), px(14.));
    // Equal heights: the cards are 14 and 28 px higher, so each one's top edge is above.
    assert_eq!(front.size.height, second.size.height);
    assert_eq!(second.size.height, third.size.height);
    // Painted bounds are snapped to whole pixels: 8.9 px reads as 9 and 17.8 px as 18.
    assert!((second.left().as_f32() - front.left().as_f32() - 8.9).abs() < 0.6);
    assert!((third.left().as_f32() - front.left().as_f32() - 17.8).abs() < 0.6);
    assert!((front.right().as_f32() - second.right().as_f32() - 8.9).abs() < 0.6);

    // Hover and let the glides complete: all five are visible at the fanned offsets.
    let hover = card(cx, ids[4]).unwrap().center();
    cx.simulate_mouse_move(hover, None, Modifiers::default());
    redraw(cx);
    settle(cx, 2200.0, 5000.0);
    let heights: Vec<f32> = ids
        .iter()
        .map(|id| card(cx, *id).expect("fanned card").size.height.as_f32())
        .collect();
    let bottoms: Vec<f32> = ids
        .iter()
        .map(|id| card(cx, *id).unwrap().bottom().as_f32())
        .collect();
    let front_bottom = bottoms[4];
    let mut expected = 0.0;
    for depth in 0..5 {
        let ix = 4 - depth;
        assert!(
            (front_bottom - bottoms[ix] - expected).abs() < 0.2,
            "depth {depth}: offset {} expected {expected}",
            front_bottom - bottoms[ix]
        );
        expected += heights[ix] + 14.0;
    }
    let widths: Vec<f32> = ids
        .iter()
        .map(|id| card(cx, *id).unwrap().size.width.as_f32())
        .collect();
    assert!(
        widths.iter().all(|width| (*width - 356.0).abs() < 0.1),
        "no insets when fanned"
    );
}

fn press_move_release(cx: &mut VisualTestContext, from: Point<Pixels>, dx: f32) {
    cx.simulate_mouse_move(from, None, Modifiers::default());
    redraw(cx);
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(
        from + point(px(dx), px(0.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    redraw(cx);
}

#[vespertine::test]
fn a_short_swipe_returns_and_a_long_swipe_dismisses(cx: &mut TestAppContext) {
    let cx = setup(cx, false);
    resize(cx, 1000.0, 800.0);
    publish(cx, ToastKind::Info, "swipe me");
    settle(cx, 1000.0, 2000.0);
    let id = card_ids(cx)[0];
    let rest = card(cx, id).unwrap();
    let start = rest.center();
    // 44 px: displaced and faded before release, back at rest afterwards.
    press_move_release(cx, start, 44.0);
    let displaced = card(cx, id).unwrap();
    assert_eq!(displaced.left() - rest.left(), px(44.));
    assert_eq!(
        displaced.size, rest.size,
        "the card moves, it does not resize"
    );
    let swipe = cx.update(|_, app| with_queue(app, |q| q.get(id).unwrap().swipe()).unwrap());
    assert_eq!(swipe, 44.0);
    cx.simulate_mouse_up(
        start + point(px(44.), px(0.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    redraw(cx);
    let swipe = cx.update(|_, app| with_queue(app, |q| q.get(id).unwrap().swipe()).unwrap());
    assert_eq!(swipe, 0.0);
    assert!(!closing(cx, id));
    assert_eq!(live(cx), ["swipe me"]);
    // 45 px dismisses.
    set_time(cx, 2100.0);
    let start = card(cx, id).unwrap().center();
    press_move_release(cx, start, 45.0);
    // The release may land outside the card: it is tracked as a release outside too.
    cx.simulate_mouse_up(
        start + point(px(60.), px(0.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    redraw(cx);
    assert!(closing(cx, id));
    set_time(cx, 2400.0);
    assert!(live(cx).is_empty());
}

#[vespertine::test]
fn a_swipe_that_leaves_the_card_keeps_tracking_the_pointer_and_dismisses(cx: &mut TestAppContext) {
    let cx = setup(cx, false);
    resize(cx, 1000.0, 800.0);
    publish(cx, ToastKind::Info, "swipe me");
    settle(cx, 1000.0, 2000.0);
    let id = card_ids(cx)[0];
    let rest = card(cx, id).unwrap();
    // Press 30 px inside the right edge and move 50 px right in one step: the pointer is now
    // outside the card, but still inside the window.
    let start = point(rest.right() - px(30.), rest.center().y);
    press_move_release(cx, start, 50.0);
    let swipe = cx.update(|_, app| with_queue(app, |q| q.get(id).unwrap().swipe()).unwrap());
    assert_eq!(
        swipe, 50.0,
        "the move outside the card still drives the swipe"
    );
    cx.simulate_mouse_up(
        start + point(px(50.), px(0.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    redraw(cx);
    assert!(closing(cx, id), "a 50 px swipe dismisses");
}

#[vespertine::test]
fn moving_left_never_displaces_and_a_move_without_the_button_clears_the_press(
    cx: &mut TestAppContext,
) {
    let cx = setup(cx, false);
    resize(cx, 1000.0, 800.0);
    publish(cx, ToastKind::Info, "careful");
    settle(cx, 1000.0, 2000.0);
    let id = card_ids(cx)[0];
    let start = card(cx, id).unwrap().center();
    press_move_release(cx, start, -30.0);
    assert_eq!(
        cx.update(|_, app| with_queue(app, |q| q.get(id).unwrap().swipe()).unwrap()),
        0.0
    );
    cx.simulate_mouse_move(
        start + point(px(20.), px(0.)),
        MouseButton::Left,
        Modifiers::default(),
    );
    redraw(cx);
    assert_eq!(
        cx.update(|_, app| with_queue(app, |q| q.get(id).unwrap().swipe()).unwrap()),
        20.0
    );
    // The button is no longer held: the press and the displacement are cleared.
    cx.simulate_mouse_move(start + point(px(40.), px(0.)), None, Modifiers::default());
    redraw(cx);
    assert_eq!(
        cx.update(|_, app| with_queue(app, |q| q.get(id).unwrap().swipe()).unwrap()),
        0.0
    );
}

#[vespertine::test]
fn an_idle_stack_keeps_one_wakeup_at_the_earliest_deadline_and_replaces_it_when_deadlines_move(
    cx: &mut TestAppContext,
) {
    let cx = setup(cx, false);
    resize(cx, 1000.0, 800.0);
    publish(cx, ToastKind::Info, "first");
    // While the entry animation runs the host asks for frames, not for a timer.
    assert_eq!(cx.update(|_, app| scheduled_wakeup(app)), None);
    settle(cx, 1000.0, 1600.0);
    assert_eq!(cx.update(|_, app| scheduled_wakeup(app)), Some(5000.0));
    // A later notice does not move the earliest deadline.
    set_time(cx, 2000.0);
    publish(cx, ToastKind::Error, "second");
    settle(cx, 2000.0, 2600.0);
    assert_eq!(cx.update(|_, app| scheduled_wakeup(app)), Some(5000.0));
    // A dismissal brings an earlier deadline: the timer is replaced.
    set_time(cx, 3000.0);
    let first = card_ids(cx)[0];
    cx.update(|_, app| update_queue(app, |queue| queue.dismiss(first, 3000.0)));
    redraw(cx);
    settle(cx, 3000.0, 3100.0);
    // Closing animates: no timer; once the exit finished the removal deadline is not pending.
    set_time(cx, 3400.0);
    assert_eq!(live(cx), ["second"]);
    settle(cx, 3400.0, 4000.0);
    assert_eq!(cx.update(|_, app| scheduled_wakeup(app)), Some(6000.0));
    // Hovering pauses every countdown: no deadline at all.
    let stack = cx.debug_bounds("toast-stack").unwrap();
    cx.simulate_mouse_move(stack.center(), None, Modifiers::default());
    redraw(cx);
    settle(cx, 4000.0, 5000.0);
    assert_eq!(cx.update(|_, app| scheduled_wakeup(app)), None);
}

#[vespertine::test]
fn take_error_empties_the_slot_now_and_publishes_on_the_next_turn(cx: &mut TestAppContext) {
    let cx = setup(cx, false);
    let mut slot = Some("disk full".to_owned());
    cx.update(|_, app| {
        take_error(&mut slot, app);
        // Emptied immediately; not visible until the deferred turn runs.
        assert!(slot.is_none());
        assert!(live_messages(app).is_empty());
    });
    cx.run_until_parked();
    assert_eq!(live(cx), ["disk full"]);
    // An empty slot does nothing.
    let mut empty: Option<String> = None;
    cx.update(|_, app| take_error(&mut empty, app));
    cx.run_until_parked();
    assert_eq!(live(cx), ["disk full"]);
}

#[vespertine::test]
fn take_error_inside_a_render_pass_never_nests_a_mutation(cx: &mut TestAppContext) {
    let (root, cx) = cx.add_window_view(|_, _| Root {
        pending_error: None,
        take_in_render: true,
    });
    cx.update(|window, app| {
        window.activate_window();
        MotionClock::set(app, 1000.0);
    });
    redraw(cx);
    root.update(cx, |root, cx| {
        root.pending_error = Some("raised while rendering".to_owned());
        cx.notify();
    });
    redraw(cx);
    redraw(cx);
    assert_eq!(live(cx), ["raised while rendering"]);
    assert!(root.read_with(cx, |root, _| root.pending_error.is_none()));
}

#[vespertine::test]
fn cards_expose_alert_and_status_roles_and_the_stack_a_stable_selector(cx: &mut TestAppContext) {
    let cx = setup(cx, false);
    resize(cx, 1000.0, 800.0);
    publish(cx, ToastKind::Error, "it broke");
    publish(cx, ToastKind::Success, "it worked");
    publish(cx, ToastKind::Info, "fyi");
    settle(cx, 1000.0, 2000.0);
    cx.update(|window, _| window.set_a11y_active_for_tests(true));
    redraw(cx);
    let tree = cx
        .update(|window, _| window.debug_a11y_tree_json())
        .expect("an accessibility tree");
    let tree: serde_json::Value = serde_json::from_str(&tree).expect("json");
    let nodes = tree["nodes"].as_object().expect("nodes");
    let role_of = |label: &str| -> Option<String> {
        nodes
            .values()
            .find(|node| node["aria"]["label"] == label)
            .and_then(|node| node["aria"]["role"].as_str().map(str::to_owned))
    };
    assert_eq!(role_of("it broke").as_deref(), Some("Alert"));
    assert_eq!(role_of("it worked").as_deref(), Some("Status"));
    assert_eq!(role_of("fyi").as_deref(), Some("Status"));
    assert!(
        tree.to_string().contains("vespertine-toast-stack"),
        "the stack carries a stable accessibility id"
    );
    assert!(cx.debug_bounds("toast-stack").is_some());
}

#[vespertine::test]
fn reduced_motion_skips_entry_and_exit_animation(cx: &mut TestAppContext) {
    let cx = setup(cx, false);
    resize(cx, 1000.0, 800.0);
    cx.update(|_, app| app.set_reduce_motion(true));
    publish(cx, ToastKind::Info, "instant");
    redraw(cx);
    redraw(cx);
    let id = card_ids(cx)[0];
    // At the creation instant the card already sits at its rest position.
    let stack = cx.debug_bounds("toast-stack").unwrap();
    let rest = card(cx, id).unwrap();
    assert_eq!(rest.bottom(), stack.bottom());
    // Dismissed: removed 200 ms later without fading.
    cx.update(|_, app| update_queue(app, |queue| queue.dismiss(id, 1000.0)));
    redraw(cx);
    set_time(cx, 1200.0);
    assert!(live(cx).is_empty());
    assert!(cx.debug_bounds("toast-stack").is_none());
}
