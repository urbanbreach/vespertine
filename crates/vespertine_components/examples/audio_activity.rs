//! CMP-027 A1: the waveform driven by deterministic scalar samples, switching
//! Waiting -> Live -> Processing and then freezing all decorative motion with reduced motion;
//! an animated-geometry orb and a rolling label run alongside.

use std::time::{Duration, Instant};

use vespertine::{
    App, AppContext as _, Bounds, Context, Entity, IntoElement, ParentElement, Render, Styled,
    Window, WindowBounds, WindowOptions, div, hsla, px, rgb, size,
};
use vespertine_components::audio::{
    AnimatedGeometry, Dot, EnvelopeBuffer, Frame, FrameRequest, FrameSource, Line, Mode, Phase,
    WaveformColors, format_clock, roll_text, waveform,
};
use vespertine_platform::application;

/// A ring of dots breathing with the animation clock and the microphone level.
struct Orb;

impl FrameSource for Orb {
    fn generate(&mut self, request: &FrameRequest, frame: &mut Frame) {
        frame.clear();
        let t = request.time;
        for i in 0..24 {
            let angle = std::f32::consts::TAU * i as f32 / 24.0;
            let radius = 36.0 + 6.0 * (t * 2.0 + angle * 3.0).sin() * (0.4 + request.mic_level);
            frame.dots.push(Dot {
                x: 60.0 + radius * angle.cos(),
                y: 60.0 + radius * angle.sin(),
                radius: 2.0,
                ink: 0.1,
                alpha: 0.9,
            });
        }
        frame.lines.push(Line {
            x1: 20.0,
            y1: 60.0,
            x2: 100.0,
            y2: 60.0,
            width: 1.0,
            ink: 0.3,
            alpha: 0.4,
        });
    }
}

struct Demo {
    started: Instant,
    envelope: EnvelopeBuffer,
    orb: Entity<AnimatedGeometry>,
    models: [&'static str; 3],
}

impl Demo {
    fn phase(&self) -> (Mode, bool) {
        match self.started.elapsed().as_secs_f32() {
            t if t < 1.5 => (Mode::Waiting, true),
            t if t < 6.0 => (Mode::Live, true),
            t if t < 8.0 => (Mode::Processing, true),
            _ => (Mode::Processing, false),
        }
    }
}

impl Render for Demo {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        let (mode, motion) = self.phase();
        if mode == Mode::Live {
            // Deterministic samples: a slow swell with a pulse.
            let t = self.started.elapsed().as_secs_f32();
            let raw = 0.004 + 0.12 * (0.5 + 0.5 * (t * 3.1).sin()).powi(2);
            if !self.envelope.is_running() {
                self.envelope.start(now);
            }
            self.envelope.record(now, raw);
        } else if mode == Mode::Processing
            && !self.envelope.is_frozen()
            && self.envelope.is_running()
        {
            self.envelope.freeze(now);
        }
        // Reduced motion freezes every decorative animation.
        self.orb
            .update(cx, |orb, cx| orb.set_reduced_motion(!motion, cx));
        cx.set_reduce_motion(!motion);
        let bars = self.envelope.bars(now, motion);
        let model = self.models[(self.started.elapsed().as_secs() as usize / 2) % 3];
        if motion {
            window.request_animation_frame();
        }
        div()
            .size_full()
            .bg(rgb(0x14161A))
            .text_color(rgb(0xE6E8EB))
            .p(px(24.0))
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(div().child(format!(
                "{:?}  {}  motion {}",
                mode,
                format_clock(self.envelope.elapsed(now)),
                if motion { "on" } else { "reduced" }
            )))
            .child(div().w(px(520.0)).h(px(64.0)).child(waveform(
                bars,
                mode,
                WaveformColors {
                    ink: hsla(0.0, 0.0, 0.92, 1.0),
                    quiet: hsla(0.0, 0.0, 0.45, 1.0),
                },
                Phase::STEADY,
                motion,
            )))
            .child(div().w(px(120.0)).h(px(120.0)).child(self.orb.clone()))
            .child(
                div()
                    .w(px(160.0))
                    .child(roll_text(window, cx, "model", model, !motion)),
            )
    }
}

fn main() {
    application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(600.0), px(420.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| {
                let orb = cx.new(|cx| {
                    let mut orb = AnimatedGeometry::new(Orb, window, cx);
                    orb.set_theme(vespertine_components::audio::InkTheme::Dark, cx);
                    orb
                });
                cx.new(|_| Demo {
                    started: Instant::now(),
                    envelope: EnvelopeBuffer::new(),
                    orb,
                    models: ["Opus 5.5", "Opus 5.1", "Sonnet 5"],
                })
            },
        )
        .expect("open window");
        cx.activate(true);
        let _ = Duration::ZERO;
    });
}
