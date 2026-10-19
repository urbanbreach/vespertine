//! RND-006 renderer tests. They render scenes through the headless renderer, which runs the
//! same recording path as a window, and read pixels back to check the composition.

use super::*;
use crate::WgpuHeadlessRenderer;
use std::any::Any;
use std::sync::Arc;
use vespertine::{
    AtlasKey, BorderStyle, ContentMask, Corners, DevicePixels, Edges, ImageId,
    PlatformHeadlessRenderer, Point, Quad, RenderImageParams, Scene, SharedGpuContext, Size, black,
};

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];

fn device_size(width: i32, height: i32) -> Size<DevicePixels> {
    Size {
        width: DevicePixels(width),
        height: DevicePixels(height),
    }
}

fn bounds(x: i32, y: i32, width: i32, height: i32) -> Bounds<ScaledPixels> {
    Bounds {
        origin: Point {
            x: ScaledPixels(x as f32),
            y: ScaledPixels(y as f32),
        },
        size: Size {
            width: ScaledPixels(width as f32),
            height: ScaledPixels(height as f32),
        },
    }
}

fn assert_pixel(image: &image::RgbaImage, x: u32, y: u32, expected: [u8; 4], tolerance: u8) {
    let actual = image.get_pixel(x, y).0;
    assert!(
        actual
            .iter()
            .zip(expected)
            .all(|(actual, expected)| actual.abs_diff(expected) <= tolerance),
        "pixel ({x}, {y}) was {actual:?}, expected {expected:?} (tolerance {tolerance})"
    );
}

fn solid_quad(rect: Bounds<ScaledPixels>, color: vespertine::Hsla) -> Quad {
    Quad {
        order: 0,
        border_style: BorderStyle::Solid,
        bounds: rect,
        content_mask: ContentMask { bounds: rect },
        background: color.into(),
        border_color: color,
        corner_radii: Corners::default(),
        border_widths: Edges::default(),
        scope: Default::default(),
    }
}

fn renderer() -> anyhow::Result<WgpuHeadlessRenderer> {
    let mut renderer = WgpuHeadlessRenderer::new_isolated()?;
    renderer.set_clear_color([1.0, 1.0, 1.0, 1.0]);
    Ok(renderer)
}

fn device_and_queue(renderer: &WgpuHeadlessRenderer) -> (Arc<wgpu::Device>, Arc<wgpu::Queue>) {
    renderer
        .shared_gpu_context()
        .expect("a healthy renderer exposes its context")
        .downcast::<wgpu::Device, wgpu::Queue>()
        .expect("the context holds the renderer's wgpu device and queue")
}

fn texture_with_usage(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("surface_test_texture"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage,
        view_formats: &[],
    })
}

const SAMPLED: wgpu::TextureUsages = wgpu::TextureUsages::TEXTURE_BINDING
    .union(wgpu::TextureUsages::COPY_DST)
    .union(wgpu::TextureUsages::RENDER_ATTACHMENT);

/// A texture holding `pixels` (row-major RGBA8), shared as an erased handle.
fn uploaded_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> Arc<dyn Any + Send + Sync> {
    let texture = texture_with_usage(device, width, height, SAMPLED);
    queue.write_texture(
        texture.as_image_copy(),
        pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    Arc::new(texture)
}

fn solid_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    width: u32,
    height: u32,
    color: [u8; 4],
) -> Arc<dyn Any + Send + Sync> {
    let pixels: Vec<u8> = color
        .iter()
        .copied()
        .cycle()
        .take((width * height * 4) as usize)
        .collect();
    uploaded_texture(device, queue, width, height, &pixels)
}

fn paint_surface(
    texture: Arc<dyn Any + Send + Sync>,
    destination: Bounds<ScaledPixels>,
    mask: Bounds<ScaledPixels>,
) -> PaintSurface {
    PaintSurface {
        order: 0,
        bounds: destination,
        content_mask: ContentMask { bounds: mask },
        opacity: 1.0,
        curved_clip: 0,
        texture,
    }
}

fn render(
    renderer: &mut WgpuHeadlessRenderer,
    scene: &mut Scene,
    width: i32,
    height: i32,
) -> anyhow::Result<image::RgbaImage> {
    scene.finish();
    renderer.render_scene_to_image(scene, device_size(width, height))
}

