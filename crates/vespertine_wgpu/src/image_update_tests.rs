//! PRF-016: in-place atlas updates, observed through pixels rendered by the headless
//! wgpu renderer.

use anyhow::{Result, ensure};
use std::borrow::Cow;
use vespertine::{
    AtlasKey, AtlasTile, Bounds, ContentMask, Corners, DevicePixels, ImageId, PlatformAtlas,
    PlatformHeadlessRenderer, PolychromeSprite, RenderImageParams, Scene, Size,
};

use crate::WgpuHeadlessRenderer;

/// Atlas bytes use the layout of ordinary image insertion: BGRA.
const RED: [u8; 4] = [0, 0, 255, 255];
const BLUE: [u8; 4] = [255, 0, 0, 255];
const RED_RGBA: [u8; 4] = [255, 0, 0, 255];
const BLUE_RGBA: [u8; 4] = [0, 0, 255, 255];

fn device_size(width: i32, height: i32) -> Size<DevicePixels> {
    Size {
        width: DevicePixels(width),
        height: DevicePixels(height),
    }
}

fn image_key(image_id: usize) -> AtlasKey {
    AtlasKey::Image(RenderImageParams {
        image_id: ImageId(image_id),
        frame_index: 0,
    })
}

fn solid(pixel: [u8; 4], width: i32, height: i32) -> Vec<u8> {
    pixel
        .iter()
        .copied()
        .cycle()
        .take(width as usize * height as usize * 4)
        .collect()
}

fn insert(
    atlas: &dyn PlatformAtlas,
    key: &AtlasKey,
    size: Size<DevicePixels>,
    bytes: Vec<u8>,
) -> Result<AtlasTile> {
    atlas
        .get_or_insert_with(key.clone(), &mut || {
            Ok(Some((size, Cow::Borrowed(&bytes[..]))))
        })?
        .ok_or_else(|| anyhow::anyhow!("builder returned a tile"))
}

/// A scene drawing `tile` 1:1 at the target origin.
fn sprite_scene(tile: AtlasTile) -> Scene {
    sprite_scene_at(tile, 0.0)
}

fn sprite_scene_at(tile: AtlasTile, x: f32) -> Scene {
    let bounds = Bounds {
        origin: vespertine::Point {
            x: x.into(),
            y: 0.0.into(),
        },
        size: Size {
            width: (tile.bounds.size.width.0 as f32).into(),
            height: (tile.bounds.size.height.0 as f32).into(),
        },
    };
    let mut scene = Scene::default();
    scene.insert_primitive(PolychromeSprite {
        order: 0,
        pad: 0,
        grayscale: false.into(),
        opacity: 1.0,
        bounds,
        content_mask: ContentMask { bounds },
        corner_radii: Corners::default(),
        tile,
        scope: Default::default(),
        effects: Default::default(),
    });
    scene.finish();
    scene
}

fn render(
    renderer: &mut WgpuHeadlessRenderer,
    tile: AtlasTile,
    target: Size<DevicePixels>,
) -> Result<image::RgbaImage> {
    renderer.render_scene_to_image(&sprite_scene(tile), target)
}

fn pixel(image: &image::RgbaImage, x: u32, y: u32) -> [u8; 4] {
    image.get_pixel(x, y).0
}

fn assert_pixel(image: &image::RgbaImage, x: u32, y: u32, expected: [u8; 4]) -> Result<()> {
    let actual = pixel(image, x, y);
    ensure!(
        actual
            .iter()
            .zip(expected)
            .all(|(actual, expected)| actual.abs_diff(expected) <= 3),
        "pixel ({x}, {y}) was {actual:?}, expected {expected:?}"
    );
    Ok(())
}

#[test]
fn same_size_update_keeps_the_tile_and_changes_pixels_then_resize_reallocates() -> Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let atlas = renderer.sprite_atlas();
    let key = image_key(1);
    let one = device_size(1, 1);

    let original = insert(atlas.as_ref(), &key, one, solid(RED, 1, 1))?;
    assert_pixel(&render(&mut renderer, original, one)?, 0, 0, RED_RGBA)?;

    // A same-size update returns exactly the same tile, with new pixels.
    let updated = atlas.update(key.clone(), one, &solid(BLUE, 1, 1))?;
    assert_eq!(updated, original);
    assert_pixel(&render(&mut renderer, updated, one)?, 0, 0, BLUE_RGBA)?;

    // A size change reallocates: different dimensions and a different tile.
    let two_by_one = device_size(2, 1);
    let mut bytes = solid(RED, 1, 1);
    bytes.extend(solid(BLUE, 1, 1));
    let resized = atlas.update(key.clone(), two_by_one, &bytes)?;
    assert_eq!(resized.bounds.size, two_by_one);
    assert_ne!(resized, original);
    let image = render(&mut renderer, resized, two_by_one)?;
    assert_pixel(&image, 0, 0, RED_RGBA)?;
    assert_pixel(&image, 1, 0, BLUE_RGBA)?;

    // The key now resolves to the new tile.
    assert_eq!(insert(atlas.as_ref(), &key, one, Vec::new())?, resized);
    Ok(())
}

#[test]
fn update_of_a_missing_key_inserts_a_tile() -> Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let atlas = renderer.sprite_atlas();
    let key = image_key(2);
    let one = device_size(1, 1);

    let tile = atlas.update(key.clone(), one, &solid(BLUE, 1, 1))?;
    assert_pixel(&render(&mut renderer, tile, one)?, 0, 0, BLUE_RGBA)?;
    assert_eq!(insert(atlas.as_ref(), &key, one, Vec::new())?, tile);
    Ok(())
}

