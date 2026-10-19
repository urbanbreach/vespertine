//! PERF-RND-002: curved overflow clipping cost (100 panels with opt-in curved clips, clipped
//! quads, a stroked path, a label and a nested clipped box).

use crate::common::{SANS, hex};
use crate::scene::{Scene, register_scene};
use vespertine::{
    AnyElement, App, IntoElement, ParentElement, PathBuilder, Styled, Window, canvas, div, point,
    px,
};

const COLUMNS: usize = 10;
const ROWS: usize = 10;

struct Rnd002;

/// Horizontal translation of odd panels for frame `f` (zero before frame 30).
fn translation(f: u64) -> f32 {
    if f < 30 {
        return 0.0;
    }
    let t = (f % 32) as f32;
    let tri = if t <= 16.0 { t } else { 32.0 - t };
    (tri - 8.0) / 2.0
}

fn panel(k: usize, f: u64) -> impl IntoElement {
    let (row, col) = (k / COLUMNS, k % COLUMNS);
    let shift = if k % 2 == 1 { translation(f) } else { 0.0 };
    // Children are positioned relative to the panel's top-left (its border box); absolute
    // children are placed inside the padding box, which sits one border width further in.
    let at = |x: f32, y: f32| (px(x - 1.0), px(y - 1.0));
    let bar_left = ((3 * f) % 64) as f32 - 24.0;

    let (l, t) = at(-10.0, 8.0);
    let blue = div()
        .absolute()
        .left(l)
        .top(t)
        .w(px(70.0))
        .h(px(14.0))
        .bg(hex(0x3E8EDE));
    let (l, t) = at(50.0, 20.0);
    let orange = div()
        .absolute()
        .left(l)
        .top(t)
        .w(px(40.0))
        .h(px(30.0))
        .bg(hex(0xDE7A3E));
    let (l, t) = at(6.0, 2.0);
    let label = div()
        .absolute()
        .left(l)
        .top(t)
        .line_height(px(18.0))
        .text_color(hex(0xFFFFFF))
        .child(format!("Panel {k:02}"));
    let (l, t) = at(0.0, 0.0);
    let stroke = canvas(
        |_, _, _| (),
        |bounds, _, window, _| {
            // The canvas covers the panel's border box; the path is panel-relative.
            let origin = bounds.origin + point(px(1.0), px(1.0));
            let mut builder = PathBuilder::stroke(px(2.0));
            let points = [(4.0, 36.0), (20.0, 24.0), (36.0, 36.0), (52.0, 24.0), (68.0, 36.0)];
            for (i, (x, y)) in points.into_iter().enumerate() {
                let p = origin + point(px(x), px(y));
                if i == 0 {
                    builder.move_to(p);
                } else {
                    builder.line_to(p);
                }
            }
            if let Ok(path) = builder.build() {
                window.paint_path(path, hex(0x8FD16A));
            }
        },
    )
    .absolute()
    .left(l)
    .top(t)
    .w(px(80.0))
    .h(px(40.0));
    let (l, t) = at(8.0, 18.0);
    let bar = div()
        .absolute()
        .left(px(bar_left))
        .top(px(0.0))
        .w(px(24.0))
        .h(px(16.0))
        .bg(hex(0xE5C84A));
    let nested = div()
        .absolute()
        .left(l)
        .top(t)
        .w(px(64.0))
        .h(px(16.0))
        .bg(hex(0x3A4058))
        .rounded(px(6.0))
        .overflow_hidden()
        .curved_overflow_clip()
        .child(bar);

    div()
        .absolute()
        .left(px(4.0 + 88.0 * col as f32 + shift))
        .top(px(4.0 + 48.0 * row as f32))
        .w(px(80.0))
        .h(px(40.0))
        .bg(hex(0x2B3040))
        .rounded(px(10.0))
        .border(px(1.0))
        .border_color(hex(0x4A5168))
        .overflow_hidden()
        .curved_overflow_clip()
        .child(blue)
        .child(orange)
        .child(label)
        .child(stroke)
        .child(nested)
}

impl Scene for Rnd002 {
    fn size(&self) -> (f32, f32) {
        (880.0, 480.0)
    }

    fn frame(&mut self, f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        let mut root = div()
            .relative()
            .size_full()
            .bg(hex(0x1E2128))
            .font_family(SANS)
            .text_size(px(14.0));
        for k in 0..COLUMNS * ROWS {
            root = root.child(panel(k, f));
        }
        root.into_any_element()
    }
}

register_scene!("PERF-RND-002", || Rnd002);
