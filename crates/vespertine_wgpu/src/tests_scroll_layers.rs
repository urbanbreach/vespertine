//! PRF-004 A2: a surfaceless GPU test comparing the pixels of a window that composites scroll
//! layers (cached 512 x 512 device-pixel tiles, paths replayed live over them) with those of a
//! window that draws the same content directly, byte for byte.
//!
//! The content has odd-device-pixel translations (0.9 logical px per frame at scale factor 2),
//! glyphs that straddle tile edges, drop shadows that reach into neighbouring rows, paths, and
//! backgrounds that change in the middle of the scroll.

use crate::{CosmicTextSystem, WgpuHeadlessRenderer};
use std::sync::Arc;
use vespertine::{
    AnyWindowHandle, App, AppContext as _, BoxShadow, Context, Entity, HeadlessAppContext,
    InputEvent as _, InteractiveElement as _, IntoElement, Modifiers, ParentElement as _,
    PathBuilder, Render, RenderImage, RequestFrameOptions, ScrollDelta, ScrollWheelEvent,
    StatefulInteractiveElement as _, Styled as _, TouchPhase, Window, canvas, div, hsla, img,
    point, px, rgb, size,
};

const ROWS: usize = 70;
const WINDOW: (f32, f32) = (440.0, 260.0);
const SCALE: f32 = 2.0;

struct Tint(u32);

struct Page {
    tint: Entity<Tint>,
}

fn drop_shadow() -> BoxShadow {
    BoxShadow {
        color: hsla(0.0, 0.0, 0.0, 0.4),
        offset: point(px(0.), px(5.)),
        blur_radius: px(9.),
        spread_radius: px(1.),
        inset: false,
    }
}

fn row(ix: usize, tint: u32) -> impl IntoElement {
    let base = if ix.is_multiple_of(2) {
        0xffffff
    } else {
        0xf7f7f7
    };
    let fill = if tint % 2 == 1 && ix % 5 < 2 {
        0xe3edff
    } else {
        base
    };
    let mut row = div()
        .id(("row", ix))
        .relative()
        .flex_none()
        .flex()
        .items_center()
        .w(px(400.))
        .h(px(40.))
        .pl(px(12.))
        .bg(rgb(fill))
        .text_size(px(14.))
        .text_color(rgb(0x202020))
        .child(format!("Item {ix}"));
    if ix % 4 == 1 {
        row = row.shadow(vec![drop_shadow()]);
    }
    if ix.is_multiple_of(3) {
        // 254 logical px is the last 2 px of a 256 px (512 device px) tile column.
        row = row.child(
            div()
                .absolute()
                .left(px(251.))
                .top(px(10.))
                .child("Straddling glyphs"),
        );
    }
    row.child(
        canvas(
            |bounds, _, _| bounds,
            |bounds, _, window, _| {
                let mut stroke = PathBuilder::stroke(px(1.));
                stroke.move_to(point(bounds.origin.x + px(150.), bounds.origin.y + px(20.)));
                stroke.line_to(point(bounds.origin.x + px(160.), bounds.origin.y + px(10.)));
                stroke.line_to(point(bounds.origin.x + px(170.), bounds.origin.y + px(25.)));
                stroke.line_to(point(bounds.origin.x + px(180.), bounds.origin.y + px(15.)));
                window.paint_path(stroke.build().expect("polyline"), rgb(0x2868dc));
            },
        )
        .absolute()
        .size_full(),
    )
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tint = self.tint.read(cx).0;
        div()
            .size_full()
            .bg(rgb(0xeeeeee))
            .font_family("Noto Sans")
            .child(
                div()
                    .id("scroller")
                    .absolute()
                    .left(px(20.))
                    .top(px(20.))
                    .w(px(400.))
                    .h(px(220.))
                    .bg(rgb(0xffffff))
                    .overflow_y_scroll()
                    .children((0..ROWS).map(move |ix| row(ix, tint))),
            )
    }
}

/// Two windows showing the same page over the shared state `S`: one composites scroll layers,
/// the other draws directly.
struct Host<S: 'static = Tint> {
    cx: HeadlessAppContext,
    state: Entity<S>,
    layered: AnyWindowHandle,
    direct: AnyWindowHandle,
}

impl Host {
    fn new() -> Self {
        Self::open(Tint(0), |tint| Page { tint })
    }

    fn tint(&mut self) {
        self.change(|tint| tint.0 += 1);
    }
}

