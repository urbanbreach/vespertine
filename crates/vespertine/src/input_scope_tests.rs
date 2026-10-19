//! Tests for focus-scoped input behavior (INP-007): safe focus restoration and
//! pane scopes, keyboard activation, accessibility metadata, the keyboard
//! context menu, cursor restoration after drags and raw key / hold-gesture
//! observation.

use crate::util::FluentBuilder as _;
use crate::{
    self as vespertine, AnyElement, App, AppContext as _, Bounds, Context, CursorStyle,
    DispatchPhase, Element, ElementId, Entity, FocusHandle, GlobalElementId, InputHandler,
    InspectorElementId, InteractiveElement as _, IntoElement, KeyDownEvent, KeyUpEvent, LayoutId,
    Modifiers, ModifiersChangedEvent, ParentElement as _, Pixels, Point, Render,
    StatefulInteractiveElement as _, Style, Styled as _, TestAppContext, UTF16Selection,
    VisualTestContext, Window, div, point, px, size,
};
use std::{
    cell::{Cell, RefCell},
    ops::Range,
    rc::Rc,
};

fn focus_and_draw(cx: &mut VisualTestContext, handle: &FocusHandle) {
    cx.update(|window, cx| {
        window.focus(handle, cx);
        window.activate_window();
        window.draw(cx).clear(cx);
    });
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn focused_id(cx: &mut VisualTestContext) -> Option<crate::FocusId> {
    cx.update(|window, cx| window.focused(cx).map(|handle| handle.id()))
}

fn next_frame(cx: &mut VisualTestContext) -> usize {
    cx.update(|window, cx| window.simulate_next_frame(cx))
}

// ---------------------------------------------------------------------------
// R2: safe focus restoration and pane focus scopes
// ---------------------------------------------------------------------------

struct RestoreView {
    root: FocusHandle,
    editor: FocusHandle,
    dialog: FocusHandle,
    editor_mounted: bool,
    dialog_mounted: bool,
}

impl Render for RestoreView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            // The pane: its scope handle is the fallback root.
            .child(
                div().id("pane").size_full().track_focus(&self.root).child(
                    div()
                        .id("editor")
                        .size(px(20.))
                        .when(self.editor_mounted, |this| this.track_focus(&self.editor)),
                ),
            )
            // A dialog outside the pane.
            .child(
                div()
                    .id("dialog")
                    .size(px(20.))
                    .when(self.dialog_mounted, |this| this.track_focus(&self.dialog)),
            )
    }
}

fn restore_view(cx: &mut TestAppContext) -> (Entity<RestoreView>, &mut VisualTestContext) {
    let (view, cx) = cx.add_window_view(|_, cx| RestoreView {
        root: cx.focus_handle(),
        editor: cx.focus_handle(),
        dialog: cx.focus_handle(),
        editor_mounted: true,
        dialog_mounted: false,
    });
    let root = view.read_with(cx, |view, _| view.root.clone());
    focus_and_draw(cx, &root);
    (view, cx)
}

#[crate::test]
fn restore_focus_to_a_mounted_element_succeeds_on_the_next_frame(cx: &mut TestAppContext) {
    let (view, cx) = restore_view(cx);
    let (root, editor) = view.read_with(cx, |view, _| (view.root.clone(), view.editor.clone()));

    cx.update(|window, _| window.restore_focus(&editor.downgrade(), &root.downgrade()));
    // The request is applied by the next frame, not synchronously.
    assert_eq!(focused_id(cx), Some(root.id()));
    assert_eq!(next_frame(cx), 1);
    assert_eq!(focused_id(cx), Some(editor.id()));
}

#[crate::test]
fn restore_focus_to_an_unmounted_element_focuses_the_fallback_root(cx: &mut TestAppContext) {
    let (view, cx) = restore_view(cx);
    let (root, editor) = view.read_with(cx, |view, _| (view.root.clone(), view.editor.clone()));
    focus_and_draw(cx, &editor);
    assert_eq!(focused_id(cx), Some(editor.id()));

    // The editor's element goes away while its handle stays alive.
    view.update(cx, |view, cx| {
        view.editor_mounted = false;
        cx.notify();
    });
    draw(cx);
    cx.update(|window, _| assert!(!window.is_focus_mounted(&editor)));

    cx.update(|window, _| window.restore_focus(&editor.downgrade(), &root.downgrade()));
    next_frame(cx);
    assert_eq!(focused_id(cx), Some(root.id()), "never the stale editor");
}

#[crate::test]
fn restore_focus_never_focuses_a_dropped_handle(cx: &mut TestAppContext) {
    let (view, cx) = restore_view(cx);
    let root = view.read_with(cx, |view, _| view.root.clone());
    let dropped = cx.update(|_, cx| cx.focus_handle());
    let weak = dropped.downgrade();
    drop(dropped);
    assert!(weak.upgrade().is_none());

    cx.update(|window, _| window.restore_focus(&weak, &root.downgrade()));
    next_frame(cx);
    assert_eq!(focused_id(cx), Some(root.id()));

    // A fallback that is itself gone leaves focus where it was.
    let dropped_fallback = cx.update(|_, cx| cx.focus_handle());
    let weak_fallback = dropped_fallback.downgrade();
    drop(dropped_fallback);
    cx.update(|window, _| window.restore_focus(&weak, &weak_fallback));
    next_frame(cx);
    assert_eq!(focused_id(cx), Some(root.id()));
}

