//! PERF-FLEX: deep flex re-layout.

use crate::scene::{Scene, register_scene};
use crate::common::{BORDER, SURFACE, WHITE, hex, hsl_deg};
use vespertine::{AnyElement, App, Div, IntoElement, ParentElement, Styled, Window, div, px};

/// Depth of the leaves below the root.
const LEAF_DEPTH: u32 = 6;
const BRANCHING: usize = 3;

pub struct FlexScene;

impl FlexScene {
    pub fn new() -> Self {
        Self
    }
}

/// Builds the node at `depth`; `next_leaf` is the running leaf index in depth-first order.
fn node(depth: u32, f: u64, next_leaf: &mut usize) -> Div {
    if depth == LEAF_DEPTH {
        let index = *next_leaf;
        *next_leaf += 1;
        return div()
            .flex_grow(1.0)
            .flex_basis(px(0.0))
            .min_w(px(2.0))
            .min_h(px(4.0))
            .bg(hsl_deg(((index * 11) % 360) as u32, 0.5, 0.7));
    }

    let mut container = div()
        .flex()
        .gap(px(2.0))
        .p(px(2.0))
        .border(px(1.0))
        .border_color(hex(BORDER))
        .bg(hex(if depth.is_multiple_of(2) { WHITE } else { SURFACE }));
    container = if depth.is_multiple_of(2) {
        container.flex_row()
    } else {
        container.flex_col()
    };
    for k in 0..BRANCHING {
        let grow = 1.0 + ((f + 10 * k as u64 + 3 * depth as u64) % 20) as f32 / 10.0;
        container = container.child(
            node(depth + 1, f, next_leaf)
                .flex_grow(grow)
                .flex_basis(px(0.0)),
        );
    }
    container
}

impl Scene for FlexScene {
    fn size(&self) -> (f32, f32) {
        (800.0, 500.0)
    }

    fn frame(&mut self, f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        let mut next_leaf = 0;
        node(0, f, &mut next_leaf)
            .size_full()
            .into_any_element()
    }
}

register_scene!("PERF-FLEX", FlexScene::new);
