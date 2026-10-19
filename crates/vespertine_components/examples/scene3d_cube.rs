//! CMP-025 A1: a retained, interactive 3-D surface showing a supplied cube scene.
//!
//! The producer is a small software rasterizer. A test harness plays the part of an asynchronous
//! renderer: it takes the queued requests (up to three in flight), renders them on the
//! background executor and delivers the results in reverse order. While it does, a script orbits
//! the camera and resizes the pane. The example checks that the displayed frame never goes
//! backwards in sequence number, that the final displayed frame carries the final key and that
//! the camera is retained throughout; it prints `scene3d_cube: ok` and then stays interactive
//! (drag to orbit, wheel or pinch to zoom, hover a vertex). `SCENE3D_EXIT=1` exits afterwards.

use std::sync::Arc;
use std::time::{Duration, Instant};
use vespertine::{
    App, AppContext, Bounds, Context, ElementId, Entity, IntoElement, ParentElement, Render,
    RenderImage, Styled, Window, WindowBounds, WindowOptions, div, point, px, rgb, size,
};
use vespertine_components::scene3d::{
    Annotation, Camera, Delivery, Dispatch, Label, LabelColor, LabelSize, Palette, Quality,
    RenderRequest, RenderResult, RendererKind, SceneFrame, SceneProducer, SceneSurface,
    SceneSurfaces, SurfaceHandle,
};
use vespertine_platform::application;

struct Cube;

fn project(point3: [f32; 3], camera: Camera, size: (f32, f32)) -> (f32, f32, f32) {
    let (sy, cy) = camera.yaw.sin_cos();
    let (sp, cp) = camera.pitch.sin_cos();
    let [x, y, z] = point3;
    let (x, z) = (x * cy + z * sy, -x * sy + z * cy);
    let (y, z) = (y * cp - z * sp, y * sp + z * cp);
    let distance = 5.0;
    let perspective = distance / (distance - z);
    let scale = size.0.min(size.1) * 0.22 * camera.zoom * perspective;
    (size.0 / 2.0 + x * scale, size.1 / 2.0 - y * scale, z)
}

const FACES: [([usize; 4], u32); 6] = [
    ([0, 1, 2, 3], 0xE06C75),
    ([4, 5, 6, 7], 0x98C379),
    ([0, 1, 5, 4], 0xE5C07B),
    ([2, 3, 7, 6], 0x61AFEF),
    ([0, 3, 7, 4], 0xC678DD),
    ([1, 2, 6, 5], 0x56B6C2),
];

fn vertex(index: usize) -> [f32; 3] {
    // Order the corners so that consecutive face indices walk around each face.
    const ORDER: [(usize, usize, usize); 8] = [
        (0, 0, 0),
        (1, 0, 0),
        (1, 1, 0),
        (0, 1, 0),
        (0, 0, 1),
        (1, 0, 1),
        (1, 1, 1),
        (0, 1, 1),
    ];
    let (a, b, c) = ORDER[index];
    [
        a as f32 * 2.0 - 1.0,
        b as f32 * 2.0 - 1.0,
        c as f32 * 2.0 - 1.0,
    ]
}