struct ScopeView {
    scope: FocusHandle,
    input: FocusHandle,
    blur_on_press: Rc<Cell<bool>>,
}

impl Render for ScopeView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        use crate::focus_restore::FocusScopeExt as _;
        let input = self.input.clone();
        let blur_on_press = self.blur_on_press.clone();
        div().size_full().child(
            div()
                .id("scope")
                .size(px(100.))
                .focus_scope(&self.scope)
                .child(
                    div()
                        .id("input")
                        .size(px(40.))
                        .track_focus(&self.input)
                        .on_mouse_down(crate::MouseButton::Left, move |_, window, cx| {
                            if blur_on_press.get() {
                                window.blur(cx);
                            } else {
                                // Claims focus while bubbling and stops the
                                // press from reaching anything else.
                                window.focus(&input, cx);
                            }
                            cx.stop_propagation();
                        }),
                ),
        )
    }
}

#[crate::test]
fn pane_scope_is_the_fallback_and_inputs_claim_focus(cx: &mut TestAppContext) {
    let blur_on_press = Rc::new(Cell::new(false));
    let (view, cx) = cx.add_window_view({
        let blur_on_press = blur_on_press.clone();
        move |_, cx| ScopeView {
            scope: cx.focus_handle(),
            input: cx.focus_handle(),
            blur_on_press,
        }
    });
    let (scope, input) = view.read_with(cx, |view, _| (view.scope.clone(), view.input.clone()));
    cx.update(|window, _| window.activate_window());
    draw(cx);
    assert_eq!(focused_id(cx), None);

    // A press on the scope's empty area focuses the scope.
    cx.simulate_click(point(px(80.), px(80.)), Modifiers::none());
    assert_eq!(focused_id(cx), Some(scope.id()));

    // A press on the input: the scope claims focus in the capture phase, the
    // input claims it while bubbling (although it stops propagation).
    cx.simulate_click(point(px(10.), px(10.)), Modifiers::none());
    assert_eq!(focused_id(cx), Some(input.id()));

    // A press inside the already focused input does not blur it first: the
    // input never loses focus during the press.
    let lost_focus = Rc::new(Cell::new(false));
    let _subscription = cx.update(|window, cx| {
        let lost_focus = lost_focus.clone();
        window.on_focus_out(&input, cx, move |_, _, _| lost_focus.set(true))
    });
    draw(cx);
    cx.simulate_click(point(px(10.), px(10.)), Modifiers::none());
    draw(cx);
    assert!(!lost_focus.get());
    assert_eq!(focused_id(cx), Some(input.id()));

    // When the press handler leaves nothing focused, the deferred check
    // focuses the scope.
    blur_on_press.set(true);
    cx.simulate_click(point(px(10.), px(10.)), Modifiers::none());
    cx.run_until_parked();
    assert_eq!(focused_id(cx), Some(scope.id()));
}

#[crate::test]
fn focus_memory_keeps_its_origin_and_yields_to_a_mounted_dialog(cx: &mut TestAppContext) {
    use crate::focus_restore::FocusMemory;
    let (view, cx) = restore_view(cx);
    let (root, editor, dialog) = view.read_with(cx, |view, _| {
        (view.root.clone(), view.editor.clone(), view.dialog.clone())
    });
    let mut memory = FocusMemory::new(&root);

    // Focus in the pane's editor is remembered; the scope's own focus is not
    // an origin.
    focus_and_draw(cx, &editor);
    cx.update(|window, cx| memory.remember(window, cx));
    assert_eq!(memory.origin().as_ref(), Some(&editor));
    focus_and_draw(cx, &root);
    cx.update(|window, cx| memory.remember(window, cx));
    assert_eq!(memory.origin().as_ref(), Some(&editor));

    // Restoring returns to the origin.
    cx.update(|window, cx| memory.restore(window, cx));
    next_frame(cx);
    assert_eq!(focused_id(cx), Some(editor.id()));

    // The editor is detached: the origin is kept, and restoring falls back
    // to the scope because the origin is unmounted.
    view.update(cx, |view, cx| {
        view.editor_mounted = false;
        cx.notify();
    });
    draw(cx);
    cx.update(|window, cx| memory.remember(window, cx));
    assert_eq!(memory.origin().as_ref(), Some(&editor));
    cx.update(|window, cx| memory.restore(window, cx));
    next_frame(cx);
    assert_eq!(focused_id(cx), Some(root.id()));

    // A mounted focus elsewhere (a dialog) takes precedence over the origin.
    view.update(cx, |view, cx| {
        view.editor_mounted = true;
        view.dialog_mounted = true;
        cx.notify();
    });
    draw(cx);
    focus_and_draw(cx, &dialog);
    cx.update(|window, cx| memory.restore(window, cx));
    next_frame(cx);
    assert_eq!(focused_id(cx), Some(dialog.id()));
}

