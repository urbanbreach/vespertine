//! GPU tests of the ordered backdrop blur (RND-003) and its storage rules (PRF-009).

use super::*;
use vespertine::{
    BackdropBlur, BorderStyle, ContentMask, Corners, Edges, Hsla, PlatformHeadlessRenderer, Quad,
    Shadow, rgb,
};

pub(super) fn device_size(width: i32, height: i32) -> Size<DevicePixels> {
    Size {
        width: DevicePixels(width),
        height: DevicePixels(height),
    }
}

pub(super) fn rect(x: f32, y: f32, width: f32, height: f32) -> Bounds<ScaledPixels> {
    Bounds {
        origin: Point {
            x: ScaledPixels(x),
            y: ScaledPixels(y),
        },
        size: Size {
            width: ScaledPixels(width),
            height: ScaledPixels(height),
        },
    }
}

pub(super) fn color(hex: u32) -> Hsla {
    Hsla::from(rgb(hex))
}

pub(super) fn quad(
    bounds: Bounds<ScaledPixels>,
    color: Hsla,
    radii: Corners<ScaledPixels>,
) -> Quad {
    Quad {
        order: 0,
        border_style: BorderStyle::Solid,
        bounds,
        content_mask: ContentMask {
            bounds: rect(-1e6, -1e6, 2e6, 2e6),
        },
        background: color.into(),
        border_color: color,
        corner_radii: radii,
        border_widths: Edges::default(),
        scope: Default::default(),
    }
}

pub(super) fn fill(scene: &mut Scene, bounds: Bounds<ScaledPixels>, color: Hsla) {
    scene.insert_primitive(quad(bounds, color, Corners::default()));
}

/// Mirrors `Window::paint_backdrop_blur`: an invisible ordering primitive then the effect, with
/// a fully transparent tint after it so the effect is never trailing.
pub(super) fn blur(
    scene: &mut Scene,
    bounds: Bounds<ScaledPixels>,
    radii: Corners<ScaledPixels>,
    sigma: f32,
    mask: Option<Bounds<ScaledPixels>>,
) {
    let content_mask = ContentMask {
        bounds: mask.unwrap_or(rect(-1e6, -1e6, 2e6, 2e6)),
    };
    scene.insert_backdrop_blur(BackdropBlur {
        order: 0,
        blur_radius: ScaledPixels(sigma),
        bounds,
        content_mask,
        corner_radii: radii,
        opacity: 1.0,
    });
    let mut tint = quad(bounds, Hsla::transparent_black(), radii);
    tint.content_mask = content_mask;
    scene.insert_primitive(tint);
}

pub(super) fn render(
    renderer: &mut WgpuHeadlessRenderer,
    mut scene: Scene,
    width: i32,
    height: i32,
) -> image::RgbaImage {
    scene.finish();
    renderer
        .render_scene_to_image(&scene, device_size(width, height))
        .expect("scene should render")
}

pub(super) fn checkerboard(scene: &mut Scene, width: f32, height: f32, cell: f32) {
    fill(scene, rect(0.0, 0.0, width, height), color(0x000000));
    let mut y = 0.0;
    let mut row = 0;
    while y < height {
        let mut x = 0.0;
        let mut column = 0;
        while x < width {
            if (row + column) % 2 == 0 {
                fill(scene, rect(x, y, cell, cell), color(0xFFFFFF));
            }
            x += cell;
            column += 1;
        }
        y += cell;
        row += 1;
    }
}

fn channel_diff(a: &image::Rgba<u8>, b: &image::Rgba<u8>) -> u8 {
    a.0.iter()
        .zip(b.0.iter())
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap_or(0)
}

/// Signed distance of the pixel center to the rounded rectangle (RND-003 R10).
fn rounded_distance(
    x: u32,
    y: u32,
    bounds: Bounds<ScaledPixels>,
    radii: Corners<ScaledPixels>,
) -> f32 {
    let half = [bounds.size.width.0 / 2.0, bounds.size.height.0 / 2.0];
    let center = [bounds.origin.x.0 + half[0], bounds.origin.y.0 + half[1]];
    let p = [x as f32 + 0.5 - center[0], y as f32 + 0.5 - center[1]];
    let q = match (p[0] < 0.0, p[1] < 0.0) {
        (true, true) => radii.top_left.0,
        (false, true) => radii.top_right.0,
        (false, false) => radii.bottom_right.0,
        (true, false) => radii.bottom_left.0,
    };
    let e = [p[0].abs() - half[0] + q, p[1].abs() - half[1] + q];
    let outside = (e[0].max(0.0).powi(2) + e[1].max(0.0).powi(2)).sqrt();
    outside + e[0].max(e[1]).min(0.0) - q
}

