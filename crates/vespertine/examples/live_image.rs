//! ELM-004: live (in-place) image updates through an image host.
//!
//! A waveform is drawn into a pixel buffer each frame and installed with
//! `ImageHostState::set_image_pixels`; the replaced image is released from the window's atlas
//! on the next render. Click the label to clear the declarative `src`: the live image stays.

#[path = "example_support/fonts.rs"]
mod example_support;

use vespertine::{
    App, Bounds, Context, ImageHostState, Window, WindowBounds, WindowOptions, div, prelude::*, px,
    rgb, size,
};
use vespertine_platform::application;

const WIDTH: usize = 256;
const HEIGHT: usize = 128;

struct LiveImage {
    host: ImageHostState,
    frame: u32,
    src_cleared: bool,
}

impl LiveImage {
    fn draw_waveform(&self) -> Vec<u8> {
        let mut pixels = vec![0u8; WIDTH * HEIGHT * 4];
        for chunk in pixels.chunks_exact_mut(4) {
            chunk.copy_from_slice(&[0x18, 0x1A, 0x1F, 0xFF]);
        }
        let phase = self.frame as f32 * 0.1;
        for x in 0..WIDTH {
            let t = x as f32 / WIDTH as f32 * std::f32::consts::TAU * 3.0;
            let y = (HEIGHT as f32 / 2.0 + (t + phase).sin() * HEIGHT as f32 * 0.4) as usize;
            let offset = (y.min(HEIGHT - 1) * WIDTH + x) * 4;
            pixels[offset..offset + 4].copy_from_slice(&[0x80, 0xC0, 0x90, 0xFF]);
        }
        pixels
    }
}

impl Render for LiveImage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.frame += 1;
        let pixels = self.draw_waveform();
        self.host
            .set_image_pixels(WIDTH as f64, HEIGHT as f64, &pixels, Some("rgba"))
            .expect("valid pixel buffer");
        window.request_animation_frame();
        let image = self.host.render("waveform", Default::default(), window, cx);
        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .size_full()
            .bg(rgb(0x181A1F))
            .text_color(rgb(0xE2E2E2))
            .child(image)
            .child(
                div()
                    .id("clear-src")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.host.set_src(None);
                        this.src_cleared = true;
                        cx.notify();
                    }))
                    .child(if self.src_cleared {
                        "src cleared: the live image is kept".to_string()
                    } else {
                        format!("frame {} (click to clear src)", self.frame)
                    }),
            )
    }
}

fn run_example() {
    application().run(|cx: &mut App| {
        if !example_support::load_fonts(cx) {
            return;
        }
        let bounds = Bounds::centered(None, size(px(320.), px(220.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|_| {
                    let mut host = ImageHostState::new();
                    host.set_src(Some("/nonexistent/declared-source.png"));
                    LiveImage {
                        host,
                        frame: 0,
                        src_cleared: false,
                    }
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });
}

fn main() {
    run_example();
}
