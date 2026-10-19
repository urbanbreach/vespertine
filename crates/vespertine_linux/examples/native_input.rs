//! PLT-001 A11: cooperating windowless producers receive framework-forwarded pointer input.
//!
//! Two native surfaces (A below, B above, overlapping) register pointer handlers and opt into
//! framework forwarding; a popover is framework content above both. The example checks that:
//!
//! * framework capture runs first: events over the popover reach no producer, and a pointer that
//!   moves under the popover counts as leaving its recipient;
//! * positions are window-content logical pixels, and slot-local positions convert by adding the
//!   slot origin;
//! * crossing from A into B sends a leave to A (carrying the new position) before B's enter;
//! * a press keeps its recipient through the matching release even when the pointer moves over
//!   the other surface;
//! * a hidden surface and a destroyed one receive nothing any more.
//!
//! It exits 0 when every check passes; `NATIVE_INTERACTIVE=1` keeps the window open.

#![cfg(any(target_os = "linux", target_os = "freebsd"))]

mod native_support;

use std::{cell::RefCell, rc::Rc};

use native_support::{Seen, SharedSeen, launch, rect};
use vespertine::{
    Bounds, Context, IntoElement, MouseButton, NativePointerEvent, ParentElement, Pixels, Render,
    Styled, Window, canvas, deferred, div, point, prelude::*, px, rgb,
};
use vespertine_linux::SolidChild;

const A: (f32, f32, f32, f32) = (30.0, 40.0, 220.0, 150.0);
const B: (f32, f32, f32, f32) = (200.0, 90.0, 220.0, 150.0);

struct Input {
    a: Option<SolidChild>,
    b: SolidChild,
    seen_a: SharedSeen,
    seen_b: SharedSeen,
    configured: bool,
    b_visible: bool,
    popover: bool,
    popover_presses: usize,
}

fn kinds(seen: &SharedSeen) -> Vec<&'static str> {
    seen.borrow()
        .events
        .iter()
        .map(|event| match event {
            NativePointerEvent::Enter(_) => "enter",
            NativePointerEvent::Move(_) => "move",
            NativePointerEvent::Leave(_) => "leave",
            NativePointerEvent::Down(_) => "down",
            NativePointerEvent::Up(_) => "up",
        })
        .collect()
}

fn configure(child: &SolidChild, seen: &SharedSeen) {
    let Some(surface) = child.surface() else {
        return;
    };
    surface.set_input_forwarding(vespertine::InputForwarding::Framework);
    let seen = seen.clone();
    surface.set_pointer_handler(move |event, _, _| {
        seen.borrow_mut().events.push(event.clone());
        true
    });
}

/// A layout slot that positions `child` and reports it hidden when `visible` is false.
fn slot(child: &SolidChild, bounds: Bounds<Pixels>, visible: bool) -> impl IntoElement {
    let surface = child.surface().cloned();
    div()
        .absolute()
        .left(bounds.origin.x)
        .top(bounds.origin.y)
        .w(bounds.size.width)
        .h(bounds.size.height)
        .child(
            canvas(
                move |bounds, window, _| {
                    if let Some(surface) = &surface {
                        surface.position(window, bounds, visible);
                    }
                },
                |_, _, _, _| {},
            )
            .size_full(),
        )
}

impl Render for Input {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(a) = &mut self.a {
            a.prepare(window);
        }
        self.b.prepare(window);
        if !self.configured
            && let Some(a) = &self.a
            && a.surface().is_some()
            && self.b.surface().is_some()
        {
            self.configured = true;
            configure(a, &self.seen_a);
            configure(&self.b, &self.seen_b);
        }
        let (a_bounds, b_bounds) = (rect(A.0, A.1, A.2, A.3), rect(B.0, B.1, B.2, B.3));
        if let Some(a) = &mut self.a {
            a.present(window, a_bounds, 0x2060c0, 0x2060c0);
        }
        self.b.present(window, b_bounds, 0x20a060, 0x20a060);

        let mut root = div()
            .relative()
            .size_full()
            .bg(rgb(0xf3f4f6))
            .text_color(rgb(0x202020))
            .child("framework background");
        if let Some(a) = &self.a {
            root = root.child(slot(a, a_bounds, true));
        }
        root = root.child(slot(&self.b, b_bounds, self.b_visible));
        if self.popover {
            root = root.child(deferred(
                div()
                    .id("popover")
                    .absolute()
                    .left(px(20.0))
                    .top(px(150.0))
                    .w(px(120.0))
                    .h(px(80.0))
                    .bg(rgb(0xffffff))
                    .occlude()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.popover_presses += 1;
                            cx.notify();
                        }),
                    )
                    .child("Popover"),
            ));
        }
        root
    }
}