// ---------------------------------------------------------------------------
// R4: keyboard activation
// ---------------------------------------------------------------------------

struct ActivationView {
    focus: FocusHandle,
    other: FocusHandle,
    activation: crate::keyboard_activation::KeyboardActivation,
    activations: Rc<RefCell<Vec<crate::keyboard_activation::ActivationKey>>>,
}

impl Render for ActivationView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let activations = self.activations.clone();
        div()
            .size_full()
            .child(self.activation.bind(
                div().id("button").size(px(20.)).track_focus(&self.focus),
                move |key, _, _| activations.borrow_mut().push(key),
            ))
            .child(div().id("other").size(px(20.)).track_focus(&self.other))
    }
}

fn key_down(cx: &mut VisualTestContext, key: &str, is_held: bool) {
    let keystroke = crate::Keystroke::parse(key).expect("valid keystroke");
    cx.simulate_event(KeyDownEvent {
        keystroke,
        is_held,
        prefer_character_input: false,
    });
}

fn key_up(cx: &mut VisualTestContext, key: &str) {
    let keystroke = crate::Keystroke::parse(key).expect("valid keystroke");
    cx.simulate_event(KeyUpEvent { keystroke });
}

#[crate::test]
fn enter_activates_on_key_down_and_space_on_key_up(cx: &mut TestAppContext) {
    use crate::keyboard_activation::ActivationKey::{Enter, Space};
    let activations = Rc::new(RefCell::new(Vec::new()));
    let (view, cx) = cx.add_window_view({
        let activations = activations.clone();
        move |_, cx| ActivationView {
            focus: cx.focus_handle(),
            other: cx.focus_handle(),
            activation: Default::default(),
            activations,
        }
    });
    let (focus, other) = view.read_with(cx, |view, _| (view.focus.clone(), view.other.clone()));
    focus_and_draw(cx, &focus);

    // Enter: exactly once, on key-down. Held and unflagged repeats of the
    // same physical press never activate again; the key-up adds nothing.
    key_down(cx, "enter", false);
    assert_eq!(*activations.borrow(), vec![Enter]);
    key_down(cx, "enter", true);
    key_down(cx, "enter", false);
    key_up(cx, "enter");
    assert_eq!(*activations.borrow(), vec![Enter]);
    key_down(cx, "enter", false);
    key_up(cx, "enter");
    assert_eq!(*activations.borrow(), vec![Enter, Enter]);
    activations.borrow_mut().clear();

    // Space: nothing on key-down, one activation on key-up.
    key_down(cx, "space", false);
    assert!(activations.borrow().is_empty());
    key_up(cx, "space");
    assert_eq!(*activations.borrow(), vec![Space]);
    activations.borrow_mut().clear();

    // Interrupted by another key: no activation.
    key_down(cx, "space", false);
    key_down(cx, "a", false);
    key_up(cx, "a");
    key_up(cx, "space");
    assert!(activations.borrow().is_empty());

    // Interrupted by a key-up of another key.
    key_down(cx, "space", false);
    key_up(cx, "b");
    key_up(cx, "space");
    assert!(activations.borrow().is_empty());

    // Focus moving between press and release: no activation.
    key_down(cx, "space", false);
    focus_and_draw(cx, &other);
    focus_and_draw(cx, &focus);
    key_up(cx, "space");
    assert!(activations.borrow().is_empty());

    // Modified Space/Enter are not activations.
    key_down(cx, "ctrl-enter", false);
    key_down(cx, "shift-space", false);
    key_up(cx, "shift-space");
    assert!(activations.borrow().is_empty());
}

// ---------------------------------------------------------------------------
// R4: accessibility metadata
// ---------------------------------------------------------------------------

fn a11y_node(mut element: impl Element) -> (Option<crate::Role>, accesskit::Node) {
    let role = element.a11y_role();
    let mut node = accesskit::Node::new(role.unwrap_or(crate::Role::GenericContainer));
    element.write_a11y_info(&mut node);
    (role, node)
}

#[test]
fn switch_exposes_role_label_and_checked_state() {
    for (checked, expected) in [(true, crate::Toggled::True), (false, crate::Toggled::False)] {
        let (role, node) = a11y_node(
            div()
                .id("wifi")
                .role(crate::Role::Switch)
                .aria_label("Wi-Fi")
                .aria_toggled(expected),
        );
        assert_eq!(role, Some(crate::Role::Switch));
        assert_eq!(node.label(), Some("Wi-Fi"));
        assert_eq!(node.toggled(), Some(expected), "checked = {checked}");
    }
}

#[test]
fn slider_exposes_value_range_and_orientation() {
    let (role, node) = a11y_node(
        div()
            .id("volume")
            .role(crate::Role::Slider)
            .aria_label("Volume")
            .aria_numeric_value(42.)
            .aria_min_numeric_value(0.)
            .aria_max_numeric_value(100.)
            .aria_numeric_value_step(5.)
            .aria_value("42 percent")
            .aria_orientation(crate::Orientation::Horizontal),
    );
    assert_eq!(role, Some(crate::Role::Slider));
    assert_eq!(node.label(), Some("Volume"));
    assert_eq!(node.numeric_value(), Some(42.));
    assert_eq!(node.min_numeric_value(), Some(0.));
    assert_eq!(node.max_numeric_value(), Some(100.));
    assert_eq!(node.numeric_value_step(), Some(5.));
    assert_eq!(node.value(), Some("42 percent"));
    assert_eq!(node.orientation(), Some(crate::Orientation::Horizontal));
}