#[test]
fn blur_snapshots_earlier_content_and_leaves_later_content_sharp() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let build = |with_effect: bool| {
        let mut scene = Scene::default();
        checkerboard(&mut scene, 200.0, 120.0, 10.0);
        if with_effect {
            blur(
                &mut scene,
                rect(40.0, 20.0, 100.0, 80.0),
                Corners::default(),
                6.0,
                None,
            );
        }
        fill(&mut scene, rect(60.0, 40.0, 20.0, 20.0), color(0xE34D59));
        scene
    };
    let reference = render(&mut renderer, build(false), 200, 120);
    let image = render(&mut renderer, build(true), 200, 120);

    // The foreground rectangle painted after the effect stays sharp.
    for (x, y) in [(61, 41), (70, 50), (78, 58)] {
        assert!(channel_diff(image.get_pixel(x, y), reference.get_pixel(x, y)) <= 2);
    }
    // The checkerboard inside the effect is blurred toward its mean.
    for (x, y) in [(100, 85), (120, 30), (95, 60), (45, 25)] {
        let value = i32::from(image.get_pixel(x, y).0[0]);
        assert!(
            (value - 127).abs() < 60,
            "pixel ({x}, {y}) should be blurred, was {value}"
        );
    }
    // Outside the effect nothing changes.
    for y in 0..120 {
        for x in 0..200 {
            let inside = (40..140).contains(&x) && (20..100).contains(&y);
            if !inside {
                assert_eq!(image.get_pixel(x, y), reference.get_pixel(x, y));
            }
        }
    }
    Ok(())
}

#[test]
fn constant_input_stays_constant_including_at_every_edge() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    for sigma in [1.4, 11.0, 20.0, 40.0] {
        let mut scene = Scene::default();
        fill(&mut scene, rect(0.0, 0.0, 150.0, 100.0), color(0x4D80B3));
        blur(
            &mut scene,
            rect(0.0, 0.0, 150.0, 100.0),
            Corners::default(),
            sigma,
            None,
        );
        let mut reference_scene = Scene::default();
        fill(
            &mut reference_scene,
            rect(0.0, 0.0, 150.0, 100.0),
            color(0x4D80B3),
        );
        let reference = render(&mut renderer, reference_scene, 150, 100);
        let image = render(&mut renderer, scene, 150, 100);
        for (x, y, pixel) in image.enumerate_pixels() {
            assert!(
                channel_diff(pixel, reference.get_pixel(x, y)) <= 2,
                "sigma {sigma}: pixel ({x}, {y}) is {pixel:?}, expected {:?}",
                reference.get_pixel(x, y)
            );
        }
    }
    Ok(())
}

#[test]
fn linear_ramp_keeps_its_position_for_non_divisible_snapshots() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let (width, height) = (1279, 799);
    let build = |with_effect: bool| {
        let mut scene = Scene::default();
        for x in 0..width {
            let level = (x as f32 * 255.0 / (width - 1) as f32).round() as u32;
            fill(
                &mut scene,
                rect(x as f32, 0.0, 1.0, height as f32),
                color(level * 0x010101),
            );
        }
        if with_effect {
            blur(
                &mut scene,
                rect(200.0, 100.0, 900.0, 600.0),
                Corners::default(),
                32.0,
                None,
            );
        }
        scene
    };
    let reference = render(&mut renderer, build(false), width, height);
    let image = render(&mut renderer, build(true), width, height);
    for x in 200..1100 {
        let blurred = i32::from(image.get_pixel(x, 400).0[0]);
        let original = i32::from(reference.get_pixel(x, 400).0[0]);
        assert!(
            (blurred - original).abs() <= 3,
            "x {x}: blurred {blurred}, ramp {original}"
        );
    }
    Ok(())
}