impl<S: 'static> Host<S> {
    fn open<V: Render>(state: S, page: impl Fn(Entity<S>) -> V + Clone + 'static) -> Self {
        let text_system = Arc::new(CosmicTextSystem::new("Noto Sans"));
        let mut cx = HeadlessAppContext::with_platform(text_system, Arc::new(()), || {
            Ok(Some(Box::new(WgpuHeadlessRenderer::new()?)
                as Box<dyn vespertine::PlatformHeadlessRenderer>))
        });
        let state = cx.update(|cx: &mut App| cx.new(|_| state));
        let mut open = |layers: bool| {
            let (state, page) = (state.clone(), page.clone());
            let handle: AnyWindowHandle = cx
                .open_window(size(px(WINDOW.0), px(WINDOW.1)), move |_, cx| {
                    cx.new(|_| page(state))
                })
                .expect("open window")
                .into();
            cx.simulate_scale_factor_change(handle, SCALE);
            cx.update_window(handle, |_, window, _| {
                window.set_scroll_layers_enabled(layers)
            })
            .expect("window");
            handle
        };
        let layered = open(true);
        let direct = open(false);
        let mut host = Self {
            cx,
            state,
            layered,
            direct,
        };
        host.frame();
        host
    }

    /// Draws and presents both windows when they changed.
    fn frame(&mut self) {
        for window in [self.layered, self.direct] {
            self.cx
                .request_frame(
                    window,
                    RequestFrameOptions {
                        require_presentation: true,
                        ..Default::default()
                    },
                )
                .expect("frame");
        }
    }

    fn wheel(&mut self, delta_y: f32) {
        for window in [self.layered, self.direct] {
            self.cx
                .simulate_platform_input(
                    window,
                    ScrollWheelEvent {
                        position: point(px(220.), px(120.)),
                        delta: ScrollDelta::Pixels(point(px(0.), px(delta_y))),
                        modifiers: Modifiers::default(),
                        touch_phase: TouchPhase::Moved,
                    }
                    .to_platform_input(),
                )
                .expect("input");
        }
        self.frame();
    }

    /// Changes the shared state, notifies the pages and draws both windows.
    fn change(&mut self, change: impl FnOnce(&mut S)) {
        let state = self.state.clone();
        self.cx.update(|cx| {
            state.update(cx, |state, cx| {
                change(state);
                cx.notify();
            })
        });
        self.frame();
    }

    fn assert_same_pixels(&mut self, what: &str) {
        let layered = self
            .cx
            .capture_screenshot(self.layered)
            .expect("screenshot");
        let direct = self.cx.capture_screenshot(self.direct).expect("screenshot");
        assert_eq!(layered.dimensions(), direct.dimensions());
        if layered.as_raw() != direct.as_raw() {
            let width = layered.width();
            let first = layered
                .as_raw()
                .chunks_exact(4)
                .zip(direct.as_raw().chunks_exact(4))
                .position(|(a, b)| a != b)
                .expect("a differing pixel");
            let differing = layered
                .as_raw()
                .chunks_exact(4)
                .zip(direct.as_raw().chunks_exact(4))
                .filter(|(a, b)| a != b)
                .count();
            panic!(
                "{what}: {differing} pixels differ, the first at ({}, {}): layered {:?}, direct {:?}",
                first as u32 % width,
                first as u32 / width,
                layered
                    .get_pixel(first as u32 % width, first as u32 / width)
                    .0,
                direct
                    .get_pixel(first as u32 % width, first as u32 / width)
                    .0
            );
        }
    }

    fn stats(&mut self, window: AnyWindowHandle) -> vespertine::FrameStats {
        self.cx
            .update_window(window, |_, window, _| window.frame_stats())
            .expect("stats")
    }
}

#[test]
fn a2_tiled_scroll_layers_draw_the_pixels_of_direct_drawing() {
    let mut host = Host::new();
    host.assert_same_pixels("initial frame");
    // 45 frames of 0.9 logical px: 81 device px, an odd translation of the path overlays.
    for step in 0..45 {
        host.wheel(-0.9);
        host.assert_same_pixels(&format!("slow scroll, frame {step}"));
    }
    // Faster scrolling crosses tile edges and repaints around the viewport.
    for step in 0..40 {
        host.wheel(-37.3);
        host.assert_same_pixels(&format!("fast scroll, frame {step}"));
    }
    // Reversal through content whose tiles were already rasterized.
    for step in 0..30 {
        host.wheel(11.1);
        host.assert_same_pixels(&format!("reversal, frame {step}"));
    }
    // A changed background repaints the layer; dirty tiles are rasterized again.
    host.tint();
    host.assert_same_pixels("tinted backgrounds");
    for step in 0..20 {
        host.wheel(-0.9);
        host.assert_same_pixels(&format!("scroll after the change, frame {step}"));
    }
    host.tint();
    host.assert_same_pixels("tinted backgrounds again");

    let (layered, direct) = (host.layered, host.direct);
    let layered = host.stats(layered);
    let direct = host.stats(direct);
    assert!(
        layered.layer_composites > 40 && layered.tiles_rasterized > 0,
        "the layered window composited tiles: {layered:?}"
    );
    assert_eq!(
        (
            direct.layer_composites,
            direct.layer_repaints,
            direct.tiles_rasterized
        ),
        (0, 0, 0),
        "the direct window used no layer"
    );
}

