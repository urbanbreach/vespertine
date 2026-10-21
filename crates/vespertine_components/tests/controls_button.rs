//! CMP-001 native button: A1 (activation rules), A3, A4, A10-facing accessibility, R7-R11 states
//! and A11 (focus indicator).

mod controls_support;

use std::{cell::Cell, rc::Rc};

use controls_support::*;
use vespertine::{IntoElement, ParentElement, Styled, div, px, rgb};
use vespertine_components::controls::{Button, disabled_group, effective_disabled};
use vespertine_test_host::require_backend;

/// A button column: [0] `a`, [1] `b`, each 120 x 36 logical px at x = 20.
fn column(
    clicks: Rc<[Cell<u32>; 2]>,
    configure: impl Fn(usize, Button) -> Button + 'static,
) -> impl Fn(&mut vespertine::Window, &mut vespertine::App) -> vespertine::AnyElement {
    move |_, _| {
        let mut column = div()
            .p(px(20.))
            .flex()
            .flex_col()
            .items_start()
            .gap(px(12.));
        for index in 0..2 {
            let clicks = clicks.clone();
            let button = Button::new(if index == 0 { "a" } else { "b" })
                .label(if index == 0 { "Alpha" } else { "Beta" })
                .w(px(120.))
                .on_click(move |_, window, _| {
                    clicks[index].set(clicks[index].get() + 1);
                    window.refresh();
                });
            column = column.child(configure(index, button));
        }
        column.into_any_element()
    }
}

fn counts() -> Rc<[Cell<u32>; 2]> {
    Rc::new([Cell::new(0), Cell::new(0)])
}

#[test]
fn controls_a1_enter_activates_once_per_physical_press_and_space_on_release() {
    require_backend!();
    let clicks = counts();
    let mut host = host(300.0, 200.0, column(clicks.clone(), |_, button| button));
    tab(&mut host); // focus Alpha, keyboard modality
    let alpha = || clicks[0].get();

    // One physical Enter press: the first key-down activates; a synthesized second key-down
    // that is not flagged as held, a flagged repeat and more repeats do not.
    host.key_down("enter", false, None).unwrap();
    assert_eq!(alpha(), 1, "first key-down activates");
    host.key_down("enter", false, None).unwrap();
    host.key_down("enter", true, None).unwrap();
    host.key_down("enter", false, None).unwrap();
    assert_eq!(alpha(), 1, "unflagged and flagged repeats are ignored");
    host.key_up("enter", None).unwrap();
    assert_eq!(
        alpha(),
        1,
        "the release does not activate a second time (base keyboard-click)"
    );

    // The next physical press activates again.
    host.key_down("enter", false, None).unwrap();
    host.key_up("enter", None).unwrap();
    assert_eq!(alpha(), 2);

    // Space: nothing on key-down (or on repeats), one activation on key-up.
    host.key_down("space", false, None).unwrap();
    host.key_down("space", true, None).unwrap();
    assert_eq!(alpha(), 2, "Space does not activate on key-down");
    host.key_up("space", None).unwrap();
    assert_eq!(alpha(), 3, "Space activates on key-up");
    host.key_up("space", None).unwrap();
    assert_eq!(alpha(), 3, "a lone release does nothing");

    // Another key between press and release cancels the Space activation.
    host.key_down("space", false, None).unwrap();
    host.key_down("a", false, None).unwrap();
    host.key_up("space", None).unwrap();
    assert_eq!(alpha(), 3);

    // Modified Enter is not an activation.
    host.key_down("shift-enter", false, None).unwrap();
    host.key_up("shift-enter", None).unwrap();
    assert_eq!(alpha(), 3);
    assert_eq!(clicks[1].get(), 0, "the other button never activated");
}

