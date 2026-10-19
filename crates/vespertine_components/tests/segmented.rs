//! CMP-017 R14, R15 (A9): the segmented control is controlled, activates independently of
//! selection and routes nothing itself.

use std::{cell::RefCell, rc::Rc};
use vespertine::{
    Context, Entity, InteractiveElement, IntoElement, KeyDownEvent, KeyUpEvent, Keystroke,
    Modifiers, ParentElement, Pixels, Point, Render, SharedString, Styled, TestAppContext,
    VisualTestContext, Window, div, px,
};
use vespertine_components::segmented::{Segment, SegmentedControl};

struct Owner {
    selected: SharedString,
    log: Rc<RefCell<Vec<String>>>,
    /// Whether the owner follows activations (routes content) or ignores them.
    follow: bool,
}

impl Render for Owner {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity();
        let selected = self.selected.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                SegmentedControl::new("modes")
                    .segments(vec![
                        Segment::new("chat", "Chat"),
                        Segment::new("files", "Files"),
                        Segment::new("tasks", "Tasks"),
                    ])
                    .selected(selected.clone())
                    .on_activate(move |id, _, cx| {
                        me.update(cx, |owner, cx| {
                            owner.log.borrow_mut().push(id.to_string());
                            if owner.follow {
                                owner.selected = id.clone();
                            }
                            cx.notify();
                        });
                    }),
            )
            // The content shown is whatever the owner routes.
            .child(
                div()
                    .id("content")
                    .debug_selector(move || format!("content:{selected}"))
                    .size(px(40.)),
            )
    }
}

fn setup(
    cx: &mut TestAppContext,
    follow: bool,
) -> (
    Entity<Owner>,
    Rc<RefCell<Vec<String>>>,
    &mut VisualTestContext,
) {
    let log: Rc<RefCell<Vec<String>>> = Rc::default();
    let sink = log.clone();
    let (owner, vcx) = cx.add_window_view(move |_, _| Owner {
        selected: "chat".into(),
        log: sink,
        follow,
    });
    vcx.update(|window, _| window.activate_window());
    redraw(vcx);
    redraw(vcx);
    (owner, log, vcx)
}

fn redraw(cx: &mut VisualTestContext) {
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
}

fn center(cx: &mut VisualTestContext, id: &str) -> Point<Pixels> {
    cx.debug_bounds(&format!("segment:{id}"))
        .unwrap_or_else(|| panic!("no segment {id}"))
        .center()
}

fn click(cx: &mut VisualTestContext, position: Point<Pixels>) {
    cx.simulate_mouse_move(position, None, Modifiers::default());
    redraw(cx);
    cx.simulate_click(position, Modifiers::default());
    redraw(cx);
}

fn press(cx: &mut VisualTestContext, key: &str) {
    let keystroke = Keystroke::parse(key).expect("keystroke");
    cx.simulate_event(KeyDownEvent {
        keystroke: keystroke.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    cx.simulate_event(KeyUpEvent { keystroke });
    redraw(cx);
}

/// The labels of the segments that report selected, from the accessibility tree.
fn selected_segments(cx: &mut VisualTestContext) -> Vec<String> {
    cx.update(|window, _| window.set_a11y_active_for_tests(true));
    redraw(cx);
    let tree = cx
        .update(|window, _| window.debug_a11y_tree_json())
        .expect("an accessibility tree");
    let tree: serde_json::Value = serde_json::from_str(&tree).expect("json");
    let mut selected: Vec<String> = tree["nodes"]
        .as_object()
        .expect("nodes")
        .values()
        .filter(|node| node["aria"]["role"] == "Tab" && node["aria"]["selected"] == true)
        .filter_map(|node| node["aria"]["label"].as_str().map(str::to_owned))
        .collect();
    selected.sort();
    selected
}

fn shown(cx: &mut VisualTestContext, id: &str) -> bool {
    cx.debug_bounds(&format!("content:{id}")).is_some()
}

#[vespertine::test]
fn selection_is_controlled_the_click_alone_changes_nothing(cx: &mut TestAppContext) {
    let (owner, log, cx) = setup(cx, false);
    assert_eq!(selected_segments(cx), ["Chat"]);
    let files = center(cx, "files");
    click(cx, files);
    assert_eq!(*log.borrow(), ["files"]);
    assert_eq!(
        selected_segments(cx),
        ["Chat"],
        "the owner did not re-render with a new id"
    );
    assert!(shown(cx, "chat") && !shown(cx, "files"));
    // The owner re-renders with a new id: only then does the displayed selection change.
    owner.update(cx, |owner, cx| {
        owner.selected = "files".into();
        cx.notify();
    });
    redraw(cx);
    assert_eq!(selected_segments(cx), ["Files"]);
}

#[vespertine::test]
fn reactivating_the_selected_segment_still_invokes_the_callback(cx: &mut TestAppContext) {
    let (_, log, cx) = setup(cx, true);
    let chat = center(cx, "chat");
    click(cx, chat);
    click(cx, chat);
    assert_eq!(*log.borrow(), ["chat", "chat"]);
    assert_eq!(selected_segments(cx), ["Chat"]);
}

#[vespertine::test]
fn enter_and_space_activate_the_focused_segment_and_selection_is_independent_of_focus(
    cx: &mut TestAppContext,
) {
    let (_, log, cx) = setup(cx, false);
    // Selected without focus: nothing is focused, so a key activates nothing.
    cx.update(|window, app| window.blur(app));
    press(cx, "enter");
    assert!(log.borrow().is_empty());
    assert_eq!(selected_segments(cx), ["Chat"]);
    // Focus moves to the first tab stop (the selected segment), then to a segment that is
    // focused without being selected: Enter and Space activate the focused one.
    cx.update(|window, app| window.focus_next(app));
    redraw(cx);
    cx.update(|window, app| window.focus_next(app));
    redraw(cx);
    press(cx, "enter");
    assert_eq!(*log.borrow(), ["files"]);
    assert_eq!(selected_segments(cx), ["Chat"], "focused is not selected");
    cx.update(|window, app| window.focus_next(app));
    redraw(cx);
    press(cx, "space");
    assert_eq!(*log.borrow(), ["files", "tasks"]);
}

#[vespertine::test]
fn the_content_shown_is_whatever_the_owner_routes(cx: &mut TestAppContext) {
    let (_, log, cx) = setup(cx, true);
    assert!(shown(cx, "chat"));
    let tasks = center(cx, "tasks");
    click(cx, tasks);
    assert_eq!(*log.borrow(), ["tasks"]);
    assert!(shown(cx, "tasks") && !shown(cx, "chat"));
    assert_eq!(selected_segments(cx), ["Tasks"]);
}

#[vespertine::test]
fn segments_are_laid_out_at_the_specified_size(cx: &mut TestAppContext) {
    let (_, _, cx) = setup(cx, true);
    let chat = cx.debug_bounds("segment:chat").expect("chat");
    let files = cx.debug_bounds("segment:files").expect("files");
    assert_eq!(chat.size.height, px(24.));
    // Segments are 4 px apart.
    assert_eq!(files.left() - chat.right(), px(4.));
}