#[test]
fn selected_expanded_alert_status_and_link_roles() {
    let (role, node) = a11y_node(
        div()
            .id("tab")
            .role(crate::Role::Tab)
            .aria_label("General")
            .aria_selected(true)
            .aria_expanded(false),
    );
    assert_eq!(role, Some(crate::Role::Tab));
    assert_eq!(node.is_selected(), Some(true));
    assert_eq!(node.is_expanded(), Some(false));

    for (accessible_role, label) in [
        (crate::Role::Alert, "Disk full"),
        (crate::Role::Status, "Saved"),
        (crate::Role::Link, "Documentation"),
        (crate::Role::Button, "Save"),
    ] {
        let (role, node) = a11y_node(div().id(label).role(accessible_role).aria_label(label));
        assert_eq!(role, Some(accessible_role));
        assert_eq!(node.label(), Some(label));
    }
}

// ---------------------------------------------------------------------------
// R4: Shift+F10 context menu
// ---------------------------------------------------------------------------

struct MenuView {
    trigger: FocusHandle,
    root: FocusHandle,
    bounds: crate::keyboard_menu::BoundsTracker,
    menu: crate::keyboard_menu::KeyboardMenu,
}

impl Render for MenuView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::keyboard_menu::{MenuKey, is_context_menu_keystroke};
        let open = self.menu.is_open();
        div()
            .size_full()
            .track_focus(&self.root)
            .child(
                div().pl(px(20.)).pt(px(10.)).child(
                    div()
                        .id("trigger")
                        .w(px(100.))
                        .h(px(30.))
                        .track_focus(&self.trigger)
                        .child(self.bounds.element())
                        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                            if is_context_menu_keystroke(&event.keystroke)
                                && let Some(anchor) = this.bounds.bottom_left()
                            {
                                this.menu.open(anchor, window, cx);
                                cx.notify();
                                cx.stop_propagation();
                            }
                        })),
                ),
            )
            .when(open, |this| {
                let anchor = self.menu.anchor().unwrap_or_default();
                this.child(
                    div()
                        .id("menu")
                        .absolute()
                        .left(anchor.x)
                        .top(anchor.y)
                        .w(px(100.))
                        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                            if this.menu.handle_key_down(event, window, cx) != MenuKey::Ignored {
                                cx.notify();
                                cx.stop_propagation();
                            }
                        }))
                        .children((0..3).map(|ix| {
                            div()
                                .id(("item", ix))
                                .h(px(20.))
                                .track_focus(self.menu.item_focus(ix))
                        })),
                )
            })
    }
}

#[crate::test]
fn shift_f10_opens_a_menu_at_the_bottom_left_and_navigation_is_cyclic(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| {
        let root = cx.focus_handle();
        MenuView {
            trigger: cx.focus_handle(),
            menu: crate::keyboard_menu::KeyboardMenu::new(3, &root, cx),
            root,
            bounds: Default::default(),
        }
    });
    let trigger = view.read_with(cx, |view, _| view.trigger.clone());
    focus_and_draw(cx, &trigger);
    let item_ids = view.read_with(cx, |view, _| {
        (0..3)
            .map(|ix| view.menu.item_focus(ix).id())
            .collect::<Vec<_>>()
    });

    cx.simulate_keystrokes("shift-f10");
    draw(cx);
    let (open, anchor) = view.read_with(cx, |view, _| (view.menu.is_open(), view.menu.anchor()));
    assert!(open);
    // The trigger occupies (20, 10) with size 100 x 30: bottom-left is (20, 40).
    assert_eq!(anchor, Some(point(px(20.), px(40.))));
    assert_eq!(
        focused_id(cx),
        Some(item_ids[0]),
        "focus moved into the menu"
    );

    // Down and Tab go forward and wrap; Up and Shift+Tab go backward and wrap.
    for expected in [1, 2, 0] {
        cx.simulate_keystrokes("down");
        assert_eq!(focused_id(cx), Some(item_ids[expected]));
    }
    for expected in [2, 1, 0] {
        cx.simulate_keystrokes("up");
        assert_eq!(focused_id(cx), Some(item_ids[expected]));
    }
    for expected in [1, 2, 0] {
        cx.simulate_keystrokes("tab");
        assert_eq!(focused_id(cx), Some(item_ids[expected]));
    }
    for expected in [2, 1, 0] {
        cx.simulate_keystrokes("shift-tab");
        assert_eq!(focused_id(cx), Some(item_ids[expected]));
    }

    // Escape closes the menu and returns focus to the element that opened it.
    cx.simulate_keystrokes("escape");
    next_frame(cx);
    draw(cx);
    assert!(!view.read_with(cx, |view, _| view.menu.is_open()));
    assert_eq!(focused_id(cx), Some(trigger.id()));
}

