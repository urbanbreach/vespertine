//! PLT-001 A5: an independently rendered colored surface under a menu and a translucent dialog
//! with shadow, moved into the dialog and back.
//!
//! The blue child is a native surface of the window (a `wl_shm` subsurface on Wayland, a child
//! window on X11) presented by its own producer. A menu and a translucent dialog with a drop
//! shadow are deferred framework content, painted into the transparent overlay above it. The
//! example checks the stacking through the surface tree and the input path through the window:
//!
//! * the child stacks below the overlay; the menu and the dialog cover it, and a press on a
//!   dialog button is handled by the framework and never reaches the child;
//! * a press on the part of the child no overlay content covers reaches the child, with window
//!   coordinates;
//! * positioned while prepainting the dialog, the child is hosted above the overlay, inside the
//!   dialog, and receives the presses there (framework capture still runs first); positioned in base content again it returns below.
//!
//! Run it on Wayland, or on X11 with `WAYLAND_DISPLAY= cargo run --example native_layering`
//! (composited when a compositing manager runs). It exits 0 when every check passes;
//! `NATIVE_INTERACTIVE=1` keeps the window open. The pixel-level stacking is checked by the
//! fidelity scenes FID-PLT-001-1 and FID-PLT-001-2.

#![cfg(any(target_os = "linux", target_os = "freebsd"))]

mod native_support;

use std::{cell::RefCell, rc::Rc};

use native_support::{Seen, SharedSeen, downs, launch, rect};
use vespertine::{
    BoxShadow, Context, IntoElement, MouseButton, ParentElement, Render, Styled, Window, deferred,
    div, hsla, point, prelude::*, px, rgb,
};
use vespertine_linux::SolidChild;

const CHILD: (f32, f32, f32, f32) = (40.0, 60.0, 360.0, 220.0);
/// The child once it lives inside the dialog.
const IN_DIALOG: (f32, f32, f32, f32) = (330.0, 150.0, 180.0, 100.0);

struct Layering {
    child: SolidChild,
    seen: SharedSeen,
    configured: bool,
    inside_dialog: bool,
    menu_open: bool,
    dialog_open: bool,
    menu_presses: usize,
    dialog_presses: usize,
}

impl Layering {
    fn child_bounds(&self) -> vespertine::Bounds<vespertine::Pixels> {
        let (x, y, w, h) = if self.inside_dialog { IN_DIALOG } else { CHILD };
        rect(x, y, w, h)
    }
}

impl Render for Layering {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.child.prepare(window);
        if !self.configured
            && let Some(surface) = self.child.surface()
        {
            self.configured = true;
            // The framework forwards the presses no overlay content captured.
            surface.set_input_forwarding(vespertine::InputForwarding::Framework);
            let seen = self.seen.clone();
            surface.set_pointer_handler(move |event, _, _| {
                seen.borrow_mut().events.push(event.clone());
                true
            });
        }
        let bounds = self.child_bounds();
        self.child.present(window, bounds, 0x2060c0, 0x2060c0);

        let mut root = div()
            .relative()
            .size_full()
            .bg(rgb(0x101820))
            .text_color(rgb(0xffffff))
            .child(
                div()
                    .absolute()
                    .left(px(40.0))
                    .top(px(20.0))
                    .child("Below the native child"),
            );
        if !self.inside_dialog {
            root = root.child(self.child.slot(bounds));
        }
        if self.menu_open {
            root = root.child(deferred(
                div()
                    .id("menu")
                    .absolute()
                    .left(px(60.0))
                    .top(px(200.0))
                    .w(px(130.0))
                    .h(px(70.0))
                    .rounded(px(8.0))
                    .bg(rgb(0xf0f0f0))
                    .text_color(rgb(0x101820))
                    .occlude()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.menu_presses += 1;
                            cx.notify();
                        }),
                    )
                    .child("Menu above child"),
            ));
        }
        if self.dialog_open {
            let mut dialog = div().absolute().size_full().child(
                div()
                    .id("dialog")
                    .absolute()
                    .left(px(280.0))
                    .top(px(100.0))
                    .w(px(280.0))
                    .h(px(190.0))
                    .rounded(px(12.0))
                    .bg(hsla(0.0, 0.0, 1.0, 0.85))
                    .text_color(rgb(0x202020))
                    .shadow(vec![BoxShadow {
                        color: hsla(0.0, 0.0, 0.0, 0.35),
                        offset: point(px(0.0), px(8.0)),
                        blur_radius: px(18.0),
                        spread_radius: px(0.0),
                        inset: false,
                    }])
                    .occlude()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.dialog_presses += 1;
                            cx.notify();
                        }),
                    )
                    .child("Translucent dialog"),
            );
            if self.inside_dialog {
                // Positioned while prepainting the deferred dialog: hosted above the overlay.
                dialog = dialog.child(self.child.slot(bounds));
            }
            root = root.child(deferred(dialog).with_priority(2));
        }
        root
    }
}

fn main() {
    let seen: SharedSeen = Rc::new(RefCell::new(Seen::default()));
    launch(
        560.0,
        340.0,
        {
            let seen = seen.clone();
            move |_, cx| {
                cx.new(|_| Layering {
                    child: SolidChild::new(),
                    seen,
                    configured: false,
                    inside_dialog: false,
                    menu_open: true,
                    dialog_open: true,
                    menu_presses: 0,
                    dialog_presses: 0,
                })
            }
        },
        move |mut script| async move {
            script.sleep(300).await;
            let child_surface = script.read(|view| view.child.surface().map(|s| s.id()));
            let unavailable = script.read(|view| view.child.unavailable().map(str::to_owned));
            script.check(
                unavailable.is_none() && child_surface.is_some(),
                "the native child is created",
            );
            let Some(child) = child_surface else {
                return script.failures;
            };
            let overlay = script.layout().overlay;

            script.check(
                script.position_of(child) < script.position_of(overlay),
                "the child stacks below the overlay",
            );

            // The menu and the dialog cover the child: their presses never reach it.
            script.click(point(px(70.0), px(210.0)));
            script.click(point(px(300.0), px(120.0)));
            script.sleep(100).await;
            let (menu, dialog) = script.read(|view| (view.menu_presses, view.dialog_presses));
            script.check(menu == 1, "the menu handles the press over the child");
            script.check(dialog == 1, "the dialog handles the press over the child");
            script.check(
                downs(&seen).is_empty(),
                "the child receives nothing where overlay content covers it",
            );

            // Where no overlay content covers the child, the press reaches it.
            let free = point(px(60.0), px(100.0));
            script.click(free);
            script.sleep(100).await;
            script.check(
                downs(&seen) == [free],
                "the child receives the press on its uncovered area, in window coordinates",
            );

            // Move the child into the dialog: it is hosted above the overlay.
            script.set(|view, _| view.inside_dialog = true);
            script.sleep(400).await;
            script.check(
                script.position_of(child) > script.position_of(overlay),
                "positioned inside the dialog the child is hosted above the overlay",
            );
            seen.borrow_mut().events.clear();
            let inside = point(px(400.0), px(200.0));
            script.click(inside);
            script.sleep(100).await;
            script.check(
                downs(&seen) == [inside],
                "the child receives the press inside the dialog",
            );

            // And back: below the overlay again.
            script.set(|view, _| view.inside_dialog = false);
            script.sleep(400).await;
            script.check(
                script.position_of(child) < script.position_of(overlay),
                "positioned in base content again the child returns below the overlay",
            );
            let stats = script.stats();
            script.check(
                stats.failed_writes == 0,
                "no native surface write failed during the run",
            );
            script.failures
        },
    );
}
