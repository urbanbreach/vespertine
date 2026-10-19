//! PERF-PLT-001: layered composition around a native child (880 x 480 logical px, scale 2).
//!
//! A sidebar in the base plane, an independently presented native child between the base and the
//! overlay, and an overlay holding a backdrop, a dialog with shadow, its title and a button. A
//! framework label shows the frame index. The attachment's bounds are a pure function of the
//! frame index: constant for frames 0..329 (warm phase 0..29, unchanged phase 30..329), then one
//! move plus resize per frame for 60 frames, then constant again.

use crate::common::hex;
use crate::fid::plt_001::{NativeChild, dialog_overlay, overlay, text_box};
use crate::scene::{Scene, register_scene};
use vespertine::{AnyElement, App, Bounds, FontWeight, IntoElement, ParentElement, Pixels, Styled};
use vespertine::{Window, div, point, px, size};

const WIDTH: f32 = 880.0;
const HEIGHT: f32 = 480.0;
/// The last frame of the unchanged phase.
const LAST_STILL_FRAME: u64 = 329;
/// Frames of the moving phase.
const MOVING_FRAMES: u64 = 60;

/// The attachment bounds of frame `i`, in logical pixels (PERF-PLT-001).
fn attachment_bounds(i: u64) -> Bounds<Pixels> {
    let k = i.saturating_sub(LAST_STILL_FRAME).min(MOVING_FRAMES) as f32;
    Bounds {
        origin: point(px(180.0 + k), px(40.0)),
        size: size(px(520.0 - k), px(340.0)),
    }
}

struct PerfPlt001 {
    native: NativeChild,
}

impl Scene for PerfPlt001 {
    fn size(&self) -> (f32, f32) {
        (WIDTH, HEIGHT)
    }

    fn frame(&mut self, f: u64, window: &mut Window, _cx: &mut App) -> AnyElement {
        // Frame 0 enables composition and creates the native producer.
        self.native.prepare(window);
        let child = attachment_bounds(f);
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
                    .h(px(HEIGHT))
                    .bg(hex(0xdde2ea)),
            )
            .child(self.native.slot(child))
            .child(overlay(dialog_overlay(WIDTH, HEIGHT)))
            .child(text_box(
                format!("Frame {f}"),
                20.0,
                440.0,
                14.0,
                FontWeight::NORMAL,
                hex(0x202020),
            ))
            .into_any_element()
    }
}

register_scene!("PERF-PLT-001", || PerfPlt001 {
    native: NativeChild::new()
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_bounds_follow_the_per_frame_function() {
        let still = attachment_bounds(0);
        assert_eq!(still, attachment_bounds(329));
        assert_eq!(f32::from(still.origin.x), 180.0);
        assert_eq!(f32::from(still.size.width), 520.0);
        let moving = attachment_bounds(330);
        assert_eq!(f32::from(moving.origin.x), 181.0);
        assert_eq!(f32::from(moving.size.width), 519.0);
        // The right edge stays at 700 for the whole moving phase.
        for i in 330..=389 {
            let bounds = attachment_bounds(i);
            assert_eq!(f32::from(bounds.origin.x + bounds.size.width), 700.0);
        }
        let last = attachment_bounds(389);
        assert_eq!(f32::from(last.origin.x), 240.0);
        assert_eq!(f32::from(last.size.width), 460.0);
        assert_eq!(last, attachment_bounds(10_000));
    }
}
