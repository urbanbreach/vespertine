//! PERF-QUADS: many styled quads with shadows.

use crate::scene::{Scene, register_scene};
use crate::common::{WHITE, hex, hsl_deg};
use vespertine::{
    AnyElement, App, BoxShadow, IntoElement, ParentElement, Styled, Window, div, point, px, rgba,
};

const COLUMNS: usize = 40;
const BOXES: usize = 1000;

pub struct QuadsScene;

impl QuadsScene {
    pub fn new() -> Self {
        Self
    }
}

impl Scene for QuadsScene {
    fn size(&self) -> (f32, f32) {
        (800.0, 500.0)
    }

    fn frame(&mut self, f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        let mut root = div().relative().size_full().bg(hex(WHITE));
        for i in 0..BOXES {
            let (row, column) = (i / COLUMNS, i % COLUMNS);
            let hue = (i as u64 * 7 + f * 3) % 360;
            root = root.child(
                div()
                    .absolute()
                    .left(px(column as f32 * 20.0))
                    .top(px(row as f32 * 20.0))
                    .w(px(16.0))
                    .h(px(16.0))
                    .rounded(px(4.0))
                    .border(px(1.0))
                    .border_color(rgba(0x0000_0033))
                    .shadow(vec![BoxShadow {
                        color: rgba(0x0000_0040).into(),
                        offset: point(px(0.0), px(1.0)),
                        blur_radius: px(3.0),
                        spread_radius: px(0.0),
                        inset: false,
                    }])
                    .bg(hsl_deg(hue as u32, 0.65, 0.6)),
            );
        }
        root.into_any_element()
    }
}

register_scene!("PERF-QUADS", QuadsScene::new);
