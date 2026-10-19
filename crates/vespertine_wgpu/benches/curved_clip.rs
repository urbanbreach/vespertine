//! Manual GPU benchmark of curved overflow clipping (RND-002 A12, PERF-RND-002).
//!
//! Compares the PERF-RND-002 scene with the opt-in flag honored (`curved_overflow_clip`) against
//! the same scene with rectangular overflow clipping only, through the production painting path
//! and the headless wgpu renderer. For each configuration it reports CPU time per frame
//! (building the element tree, painting, encoding and submitting), GPU completion time, render
//! pass count, geometry upload bytes and heap allocations, and asserts that opting in adds no
//! render pass, no offscreen target and no curved-clip allocation beyond 48 bytes of shape data
//! (plus a 4-byte chain link) per active shape.
//!
//! ```sh
//! cargo bench -p vespertine_wgpu --features bench-support --bench curved_clip
//! ```
//!
//! GPU time: the benchmark device does not request timestamp queries (only dual-source
//! blending), so "GPU completion" is the wall time from the end of submission until the
//! device reports the frame complete; it is a lower bound of the frame's GPU time that is
//! only comparable between the two configurations of one run.
//!
//! [`perf_rnd_002_content`] is the scene content as a pure function of the frame index, for
//! reuse by the performance scene.

use std::{
    alloc::System,
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};
use vespertine::{
    AnyElement, App, Context, HeadlessAppContext, IntoElement, ParentElement, PathBuilder,
    PlatformAtlas, PlatformHeadlessRenderer, Render, Styled, Window, canvas, div, point, px, rgb,
    size,
};
use vespertine_bench_metrics::{CountingAllocator, allocation_stats};
use vespertine_wgpu::{ClipStats, CosmicTextSystem, RecordStats, WgpuHeadlessRenderer};

#[global_allocator]
static ALLOCATOR: CountingAllocator<System> = CountingAllocator::new(System);

const WARMUP_FRAMES: u32 = 30;
const MEASURED_FRAMES: u32 = 300;
/// Window size in logical pixels; the window runs at scale factor 2.
const WINDOW: (f32, f32) = (880.0, 480.0);

/// PERF-RND-002 content for frame `frame`: 100 panels in a 10 x 10 grid, each with two partly
/// outside quads, a label, a stroked path and a nested box with a moving bar. With `curved`
/// every panel and nested box opts into curved overflow clipping, otherwise they keep
/// rectangular overflow masking only.
pub fn perf_rnd_002_content(frame: u32, curved: bool) -> AnyElement {
    let triangle = |t: u32| if t <= 16 { t } else { 32 - t };
    let mut root = div()
        .size_full()
        .bg(rgb(0x1e2128))
        .font_family("Noto Sans")
        .text_size(px(14.0));
    for k in 0..100u32 {
        let (row, col) = (k / 10, k % 10);
        // Odd panels slide horizontally from frame 30 on.
        let shift = if frame >= 30 && k % 2 == 1 {
            (triangle(frame % 32) as f32 - 8.0) / 2.0
        } else {
            0.0
        };
        let origin = (4.0 + 88.0 * col as f32 + shift, 4.0 + 48.0 * row as f32);
        // Children are positioned relative to the panel's border box; absolute insets
        // start at the padding box, one border width in.
        let at = |x: f32, y: f32| (px(x - 1.0), px(y - 1.0));
        let panel = div()
            .absolute()
            .left(px(origin.0))
            .top(px(origin.1))
            .w(px(80.0))
            .h(px(40.0))
            .bg(rgb(0x2b3040))
            .rounded(px(10.0))
            .border(px(1.0))
            .border_color(rgb(0x4a5168))
            .overflow_hidden();
        let panel = if curved {
            panel.curved_overflow_clip()
        } else {
            panel
        };
        let (left, top) = at(-10.0, 8.0);
        let (right_left, right_top) = at(50.0, 20.0);
        let (label_left, label_top) = at(6.0, 2.0);
        let (box_left, box_top) = at(8.0, 18.0);
        let bar_left = ((3 * frame) % 64) as f32 - 24.0;
        let nested = div()
            .absolute()
            .left(box_left)
            .top(box_top)
            .w(px(64.0))
            .h(px(16.0))
            .bg(rgb(0x3a4058))
            .rounded(px(6.0))
            .overflow_hidden();
        let nested = if curved {
            nested.curved_overflow_clip()
        } else {
            nested
        };
        root = root.child(
            panel
                .child(
                    div()
                        .absolute()
                        .left(left)
                        .top(top)
                        .w(px(70.0))
                        .h(px(14.0))
                        .bg(rgb(0x3e8ede)),
                )
                .child(
                    div()
                        .absolute()
                        .left(right_left)
                        .top(right_top)
                        .w(px(40.0))
                        .h(px(30.0))
                        .bg(rgb(0xde7a3e)),
                )
                .child(
                    div()
                        .absolute()
                        .left(label_left)
                        .top(label_top)
                        .line_height(px(18.0))
                        .text_color(rgb(0xffffff))
                        .child(format!("Panel {k:02}")),
                )
                .child(
                    canvas(
                        |_, _, _| (),
                        |bounds, _, window, _| {
                            // The padding box starts one border width inside the panel.
                            let panel = bounds.origin - point(px(1.0), px(1.0));
                            let mut path = PathBuilder::stroke(px(2.0));
                            for (index, (x, y)) in [
                                (4.0, 36.0),
                                (20.0, 24.0),
                                (36.0, 36.0),
                                (52.0, 24.0),
                                (68.0, 36.0),
                            ]
                            .into_iter()
                            .enumerate()
                            {
                                let vertex = panel + point(px(x), px(y));
                                if index == 0 {
                                    path.move_to(vertex);
                                } else {
                                    path.line_to(vertex);
                                }
                            }
                            if let Ok(path) = path.build() {
                                window.paint_path(path, rgb(0x8fd16a));
                            }
                        },
                    )
                    .absolute()
                    .size_full(),
                )
                .child(
                    nested.child(
                        div()
                            .absolute()
                            .left(px(bar_left))
                            .top(px(0.0))
                            .w(px(24.0))
                            .h(px(16.0))
                            .bg(rgb(0xe5c84a)),
                    ),
                ),
        );
    }
    root.into_any_element()
}