// ---------------------------------------------------------------------------
// R5: cursor restoration
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Payload;

struct CursorView {
    drops: Rc<RefCell<Vec<&'static str>>>,
}

impl Render for CursorView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let accepted = self.drops.clone();
        let rejected = self.drops.clone();
        div()
            .size_full()
            .flex()
            .flex_row()
            // Source (0..100): open hand.
            .child(
                div()
                    .id("source")
                    .w(px(100.))
                    .h(px(100.))
                    .cursor_grab()
                    .on_drag(Payload, |_, _, _, cx| cx.new(|_| crate::Empty)),
            )
            // Accepting target (100..200): pointing hand.
            .child(
                div()
                    .id("accepting")
                    .w(px(100.))
                    .h(px(100.))
                    .cursor_pointer()
                    .on_drop(move |_: &Payload, _, _| accepted.borrow_mut().push("accepted")),
            )
            // Rejecting target (200..300): text cursor, refuses the payload.
            .child(
                div()
                    .id("rejecting")
                    .w(px(100.))
                    .h(px(100.))
                    .cursor_text()
                    .can_drop(|_, _, _| false)
                    .on_drop(move |_: &Payload, _, _| rejected.borrow_mut().push("rejected")),
            )
    }
}

fn start_drag(cx: &mut VisualTestContext) {
    cx.simulate_mouse_move(point(px(50.), px(50.)), None, Modifiers::none());
    cx.simulate_mouse_down(
        point(px(50.), px(50.)),
        crate::MouseButton::Left,
        Modifiers::none(),
    );
    cx.simulate_mouse_move(
        point(px(60.), px(50.)),
        Some(crate::MouseButton::Left),
        Modifiers::none(),
    );
    draw(cx);
    assert!(cx.update(|_, cx| cx.has_active_drag()));
}

fn set_drag_cursor(cx: &mut VisualTestContext, style: CursorStyle) {
    cx.update(|window, cx| {
        assert!(cx.set_active_drag_cursor_style(style, window));
    });
    draw(cx);
}

#[crate::test]
fn drag_cursor_overrides_hover_and_is_restored_when_the_drag_ends(cx: &mut TestAppContext) {
    let drops = Rc::new(RefCell::new(Vec::new()));
    let (_, cx) = cx.add_window_view({
        let drops = drops.clone();
        move |_, _| CursorView { drops }
    });
    cx.update(|window, _| window.activate_window());
    cx.simulate_hover_status_change(true);
    draw(cx);

    // Hover selects the innermost region's cursor; leaving restores it.
    cx.simulate_mouse_move(point(px(150.), px(50.)), None, Modifiers::none());
    assert_eq!(cx.active_cursor_style(), CursorStyle::PointingHand);
    cx.simulate_mouse_move(point(px(250.), px(50.)), None, Modifiers::none());
    assert_eq!(cx.active_cursor_style(), CursorStyle::IBeam);
    cx.simulate_mouse_move(point(px(350.), px(50.)), None, Modifiers::none());
    assert_eq!(cx.active_cursor_style(), CursorStyle::Arrow);
    cx.simulate_mouse_move(point(px(50.), px(50.)), None, Modifiers::none());
    assert_eq!(cx.active_cursor_style(), CursorStyle::OpenHand);

    // (release position, expected underlying cursor, expected drop record)
    let cases: [(Point<Pixels>, CursorStyle, Option<&'static str>); 3] = [
        (
            point(px(150.), px(50.)),
            CursorStyle::PointingHand,
            Some("accepted"),
        ),
        (point(px(250.), px(50.)), CursorStyle::IBeam, None),
        (point(px(350.), px(50.)), CursorStyle::Arrow, None),
    ];
    for (position, underlying, dropped) in cases {
        drops.borrow_mut().clear();
        start_drag(cx);
        set_drag_cursor(cx, CursorStyle::ClosedHand);
        // The drag cursor overrides whatever is hovered.
        cx.simulate_mouse_move(position, Some(crate::MouseButton::Left), Modifiers::none());
        draw(cx);
        assert_eq!(cx.active_cursor_style(), CursorStyle::ClosedHand);

        cx.simulate_mouse_up(position, crate::MouseButton::Left, Modifiers::none());
        draw(cx);
        assert!(!cx.update(|_, cx| cx.has_active_drag()));
        assert_eq!(
            cx.active_cursor_style(),
            underlying,
            "after a drop at {position:?}"
        );
        assert_eq!(drops.borrow().first().copied(), dropped);
    }

    // Cancelled by Escape.
    start_drag(cx);
    set_drag_cursor(cx, CursorStyle::ClosedHand);
    cx.simulate_mouse_move(
        point(px(150.), px(50.)),
        Some(crate::MouseButton::Left),
        Modifiers::none(),
    );
    draw(cx);
    assert_eq!(cx.active_cursor_style(), CursorStyle::ClosedHand);
    cx.update(|window, cx| assert!(cx.stop_active_drag(window)));
    draw(cx);
    assert_eq!(cx.active_cursor_style(), CursorStyle::PointingHand);
}

// ---------------------------------------------------------------------------
// R6: raw key observation and hold gestures
// ---------------------------------------------------------------------------

/// A focused surface that is composing text (input method pre-edit active)
/// and observes raw key and modifier events.
#[derive(Clone)]
struct ComposingSurface {
    focus: FocusHandle,
    log: Rc<RefCell<Vec<String>>>,
    hold: Rc<RefCell<Option<Entity<crate::hold_gesture::ModifierHold>>>>,
}

impl Element for ComposingSurface {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some("composing-surface".into())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let style = Style {
            size: size(px(50.).into(), px(50.).into()),
            ..Style::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        window.set_focus_handle(&self.focus, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.handle_input(&self.focus, self.clone(), cx);
        let log = self.log.clone();
        window.on_key_event(move |event: &KeyDownEvent, phase, _, _| {
            if phase == DispatchPhase::Bubble {
                log.borrow_mut().push(format!(
                    "down:{}:held={}",
                    event.keystroke.key, event.is_held
                ));
            }
        });
        let log = self.log.clone();
        window.on_key_event(move |event: &KeyUpEvent, phase, _, _| {
            if phase == DispatchPhase::Bubble {
                log.borrow_mut().push(format!("up:{}", event.keystroke.key));
            }
        });
        let log = self.log.clone();
        let hold = self.hold.clone();
        window.on_modifiers_changed(move |event: &ModifiersChangedEvent, window, cx| {
            log.borrow_mut().push(format!(
                "mods:ctrl={},alt={},shift={}",
                event.modifiers.control, event.modifiers.alt, event.modifiers.shift
            ));
            if let Some(hold) = hold.borrow().as_ref() {
                hold.update(cx, |hold, cx| hold.modifiers_changed(event, window, cx));
            }
        });
    }
}

impl IntoElement for ComposingSurface {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl InputHandler for ComposingSurface {
    fn accepts_text_input(&mut self, _: &mut Window, _: &mut App) -> bool {
        true
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut App,
    ) -> Option<UTF16Selection> {
        None
    }

    /// Composition is in progress: the pre-edit text occupies 0..3.
    fn marked_text_range(&mut self, _: &mut Window, _: &mut App) -> Option<Range<usize>> {
        Some(0..3)
    }

    fn text_for_range(
        &mut self,
        _: Range<usize>,
        _: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut App,
    ) -> Option<String> {
        None
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut App) {}

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        _text: &str,
        _marked: Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }

    fn replace_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        _text: &str,
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        _: &mut Window,
        _: &mut App,
    ) -> Option<Bounds<Pixels>> {
        None
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut App,
    ) -> Option<usize> {
        None
    }
}

struct SurfaceView {
    surface: ComposingSurface,
    other: FocusHandle,
}

impl Render for SurfaceView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(AnyElement::new(self.surface.clone()))
            .child(div().id("other").size(px(20.)).track_focus(&self.other))
    }
}

