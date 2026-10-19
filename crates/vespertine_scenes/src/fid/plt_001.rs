//! FID-PLT-001-1 and FID-PLT-001-2: scene layering around native child content.
//!
//! The native child is an independently presented surface (a `wl_shm` surface on Wayland, a
//! child window on X11, see [`vespertine_platform::SolidProducer`]) that always shows its solid
//! color(s) over its whole bounds. The overlay content is deferred framework content: it is
//! painted into the transparent overlay plane above the native child.

use crate::common::{SANS, WHITE, hex};
use crate::scene::{Scene, register_scene};
use vespertine::{
    AnyElement, App, Bounds, BoxShadow, FontWeight, Hsla, IntoElement, ParentElement, Pixels,
    Styled, Window, deferred, div, hsla, point, px,
};
pub(crate) use vespertine_platform::SolidChild as NativeChild;

fn bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
    Bounds {
        origin: point(px(x), px(y)),
        size: vespertine::size(px(width), px(height)),
    }
}

fn black(opacity: f32) -> Hsla {
    hsla(0.0, 0.0, 0.0, opacity)
}

/// A single-line text box with its top-left at `(left, top)`, line height 1.2 times the size.
#[allow(clippy::too_many_arguments)]
pub(crate) fn text_box(
    text: impl Into<vespertine::SharedString>,
    left: f32,
    top: f32,
    size: f32,
    weight: FontWeight,
    color: Hsla,
) -> impl IntoElement {
    div()
        .absolute()
        .left(px(left))
        .top(px(top))
        .whitespace_nowrap()
        .font_family(SANS)
        .font_weight(weight)
        .text_size(px(size))
        .line_height(px(size * 1.2))
        .text_color(color)
        .child(text.into())
}

/// The overlay of FID-PLT-001-2 and PERF-PLT-001: a backdrop over the whole client area, a
/// dialog with border and drop shadow, its title and a button.
pub(crate) fn dialog_overlay(width: f32, height: f32) -> AnyElement {
    div()
        .absolute()
        .left(px(0.0))
        .top(px(0.0))
        .w(px(width))
        .h(px(height))
        .child(
            div()
                .absolute()
                .left(px(0.0))
                .top(px(0.0))
                .w(px(width))
                .h(px(height))
                .bg(black(0.2)),
        )
        .child(
            div()
                .absolute()
                .left(px(260.0))
                .top(px(100.0))
                .w(px(340.0))
                .h(px(220.0))
                .bg(hex(WHITE))
                .rounded(px(12.0))
                .border(px(1.0))
                .border_color(hex(0xb8bec8))
                .shadow(vec![BoxShadow {
                    color: black(0.25),
                    offset: point(px(0.0), px(6.0)),
                    blur_radius: px(16.0),
                    spread_radius: px(0.0),
                    inset: false,
                }]),
        )
        .child(text_box(
            "Overlay above native content",
            280.0,
            124.0,
            18.0,
            FontWeight::BOLD,
            hex(0x202020),
        ))
        .child(
            div()
                .absolute()
                .left(px(450.0))
                .top(px(250.0))
                .w(px(120.0))
                .h(px(36.0))
                .rounded(px(6.0))
                .bg(hex(0x2868dc))
                .flex()
                .items_center()
                .justify_center()
                .font_family(SANS)
                .font_weight(FontWeight::NORMAL)
                .text_size(px(14.0))
                .line_height(px(14.0 * 1.2))
                .text_color(hex(WHITE))
                .child("Close"),
        )
        .into_any_element()
}

/// The deferred overlay wrapper: the content is painted into the overlay plane.
pub(crate) fn overlay(content: impl IntoElement) -> AnyElement {
    deferred(content).with_priority(1).into_any_element()
}

struct Plt001Layering {
    native: NativeChild,
}

impl Scene for Plt001Layering {
    fn size(&self) -> (f32, f32) {
        (640.0, 400.0)
    }

    fn frame(&mut self, _f: u64, window: &mut Window, _cx: &mut App) -> AnyElement {
        self.native.prepare(window);
        let child = bounds(160.0, 100.0, 320.0, 180.0);
        self.native.present(window, child, 0x2060c0, 0x2060c0);
        div()
            .relative()
            .size_full()
            .bg(hex(0x101820))
            .child(text_box(
                "Below native child",
                40.0,
                40.0,
                24.0,
                FontWeight::NORMAL,
                hex(WHITE),
            ))
            .child(self.native.slot(child))
            .child(overlay(
                div()
                    .absolute()
                    .left(px(0.0))
                    .top(px(0.0))
                    .size_full()
                    .child(
                        div()
                            .absolute()
                            .left(px(320.0))
                            .top(px(160.0))
                            .w(px(240.0))
                            .h(px(120.0))
                            .rounded(px(16.0))
                            .bg(hex(0xf0f0f0)),
                    )
                    .child(text_box(
                        "Menu above child",
                        344.0,
                        206.0,
                        24.0,
                        FontWeight::NORMAL,
                        hex(0x101820),
                    )),
            ))
            .into_any_element()
    }
}

register_scene!("FID-PLT-001-1", || Plt001Layering {
    native: NativeChild::new(),
});

struct Plt001Dialog {
    native: NativeChild,
}

impl Scene for Plt001Dialog {
    fn size(&self) -> (f32, f32) {
        (800.0, 460.0)
    }

    fn frame(&mut self, _f: u64, window: &mut Window, _cx: &mut App) -> AnyElement {
        self.native.prepare(window);
        let child = bounds(180.0, 40.0, 580.0, 340.0);
        self.native.present(window, child, 0x2368b4, 0x68b0e8);
        div()
            .relative()
            .size_full()
            .bg(hex(0xf3f4f6))
            .child(
                div()
                    .absolute()
                    .left(px(0.0))
                    .top(px(0.0))
                    .w(px(160.0))
                    .h(px(460.0))
                    .bg(hex(0xdde2ea)),
            )
            .child(self.native.slot(child))
            .child(overlay(dialog_overlay(800.0, 460.0)))
            .into_any_element()
    }
}

register_scene!("FID-PLT-001-2", || Plt001Dialog {
    native: NativeChild::new(),
});
