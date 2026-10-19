//! Headless rendering for the rounded-clip examples: a real element tree, the platform's
//! text system and the platform's headless renderer, drawn at the test window's scale
//! factor 2 and read back as a device-pixel image.
#![allow(dead_code)]

use image::RgbaImage;
use std::{path::PathBuf, sync::Arc};
use vespertine::{
    AnyElement, App, AppContext as _, Context, HeadlessAppContext, IntoElement, Render,
    ScopeParams, Window, px, size,
};

struct Root {
    build: Box<dyn Fn(&mut Window, &mut App) -> AnyElement>,
}

impl Render for Root {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        (self.build)(window, cx)
    }
}

pub struct Host {
    cx: HeadlessAppContext,
}

impl Host {
    pub fn new() -> Self {
        let text_system = vespertine_platform::current_platform(true).text_system();
        let cx = HeadlessAppContext::with_platform(text_system, Arc::new(()), || {
            vespertine_platform::current_headless_renderer()
        });
        Self { cx }
    }

    /// Draws `build` in a window of `width` x `height` logical pixels and returns the image
    /// (twice that size: the window's scale factor is 2).
    pub fn render(
        &mut self,
        width: f32,
        height: f32,
        build: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
    ) -> RgbaImage {
        let handle = self
            .cx
            .open_window(size(px(width), px(height)), |_, cx| {
                cx.new(|_| Root {
                    build: Box::new(build),
                })
            })
            .expect("open window");
        self.cx
            .update_window(handle.into(), |_, window, cx| window.draw(cx).clear(cx))
            .expect("draw");
        self.cx
            .capture_screenshot(handle.into())
            .expect("capture screenshot: is a GPU adapter available?")
    }
}

/// The directory given as `--out <dir>`, when screenshots should be written.
pub fn output_directory() -> Option<PathBuf> {
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == "--out" {
            let directory = PathBuf::from(arguments.next().expect("--out needs a directory"));
            std::fs::create_dir_all(&directory).expect("create the output directory");
            return Some(directory);
        }
    }
    None
}

pub fn save(directory: &Option<PathBuf>, name: &str, image: &RgbaImage) {
    if let Some(directory) = directory {
        let path = directory.join(format!("{name}.png"));
        image.save(&path).expect("write screenshot");
        println!("wrote {}", path.display());
    }
}

/// A rounded clip scope with `bounds` (left, top, right, bottom) and `radius`, in device
/// pixels, for the reference coverage K.
pub fn scope(bounds: [f32; 4], radius: f32) -> ScopeParams {
    ScopeParams {
        clip_bounds: bounds,
        clip_radius: radius,
        ..Default::default()
    }
}

/// Number of pixels of `image` that differ from `reference` outside the curve (K = 0, with a
/// one pixel margin) by more than `tolerance` in any channel; and the number inside the curve
/// (K = 1) that differ at all, i.e. that the clipped content actually painted.
pub fn compare_with_reference(
    image: &RgbaImage,
    reference: &RgbaImage,
    scope: &ScopeParams,
    tolerance: u8,
) -> (usize, usize) {
    assert_eq!(image.dimensions(), reference.dimensions());
    let (mut leaked, mut painted) = (0, 0);
    for (x, y, pixel) in image.enumerate_pixels() {
        let p = [x as f32 + 0.5, y as f32 + 0.5];
        let base = reference.get_pixel(x, y);
        let differs = pixel
            .0
            .iter()
            .zip(base.0)
            .any(|(a, b)| a.abs_diff(b) > tolerance);
        if scope.rounded_coverage(p) == 0.0 {
            if differs {
                leaked += 1;
            }
        } else if scope.rounded_coverage(p) == 1.0 && pixel.0 != base.0 {
            painted += 1;
        }
    }
    (leaked, painted)
}
