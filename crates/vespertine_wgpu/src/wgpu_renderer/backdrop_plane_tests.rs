//! GPU tests of the overlay plane's backdrop (PLT-001 R9): an overlay plane's backdrop-blur
//! effects blur the finished base plane with the plane's own earlier content blended over it.

use super::blur_tests::{blur, color, device_size, fill, rect};
use super::*;
use vespertine::{Corners, PlatformHeadlessRenderer};

const WIDTH: i32 = 128;
const HEIGHT: i32 = 64;

/// The base plane: red on the left half, blue on the right half.
fn base_scene() -> Scene {
    let mut scene = Scene::default();
    fill(
        &mut scene,
        rect(0.0, 0.0, 64.0, HEIGHT as f32),
        color(0xFF0000),
    );
    fill(
        &mut scene,
        rect(64.0, 0.0, 64.0, HEIGHT as f32),
        color(0x0000FF),
    );
    scene.finish();
    scene
}

/// An overlay plane: `content` painted first, then a blur over the middle of the plane.
fn overlay_scene(content: impl FnOnce(&mut Scene)) -> Scene {
    let mut scene = Scene::default();
    content(&mut scene);
    blur(
        &mut scene,
        rect(16.0, 8.0, 96.0, 48.0),
        Corners::default(),
        4.0,
        None,
    );
    scene.finish();
    scene
}

fn close(actual: [u8; 4], expected: [u8; 4], tolerance: u8) -> bool {
    actual
        .iter()
        .zip(expected)
        .all(|(actual, expected)| actual.abs_diff(expected) <= tolerance)
}

fn assert_pixel(image: &image::RgbaImage, x: u32, y: u32, expected: [u8; 4], tolerance: u8) {
    let actual = image.get_pixel(x, y).0;
    assert!(
        close(actual, expected, tolerance),
        "pixel ({x}, {y}) was {actual:?}, expected {expected:?}"
    );
}

struct Planes {
    base: WgpuHeadlessRenderer,
    overlay: WgpuHeadlessRenderer,
}

impl Planes {
    fn new() -> anyhow::Result<Self> {
        Ok(Self {
            base: WgpuHeadlessRenderer::new()?,
            overlay: WgpuHeadlessRenderer::new_transparent()?,
        })
    }

    /// Draws the base plane, then the overlay over it, and reads the overlay plane back.
    fn overlay_over_base(&mut self, overlay: &Scene) -> anyhow::Result<image::RgbaImage> {
        self.base
            .render_scene(&base_scene(), device_size(WIDTH, HEIGHT))?;
        let plane = self
            .base
            .backdrop_plane()
            .expect("a drawn opaque base plane");
        self.overlay
            .render_scene_to_image_over(overlay, device_size(WIDTH, HEIGHT), &plane)
    }
}

#[test]
fn overlay_blur_blurs_the_base_plane_inside_its_region_only() -> anyhow::Result<()> {
    let mut planes = Planes::new()?;
    let image = planes.overlay_over_base(&overlay_scene(|_| {}))?;

    // Far from the seam the blurred base is the base color, fully opaque.
    assert_pixel(&image, 24, 32, [255, 0, 0, 255], 3);
    assert_pixel(&image, 104, 32, [0, 0, 255, 255], 3);
    // At the seam red and blue mix; the plane is opaque there too.
    let seam = image.get_pixel(64, 32).0;
    assert!(
        (60..=200).contains(&seam[0]) && (60..=200).contains(&seam[2]) && seam[3] == 255,
        "the seam is a mix of red and blue, was {seam:?}"
    );
    // The overlay plane stays transparent outside the effect region.
    assert_pixel(&image, 4, 32, [0, 0, 0, 0], 0);
    assert_pixel(&image, 124, 60, [0, 0, 0, 0], 0);
    assert_pixel(&image, 64, 2, [0, 0, 0, 0], 0);
    Ok(())
}

#[test]
fn without_a_base_plane_the_effect_sees_only_the_overlays_own_content() -> anyhow::Result<()> {
    let mut overlay = WgpuHeadlessRenderer::new_transparent()?;
    let scene = overlay_scene(|_| {});
    let image = overlay.render_scene_to_image(&scene, device_size(WIDTH, HEIGHT))?;
    // The documented fallback: the region holds the blurred (here empty) overlay content, so
    // the base plane shows through unblurred.
    assert_pixel(&image, 24, 32, [0, 0, 0, 0], 0);
    assert_pixel(&image, 64, 32, [0, 0, 0, 0], 0);
    Ok(())
}

#[test]
fn earlier_overlay_content_is_blended_over_the_base_plane_before_blurring() -> anyhow::Result<()> {
    let mut planes = Planes::new()?;
    // Half-transparent green over the red left half: the snapshot is their premultiplied
    // "over" composite, so the blurred region reads (127, 127, 0).
    let scene = overlay_scene(|scene| {
        fill(
            scene,
            rect(0.0, 0.0, 128.0, 64.0),
            color(0x00FF00).opacity(0.5),
        )
    });
    let image = planes.overlay_over_base(&scene)?;
    assert_pixel(&image, 24, 32, [127, 127, 0, 255], 4);
    assert_pixel(&image, 104, 32, [0, 127, 127, 255], 4);
    // Outside the region the overlay keeps its own half-transparent green (premultiplied).
    assert_pixel(&image, 4, 32, [0, 127, 0, 127], 4);
    Ok(())
}

#[test]
fn an_unpainted_base_plane_is_treated_as_opaque_like_the_compositor_shows_it() -> anyhow::Result<()>
{
    let mut planes = Planes::new()?;
    // The opaque surface's alpha channel is ignored by the compositor, so a cleared-to-
    // transparent base plane shows black there; the blurred region must be that black, opaque.
    planes.base.set_clear_color([0.0; 4]);
    planes
        .base
        .render_scene(&Scene::default(), device_size(WIDTH, HEIGHT))?;
    let plane = planes.base.backdrop_plane().expect("a drawn base plane");
    let image = planes.overlay.render_scene_to_image_over(
        &overlay_scene(|_| {}),
        device_size(WIDTH, HEIGHT),
        &plane,
    )?;
    assert_pixel(&image, 64, 32, [0, 0, 0, 255], 0);
    assert_pixel(&image, 4, 32, [0, 0, 0, 0], 0);
    Ok(())
}

#[test]
fn a_base_plane_of_another_size_falls_back_to_the_overlays_own_content() -> anyhow::Result<()> {
    let mut planes = Planes::new()?;
    planes
        .base
        .render_scene(&base_scene(), device_size(WIDTH / 2, HEIGHT / 2))?;
    let plane = planes.base.backdrop_plane().expect("a drawn base plane");
    let image = planes.overlay.render_scene_to_image_over(
        &overlay_scene(|_| {}),
        device_size(WIDTH, HEIGHT),
        &plane,
    )?;
    assert_eq!(image.dimensions(), (WIDTH as u32, HEIGHT as u32));
    assert_pixel(&image, 24, 32, [0, 0, 0, 0], 0);
    Ok(())
}

#[test]
fn a_transparent_renderer_never_offers_its_plane() -> anyhow::Result<()> {
    let mut overlay = WgpuHeadlessRenderer::new_transparent()?;
    overlay.render_scene(&base_scene(), device_size(WIDTH, HEIGHT))?;
    assert!(
        overlay.backdrop_plane().is_none(),
        "a translucent base plane is composed with the desktop, which the overlay cannot see"
    );
    Ok(())
}