#[test]
fn controls_a1_pointer_click_activates_once_and_disabled_never() {
    require_backend!();
    let clicks = counts();
    let mut host = host(
        300.0,
        200.0,
        column(clicks.clone(), |index, button| button.disabled(index == 1)),
    );
    host.click(60.0, 38.0, 0, "").unwrap();
    assert_eq!(clicks[0].get(), 1);
    host.click(60.0, 86.0, 0, "").unwrap();
    assert_eq!(clicks[1].get(), 0, "no activation while disabled");
    // A right click is not an activation.
    host.click(60.0, 38.0, 2, "").unwrap();
    assert_eq!(clicks[0].get(), 1);
}

#[test]
fn controls_a3_disabled_button_leaves_the_tab_order_and_never_activates() {
    require_backend!();
    let clicks = counts();
    let mut host = host(
        300.0,
        200.0,
        column(clicks.clone(), |index, button| button.disabled(index == 0)),
    );
    // Sequential focus skips the disabled first button and lands on the second.
    tab(&mut host);
    host.key_down("enter", false, None).unwrap();
    host.key_up("enter", None).unwrap();
    assert_eq!(clicks[0].get(), 0);
    assert_eq!(clicks[1].get(), 1, "Tab skipped the disabled button");

    // Click and assistive technology cannot activate it either.
    host.click(60.0, 38.0, 0, "").unwrap();
    let alpha = node(&mut host, "button", Some("Alpha"));
    assert!(alpha.disabled());
    assert!(!alpha.can_click(), "no click action is offered");
    a11y_click(&mut host, &alpha);
    assert_eq!(clicks[0].get(), 0);
}

#[test]
fn controls_a3_focusable_when_disabled_is_focusable_but_never_activates() {
    require_backend!();
    let clicks = counts();
    let mut host = host(
        300.0,
        200.0,
        column(clicks.clone(), |index, button| {
            button
                .disabled(index == 0)
                .focusable_when_disabled(index == 0)
        }),
    );
    // Not a tab stop: Tab goes to the enabled button.
    tab(&mut host);
    host.key_down("enter", false, None).unwrap();
    host.key_up("enter", None).unwrap();
    assert_eq!(clicks[1].get(), 1);
    // A pointer press focuses it, but no key or click activates it.
    host.click(60.0, 38.0, 0, "").unwrap();
    for key in ["enter", "space"] {
        host.key_down(key, false, None).unwrap();
        host.key_up(key, None).unwrap();
    }
    assert_eq!(clicks[0].get(), 0);
    assert_eq!(
        clicks[1].get(),
        1,
        "the focused disabled button took the keys"
    );
}

#[test]
fn controls_a4_disabled_group_disables_enabled_looking_children() {
    require_backend!();
    let clicks = counts();
    let inner = clicks.clone();
    let mut host = host(300.0, 200.0, move |window, cx| {
        let _ = (&window, &cx);
        let clicks = inner.clone();
        let group = div()
            .p(px(20.))
            .flex()
            .flex_col()
            .items_start()
            .gap(px(12.))
            .child(Button::new("child").label("Child").w(px(120.)).on_click({
                let clicks = clicks.clone();
                move |_, _, _| clicks[0].set(clicks[0].get() + 1)
            }));
        let outside = Button::new("outside")
            .label("Outside")
            .w(px(120.))
            .on_click(move |_, _, _| clicks[1].set(clicks[1].get() + 1));
        div()
            .child(disabled_group(true, group))
            .child(div().px(px(20.)).child(outside))
            .into_any_element()
    });

    // The child exposes role, label and the disabled state.
    let child = node(&mut host, "button", Some("Child"));
    assert!(child.disabled());
    assert!(!child.can_click());
    let outside = node(&mut host, "button", Some("Outside"));
    assert!(
        !outside.disabled(),
        "controls outside the group are unaffected"
    );

    // It is skipped by sequential focus: Tab lands on the control outside the group.
    tab(&mut host);
    host.key_down("enter", false, None).unwrap();
    host.key_up("enter", None).unwrap();
    assert_eq!(clicks[0].get(), 0);
    assert_eq!(clicks[1].get(), 1);

    // It does not activate by pointer, keyboard or assistive technology.
    host.click(60.0, 38.0, 0, "").unwrap();
    a11y_click(&mut host, &child);
    host.key_down("space", false, None).unwrap();
    host.key_up("space", None).unwrap();
    assert_eq!(clicks[0].get(), 0);
}