#[test]
fn rounded_edge_is_a_hard_cutoff_and_mask_clips_the_effect() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let bounds = rect(30.0, 20.0, 100.0, 60.0);
    let radii = Corners {
        top_left: ScaledPixels(20.0),
        top_right: ScaledPixels(0.0),
        bottom_right: ScaledPixels(10.0),
        bottom_left: ScaledPixels(30.0),
    };
    let mask = rect(50.0, 10.0, 60.0, 80.0);
    let build = |with_effect: bool| {
        let mut scene = Scene::default();
        checkerboard(&mut scene, 160.0, 100.0, 7.0);
        if with_effect {
            blur(&mut scene, bounds, radii, 3.0, Some(mask));
        }
        scene
    };
    let reference = render(&mut renderer, build(false), 160, 100);
    let image = render(&mut renderer, build(true), 160, 100);
    let mut inside_total = 0;
    let mut inside_changed = 0;
    for y in 0..100 {
        for x in 0..160 {
            let distance = rounded_distance(x, y, bounds, radii);
            let in_mask = (50..110).contains(&x) && (10..90).contains(&y);
            let same = image.get_pixel(x, y) == reference.get_pixel(x, y);
            if distance > 0.0 || !in_mask {
                assert!(same, "pixel ({x}, {y}) changed outside the effect");
            } else if distance < -3.0 && x > 55 && x < 105 {
                inside_total += 1;
                inside_changed += usize::from(!same);
            }
        }
    }
    assert!(inside_total > 500);
    assert!(
        inside_changed * 100 >= inside_total * 95,
        "{inside_changed} of {inside_total} interior pixels changed"
    );
    Ok(())
}

#[test]
fn four_distinct_corner_radii_produce_four_distinct_corners() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let bounds = rect(10.0, 10.0, 100.0, 100.0);
    let radii = Corners {
        top_left: ScaledPixels(4.0),
        top_right: ScaledPixels(24.0),
        bottom_right: ScaledPixels(40.0),
        bottom_left: ScaledPixels(12.0),
    };
    let build = |with_effect: bool| {
        let mut scene = Scene::default();
        checkerboard(&mut scene, 120.0, 120.0, 5.0);
        if with_effect {
            blur(&mut scene, bounds, radii, 2.0, None);
        }
        scene
    };
    let reference = render(&mut renderer, build(false), 120, 120);
    let image = render(&mut renderer, build(true), 120, 120);
    let corner_unchanged = |x0: u32, y0: u32, flip_x: bool, flip_y: bool| {
        let mut count = 0;
        for dy in 0..40 {
            for dx in 0..40 {
                let x = if flip_x { x0 - dx } else { x0 + dx };
                let y = if flip_y { y0 - dy } else { y0 + dy };
                count += usize::from(image.get_pixel(x, y) == reference.get_pixel(x, y));
            }
        }
        count
    };
    let counts = [
        corner_unchanged(10, 10, false, false),
        corner_unchanged(109, 10, true, false),
        corner_unchanged(109, 109, true, true),
        corner_unchanged(10, 109, false, true),
    ];
    // Larger radii cut away more of the corner window, so the unchanged area grows with it.
    assert!(counts[0] < counts[3], "{counts:?}");
    assert!(counts[3] < counts[1], "{counts:?}");
    assert!(counts[1] < counts[2], "{counts:?}");
    Ok(())
}

#[test]
fn overlapping_effects_chain_and_each_uses_its_own_sigma() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let build = |first: bool, second: bool| {
        let mut scene = Scene::default();
        checkerboard(&mut scene, 200.0, 120.0, 8.0);
        if first {
            blur(
                &mut scene,
                rect(10.0, 10.0, 100.0, 90.0),
                Corners::default(),
                5.0,
                None,
            );
        }
        if second {
            blur(
                &mut scene,
                rect(60.0, 30.0, 120.0, 80.0),
                Corners::default(),
                2.0,
                None,
            );
        }
        scene
    };
    let first_only = render(&mut renderer, build(true, false), 200, 120);
    let second_only = render(&mut renderer, build(false, true), 200, 120);
    let both = render(&mut renderer, build(true, true), 200, 120);
    // Inside the overlap the second blur's input already contains the first blur.
    let mut differing = 0;
    for y in 40..95 {
        for x in 75..105 {
            differing +=
                usize::from(channel_diff(both.get_pixel(x, y), second_only.get_pixel(x, y)) > 1);
        }
    }
    assert!(differing > 0, "second blur must see the first blur");
    // Outside the overlap each region matches its own single-effect render.
    assert_eq!(both.get_pixel(20, 20), first_only.get_pixel(20, 20));
    assert_eq!(both.get_pixel(170, 100), second_only.get_pixel(170, 100));
    Ok(())
}