#[test]
fn parameter_blocks_respect_uniform_offset_alignment() {
    assert_eq!(PARAMS_SIZE, 48);
    assert_eq!(params_stride(256), 256);
    assert_eq!(params_stride(64), 64);
    assert_eq!(params_stride(16), 48);
    assert_eq!(params_stride(32), 64);
    assert_eq!(params_stride(0), 48);
}

/// FID-RND-006 (A3): two surfaces with distinct destinations and clips in one frame, and an
/// opaque quad painted after both. If every surface used the last surface's parameters, A
/// would appear at B's position and clip.
#[test]
fn fid_rnd_006_two_surfaces_keep_their_own_bounds_and_clips() -> anyhow::Result<()> {
    let mut renderer = renderer()?;
    let (device, queue) = device_and_queue(&renderer);

    // Texture A: 4x4, left two columns #FF0000, right two columns #0000FF.
    let mut pixels_a = Vec::new();
    for _row in 0..4 {
        for column in 0..4 {
            pixels_a.extend_from_slice(if column < 2 { &RED } else { &BLUE });
        }
    }
    let texture_a = uploaded_texture(&device, &queue, 4, 4, &pixels_a);
    let texture_b = solid_texture(&device, &queue, 2, 2, GREEN);

    // Window 480 x 240 logical at scale factor 2: 960 x 480 device pixels.
    let window = bounds(0, 0, 960, 480);
    let mut scene = Scene::default();
    scene.insert_primitive(paint_surface(texture_a, bounds(80, 80, 320, 160), window));
    scene.insert_primitive(paint_surface(
        texture_b,
        bounds(480, 120, 240, 240),
        bounds(520, 160, 160, 160),
    ));
    scene.insert_primitive(solid_quad(bounds(200, 200, 80, 80), black()));
    let image = render(&mut renderer, &mut scene, 960, 480)?;

    // Surface A: solid red up to x = 200, a linear blend to blue at x = 280, solid blue after.
    for x in [80, 100, 150, 198] {
        assert_pixel(&image, x, 100, RED, 3);
    }
    assert_pixel(&image, 239, 100, [129, 0, 126, 255], 4);
    assert_pixel(&image, 240, 100, [126, 0, 129, 255], 4);
    for x in [282, 300, 350, 399] {
        assert_pixel(&image, x, 100, BLUE, 3);
    }
    // No vertical variation, exact edges.
    for y in [80, 160, 239] {
        assert_pixel(&image, 100, y, RED, 3);
        assert_pixel(&image, 300, y, BLUE, 3);
    }
    assert_pixel(&image, 79, 100, WHITE, 0);
    assert_pixel(&image, 400, 100, WHITE, 0);
    assert_pixel(&image, 100, 79, WHITE, 0);
    assert_pixel(&image, 100, 240, WHITE, 0);

    // Surface B: only the clipped square (device 520..680, 160..320) is green.
    for (x, y) in [(520, 160), (600, 240), (679, 319), (520, 319), (679, 160)] {
        assert_pixel(&image, x, y, GREEN, 0);
    }
    for (x, y) in [
        (519, 240),
        (680, 240),
        (600, 159),
        (600, 320),
        (490, 130),
        (710, 350),
        (500, 240),
        (700, 240),
    ] {
        assert_pixel(&image, x, y, WHITE, 0);
    }

    // The quad painted after the surfaces covers the part of A it overlaps (y 200..240).
    for (x, y) in [(200, 200), (240, 220), (279, 239), (240, 270), (279, 279)] {
        assert_pixel(&image, x, y, BLACK, 0);
    }
    assert_pixel(&image, 240, 199, [127, 0, 128, 255], 4);
    assert_pixel(&image, 199, 220, RED, 3);
    assert_pixel(&image, 280, 220, BLUE, 3);
    assert_pixel(&image, 240, 280, WHITE, 0);

    // Nothing of A leaked to B's position or the reverse.
    assert_pixel(&image, 600, 100, WHITE, 0);
    assert_pixel(&image, 100, 300, WHITE, 0);
    Ok(())
}

