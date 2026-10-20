//! Composite-time glyph transform and blur (RND-005), rendered headlessly through the real
//! wgpu renderer with Noto Sans text.
//!
//! Row one compares ordinary monochrome glyph painting with the transformed painter given
//! an identity transformation and zero blur (they must be pixel-identical); the rest shows a
//! rotation, a scale, a translation and a positive blur sigma of the same cached glyph. The
//! example asserts what it shows and, with a path argument, also writes the image:
//!
//! ```text
//! cargo run -p vespertine_wgpu --example glyph_transform --features test-support [out.png]
//! ```

use std::sync::Arc;
use vespertine::{
    AppContext as _, Context, HeadlessAppContext, Hsla, IntoElement, ParentElement as _, Render,
    ScaledPixels, Styled as _, TextRun, TransformationMatrix, Window, canvas, div, font, point, px,
    radians, rgb, size,
};
use vespertine_wgpu::{CosmicTextSystem, WgpuHeadlessRenderer};

const BACKGROUND: u32 = 0x202838;
const FOREGROUND: u32 = 0xF0D080;
/// Logical window size; the image is twice as large (scale factor 2).
const WIDTH: f32 = 720.0;
const HEIGHT: f32 = 200.0;
const BASELINE: f32 = 130.0;
const FONT_SIZE: f32 = 80.0;

#[derive(Clone, Copy)]
enum Variant {
    Ordinary,
    Identity,
    Rotated,
    Scaled,
    Translated,
    Blurred,
}

/// Device-space transformation about the device point `pivot` (t = pivot - M pivot).
fn about(
    pivot: (f32, f32),
    transformation: impl Fn(TransformationMatrix) -> TransformationMatrix,
) -> TransformationMatrix {
    TransformationMatrix::unit()
        .translate(point(ScaledPixels(pivot.0), ScaledPixels(pivot.1)))
        .compose(transformation(TransformationMatrix::unit()))
        .translate(point(ScaledPixels(-pivot.0), ScaledPixels(-pivot.1)))
}

struct Scene(Vec<(Variant, f32)>);

impl Render for Scene {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let glyphs = self.0.clone();
        let color: Hsla = rgb(FOREGROUND).into();
        div()
            .size_full()
            .bg(rgb(BACKGROUND))
            .child(
                canvas(
                    |_, _, _| {},
                    move |_, _, window, _| {
                        let run = TextRun {
                            strikethrough: None,
                            underline: None,
                            background_color: None,
                            color,
                            font: font("Noto Sans"),
                            len: 1,
                        };
                        let line = window.text_system().shape_line(
                            "A".into(),
                            px(FONT_SIZE),
                            &[run],
                            None,
                        );
                        let run = &line.runs[0];
                        let (font_id, glyph_id) = (run.font_id, run.glyphs[0].id);
                        for (variant, x) in &glyphs {
                            let origin = point(px(*x), px(BASELINE));
                            let device = (*x * 2.0, BASELINE * 2.0);
                            let unit = TransformationMatrix::unit();
                            let (transformation, blur) = match variant {
                                Variant::Ordinary => {
                                    window
                                        .paint_glyph(
                                            origin,
                                            font_id,
                                            glyph_id,
                                            px(FONT_SIZE),
                                            color,
                                        )
                                        .unwrap();
                                    continue;
                                }
                                Variant::Identity => (unit, 0.0),
                                Variant::Rotated => (
                                    about(device, |t| t.rotate(radians(20.0_f32.to_radians()))),
                                    0.0,
                                ),
                                Variant::Scaled => {
                                    (about(device, |t| t.scale(size(1.5, 1.5))), 0.0)
                                }
                                Variant::Translated => (
                                    unit.translate(point(ScaledPixels(60.0), ScaledPixels(-30.0))),
                                    0.0,
                                ),
                                Variant::Blurred => (unit, 3.0),
                            };
                            window
                                .paint_glyph_transformed(
                                    origin,
                                    font_id,
                                    glyph_id,
                                    px(FONT_SIZE),
                                    color,
                                    transformation,
                                    px(blur),
                                )
                                .unwrap();
                        }
                    },
                )
                .size_full(),
            )
            .into_any_element()
    }
}

fn render(cx: &mut HeadlessAppContext, glyphs: Vec<(Variant, f32)>) -> image::RgbaImage {
    let handle = cx
        .open_window(size(px(WIDTH), px(HEIGHT)), |_, cx| {
            cx.new(|_| Scene(glyphs))
        })
        .expect("open window");
    cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear(cx))
        .expect("draw");
    cx.capture_screenshot(handle.into()).expect("screenshot")
}

/// Bounding box and ink-weighted centroid of the pixels differing from the background.
struct Ink {
    bounds: (u32, u32, u32, u32),
    centroid: (f32, f32),
}

