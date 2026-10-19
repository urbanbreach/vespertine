//! LAY-005 A1: every side and alignment of a floating layer around a trigger.
//!
//! Twelve layers are attached to one trigger. After the first painted frame the example compares,
//! for each layer, the recorded automation bounds (`FloatingLayer::bounds_tracker`), the bounds
//! the content actually painted at, and the bounds the pure placement function computes, and
//! prints one line per layer. With `--check` it exits (status 1 on any mismatch) after reporting.

#[path = "example_support/fonts.rs"]
mod example_support;

use std::{cell::Cell, rc::Rc};

use vespertine::{
    App, Bounds, BoundsTracker, Context, FloatingAlign, FloatingOptions, FloatingSide, Pixels,
    Window, WindowOptions, canvas, div, floating_layer, hsla, place_floating, point, prelude::*,
    px, rgb, size,
};
use vespertine_platform::application;

const SIDES: [(FloatingSide, &str); 4] = [
    (FloatingSide::Top, "top"),
    (FloatingSide::Right, "right"),
    (FloatingSide::Bottom, "bottom"),
    (FloatingSide::Left, "left"),
];
const ALIGNS: [(FloatingAlign, &str); 3] = [
    (FloatingAlign::Start, "start"),
    (FloatingAlign::Center, "center"),
    (FloatingAlign::End, "end"),
];

struct Layer {
    side: FloatingSide,
    align: FloatingAlign,
    label: String,
    recorded: BoundsTracker,
    painted: Rc<Cell<Option<Bounds<Pixels>>>>,
}

struct FloatingLayers {
    layers: Vec<Layer>,
    check_only: bool,
    report_scheduled: bool,
}

const TRIGGER: Bounds<Pixels> = Bounds {
    origin: point(px(270.), px(220.)),
    size: size(px(100.), px(40.)),
};
const CONTENT: vespertine::Size<Pixels> = size(px(56.), px(28.));
const GAP: Pixels = px(6.);

impl FloatingLayers {
    fn new(check_only: bool) -> Self {
        let layers = SIDES
            .iter()
            .flat_map(|(side, side_name)| {
                ALIGNS.iter().map(move |(align, align_name)| Layer {
                    side: *side,
                    align: *align,
                    label: format!("{side_name}/{align_name}"),
                    recorded: BoundsTracker::new(),
                    painted: Rc::new(Cell::new(None)),
                })
            })
            .collect();
        Self {
            layers,
            check_only,
            report_scheduled: false,
        }
    }

    /// Prints one line per layer and returns whether every layer matched.
    fn report(&self, window: &Window) -> bool {
        let viewport = window.viewport_size();
        let mut all_match = true;
        for layer in &self.layers {
            let options = FloatingOptions {
                side: layer.side,
                align: layer.align,
                gap: GAP,
                ..Default::default()
            };
            let computed = place_floating(&options, TRIGGER, None, CONTENT, viewport, px(0.));
            let recorded = layer.recorded.get();
            let painted = layer.painted.get();
            let matches = recorded == Some(computed) && painted == recorded;
            all_match &= matches;
            println!(
                "{:<14} recorded={recorded:?} painted={painted:?} computed={computed:?} {}",
                layer.label,
                if matches { "OK" } else { "MISMATCH" }
            );
        }
        all_match
    }
}

impl Render for FloatingLayers {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.report_scheduled {
            self.report_scheduled = true;
            let this = cx.entity();
            window.on_next_frame(move |window, cx| {
                this.update(cx, |this, cx| {
                    let ok = this.report(window);
                    if this.check_only {
                        cx.quit();
                        if !ok {
                            std::process::exit(1);
                        }
                    }
                });
            });
        }

        div().size_full().bg(rgb(0xFFFFFF)).child(
            div()
                .absolute()
                .left(TRIGGER.origin.x)
                .top(TRIGGER.origin.y)
                .w(TRIGGER.size.width)
                .h(TRIGGER.size.height)
                .bg(rgb(0x808080))
                .children(self.layers.iter().enumerate().map(|(index, layer)| {
                    let painted = layer.painted.clone();
                    floating_layer(("layer", index))
                        .side(layer.side)
                        .align(layer.align)
                        .gap(GAP)
                        .bounds_tracker(&layer.recorded)
                        .w(CONTENT.width)
                        .h(CONTENT.height)
                        .bg(hsla(index as f32 / 12., 0.6, 0.5, 1.))
                        .child(
                            canvas(
                                |_, _, _| (),
                                move |bounds, _, _, _| painted.set(Some(bounds)),
                            )
                            .size_full(),
                        )
                })),
        )
    }
}

fn main() {
    let check_only = std::env::args().any(|arg| arg == "--check");
    application().run(move |cx: &mut App| {
        if !example_support::load_fonts(cx) {
            return;
        }
        cx.open_window(
            WindowOptions {
                window_bounds: Some(vespertine::WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(640.), px(480.)),
                    cx,
                ))),
                ..Default::default()
            },
            move |_, cx| cx.new(|_| FloatingLayers::new(check_only)),
        )
        .unwrap();
        cx.activate(true);
    });
}