impl SceneProducer for Cube {
    fn render(&self, request: &RenderRequest) -> Result<SceneFrame, String> {
        let (w, h) = request.key.pixel_size();
        let scale = request.key.scale;
        let samples = request.key.quality.samples_per_pixel((w, h));
        let axis = (samples as f32).sqrt() as u32;
        let camera = Camera::new(request.key.yaw, request.key.pitch, request.key.zoom);
        let logical = (w as f32 / scale, h as f32 / scale);
        let projected: Vec<(f32, f32, f32)> = (0..8)
            .map(|i| project(vertex(i), camera, logical))
            .collect();
        let mut faces: Vec<(f32, [usize; 4], u32)> = FACES
            .iter()
            .map(|(indices, color)| {
                let depth = indices.iter().map(|&i| projected[i].2).sum::<f32>() / 4.0;
                (depth, *indices, *color)
            })
            .collect();
        faces.sort_by(|a, b| a.0.total_cmp(&b.0));

        let mut accumulation = vec![[0.0f32; 4]; (w * h) as usize];
        for (_, indices, color) in &faces {
            let device: Vec<(f32, f32)> = indices
                .iter()
                .map(|&i| (projected[i].0 * scale, projected[i].1 * scale))
                .collect();
            let (min_x, max_x) = device.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| {
                (lo.min(p.0), hi.max(p.0))
            });
            let (min_y, max_y) = device.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| {
                (lo.min(p.1), hi.max(p.1))
            });
            let rgb = [
                (color >> 16 & 255) as f32 / 255.0,
                (color >> 8 & 255) as f32 / 255.0,
                (color & 255) as f32 / 255.0,
            ];
            for y in (min_y.floor().max(0.0) as u32)..(max_y.ceil().min(h as f32) as u32) {
                for x in (min_x.floor().max(0.0) as u32)..(max_x.ceil().min(w as f32) as u32) {
                    let mut covered = 0u32;
                    for sy in 0..axis {
                        for sx in 0..axis {
                            let p = (
                                x as f32 + (sx as f32 + 0.5) / axis as f32,
                                y as f32 + (sy as f32 + 0.5) / axis as f32,
                            );
                            if inside(&device, p) {
                                covered += 1;
                            }
                        }
                    }
                    if covered > 0 {
                        let a = covered as f32 / (axis * axis) as f32 * 0.9;
                        let px = &mut accumulation[(y * w + x) as usize];
                        // Source-over in straight alpha.
                        let out_a = a + px[3] * (1.0 - a);
                        for c in 0..3 {
                            px[c] = (rgb[c] * a + px[c] * px[3] * (1.0 - a)) / out_a;
                        }
                        px[3] = out_a;
                    }
                }
            }
        }
        let mut bytes = Vec::with_capacity((w * h * 4) as usize);
        for px in accumulation {
            bytes.extend_from_slice(&[
                (px[2] * 255.0) as u8,
                (px[1] * 255.0) as u8,
                (px[0] * 255.0) as u8,
                (px[3] * 255.0) as u8,
            ]);
        }
        let image = RenderImage::from_bgra(w, h, bytes).ok_or("empty frame")?;
        let annotations = (0..8)
            .map(|i| Annotation {
                position: point(px(projected[i].0), px(projected[i].1)),
                radius: 5.0,
                text: format!("vertex {i}").into(),
            })
            .collect();
        let top = projected.iter().map(|p| p.1).fold(f32::MAX, f32::min);
        Ok(SceneFrame {
            image: Arc::new(image),
            annotations,
            labels: vec![Label {
                position: point(px(logical.0 / 2.0), px((top - 6.0).max(14.0))),
                text: "cube".into(),
                color: LabelColor::Accent,
                size: LabelSize::Medium,
            }],
        })
    }
}

fn inside(polygon: &[(f32, f32)], p: (f32, f32)) -> bool {
    let mut sign = 0.0f32;
    for i in 0..polygon.len() {
        let (a, b) = (polygon[i], polygon[(i + 1) % polygon.len()]);
        let cross = (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0);
        if cross != 0.0 {
            if sign != 0.0 && cross.signum() != sign.signum() {
                return false;
            }
            sign = cross;
        }
    }
    true
}

struct Page {
    pane_width: f32,
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex_col()
            .flex()
            .gap_2()
            .p_2()
            .bg(rgb(0x181A1F))
            .text_color(rgb(0xF0F1F3))
            .child("Drag to orbit, wheel or pinch to zoom, hover a vertex")
            .child(
                div().w(px(self.pane_width)).child(
                    SceneSurface::new("cube", Cube)
                        .height(px(360.))
                        .renderer(RendererKind::Software)
                        .quality_interacting(Quality::Draft)
                        .max_in_flight(3)
                        .palette(Palette::neutral())
                        .dispatch(Dispatch::Manual),
                ),
            )
    }
}