/// A5: an opaque rectangle painted after a surface covers it; one painted before is covered.
#[test]
fn surfaces_keep_z_order_with_other_primitives() -> anyhow::Result<()> {
    let mut renderer = renderer()?;
    let (device, queue) = device_and_queue(&renderer);
    let window = bounds(0, 0, 100, 60);

    let mut after = Scene::default();
    after.insert_primitive(paint_surface(
        solid_texture(&device, &queue, 2, 2, GREEN),
        bounds(10, 10, 40, 40),
        window,
    ));
    after.insert_primitive(solid_quad(bounds(20, 20, 10, 10), black()));
    let image = render(&mut renderer, &mut after, 100, 60)?;
    assert_pixel(&image, 25, 25, BLACK, 0);
    assert_pixel(&image, 15, 15, GREEN, 0);
    assert_pixel(&image, 45, 45, GREEN, 0);

    let mut before = Scene::default();
    before.insert_primitive(solid_quad(bounds(20, 20, 10, 10), black()));
    before.insert_primitive(paint_surface(
        solid_texture(&device, &queue, 2, 2, GREEN),
        bounds(10, 10, 40, 40),
        window,
    ));
    let image = render(&mut renderer, &mut before, 100, 60)?;
    assert_pixel(&image, 25, 25, GREEN, 0);
    assert_pixel(&image, 15, 15, GREEN, 0);

    // A surface between two quads: the earlier is covered, the later covers.
    let mut sandwich = Scene::default();
    sandwich.insert_primitive(solid_quad(bounds(60, 10, 30, 30), black()));
    sandwich.insert_primitive(paint_surface(
        solid_texture(&device, &queue, 2, 2, GREEN),
        bounds(70, 20, 20, 20),
        window,
    ));
    sandwich.insert_primitive(solid_quad(bounds(80, 30, 20, 20), black()));
    let image = render(&mut renderer, &mut sandwich, 100, 60)?;
    assert_pixel(&image, 62, 12, BLACK, 0);
    assert_pixel(&image, 75, 25, GREEN, 0);
    assert_pixel(&image, 85, 35, BLACK, 0);
    Ok(())
}

/// RND-002 R15: a surface under a curved overflow clip is cut by the shape, with coverage
/// following the shape's own antialiasing, while a surface outside any shape is untouched.
#[test]
fn a_surface_inside_a_curved_overflow_clip_is_cut_by_the_shape() -> anyhow::Result<()> {
    let mut renderer = renderer()?;
    let (device, queue) = device_and_queue(&renderer);
    let window = bounds(0, 0, 120, 60);
    let shape = vespertine::CurvedClipShape {
        bounds: [0.0, 0.0, 60.0, 60.0],
        radii_x: [24.0; 4],
        radii_y: [24.0; 4],
    };
    let mut scene = Scene::default();
    scene.push_curved_clip(shape);
    scene.insert_primitive(paint_surface(
        solid_texture(&device, &queue, 2, 2, GREEN),
        bounds(0, 0, 60, 60),
        window,
    ));
    scene.pop_curved_clip();
    scene.insert_primitive(paint_surface(
        solid_texture(&device, &queue, 2, 2, GREEN),
        bounds(60, 0, 60, 60),
        window,
    ));
    let image = render(&mut renderer, &mut scene, 120, 60)?;
    // The cut-away corner stays the (white) clear color, the interior and the straight edge are
    // green, and the unclipped surface keeps its square corner.
    assert_pixel(&image, 1, 1, WHITE, 0);
    assert_pixel(&image, 30, 30, GREEN, 0);
    assert_pixel(&image, 30, 1, GREEN, 0);
    assert_pixel(&image, 61, 1, GREEN, 0);
    // A pixel on the arc follows the shape's coverage.
    let coverage = vespertine::curved_clip_coverage([&shape], [7.5, 6.5]);
    assert!(coverage > 0.0 && coverage < 1.0, "probe is on the arc");
    let faded = (255.0 * (1.0 - coverage)).round() as u8;
    let expected = [faded, 255, faded, 255];
    assert_pixel(&image, 7, 6, expected, 3);
    Ok(())
}