/// A button in its own view, recording the disabled state it resolved when it was last rendered.
struct GroupedButton {
    resolved: Rc<Cell<Option<bool>>>,
    label: &'static str,
}

impl vespertine::Render for GroupedButton {
    fn render(
        &mut self,
        _: &mut vespertine::Window,
        cx: &mut vespertine::Context<Self>,
    ) -> impl IntoElement {
        self.resolved.set(Some(effective_disabled(false, cx)));
        Button::new("grouped").label(self.label).w(px(120.))
    }
}

struct DisabledForm {
    disabled: bool,
    button: vespertine::Entity<GroupedButton>,
}

impl vespertine::Render for DisabledForm {
    fn render(
        &mut self,
        _: &mut vespertine::Window,
        _: &mut vespertine::Context<Self>,
    ) -> impl IntoElement {
        div()
            .p(px(20.))
            .child(disabled_group(self.disabled, self.button.clone()))
    }
}

/// The disabled context reaches a control in a nested view that is built again on its own, and
/// a nested view that did not change itself follows its group.
#[test]
fn controls_a4_disabled_group_reaches_a_nested_view_built_on_its_own() {
    use vespertine::AppContext as _;
    require_backend!();
    let resolved = Rc::new(Cell::new(None));
    let views = Rc::new(std::cell::RefCell::new(None));
    let mut host = vespertine_test_host::TestHost::with_view(
        vespertine_test_host::HostOptions {
            complete_frames: false,
            ..vespertine_test_host::HostOptions::with_size(300.0, 120.0)
        },
        {
            let (resolved, views) = (resolved.clone(), views.clone());
            move |_, cx| {
                let button = cx.new(|_| GroupedButton {
                    resolved,
                    label: "Alpha",
                });
                let form = cx.new(|_| DisabledForm {
                    disabled: true,
                    button: button.clone(),
                });
                *views.borrow_mut() = Some((form.clone(), button));
                form
            }
        },
    )
    .unwrap();
    let (form, button) = views.borrow_mut().take().unwrap();
    assert_eq!(resolved.get(), Some(true));

    // Only the button's view changes.
    resolved.set(None);
    host.app().update(|cx| {
        button.update(cx, |button, cx| {
            button.label = "Beta";
            cx.notify();
        })
    });
    host.flush();
    assert_eq!(resolved.get(), Some(true), "the rebuilt button is disabled");

    // Only the group changes.
    resolved.set(None);
    host.app().update(|cx| {
        form.update(cx, |form, cx| {
            form.disabled = false;
            cx.notify();
        })
    });
    host.flush();
    assert_eq!(
        resolved.get(),
        Some(false),
        "the button is rendered again, enabled with its group"
    );
}

#[test]
fn controls_busy_button_exposes_busy_stays_focusable_and_starts_nothing() {
    require_backend!();
    let clicks = counts();
    let mut host = host(
        300.0,
        200.0,
        column(clicks.clone(), |index, button| button.busy(index == 0)),
    );
    let alpha = node(&mut host, "button", Some("Alpha"));
    assert!(alpha.busy());
    assert!(!alpha.disabled());
    // Focusable: the first Tab lands on the busy button, which swallows no activation.
    tab(&mut host);
    host.key_down("enter", false, None).unwrap();
    host.key_up("enter", None).unwrap();
    host.key_down("space", false, None).unwrap();
    host.key_up("space", None).unwrap();
    host.click(60.0, 38.0, 0, "").unwrap();
    assert_eq!(clicks[0].get(), 0, "a busy control starts no activation");
    // Focus really was on the busy button (the second button received nothing either).
    assert_eq!(clicks[1].get(), 0);
    tab(&mut host);
    host.key_down("enter", false, None).unwrap();
    host.key_up("enter", None).unwrap();
    assert_eq!(
        clicks[1].get(),
        1,
        "the second Tab reached the second button"
    );
}