#[test]
fn only_the_first_thirty_two_effects_render() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let cells: Vec<(f32, f32)> = (0..33)
        .map(|k| (4.0 + 26.0 * (k % 11) as f32, 4.0 + 26.0 * (k / 11) as f32))
        .collect();
    let build = |with_effects: bool| {
        let mut scene = Scene::default();
        checkerboard(&mut scene, 300.0, 100.0, 5.0);
        if with_effects {
            for (x, y) in &cells {
                blur(
                    &mut scene,
                    rect(*x, *y, 20.0, 20.0),
                    Corners::default(),
                    2.0,
                    None,
                );
            }
        }
        scene
    };
    let reference = render(&mut renderer, build(false), 300, 100);
    let image = render(&mut renderer, build(true), 300, 100);
    for (k, (x, y)) in cells.iter().enumerate() {
        let mut changed = 0;
        for dy in 0..20 {
            for dx in 0..20 {
                let (px, py) = (*x as u32 + dx, *y as u32 + dy);
                changed += usize::from(image.get_pixel(px, py) != reference.get_pixel(px, py));
            }
        }
        if k < 32 {
            assert!(
                changed > 100,
                "effect {k} should render ({changed} changed)"
            );
        } else {
            assert_eq!(changed, 0, "effect 33 must not alter the image");
        }
    }
    let stats = renderer.blur_stats();
    assert_eq!(stats.effects_rendered, 32);
    assert_eq!(stats.effects_skipped, 1);
    Ok(())
}

#[test]
fn a_fully_clipped_effect_does_not_damage_earlier_pixels() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let build = |with_effect: bool| {
        let mut scene = Scene::default();
        checkerboard(&mut scene, 100.0, 80.0, 5.0);
        if with_effect {
            // Entirely outside the drawable, and one clipped away by its own mask.
            blur(
                &mut scene,
                rect(500.0, 10.0, 50.0, 50.0),
                Corners::default(),
                4.0,
                None,
            );
            blur(
                &mut scene,
                rect(10.0, 10.0, 40.0, 40.0),
                Corners::default(),
                4.0,
                Some(rect(70.0, 70.0, 5.0, 5.0)),
            );
        }
        scene
    };
    let reference = render(&mut renderer, build(false), 100, 80);
    let image = render(&mut renderer, build(true), 100, 80);
    assert_eq!(image, reference);
    Ok(())
}

#[test]
fn direct_and_intermediate_presentation_routes_match_and_keep_alpha() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    renderer.set_clear_color([0.0, 0.0, 0.0, 0.0]);
    let build = || {
        let mut scene = Scene::default();
        let mut translucent = quad(
            rect(0.0, 0.0, 120.0, 80.0),
            color(0x80C0FF),
            Corners::default(),
        );
        translucent.background = color(0x80C0FF).opacity(0.5).into();
        scene.insert_primitive(translucent);
        fill(&mut scene, rect(10.0, 10.0, 30.0, 30.0), color(0xFF0000));
        blur(
            &mut scene,
            rect(20.0, 20.0, 80.0, 50.0),
            Corners::default(),
            5.0,
            None,
        );
        scene
    };
    let direct = render(&mut renderer, build(), 120, 80);
    let direct_stats = renderer.blur_resources();
    assert!(
        !direct_stats.intermediate,
        "no full-frame intermediate when the target is copyable"
    );
    renderer.set_force_presentation_intermediate(true);
    let through = render(&mut renderer, build(), 120, 80);
    assert!(renderer.blur_resources().intermediate);
    assert_eq!(direct, through);
    // Transparent target alpha survives both routes.
    let alpha = direct.get_pixel(110, 75).0[3];
    assert!((100..160).contains(&alpha), "alpha was {alpha}");
    assert_eq!(direct.get_pixel(119, 79).0[3], alpha);
    Ok(())
}