/// R9 at scale: many surfaces (more than the initial parameter capacity, spread over several
/// batches by interleaved quads) each draw with their own block.
#[test]
fn many_surfaces_each_use_their_own_parameter_block() -> anyhow::Result<()> {
    let mut renderer = renderer()?;
    let (device, queue) = device_and_queue(&renderer);
    let window = bounds(0, 0, 200, 120);
    let mut scene = Scene::default();
    let mut expected = Vec::new();
    for index in 0..30 {
        let (column, row) = (index % 10, index / 10);
        let color = [(index * 8) as u8, 255 - (index * 8) as u8, 40, 255];
        let destination = bounds(column * 20 + 2, row * 20 + 2, 16, 16);
        scene.insert_primitive(paint_surface(
            solid_texture(&device, &queue, 2, 2, color),
            destination,
            window,
        ));
        if index % 7 == 3 {
            // A quad that overlaps nothing but splits the surfaces into batches.
            scene.insert_primitive(solid_quad(
                bounds(column * 20 + 2, row * 20 + 2 + 90, 4, 4),
                black(),
            ));
        }
        expected.push((column * 20 + 10, row * 20 + 10, color));
    }
    let image = render(&mut renderer, &mut scene, 200, 120)?;
    for (x, y, color) in expected {
        assert_pixel(&image, x as u32, y as u32, color, 1);
    }
    Ok(())
}

/// R8: a texture of another concrete type draws nothing and never reaches the GPU; surfaces
/// before and after it still draw with their own parameters.
#[test]
fn wrong_type_textures_are_skipped_without_side_effects() -> anyhow::Result<()> {
    let mut renderer = renderer()?;
    let (device, queue) = device_and_queue(&renderer);
    let window = bounds(0, 0, 90, 30);
    let mut scene = Scene::default();
    scene.insert_primitive(paint_surface(
        solid_texture(&device, &queue, 2, 2, RED),
        bounds(0, 0, 30, 30),
        window,
    ));
    scene.insert_primitive(paint_surface(
        Arc::new(String::from("not a texture")),
        bounds(30, 0, 30, 30),
        window,
    ));
    scene.insert_primitive(paint_surface(
        solid_texture(&device, &queue, 2, 2, BLUE),
        bounds(60, 0, 30, 30),
        window,
    ));
    let image = render(&mut renderer, &mut scene, 90, 30)?;
    assert_pixel(&image, 15, 15, RED, 0);
    assert_pixel(&image, 45, 15, WHITE, 0);
    assert_pixel(&image, 75, 15, BLUE, 0);

    // The only surface is of the wrong type.
    let mut only = Scene::default();
    only.insert_primitive(paint_surface(Arc::new(7_u32), bounds(0, 0, 30, 30), window));
    let image = render(&mut renderer, &mut only, 90, 30)?;
    assert_pixel(&image, 15, 15, WHITE, 0);
    Ok(())
}

/// Zero-size destinations and clips draw nothing; textures the device rejects (no binding
/// usage, multisampled) are skipped and do not poison later frames.
#[test]
fn degenerate_and_unsampleable_surfaces_are_skipped() -> anyhow::Result<()> {
    let mut renderer = renderer()?;
    let (device, queue) = device_and_queue(&renderer);
    let window = bounds(0, 0, 80, 20);
    let mut scene = Scene::default();
    scene.insert_primitive(paint_surface(
        solid_texture(&device, &queue, 2, 2, RED),
        bounds(0, 0, 0, 20),
        window,
    ));
    scene.insert_primitive(paint_surface(
        solid_texture(&device, &queue, 2, 2, RED),
        bounds(0, 0, 20, 20),
        bounds(0, 0, 0, 0),
    ));
    // No TEXTURE_BINDING usage: it cannot be sampled.
    scene.insert_primitive(paint_surface(
        Arc::new(texture_with_usage(
            &device,
            2,
            2,
            wgpu::TextureUsages::COPY_DST,
        )),
        bounds(20, 0, 20, 20),
        window,
    ));
    scene.insert_primitive(paint_surface(
        solid_texture(&device, &queue, 2, 2, GREEN),
        bounds(40, 0, 20, 20),
        window,
    ));
    let image = render(&mut renderer, &mut scene, 80, 20)?;
    assert_pixel(&image, 10, 10, WHITE, 0);
    assert_pixel(&image, 30, 10, WHITE, 0);
    assert_pixel(&image, 50, 10, GREEN, 0);

    // The rejection stayed inside the surface path: the next frame still renders.
    let mut next = Scene::default();
    next.insert_primitive(paint_surface(
        solid_texture(&device, &queue, 2, 2, BLUE),
        bounds(0, 0, 20, 20),
        window,
    ));
    let image = render(&mut renderer, &mut next, 80, 20)?;
    assert_pixel(&image, 10, 10, BLUE, 0);
    Ok(())
}

