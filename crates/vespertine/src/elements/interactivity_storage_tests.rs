//! Tests for PRF-003: compact interaction storage.

use super::div::{AriaProperties, DEFAULT_ARIA_PROPERTIES};
use crate::{
    self as vespertine, ClickEvent, Context, Div, FocusHandle, InteractiveElement, Interactivity,
    IntoElement, Modifiers, MouseButton, ParentElement, Render, StyleRefinement, Styled,
    TestAppContext, Window, counting_allocator::count_allocations, div, point, px,
};
use std::{cell::RefCell, mem::size_of, rc::Rc};
use thin_vec::ThinVec;

crate::actions!(interactivity_storage_tests, [Ping]);

fn baseline_interactivity_allocations() -> usize {
    // An `Interactivity` owns its base style indirectly; nothing else is allocated for an
    // element without interaction state.
    count_allocations(|| Box::new(StyleRefinement::default())).1
}

#[test]
fn empty_collections_and_rare_values_occupy_one_pointer() {
    let pointer = size_of::<usize>();
    assert_eq!(size_of::<ThinVec<Box<dyn Fn()>>>(), pointer);
    assert_eq!(size_of::<Option<Box<AriaProperties>>>(), pointer);
    assert_eq!(size_of::<Option<Box<super::div::GroupStyle>>>(), pointer);
    assert_eq!(
        size_of::<Option<Box<super::div::TooltipBuilder>>>(),
        pointer
    );
    assert_eq!(size_of::<Option<Box<super::div::DragListener>>>(), pointer);
    assert_eq!(size_of::<Option<Box<crate::ScrollAnchor>>>(), pointer);

    let interactivity = Interactivity::new();
    // The footprint of the previous inline layout: 24-byte vectors and inline optional values.
    let inline_layout = size_of::<Interactivity>()
        + 18 * (size_of::<Vec<u8>>() - pointer)
        + 2 * (size_of::<Option<super::div::GroupStyle>>() - pointer)
        + (size_of::<Option<super::div::TooltipBuilder>>() - pointer)
        + (size_of::<Option<super::div::DragListener>>() - pointer)
        + (size_of::<Option<Box<dyn Fn()>>>() - pointer) * 2
        + (size_of::<Option<crate::ScrollAnchor>>() - pointer)
        + (size_of::<AriaProperties>() - pointer);
    eprintln!(
        "Interactivity: {} bytes (inline layout: {inline_layout} bytes)",
        size_of::<Interactivity>()
    );
    assert!(size_of::<Interactivity>() + 400 < inline_layout);
    // Every listener collection is a single pointer; the fields of `Interactivity` that are
    // listener collections therefore contribute exactly `pointer` bytes each.
    assert_eq!(size_of_val(&interactivity.mouse_down_listeners), pointer);
    assert_eq!(size_of_val(&interactivity.click_listeners), pointer);
    assert_eq!(size_of_val(&interactivity.a11y_action_listeners), pointer);
    assert_eq!(size_of_val(&interactivity.group_hover_style), pointer);
    assert_eq!(size_of_val(&interactivity.tooltip_builder), pointer);
    assert_eq!(size_of_val(&interactivity.drag_listener), pointer);
    assert_eq!(size_of_val(&interactivity.can_drop_predicate), pointer);
    assert_eq!(size_of_val(&interactivity.scroll_anchor), pointer);
    assert_eq!(size_of_val(&interactivity.a11y_synthetic_children), pointer);
    assert_eq!(size_of_val(&interactivity.aria), pointer);
    assert_eq!(size_of_val(&interactivity.on_painted), pointer);
}

#[test]
fn inert_elements_allocate_no_listener_vectors_and_no_property_blocks() {
    let baseline = baseline_interactivity_allocations();
    let (elements, allocations) =
        count_allocations(|| (0..2000).map(|_| div()).collect::<Vec<Div>>());
    // The only per-element allocation is the base style box (plus the vector holding them).
    assert!(
        allocations <= 2000 * baseline + 16,
        "{allocations} allocations for 2000 inert divs"
    );
    let mut elements = elements;
    for element in &mut elements {
        let interactivity = element.interactivity();
        assert_eq!(interactivity.allocated_listener_collections(), 0);
        assert!(!interactivity.has_accessibility_property_block());
    }
}

