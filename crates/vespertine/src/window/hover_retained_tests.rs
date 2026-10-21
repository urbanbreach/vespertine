//! Tests of the redraw fallback for hover changes nothing reported (PRF-001 R5): a view that
//! painted the hover state of a hitbox is rebuilt when that state changes, whether or not a
//! listener of the view noticed.

use crate::{
    AnyWindowHandle, AppContext as _, Bounds, Context, Entity, InputEvent as _,
    InteractiveElement as _, IntoElement, Modifiers, MouseMoveEvent, ParentElement as _, Pixels,
    Render, Styled as _, TestAppContext, Window, div, point, px, rgb, size,
};

struct Item {
    ix: usize,
}

impl Render for Item {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id(("item", self.ix))
            .w(px(200.))
            .h(px(40.))
            .bg(rgb(0xffffff))
            .hover(|style| style.bg(rgb(0xd0e0ff)))
            .child(format!("item {}", self.ix))
    }
}

struct Host {
    items: Vec<Entity<Item>>,
    /// A view that blocks the pointer from reaching the second item.
    popup: bool,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let mut root = div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .children(self.items.iter().cloned());
        if self.popup {
            root = root.child(
                div()
                    .absolute()
                    .top(px(40.))
                    .left(px(0.))
                    .w(px(200.))
                    .h(px(40.))
                    .occlude()
                    .bg(rgb(0xeeeeee)),
            );
        }
        root
    }
}

fn open(cx: &mut TestAppContext, retained: bool, popup: bool) -> (AnyWindowHandle, Entity<Host>) {
    let handle = cx.add_window(move |_, cx| Host {
        items: (0..3).map(|ix| cx.new(|_| Item { ix })).collect(),
        popup,
    });
    let root = handle.root(cx).unwrap();
    let window: AnyWindowHandle = handle.into();
    cx.update_window(window, |_, window, _| {
        window.set_retention_enabled(retained)
    })
    .unwrap();
    draw(cx, window);
    draw(cx, window);
    (window, root)
}

fn draw(cx: &mut TestAppContext, window: AnyWindowHandle) {
    cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
}

fn move_pointer(cx: &mut TestAppContext, window: AnyWindowHandle, x: f32, y: f32) {
    let moved = MouseMoveEvent {
        modifiers: Modifiers::default(),
        position: point(px(x), px(y)),
        pressed_button: None,
    };
    cx.update_window(window, |_, window, cx| {
        window.dispatch_event(moved.to_platform_input(), cx)
    })
    .unwrap();
}

fn signature(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
) -> (Vec<String>, Vec<Vec<Bounds<Pixels>>>) {
    cx.update_window(window, |_, window, _| {
        let hits = [(10., 10.), (10., 50.), (10., 90.)]
            .into_iter()
            .map(|(x, y)| window.hit_test_bounds(point(px(x), px(y))))
            .collect();
        (window.painted_primitives(), hits)
    })
    .unwrap()
}

/// How many quads of the last frame carry the hover color.
fn hovered_quads(cx: &mut TestAppContext, window: AnyWindowHandle) -> usize {
    signature(cx, window)
        .0
        .iter()
        .filter(|primitive| primitive.starts_with("Quad") && primitive.contains("l: 0.9078431"))
        .count()
}

fn settle(cx: &mut TestAppContext, windows: [AnyWindowHandle; 2]) {
    for _ in 0..3 {
        for window in windows {
            draw(cx, window);
        }
    }
}

/// A hover listener compares the hitbox with state it keeps; after the pointer position was
/// reset by a resize that state is out of date, so a move within the same item notifies nobody.
/// The retained window still shows the hover the oracle shows.
#[crate::test]
fn a_hover_change_no_listener_reported_rebuilds_the_views_that_painted_it(cx: &mut TestAppContext) {
    let (retained, _) = open(cx, true, false);
    let (full, _) = open(cx, false, false);
    for window in [retained, full] {
        move_pointer(cx, window, 10., 50.);
    }
    settle(cx, [retained, full]);
    assert_eq!(signature(cx, retained), signature(cx, full));
    assert_eq!(hovered_quads(cx, retained), 1, "the second item is hovered");

    // The platform window resizes: the pointer position resets to the origin, over the first
    // item, and the second item keeps its listener state ("hovered").
    for window in [retained, full] {
        cx.simulate_window_resize(window, size(px(300.), px(200.)));
    }
    settle(cx, [retained, full]);
    assert_eq!(signature(cx, retained), signature(cx, full));

    // The pointer moves within the second item: its listener sees no change.
    for window in [retained, full] {
        move_pointer(cx, window, 10., 60.);
    }
    settle(cx, [retained, full]);
    assert_eq!(
        signature(cx, retained),
        signature(cx, full),
        "the retained window shows the hover the full window shows"
    );
    assert_eq!(hovered_quads(cx, retained), 1);
}