/// R6: straight alpha like image sprites, multiplied by the element opacity.
#[test]
fn straight_alpha_and_opacity_blend_like_image_sprites() -> anyhow::Result<()> {
    let mut renderer = renderer()?;
    let (device, queue) = device_and_queue(&renderer);
    let window = bounds(0, 0, 60, 20);
    let mut scene = Scene::default();
    // Straight-alpha red at 50%: source color is not premultiplied.
    scene.insert_primitive(paint_surface(
        solid_texture(&device, &queue, 2, 2, [255, 0, 0, 128]),
        bounds(0, 0, 20, 20),
        window,
    ));
    // Opaque green with element opacity 0.5.
    let mut half = paint_surface(
        solid_texture(&device, &queue, 2, 2, GREEN),
        bounds(20, 0, 20, 20),
        window,
    );
    half.opacity = 0.5;
    scene.insert_primitive(half);
    let image = render(&mut renderer, &mut scene, 60, 20)?;
    assert_pixel(&image, 10, 10, [255, 127, 127, 255], 2);
    assert_pixel(&image, 30, 10, [127, 255, 127, 255], 2);
    Ok(())
}

/// A different clip per surface keeps clipping each to its own rectangle, with output zero
/// outside (R7).
#[test]
fn content_mask_clips_each_surface_independently() -> anyhow::Result<()> {
    let mut renderer = renderer()?;
    let (device, queue) = device_and_queue(&renderer);
    let mut scene = Scene::default();
    scene.insert_primitive(paint_surface(
        solid_texture(&device, &queue, 2, 2, RED),
        bounds(0, 0, 40, 40),
        bounds(10, 10, 10, 10),
    ));
    scene.insert_primitive(paint_surface(
        solid_texture(&device, &queue, 2, 2, BLUE),
        bounds(50, 0, 40, 40),
        bounds(70, 20, 20, 20),
    ));
    let image = render(&mut renderer, &mut scene, 100, 50)?;
    assert_pixel(&image, 15, 15, RED, 0);
    assert_pixel(&image, 5, 5, WHITE, 0);
    assert_pixel(&image, 25, 15, WHITE, 0);
    assert_pixel(&image, 80, 30, BLUE, 0);
    assert_pixel(&image, 55, 5, WHITE, 0);
    assert_pixel(&image, 69, 30, WHITE, 0);
    Ok(())
}

/// A producer that owns device resources, as an embedder would.
struct Producer {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    texture: Arc<wgpu::Texture>,
}

impl Producer {
    fn new(context: &SharedGpuContext) -> Self {
        let (device, queue) = context
            .downcast::<wgpu::Device, wgpu::Queue>()
            .expect("wgpu device and queue");
        let texture = Arc::new(texture_with_usage(&device, 8, 8, SAMPLED));
        Self {
            device,
            queue,
            texture,
        }
    }

    /// Clears the texture to `color` in one GPU render pass.
    fn clear(&self, color: wgpu::Color) {
        let view = self
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(color),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            ..Default::default()
        }));
        self.queue.submit(std::iter::once(encoder.finish()));
    }

    fn scene(&self) -> Scene {
        let mut scene = Scene::default();
        scene.insert_primitive(paint_surface(
            self.texture.clone(),
            bounds(8, 8, 48, 48),
            bounds(0, 0, 64, 64),
        ));
        scene
    }
}