#[test]
fn first_registration_allocates_exactly_one_vector() {
    let mut interactivity = Interactivity::new();
    assert_eq!(interactivity.allocated_listener_collections(), 0);
    let (_, allocations) = count_allocations(|| interactivity.on_modifiers_changed(|_, _, _| {}));
    assert_eq!(allocations, 1);
    assert_eq!(interactivity.allocated_listener_collections(), 1);

    let (_, allocations) = count_allocations(|| interactivity.on_modifiers_changed(|_, _, _| {}));
    assert_eq!(
        allocations, 0,
        "a collection is one vector, grown only when full"
    );
    assert_eq!(interactivity.modifiers_changed_listeners.len(), 2);
    assert_eq!(interactivity.allocated_listener_collections(), 1);
}

#[test]
fn empty_collections_stay_unallocated_when_moved_and_drained() {
    struct Parent {
        _marker: u8,
        child: Interactivity,
    }

    let child = Interactivity::new();
    let (parent, allocations) = count_allocations(|| Parent { _marker: 0, child });
    assert_eq!(allocations, 0, "moving into a parent container");
    let mut containers: Vec<Parent> = Vec::with_capacity(1);
    let (_, allocations) = count_allocations(|| containers.push(parent));
    assert_eq!(allocations, 0, "moving into a vector");
    let mut child = containers.pop().unwrap().child;

    let (_, allocations) = count_allocations(|| {
        let taken_down = std::mem::take(&mut child.mouse_down_listeners);
        let taken_clicks = std::mem::take(&mut child.click_listeners);
        let taken_actions = std::mem::take(&mut child.action_listeners);
        drop((taken_down, taken_clicks, taken_actions));
        for _listener in child.mouse_up_listeners.drain(..) {}
        for _listener in child.key_down_listeners.drain(..) {}
        for _listener in child.a11y_action_listeners.drain(..) {}
        let drained: Vec<_> = child.scroll_wheel_listeners.drain(..).collect();
        drained.len()
    });
    // Only the empty `collect` vector may exist; it has no capacity either.
    assert_eq!(allocations, 0, "taking and draining empty collections");
    assert_eq!(child.allocated_listener_collections(), 0);
}

#[test]
fn drained_collection_keeps_registration_order_and_ownership() {
    let mut interactivity = Interactivity::new();
    for _ in 0..3 {
        interactivity.on_click(|_: &ClickEvent, _, _| {});
    }
    let before: Vec<usize> = interactivity
        .click_listeners
        .iter()
        .map(|listener| Rc::as_ptr(listener) as *const () as usize)
        .collect();
    let buffer = interactivity.click_listeners.as_ptr();

    let moved = std::mem::take(&mut interactivity.click_listeners);
    assert_eq!(
        moved.as_ptr(),
        buffer,
        "moving a collection does not reallocate it"
    );
    assert_eq!(interactivity.click_listeners.len(), 0);
    assert_eq!(interactivity.allocated_listener_collections(), 0);

    let after: Vec<usize> = moved
        .into_iter()
        .map(|listener| Rc::as_ptr(&listener) as *const () as usize)
        .collect();
    assert_eq!(before, after);
}

