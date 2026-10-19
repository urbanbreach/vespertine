//! PLT-001 A13 / PERF-PLT-001: the counters of the performance workload, measured on the real
//! platform.
//!
//! The example plays the scene PERF-PLT-001 in a window frame by frame (as the runner does) and
//! reads, after every frame, the composition counters and the process-wide GPU device and atlas
//! creation counters. The targets: in the unchanged phase (frames 30..329) no order
//! synchronization, no bounds write, no device and no atlas creation per frame; in the moving
//! phase (frames 330..389) exactly one bounds write per frame and no order synchronization,
//! device or atlas creation; after frame 0 no frame creates a device or an atlas (frame 0
//! creates one device and one atlas for the window; creating the overlay renderer shares them);
//! and the native child stacks below the overlay on every frame. It exits 0 when every target
//! holds.

#![cfg(any(target_os = "linux", target_os = "freebsd"))]

use vespertine::{
    App, AppContext as _, Bounds, Context, IntoElement, Render, Window, WindowOptions, px, size,
};
use vespertine_platform::application;
use vespertine_scenes::{Scene, create_scene};
use vespertine_wgpu::gpu_creation_counts;

const FRAMES: u64 = 420;

#[derive(Clone, Copy, Default)]
struct Sample {
    order_syncs: usize,
    bounds_writes: usize,
    failed_writes: usize,
    devices: usize,
    atlases: usize,
    native_below_overlay: bool,
}

struct Counters {
    scene: Box<dyn Scene>,
    frame: u64,
    /// The counters at the start of each frame's render; frame `f`'s effects are the
    /// difference to the sample of frame `f + 1`.
    samples: Vec<Sample>,
}

impl Counters {
    fn sample(window: &Window) -> Sample {
        let created = gpu_creation_counts();
        let Some(stats) = window.composition_stats() else {
            return Sample {
                devices: created.devices,
                atlases: created.atlases,
                ..Sample::default()
            };
        };
        let layout = window
            .composition()
            .expect("the composition is enabled")
            .layout();
        let overlay = layout.position(layout.overlay);
        let native_below_overlay = layout
            .entries
            .iter()
            .filter(|entry| entry.kind.is_attachment())
            .all(|entry| layout.position(entry.id) < overlay);
        Sample {
            order_syncs: stats.order_syncs,
            bounds_writes: stats.bounds_writes,
            failed_writes: stats.failed_writes,
            devices: created.devices,
            atlases: created.atlases,
            native_below_overlay,
        }
    }

    fn verify(&self) -> Vec<String> {
        let mut failures = Vec::new();
        let delta = |frame: u64| {
            let (before, after) = (
                self.samples[frame as usize],
                self.samples[frame as usize + 1],
            );
            (
                after.order_syncs - before.order_syncs,
                after.bounds_writes - before.bounds_writes,
                after.devices - before.devices,
                after.atlases - before.atlases,
            )
        };
        for frame in 1..FRAMES - 1 {
            let (syncs, writes, devices, atlases) = delta(frame);
            let expected_writes = usize::from((330..=389).contains(&frame));
            if syncs != 0 {
                failures.push(format!("frame {frame}: {syncs} order synchronizations"));
            }
            if frame >= 30 && writes != expected_writes {
                failures.push(format!(
                    "frame {frame}: {writes} bounds writes, expected {expected_writes}"
                ));
            }
            if devices != 0 || atlases != 0 {
                failures.push(format!(
                    "frame {frame}: {devices} device and {atlases} atlas creations"
                ));
            }
        }
        for (frame, sample) in self.samples.iter().enumerate().skip(2) {
            if !sample.native_below_overlay {
                failures.push(format!(
                    "frame {frame}: the native child is not below the overlay"
                ));
            }
            if sample.failed_writes != 0 {
                failures.push(format!("frame {frame}: a native write failed"));
            }
        }
        failures
    }
}

impl Render for Counters {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let frame = self.frame;
        self.frame += 1;
        self.samples.push(Self::sample(window));
        if frame == FRAMES {
            let failures = self.verify();
            let moving: usize = (330..=389)
                .map(|f| {
                    self.samples[f as usize + 1].bounds_writes
                        - self.samples[f as usize].bounds_writes
                })
                .sum();
            println!(
                "{} frames, {} bounds writes in the 60 moving frames",
                FRAMES, moving
            );
            if failures.is_empty() {
                println!("ok   every counter is at its target");
                std::process::exit(0);
            }
            for failure in &failures {
                println!("FAIL {failure}");
            }
            std::process::exit(1);
        }
        let element = self.scene.frame(frame, window, cx);
        window.request_animation_frame();
        element
    }
}

fn main() {
    application().run(|cx: &mut App| {
        let scene = create_scene("PERF-PLT-001").expect("PERF-PLT-001 is registered");
        let (width, height) = scene.size();
        let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(vespertine::WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|_| Counters {
                    scene,
                    frame: 0,
                    samples: Vec::new(),
                })
            },
        )
        .expect("open the window");
        cx.activate(true);
    });
}
