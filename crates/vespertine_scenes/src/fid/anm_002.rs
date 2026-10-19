//! FID-ANM-002-1 / FID-ANM-002-2: declarative motion tracks frozen at 500 ms.

use std::time::Duration;

use crate::common::{SANS, hex};
use crate::scene::{Scene, register_scene};
use vespertine::motion::{
    Easing, MotionDescription, MotionExt as _, MotionInitial, MotionProps, MotionTransition,
};
use vespertine::{AnyElement, App, Div, IntoElement, ParentElement, Styled, Window, div, px};

const FROZEN_AT: Duration = Duration::from_millis(500);

fn linear_one_second() -> MotionTransition {
    MotionTransition::new().duration(1.0).easing(Easing::LINEAR)
}

/// A width track from 0 to 100 over one second, frozen at 500 ms.
struct WidthTrack;

impl Scene for WidthTrack {
    fn size(&self) -> (f32, f32) {
        (240.0, 120.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        let description = MotionDescription::new(MotionProps::new().width(100.0))
            .initial(MotionInitial::Style(MotionProps::new().width(0.0)))
            .transition(linear_one_second());
        div()
            .size_full()
            .bg(hex(0xffffff))
            .child(
                div()
                    .absolute()
                    .left(px(20.0))
                    .top(px(40.0))
                    .h(px(40.0))
                    .rounded(px(4.0))
                    .bg(hex(0x3366cc))
                    .with_motion("width-track", description)
                    .frozen_at(FROZEN_AT),
            )
            .into_any_element()
    }
}

/// An opacity track from 0 to 1 over one second on the first card, frozen at 500 ms, next to a
/// static card.
struct OpacityTrack;

fn card(top: f32, fill: u32, label: &'static str) -> Div {
    div()
        .absolute()
        .left(px(280.0))
        .top(px(top))
        .w(px(240.0))
        .h(px(48.0))
        .pl(px(16.0))
        .pt(px(14.0))
        .rounded(px(8.0))
        .bg(hex(fill))
        .text_size(px(14.0))
        .line_height(px(20.0))
        .text_color(hex(0xf0f1f3))
        .whitespace_nowrap()
        .child(label)
}

impl Scene for OpacityTrack {
    fn size(&self) -> (f32, f32) {
        (800.0, 480.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        let description = MotionDescription::new(MotionProps::new().opacity(1.0))
            .initial(MotionInitial::Style(MotionProps::new().opacity(0.0)))
            .transition(linear_one_second());
        div()
            .size_full()
            .bg(hex(0x181a1f))
            .font_family(SANS)
            .child(
                card(216.0, 0x405470, "Moving input")
                    .with_motion("opacity-track", description)
                    .frozen_at(FROZEN_AT),
            )
            .child(card(300.0, 0x30343d, "Stationary"))
            .into_any_element()
    }
}

register_scene!("FID-ANM-002-1", || WidthTrack);
register_scene!("FID-ANM-002-2", || OpacityTrack);