fn surface_view(
    cx: &mut TestAppContext,
) -> (
    Entity<SurfaceView>,
    &mut VisualTestContext,
    Rc<RefCell<Vec<String>>>,
) {
    let log = Rc::new(RefCell::new(Vec::new()));
    let (view, cx) = cx.add_window_view({
        let log = log.clone();
        move |_, cx| SurfaceView {
            surface: ComposingSurface {
                focus: cx.focus_handle(),
                log,
                hold: Default::default(),
            },
            other: cx.focus_handle(),
        }
    });
    let focus = view.read_with(cx, |view, _| view.surface.focus.clone());
    focus_and_draw(cx, &focus);
    (view, cx, log)
}

#[crate::test]
fn raw_key_events_reach_a_surface_that_is_composing_text(cx: &mut TestAppContext) {
    let (_, cx, log) = surface_view(cx);
    // The surface has claimed text input (a composition is in progress), so
    // the keystrokes below are character input rather than shortcuts; raw
    // observers still see all of them.
    cx.simulate_event(KeyDownEvent {
        keystroke: crate::Keystroke::parse("b").expect("keystroke"),
        is_held: false,
        prefer_character_input: true,
    });
    assert_eq!(log.borrow().as_slice(), ["down:b:held=false"]);
    log.borrow_mut().clear();

    key_down(cx, "a", false);
    key_down(cx, "a", true);
    key_up(cx, "a");
    cx.simulate_modifiers_change(Modifiers {
        control: true,
        ..Modifiers::none()
    });
    cx.simulate_modifiers_change(Modifiers::none());
    assert_eq!(
        log.borrow().as_slice(),
        [
            "down:a:held=false",
            "down:a:held=true",
            "up:a",
            "mods:ctrl=true,alt=false,shift=false",
            "mods:ctrl=false,alt=false,shift=false",
        ]
    );
}

