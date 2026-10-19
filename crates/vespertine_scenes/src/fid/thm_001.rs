//! FID-THM-001: twelve swatches whose fills are CSS color text parsed by the shared parser.

use crate::scene::{Scene, register_scene};
use vespertine::{
    AnyElement, App, IntoElement, ParentElement, Styled, Window, css_color_to_rgba, div, px, rgb,
};

const FILLS: [&str; 12] = [
    "rgb(255 0 0)",
    "oklch(0.62796 0.25768 29.23388)",
    "rgba(255,0,0,0.5)",
    "rebeccapurple",
    "hsl(120 100% 25%)",
    "#0f08",
    "hwb(210 20% 30%)",
    "hsv(30 100% 100%)",
    "lab(60% 40 50)",
    "oklch(90% 0.4 40)",
    "rgb(from #bad455 b r g / alpha)",
    "oklab(from #bad455 calc(l * 0.7) a b)",
];

struct Thm001;

impl Scene for Thm001 {
    fn size(&self) -> (f32, f32) {
        (480.0, 180.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        let mut root = div().relative().size_full().bg(rgb(0xFFFFFF));
        for (index, text) in FILLS.into_iter().enumerate() {
            let fill = css_color_to_rgba(text).expect("FID-THM-001 fill text parses");
            let left = 20.0 + 80.0 * (index % 6) as f32;
            let top = if index < 6 { 30.0 } else { 110.0 };
            root = root.child(
                div()
                    .absolute()
                    .left(px(left))
                    .top(px(top))
                    .w(px(40.0))
                    .h(px(40.0))
                    .bg(fill),
            );
        }
        root.into_any_element()
    }
}

register_scene!("FID-THM-001", || Thm001);