struct Scene {
    frame: Rc<Cell<u32>>,
    curved: bool,
}

impl Render for Scene {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        perf_rnd_002_content(self.frame.get(), self.curved)
    }
}

/// A headless renderer shared between the window and the benchmark, so that the benchmark can
/// read its counters and wait for the GPU.
struct SharedRenderer(Rc<RefCell<WgpuHeadlessRenderer>>);

impl PlatformHeadlessRenderer for SharedRenderer {
    fn render_scene_to_image(
        &mut self,
        scene: &vespertine::Scene,
        size: vespertine::Size<vespertine::DevicePixels>,
    ) -> anyhow::Result<image::RgbaImage> {
        // The benchmark presents frames through `Window::render_to_image`: render the frame
        // exactly as presenting does (no read-back) and hand back a placeholder image.
        self.0.borrow_mut().render_scene(scene, size)?;
        Ok(image::RgbaImage::new(1, 1))
    }

    fn render_scene(
        &mut self,
        scene: &vespertine::Scene,
        size: vespertine::Size<vespertine::DevicePixels>,
    ) -> anyhow::Result<()> {
        self.0.borrow_mut().render_scene(scene, size)
    }

    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.0.borrow().sprite_atlas()
    }
}

#[derive(Debug)]
struct Report {
    cpu: Duration,
    gpu: Duration,
    counters: RecordStats,
    clip: ClipStats,
    allocations: u64,
    allocated_bytes: u64,
}

fn run(curved: bool) -> Report {
    let renderer = Rc::new(RefCell::new(
        WgpuHeadlessRenderer::new().expect("a GPU adapter is required"),
    ));
    let text_system = CosmicTextSystem::new("Noto Sans");
    let factory_renderer = renderer.clone();
    let mut cx =
        HeadlessAppContext::with_platform(Arc::new(text_system), Arc::new(()), move || {
            Ok(Some(Box::new(SharedRenderer(factory_renderer.clone()))
                as Box<dyn PlatformHeadlessRenderer>))
        });
    let frame = Rc::new(Cell::new(0));
    let window = {
        let frame = frame.clone();
        cx.open_window(size(px(WINDOW.0), px(WINDOW.1)), move |_, cx| {
            use vespertine::AppContext as _;
            cx.new(|_| Scene { frame, curved })
        })
        .expect("open window")
    };

    let draw = |cx: &mut HeadlessAppContext, index: u32| {
        frame.set(index);
        cx.update_window(window.into(), |_, window: &mut Window, cx: &mut App| {
            window.refresh();
            window.draw(cx).clear(cx);
            window.render_to_image().expect("render the frame");
        })
        .expect("draw frame");
    };
    for index in 0..WARMUP_FRAMES {
        draw(&mut cx, index);
    }
    renderer.borrow().wait_for_gpu().expect("wait for the GPU");
    renderer.borrow_mut().reset_record_stats();

    let (mut cpu, mut gpu) = (Duration::ZERO, Duration::ZERO);
    let before = allocation_stats().expect("the counting allocator is installed");
    for index in WARMUP_FRAMES..WARMUP_FRAMES + MEASURED_FRAMES {
        let start = Instant::now();
        draw(&mut cx, index);
        let submitted = Instant::now();
        renderer.borrow().wait_for_gpu().expect("wait for the GPU");
        cpu += submitted - start;
        gpu += submitted.elapsed();
    }
    let after = allocation_stats().expect("the counting allocator is installed");
    let counters = renderer.borrow().record_stats();
    let clip = renderer.borrow().clip_stats();
    Report {
        cpu: cpu / MEASURED_FRAMES,
        gpu: gpu / MEASURED_FRAMES,
        counters,
        clip,
        allocations: (after.allocations - before.allocations) / u64::from(MEASURED_FRAMES),
        allocated_bytes: (after.allocated_bytes - before.allocated_bytes)
            / u64::from(MEASURED_FRAMES),
    }
}

