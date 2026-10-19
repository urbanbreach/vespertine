//! CMP-001 slider: A6 (endpoints, stops, keyboard), A7 (presentation of stops), A8 (press, drag and
//! per-frame coalescing), A9 (reveal on interaction), accessibility and disabled behavior.

mod controls_support;

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use controls_support::*;
use vespertine::{
    InteractiveElement, IntoElement, ParentElement, PlatformInput, Styled, div, point, px,
};
use vespertine_components::controls::{Button, Slider, SliderRange};
use vespertine_test_host::{TestHost, require_backend};

/// The slider surface of the pixel range test: 240 x 28 logical px at (20, 20).
const LEFT: f64 = 20.0;
const TOP: f64 = 20.0;
const WIDTH: f64 = 240.0;
const INSET: f64 = 7.0;

fn pixels() -> SliderRange {
    SliderRange::new(560.0, 1200.0)
        .with_step(16.0)
        .with_default(736.0)
}

/// The owner of a slider: its value and every notification it received.
#[derive(Clone, Default)]
struct Owner {
    value: Rc<Cell<f64>>,
    log: Rc<RefCell<Vec<f64>>>,
}

impl Owner {
    fn new(value: f64) -> Self {
        Self {
            value: Rc::new(Cell::new(value)),
            log: Rc::default(),
        }
    }

    fn notifications(&self) -> Vec<f64> {
        self.log.borrow().clone()
    }

    fn notify(&self) -> impl Fn(f64, &mut vespertine::Window, &mut vespertine::App) + 'static {
        let owner = self.clone();
        move |value, window, _| {
            owner.log.borrow_mut().push(value);
            owner.value.set(value);
            window.refresh();
        }
    }
}

/// A slider over `range` owned by `owner`, followed by a button used as a layout marker.
fn slider_host(range: SliderRange, owner: &Owner, configure: fn(Slider) -> Slider) -> TestHost {
    let owner = owner.clone();
    host(320.0, 240.0, move |_, _| {
        let slider = Slider::new("slider", range.clone())
            .value(owner.value.get())
            .accessible_label("Width")
            .on_change(owner.notify());
        div()
            .p(px(20.))
            .flex()
            .flex_col()
            .items_start()
            .gap(px(8.))
            .child(configure(slider))
            .child(Button::new("marker").label("Marker").w(px(80.)))
            .into_any_element()
    })
}

fn at(x: f64) -> f64 {
    LEFT + x
}

#[test]
fn controls_a6_pointer_positions_map_to_endpoints_midpoint_and_steps() {
    require_backend!();
    let owner = Owner::new(736.0);
    let mut host = slider_host(pixels(), &owner, |slider| slider);
    let y = TOP + 14.0;
    // At or left of the first inset: the minimum.
    host.mouse_down(at(INSET), y, 0, "").unwrap();
    host.mouse_up(at(INSET), y, 0, "").unwrap();
    assert_eq!(owner.value.get(), 560.0);
    // At or right of the last inset: the maximum.
    host.mouse_down(at(WIDTH - INSET), y, 0, "").unwrap();
    host.mouse_up(at(WIDTH - INSET), y, 0, "").unwrap();
    assert_eq!(owner.value.get(), 1200.0);
    // The midpoint.
    host.mouse_down(at(WIDTH / 2.0), y, 0, "").unwrap();
    host.mouse_up(at(WIDTH / 2.0), y, 0, "").unwrap();
    assert_eq!(owner.value.get(), 880.0);
    // Every value delivered is a multiple of the step above the minimum.
    for x in (0..240).step_by(3) {
        host.mouse_down(at(f64::from(x)), y, 0, "").unwrap();
        host.mouse_up(at(f64::from(x)), y, 0, "").unwrap();
    }
    for value in owner.notifications() {
        assert!((560.0..=1200.0).contains(&value));
        assert_eq!((value - 560.0) % 16.0, 0.0, "{value}");
    }
    // A press that normalizes to the current value does not notify.
    let before = owner.notifications().len();
    let current = owner.value.get();
    let x = INSET + (current - 560.0) / 640.0 * (WIDTH - 2.0 * INSET);
    host.mouse_down(at(x), y, 0, "").unwrap();
    host.mouse_up(at(x), y, 0, "").unwrap();
    assert_eq!(owner.notifications().len(), before);
}