/// A view that blocked the pointer goes away without the pointer moving: the item under it is
/// hovered in the next frame's hit test although it was replayed. The window rebuilds it in the
/// frame that follows (no further input needed), and then settles.
#[crate::test]
fn a_view_that_stops_blocking_the_pointer_gives_the_hover_back(cx: &mut TestAppContext) {
    let (retained, retained_root) = open(cx, true, true);
    let (full, full_root) = open(cx, false, true);
    for window in [retained, full] {
        move_pointer(cx, window, 10., 50.);
    }
    settle(cx, [retained, full]);
    assert_eq!(signature(cx, retained), signature(cx, full));
    assert_eq!(
        hovered_quads(cx, retained),
        0,
        "the popup blocks the pointer"
    );

    for root in [&retained_root, &full_root] {
        root.update(cx, |host, cx| {
            host.popup = false;
            cx.notify();
        });
    }
    settle(cx, [retained, full]);
    assert_eq!(
        signature(cx, retained),
        signature(cx, full),
        "the item shows the hover once nothing blocks the pointer"
    );
    assert_eq!(hovered_quads(cx, retained), 1);

    // Nothing keeps rebuilding: the fallback settles.
    cx.update_window(retained, |_, window, _| window.reset_frame_stats())
        .unwrap();
    for _ in 0..4 {
        draw(cx, retained);
    }
    let after = cx
        .update_window(retained, |_, window, _| window.frame_stats())
        .unwrap();
    assert_eq!(after.views_built, 0, "{after:?}");
}

/// A view styled by the hover state of a group declared around it, outside the view.
struct GroupMember {
    value: u32,
}

impl Render for GroupMember {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(200.))
            .h(px(20.))
            .bg(rgb(0xffffff))
            .group_hover("row", |style| style.bg(rgb(0xd0e0ff)))
            .child(format!("value {}", self.value))
    }
}

struct GroupRow {
    member: Entity<GroupMember>,
}

impl Render for GroupRow {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            div()
                .group("row")
                .w(px(200.))
                .h(px(40.))
                .child(self.member.clone()),
        )
    }
}

/// A view nested in a group is built again on its own while the pointer is over the group; its
/// group hover still follows the pointer out of the group.
#[crate::test]
fn a_view_rebuilt_inside_a_group_keeps_following_the_group_hover(cx: &mut TestAppContext) {
    let mut members = Vec::new();
    let mut windows = Vec::new();
    for retained in [true, false] {
        let member = cx.new(|_| GroupMember { value: 0 });
        let handle = cx.add_window({
            let member = member.clone();
            move |_, _| GroupRow { member }
        });
        let window: AnyWindowHandle = handle.into();
        cx.update_window(window, |_, window, _| {
            window.set_retention_enabled(retained)
        })
        .unwrap();
        members.push(member);
        windows.push(window);
    }
    let windows = [windows[0], windows[1]];
    settle(cx, windows);
    for window in windows {
        move_pointer(cx, window, 10., 30.);
    }
    settle(cx, windows);
    assert_eq!(signature(cx, windows[0]), signature(cx, windows[1]));
    assert_eq!(hovered_quads(cx, windows[0]), 1, "the group is hovered");

    for member in &members {
        member.update(cx, |member, cx| {
            member.value += 1;
            cx.notify();
        });
    }
    settle(cx, windows);
    assert_eq!(signature(cx, windows[0]), signature(cx, windows[1]));

    for window in windows {
        move_pointer(cx, window, 10., 100.);
    }
    settle(cx, windows);
    assert_eq!(
        signature(cx, windows[0]),
        signature(cx, windows[1]),
        "the retained window shows the group hover the full window shows"
    );
    assert_eq!(hovered_quads(cx, windows[0]), 0, "the group is not hovered");
}
