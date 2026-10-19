//! CMP-001 switch: R12-R14 and A5 (role and checked state, retargetable thumb, reduced motion,
//! frames only while moving), plus the keyboard-click activation and disabled behavior.

mod controls_support;

use std::{cell::Cell, rc::Rc};

use controls_support::*;
use vespertine::{IntoElement, ParentElement, Styled, div, px};
use vespertine_components::controls::Switch;
use vespertine_test_host::{TestHost, require_backend};

/// A host with one switch labelled "Wifi" whose state the test owns.
fn switch_host(checked: &Rc<Cell<bool>>, disabled: bool) -> TestHost {
    let checked = checked.clone();
    host(300.0, 120.0, move |_, _| {
        let state = checked.clone();
        div()
            .p(px(20.))
            .child(
                Switch::new("wifi")
                    .checked(checked.get())
                    .label("Wifi")
                    .disabled(disabled)
                    .on_change(move |next, window, _| {
                        state.set(next);
                        window.refresh();
                    }),
            )
            .into_any_element()
    })
}

/// Left edge of the thumb inside the track as a position between 0 and 1, read from the painted
/// frame: the track occupies x 22..58 at y 32 and the thumb is the white run on it.
fn thumb_position(host: &mut TestHost) -> f64 {
    host.flush();
    let scale = f64::from(host.scale_factor());
    let image = host.screenshot_image().unwrap();
    let y = (32.0 * scale) as u32;
    let mut left = None;
    for step in 0..(36.0 * scale) as u32 {
        let x = (22.0 * scale) as u32 + step;
        let pixel = image.get_pixel(x, y);
        if pixel[0] > 0xE0 && pixel[1] > 0xE0 && pixel[2] > 0xE0 {
            left = Some(f64::from(x) / scale);
            break;
        }
    }
    let left = left.expect("the thumb is painted");
    // Thumb left = 22 (track) + 2 (inset) + 16 * position.
    (left - 24.0) / 16.0
}

fn near(actual: f64, expected: f64) -> bool {
    (actual - expected).abs() < 0.07
}

/// Number of frame callbacks the window still has pending: the frames the switch asked for.
fn pending_frames(host: &mut TestHost) -> usize {
    let window = host.window();
    host.app()
        .update_window(window, |_, window, cx| window.simulate_next_frame(cx))
        .unwrap()
}

#[test]
fn controls_a5_switch_exposes_role_switch_and_a_checked_state_that_follows_the_owner() {
    require_backend!();
    let checked = Rc::new(Cell::new(false));
    let mut host = switch_host(&checked, false);
    let off = node(&mut host, "switch", Some("Wifi"));
    assert_eq!(off.text("toggled"), Some("false"));
    assert!(!off.disabled());
    assert!(off.can_click());

    // The owner owns the state: activating asks for the opposite but changes nothing by itself.
    let asked = Rc::new(Cell::new(None));
    let asked_in = asked.clone();
    let mut inert = host_with_inert_owner(asked_in);
    let wifi = node(&mut inert, "switch", Some("Wifi"));
    let (x, y) = center(&mut inert, &wifi);
    inert.click(x, y, 0, "").unwrap();
    assert_eq!(
        asked.get(),
        Some(true),
        "the opposite of unchecked is requested"
    );
    assert_eq!(
        node(&mut inert, "switch", Some("Wifi")).text("toggled"),
        Some("false"),
        "the switch did not change without the owner"
    );

    // A controlled switch follows the owner.
    let (x, y) = center(&mut host, &off);
    host.click(x, y, 0, "").unwrap();
    assert!(checked.get());
    assert_eq!(
        node(&mut host, "switch", Some("Wifi")).text("toggled"),
        Some("true")
    );
    checked.set(false);
    host.paint_frame();
    assert_eq!(
        node(&mut host, "switch", Some("Wifi")).text("toggled"),
        Some("false")
    );
}

fn host_with_inert_owner(asked: Rc<Cell<Option<bool>>>) -> TestHost {
    host(300.0, 120.0, move |_, _| {
        let asked = asked.clone();
        div()
            .p(px(20.))
            .child(
                Switch::new("wifi")
                    .checked(false)
                    .label("Wifi")
                    .on_change(move |next, _, _| asked.set(Some(next))),
            )
            .into_any_element()
    })
}

