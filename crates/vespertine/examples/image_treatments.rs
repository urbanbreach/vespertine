//! Cached source-space image treatments (ELM-005).
//!
//! Run with: `cargo run -p vespertine --example image_treatments`
//!
//! A generated picture is shown through each treatment in turn (none, scanlines,
//! dither, halftone, text-density), the effect changing every 400 ms. Resize the
//! window while it runs: the treated images are computed once per source and
//! effect on the background executor and reused for every size, and while a new
//! treatment is being prepared the current image stays visible. When it is
//! ready the new image crossfades in over the old one in 180 ms. Click the
//! window to switch between dark and light paper.

#[path = "example_support/fonts.rs"]
mod example_support;

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use vespertine::{
    App, Bounds, Context, CrossfadeTracker, GlyphPalette, ImageSource, ImageTreatment, ObjectFit,
    StyledImage, TreatmentAppearance, TreatmentCache, Window, WindowBounds, WindowOptions, div,
    img, prelude::*, px, rgb, size,
};
use vespertine_platform::application;

const EFFECTS: [(ImageTreatment, &str); 5] = [
    (ImageTreatment::None, "none"),
    (ImageTreatment::Scanlines, "scanlines"),
    (ImageTreatment::Dither, "dither"),
    (ImageTreatment::Halftone, "halftone"),
    (ImageTreatment::TextDensity, "text density"),
];

struct Treatments {
    cache: TreatmentCache,
    source: PathBuf,
    effect: usize,
    light: bool,
    tracker: CrossfadeTracker<(), PathBuf>,
}

impl Treatments {
    fn new(source: PathBuf, cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(400))
                    .await;
                let alive = this.update(cx, |this, cx| {
                    this.effect = (this.effect + 1) % EFFECTS.len();
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            cache: TreatmentCache::new(GlyphPalette::ink_ramp()),
            source,
            effect: 0,
            light: false,
            tracker: CrossfadeTracker::new(),
        }
    }
}

impl Render for Treatments {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (treatment, name) = EFFECTS[self.effect];
        let appearance = if self.light {
            TreatmentAppearance::Light
        } else {
            TreatmentAppearance::Dark
        };
        // Nothing while the background job runs: the tracker keeps the current image.
        let treated = self.cache.request(treatment, &self.source, appearance, cx);
        let frame = self.tracker.frame(
            treated.as_ref(),
            &self.source,
            &(),
            true,
            cx.reduce_motion(),
            Instant::now(),
        );
        if frame.active {
            window.request_animation_frame();
        }
        let (previous_opacity, current_opacity) = frame.layer_opacities(1.0);
        let layer = |image: Option<(std::sync::Arc<vespertine::RenderImage>, ())>, opacity: f32| {
            image.map(|(image, _)| {
                img(ImageSource::Render(image))
                    .object_fit(ObjectFit::Cover)
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .opacity(opacity)
            })
        };

        div()
            .id("treatments")
            .size_full()
            .relative()
            .bg(if self.light {
                rgb(0xffffff)
            } else {
                rgb(0x000000)
            })
            .text_color(rgb(0x808080))
            .on_click(cx.listener(|this, _, _, cx| {
                this.light = !this.light;
                cx.notify();
            }))
            .children(layer(frame.previous, previous_opacity))
            .children(layer(frame.current, current_opacity))
            .child(div().absolute().bottom_2().left_2().child(format!(
                "{name} ({}), {} job(s) started",
                if self.light { "light" } else { "dark" },
                self.cache.jobs_started()
            )))
    }
}

/// Writes a colorful 640 x 400 test picture and returns its path.
fn generate_source() -> PathBuf {
    let picture = image::RgbaImage::from_fn(640, 400, |x, y| {
        let ring = (((x as f32 - 320.).powi(2) + (y as f32 - 200.).powi(2)).sqrt() / 8.).sin();
        image::Rgba([
            (x * 255 / 639) as u8,
            (y * 255 / 399) as u8,
            (128. + 127. * ring) as u8,
            255,
        ])
    });
    let path =
        std::env::temp_dir().join(format!("vespertine-treatments-{}.png", std::process::id()));
    picture
        .save_with_format(&path, image::ImageFormat::Png)
        .expect("write the generated picture");
    path
}

fn main() {
    application().run(|cx: &mut App| {
        if !example_support::load_fonts(cx) {
            return;
        }
        let source = generate_source();
        let bounds = Bounds::centered(None, size(px(640.), px(400.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|cx| Treatments::new(source, cx)),
        )
        .unwrap();
        cx.activate(true);
    });
}
