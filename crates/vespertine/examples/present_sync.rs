//! PLT-002: presentation-synchronized native updates.
//!
//! A native X11 child window (solid blue, produced by a separate X connection)
//! follows a marker element in the framework's layout. The child is moved,
//! resized and shown only from `Window::on_present` callbacks, so its geometry
//! changes in the same step as the framework pixels it is aligned with, including
//! while the window is resized.
//!
//! Run it on X11 (`WAYLAND_DISPLAY= cargo run --example present_sync`) and drag
//! the window edge: the blue square stays glued to the white marker outline. A
//! second line of text shows the frame in which the child was last placed.

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
#[path = "example_support/x11_native_child.rs"]
mod native_child;

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod native {
    use std::{cell::Cell, rc::Rc};

    use vespertine::{
        App, Bounds, Context, Pixels, Render, Window, WindowBounds, WindowOptions, canvas, div,
        prelude::*, px, rgb, size,
    };
    use vespertine_platform::application;

    use super::native_child::NativeChild;

    struct Demo {
        child: Option<Rc<NativeChild>>,
        placements: Rc<Cell<u64>>,
    }

    impl Demo {
        fn new(window: &Window) -> Self {
            let child = NativeChild::for_window(window, 0xff2868dc).map(Rc::new);
            Self {
                child,
                placements: Rc::new(Cell::new(0)),
            }
        }
    }

    impl Render for Demo {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let child = self.child.clone();
            let placements = self.placements.clone();
            let marker = canvas(
                |_, _, _| (),
                move |bounds: Bounds<Pixels>, _, window, _| {
                    let Some(child) = child else {
                        return;
                    };
                    let scale = window.scale_factor();
                    let placements = placements.clone();
                    window.on_present(move |_, _| {
                        child.place(
                            (f32::from(bounds.origin.x) * scale) as i32,
                            (f32::from(bounds.origin.y) * scale) as i32,
                            (f32::from(bounds.size.width) * scale) as u32,
                            (f32::from(bounds.size.height) * scale) as u32,
                        );
                        placements.set(placements.get() + 1);
                    });
                },
            )
            .size_full();

            div()
                .size_full()
                .bg(rgb(0xffffff))
                .p_4()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .w(px(260.))
                        .h(px(160.))
                        .border_2()
                        .border_color(rgb(0x202020))
                        .child(marker),
                )
                .child(
                    div()
                        .text_color(rgb(0x202020))
                        .child("The blue square is a native X11 child placed from on_present."),
                )
                .child(
                    div()
                        .text_color(rgb(0x606060))
                        .child(format!("placements so far: {}", self.placements.get())),
                )
        }
    }

    pub fn run() {
        application().run(|cx: &mut App| {
            let bounds = Bounds::centered(None, size(px(420.), px(320.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |window, cx| cx.new(|_| Demo::new(window)),
            )
            .unwrap();
            cx.activate(true);
        });
    }
}

fn main() {
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    native::run();
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    eprintln!("present_sync demonstrates X11 native children and only runs on Linux");
}
