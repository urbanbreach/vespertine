//! PLT-001 A9: resizing a window with a native child and an overlay dialog anchored to its
//! bottom-right corner.
//!
//! The window is resized through a sequence of sizes. Every frame's presentation callbacks apply
//! the native child's geometry together with the framework buffer of the frame; the example
//! records, inside the presentation callback of each frame, the window size of that frame and the
//! origin the child was given, and checks that they always agree (no frame in which child and
//! window geometry disagree), that the child was rewritten exactly when its bounds changed and
//! that the surface order was never synchronized again. On Wayland the framework planes of the
//! frame are synchronized subsurfaces committed with the window surface; on X11 the child
//! geometry is flushed in the same batch right before the window's own presentation.
//!
//! Run it on outputs of scale 1 and 2 (or on X11 with `WAYLAND_DISPLAY=`). It exits 0 when every
//! check passes; `NATIVE_INTERACTIVE=1` keeps the window open for dragging its edge.

#![cfg(any(target_os = "linux", target_os = "freebsd"))]

mod native_support;

use std::{cell::RefCell, rc::Rc};

use native_support::{launch, rect};
use vespertine::{
    Context, IntoElement, ParentElement, Pixels, Point, Render, Size, Styled, Window, canvas,
    deferred, div, point, prelude::*, px, rgb, size,
};
use vespertine_linux::SolidChild;

const CHILD: (f32, f32) = (240.0, 140.0);
const MARGIN: f32 = 24.0;

/// What one presented frame looked like inside its presentation callback.
#[derive(Clone, Copy, Debug)]
struct Presented {
    window: Size<Pixels>,
    child_origin: Point<Pixels>,
}

struct Resizing {
    child: SolidChild,
    presented: Rc<RefCell<Vec<Presented>>>,
}

impl Render for Resizing {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.child.prepare(window);
        let viewport = window.viewport_size();
        let bounds = rect(
            f32::from(viewport.width) - CHILD.0 - MARGIN,
            f32::from(viewport.height) - CHILD.1 - MARGIN,
            CHILD.0,
            CHILD.1,
        );
        self.child.present(window, bounds, 0x2060c0, 0x68b0e8);
        let surface = self.child.surface().cloned();
        let presented = self.presented.clone();
        div()
            .relative()
            .size_full()
            .bg(rgb(0xf3f4f6))
            .text_color(rgb(0x202020))
            .child("resize the window")
            .child(self.child.slot(bounds))
            // Registered after the child's own position callback: it runs after it in the same
            // presentation batch.
            .child(
                canvas(
                    |_, _, _| (),
                    move |_, _, window, _| {
                        let surface = surface.clone();
                        window.on_present(move |window, _| {
                            if let Some(surface) = &surface {
                                presented.borrow_mut().push(Presented {
                                    window: window.viewport_size(),
                                    child_origin: surface.window_position(point(px(0.0), px(0.0))),
                                });
                            }
                        });
                    },
                )
                .size(px(1.0)),
            )
            .child(deferred(
                div()
                    .absolute()
                    .left(bounds.origin.x - px(12.0))
                    .top(bounds.origin.y - px(12.0))
                    .w(px(CHILD.0 + 24.0))
                    .h(px(36.0))
                    .rounded(px(8.0))
                    .bg(rgb(0xffffff))
                    .child("overlay anchored to the child"),
            ))
    }
}

fn main() {
    let presented: Rc<RefCell<Vec<Presented>>> = Rc::default();
    launch(
        520.0,
        360.0,
        {
            let presented = presented.clone();
            move |_, cx| {
                cx.new(|_| Resizing {
                    child: SolidChild::new(),
                    presented,
                })
            }
        },
        move |mut script| async move {
            script.sleep(300).await;
            let scale = script
                .cx
                .update_window(script.handle.into(), |_, window, _| window.scale_factor())
                .expect("window");
            println!("scale factor {scale}");
            let child = script.read(|view| view.child.surface().map(|s| s.id()));
            script.check(child.is_some(), "the native child is created");
            let Some(child) = child else {
                return script.failures;
            };
            let overlay = script.layout().overlay;
            script.check(
                script.position_of(child) < script.position_of(overlay),
                "the child stacks below the overlay",
            );
            let before = script.stats();
            presented.borrow_mut().clear();

            // Resize through a series of sizes, one frame each.
            let mut sizes = Vec::new();
            for step in 0..24 {
                let width = 520.0 + 12.0 * step as f32;
                let height = 360.0 + 7.0 * step as f32;
                sizes.push(size(px(width), px(height)));
            }
            for size in sizes {
                script
                    .cx
                    .clone()
                    .update_window(script.handle.into(), |_, window, _| window.resize(size))
                    .expect("window");
                script.sleep(60).await;
            }
            script.sleep(300).await;

            let frames = presented.borrow().clone();
            script.check(frames.len() >= 10, "the resize produced presented frames");
            let mut distinct: Vec<(i32, i32)> = frames
                .iter()
                .map(|frame| {
                    (
                        f32::from(frame.window.width) as i32,
                        f32::from(frame.window.height) as i32,
                    )
                })
                .collect();
            distinct.sort_unstable();
            distinct.dedup();
            script.check(
                distinct.len() >= 5,
                "the window went through several sizes while resizing",
            );
            let mismatched = frames
                .iter()
                .filter(|frame| {
                    let expected = point(
                        frame.window.width - px(CHILD.0 + MARGIN),
                        frame.window.height - px(CHILD.1 + MARGIN),
                    );
                    (f32::from(frame.child_origin.x) - f32::from(expected.x)).abs() > 0.5
                        || (f32::from(frame.child_origin.y) - f32::from(expected.y)).abs() > 0.5
                })
                .count();
            script.check(
                mismatched == 0,
                "in every presented frame the child's geometry matches the frame's window size",
            );
            let after = script.stats();
            script.check(
                after.order_syncs == before.order_syncs,
                "resizing never synchronizes the surface order",
            );
            script.check(
                after.bounds_writes - before.bounds_writes <= frames.len() + 1
                    && after.bounds_writes > before.bounds_writes,
                "the child is rewritten at most once per presented frame",
            );
            script.check(after.failed_writes == 0, "no native write failed");
            script.failures
        },
    );
}
