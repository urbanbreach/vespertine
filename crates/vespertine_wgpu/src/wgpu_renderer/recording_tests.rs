//! GPU tests of the recording path (PRF-007) and the bounded-resource rules (PRF-010).

use super::blur_tests::{color, device_size, fill, rect, render};
use super::*;
use vespertine::{ContentMask, Path, PlatformHeadlessRenderer, point, px};

fn triangle(
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    hex: u32,
    mask: Bounds<ScaledPixels>,
) -> Path<ScaledPixels> {
    let mut path = Path::new(point(px(x), px(y)));
    path.line_to(point(px(x + width), px(y)));
    path.line_to(point(px(x + width / 2.0), px(y + height)));
    path.color = color(hex).into();
    let mut path = path.scale(1.0);
    path.content_mask = ContentMask { bounds: mask };
    path
}

/// One paint layer per cell so cells (quad then path) alternate in paint order. Cells are
/// `pitch` apart; paths are `width` wide.
fn cell_scene(count: usize, pitch: f32, width: f32, mask: Option<Bounds<ScaledPixels>>) -> Scene {
    let mut scene = Scene::default();
    let everything = rect(0.0, 0.0, 4000.0, 200.0);
    for i in 0..count {
        let x = 4.0 + pitch * i as f32;
        scene.push_layer(everything);
        fill(
            &mut scene,
            rect(x, 10.0, width + 4.0, 80.0),
            color(0xF6F8FA),
        );
        scene.insert_primitive(triangle(
            x + 2.0,
            20.0,
            width,
            60.0,
            0x2563EB,
            mask.unwrap_or(everything),
        ));
        scene.pop_layer();
    }
    scene
}

fn counters(renderer: &WgpuHeadlessRenderer) -> RecordStats {
    renderer.record_stats()
}

fn render_counted(
    renderer: &mut WgpuHeadlessRenderer,
    scene: Scene,
    width: i32,
    height: i32,
) -> (image::RgbaImage, RecordStats) {
    let before = counters(renderer);
    let image = render(renderer, scene, width, height);
    let after = counters(renderer);
    let delta = RecordStats {
        frames: after.frames - before.frames,
        instance_writes: after.instance_writes - before.instance_writes,
        instance_bytes: after.instance_bytes - before.instance_bytes,
        binding_creations: after.binding_creations - before.binding_creations,
        pipeline_sets: after.pipeline_sets - before.pipeline_sets,
        bind_group_sets: after.bind_group_sets - before.bind_group_sets,
        render_passes: after.render_passes - before.render_passes,
        path_rasterization_passes: after.path_rasterization_passes
            - before.path_rasterization_passes,
        global_uploads: after.global_uploads - before.global_uploads,
        path_intermediate_allocations: after.path_intermediate_allocations
            - before.path_intermediate_allocations,
        instance_buffer_replacements: after.instance_buffer_replacements
            - before.instance_buffer_replacements,
    };
    (image, delta)
}

#[test]
fn grouped_path_rasterization_matches_the_per_batch_baseline() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let scenes: Vec<(&str, Box<dyn Fn() -> Scene>)> = vec![
        ("separated", Box::new(|| cell_scene(6, 44.0, 36.0, None))),
        ("overlapping", Box::new(|| cell_scene(6, 8.0, 60.0, None))),
        (
            "masked",
            Box::new(|| cell_scene(6, 44.0, 36.0, Some(rect(20.0, 30.0, 150.0, 30.0)))),
        ),
        ("many", Box::new(|| cell_scene(70, 14.0, 8.0, None))),
    ];
    for (name, build) in scenes {
        renderer.set_path_group_limit(1);
        let baseline = render(&mut renderer, build(), 1000, 100);
        renderer.set_path_group_limit(recording::MAX_PATH_GROUP);
        let grouped = render(&mut renderer, build(), 1000, 100);
        assert_eq!(baseline, grouped, "scene {name}");
        assert!(
            grouped
                .pixels()
                .any(|pixel| pixel.0[2] > 200 && pixel.0[0] < 100),
            "scene {name} should show the path color"
        );
    }
    Ok(())
}

