//! PLT-001 A6: a native-colored child between a lower label and an upper popover.
//!
//! The label is base content under the child, the child is an independently presented native
//! surface, and the popover is deferred framework content above it. The example checks through
//! the window's input path that:
//!
//! * the popover covers the child: a press where they overlap is handled by the popover and
//!   never reaches the child, which does receive a press on its uncovered area;
//! * a tooltip-only overlay does not capture the child's input: with the tooltip shown over the
//!   child, a press on the tooltip's own area still reaches the child.
//!
//! It exits 0 when every check passes; `NATIVE_INTERACTIVE=1` keeps the window open.

#![cfg(any(target_os = "linux", target_os = "freebsd"))]

mod native_support;

use std::{cell::Cell, cell::RefCell, rc::Rc};

use native_support::{Seen, SharedSeen, downs, launch, rect};
use vespertine::{
    AnyView, Bounds, Context, IntoElement, MouseButton, ParentElement, Pixels, Render, Styled,
    Window, canvas, deferred, div, point, prelude::*, px, rgb,
};
use vespertine_linux::SolidChild;

const CHILD: (f32, f32, f32, f32) = (120.0, 70.0, 300.0, 180.0);

struct Tip {
    shown: Rc<Cell<bool>>,
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.shown.set(true);
        let bounds = self.bounds.clone();
        div()
            .w(px(100.0))
            .h(px(30.0))
            .bg(rgb(0x202020))
            .text_color(rgb(0xffffff))
            .child("tooltip")
            .child(
                canvas(
                    move |bounds, _, _| bounds,
                    move |_, tip_bounds, _, _| bounds.set(Some(tip_bounds)),
                )
                .absolute()
                .size_full(),
            )
    }
}

struct Popover {
    child: SolidChild,
    seen: SharedSeen,
    configured: bool,
    popover_open: bool,
    popover_presses: usize,
    tip_shown: Rc<Cell<bool>>,
    tip_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}

impl Render for Popover {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.child.prepare(window);
        if !self.configured
            && let Some(surface) = self.child.surface()
        {
            self.configured = true;
            surface.set_input_forwarding(vespertine::InputForwarding::Framework);
            let seen = self.seen.clone();
            surface.set_pointer_handler(move |event, _, _| {
                seen.borrow_mut().events.push(event.clone());
                true
            });
        }
        let (x, y, w, h) = CHILD;
        let bounds = rect(x, y, w, h);
        self.child.present(window, bounds, 0xe07020, 0xe07020);
        let (tip_shown, tip_bounds) = (self.tip_shown.clone(), self.tip_bounds.clone());

        let mut root = div()
            .relative()
            .size_full()
            .bg(rgb(0xf3f4f6))
            .text_color(rgb(0x202020))
            // The lower label: base content under the child.
            .child(
                div()
                    .absolute()
                    .left(px(20.0))
                    .top(px(100.0))
                    .child("Lower label, covered by the child"),
            )
            .child(self.child.slot(bounds))
            // The tooltip target is base content under the child.
            .child(
                div()
                    .id("target")
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .w(px(w))
                    .h(px(h))
                    .tooltip(move |_, cx| {
                        let (shown, bounds) = (tip_shown.clone(), tip_bounds.clone());
                        AnyView::from(cx.new(|_| Tip { shown, bounds }))
                    }),
            );
        if self.popover_open {
            root = root.child(deferred(
                div()
                    .id("popover")
                    .absolute()
                    .left(px(300.0))
                    .top(px(150.0))
                    .w(px(200.0))
                    .h(px(120.0))
                    .rounded(px(8.0))
                    .bg(rgb(0xffffff))
                    .border_1()
                    .border_color(rgb(0xb8bec8))
                    .occlude()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.popover_presses += 1;
                            cx.notify();
                        }),
                    )
                    .child("Popover above the child"),
            ));
        }
        root
    }
}

fn main() {
    let seen: SharedSeen = Rc::new(RefCell::new(Seen::default()));
    launch(
        540.0,
        320.0,
        {
            let seen = seen.clone();
            move |_, cx| {
                cx.new(|_| Popover {
                    child: SolidChild::new(),
                    seen,
                    configured: false,
                    popover_open: true,
                    popover_presses: 0,
                    tip_shown: Rc::new(Cell::new(false)),
                    tip_bounds: Rc::new(Cell::new(None)),
                })
            }
        },
        move |mut script| async move {
            script.sleep(300).await;
            let child = script.read(|view| view.child.surface().map(|s| s.id()));
            script.check(child.is_some(), "the native child is created");
            let Some(child) = child else {
                return script.failures;
            };
            let overlay = script.layout().overlay;
            script.check(
                script.position_of(child) < script.position_of(overlay),
                "the child stacks between the base label and the overlay popover",
            );

            // Where the popover overlaps the child, the popover takes the press.
            script.click(point(px(350.0), px(200.0)));
            script.sleep(100).await;
            script.check(
                script.read(|view| view.popover_presses) == 1,
                "the popover handles a press over the child",
            );
            script.check(
                downs(&seen).is_empty(),
                "the child receives nothing where the popover covers it",
            );
            // Where the popover does not reach, the child does.
            let free = point(px(150.0), px(100.0));
            script.click(free);
            script.sleep(100).await;
            script.check(
                downs(&seen) == [free],
                "the child receives a press the popover does not cover",
            );

            // A tooltip-only overlay does not capture the child's input.
            script.set(|view, _| view.popover_open = false);
            script.sleep(300).await;
            seen.borrow_mut().events.clear();
            let hover = point(px(160.0), px(110.0));
            script.mouse_move(hover);
            let shown = script.until(|view| view.tip_shown.get()).await;
            script.check(shown, "the tooltip shows over the child");
            script.sleep(200).await;
            let tip = script.read(|view| view.tip_bounds.get());
            script.check(tip.is_some(), "the tooltip painted its bounds");
            if let Some(tip) = tip {
                let inside = point(
                    tip.origin.x + tip.size.width / 2.0,
                    tip.origin.y + tip.size.height / 2.0,
                );
                script.click(inside);
                script.sleep(100).await;
                script.check(
                    downs(&seen).contains(&inside),
                    "a press on the tooltip's area still reaches the child",
                );
            }
            script.failures
        },
    );
}