#[test]
fn controls_a5_switch_label_is_part_of_the_activation_target_and_keys_activate_once() {
    require_backend!();
    let checked = Rc::new(Cell::new(false));
    let mut host = switch_host(&checked, false);
    // The label sits after the track and is clickable.
    host.click(100.0, 32.0, 0, "").unwrap();
    assert!(checked.get(), "clicking the label toggles");
    // Tab, then the base keyboard-click: once on release for Enter and for Space.
    tab(&mut host);
    host.key_down("enter", false, None).unwrap();
    assert!(checked.get(), "nothing before the release");
    host.key_up("enter", None).unwrap();
    assert!(!checked.get(), "Enter toggles once on release");
    host.key_down("space", false, None).unwrap();
    host.key_up("space", None).unwrap();
    assert!(checked.get(), "Space toggles once on release");
}

#[test]
fn controls_a5_disabled_switch_keeps_semantics_and_never_activates() {
    require_backend!();
    let checked = Rc::new(Cell::new(true));
    let mut host = switch_host(&checked, true);
    let wifi = node(&mut host, "switch", Some("Wifi"));
    assert!(wifi.disabled());
    assert_eq!(wifi.text("toggled"), Some("true"));
    assert!(!wifi.can_click());
    host.click(40.0, 32.0, 0, "").unwrap();
    tab(&mut host);
    host.key_down("enter", false, None).unwrap();
    host.key_up("enter", None).unwrap();
    a11y_click(&mut host, &wifi);
    assert!(checked.get(), "a disabled switch does not change");
}

#[test]
fn controls_a5_thumb_travels_in_180_ms_and_a_mid_travel_retarget_starts_from_the_current_position()
{
    require_backend!();
    let checked = Rc::new(Cell::new(false));
    let mut host = switch_host(&checked, false);
    // First appearance: at the target, with no frame requested.
    assert!(near(thumb_position(&mut host), 0.0));
    assert_eq!(pending_frames(&mut host), 0);

    let t0 = host.motion_pause();
    host.click(40.0, 32.0, 0, "").unwrap();
    assert!(checked.get());
    // Colors change at once; only the thumb interpolates.
    assert!(
        is_color(pixel(&mut host, 57.0, 32.0), 0x4C8DF6, 3),
        "the on-track color is immediate"
    );
    assert!(
        near(thumb_position(&mut host), 0.0),
        "the thumb has not moved yet"
    );
    assert!(
        pending_frames(&mut host) > 0,
        "frames are requested while traveling"
    );

    // Halfway through 180 ms: 1 - (1 - 0.5)^3 = 0.875.
    host.motion_set(t0 + 90.0).unwrap();
    let mid = thumb_position(&mut host);
    assert!(near(mid, 0.875), "at 90 ms: {mid}");

    // Retarget mid-travel: it starts from the current position, without a jump.
    host.click(40.0, 32.0, 0, "").unwrap();
    assert!(!checked.get());
    let right_after = thumb_position(&mut host);
    assert!(near(right_after, mid), "no jump: {right_after} vs {mid}");
    host.motion_set(t0 + 90.0 + 90.0).unwrap();
    // From 0.875 to 0 with t = 0.5: 0.875 * (1 - 0.875) = 0.109.
    let back = thumb_position(&mut host);
    assert!(near(back, 0.109), "second leg at its half: {back}");

    // Arrival: within 0.001 of the target, no further frames are requested.
    host.motion_set(t0 + 90.0 + 180.0).unwrap();
    assert!(near(thumb_position(&mut host), 0.0));
    // Frames requested while traveling are drained; a render of the settled switch asks for none.
    pending_frames(&mut host);
    host.paint_frame();
    assert_eq!(
        pending_frames(&mut host),
        0,
        "no frames once the position settled"
    );
}

#[test]
fn controls_a5_reduced_motion_snaps_the_thumb_to_the_target_on_the_next_frame() {
    require_backend!();
    let checked = Rc::new(Cell::new(false));
    let mut host = switch_host(&checked, false);
    host.motion_pause();
    host.app().update(|cx| cx.set_reduce_motion(true));
    host.click(40.0, 32.0, 0, "").unwrap();
    assert!(checked.get());
    assert!(
        near(thumb_position(&mut host), 1.0),
        "at the target on the next frame"
    );
    assert_eq!(pending_frames(&mut host), 0);
    host.click(40.0, 32.0, 0, "").unwrap();
    assert!(near(thumb_position(&mut host), 0.0));
}
