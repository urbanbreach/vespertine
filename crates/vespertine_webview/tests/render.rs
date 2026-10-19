//! PLT-015 R13 / A6 / A7: the texture-surface path and the image path draw identical pixels,
//! rows honour the pool stride, and only the damage rectangle is rewritten.

use std::borrow::Cow;
use std::os::fd::AsRawFd;
use std::sync::Arc;
use std::time::Duration;
use vespertine::{
    AtlasKey, Bounds, ContentMask, Corners, DevicePixels, PaintSurface, PlatformAtlas,
    PlatformHeadlessRenderer, PolychromeSprite, RenderImage, RenderImageParams, ScaledPixels,
    Scene, Size, point, size,
};
use vespertine_webview::frames::{Frame, Pool, write_memfd};
use vespertine_webview::gpu;
use vespertine_webview::protocol::kind;
use vespertine_webview::testing::{Peer, wait_until};
use vespertine_wgpu::{WgpuHeadlessRenderer, wgpu};

const WIDTH: u32 = 1000;
const HEIGHT: u32 = 24;
const STRIDE: u32 = 4096;

fn pattern(x: u32, y: u32) -> [u8; 4] {
    [
        (x % 251) as u8,
        (y * 9 % 251) as u8,
        (x * 7 % 251) as u8,
        255,
    ]
}

/// A buffer frame with stride 4096 for width 1000, filled with a position-dependent pattern.
fn pattern_frame(
    scale: f32,
) -> (
    Frame,
    Peer,
    vespertine_webview::NativePage,
    Vec<std::os::fd::OwnedFd>,
) {
    let (helper, peer) = Peer::start();
    let page = helper.new_page().unwrap();
    let (mut object, fds) = Pool::announcement_for_tests(1, WIDTH, HEIGHT, STRIDE, scale, 2);
    let mut slot = vec![0u8; (STRIDE * HEIGHT) as usize];
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let offset = (y * STRIDE + x * 4) as usize;
            slot[offset..offset + 4].copy_from_slice(&pattern(x, y));
        }
    }
    write_memfd(&fds[0], 0, &slot);
    object.insert("scale".into(), f64::from(scale).into());
    let raws: Vec<_> = fds.iter().map(|fd| fd.as_raw_fd()).collect();
    peer.send_with_fds(
        kind::POOL,
        1,
        serde_json::Value::Object(object).to_string().as_bytes(),
        &raws,
    );
    assert!(wait_until(Duration::from_secs(5), || page.pool().is_some()));
    peer.send_json(
        kind::BUFFER,
        1,
        serde_json::json!({"generation": 1, "slot": 0, "seq": 1, "width": WIDTH, "height": HEIGHT,
            "scale": f64::from(scale), "damage": [0, 0, WIDTH, HEIGHT]}),
    );
    assert!(wait_until(Duration::from_secs(5), || page.has_pending_frame()));
    let frame = page.take_frame().unwrap();
    // `helper` must outlive the page: keep it alive through the page's own handle.
    let _ = helper;
    (frame, peer, page, fds)
}

fn device_bounds(width: u32, height: u32) -> Bounds<ScaledPixels> {
    Bounds {
        origin: point(ScaledPixels(0.0), ScaledPixels(0.0)),
        size: size(ScaledPixels(width as f32), ScaledPixels(height as f32)),
    }
}

fn texture_scene(texture: Arc<wgpu::Texture>, width: u32, height: u32) -> Scene {
    let bounds = device_bounds(width, height);
    let mut scene = Scene::default();
    scene.insert_primitive(PaintSurface {
        order: 0,
        bounds,
        content_mask: ContentMask { bounds },
        opacity: 1.0,
        texture,
    });
    scene.finish();
    scene
}