#[test]
fn controls_a6_keyboard_steps_home_end_and_clamps_without_notifying() {
    require_backend!();
    let owner = Owner::new(736.0);
    let mut host = slider_host(pixels(), &owner, |slider| slider);
    tab(&mut host);
    host.key_down("left", false, None).unwrap();
    assert_eq!(owner.value.get(), 720.0);
    host.key_down("right", false, None).unwrap();
    host.key_down("up", false, None).unwrap();
    assert_eq!(owner.value.get(), 752.0);
    host.key_down("down", false, None).unwrap();
    assert_eq!(owner.value.get(), 736.0);
    host.key_down("home", false, None).unwrap();
    assert_eq!(owner.value.get(), 560.0);
    host.key_down("left", false, None).unwrap();
    assert_eq!(owner.value.get(), 560.0, "no wraparound below the minimum");
    host.key_down("end", false, None).unwrap();
    assert_eq!(owner.value.get(), 1200.0);
    let before = owner.notifications().len();
    host.key_down("right", false, None).unwrap();
    assert_eq!(owner.value.get(), 1200.0);
    assert_eq!(
        owner.notifications().len(),
        before,
        "Right at the maximum stays and does not notify"
    );
    // Modified keys and other keys pass through untouched.
    host.key_down("shift-left", false, None).unwrap();
    host.key_down("a", false, None).unwrap();
    assert_eq!(owner.value.get(), 1200.0);
}

#[test]
fn controls_a6_handled_keys_stop_propagation_and_others_pass_through() {
    require_backend!();
    let owner = Owner::new(736.0);
    let seen = Rc::new(RefCell::new(Vec::<String>::new()));
    let outer = seen.clone();
    let range = pixels();
    let mut host = host(320.0, 120.0, {
        move |_, _| {
            let outer = outer.clone();
            div()
                .p(px(20.))
                .on_key_down(move |event, _, _| {
                    outer.borrow_mut().push(event.keystroke.key.clone())
                })
                .child(
                    Slider::new("slider", range.clone())
                        .value(owner.value.get())
                        .on_change(owner.notify()),
                )
                .into_any_element()
        }
    });
    tab(&mut host);
    seen.borrow_mut().clear();
    host.key_down("left", false, None).unwrap();
    host.key_down("home", false, None).unwrap();
    assert!(
        seen.borrow().is_empty(),
        "handled keys stop propagation: {:?}",
        seen.borrow()
    );
    host.key_down("a", false, None).unwrap();
    assert_eq!(*seen.borrow(), ["a"], "other keys pass through");
}

#[test]
fn controls_a6_continuous_slider_without_a_step_moves_by_a_hundredth_of_the_range() {
    require_backend!();
    let owner = Owner::new(50.0);
    let mut host = slider_host(SliderRange::new(0.0, 200.0), &owner, |slider| slider);
    tab(&mut host);
    host.key_down("right", false, None).unwrap();
    assert_eq!(owner.value.get(), 52.0);
    host.key_down("left", false, None).unwrap();
    host.key_down("left", false, None).unwrap();
    assert_eq!(owner.value.get(), 48.0);
}

#[test]
fn controls_a6_slider_exposes_role_label_value_range_step_and_value_text() {
    require_backend!();
    let owner = Owner::new(736.0);
    let mut host = slider_host(pixels(), &owner, |slider| {
        slider.value_text(|value| format!("{value} px"))
    });
    let slider = node(&mut host, "slider", Some("Width"));
    assert_eq!(slider.number("numericValue"), Some(736.0));
    assert_eq!(slider.number("minNumericValue"), Some(560.0));
    assert_eq!(slider.number("maxNumericValue"), Some(1200.0));
    assert_eq!(slider.number("numericValueStep"), Some(16.0));
    assert_eq!(slider.text("value"), Some("736 px"));
    assert!(!slider.disabled());
    tab(&mut host);
    host.key_down("end", false, None).unwrap();
    let slider = node(&mut host, "slider", Some("Width"));
    assert_eq!(slider.number("numericValue"), Some(1200.0));
    assert_eq!(slider.text("value"), Some("1200 px"));
}

#[test]
fn controls_a6_disabled_slider_reports_disabled_takes_no_focus_and_ignores_input() {
    require_backend!();
    let owner = Owner::new(736.0);
    let mut host = slider_host(pixels(), &owner, |slider| slider.disabled(true));
    let slider = node(&mut host, "slider", Some("Width"));
    assert!(slider.disabled());
    // Pointer and keyboard do nothing; Tab goes past it to the marker button.
    host.mouse_down(at(INSET), TOP + 14.0, 0, "").unwrap();
    host.mouse_up(at(INSET), TOP + 14.0, 0, "").unwrap();
    tab(&mut host);
    host.key_down("left", false, None).unwrap();
    host.key_down("home", false, None).unwrap();
    assert!(owner.notifications().is_empty());
    assert_eq!(owner.value.get(), 736.0);
}