#[test]
fn default_accessibility_properties_are_read_without_allocating() {
    let mut interactivity = Interactivity::new();
    let mut node = accesskit::Node::new(accesskit::Role::Button);
    let expected = accesskit::Node::new(accesskit::Role::Button);

    let (_, allocations) = count_allocations(|| {
        assert!(std::ptr::eq(interactivity.aria(), &DEFAULT_ARIA_PROPERTIES));
        let aria = interactivity.aria();
        assert!(aria.author_id.is_none() && aria.label.is_none() && aria.value.is_none());
        assert!(aria.selected.is_none() && aria.numeric_value.is_none() && aria.level.is_none());
        interactivity.write_a11y_info(&mut node);
    });
    assert_eq!(allocations, 0, "reading defaults of an unmutated element");
    assert_eq!(node, expected, "defaults write nothing to the node");
    assert!(!interactivity.has_accessibility_property_block());

    let (_, allocations) = count_allocations(|| interactivity.aria_mut().numeric_value = Some(3.0));
    assert_eq!(
        allocations, 1,
        "the first mutation allocates the block once"
    );
    assert!(interactivity.has_accessibility_property_block());
    let (_, allocations) = count_allocations(|| interactivity.aria_mut().level = Some(2));
    assert_eq!(allocations, 0, "later mutations reuse the block");
    assert_eq!(interactivity.aria().numeric_value, Some(3.0));
    assert_eq!(interactivity.aria().level, Some(2));
    assert!(interactivity.aria().label.is_none());
}

struct OrderRoot {
    log: Rc<RefCell<Vec<&'static str>>>,
    focus: FocusHandle,
}

impl OrderRoot {
    fn new(log: Rc<RefCell<Vec<&'static str>>>, cx: &mut Context<Self>) -> Self {
        Self {
            log,
            focus: cx.focus_handle(),
        }
    }
}

impl Render for OrderRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let log = |name: &'static str| {
            let log = self.log.clone();
            move |_: &_, _: &mut Window, _: &mut crate::App| log.borrow_mut().push(name)
        };
        let action_log = |name: &'static str| {
            let log = self.log.clone();
            move |_: &Ping, _: &mut Window, cx: &mut crate::App| {
                log.borrow_mut().push(name);
                // Action handlers stop propagation unless told otherwise.
                cx.propagate();
            }
        };
        let inner = div()
            .id("inner")
            .size(px(50.))
            .track_focus(&self.focus)
            .on_mouse_down(MouseButton::Left, log("inner-bubble-1"))
            .on_mouse_down(MouseButton::Left, log("inner-bubble-2"))
            .capture_any_mouse_down(log("inner-capture"))
            .on_action(action_log("inner-action-1"))
            .on_action(action_log("inner-action-2"))
            .capture_action(action_log("inner-capture-action"));
        div()
            .id("outer")
            .size(px(100.))
            .on_mouse_down(MouseButton::Left, log("outer-bubble-1"))
            .on_mouse_down(MouseButton::Left, log("outer-bubble-2"))
            .capture_any_mouse_down(log("outer-capture"))
            .on_action(action_log("outer-action-1"))
            .on_action(action_log("outer-action-2"))
            .capture_action(action_log("outer-capture-action"))
            .children((0..300).map(|_| div().size(px(0.))))
            .child(inner)
    }
}

#[vespertine::test]
fn callbacks_fire_once_in_registration_order_with_capture_bubble_unchanged(
    cx: &mut TestAppContext,
) {
    let log = Rc::new(RefCell::new(Vec::new()));
    let (root, cx) = cx.add_window_view({
        let log = log.clone();
        |_, cx| OrderRoot::new(log, cx)
    });
    cx.update(|window, cx| {
        let focus = root.read(cx).focus.clone();
        window.focus(&focus, cx);
        window.draw(cx).clear(cx);
    });
    // Children are laid out in a column: 300 zero-height divs, then the inner box at the top.
    cx.simulate_click(point(px(10.), px(10.)), Modifiers::none());
    // Capture runs outermost to innermost over listeners in paint order; bubble runs the
    // same flat listener list in reverse.
    assert_eq!(
        log.borrow().as_slice(),
        [
            "outer-capture",
            "inner-capture",
            "inner-bubble-2",
            "inner-bubble-1",
            "outer-bubble-2",
            "outer-bubble-1",
        ]
    );

    log.borrow_mut().clear();
    cx.dispatch_action(Ping);
    // Capture walks the dispatch path root to target, bubble target to root, and the listeners
    // of a single node run in registration order in both phases.
    assert_eq!(
        log.borrow().as_slice(),
        [
            "outer-capture-action",
            "inner-capture-action",
            "inner-action-1",
            "inner-action-2",
            "outer-action-1",
            "outer-action-2",
        ]
    );
}