#[test]
fn effect_free_frames_do_not_use_the_presentation_intermediate() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    renderer.set_force_presentation_intermediate(true);
    let mut scene = Scene::default();
    fill(&mut scene, rect(0.0, 0.0, 50.0, 50.0), color(0x336699));
    render(&mut renderer, scene, 50, 50);
    assert!(!renderer.blur_resources().intermediate);
    assert_eq!(renderer.blur_stats().intermediate_allocations, 0);
    Ok(())
}

#[test]
fn sigma_beyond_the_table_uses_the_full_support_fallback() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    // Left half white, right half black; sigma 400 gives an effective sigma of 100 (r = 300).
    let (width, height) = (600, 100);
    let mut scene = Scene::default();
    fill(&mut scene, rect(0.0, 0.0, 300.0, 100.0), color(0xFFFFFF));
    fill(&mut scene, rect(300.0, 0.0, 300.0, 100.0), color(0x000000));
    blur(
        &mut scene,
        rect(0.0, 0.0, 600.0, 100.0),
        Corners::default(),
        400.0,
        None,
    );
    let image = render(&mut renderer, scene, width, height);

    // Reference: clamp-to-edge Gaussian of the step with sigma 400 px.
    let signal: Vec<f64> = (0..width)
        .map(|x| if x < 300 { 255.0 } else { 0.0 })
        .collect();
    let sigma = 400.0_f64;
    let reference = |x: i64| {
        let mut sum = 0.0;
        let mut norm = 0.0;
        for j in -1600_i64..=1600 {
            let weight = (-(j * j) as f64 / (2.0 * sigma * sigma)).exp();
            let at = (x + j).clamp(0, width as i64 - 1) as usize;
            sum += weight * signal[at];
            norm += weight;
        }
        sum / norm
    };
    for x in [10, 150, 300, 450, 590] {
        let expected = reference(x);
        let actual = f64::from(image.get_pixel(x as u32, 50).0[0]);
        assert!(
            (actual - expected).abs() < 6.0,
            "x {x}: expected {expected:.1}, got {actual}"
        );
    }
    Ok(())
}

#[test]
fn blur_storage_is_reused_replaced_and_released() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let frame = |renderer: &mut WgpuHeadlessRenderer, bounds: Bounds<ScaledPixels>, sigma: f32| {
        let mut scene = Scene::default();
        checkerboard(&mut scene, 800.0, 600.0, 10.0);
        blur(&mut scene, bounds, Corners::default(), sigma, None);
        render(renderer, scene, 800, 600)
    };
    frame(&mut renderer, rect(100.0, 100.0, 100.0, 50.0), 8.0);
    let first = renderer
        .blur_resources()
        .set
        .expect("a set exists after an effect");
    assert_eq!((first.width, first.height, first.downsample), (256, 256, 1));
    assert_eq!(renderer.blur_stats().set_allocations, 1);

    // A different region at the same downsampling factor fits the same set.
    frame(&mut renderer, rect(300.0, 200.0, 120.0, 60.0), 8.0);
    assert_eq!(renderer.blur_stats().set_allocations, 1);

    // A change of downsampling factor replaces it.
    frame(&mut renderer, rect(300.0, 200.0, 120.0, 60.0), 16.0);
    assert_eq!(renderer.blur_stats().set_allocations, 2);
    assert_eq!(renderer.blur_resources().set.unwrap().downsample, 2);

    // An envelope larger than the allocation replaces it, clamped to the drawable.
    frame(&mut renderer, rect(10.0, 10.0, 780.0, 580.0), 16.0);
    assert_eq!(renderer.blur_stats().set_allocations, 3);
    let large = renderer.blur_resources().set.unwrap();
    assert_eq!((large.width, large.height), (800, 600));

    // 29 effect-free frames retain the storage, the 30th releases it, and a listed but
    // invisible effect keeps resetting the counter.
    let free_frame = |renderer: &mut WgpuHeadlessRenderer| {
        let mut scene = Scene::default();
        fill(&mut scene, rect(0.0, 0.0, 800.0, 600.0), color(0x223344));
        render(renderer, scene, 800, 600);
    };
    for _ in 0..29 {
        free_frame(&mut renderer);
    }
    assert!(renderer.blur_resources().set.is_some());
    assert_eq!(renderer.blur_resources().effect_free_frames, 29);
    let mut hidden = Scene::default();
    fill(&mut hidden, rect(0.0, 0.0, 800.0, 600.0), color(0x223344));
    blur(
        &mut hidden,
        rect(5000.0, 0.0, 50.0, 50.0),
        Corners::default(),
        4.0,
        None,
    );
    render(&mut renderer, hidden, 800, 600);
    assert_eq!(renderer.blur_resources().effect_free_frames, 0);
    for _ in 0..29 {
        free_frame(&mut renderer);
    }
    assert!(renderer.blur_resources().set.is_some());
    free_frame(&mut renderer);
    let state = renderer.blur_resources();
    assert!(state.set.is_none());
    assert!(!state.intermediate);
    assert_eq!(state.kernels, 0);
    assert_eq!(state.retained_bytes, 0);

    // A later effect recreates what it needs.
    frame(&mut renderer, rect(100.0, 100.0, 100.0, 50.0), 8.0);
    assert!(renderer.blur_resources().set.is_some());
    Ok(())
}