#[test]
fn controls_a8_a_press_focuses_the_slider_and_applies_the_value_at_once() {
    require_backend!();
    let owner = Owner::new(736.0);
    let mut host = slider_host(pixels(), &owner, |slider| slider);
    let y = TOP + 14.0;
    // Another control holds focus first.
    host.click(30.0, 90.0, 0, "").unwrap();
    host.mouse_down(at(WIDTH / 2.0 + 60.0), y, 0, "").unwrap();
    // Applied immediately: before the button is released.
    assert_eq!(owner.notifications().len(), 1);
    // Focus moved to the slider: the keyboard now drives it.
    host.key_down("home", false, None).unwrap();
    assert_eq!(owner.value.get(), 560.0, "the slider has focus");
    host.mouse_up(at(WIDTH / 2.0 + 60.0), y, 0, "").unwrap();
    // Non-left buttons do nothing.
    let before = owner.notifications().len();
    host.mouse_down(at(INSET + 100.0), y, 2, "").unwrap();
    host.mouse_up(at(INSET + 100.0), y, 2, "").unwrap();
    assert_eq!(owner.notifications().len(), before);
}

#[test]
fn controls_a8_a_drag_leaving_the_surface_keeps_updating_until_release() {
    require_backend!();
    let owner = Owner::new(736.0);
    let mut host = slider_host(pixels(), &owner, |slider| slider);
    let y = TOP + 14.0;
    host.mouse_down(at(60.0), y, 0, "").unwrap();
    host.paint_frame();
    // Far outside the 28 px tall surface, and outside its horizontal extent.
    host.mouse_move(at(150.0), 200.0, Some(0), "").unwrap();
    host.paint_frame();
    host.mouse_move(at(WIDTH + 60.0), 215.0, Some(0), "")
        .unwrap();
    host.paint_frame();
    assert_eq!(
        owner.value.get(),
        1200.0,
        "the drag follows the pointer beyond the surface"
    );
    host.mouse_move(at(0.0) - 30.0, 230.0, Some(0), "").unwrap();
    host.paint_frame();
    assert_eq!(owner.value.get(), 560.0);
    host.mouse_up(at(100.0), 230.0, 0, "").unwrap();
    // Released: further moves do nothing.
    let before = owner.notifications().len();
    host.mouse_move(at(180.0), TOP + 14.0, None, "").unwrap();
    host.paint_frame();
    assert_eq!(owner.notifications().len(), before);
}

#[test]
fn controls_a8_two_pointer_moves_inside_one_frame_notify_the_owner_once() {
    require_backend!();
    let owner = Owner::new(736.0);
    let mut host = slider_host(pixels(), &owner, |slider| slider);
    let y = TOP + 14.0;
    host.mouse_down(at(INSET), y, 0, "").unwrap();
    host.paint_frame();
    let before = owner.notifications().len();
    let window = host.window();
    let moves = [at(80.0), at(160.0)];
    host.app()
        .update_window(window, |_, window, cx| {
            for x in moves {
                window.dispatch_event(
                    PlatformInput::MouseMove(vespertine::MouseMoveEvent {
                        position: point(px(x as f32), px(y as f32)),
                        pressed_button: Some(vespertine::MouseButton::Left),
                        modifiers: vespertine::Modifiers::none(),
                    }),
                    cx,
                );
            }
        })
        .unwrap();
    assert_eq!(
        owner.notifications().len(),
        before,
        "nothing before the frame"
    );
    next_frame(&mut host);
    let delivered = owner.notifications();
    assert_eq!(
        delivered.len(),
        before + 1,
        "one notification for two moves"
    );
    // It carries the newest pending value.
    let expected = pixels()
        .value_for_x(at(160.0) as f32, LEFT as f32, WIDTH as f32, INSET as f32)
        .unwrap();
    assert_eq!(*delivered.last().unwrap(), expected);
    host.mouse_up(at(160.0), y, 0, "").unwrap();
}