fn main() {
    // Run each configuration twice, alternating, and keep the faster CPU run: the machine is
    // shared and a single run is noisy.
    let mut rectangular = run(false);
    let mut curved = run(true);
    for _ in 0..1 {
        let again = run(false);
        if again.cpu < rectangular.cpu {
            rectangular = again;
        }
        let again = run(true);
        if again.cpu < curved.cpu {
            curved = again;
        }
    }

    let frames = u64::from(MEASURED_FRAMES);
    println!("PERF-RND-002, {MEASURED_FRAMES} frames after {WARMUP_FRAMES} warmup frames");
    println!("{:<28} {:>14} {:>14}", "", "rectangular", "curved (opt-in)");
    let row = |label: &str, a: String, b: String| println!("{label:<28} {a:>14} {b:>14}");
    row(
        "CPU time / frame",
        format!("{:.3} ms", rectangular.cpu.as_secs_f64() * 1e3),
        format!("{:.3} ms", curved.cpu.as_secs_f64() * 1e3),
    );
    row(
        "GPU completion / frame",
        format!("{:.3} ms", rectangular.gpu.as_secs_f64() * 1e3),
        format!("{:.3} ms", curved.gpu.as_secs_f64() * 1e3),
    );
    row(
        "render passes / frame",
        (rectangular.counters.render_passes / frames).to_string(),
        (curved.counters.render_passes / frames).to_string(),
    );
    row(
        "instance upload B / frame",
        (rectangular.counters.instance_bytes / frames).to_string(),
        (curved.counters.instance_bytes / frames).to_string(),
    );
    row(
        "curved clip upload B / frame",
        (rectangular.clip.upload_bytes / frames).to_string(),
        (curved.clip.upload_bytes / frames).to_string(),
    );
    row(
        "curved clip allocations",
        rectangular.clip.buffer_allocations.to_string(),
        curved.clip.buffer_allocations.to_string(),
    );
    row(
        "path intermediate allocs",
        rectangular
            .counters
            .path_intermediate_allocations
            .to_string(),
        curved.counters.path_intermediate_allocations.to_string(),
    );
    row(
        "heap allocations / frame",
        rectangular.allocations.to_string(),
        curved.allocations.to_string(),
    );
    row(
        "heap bytes / frame",
        rectangular.allocated_bytes.to_string(),
        curved.allocated_bytes.to_string(),
    );
    println!(
        "CPU ratio curved / rectangular: {:.3} (PERF-RND-002 target 1.00 to 1.25)",
        curved.cpu.as_secs_f64() / rectangular.cpu.as_secs_f64()
    );
    println!(
        "GPU time: timestamp queries are not enabled on the benchmark device; GPU completion \
         is wall time from submission end until the device reports idle."
    );

    // No extra pass and no offscreen target for opting in.
    assert_eq!(
        curved.counters.render_passes, rectangular.counters.render_passes,
        "opting in must not add render passes"
    );
    assert_eq!(
        curved.counters.path_intermediate_allocations,
        rectangular.counters.path_intermediate_allocations,
        "opting in must not add offscreen targets"
    );
    // A scene without opted-in elements allocates and uploads no curved-clip data.
    assert_eq!(rectangular.clip.buffer_allocations, 0);
    assert_eq!(rectangular.clip.upload_bytes, 0);
    // 48 bytes per shape plus the 4-byte chain link, at most 200 shapes (100 panels and 100
    // nested boxes); the buffers are allocated once and reused.
    let per_frame = curved.clip.upload_bytes / frames;
    assert!(
        per_frame > 0 && per_frame <= 200 * 52,
        "clip upload {per_frame} B per frame"
    );
    assert_eq!(per_frame % 52, 0);
    assert!(curved.clip.buffer_allocations <= 1);
}