/// A picture the page shows in some of its rows, and the scroller's background.
struct Picture {
    image: Arc<RenderImage>,
    background: u32,
}

/// Rows narrower than the scroller, so that its background shows beside them.
struct PicturePage {
    picture: Entity<Picture>,
}

impl Render for PicturePage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let picture = self.picture.read(cx);
        let image = picture.image.clone();
        div().size_full().bg(rgb(0xeeeeee)).child(
            div()
                .id("scroller")
                .absolute()
                .left(px(20.))
                .top(px(20.))
                .w(px(400.))
                .h(px(220.))
                .bg(rgb(picture.background))
                .overflow_y_scroll()
                .children((0..ROWS).map(move |ix| {
                    let row =
                        div()
                            .flex_none()
                            .w(px(300.))
                            .h(px(40.))
                            .bg(rgb(if ix.is_multiple_of(2) {
                                0xffffff
                            } else {
                                0xf7f7f7
                            }));
                    if ix % 3 == 0 {
                        row.child(img(image.clone()).w(px(40.)).h(px(30.)))
                    } else {
                        row
                    }
                })),
        )
    }
}

fn solid_frames(pixel: [u8; 4]) -> Vec<image::Frame> {
    vec![image::Frame::new(image::RgbaImage::from_pixel(
        8,
        8,
        image::Rgba(pixel),
    ))]
}

impl Host<Picture> {
    /// Gives the picture new pixels in place in both windows.
    fn update_image_in_place(&mut self, pixel: [u8; 4]) -> Arc<RenderImage> {
        let image = self
            .state
            .read_with(&self.cx, |picture, _| picture.image.clone());
        let updated = Arc::new(image.with_updated_frames(solid_frames(pixel)));
        for window in [self.layered, self.direct] {
            let updated = updated.clone();
            self.cx
                .update_window(window, move |_, window, _| {
                    assert!(window.update_image(&updated).expect("update"));
                })
                .expect("window");
        }
        updated
    }
}

/// Tiles of a layer show what its content looks like now: an image updated in place and a
/// changed background of the scroller reach them, whether the frame is presented again or the
/// layer is repainted.
#[test]
fn a2_scroll_layer_tiles_follow_images_updated_in_place_and_a_new_background() {
    let mut host = Host::open(
        Picture {
            image: Arc::new(RenderImage::new(solid_frames([220, 40, 40, 255]))),
            background: 0xffffff,
        },
        |picture| PicturePage { picture },
    );
    for _ in 0..6 {
        host.wheel(-7.3);
    }
    host.assert_same_pixels("scrolled");

    // The cached frame is presented again with the new pixels.
    host.update_image_in_place([40, 40, 220, 255]);
    for window in [host.layered, host.direct] {
        host.cx
            .update_window(window, |_, window, _| {
                assert!(window.present_cached_frame());
            })
            .expect("window");
    }
    host.assert_same_pixels("image updated in place, cached frame presented");

    // The page is painted again after another update in place.
    let updated = host.update_image_in_place([40, 160, 40, 255]);
    host.change(|picture| picture.image = updated);
    host.assert_same_pixels("image updated in place, page repainted");

    // The background changes while the layer is in use.
    host.wheel(-7.3);
    host.wheel(-7.3);
    host.change(|picture| picture.background = 0xdde8ff);
    host.assert_same_pixels("scroller background changed");
    for step in 0..3 {
        host.wheel(-7.3);
        host.assert_same_pixels(&format!("scroll after the background change, frame {step}"));
    }

    let layered = host.layered;
    let layered = host.stats(layered);
    assert!(
        layered.layer_composites > 0,
        "the layered window composited tiles: {layered:?}"
    );
}