#[test]
fn controls_a7_stops_fill_up_to_the_animated_thumb_front() {
    require_backend!();
    let owner = Owner::new(0.0);
    let range = SliderRange::new(0.0, 100.0).with_stops([0.0, 25.0, 50.0, 75.0, 100.0]);
    let mut host = slider_host(range, &owner, |slider| slider);
    let travel = WIDTH - 2.0 * INSET;
    // The top of a stop mark, above the rail: filled accent or unfilled rail color.
    let mark = |host: &mut TestHost, index: usize| -> [u8; 3] {
        host.flush();
        pixel(host, at(INSET + travel * index as f64 / 4.0), TOP + 11.0)
    };
    for index in 1..5 {
        assert!(
            is_color(mark(&mut host, index), 0x3F4450, 6),
            "stop {index} unfilled at rest"
        );
    }
    let t0 = host.motion_pause();
    // Select the last stop: the value jumps, the thumb glides.
    host.mouse_down(at(WIDTH - INSET), TOP + 14.0, 0, "")
        .unwrap();
    host.mouse_up(at(WIDTH - INSET), TOP + 14.0, 0, "").unwrap();
    assert_eq!(owner.value.get(), 100.0);
    host.motion_set(t0 + 90.0).unwrap();
    host.flush();
    // At 90 ms the front is at 0.875: the stops at 0.25, 0.5 and 0.75 are filled, the last is not
    // (the fill follows the animated front rather than jumping ahead).
    for index in 1..4 {
        assert!(
            is_color(mark(&mut host, index), 0x4C8DF6, 6),
            "stop {index} filled at 90 ms"
        );
    }
    assert!(
        is_color(mark(&mut host, 4), 0x3F4450, 6),
        "the last stop is not reached at 90 ms"
    );
    // Retarget mid-flight: back to the first stop; it starts from the current front.
    host.mouse_down(at(INSET), TOP + 14.0, 0, "").unwrap();
    host.mouse_up(at(INSET), TOP + 14.0, 0, "").unwrap();
    assert_eq!(owner.value.get(), 0.0);
    host.flush();
    assert!(
        is_color(mark(&mut host, 3), 0x4C8DF6, 6),
        "no jump at the retarget instant"
    );
    host.motion_set(t0 + 90.0 + 180.0).unwrap();
    host.flush();
    for index in 1..5 {
        assert!(
            is_color(mark(&mut host, index), 0x3F4450, 6),
            "stop {index} after arriving"
        );
    }
}

#[test]
fn controls_a7_reduced_motion_snaps_the_discrete_thumb() {
    require_backend!();
    let owner = Owner::new(0.0);
    let range = SliderRange::new(0.0, 100.0).with_stops([0.0, 25.0, 50.0, 75.0, 100.0]);
    let mut host = slider_host(range, &owner, |slider| slider);
    host.motion_pause();
    host.app().update(|cx| cx.set_reduce_motion(true));
    host.mouse_down(at(WIDTH - INSET), TOP + 14.0, 0, "")
        .unwrap();
    host.mouse_up(at(WIDTH - INSET), TOP + 14.0, 0, "").unwrap();
    host.flush();
    let travel = WIDTH - 2.0 * INSET;
    // The thumb now covers the last stop; the others are filled at once.
    for index in 1..4 {
        assert!(
            is_color(
                pixel(
                    &mut host,
                    at(INSET + travel * index as f64 / 4.0),
                    TOP + 11.0
                ),
                0xFFFFFF,
                255
            ) && is_color(
                pixel(
                    &mut host,
                    at(INSET + travel * index as f64 / 4.0),
                    TOP + 11.0
                ),
                0x4C8DF6,
                6
            ),
            "stop {index} filled at once"
        );
    }
    assert!(
        is_color(pixel(&mut host, at(WIDTH - INSET), TOP + 14.0), 0xFFFFFF, 8),
        "the thumb is at the last stop"
    );
}

/// A slider with every piece of furniture, revealed on interaction, above a marker button.
fn furniture_host(owner: &Owner, resets: &Rc<RefCell<Vec<f64>>>) -> TestHost {
    let resets = resets.clone();
    let owner = owner.clone();
    let range = pixels();
    host(320.0, 240.0, move |_, _| {
        let resets = resets.clone();
        div()
            .p(px(20.))
            .flex()
            .flex_col()
            .items_start()
            .gap(px(8.))
            .child(
                Slider::new("slider", range.clone())
                    .value(owner.value.get())
                    .accessible_label("Width")
                    .label("Panel width")
                    .show_value(true)
                    .value_text(|value| format!("{value} px"))
                    .range_labels("560 px", "1200 px")
                    .on_reset(move |default, _, _| resets.borrow_mut().push(default))
                    .reveal_on_interaction(true)
                    .on_change(owner.notify()),
            )
            .child(Button::new("marker").label("Marker").w(px(80.)))
            .into_any_element()
    })
}

fn texts(host: &mut TestHost) -> Vec<String> {
    host.flush();
    host.painted_text()
}