#[test]
fn separated_path_batches_share_one_rasterization_pass() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let (_, stats) = render_counted(&mut renderer, cell_scene(6, 44.0, 36.0, None), 400, 100);
    assert_eq!(stats.path_rasterization_passes, 1);
    assert_eq!(
        stats.render_passes, 2,
        "one rasterization pass and the main pass"
    );

    let (_, stats) = render_counted(&mut renderer, cell_scene(6, 8.0, 60.0, None), 400, 100);
    assert_eq!(stats.path_rasterization_passes, 6);
    assert_eq!(stats.render_passes, 12);
    Ok(())
}

#[test]
fn a_hundred_thirty_separated_batches_form_three_groups() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let (_, stats) = render_counted(&mut renderer, cell_scene(130, 12.0, 6.0, None), 1700, 100);
    assert_eq!(stats.path_rasterization_passes, 3);
    Ok(())
}

#[test]
fn a_frame_uploads_its_instance_data_once_and_globals_only_when_changed() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let scene = || {
        let mut scene = Scene::default();
        fill(&mut scene, rect(0.0, 0.0, 64.0, 64.0), color(0x336699));
        fill(&mut scene, rect(8.0, 8.0, 20.0, 20.0), color(0xFF0000));
        scene
    };
    let (_, first) = render_counted(&mut renderer, scene(), 64, 64);
    assert_eq!(first.instance_writes, 1);
    assert!(first.global_uploads >= 2);
    let (_, second) = render_counted(&mut renderer, scene(), 64, 64);
    assert_eq!(second.instance_writes, 1);
    assert_eq!(
        second.global_uploads, 0,
        "identical frames upload no globals"
    );
    assert_eq!(second.binding_creations, 0, "bindings are reused");
    let (_, resized) = render_counted(&mut renderer, scene(), 80, 64);
    assert_eq!(resized.global_uploads, 1);
    Ok(())
}

#[test]
fn unused_bindings_are_released_after_two_frames() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let mut scene = Scene::default();
    fill(&mut scene, rect(0.0, 0.0, 16.0, 16.0), color(0x336699));
    render(&mut renderer, scene, 16, 16);
    assert_eq!(renderer.cached_bindings(), 1);
    render(&mut renderer, Scene::default(), 16, 16);
    render(&mut renderer, Scene::default(), 16, 16);
    assert_eq!(
        renderer.cached_bindings(),
        1,
        "still within the two-frame window"
    );
    render(&mut renderer, Scene::default(), 16, 16);
    assert_eq!(renderer.cached_bindings(), 0);
    Ok(())
}

/// Quads that need `bytes` of packed instance data, each 1x1 on a distinct pixel.
fn quad_scene(bytes: u64, side: u32) -> Scene {
    let count = bytes.div_ceil(std::mem::size_of::<Quad>() as u64) as u32;
    let mut scene = Scene::default();
    for i in 0..count {
        fill(
            &mut scene,
            rect((i % side) as f32, ((i / side) % side) as f32, 1.0, 1.0),
            color(0x336699),
        );
    }
    scene
}

#[test]
fn growth_replaces_the_buffer_without_aliasing_old_bindings() -> anyhow::Result<()> {
    let build = |extra: bool| {
        let mut scene = Scene::default();
        fill(&mut scene, rect(0.0, 0.0, 64.0, 64.0), color(0x112233));
        fill(&mut scene, rect(4.0, 4.0, 30.0, 30.0), color(0xCC8844));
        if extra {
            for i in 0..20_000_u32 {
                fill(
                    &mut scene,
                    rect((i % 60) as f32, ((i / 60) % 60) as f32, 1.0, 1.0),
                    color(0x88CC44),
                );
            }
        }
        scene
    };
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let small = render(&mut renderer, build(false), 64, 64);
    let generation = renderer.instance_buffer_generation();
    let large = render(&mut renderer, build(true), 64, 64);
    assert!(
        renderer.instance_buffer_generation() > generation,
        "the buffer was grown"
    );
    assert!(renderer.instance_capacity() > 2 * 1024 * 1024);
    let mut fresh = WgpuHeadlessRenderer::new()?;
    assert_eq!(render(&mut fresh, build(true), 64, 64), large);
    // Back to the small scene on the grown buffer: equal offsets in two buffers never alias.
    assert_eq!(render(&mut renderer, build(false), 64, 64), small);
    Ok(())
}