#[test]
fn a_reused_larger_allocation_produces_the_same_image_as_a_fresh_one() -> anyhow::Result<()> {
    let scene = || {
        let mut scene = Scene::default();
        checkerboard(&mut scene, 700.0, 500.0, 9.0);
        blur(
            &mut scene,
            rect(520.0, 380.0, 160.0, 100.0),
            Corners::default(),
            8.0,
            None,
        );
        scene
    };
    let mut fresh = WgpuHeadlessRenderer::new()?;
    let expected = render(&mut fresh, scene(), 700, 500);

    let mut reused = WgpuHeadlessRenderer::new()?;
    let mut big = Scene::default();
    checkerboard(&mut big, 700.0, 500.0, 9.0);
    blur(
        &mut big,
        rect(20.0, 20.0, 600.0, 450.0),
        Corners::default(),
        8.0,
        None,
    );
    render(&mut reused, big, 700, 500);
    let big_set = reused.blur_resources().set.unwrap();
    let actual = render(&mut reused, scene(), 700, 500);
    assert_eq!(reused.blur_resources().set.unwrap(), big_set);
    assert_eq!(reused.blur_stats().set_allocations, 1);
    for (x, y, pixel) in actual.enumerate_pixels() {
        assert!(
            channel_diff(pixel, expected.get_pixel(x, y)) <= 1,
            "pixel ({x}, {y})"
        );
    }
    Ok(())
}

#[test]
fn narrow_regions_shade_fewer_texels_than_full_targets() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let mut scene = Scene::default();
    checkerboard(&mut scene, 900.0, 500.0, 16.0);
    blur(
        &mut scene,
        rect(100.0, 200.0, 700.0, 24.0),
        Corners::default(),
        16.0,
        None,
    );
    render(&mut renderer, scene, 900, 500);
    let stats = renderer.blur_stats();
    assert_eq!(stats.effects_rendered, 1);
    assert!(stats.shaded_texels > 0);
    assert!(stats.shaded_texels < stats.full_target_texels);
    Ok(())
}

#[test]
fn trim_releases_blur_storage_only_after_an_effect_free_frame() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let mut with_effect = Scene::default();
    checkerboard(&mut with_effect, 200.0, 100.0, 10.0);
    blur(
        &mut with_effect,
        rect(20.0, 20.0, 100.0, 50.0),
        Corners::default(),
        8.0,
        None,
    );
    render(&mut renderer, with_effect, 200, 100);
    renderer.trim();
    assert!(
        renderer.blur_resources().set.is_some(),
        "an effect frame keeps its set for a redraw"
    );
    let mut free = Scene::default();
    fill(&mut free, rect(0.0, 0.0, 200.0, 100.0), color(0x123456));
    render(&mut renderer, free, 200, 100);
    renderer.trim();
    let state = renderer.blur_resources();
    assert!(state.set.is_none() && !state.intermediate && state.kernels == 0);
    Ok(())
}