fn ink(image: &image::RgbaImage, threshold: f32) -> Ink {
    let background = [
        (BACKGROUND >> 16) as f32,
        ((BACKGROUND >> 8) & 0xFF) as f32,
        (BACKGROUND & 0xFF) as f32,
    ];
    let (mut left, mut top, mut right, mut bottom) = (u32::MAX, u32::MAX, 0, 0);
    let (mut sum, mut sx, mut sy) = (0.0, 0.0, 0.0);
    for (x, y, pixel) in image.enumerate_pixels() {
        let weight = (0..3)
            .map(|c| (f32::from(pixel.0[c]) - background[c]).abs())
            .fold(0.0, f32::max);
        if weight > threshold {
            left = left.min(x);
            right = right.max(x);
            top = top.min(y);
            bottom = bottom.max(y);
        }
        sum += weight;
        sx += weight * (x as f32 + 0.5);
        sy += weight * (y as f32 + 0.5);
    }
    Ink {
        bounds: (left, top, right, bottom),
        centroid: (sx / sum, sy / sum),
    }
}

fn main() {
    let text_system = Arc::new(CosmicTextSystem::new("Noto Sans"));
    let mut cx = HeadlessAppContext::with_platform(text_system, Arc::new(()), || {
        Ok(Some(
            Box::new(WgpuHeadlessRenderer::new()?) as Box<dyn vespertine::PlatformHeadlessRenderer>
        ))
    });

    // Ordinary and identity/zero-blur painting must agree to the pixel.
    let ordinary = render(&mut cx, vec![(Variant::Ordinary, 60.0)]);
    let identity = render(&mut cx, vec![(Variant::Identity, 60.0)]);
    assert_eq!(
        ordinary, identity,
        "identity transform and zero blur must be pixel-identical"
    );

    // The showcase: sharp, rotated 20 degrees clockwise, scaled 1.5x, translated, blurred.
    let showcase = render(
        &mut cx,
        vec![
            (Variant::Identity, 60.0),
            (Variant::Rotated, 190.0),
            (Variant::Scaled, 330.0),
            (Variant::Translated, 450.0),
            (Variant::Blurred, 600.0),
        ],
    );
    let mut solo = |variant, x| render(&mut cx, vec![(variant, x)]);
    let sharp = ink(&solo(Variant::Identity, 60.0), 8.0);
    let rotated = ink(&solo(Variant::Rotated, 190.0), 8.0);
    let scaled = ink(&solo(Variant::Scaled, 330.0), 8.0);
    let translated = ink(&solo(Variant::Translated, 450.0), 8.0);
    let blurred = ink(&solo(Variant::Blurred, 600.0), 1.0);
    let reference = ink(&solo(Variant::Identity, 600.0), 8.0);

    let height = |ink: &Ink| (ink.bounds.3 - ink.bounds.1) as f32;
    let width = |ink: &Ink| (ink.bounds.2 - ink.bounds.0) as f32;
    // Scale about the origin: 1.5 times larger.
    assert!((height(&scaled) / height(&sharp) - 1.5).abs() < 0.05);
    assert!((width(&scaled) / width(&sharp) - 1.5).abs() < 0.05);
    // Translation: the ink moves by exactly (60, -30) device pixels.
    let moved = (
        translated.centroid.0 - ink(&solo(Variant::Identity, 450.0), 8.0).centroid.0,
        translated.centroid.1 - ink(&solo(Variant::Identity, 450.0), 8.0).centroid.1,
    );
    assert!(
        (moved.0 - 60.0).abs() < 0.5 && (moved.1 + 30.0).abs() < 0.5,
        "{moved:?}"
    );
    // Rotation: clockwise about the origin point (y grows downward on screen).
    let unrotated = ink(&solo(Variant::Identity, 190.0), 8.0);
    let (pivot_x, pivot_y) = (380.0, BASELINE * 2.0);
    let (sin, cos) = 20.0_f32.to_radians().sin_cos();
    let (dx, dy) = (
        unrotated.centroid.0 - pivot_x,
        unrotated.centroid.1 - pivot_y,
    );
    let expected = (pivot_x + cos * dx - sin * dy, pivot_y + sin * dx + cos * dy);
    assert!(
        (rotated.centroid.0 - expected.0).abs() < 1.0
            && (rotated.centroid.1 - expected.1).abs() < 1.0,
        "rotated centroid {:?}, expected {expected:?}",
        rotated.centroid
    );
    // Blur sigma 3 logical = 6 device: the footprint grows within the 18 pixel halo and
    // the glyph's own position is unchanged.
    assert!(
        blurred.bounds.0 + 6 <= reference.bounds.0 && blurred.bounds.0 + 18 >= reference.bounds.0
    );
    assert!(
        blurred.bounds.2 >= reference.bounds.2 + 6 && blurred.bounds.2 <= reference.bounds.2 + 18
    );
    assert!((blurred.centroid.0 - reference.centroid.0).abs() < 1.0);
    assert!((blurred.centroid.1 - reference.centroid.1).abs() < 1.0);

    if let Some(path) = std::env::args().nth(1) {
        showcase.save(&path).expect("write image");
        println!("wrote {path}");
    }
    println!(
        "ok: identity == ordinary; scale x1.5, translate (60, -30), rotate 20 deg, blur sigma 3 verified"
    );
}
