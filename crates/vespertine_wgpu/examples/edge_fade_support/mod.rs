//! Shared headless host and reference arithmetic of the RND-001 examples: a real element
//! tree, the real text system and the real wgpu renderer drawn offscreen at a chosen scale
//! factor, plus the specification's fragment factor (R6) written independently of the
//! renderer's own reference code.

use image::RgbaImage;
use std::sync::Arc;
use vespertine::{
    AnyElement, App, AppContext as _, Context, HeadlessAppContext, IntoElement, Render, Window, px,
    size,
};
use vespertine_wgpu::{CosmicTextSystem, WgpuHeadlessRenderer};

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
        let text_system = Arc::new(CosmicTextSystem::new("Noto Sans"));
        let cx = HeadlessAppContext::with_platform(text_system, Arc::new(()), || {
            Ok(Some(Box::new(WgpuHeadlessRenderer::new()?)
                as Box<dyn vespertine::PlatformHeadlessRenderer>))
        });
        Self { cx }
    }

    /// Renders the element built by `build` in a window of `width` × `height` logical pixels
    /// at `scale`, returning the device-pixel image.
    pub fn render(
        &mut self,
        width: f32,
        height: f32,
        scale: f32,
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
        self.cx.simulate_scale_factor_change(handle.into(), scale);
        self.cx
            .update_window(handle.into(), |_, window, cx| window.draw(cx).clear(cx))
            .expect("draw");
        let image = self
            .cx
            .capture_screenshot(handle.into())
            .expect("screenshot");
        self.cx
            .update_window(handle.into(), |_, window, _| window.remove_window())
            .expect("close window");
        image
    }
}

/// A fade region in device pixels and its four effective device band widths (top, bottom,
/// left, right); a width of zero means the edge is disabled.
#[derive(Clone, Copy)]
pub struct DeviceFade {
    pub top: f32,
    pub bottom: f32,
    pub left: f32,
    pub right: f32,
    pub bands: [f32; 4],
}

impl DeviceFade {
    /// RND-001 R6: the edge coverage of the fragment at the center of device pixel (x, y).
    pub fn coverage(&self, x: u32, y: u32) -> f32 {
        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
        let mut u = 1.0f32;
        if self.bands[0] > 0.0 {
            u = u.min(((py - self.top) / self.bands[0]).clamp(0.0, 1.0));
        }
        if self.bands[1] > 0.0 {
            u = u.min(((self.bottom - py) / self.bands[1]).clamp(0.0, 1.0));
        }
        if self.bands[2] > 0.0 {
            u = u.min(((px - self.left) / self.bands[2]).clamp(0.0, 1.0));
        }
        if self.bands[3] > 0.0 {
            u = u.min(((self.right - px) / self.bands[3]).clamp(0.0, 1.0));
        }
        u * u
    }
}