/// The effect and its ordering primitive without any following tint.
fn trailing_blur(scene: &mut Scene, bounds: Bounds<ScaledPixels>, sigma: f32) {
    let content_mask = ContentMask {
        bounds: rect(-1e6, -1e6, 2e6, 2e6),
    };
    scene.insert_backdrop_blur(BackdropBlur {
        order: 0,
        blur_radius: ScaledPixels(sigma),
        bounds,
        content_mask,
        corner_radii: Corners::default(),
        opacity: 1.0,
    });
}

#[test]
fn boundary_primitive_alone_is_invisible_and_a_trailing_effect_still_renders() -> anyhow::Result<()>
{
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let base = || {
        let mut scene = Scene::default();
        checkerboard(&mut scene, 100.0, 80.0, 5.0);
        scene
    };
    let reference = render(&mut renderer, base(), 100, 80);

    let mut boundary_only = base();
    let bounds = rect(10.0, 10.0, 60.0, 40.0);
    boundary_only.insert_primitive(Shadow {
        order: 0,
        blur_radius: ScaledPixels(0.0),
        bounds,
        corner_radii: Corners::default(),
        content_mask: ContentMask { bounds },
        color: Hsla::transparent_black(),
        element_bounds: bounds,
        element_corner_radii: Corners::default(),
        inset: 0,
        curved_clip: 0,
    });
    assert_eq!(render(&mut renderer, boundary_only, 100, 80), reference);

    let mut trailing = base();
    trailing_blur(&mut trailing, bounds, 4.0);
    assert_ne!(
        render(&mut renderer, trailing, 100, 80),
        reference,
        "the effect's own ordering primitive is a batch at its order, so it is processed"
    );

    let mut followed = base();
    blur(&mut followed, bounds, Corners::default(), 4.0, None);
    assert_ne!(render(&mut renderer, followed, 100, 80), reference);
    Ok(())
}

/// A floating card painted in one layer: blur, tint, border-like quads and "text" quads.
fn card(scene: &mut Scene) {
    let bounds = rect(30.0, 20.0, 120.0, 80.0);
    scene.push_layer(bounds);
    blur(scene, bounds, Corners::default(), 6.0, None);
    scene.insert_primitive(quad(
        bounds,
        color(0x20242C).opacity(0.8),
        Corners::default(),
    ));
    for i in 0..6 {
        fill(
            scene,
            rect(40.0 + 8.0 * i as f32, 30.0, 4.0, 12.0),
            color(0xF0F1F3),
        );
    }
    scene.pop_layer();
}

fn card_scene(hover_first: Option<Bounds<ScaledPixels>>) -> Scene {
    let mut scene = Scene::default();
    checkerboard(&mut scene, 200.0, 120.0, 6.0);
    if let Some(hover) = hover_first {
        fill(&mut scene, hover, color(0x00AA55));
    }
    card(&mut scene);
    scene
}

#[test]
fn the_card_is_pixel_identical_after_an_unrelated_repaint() -> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let first = render(&mut renderer, card_scene(None), 200, 120);
    // An unrelated region repainted before the card, and one overlapping its edge, raise the
    // card layer's order but never change the relative order of its blur and contents.
    for hover in [rect(170.0, 10.0, 20.0, 20.0), rect(10.0, 10.0, 40.0, 30.0)] {
        let second = render(&mut renderer, card_scene(Some(hover)), 200, 120);
        // The blur is below the tint and the "text": the text columns stay sharp.
        for i in 0..6 {
            let x = 40 + 8 * i + 2;
            assert_eq!(second.get_pixel(x, 36).0[..3], [0xF0, 0xF1, 0xF3]);
            assert_eq!(first.get_pixel(x, 36).0[..3], [0xF0, 0xF1, 0xF3]);
        }
        // Where the hover rectangle does not influence the blur, the card is identical.
        for y in 60..100 {
            for x in 60..150 {
                assert_eq!(first.get_pixel(x, y), second.get_pixel(x, y), "({x}, {y})");
            }
        }
    }
    Ok(())
}