fn fail(what: &str) -> ! {
    eprintln!("scene3d_cube: FAILED: {what}");
    std::process::exit(1)
}

/// Renders the queued requests on the background executor and delivers the results in reverse
/// order. Returns the displayed sequence after each delivery.
async fn serve(
    cx: &mut vespertine::AsyncApp,
    handle: &SurfaceHandle,
    window: vespertine::WindowId,
) -> Vec<Option<u64>> {
    let _ = window;
    let requests = handle.take_requests();
    let mut tasks = Vec::new();
    for request in requests {
        let task = cx.background_executor().spawn(async move {
            let started = Instant::now();
            let outcome = Cube.render(&request);
            (request, outcome, started.elapsed())
        });
        tasks.push(task);
    }
    let mut finished = Vec::new();
    for task in tasks {
        finished.push(task.await);
    }
    let mut observed = Vec::new();
    for (request, outcome, elapsed) in finished.into_iter().rev() {
        let result = match outcome {
            Ok(frame) => RenderResult::success(&request, frame, elapsed),
            Err(message) => RenderResult::failure(&request, message, elapsed),
        };
        let delivery = cx.update(|cx| handle.deliver(result, cx));
        observed.push(handle.displayed_sequence());
        if let Delivery::Discarded { .. } = delivery {
            // An out-of-order result was dropped and its image released.
        }
    }
    observed
}

fn main() {
    application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(760.), px(520.)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |_, cx| cx.new(|_| Page { pane_width: 640.0 }),
            )
            .unwrap();
        let page: Entity<Page> = window.entity(cx).unwrap();
        let window_id = window.window_id();
        cx.spawn(async move |cx| {
            let wait = |cx: &mut vespertine::AsyncApp, ms| {
                cx.background_executor().timer(Duration::from_millis(ms))
            };
            wait(cx, 400).await;
            let key = ElementId::from("cube");
            let Some(handle) = cx.update(|cx| SceneSurfaces::get(cx, window_id, &key)) else {
                fail("the surface was not created");
            };
            let mut last_sequence = 0u64;
            let mut expected = Camera::default();
            for step in 0..12 {
                // Orbit and resize while results are in flight.
                expected = Camera::new(expected.yaw + 0.35, expected.pitch + 0.05, expected.zoom);
                cx.update(|cx| handle.set_camera(expected, cx));
                page.update(cx, |page, cx| {
                    page.pane_width = if step % 2 == 0 { 560.0 } else { 640.0 };
                    cx.notify();
                });
                wait(cx, 120).await;
                for sequence in serve(cx, &handle, window_id).await.into_iter().flatten() {
                    if sequence < last_sequence {
                        fail("the displayed sequence went backwards");
                    }
                    last_sequence = sequence;
                }
                if handle.camera() != expected {
                    fail("the camera was not retained");
                }
            }
            // Settle: deliver until no request is outstanding.
            for _ in 0..20 {
                wait(cx, 100).await;
                serve(cx, &handle, window_id).await;
                if handle.in_flight() == 0 && handle.take_requests().is_empty() {
                    break;
                }
            }
            let displayed = handle
                .displayed_key()
                .unwrap_or_else(|| fail("no frame displayed"));
            let camera = handle.camera();
            if (displayed.yaw - camera.yaw).abs() > 1e-4
                || (displayed.pitch - camera.pitch).abs() > 1e-4
            {
                fail("the final displayed frame does not carry the final key");
            }
            println!(
                "scene3d_cube: ok (final sequence {})",
                handle.displayed_sequence().unwrap_or(0)
            );
            if std::env::var_os("SCENE3D_EXIT").is_some() {
                cx.update(|cx| cx.quit());
                return;
            }
            loop {
                wait(cx, 60).await;
                serve(cx, &handle, window_id).await;
            }
        })
        .detach();
        cx.activate(true);
    });
}
