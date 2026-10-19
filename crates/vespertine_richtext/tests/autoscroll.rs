//! TXT-008 A5: edge autoscroll of a registered list during a live drag.

mod support;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;
use support::{Build, Harness, at};
use vespertine::{
    Bounds, IntoElement, MouseButton, ParentElement, Pixels, Styled, canvas, div, point, px, rgb,
    size,
};
use vespertine_richtext::autoscroll::{RegisteredList, ScrollTarget, TICK};
use vespertine_richtext::{RichText, TextService, text_root};

struct Scroller {
    deltas: RefCell<Vec<f32>>,
    position: Cell<f32>,
}

impl ScrollTarget for Scroller {
    fn scroll_by(&self, delta: Pixels) -> bool {
        self.deltas.borrow_mut().push(f32::from(delta));
        self.position.set(self.position.get() + f32::from(delta));
        true
    }
}

fn harness(height: f32, scroller: &Rc<Scroller>) -> Harness {
    let scroller = scroller.clone();
    let build: Build = Rc::new(move |_, _| {
        let scroller = scroller.clone();
        text_root(
            div()
                .size_full()
                .bg(rgb(0x181818))
                .font_family("Noto Sans")
                .text_size(px(16.))
                .line_height(px(24.))
                .child(RichText::new("a", "first row of text"))
                .child(RichText::new("b", "second row of text"))
                .child(canvas(
                    |_, _, _| (),
                    move |_, (), _, _| {
                        TextService::current()
                            .unwrap()
                            .register_list(RegisteredList {
                                id: 1,
                                viewport: Bounds {
                                    origin: point(px(0.), px(0.)),
                                    size: size(px(300.), px(height)),
                                },
                                target: scroller,
                            });
                    },
                )),
        )
        .into_any_element()
    });
    Harness::new(300., height, build)
}

#[test]
fn ticks_scroll_by_the_quadratic_step_every_24ms_and_stop_with_the_drag() {
    let scroller = Rc::new(Scroller {
        deltas: RefCell::default(),
        position: Cell::new(0.),
    });
    let mut harness = harness(300., &scroller);
    assert_eq!(TICK, Duration::from_millis(24));
    harness.press(at(4., 12.), MouseButton::Left, 1);
    // 18 px into the bottom band of a 300 px viewport (edge 36): step 24 x 0.25 = 6.
    harness.move_to(at(40., 282.), Some(MouseButton::Left));
    assert!(
        scroller.deltas.borrow().is_empty(),
        "no scroll before the first tick"
    );
    harness.cx.advance_clock(TICK);
    harness.draw();
    assert_eq!(scroller.deltas.borrow().as_slice(), &[6.0]);
    harness.cx.advance_clock(TICK);
    harness.draw();
    assert_eq!(scroller.deltas.borrow().as_slice(), &[6.0, 6.0]);
    // A pointer deeper in the band speeds up: 36 px or more is the maximum step.
    harness.move_to(at(40., 299.9), Some(MouseButton::Left));
    harness.cx.advance_clock(TICK);
    harness.draw();
    assert!(scroller.deltas.borrow().last().copied().unwrap() > 22.0);
    // Releasing ends the loop.
    harness.release(at(40., 299.), MouseButton::Left, 1);
    let calls = scroller.deltas.borrow().len();
    harness.cx.advance_clock(TICK * 3);
    harness.draw();
    assert_eq!(
        scroller.deltas.borrow().len(),
        calls,
        "scrolling stops when the drag ends"
    );
}

#[test]
fn a_pointer_outside_the_bands_schedules_no_tick() {
    let scroller = Rc::new(Scroller {
        deltas: RefCell::default(),
        position: Cell::new(0.),
    });
    let mut harness = harness(300., &scroller);
    harness.press(at(4., 12.), MouseButton::Left, 1);
    harness.move_to(at(40., 150.), Some(MouseButton::Left));
    harness.cx.advance_clock(TICK * 4);
    harness.draw();
    assert!(scroller.deltas.borrow().is_empty());
    // A 120 px viewport has a 20 px band: 10 px into it is progress 0.5, step 6.
    let scroller = Rc::new(Scroller {
        deltas: RefCell::default(),
        position: Cell::new(0.),
    });
    let mut small = harness_small(&scroller);
    small.press(at(4., 12.), MouseButton::Left, 1);
    small.move_to(at(40., 110.), Some(MouseButton::Left));
    small.cx.advance_clock(TICK);
    small.draw();
    assert_eq!(scroller.deltas.borrow().as_slice(), &[6.0]);
}

fn harness_small(scroller: &Rc<Scroller>) -> Harness {
    harness(120., scroller)
}
