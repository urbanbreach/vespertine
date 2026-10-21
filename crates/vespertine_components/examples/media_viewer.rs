//! CMP-020 A11: the zoom and pan image viewer.
//!
//! The window shows a 1000 x 500 test image fitted to its viewport. A script runs on start: it
//! checks the fit, zooms about the viewport centre, zooms about a corner and checks that the
//! source point under the anchor did not move, pans, resizes the window (the zoom is kept and
//! the pan re-clamped) and finally returns to the fitted state. It prints `media_viewer: ok` when
//! every check passed and exits with status 1 otherwise. Afterwards the viewer stays interactive:
//! control + wheel or a pinch zooms about the pointer, the wheel or a left drag pans.
//!
//! `MEDIA_VIEWER_EXIT=1` exits after a successful script.

use std::sync::Arc;
use std::time::Duration;
use vespertine::prelude::FluentBuilder as _;
use vespertine::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Pixels, Point, Render,
    RenderImage, Styled, Window, WindowBounds, WindowOptions, div, point, px, rgb, size,
};
use vespertine_components::media::ImageView;
use vespertine_platform::application;

const NATURAL: (f32, f32) = (1000.0, 500.0);

/// A BGRA image with four coloured quadrants and a fine grid, so panning and zooming are visible.
fn test_image() -> Arc<RenderImage> {
    let (w, h) = (NATURAL.0 as u32, NATURAL.1 as u32);
    let mut bytes = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let quadrant = match (x < w / 2, y < h / 2) {
                (true, true) => (0xE0u8, 0x70u8, 0x60u8),
                (false, true) => (0x60, 0xC0, 0x70),
                (true, false) => (0x70, 0x90, 0xE0),
                (false, false) => (0xE0, 0xC0, 0x60),
            };
            let grid = x % 50 == 0 || y % 50 == 0;
            let (r, g, b) = if grid { (0x20, 0x20, 0x20) } else { quadrant };
            bytes.extend_from_slice(&[b, g, r, 0xFF]);
        }
    }
    Arc::new(RenderImage::from_bgra(w, h, bytes).expect("image"))
}

struct Viewer {
    view: ImageView,
    image: Arc<RenderImage>,
    /// Width the script gives the viewer pane; `None` lets it fill the window.
    pane_width: Option<Pixels>,
}

impl Render for Viewer {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex_col()
            .flex()
            .gap_2()
            .p_2()
            .bg(rgb(0x181A1F))
            .text_color(rgb(0xF0F1F3))
            .child("Control + wheel or pinch zooms, the wheel or a left drag pans")
            .child(
                div()
                    .flex_1()
                    .flex()
                    .min_h_0()
                    .when_some(self.pane_width, |pane, width| pane.w(width))
                    .child(self.view.render(
                        self.image.clone(),
                        size(px(NATURAL.0), px(NATURAL.1)),
                        None,
                        Some(rgb(0x2A2E38).into()),
                    )),
            )
    }
}

fn check(condition: bool, what: &str) {
    if !condition {
        eprintln!("media_viewer: FAILED: {what}");
        std::process::exit(1);
    }
}

fn f(p: Pixels) -> f32 {
    f32::from(p)
}

/// The natural-image point under a viewport-space anchor.
fn source_under(view: &ImageView, anchor: Point<Pixels>) -> (f32, f32) {
    let origin = view.image_origin();
    (
        (f(anchor.x) - f(origin.x)) / view.scale(),
        (f(anchor.y) - f(origin.y)) / view.scale(),
    )
}

fn close(a: (f32, f32), b: (f32, f32)) -> bool {
    (a.0 - b.0).abs() < 0.01 && (a.1 - b.1).abs() < 0.01
}

fn within_pan_limit(view: &ImageView) -> bool {
    let (n, v, s, p) = (
        view.natural_size(),
        view.viewport_size(),
        view.scale(),
        view.pan(),
    );
    let limit = |n: Pixels, v: Pixels| ((f(n) * s - f(v)) / 2.0).max(0.0);
    f(p.x).abs() <= limit(n.width, v.width) + 0.01
        && f(p.y).abs() <= limit(n.height, v.height) + 0.01
}

