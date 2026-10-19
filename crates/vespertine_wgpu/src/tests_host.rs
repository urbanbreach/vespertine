//! A production-path pixel host for renderer tests: real element tree, real text system,
//! real wgpu renderer, offscreen target. One window is drawn per `render` call at scale
//! factor 2 (the scene environment of the fidelity scenes).

use crate::{CosmicTextSystem, WgpuHeadlessRenderer};
use image::RgbaImage;
use std::sync::Arc;
use vespertine::{
    AnyElement, App, AppContext as _, HeadlessAppContext, IntoElement, Pixels, Render, Size,
    Window, px, size,
};

struct Root {
    build: Box<dyn Fn(&mut Window, &mut App) -> AnyElement>,
}

impl Render for Root {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut vespertine::Context<Self>,
    ) -> impl IntoElement {
        (self.build)(window, cx)
    }
}

/// Renders element trees to pixels through the production painting path.
pub(crate) struct RenderHost {
    cx: HeadlessAppContext,
}

impl RenderHost {
    pub(crate) fn new() -> Self {
        Self::with_renderer(WgpuHeadlessRenderer::new)
    }

    /// A host whose target is a transparent window composited as premultiplied alpha:
    /// unpainted pixels read back as `[0, 0, 0, 0]`.
    pub(crate) fn new_transparent() -> Self {
        Self::with_renderer(WgpuHeadlessRenderer::new_transparent)
    }

    fn with_renderer(
        renderer: impl Fn() -> anyhow::Result<WgpuHeadlessRenderer> + 'static,
    ) -> Self {
        let text_system = Arc::new(CosmicTextSystem::new("Noto Sans"));
        let cx = HeadlessAppContext::with_platform(text_system, Arc::new(()), move || {
            Ok(Some(
                Box::new(renderer()?) as Box<dyn vespertine::PlatformHeadlessRenderer>
            ))
        });
        Self { cx }
    }

    /// Like [`Self::render_logical`] at an explicit display scale factor.
    pub(crate) fn render_at_scale(
        &mut self,
        width: f32,
        height: f32,
        scale_factor: f32,
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
            .simulate_scale_factor_change(handle.into(), scale_factor);
        self.cx
            .update_window(handle.into(), |_, window, cx| window.draw(cx).clear(cx))
            .expect("draw");
        self.cx
            .capture_screenshot(handle.into())
            .expect("screenshot")
    }

    /// Draws the element returned by `build` in a window of the given logical size and
    /// returns the device-pixel image.
    pub(crate) fn render(
        &mut self,
        size: Size<Pixels>,
        build: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
    ) -> RgbaImage {
        let handle = self
            .cx
            .open_window(size, |_, cx| {
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
            .expect("screenshot")
    }

    /// Convenience for a logical width and height.
    pub(crate) fn render_logical(
        &mut self,
        width: f32,
        height: f32,
        build: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
    ) -> RgbaImage {
        self.render(size(px(width), px(height)), build)
    }
}

#[test]
fn host_renders_a_styled_div_at_scale_factor_two() {
    use vespertine::{Styled, div, rgb};
    let mut host = RenderHost::new();
    let image = host.render_logical(40.0, 20.0, |_, _| {
        div().size_full().bg(rgb(0x204060)).into_any_element()
    });
    assert_eq!(image.dimensions(), (80, 40));
    let pixel = image.get_pixel(10, 10).0;
    assert!(
        pixel[0].abs_diff(0x20) <= 2
            && pixel[1].abs_diff(0x40) <= 2
            && pixel[2].abs_diff(0x60) <= 2
    );
}