fn main() {
    let seen_a: SharedSeen = Rc::new(RefCell::new(Seen::default()));
    let seen_b: SharedSeen = Rc::new(RefCell::new(Seen::default()));
    launch(
        460.0,
        280.0,
        {
            let (seen_a, seen_b) = (seen_a.clone(), seen_b.clone());
            move |_, cx| {
                cx.new(|_| Input {
                    a: Some(SolidChild::new()),
                    b: SolidChild::new(),
                    seen_a,
                    seen_b,
                    configured: false,
                    b_visible: true,
                    popover: false,
                    popover_presses: 0,
                })
            }
        },
        move |mut script| async move {
            script.sleep(300).await;
            let ready = script.read(|view| {
                view.a.as_ref().and_then(|a| a.surface()).is_some() && view.b.surface().is_some()
            });
            script.check(ready, "both native surfaces are created");
            if !ready {
                return script.failures;
            }
            let reset = || {
                seen_a.borrow_mut().events.clear();
                seen_b.borrow_mut().events.clear();
            };

            // Slot-local to window-content conversion.
            let converted = script.read(|view| {
                view.a
                    .as_ref()
                    .and_then(|a| a.surface())
                    .map(|surface| surface.window_position(point(px(10.0), px(5.0))))
            });
            script.check(
                converted == Some(point(px(A.0 + 10.0), px(A.1 + 5.0))),
                "slot-local positions convert to window coordinates by adding the slot origin",
            );

            // Enter A, then cross into B: A gets a leave carrying the new position first.
            reset();
            let in_a = point(px(60.0), px(70.0));
            let in_both = point(px(230.0), px(120.0));
            script.mouse_move(in_a);
            script.mouse_move(in_both);
            script.sleep(50).await;
            script.check(
                kinds(&seen_a) == ["enter", "move", "leave"],
                "A sees enter, move, then a leave when the pointer crosses into B",
            );
            script.check(
                kinds(&seen_b) == ["enter", "move"],
                "B (above A) receives the pointer where they overlap",
            );
            let leave_position = seen_a.borrow().events.iter().find_map(|event| match event {
                NativePointerEvent::Leave(leave) => Some(leave.position),
                _ => None,
            });
            script.check(
                leave_position == Some(in_both),
                "the leave carries the position the pointer moved to, in window coordinates",
            );

            // A press keeps its recipient through the matching release.
            reset();
            script.mouse_move(in_a);
            script.press(in_a);
            script.mouse_move(in_both);
            script.release(in_both);
            script.sleep(50).await;
            let a_kinds = kinds(&seen_a);
            script.check(
                a_kinds.contains(&"down") && a_kinds.contains(&"up"),
                "A owns the press and gets the release although the pointer is over B",
            );
            script.check(
                !kinds(&seen_b).contains(&"down") && !kinds(&seen_b).contains(&"up"),
                "B receives neither the press nor the release it does not own",
            );

            // Framework capture runs first.
            script.set(|view, _| view.popover = true);
            script.sleep(300).await;
            reset();
            script.mouse_move(point(px(60.0), px(120.0)));
            script.sleep(50).await;
            let before = kinds(&seen_a);
            script.click(point(px(60.0), px(170.0)));
            script.sleep(50).await;
            script.check(
                script.read(|view| view.popover_presses) == 1,
                "the popover handles the press first",
            );
            script.check(
                kinds(&seen_a)
                    .iter()
                    .filter(|kind| **kind == "down")
                    .count()
                    == 0
                    && kinds(&seen_a).contains(&"leave")
                    && before.contains(&"enter"),
                "a pointer moving under the popover leaves A and A receives no press there",
            );
            script.set(|view, _| view.popover = false);
            script.sleep(300).await;

            // Hidden and destroyed surfaces receive nothing any more.
            script.set(|view, _| view.b_visible = false);
            script.sleep(300).await;
            reset();
            script.click(point(px(380.0), px(200.0)));
            script.sleep(50).await;
            script.check(
                kinds(&seen_b).is_empty(),
                "a hidden surface stops participating immediately",
            );
            script.set(|view, _| view.a = None);
            script.sleep(300).await;
            reset();
            script.click(point(px(60.0), px(70.0)));
            script.sleep(50).await;
            script.check(
                kinds(&seen_a).is_empty(),
                "a destroyed surface receives nothing",
            );
            script.failures
        },
    );
}