#[test]
fn update_leaves_neighbouring_tiles_untouched() -> Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let atlas = renderer.sprite_atlas();
    let size = device_size(4, 4);

    let first = insert(atlas.as_ref(), &image_key(3), size, solid(RED, 4, 4))?;
    let second = insert(atlas.as_ref(), &image_key(4), size, solid(RED, 4, 4))?;
    assert_eq!(first.texture_id, second.texture_id);
    assert_ne!(first.bounds.origin, second.bounds.origin);

    let updated = atlas.update(image_key(3), size, &solid(BLUE, 4, 4))?;
    assert_eq!(updated, first);

    let image = render(&mut renderer, first, size)?;
    assert_pixel(&image, 0, 0, BLUE_RGBA)?;
    assert_pixel(&image, 3, 3, BLUE_RGBA)?;
    let image = render(&mut renderer, second, size)?;
    assert_pixel(&image, 0, 0, RED_RGBA)?;
    assert_pixel(&image, 3, 3, RED_RGBA)?;
    Ok(())
}

/// R4: the queue write is ordered after the frame that sampled the old pixels and
/// before the next frame, for every alternation, and the newest of several updates
/// between two frames wins.
#[test]
fn queue_writes_are_ordered_against_frames_sampling_the_texture() -> Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let atlas = renderer.sprite_atlas();
    let key = image_key(5);
    let size = device_size(8, 8);

    let tile = insert(atlas.as_ref(), &key, size, solid(RED, 8, 8))?;
    assert_pixel(&render(&mut renderer, tile, size)?, 4, 4, RED_RGBA)?;

    for frame in 0..16usize {
        let (color, expected) = if frame.is_multiple_of(2) {
            (BLUE, BLUE_RGBA)
        } else {
            (RED, RED_RGBA)
        };
        let updated = atlas.update(key.clone(), size, &solid(color, 8, 8))?;
        assert_eq!(updated, tile);
        // Submitted without waiting: the following update must not leak into it.
        renderer.render_scene(&sprite_scene(updated), size)?;
        let image = render(&mut renderer, updated, size)?;
        assert_pixel(&image, 0, 0, expected)?;
        assert_pixel(&image, 7, 7, expected)?;
    }

    atlas.update(key.clone(), size, &solid(BLUE, 8, 8))?;
    let last = atlas.update(key, size, &solid(RED, 8, 8))?;
    assert_eq!(last, tile);
    assert_pixel(&render(&mut renderer, last, size)?, 4, 4, RED_RGBA)?;
    Ok(())
}

#[test]
fn update_with_mismatched_bytes_errors_and_keeps_the_existing_tile() -> Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let atlas = renderer.sprite_atlas();
    let key = image_key(6);
    let size = device_size(2, 2);

    let tile = insert(atlas.as_ref(), &key, size, solid(RED, 2, 2))?;
    assert!(atlas.update(key.clone(), size, &solid(BLUE, 1, 1)).is_err());
    assert!(atlas.update(key.clone(), size, &[]).is_err());

    assert_eq!(insert(atlas.as_ref(), &key, size, Vec::new())?, tile);
    let image = render(&mut renderer, tile, size)?;
    assert_pixel(&image, 1, 1, RED_RGBA)?;
    Ok(())
}

/// A5 at the backend: after a resize reallocates, a redraw of the new geometry is
/// correct and later same-size updates are in place again.
#[test]
fn resized_image_redraws_new_geometry_then_updates_in_place_again() -> Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let atlas = renderer.sprite_atlas();
    let key = image_key(7);
    let small = device_size(2, 2);
    let large = device_size(4, 2);

    let before = insert(atlas.as_ref(), &key, small, solid(RED, 2, 2))?;
    let resized = atlas.update(key.clone(), large, &solid(BLUE, 4, 2))?;
    assert_ne!(resized, before);
    assert_eq!(resized.bounds.size, large);

    let image = render(&mut renderer, resized, large)?;
    assert_pixel(&image, 0, 0, BLUE_RGBA)?;
    assert_pixel(&image, 3, 1, BLUE_RGBA)?;

    let retained = atlas.update(key, large, &solid(RED, 4, 2))?;
    assert_eq!(retained, resized);
    let image = render(&mut renderer, retained, large)?;
    assert_pixel(&image, 0, 0, RED_RGBA)?;
    assert_pixel(&image, 3, 1, RED_RGBA)?;
    Ok(())
}

/// Updating a texture that spans a flushed upload and a pending one works.
#[test]
fn update_before_the_first_flush_supersedes_the_pending_insert() -> Result<()> {
    let mut renderer = WgpuHeadlessRenderer::new()?;
    let atlas = renderer.sprite_atlas();
    let key = image_key(8);
    let size = device_size(2, 2);

    let tile = insert(atlas.as_ref(), &key, size, solid(RED, 2, 2))?;
    let updated = atlas.update(key, size, &solid(BLUE, 2, 2))?;
    assert_eq!(updated, tile);
    let image = renderer.render_scene_to_image(&sprite_scene_at(tile, 0.0), size)?;
    assert_pixel(&image, 0, 0, BLUE_RGBA)?;
    Ok(())
}