#[crate::test]
fn hold_gestures_end_on_modifier_release_focus_loss_and_deactivation(cx: &mut TestAppContext) {
    use crate::hold_gesture::{HoldEnd, ModifierHold, ModifierHoldEvent};
    let (view, cx, _log) = surface_view(cx);
    let (surface_focus, other, hold_slot) = view.read_with(cx, |view, _| {
        (
            view.surface.focus.clone(),
            view.other.clone(),
            view.surface.hold.clone(),
        )
    });
    let required = Modifiers {
        control: true,
        alt: true,
        ..Modifiers::none()
    };
    let hold = cx
        .update(|window, cx| cx.new(|cx| ModifierHold::new(required, &surface_focus, window, cx)));
    *hold_slot.borrow_mut() = Some(hold.clone());
    let events = Rc::new(RefCell::new(Vec::new()));
    let _subscription = cx.update(|_, cx| {
        let events = events.clone();
        cx.subscribe(&hold, move |_, event: &ModifierHoldEvent, _| {
            events.borrow_mut().push(*event)
        })
    });
    draw(cx);
    let held = Modifiers {
        control: true,
        alt: true,
        ..Modifiers::none()
    };

    // Releasing one required modifier cancels the gesture.
    cx.simulate_modifiers_change(Modifiers {
        control: true,
        ..Modifiers::none()
    });
    assert!(
        events.borrow().is_empty(),
        "not all required modifiers held yet"
    );
    cx.simulate_modifiers_change(held);
    assert_eq!(*events.borrow(), vec![ModifierHoldEvent::Started]);
    assert!(hold.read_with(cx, |hold, _| hold.is_active()));
    cx.simulate_modifiers_change(Modifiers {
        control: true,
        ..Modifiers::none()
    });
    assert_eq!(
        *events.borrow(),
        vec![
            ModifierHoldEvent::Started,
            ModifierHoldEvent::Ended(HoldEnd::ModifierReleased)
        ]
    );
    events.borrow_mut().clear();

    // Moving focus away cancels it.
    cx.simulate_modifiers_change(held);
    assert_eq!(*events.borrow(), vec![ModifierHoldEvent::Started]);
    focus_and_draw(cx, &other);
    assert_eq!(
        *events.borrow(),
        vec![
            ModifierHoldEvent::Started,
            ModifierHoldEvent::Ended(HoldEnd::FocusLost)
        ]
    );
    events.borrow_mut().clear();
    // It does not start while the surface is not focused.
    cx.simulate_modifiers_change(Modifiers::none());
    cx.simulate_modifiers_change(held);
    assert!(events.borrow().is_empty());

    // Deactivating the window cancels it.
    focus_and_draw(cx, &surface_focus);
    cx.simulate_modifiers_change(Modifiers::none());
    cx.simulate_modifiers_change(held);
    assert_eq!(*events.borrow(), vec![ModifierHoldEvent::Started]);
    cx.deactivate_window();
    assert_eq!(
        *events.borrow(),
        vec![
            ModifierHoldEvent::Started,
            ModifierHoldEvent::Ended(HoldEnd::WindowDeactivated)
        ]
    );
}

// ---------------------------------------------------------------------------
// R1: focus-scoped actions (the behavior of the focus_scoped_shortcuts example)
// ---------------------------------------------------------------------------

actions!(input_scope_tests, [PaneAAction, PaneBAction]);

struct ScopedShortcutsView {
    pane_a: FocusHandle,
    pane_b: FocusHandle,
    modal: FocusHandle,
    modal_open: bool,
    active: &'static str,
    log: Rc<RefCell<Vec<&'static str>>>,
}

impl Render for ScopedShortcutsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.pane_a.contains_focused(window, cx) {
            self.active = "PaneA";
        } else if self.pane_b.contains_focused(window, cx) {
            self.active = "PaneB";
        }
        let (log_a, log_b) = (self.log.clone(), self.log.clone());
        div()
            .size_full()
            .key_context(self.active)
            .on_action(move |_: &PaneAAction, _, _| log_a.borrow_mut().push("A"))
            .on_action(move |_: &PaneBAction, _, _| log_b.borrow_mut().push("B"))
            .child(div().id("a").size(px(20.)).track_focus(&self.pane_a))
            .child(div().id("b").size(px(20.)).track_focus(&self.pane_b))
            .when(self.modal_open, |this| {
                this.child(
                    div()
                        .id("modal")
                        .size(px(20.))
                        .key_context("Modal")
                        .track_focus(&self.modal),
                )
            })
    }
}