fn furniture_visible(host: &mut TestHost, owner: &Owner) -> bool {
    let text = texts(host);
    let readout = format!("{} px", owner.value.get());
    let value = text.contains(&readout);
    let range =
        text.iter().any(|text| text == "560 px") && text.iter().any(|text| text == "1200 px");
    let reset = text.iter().any(|text| text == "Reset");
    assert_eq!(
        value, range,
        "furniture is shown or hidden as a whole: {text:?}"
    );
    assert_eq!(
        value, reset,
        "furniture is shown or hidden as a whole: {text:?}"
    );
    // The label is not furniture to reveal: it is always there.
    assert!(text.iter().any(|text| text == "Panel width"), "{text:?}");
    value
}

#[test]
fn controls_a9_furniture_is_revealed_by_interaction_and_keeps_its_layout_space() {
    require_backend!();
    let owner = Owner::new(736.0);
    let resets = Rc::new(RefCell::new(Vec::new()));
    let mut host = furniture_host(&owner, &resets);
    let rest_slider = node(&mut host, "slider", Some("Width")).bounds();
    let rest_marker = node(&mut host, "button", Some("Marker")).bounds();
    let still = |host: &mut TestHost, what: &str| {
        assert_eq!(
            node(host, "slider", Some("Width")).bounds(),
            rest_slider,
            "{what}: slider box"
        );
        assert_eq!(
            node(host, "button", Some("Marker")).bounds(),
            rest_marker,
            "{what}: layout below"
        );
    };

    // At rest: invisible, yet laid out (nothing below moved, see `still`).
    assert!(!furniture_visible(&mut host, &owner), "hidden at rest");

    // Hovered.
    let (x, y) = {
        let slider = node(&mut host, "slider", Some("Width"));
        center(&mut host, &slider)
    };
    host.mouse_move(x, y, None, "").unwrap();
    assert!(
        furniture_visible(&mut host, &owner),
        "visible while hovered"
    );
    still(&mut host, "hovered");
    host.mouse_exit(x, y).unwrap();
    host.mouse_move(300.0, 230.0, None, "").unwrap();
    assert!(
        !furniture_visible(&mut host, &owner),
        "hidden again when the pointer leaves"
    );
    still(&mut host, "left");

    // Dragged: visible while the press is held, even with the pointer elsewhere.
    host.mouse_down(x, y, 0, "").unwrap();
    host.mouse_move(300.0, 230.0, Some(0), "").unwrap();
    assert!(
        furniture_visible(&mut host, &owner),
        "visible during a drag"
    );
    still(&mut host, "dragging");
    host.mouse_up(300.0, 230.0, 0, "").unwrap();
    host.mouse_move(310.0, 235.0, None, "").unwrap();
    host.paint_frame();
    assert!(
        !furniture_visible(&mut host, &owner),
        "hidden after the drag ends away from the slider"
    );

    // Focused after keyboard use; Tab alone does not reveal it.
    host.blur().unwrap();
    host.focus_next().unwrap();
    assert!(
        !furniture_visible(&mut host, &owner),
        "focus by itself does not reveal"
    );
    host.key_down("right", false, None).unwrap();
    host.paint_frame();
    assert!(
        texts(&mut host).iter().any(|text| text == "Reset"),
        "visible after a key press"
    );
    still(&mut host, "keyboard");
    // Blur hides it again.
    host.blur().unwrap();
    host.paint_frame();
    assert!(!furniture_visible(&mut host, &owner), "hidden after blur");

    // Keyboard-active is cleared when the pointer hovers again.
    host.focus_next().unwrap();
    host.key_down("left", false, None).unwrap();
    host.paint_frame();
    host.mouse_move(x, y, None, "").unwrap();
    host.mouse_move(300.0, 230.0, None, "").unwrap();
    host.paint_frame();
    assert!(
        !furniture_visible(&mut host, &owner),
        "a hover after keyboard use clears keyboard-active"
    );
}

#[test]
fn controls_a9_the_reset_action_restores_the_default_value() {
    require_backend!();
    let owner = Owner::new(900.0);
    let resets = Rc::new(RefCell::new(Vec::new()));
    let mut host = furniture_host(&owner, &resets);
    let (x, y) = {
        let slider = node(&mut host, "slider", Some("Width"));
        center(&mut host, &slider)
    };
    host.mouse_move(x, y, None, "").unwrap();
    host.flush();
    let reset = node(&mut host, "button", Some("Reset"));
    let (rx, ry) = center(&mut host, &reset);
    host.click(rx, ry, 0, "").unwrap();
    assert_eq!(
        *resets.borrow(),
        [736.0],
        "the callback receives the declared default"
    );
}