#[test]
fn a_nested_overlay_layer_draws_above_the_card_even_for_earlier_primitive_kinds()
-> anyhow::Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let mut scene = Scene::default();
    checkerboard(&mut scene, 200.0, 120.0, 6.0);
    let bounds = rect(30.0, 20.0, 140.0, 80.0);
    scene.push_layer(bounds);
    blur(&mut scene, bounds, Corners::default(), 6.0, None);
    scene.insert_primitive(quad(
        bounds,
        color(0x20242C).opacity(0.8),
        Corners::default(),
    ));
    // An underline sorts after quads within one layer order.
    scene.insert_primitive(vespertine::Underline {
        order: 0,
        curved_clip: 0,
        bounds: rect(40.0, 50.0, 100.0, 6.0),
        content_mask: ContentMask {
            bounds: rect(-1e6, -1e6, 2e6, 2e6),
        },
        color: color(0xFF00FF),
        thickness: ScaledPixels(6.0),
        wavy: false.into(),
    });
    let overlay = rect(60.0, 44.0, 40.0, 20.0);
    scene.push_layer(overlay);
    fill(&mut scene, overlay, color(0x406090));
    scene.pop_layer();
    scene.pop_layer();
    let image = render(&mut renderer, scene, 200, 120);
    // The overlay quad covers the underline where they overlap.
    assert_eq!(image.get_pixel(80, 53).0[..3], [0x40, 0x60, 0x90]);
    // The underline is visible outside the overlay.
    assert_eq!(image.get_pixel(120, 53).0[..3], [0xFF, 0x00, 0xFF]);
    Ok(())
}

#[test]
fn nested_panels_in_separate_layers_see_earlier_panels_but_not_later_siblings() -> anyhow::Result<()>
{
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let panel = |scene: &mut Scene, bounds: Bounds<ScaledPixels>, sigma: f32, tint: u32| {
        scene.push_layer(bounds);
        blur(scene, bounds, Corners::default(), sigma, None);
        scene.insert_primitive(quad(bounds, color(tint).opacity(0.2), Corners::default()));
        scene.pop_layer();
    };
    let first_bounds = rect(10.0, 10.0, 100.0, 80.0);
    let second_bounds = rect(60.0, 30.0, 100.0, 80.0);
    let build = |first: bool, second: bool| {
        let mut scene = Scene::default();
        checkerboard(&mut scene, 200.0, 120.0, 6.0);
        if first {
            panel(&mut scene, first_bounds, 5.0, 0xFF0000);
        }
        if second {
            panel(&mut scene, second_bounds, 2.0, 0x0000FF);
        }
        scene
    };
    let second_only = render(&mut renderer, build(false, true), 200, 120);
    let both = render(&mut renderer, build(true, true), 200, 120);
    let mut differing = 0;
    for y in 40..90 {
        for x in 70..105 {
            differing +=
                usize::from(channel_diff(both.get_pixel(x, y), second_only.get_pixel(x, y)) > 2);
        }
    }
    assert!(
        differing > 100,
        "the later panel's input contains the earlier panel"
    );
    // The first panel never sees the later sibling: its exclusive region matches a render
    // without the second panel.
    let first_only = render(&mut renderer, build(true, false), 200, 120);
    for y in 12..28 {
        for x in 12..108 {
            assert_eq!(both.get_pixel(x, y), first_only.get_pixel(x, y));
        }
    }
    Ok(())
}

#[test]
fn opaque_styling_renders_without_translucency_and_the_blur_stays_available() -> anyhow::Result<()>
{
    // The headless target is opaque: no desktop translucency is requested anywhere.
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let bounds = rect(20.0, 20.0, 60.0, 40.0);
    let mut opaque = Scene::default();
    checkerboard(&mut opaque, 100.0, 80.0, 5.0);
    fill(&mut opaque, bounds, color(0x20242C));
    let image = render(&mut renderer, opaque, 100, 80);
    for (x, y) in [(25, 25), (50, 40), (75, 55)] {
        assert_eq!(image.get_pixel(x, y).0, [0x20, 0x24, 0x2C, 255]);
    }

    let mut blurred = Scene::default();
    checkerboard(&mut blurred, 100.0, 80.0, 5.0);
    blur(&mut blurred, bounds, Corners::default(), 4.0, None);
    let image = render(&mut renderer, blurred, 100, 80);
    let value = i32::from(image.get_pixel(50, 40).0[0]);
    assert!(
        (value - 127).abs() < 60,
        "the blur works independently, got {value}"
    );
    Ok(())
}