#[crate::test]
fn one_shortcut_runs_per_focused_pane_and_a_modal_blocks_it_for_both(cx: &mut TestAppContext) {
    use crate::{KeyBinding, NoAction};
    cx.update(|cx| {
        cx.bind_keys([
            KeyBinding::new("ctrl-k", PaneAAction, Some("PaneA")),
            KeyBinding::new("ctrl-k", PaneBAction, Some("PaneB")),
            // The no-action marker in the more specific context suppresses the
            // outranked pane bindings for that keystroke.
            KeyBinding::new("ctrl-k", NoAction, Some("Modal")),
        ])
    });
    let log = Rc::new(RefCell::new(Vec::new()));
    let (view, cx) = cx.add_window_view({
        let log = log.clone();
        move |_, cx| ScopedShortcutsView {
            pane_a: cx.focus_handle(),
            pane_b: cx.focus_handle(),
            modal: cx.focus_handle(),
            modal_open: false,
            active: "PaneA",
            log,
        }
    });
    let (pane_a, pane_b, modal) = view.read_with(cx, |view, _| {
        (view.pane_a.clone(), view.pane_b.clone(), view.modal.clone())
    });
    let set_modal = |cx: &mut VisualTestContext, open: bool, focus: &FocusHandle| {
        view.update(cx, |view, cx| {
            view.modal_open = open;
            cx.notify();
        });
        focus_and_draw(cx, focus);
    };
    let take = || std::mem::take(&mut *log.borrow_mut());

    focus_and_draw(cx, &pane_a);
    cx.simulate_keystrokes("ctrl-k");
    focus_and_draw(cx, &pane_b);
    cx.simulate_keystrokes("ctrl-k");
    assert_eq!(take(), ["A", "B"], "the focused pane selects the action");

    // The modal opened over pane B blocks the shortcut...
    set_modal(cx, true, &modal);
    cx.simulate_keystrokes("ctrl-k");
    assert!(take().is_empty(), "blocked over pane B");
    // ...and closing it restores it.
    set_modal(cx, false, &pane_b);
    cx.simulate_keystrokes("ctrl-k");
    assert_eq!(take(), ["B"]);

    // The same over pane A.
    focus_and_draw(cx, &pane_a);
    set_modal(cx, true, &modal);
    cx.simulate_keystrokes("ctrl-k");
    assert!(take().is_empty(), "blocked over pane A");
    set_modal(cx, false, &pane_a);
    cx.simulate_keystrokes("ctrl-k");
    assert_eq!(take(), ["A"]);
}

// ---------------------------------------------------------------------------
// Modal scope (CMP-003 R14) and scoped tab traversal (INP-004)
// ---------------------------------------------------------------------------

actions!(input_scope_tests, [ScopeProbe]);

struct ModalScopeView {
    outside: FocusHandle,
    first: FocusHandle,
    second: FocusHandle,
    scope: FocusHandle,
    log: Rc<RefCell<Vec<&'static str>>>,
}

impl Render for ModalScopeView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let (action_log, key_log) = (self.log.clone(), self.log.clone());
        div()
            .size_full()
            .on_action(move |_: &ScopeProbe, _, _| action_log.borrow_mut().push("window"))
            .on_key_down(move |_, _, _| key_log.borrow_mut().push("window-key"))
            .child(div().id("outside").size(px(20.)).track_focus(&self.outside))
            .child(
                div()
                    .id("scope")
                    .key_context(vespertine::MODAL_SCOPE_CONTEXT)
                    .track_focus(&self.scope)
                    .child(div().id("first").size(px(20.)).track_focus(&self.first))
                    .child(div().id("second").size(px(20.)).track_focus(&self.second)),
            )
    }
}

#[crate::test]
fn a_modal_scope_blocks_window_level_listeners_above_it_but_not_app_level_actions(
    cx: &mut TestAppContext,
) {
    use crate::KeyBinding;
    let log = Rc::new(RefCell::new(Vec::new()));
    cx.update(|cx| {
        cx.bind_keys([KeyBinding::new("ctrl-j", ScopeProbe, None)]);
        let app_log = log.clone();
        cx.on_action(move |_: &ScopeProbe, _| app_log.borrow_mut().push("app"));
    });
    let (view, cx) = cx.add_window_view({
        let log = log.clone();
        move |_, cx| ModalScopeView {
            outside: cx.focus_handle().tab_stop(true),
            first: cx.focus_handle().tab_stop(true),
            second: cx.focus_handle().tab_stop(true),
            scope: cx.focus_handle(),
            log,
        }
    });
    let (outside, first, second, scope) = view.read_with(cx, |view, _| {
        (
            view.outside.clone(),
            view.first.clone(),
            view.second.clone(),
            view.scope.clone(),
        )
    });
    let take = || std::mem::take(&mut *log.borrow_mut());

    focus_and_draw(cx, &outside);
    cx.simulate_keystrokes("ctrl-j x");
    assert_eq!(take(), ["window", "window-key"], "outside the scope");

    focus_and_draw(cx, &first);
    cx.simulate_keystrokes("ctrl-j x");
    assert_eq!(
        take(),
        ["app"],
        "inside the scope the window-level action and key listeners above it do not run, the \
         application-level action does"
    );

    // Scoped traversal wraps inside the scope and never visits the page.
    cx.update(|window, cx| assert!(window.focus_next_within(&scope, cx)));
    assert_eq!(focused_id(cx), Some(second.id()));
    cx.update(|window, cx| assert!(window.focus_next_within(&scope, cx)));
    assert_eq!(focused_id(cx), Some(first.id()), "wraps last to first");
    cx.update(|window, cx| assert!(window.focus_prev_within(&scope, cx)));
    assert_eq!(focused_id(cx), Some(second.id()), "wraps first to last");
    focus_and_draw(cx, &outside);
    cx.update(|window, cx| assert!(window.focus_next_within(&scope, cx)));
    assert_eq!(focused_id(cx), Some(first.id()), "enters at the first");
    focus_and_draw(cx, &outside);
    cx.update(|window, cx| assert!(window.focus_prev_within(&scope, cx)));
    assert_eq!(
        focused_id(cx),
        Some(second.id()),
        "reverse enters at the last"
    );
}