/// A4: forced device loss, the loss query, recovery, context reacquisition, producer
/// resources rebuilt and a surface presented again.
#[test]
fn device_loss_and_recovery_present_a_surface_again() -> anyhow::Result<()> {
    let mut renderer = renderer()?;
    assert_eq!(renderer.gpu_device_lost(), Some(false));

    let producer = Producer::new(&renderer.shared_gpu_context().expect("healthy context"));
    producer.clear(wgpu::Color {
        r: 1.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    });
    let image = render(&mut renderer, &mut producer.scene(), 64, 64)?;
    assert_pixel(&image, 32, 32, RED, 1);
    assert_pixel(&image, 4, 4, WHITE, 0);

    // The loss is observed through the loss query alone: the context is not requested while
    // the device is lost.
    renderer.force_device_loss();
    assert_eq!(renderer.gpu_device_lost(), Some(true));
    assert!(
        render(&mut renderer, &mut producer.scene(), 64, 64).is_err(),
        "a lost device does not render"
    );

    // The embedder waits for the platform to recover.
    renderer.recover()?;
    assert_eq!(renderer.gpu_device_lost(), Some(false));
    let context = renderer
        .shared_gpu_context()
        .expect("the recovered device is reacquired through the context accessor");
    let (new_device, _) = context.downcast::<wgpu::Device, wgpu::Queue>().unwrap();
    assert!(
        !Arc::ptr_eq(&producer.device, &new_device),
        "recovery created a new device"
    );

    // A source still referencing the lost device's texture is skipped, not a crash.
    let image = render(&mut renderer, &mut producer.scene(), 64, 64)?;
    assert_pixel(&image, 32, 32, WHITE, 0);

    // Producer resources rebuilt on the new device, and the surface is presented again.
    let rebuilt = Producer::new(&context);
    rebuilt.clear(wgpu::Color {
        r: 0.0,
        g: 0.0,
        b: 1.0,
        a: 1.0,
    });
    let image = render(&mut renderer, &mut rebuilt.scene(), 64, 64)?;
    assert_pixel(&image, 32, 32, BLUE, 1);
    Ok(())
}

/// R2: requesting the context while the device is lost returns no value and never panics.
#[test]
fn context_request_during_loss_returns_none() -> anyhow::Result<()> {
    let renderer = renderer()?;
    assert!(renderer.shared_gpu_context().is_some());
    renderer.force_device_loss();
    assert_eq!(renderer.gpu_device_lost(), Some(true));
    assert!(renderer.shared_gpu_context().is_none());
    assert_eq!(renderer.gpu_device_lost(), Some(true));
    Ok(())
}

/// A6: the surface path reads nothing back and uploads no image: the GPU producer clears the
/// texture, the window composites it, and the only atlas (the sole CPU-to-GPU image upload
/// path) stays empty.
#[test]
fn surface_path_has_zero_readback_and_zero_image_upload() -> anyhow::Result<()> {
    // The copy-back comparison arm moves one 1280 x 720 RGBA frame each way every frame.
    assert_eq!(1280_u64 * 720 * 4, 3_686_400);

    let mut renderer = renderer()?;
    let producer = Producer::new(&renderer.shared_gpu_context().expect("healthy context"));
    for frame in 0..30 {
        producer.clear(wgpu::Color {
            r: f64::from(frame) / 30.0,
            g: 0.5,
            b: 0.25,
            a: 1.0,
        });
        let mut scene = producer.scene();
        scene.finish();
        renderer.render_scene(&scene, device_size(64, 64))?;
    }
    assert_eq!(renderer.readback_bytes(), 0, "no GPU readback per frame");
    assert!(
        !renderer.atlas_has_textures(),
        "no CPU-to-GPU image upload per frame"
    );

    // Both counters are live: a readback is counted, and an atlas upload is seen.
    let image = render(&mut renderer, &mut producer.scene(), 64, 64)?;
    assert_eq!(
        renderer.readback_bytes(),
        u64::from(image.width() * image.height() * 4)
    );
    let atlas = renderer.sprite_atlas();
    atlas.get_or_insert_with(
        AtlasKey::Image(RenderImageParams {
            image_id: ImageId(1),
            frame_index: 0,
        }),
        &mut || Ok(Some((device_size(2, 2), vec![255u8; 16].into()))),
    )?;
    assert!(
        renderer.atlas_has_textures(),
        "the probe sees an image tile upload"
    );
    Ok(())
}