#[test]
fn path_intermediates_are_created_on_demand_and_released_after_inactivity() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    assert!(!renderer.has_path_intermediates());
    let free = || {
        let mut scene = Scene::default();
        fill(&mut scene, rect(0.0, 0.0, 32.0, 32.0), color(0x336699));
        scene
    };
    render(&mut renderer, free(), 32, 32);
    assert!(
        !renderer.has_path_intermediates(),
        "path-free frames allocate none"
    );
    assert_eq!(renderer.record_stats().path_intermediate_allocations, 0);

    let pathed = || cell_scene(1, 10.0, 20.0, None);
    render(&mut renderer, pathed(), 100, 100);
    assert!(renderer.has_path_intermediates());
    assert_eq!(renderer.record_stats().path_intermediate_allocations, 1);
    // A cached intermediate of the same size is reused.
    render(&mut renderer, pathed(), 100, 100);
    assert_eq!(renderer.record_stats().path_intermediate_allocations, 1);

    for _ in 0..29 {
        render(&mut renderer, free(), 100, 100);
    }
    assert!(
        renderer.has_path_intermediates(),
        "the 29th path-free frame retains them"
    );
    render(&mut renderer, free(), 100, 100);
    assert!(!renderer.has_path_intermediates(), "the 30th releases them");

    // A path frame recreates them and resets the counter.
    render(&mut renderer, pathed(), 100, 100);
    assert!(renderer.has_path_intermediates());
    assert_eq!(renderer.inactivity_counters().0, 0);
    for _ in 0..10 {
        render(&mut renderer, free(), 100, 100);
    }
    render(&mut renderer, pathed(), 100, 100);
    for _ in 0..29 {
        render(&mut renderer, free(), 100, 100);
    }
    assert!(renderer.has_path_intermediates());

    // Resizing does not allocate until the next path frame.
    let allocations = renderer.record_stats().path_intermediate_allocations;
    render(&mut renderer, free(), 120, 100);
    assert_eq!(
        renderer.record_stats().path_intermediate_allocations,
        allocations
    );
    render(&mut renderer, cell_scene(1, 10.0, 20.0, None), 120, 100);
    assert_eq!(
        renderer.record_stats().path_intermediate_allocations,
        allocations + 1
    );
    Ok(())
}

#[test]
fn parking_after_one_path_free_frame_releases_path_intermediates() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    render(&mut renderer, cell_scene(1, 10.0, 20.0, None), 100, 100);
    renderer.trim();
    assert!(
        renderer.has_path_intermediates(),
        "the last frame had paths"
    );
    let mut free = Scene::default();
    fill(&mut free, rect(0.0, 0.0, 10.0, 10.0), color(0x336699));
    let image = render(&mut renderer, free, 100, 100);
    renderer.trim();
    assert!(!renderer.has_path_intermediates());
    assert_eq!(
        renderer.instance_capacity(),
        2 * 1024 * 1024,
        "trim never touches the instance buffer"
    );
    // A redraw after trim renders identical pixels.
    let mut again = Scene::default();
    fill(&mut again, rect(0.0, 0.0, 10.0, 10.0), color(0x336699));
    assert_eq!(render(&mut renderer, again, 100, 100), image);
    Ok(())
}

fn small_frame(renderer: &mut WgpuHeadlessRenderer) -> anyhow::Result<()> {
    let mut scene = Scene::default();
    fill(&mut scene, rect(0.0, 0.0, 8.0, 8.0), color(0x336699));
    scene.finish();
    renderer.render_scene(&scene, device_size(8, 8))
}

fn large_frame(renderer: &mut WgpuHeadlessRenderer, bytes: u64) -> anyhow::Result<()> {
    let mut scene = quad_scene(bytes, 400);
    scene.finish();
    renderer.render_scene(&scene, device_size(400, 400))
}

const MIB: u64 = 1024 * 1024;

#[test]
fn instance_buffer_shrinks_after_sustained_low_usage_and_never_below_the_floor()
-> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    assert_eq!(renderer.instance_capacity(), 2 * MIB);
    large_frame(&mut renderer, 6 * MIB)?;
    assert_eq!(renderer.instance_capacity(), 8 * MIB);

    for _ in 0..119 {
        small_frame(&mut renderer)?;
    }
    assert_eq!(renderer.instance_capacity(), 8 * MIB);
    small_frame(&mut renderer)?;
    assert_eq!(renderer.instance_capacity(), 4 * MIB);
    assert_eq!(
        renderer.inactivity_counters().1,
        0,
        "the streak resets after a shrink"
    );

    for _ in 0..120 {
        small_frame(&mut renderer)?;
    }
    assert_eq!(renderer.instance_capacity(), 2 * MIB);
    for _ in 0..200 {
        small_frame(&mut renderer)?;
    }
    assert_eq!(
        renderer.instance_capacity(),
        2 * MIB,
        "never below the floor"
    );
    assert_eq!(
        renderer.inactivity_counters().1,
        0,
        "frames at the floor reset the count"
    );
    Ok(())
}