#[test]
fn controls_button_exposes_selected_as_toggled_and_icon_only_label() {
    require_backend!();
    let mut host = host(300.0, 200.0, |_, _| {
        div()
            .p(px(20.))
            .flex()
            .flex_col()
            .items_start()
            .gap(px(12.))
            .child(Button::new("on").label("Bold").selected(true))
            .child(Button::new("plain").label("Plain"))
            .child(
                Button::icon_only(
                    "save",
                    "Save document",
                    div().size(px(14.)).bg(rgb(0xffffff)),
                )
                .disabled(true),
            )
            .into_any_element()
    });
    let bold = node(&mut host, "button", Some("Bold"));
    assert_eq!(bold.text("toggled"), Some("true"));
    let plain = node(&mut host, "button", Some("Plain"));
    assert_eq!(
        plain.text("toggled"),
        None,
        "not a toggle unless selected is given"
    );
    // A disabled icon-only button still exposes its role, label and disabled state.
    let save = node(&mut host, "button", Some("Save document"));
    assert!(save.disabled());
}

/// Two icon-only buttons: `save` (label is the tooltip) at (20, 20) and `find` (explicit
/// tooltip text) at (20, 68), each 36 x 36.
fn icon_column(
    clicks: Rc<[Cell<u32>; 2]>,
) -> impl Fn(&mut vespertine::Window, &mut vespertine::App) -> vespertine::AnyElement {
    move |_, _| {
        let (save, find) = (clicks.clone(), clicks.clone());
        div()
            .p(px(20.))
            .flex()
            .flex_col()
            .items_start()
            .gap(px(12.))
            .child(
                Button::icon_only(
                    "save",
                    "Save document",
                    div().size(px(14.)).bg(rgb(0xffffff)),
                )
                .on_click(move |_, window, _| {
                    save[0].set(save[0].get() + 1);
                    window.refresh();
                }),
            )
            .child(
                Button::icon_only("find", "Find", div().size(px(14.)).bg(rgb(0xffffff)))
                    .tooltip_text("Find in file (Ctrl+F)")
                    .on_click(move |_, window, _| {
                        find[1].set(find[1].get() + 1);
                        window.refresh();
                    }),
            )
            .into_any_element()
    }
}

fn shows(host: &vespertine_test_host::TestHost, text: &str) -> bool {
    host.painted_text().iter().any(|painted| painted == text)
}

/// Focus listeners run when the window's effects flush, after the frame that moved focus; the
/// state they change draws in the next frame.
fn settle(host: &mut vespertine_test_host::TestHost) {
    host.flush();
    host.app().update(|_| ());
    host.flush();
}

#[test]
fn controls_icon_only_button_shows_its_label_as_the_tooltip_after_500_ms_of_hover() {
    require_backend!();
    let mut host = host(300.0, 200.0, icon_column(counts()));
    assert!(!host.painted_text().iter().any(|text| text.contains("Save")));
    host.mouse_move(38.0, 38.0, None, "").unwrap();
    host.advance_executor_time(499.0).unwrap();
    assert!(!shows(&host, "Save document"), "not yet at 499 ms");
    host.advance_executor_time(1.0).unwrap();
    assert!(
        shows(&host, "Save document"),
        "the label is the tooltip text: {:?}",
        host.painted_text()
    );
    // Leaving hides it as soon as the pointer is off the control.
    host.mouse_move(250.0, 180.0, None, "").unwrap();
    host.flush();
    assert!(!shows(&host, "Save document"));
    // An explicit tooltip text wins over the label.
    host.mouse_move(38.0, 86.0, None, "").unwrap();
    host.advance_executor_time(1000.0).unwrap();
    assert!(
        shows(&host, "Find in file (Ctrl+F)"),
        "{:?}",
        host.painted_text()
    );
    // The accessible name is the label either way.
    node(&mut host, "button", Some("Find"));
}