fn image_scene(atlas: &Arc<dyn PlatformAtlas>, frame: &Frame, id: usize) -> Scene {
    let packed = frame.packed_pixels();
    let buffer = image::RgbaImage::from_raw(frame.width(), frame.height(), packed).unwrap();
    let render_image = RenderImage::new(vec![image::Frame::new(buffer)]);
    let _ = id;
    let tile = atlas
        .get_or_insert_with(
            AtlasKey::Image(RenderImageParams {
                image_id: render_image.id,
                frame_index: 0,
            }),
            &mut || {
                Ok(Some((
                    render_image.size(0),
                    Cow::Borrowed(render_image.as_bytes(0).unwrap()),
                )))
            },
        )
        .unwrap()
        .unwrap();
    let bounds = device_bounds(frame.width(), frame.height());
    let mut scene = Scene::default();
    scene.insert_primitive(PolychromeSprite {
        order: 0,
        pad: 0,
        grayscale: false.into(),
        bounds,
        content_mask: ContentMask { bounds },
        corner_radii: Corners::default(),
        tile,
        opacity: 1.0,
    });
    scene.finish();
    scene
}

fn device_size(width: u32, height: u32) -> Size<DevicePixels> {
    size(DevicePixels(width as i32), DevicePixels(height as i32))
}

#[test]
fn texture_and_image_paths_draw_identical_pixels_with_a_padded_stride() {
    for scale in [1.0f32, 2.0] {
        let (frame, _peer, _page, _fds) = pattern_frame(scale);
        let Frame::Buffer(buffer) = &frame else {
            panic!("a buffer frame");
        };
        let mut renderer = WgpuHeadlessRenderer::new_isolated().unwrap();
        renderer.set_clear_color([1.0, 1.0, 1.0, 1.0]);
        let (device, queue) = renderer
            .shared_gpu_context()
            .unwrap()
            .downcast::<wgpu::Device, wgpu::Queue>()
            .unwrap();
        let mut slot = None;
        let mut shadow = None;
        let texture = gpu::upload(&mut slot, &device, &queue, buffer, &mut shadow);
        let via_texture = renderer
            .render_scene_to_image(
                &texture_scene(texture, WIDTH, HEIGHT),
                device_size(WIDTH, HEIGHT),
            )
            .unwrap();
        let atlas = renderer.sprite_atlas();
        let via_image = renderer
            .render_scene_to_image(&image_scene(&atlas, &frame, 0), device_size(WIDTH, HEIGHT))
            .unwrap();
        assert_eq!(via_texture.dimensions(), (WIDTH, HEIGHT));
        // The pixel at column 999 of row 1 is the byte pattern placed at offset 4096 + 3996.
        let expected = pattern(999, 1);
        for (name, image) in [("texture", &via_texture), ("image", &via_image)] {
            let pixel = image.get_pixel(999, 1).0;
            // The renderer reads back RGBA; the frame is BGRA.
            assert_eq!(
                [pixel[2], pixel[1], pixel[0], pixel[3]],
                expected,
                "{name} path at scale {scale}"
            );
        }
        let mut differing = 0;
        for (a, b) in via_texture.pixels().zip(via_image.pixels()) {
            if a.0.iter().zip(b.0).any(|(x, y)| x.abs_diff(y) > 1) {
                differing += 1;
            }
        }
        assert_eq!(differing, 0, "paths differ at scale {scale}");
        // The shadow copy used by the image fallback equals the packed rows.
        assert_eq!(shadow.as_deref(), Some(frame.packed_pixels().as_slice()));
    }
}