#[test]
fn a_frame_above_half_capacity_restarts_the_shrink_streak() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    large_frame(&mut renderer, 6 * MIB)?;
    assert_eq!(renderer.instance_capacity(), 8 * MIB);
    for _ in 0..119 {
        small_frame(&mut renderer)?;
    }
    // 5 MiB is above half of the 8 MiB capacity.
    large_frame(&mut renderer, 5 * MIB)?;
    assert_eq!(renderer.instance_capacity(), 8 * MIB);
    for _ in 0..119 {
        small_frame(&mut renderer)?;
    }
    assert_eq!(renderer.instance_capacity(), 8 * MIB);
    small_frame(&mut renderer)?;
    assert_eq!(renderer.instance_capacity(), 4 * MIB);
    Ok(())
}

#[test]
fn frames_after_a_shrink_render_identical_pixels() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let scene = || {
        let mut scene = Scene::default();
        fill(&mut scene, rect(0.0, 0.0, 32.0, 32.0), color(0x224466));
        fill(&mut scene, rect(5.0, 5.0, 12.0, 12.0), color(0xDD9933));
        scene
    };
    let before = render(&mut renderer, scene(), 32, 32);
    large_frame(&mut renderer, 6 * MIB)?;
    for _ in 0..240 {
        small_frame(&mut renderer)?;
    }
    assert_eq!(renderer.instance_capacity(), 2 * MIB);
    assert_eq!(render(&mut renderer, scene(), 32, 32), before);
    Ok(())
}

#[test]
fn memory_reports_follow_the_injected_clock_and_list_every_component() -> anyhow::Result<()> {
    use crate::memory_report::MemoryReporter;
    use std::{
        cell::RefCell,
        rc::Rc,
        sync::Mutex,
        time::{Duration, Instant},
    };

    let now = Arc::new(Mutex::new(Instant::now()));
    let clock_now = now.clone();
    let clock: crate::memory_report::Clock = Arc::new(move || *clock_now.lock().unwrap());
    let lines = Rc::new(RefCell::new(Vec::<String>::new()));
    let reporter = |enabled: bool| {
        let sink = lines.clone();
        MemoryReporter::with_parts(
            enabled,
            clock.clone(),
            Box::new(move |line| sink.borrow_mut().push(line.to_string())),
        )
    };
    let frame = |renderer: &mut WgpuHeadlessRenderer| {
        let mut scene = Scene::default();
        fill(&mut scene, rect(0.0, 0.0, 32.0, 32.0), color(0x336699));
        render(renderer, scene, 32, 32);
    };

    let mut renderer = WgpuHeadlessRenderer::new()?;
    renderer.set_memory_reporter(reporter(false));
    frame(&mut renderer);
    assert!(lines.borrow().is_empty(), "disabled reporters stay silent");

    renderer.set_memory_reporter(reporter(true));
    frame(&mut renderer);
    assert_eq!(lines.borrow().len(), 1, "the first completed frame reports");
    *now.lock().unwrap() += Duration::from_millis(9_900);
    frame(&mut renderer);
    assert_eq!(lines.borrow().len(), 1);
    *now.lock().unwrap() += Duration::from_millis(100);
    frame(&mut renderer);
    assert_eq!(lines.borrow().len(), 2);
    let line = lines.borrow()[1].clone();
    assert!(line.starts_with(crate::memory_report::REPORT_PREFIX));
    assert!(line.contains("instance 1 buffers [2.0 MiB]"), "{line}");
    assert!(line.contains("backdrop 0.0 MiB"), "{line}");
    assert!(line.contains("path 0.0 MiB"), "{line}");
    assert!(line.contains("atlas 0 textures 0.0 MiB"), "{line}");

    // A second renderer keeps its own timer.
    let mut other = WgpuHeadlessRenderer::new()?;
    other.set_memory_reporter(reporter(true));
    let before = lines.borrow().len();
    frame(&mut other);
    assert_eq!(lines.borrow().len(), before + 1);
    Ok(())
}