#[test]
fn controls_icon_only_tooltip_opens_at_once_for_keyboard_focus_and_never_takes_focus() {
    require_backend!();
    let clicks = counts();
    let mut host = host(300.0, 200.0, icon_column(clicks.clone()));
    // Tab: keyboard modality, so the tooltip opens with no delay and no time passing.
    tab(&mut host);
    settle(&mut host);
    assert!(
        shows(&host, "Save document"),
        "Tab opens the label tooltip at once: {:?}",
        host.painted_text()
    );
    // The surface holds no focus: Enter still activates the focused button.
    host.key_down("enter", false, None).unwrap();
    host.key_up("enter", None).unwrap();
    assert_eq!(clicks[0].get(), 1, "focus stayed on the button");
    // Escape closes it (through the dismissal stack) without moving focus.
    host.simulate_keystrokes("escape", None).unwrap();
    settle(&mut host);
    assert!(!shows(&host, "Save document"), "Escape closes it");
    host.key_down("enter", false, None).unwrap();
    host.key_up("enter", None).unwrap();
    assert_eq!(clicks[0].get(), 2, "closing did not move focus");
    // Tab on to the next button: the first closes (blur), the second opens its own text.
    tab(&mut host);
    settle(&mut host);
    assert!(shows(&host, "Find in file (Ctrl+F)"));
    assert!(!shows(&host, "Save document"));
    // Blur closes it.
    host.blur().unwrap();
    settle(&mut host);
    assert!(!shows(&host, "Find in file (Ctrl+F)"), "blur closes it");
}

#[test]
fn controls_icon_only_tooltip_does_not_open_for_a_mouse_press_and_a_press_closes_it() {
    require_backend!();
    let clicks = counts();
    let mut host = host(300.0, 200.0, icon_column(clicks.clone()));
    // A press focuses the button by pointer: no tooltip stays open (the pointer then rests
    // elsewhere so the hover tooltip is not involved).
    host.click(38.0, 38.0, 0, "").unwrap();
    host.mouse_move(250.0, 180.0, None, "").unwrap();
    settle(&mut host);
    assert_eq!(clicks[0].get(), 1);
    assert!(
        !shows(&host, "Save document"),
        "pointer-initiated focus leaves nothing open"
    );
    // Keyboard focus opens the second button's tooltip; a press on it closes it.
    tab(&mut host);
    settle(&mut host);
    assert!(shows(&host, "Find in file (Ctrl+F)"));
    host.mouse_down(38.0, 86.0, 0, "").unwrap();
    settle(&mut host);
    assert!(!shows(&host, "Find in file (Ctrl+F)"), "a press closes it");
}

/// The fill sampled inside the button box at (30, 38), clear of the label.
fn fill(host: &mut vespertine_test_host::TestHost) -> [u8; 3] {
    host.flush();
    pixel(host, 30.0, 38.0)
}