#[test]
fn only_the_damage_rectangle_is_rewritten_into_the_persistent_texture() {
    let (_helper, peer) = Peer::start();
    let page = _helper.new_page().unwrap();
    let (object, fds) = Pool::announcement_for_tests(1, 64, 8, 256, 1.0, 2);
    let raws: Vec<_> = fds.iter().map(|fd| fd.as_raw_fd()).collect();
    // Slot 0 is all (1,2,3,255); slot 1 is all (200,100,50,255).
    for (slot, color) in [(0, [1u8, 2, 3, 255]), (1, [200, 100, 50, 255])] {
        let bytes: Vec<u8> = color.into_iter().cycle().take(256 * 8).collect();
        write_memfd(&fds[slot], 0, &bytes);
    }
    peer.send_with_fds(
        kind::POOL,
        1,
        serde_json::Value::Object(object).to_string().as_bytes(),
        &raws,
    );
    assert!(wait_until(Duration::from_secs(5), || page.pool().is_some()));
    let send = |slot: u32, seq: u64, damage: [u32; 4]| {
        peer.send_json(
            kind::BUFFER,
            1,
            serde_json::json!({"generation": 1, "slot": slot, "seq": seq, "width": 64, "height": 8,
                "scale": 1.0, "damage": damage}),
        )
    };
    let mut renderer = WgpuHeadlessRenderer::new_isolated().unwrap();
    renderer.set_clear_color([1.0, 1.0, 1.0, 1.0]);
    let (device, queue) = renderer
        .shared_gpu_context()
        .unwrap()
        .downcast::<wgpu::Device, wgpu::Queue>()
        .unwrap();
    let mut slot = None;
    let mut shadow = None;
    send(0, 1, [0, 0, 64, 8]);
    assert!(wait_until(Duration::from_secs(5), || page.has_pending_frame()));
    let Some(Frame::Buffer(first)) = page.take_frame() else {
        panic!()
    };
    let texture = gpu::upload(&mut slot, &device, &queue, &first, &mut shadow);
    // The second frame (slot 1) only reports damage (8, 2, 4, 3): only that region may change in
    // the persistent texture, although slot 1 differs everywhere.
    send(1, 2, [8, 2, 4, 3]);
    assert!(wait_until(Duration::from_secs(5), || page.has_pending_frame()));
    let Some(Frame::Buffer(second)) = page.take_frame() else {
        panic!()
    };
    let again = gpu::upload(&mut slot, &device, &queue, &second, &mut shadow);
    assert!(Arc::ptr_eq(&texture, &again), "the texture is persistent");
    let image = renderer
        .render_scene_to_image(&texture_scene(again, 64, 8), device_size(64, 8))
        .unwrap();
    for y in 0..8u32 {
        for x in 0..64u32 {
            let inside = (8..12).contains(&x) && (2..5).contains(&y);
            let expected = if inside { [50, 100, 200] } else { [3, 2, 1] };
            let pixel = image.get_pixel(x, y).0;
            assert_eq!([pixel[0], pixel[1], pixel[2]], expected, "pixel ({x}, {y})");
        }
    }
    // A size change allocates a new texture and uploads the full frame.
    let (object, fds) = Pool::announcement_for_tests(2, 32, 4, 128, 1.0, 2);
    let raws: Vec<_> = fds.iter().map(|fd| fd.as_raw_fd()).collect();
    let bytes: Vec<u8> = [9u8, 9, 9, 255].into_iter().cycle().take(128 * 4).collect();
    write_memfd(&fds[0], 0, &bytes);
    peer.send_with_fds(
        kind::POOL,
        1,
        serde_json::Value::Object(object).to_string().as_bytes(),
        &raws,
    );
    assert!(wait_until(Duration::from_secs(5), || page
        .pool()
        .is_some_and(|p| p.generation == 2)));
    peer.send_json(
        kind::BUFFER,
        1,
        serde_json::json!({"generation": 2, "slot": 0, "seq": 3, "width": 32, "height": 4,
            "scale": 1.0, "damage": [0, 0, 1, 1]}),
    );
    assert!(wait_until(Duration::from_secs(5), || page.has_pending_frame()));
    let Some(Frame::Buffer(third)) = page.take_frame() else {
        panic!()
    };
    let resized = gpu::upload(&mut slot, &device, &queue, &third, &mut shadow);
    assert!(!Arc::ptr_eq(&texture, &resized));
    let image = renderer
        .render_scene_to_image(&texture_scene(resized, 32, 4), device_size(32, 4))
        .unwrap();
    assert_eq!(
        image.get_pixel(31, 3).0[..3],
        [9, 9, 9],
        "full upload after a resize"
    );
}