async fn script(
    cx: &mut vespertine::AsyncApp,
    view: ImageView,
    viewer: Entity<Viewer>,
    window: vespertine::AnyWindowHandle,
) {
    let wait =
        |cx: &mut vespertine::AsyncApp| cx.background_executor().timer(Duration::from_millis(300));
    wait(cx).await;

    // 1. Fit: the whole image is visible and never enlarged.
    check(view.is_fitted(), "starts fitted");
    check(
        view.scale() == view.fit() && view.scale() <= 1.0,
        "fit scale",
    );
    let viewport = view.viewport_size();
    check(f(viewport.width) > 0.0, "the viewport was measured");

    // 2. Zoom about the viewport centre until the image is larger than the viewport on both
    // axes (otherwise the pan clamp rightly keeps a small image centred).
    let centre = point(viewport.width / 2.0, viewport.height / 2.0);
    let before = source_under(&view, centre);
    let big = 1.6 * (f(viewport.width) / NATURAL.0).max(f(viewport.height) / NATURAL.1);
    view.zoom(big, centre);
    check(view.scale() == big && !view.is_fitted(), "zoom in");
    check(
        close(before, source_under(&view, centre)),
        "centre anchor stays fixed",
    );

    // 3. Zoom about a corner: the source point under the anchor does not move.
    let corner = point(px(0.), px(0.));
    let before = source_under(&view, corner);
    let scale = view.scale();
    view.zoom(scale * 1.1, corner);
    check(within_pan_limit(&view), "pan stays within the limit");
    let after = source_under(&view, corner);
    println!(
        "media_viewer: corner anchor source point {:.2},{:.2} -> {:.2},{:.2} at scale {:.3}",
        before.0,
        before.1,
        after.0,
        after.1,
        view.scale()
    );
    check(close(before, after), "corner anchor stays fixed");
    viewer.update(cx, |_, cx| cx.notify());
    wait(cx).await;

    // 4. Pan (what a drag does), clamped so the image is never lost.
    view.pan_by(point(px(40.), px(-25.)));
    check(within_pan_limit(&view), "pan within limit after drag");
    viewer.update(cx, |_, cx| cx.notify());
    wait(cx).await;

    // 5. Resize: the window (when the compositor honours it) and the viewer pane. The zoom is
    // kept (not refitted) and the pan re-clamped to the smaller viewport.
    let kept = view.scale();
    let before_size = view.viewport_size();
    window
        .update(cx, |_, window, _| window.resize(size(px(560.), px(380.))))
        .ok();
    viewer.update(cx, |viewer, cx| {
        viewer.pane_width = Some(px(300.));
        cx.notify();
    });
    wait(cx).await;
    check(
        f(view.viewport_size().width) < f(before_size.width),
        "the viewport shrank",
    );
    check(
        view.scale() == kept && !view.is_fitted(),
        "zoom survives a resize",
    );
    check(within_pan_limit(&view), "pan re-clamped after resize");
    viewer.update(cx, |viewer, cx| {
        viewer.pane_width = None;
        cx.notify();
    });
    wait(cx).await;
    check(view.scale() == kept, "zoom survives growing again");

    // 6. Return to fit.
    view.reset();
    viewer.update(cx, |_, cx| cx.notify());
    wait(cx).await;
    check(
        view.is_fitted() && view.scale() == view.fit(),
        "back to fit",
    );
    check(view.pan() == point(px(0.), px(0.)), "fit centres the image");
    println!("media_viewer: ok");
    if std::env::var_os("MEDIA_VIEWER_EXIT").is_some() {
        cx.update(|cx| cx.quit());
    }
}

fn main() {
    application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(800.), px(560.)), cx);
        let view = ImageView::new();
        let image = test_image();
        let handle = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                {
                    let (view, image) = (view.clone(), image);
                    move |_, cx| {
                        cx.new(|_| Viewer {
                            view,
                            image,
                            pane_width: None,
                        })
                    }
                },
            )
            .unwrap();
        let viewer = handle.entity(cx).unwrap();
        let any = handle.into();
        cx.spawn(async move |cx| script(cx, view, viewer, any).await)
            .detach();
        cx.activate(true);
    });
}