#[test]
fn controls_button_states_share_one_box_and_follow_precedence() {
    require_backend!();
    let busy = Rc::new(Cell::new(false));
    let disabled = Rc::new(Cell::new(false));
    let selected = Rc::new(Cell::new(false));
    let (b, d, s) = (busy.clone(), disabled.clone(), selected.clone());
    let mut host = host(300.0, 120.0, move |_, _| {
        div()
            .p(px(20.))
            .child(
                Button::new("state")
                    .label("State")
                    .w(px(120.))
                    .busy(b.get())
                    .disabled(d.get())
                    .selected(s.get()),
            )
            .into_any_element()
    });
    let reference = node(&mut host, "button", Some("State")).bounds();
    let same_box = |host: &mut vespertine_test_host::TestHost, what: &str| {
        assert_eq!(
            node(host, "button", Some("State")).bounds(),
            reference,
            "{what}"
        );
    };

    // Default.
    host.mouse_move(250.0, 100.0, None, "").unwrap();
    assert!(
        is_color(fill(&mut host), 0x30343D, 2),
        "default {:?}",
        fill(&mut host)
    );
    // Hovered.
    host.mouse_move(80.0, 38.0, None, "").unwrap();
    assert!(
        is_color(fill(&mut host), 0x3A3F49, 2),
        "hovered {:?}",
        fill(&mut host)
    );
    same_box(&mut host, "hovered");
    // Pressed beats hovered.
    host.mouse_down(80.0, 38.0, 0, "").unwrap();
    assert!(
        is_color(fill(&mut host), 0x262A31, 2),
        "pressed {:?}",
        fill(&mut host)
    );
    same_box(&mut host, "pressed");
    host.mouse_up(80.0, 38.0, 0, "").unwrap();

    // Busy beats pressed and hovered: the pointer is still over the button, nothing changes.
    busy.set(true);
    host.paint_frame();
    host.mouse_move(250.0, 100.0, None, "").unwrap();
    let busy_rest = fill(&mut host);
    assert!(is_color(busy_rest, 0x30343D, 2), "busy {busy_rest:?}");
    same_box(&mut host, "busy");
    host.mouse_move(80.0, 38.0, None, "").unwrap();
    assert_eq!(fill(&mut host), busy_rest, "hover is suppressed while busy");
    host.mouse_down(80.0, 38.0, 0, "").unwrap();
    assert_eq!(fill(&mut host), busy_rest, "press is suppressed while busy");
    host.mouse_up(80.0, 38.0, 0, "").unwrap();

    // Disabled beats busy: dimmed, and hover or press change nothing.
    disabled.set(true);
    host.paint_frame();
    host.mouse_move(250.0, 100.0, None, "").unwrap();
    let disabled_rest = fill(&mut host);
    assert_ne!(disabled_rest, busy_rest, "disabled paints dimmed");
    same_box(&mut host, "disabled");
    host.mouse_move(80.0, 38.0, None, "").unwrap();
    assert_eq!(fill(&mut host), disabled_rest);
    host.mouse_down(80.0, 38.0, 0, "").unwrap();
    assert_eq!(fill(&mut host), disabled_rest);
    host.mouse_up(80.0, 38.0, 0, "").unwrap();
    disabled.set(false);
    busy.set(false);
    host.paint_frame();

    // Selected is an overlay: the accent fill in the rest state, still hover/press reactive.
    selected.set(true);
    host.paint_frame();
    host.mouse_move(250.0, 100.0, None, "").unwrap();
    assert!(
        is_color(fill(&mut host), 0x4C8DF6, 2),
        "selected {:?}",
        fill(&mut host)
    );
    same_box(&mut host, "selected");
    host.mouse_move(80.0, 38.0, None, "").unwrap();
    let selected_hover = fill(&mut host);
    assert_ne!(selected_hover, [0x4C, 0x8D, 0xF6]);
}

