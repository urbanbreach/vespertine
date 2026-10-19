//! PLT-004 / FID-PLT-004: explicit toolkit scale on X11.
//!
//! Run on X11 with a session that sets no `Xft.dpi`:
//!
//! ```text
//! WAYLAND_DISPLAY= GDK_SCALE=2 cargo run -p vespertine --example x11_toolkit_scale
//! ```
//!
//! The framework square (left) and a native X11 child (right, a solid fill from a
//! separate X connection that applies the same scale) are both 100 logical pixels,
//! so both are drawn 200 device pixels wide. The window prints the scale factor it
//! was given and the device size of the squares. With `X11_TOOLKIT_SCALE_REPORT=1`
//! the program prints the scale and both square sizes as the X server holds them
//! after the first presentation, then exits.

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
#[path = "example_support/x11_native_child.rs"]
mod native_child;

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod native {
    use std::rc::Rc;

    use vespertine::{
        App, Bounds, Context, Pixels, Render, Window, WindowBounds, WindowOptions, canvas, div,
        prelude::*, px, rgb, size,
    };
    use vespertine_platform::application;

    use super::native_child::NativeChild;

    const SQUARE: f32 = 100.;
    const FILL: u32 = 0x2868dc;

    struct Scene {
        child: Option<Rc<NativeChild>>,
    }

    impl Render for Scene {
        fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let scale = window.scale_factor();
            let child = self.child.clone();
            let native = canvas(
                |_, _, _| (),
                move |bounds: Bounds<Pixels>, _, window, _| {
                    let Some(child) = child else {
                        return;
                    };
                    let scale = window.scale_factor();
                    let report = std::env::var_os("X11_TOOLKIT_SCALE_REPORT").is_some();
                    window.on_present(move |_, _| {
                        child.place(
                            (f32::from(bounds.origin.x) * scale).round() as i32,
                            (f32::from(bounds.origin.y) * scale).round() as i32,
                            (f32::from(bounds.size.width) * scale).round() as u32,
                            (f32::from(bounds.size.height) * scale).round() as u32,
                        );
                        if report {
                            // Verification mode: report what the X server holds, then exit.
                            let framework = (f32::from(bounds.size.width) * scale).round();
                            let native = child.server_size();
                            println!(
                                "scale={scale} framework_square_px={framework} native_child_px={native:?}"
                            );
                            std::process::exit(0);
                        }
                    });
                },
            )
            .absolute()
            .left(px(150.))
            .top(px(20.))
            .size(px(SQUARE));

            div()
                .relative()
                .size_full()
                .bg(rgb(0xffffff))
                .child(
                    div()
                        .absolute()
                        .left(px(20.))
                        .top(px(20.))
                        .size(px(SQUARE))
                        .bg(rgb(FILL)),
                )
                .child(native)
                .child(
                    div()
                        .absolute()
                        .left(px(20.))
                        .top(px(160.))
                        .text_size(px(16.))
                        .line_height(px(19.2))
                        .text_color(rgb(0x202020))
                        .child("Equal logical sizes"),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(20.))
                        .top(px(190.))
                        .text_size(px(12.))
                        .text_color(rgb(0x606060))
                        .child(format!(
                            "scale factor {scale}: each square is {} device pixels",
                            (SQUARE * scale).round()
                        )),
                )
        }
    }

    pub fn run() {
        application().run(|cx: &mut App| {
            let bounds = Bounds::centered(None, size(px(500.), px(240.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |window, cx| {
                    let child = NativeChild::for_window(window, 0xff00_0000 | FILL).map(Rc::new);
                    cx.new(|_| Scene { child })
                },
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
    eprintln!("x11_toolkit_scale demonstrates an X11 session scale and only runs on Linux");
}