#[test]
fn controls_a11_focus_indicator_follows_keyboard_modality() {
    require_backend!();
    let clicks = counts();
    let mut host = host(300.0, 200.0, column(clicks, |_, button| button));
    // The reserved focus edge: the 2 px border at the left of the first button.
    let edge = |host: &mut vespertine_test_host::TestHost| {
        host.flush();
        pixel(host, 21.0, 38.0)
    };
    let resting = edge(&mut host);
    assert!(
        is_color(resting, 0x30343D, 3),
        "transparent edge shows the fill {resting:?}"
    );

    // Tab: focus indicator.
    tab(&mut host);
    assert!(
        is_color(edge(&mut host), 0x7AA7FF, 3),
        "after Tab {:?}",
        edge(&mut host)
    );
    let focused_box = node(&mut host, "button", Some("Alpha")).bounds();

    // A mouse press on the same control: focused, but no indicator, and the box is unchanged.
    host.click(60.0, 38.0, 0, "").unwrap();
    host.mouse_move(250.0, 180.0, None, "").unwrap();
    host.paint_frame();
    let after_mouse = edge(&mut host);
    assert!(
        is_color(after_mouse, 0x30343D, 3),
        "after a mouse press {after_mouse:?}"
    );
    assert_eq!(
        node(&mut host, "button", Some("Alpha")).bounds(),
        focused_box
    );

    // Keyboard modality again (a key press) brings it back on the still-focused control.
    host.key_down("a", false, None).unwrap();
    assert!(is_color(edge(&mut host), 0x7AA7FF, 3), "after a key press");

    // Small pointer jitter after typing keeps the indicator: at most 8 logical px from the
    // position where typing started (INP-001), measured from that origin every time.
    host.mouse_move(250.0, 150.0, None, "").unwrap();
    host.key_down("a", false, None).unwrap();
    assert!(
        is_color(edge(&mut host), 0x7AA7FF, 3),
        "keyboard modality at the origin"
    );
    host.mouse_move(255.0, 155.0, None, "").unwrap();
    assert!(
        is_color(edge(&mut host), 0x7AA7FF, 3),
        "a 7 px move keeps it"
    );
    host.mouse_move(258.0, 150.0, None, "").unwrap();
    assert!(
        is_color(edge(&mut host), 0x7AA7FF, 3),
        "exactly 8 px keeps it"
    );
    host.mouse_move(259.0, 150.0, None, "").unwrap();
    host.paint_frame();
    assert!(
        is_color(edge(&mut host), 0x30343D, 3),
        "9 px leaves keyboard modality"
    );
}

/// A host with one button whose busy flag the test owns.
fn busy_host(busy: &Rc<Cell<bool>>) -> vespertine_test_host::TestHost {
    let busy = busy.clone();
    host(300.0, 120.0, move |_, _| {
        div()
            .p(px(20.))
            .child(
                Button::new("sync")
                    .label("Sync")
                    .w(px(120.))
                    .busy(busy.get()),
            )
            .into_any_element()
    })
}

fn leases(host: &mut vespertine_test_host::TestHost) -> usize {
    host.app().update(|cx| cx.animation_lease_count())
}

#[test]
fn controls_busy_indicator_runs_on_the_shared_clock_only_while_painted_and_not_reduced() {
    require_backend!();
    // Not busy: no animation interest at all.
    let busy = Rc::new(Cell::new(false));
    let mut host = busy_host(&busy);
    assert_eq!(leases(&mut host), 0);
    // Busy under normal motion: the painted indicator holds a lease on the shared clock.
    busy.set(true);
    host.paint_frame();
    assert!(
        leases(&mut host) >= 1,
        "a painted busy indicator renews a lease"
    );
    // The indicator moves with the shared clock: a quarter of a sweep later the frame differs.
    let before = host.screenshot_image().unwrap();
    host.advance_executor_time(300.0).unwrap();
    host.paint_frame();
    let after = host.screenshot_image().unwrap();
    assert_ne!(
        before.as_raw(),
        after.as_raw(),
        "the busy indicator animates"
    );

    // Forced reduction: static, and no lease is renewed.
    let still = Rc::new(Cell::new(false));
    let mut reduced = busy_host(&still);
    reduced.app().update(|cx| {
        cx.set_motion_preference(vespertine::MotionPreference::On);
    });
    still.set(true);
    reduced.paint_frame();
    assert_eq!(leases(&mut reduced), 0, "forced reduction renews no lease");
    let first = {
        reduced.paint_frame();
        reduced.screenshot_image().unwrap()
    };
    reduced.advance_executor_time(700.0).unwrap();
    reduced.paint_frame();
    let later = reduced.screenshot_image().unwrap();
    assert_eq!(
        first.as_raw(),
        later.as_raw(),
        "a reduced busy indicator is still"
    );
}
