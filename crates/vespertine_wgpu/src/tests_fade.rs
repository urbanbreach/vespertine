//! GPU pixel tests of the scoped edge fade (RND-001 A1, A2, A5, A6, A7) through the headless
//! renderer. The scope parameters are built the way `Window::with_edge_fade` builds them:
//! device-space edge coordinates and device-space band widths (logical band × scale).

use crate::{
    WgpuContext, WgpuHeadlessRenderer,
    tests_scope::{bounds, device_size, quad},
};
use image::RgbaImage;
use std::borrow::Cow;
use vespertine::{
    AtlasKey, AtlasTile, ContentMask, Corners, DevicePixels, Edges, FontId, GlyphId, Hsla, ImageId,
    MonochromeSprite, PlatformHeadlessRenderer, Point, PolychromeSprite, RenderGlyphParams,
    RenderImageParams, RenderSvgParams, ScaledPixels, Scene, ScopeParams, Size, SubpixelSprite,
    TransformationMatrix, px, rgb, white,
};

/// Tolerance of one 8-bit unorm blend plus rounding, as a fraction of full scale.
const TOLERANCE: f32 = 2.5 / 255.0;

/// Edge fade landmarks of a single band (RND-001 parameters): 0%, 25%, 50%, 75%, 100%.
const LANDMARKS: [(f32, f32); 5] = [
    (0.0, 0.0),
    (0.25, 0.0625),
    (0.5, 0.25),
    (0.75, 0.5625),
    (1.0, 1.0),
];

/// (logical band, scale factor); the device band is their product.
const BAND_SCALES: [(f32, f32); 3] = [(40.0, 1.0), (20.0, 2.0), (12.0, 2.0)];

const TOP: usize = 0;
const BOTTOM: usize = 1;
const LEFT: usize = 2;
const RIGHT: usize = 3;

/// Device size of the probe image and the device coordinates of its four fade edges. The
/// edges sit on pixel centers minus a half pixel so landmark fragments are pixel centers.
const SIZE: i32 = 160;
const EDGE_NEAR: f32 = 8.5;
const EDGE_FAR: f32 = 151.5;

fn channel(image: &RgbaImage, x: u32, y: u32, channel: usize) -> f32 {
    f32::from(image.get_pixel(x, y).0[channel]) / 255.0
}

fn red(image: &RgbaImage, x: u32, y: u32) -> f32 {
    channel(image, x, y, 0)
}

/// Device-space scope with one fade edge enabled with `logical_band` at `scale`.
fn scope_for(edges: &[(usize, f32)], scale: f32) -> ScopeParams {
    let mut scope = ScopeParams {
        // Logical edge coordinates multiplied by the scale, as `EdgeFade::scaled` does.
        fade_edges: [
            EDGE_NEAR / scale * scale,
            EDGE_FAR / scale * scale,
            EDGE_NEAR / scale * scale,
            EDGE_FAR / scale * scale,
        ],
        ..Default::default()
    };
    for &(edge, logical_band) in edges {
        scope.fade_bands[edge] = logical_band * scale;
    }
    scope
}

fn render_white_square(scope: ScopeParams) -> RgbaImage {
    let mut scene = Scene::default();
    scene.insert_primitive(quad(
        bounds(0.0, 0.0, SIZE as f32, SIZE as f32),
        white(),
        scope,
    ));
    crate::tests_scope::render(&mut scene, SIZE, SIZE)
}

/// Pixel coordinates (x, y) of the landmark `through` (0..=1) of the band of `edge`, on the
/// middle line of the image.
fn landmark_pixel(edge: usize, device_band: f32, through: f32) -> (u32, u32) {
    let offset = (through * device_band).round() as i32;
    let mid = SIZE / 2;
    let (x, y) = match edge {
        TOP => (mid, (EDGE_NEAR - 0.5) as i32 + offset),
        BOTTOM => (mid, (EDGE_FAR - 0.5) as i32 - offset),
        LEFT => ((EDGE_NEAR - 0.5) as i32 + offset, mid),
        RIGHT => ((EDGE_FAR - 0.5) as i32 - offset, mid),
        _ => unreachable!(),
    };
    (x as u32, y as u32)
}

#[test]
fn a1_single_band_landmarks_on_every_edge_and_scale() {
    for &(logical_band, scale) in &BAND_SCALES {
        let device_band = logical_band * scale;
        for edge in [TOP, BOTTOM, LEFT, RIGHT] {
            let scope = scope_for(&[(edge, logical_band)], scale);
            let image = render_white_square(scope);
            for (through, expected) in LANDMARKS {
                let (x, y) = landmark_pixel(edge, device_band, through);
                let actual = red(&image, x, y);
                assert!(
                    (actual - expected).abs() <= TOLERANCE,
                    "edge {edge}, band {logical_band}x{scale}, {through}: got {actual}, want {expected}"
                );
                // The pure reference agrees with the landmark constants.
                let reference = scope.edge_coverage([x as f32 + 0.5, y as f32 + 0.5]);
                assert!(
                    (reference - expected).abs() < 1e-5,
                    "reference at {through}"
                );
            }
            // Disabled edges never attenuate: pixels two device pixels inside every other
            // image edge are untouched (beyond the band of the enabled edge).
            let mid = SIZE as u32 / 2;
            let far = SIZE as u32 - 3;
            for (other, x, y) in [
                (LEFT, 2, mid),
                (RIGHT, far, mid),
                (TOP, mid, 2),
                (BOTTOM, mid, far),
            ] {
                if other == edge {
                    continue;
                }
                assert!(
                    (red(&image, x, y) - 1.0).abs() <= TOLERANCE,
                    "disabled edge {other} attenuated at ({x}, {y}) with only {edge} enabled"
                );
            }
        }
    }
}

#[test]
fn a2_corner_combines_by_minimum_not_product() {
    for &(logical_band, scale) in &BAND_SCALES {
        let device_band = logical_band * scale;
        // Top-left corner, both normalized distances 0.5.
        let scope = scope_for(&[(TOP, logical_band), (LEFT, logical_band)], scale);
        let image = render_white_square(scope);
        let offset = (0.5 * device_band) as i32;
        let x = (EDGE_NEAR - 0.5) as i32 + offset;
        let actual = red(&image, x as u32, x as u32);
        assert!(
            (actual - 0.25).abs() <= TOLERANCE,
            "band {logical_band}x{scale}: {actual}, want 0.25 and not {}",
            0.0625
        );
        assert!((actual - 0.0625).abs() > 0.1);

        // Bottom-right corner with different bands: both distances 0.5.
        let band_bottom = logical_band;
        let band_right = logical_band / 2.0;
        let scope = scope_for(&[(BOTTOM, band_bottom), (RIGHT, band_right)], scale);
        let image = render_white_square(scope);
        let y = (EDGE_FAR - 0.5) as i32 - (0.5 * band_bottom * scale) as i32;
        let x = (EDGE_FAR - 0.5) as i32 - (0.5 * band_right * scale) as i32;
        let actual = red(&image, x as u32, y as u32);
        assert!(
            (actual - 0.25).abs() <= TOLERANCE,
            "mixed bands {band_bottom}/{band_right}x{scale}: {actual}"
        );
    }
}

// --- sprites --- //

struct Harness {
    renderer: WgpuHeadlessRenderer,
}

impl Harness {
    fn new() -> Self {
        Self {
            renderer: WgpuHeadlessRenderer::new().expect("headless renderer"),
        }
    }

    fn render(&mut self, scene: &mut Scene, width: i32, height: i32) -> RgbaImage {
        scene.finish();
        self.renderer
            .render_scene_to_image(scene, device_size(width, height))
            .expect("render")
    }

    fn insert(&self, key: AtlasKey, width: i32, height: i32, bytes: Vec<u8>) -> AtlasTile {
        let size = Size {
            width: DevicePixels(width),
            height: DevicePixels(height),
        };
        self.renderer
            .sprite_atlas()
            .get_or_insert_with(key, &mut || Ok(Some((size, Cow::Owned(bytes.clone())))))
            .expect("atlas insert")
            .expect("tile")
    }

    /// A monochrome (R8) tile; `coverage(x, y)` gives the 8-bit coverage.
    fn mono_tile(
        &self,
        name: &str,
        width: i32,
        height: i32,
        coverage: impl Fn(i32, i32) -> u8,
    ) -> AtlasTile {
        let bytes = (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .map(|(x, y)| coverage(x, y))
            .collect();
        self.insert(
            AtlasKey::Svg(RenderSvgParams {
                path: name.to_string().into(),
                size: Size {
                    width: DevicePixels(width),
                    height: DevicePixels(height),
                },
            }),
            width,
            height,
            bytes,
        )
    }

    /// A polychrome tile from straight BGRA bytes.
    fn image_tile(&self, id: usize, width: i32, height: i32, bgra: [u8; 4]) -> AtlasTile {
        let bytes = (0..width * height).flat_map(|_| bgra).collect();
        self.insert(
            AtlasKey::Image(RenderImageParams {
                image_id: ImageId(id),
                frame_index: 0,
            }),
            width,
            height,
            bytes,
        )
    }

    /// A subpixel (BGRA) tile where texel channels (r, g, b) are the three coverages.
    fn subpixel_tile(&self, id: u32, width: i32, height: i32, rgb_coverage: [u8; 3]) -> AtlasTile {
        let [r, g, b] = rgb_coverage;
        let bytes = (0..width * height).flat_map(|_| [b, g, r, 255]).collect();
        self.insert(
            AtlasKey::Glyph(RenderGlyphParams {
                font_id: FontId(0),
                glyph_id: GlyphId(id),
                font_size: px(16.0),
                subpixel_variant: Point::default(),
                scale_factor: 1.0,
                is_emoji: false,
                subpixel_rendering: true,
                dilation: 0,
            }),
            width,
            height,
            bytes,
        )
    }
}

fn mono_sprite(
    tile: AtlasTile,
    x: f32,
    y: f32,
    color: Hsla,
    scope: ScopeParams,
) -> MonochromeSprite {
    let area = bounds(
        x,
        y,
        tile.bounds.size.width.0 as f32,
        tile.bounds.size.height.0 as f32,
    );
    MonochromeSprite {
        order: 0,
        blur: 0.0,
        bounds: area,
        content_mask: ContentMask { bounds: area },
        color,
        tile,
        transformation: TransformationMatrix::unit(),
        scope,
    }
}

fn poly_sprite(tile: AtlasTile, x: f32, y: f32, scope: ScopeParams) -> PolychromeSprite {
    let area = bounds(
        x,
        y,
        tile.bounds.size.width.0 as f32,
        tile.bounds.size.height.0 as f32,
    );
    PolychromeSprite {
        order: 0,
        pad: 0,
        grayscale: false.into(),
        opacity: 1.0,
        bounds: area,
        content_mask: ContentMask { bounds: area },
        corner_radii: Corners::default(),
        tile,
        scope,
        effects: Default::default(),
    }
}

fn subpixel_sprite(
    tile: AtlasTile,
    x: f32,
    y: f32,
    color: Hsla,
    scope: ScopeParams,
) -> SubpixelSprite {
    let area = bounds(
        x,
        y,
        tile.bounds.size.width.0 as f32,
        tile.bounds.size.height.0 as f32,
    );
    SubpixelSprite {
        order: 0,
        pad: 0,
        bounds: area,
        content_mask: ContentMask { bounds: area },
        color,
        tile,
        transformation: TransformationMatrix::unit(),
        scope,
    }
}

fn top_fade(edge: f32, band: f32) -> ScopeParams {
    ScopeParams {
        fade_edges: [edge, 0.0, 0.0, 0.0],
        fade_bands: [band, 0.0, 0.0, 0.0],
        ..Default::default()
    }
}

/// Asserts that a column of pixels follows `expected(y)` exactly, ramps continuously (no
/// adjacent-row step larger than the steepest slope of the curve) and returns the column.
fn assert_continuous_ramp(
    image: &RgbaImage,
    x: u32,
    rows: std::ops::Range<u32>,
    expected: impl Fn(u32) -> f32,
    max_step: f32,
    what: &str,
) {
    let mut previous: Option<f32> = None;
    for y in rows {
        let actual = red(image, x, y);
        let want = expected(y);
        assert!(
            (actual - want).abs() <= TOLERANCE,
            "{what}: row {y}: got {actual}, want {want}"
        );
        if let Some(previous) = previous {
            assert!(
                (actual - previous).abs() <= max_step + TOLERANCE,
                "{what}: step {previous} -> {actual} at row {y} exceeds {max_step}"
            );
        }
        previous = Some(actual);
    }
}

#[test]
fn a5_large_image_and_glyph_fade_per_pixel_with_no_primitive_wide_step() {
    // One edge across the middle of each sprite: edge at y = 64.5, band 48 device pixels,
    // content spans y = 0..192 so it is on both sides of the edge and the whole band.
    let mut harness = Harness::new();
    let band = 48.0;
    let edge = 64.5;
    let scope = top_fade(edge, band);
    let ramp = |y: u32| {
        let u = ((y as f32 + 0.5 - edge) / band).clamp(0.0, 1.0);
        u * u
    };
    // Steepest slope of u² over the band is 2 / band per pixel.
    let max_step = 2.0 / band;

    let image_tile = harness.image_tile(1, 32, 192, [255, 255, 255, 255]);
    let glyph_tile = harness.mono_tile("a5-glyph", 32, 192, |x, _| {
        if (8..24).contains(&x) { 255 } else { 0 }
    });
    let mut scene = Scene::default();
    scene.insert_primitive(poly_sprite(image_tile, 0.0, 0.0, scope));
    scene.insert_primitive(mono_sprite(glyph_tile, 48.0, 0.0, white(), scope));
    let image = harness.render(&mut scene, 96, 192);

    // The image is one primitive crossing the edge: a continuous ramp, nothing primitive-wide.
    assert_continuous_ramp(&image, 16, 0..192, ramp, max_step, "image");
    // Away from the edge the image is fully visible, and fully transparent beyond the edge.
    assert!(
        red(&image, 16, 191) > 0.99,
        "image must stay visible far from the edge"
    );
    assert!(
        red(&image, 16, 10) < 0.01,
        "image beyond the edge is transparent"
    );
    assert!(
        (red(&image, 16, 100) - ramp(100)).abs() <= TOLERANCE && red(&image, 16, 100) > 0.0,
        "rows inside the band show partial alpha"
    );

    // The glyph-like mask: the stem follows the same ramp scaled by its full coverage.
    let stem = red(&image, 48 + 16, 191);
    assert!(
        stem > 0.99,
        "fully covered stem must reach full alpha: {stem}"
    );
    assert_continuous_ramp(
        &image,
        48 + 16,
        0..192,
        |y| ramp(y) * stem,
        max_step,
        "glyph",
    );
    // Outside the stem columns the mask stays empty at every row.
    for y in [0, 80, 191] {
        assert!(
            red(&image, 48 + 2, y) < 0.01,
            "glyph outside stem at row {y}"
        );
    }
}

#[test]
fn a5_image_ramp_follows_horizontal_edges_too() {
    let mut harness = Harness::new();
    let tile = harness.image_tile(2, 128, 16, [255, 255, 255, 255]);
    let band = 40.0;
    // Right edge fade: right = 100.5, band 40; sprite is 128 wide so it crosses the edge.
    let scope = ScopeParams {
        fade_edges: [0.0, 0.0, 0.0, 100.5],
        fade_bands: [0.0, 0.0, 0.0, band],
        ..Default::default()
    };
    let mut scene = Scene::default();
    scene.insert_primitive(poly_sprite(tile, 0.0, 0.0, scope));
    let image = harness.render(&mut scene, 128, 16);
    for x in 0..128u32 {
        let u = ((100.5 - (x as f32 + 0.5)) / band).clamp(0.0, 1.0);
        assert!(
            (red(&image, x, 8) - u * u).abs() <= TOLERANCE,
            "column {x}: {} vs {}",
            red(&image, x, 8),
            u * u
        );
    }
}

#[test]
fn a6_monochrome_neutral_block_is_bit_identical_to_factor_one() {
    // A zeroed block is the neutral state. A block with an active but fully-open fade
    // (every fragment has F = 1) must produce exactly the same bytes for every
    // primitive class, which also pins that the neutral state adds no attenuation.
    let mut harness = Harness::new();
    let neutral = ScopeParams::default();
    // Edges far away: E = 1 everywhere in the 96x96 target.
    let open = ScopeParams {
        fade_edges: [-1000.0, 1000.0, -1000.0, 1000.0],
        fade_bands: [8.0, 8.0, 8.0, 8.0],
        ..Default::default()
    };
    let build = |harness: &Harness, scope: ScopeParams| {
        let mono = harness.mono_tile("a6-mono", 24, 24, |x, y| ((x * 10 + y * 3) % 256) as u8);
        let color = harness.image_tile(10, 24, 24, [40, 120, 220, 255]);
        let mut scene = Scene::default();
        scene.insert_primitive(mono_sprite(mono, 4.0, 4.0, rgb(0xE0A040).into(), scope));
        scene.insert_primitive(poly_sprite(color, 40.0, 4.0, scope));
        scene.insert_primitive(quad(
            bounds(4.0, 40.0, 40.0, 40.0),
            Hsla::from(rgb(0x3060C0)),
            scope,
        ));
        scene
    };
    let mut baseline_scene = build(&harness, neutral);
    let baseline = harness.render(&mut baseline_scene, 96, 96);
    let mut open_scene = build(&harness, open);
    let open_image = harness.render(&mut open_scene, 96, 96);
    assert_eq!(
        baseline.as_raw(),
        open_image.as_raw(),
        "an open fade must be bit-identical to the neutral block"
    );
    // And the neutral render does contain the content (it is not trivially blank).
    assert!(baseline.pixels().any(|pixel| pixel.0[0] > 30));
}

/// Whether the headless adapter exposes dual-source blending (needed for subpixel text).
fn adapter_has_dual_source_blending() -> bool {
    WgpuContext::new_headless()
        .expect("headless context")
        .0
        .supports_dual_source_blending()
}

#[test]
fn a6_subpixel_keeps_channel_ordering_and_neutral_block_is_unchanged() {
    let mut harness = Harness::new();
    if !adapter_has_dual_source_blending() {
        // Without dual-source blending the renderer builds no subpixel pipeline and
        // subpixel text is disabled by the platform; the monochrome neutral-state
        // equivalence above is the only subpixel-adjacent coverage possible here.
        assert!(!adapter_has_dual_source_blending());
        eprintln!("adapter lacks dual-source blending: subpixel half of A6 not applicable");
        return;
    }
    // Distinct per-channel coverage so channel order is observable.
    let coverage = [230u8, 128, 51];
    let tile = harness.subpixel_tile(7, 16, 64, coverage);
    let edge = 8.5;
    let band = 40.0;
    let faded = top_fade(edge, band);

    let render = |harness: &mut Harness, scope: ScopeParams| {
        let mut scene = Scene::default();
        scene.insert_primitive(subpixel_sprite(tile, 0.0, 0.0, white(), scope));
        harness.render(&mut scene, 16, 64)
    };
    let plain = render(&mut harness, ScopeParams::default());
    let open = render(
        &mut harness,
        ScopeParams {
            fade_edges: [-1000.0, 1000.0, -1000.0, 1000.0],
            fade_bands: [8.0, 8.0, 8.0, 8.0],
            ..Default::default()
        },
    );
    assert_eq!(
        plain.as_raw(),
        open.as_raw(),
        "neutral and fully open parameter blocks must give identical subpixel output"
    );
    let reference = plain.get_pixel(8, 32).0;
    assert!(
        reference[0] > reference[1] && reference[1] > reference[2] && reference[2] > 0,
        "unscoped subpixel channels keep their coverage order: {reference:?}"
    );

    let image = render(&mut harness, faded);
    for y in 0..64u32 {
        let u = ((y as f32 + 0.5 - edge) / band).clamp(0.0, 1.0);
        let factor = u * u;
        let pixel = image.get_pixel(8, y).0;
        for channel in 0..3 {
            let want = f32::from(reference[channel]) / 255.0 * factor;
            let actual = f32::from(pixel[channel]) / 255.0;
            assert!(
                (actual - want).abs() <= TOLERANCE,
                "row {y} channel {channel}: {actual} vs {want} (factor {factor})"
            );
        }
        if factor > 0.2 {
            assert!(
                pixel[0] > pixel[1] && pixel[1] > pixel[2],
                "channel order lost at row {y}: {pixel:?}"
            );
        }
    }
}

// --- quad routes (A7) --- //

#[test]
fn a7_quad_routes_receive_the_factor_the_spec_gives_them() {
    // One probe row (y = 64) where the top-edge ramp gives F = 0.5 exactly.
    let device_band = 100.0f32;
    let edge = 64.5 - device_band * std::f32::consts::FRAC_1_SQRT_2;
    let fade = top_fade(edge, device_band);
    assert!((fade.factor([10.0, 64.5]) - 0.5).abs() < 1e-5);
    let row = 64u32;

    let blue = rgb(0x0000FF);
    let mut scene = Scene::default();

    // (a) rounded, bordered quad: tall, probe row is far from every corner.
    let mut bordered = quad(bounds(0.0, 0.0, 64.0, 128.0), white(), fade);
    bordered.corner_radii = Corners::all(ScaledPixels(24.0));
    bordered.border_widths = Edges::all(ScaledPixels(4.0));
    bordered.border_color = blue.into();
    scene.insert_primitive(bordered);

    // (b) rounded fill-only quad whose top edge is 8 pixels above the probe row: pixel
    // x = 92 lies inside the corner region and inside the curve.
    let mut rounded = quad(bounds(80.0, 56.0, 64.0, 64.0), white(), fade);
    rounded.corner_radii = Corners::all(ScaledPixels(24.0));
    scene.insert_primitive(rounded);

    // (c) unrounded, borderless rectangle inside the scope.
    scene.insert_primitive(quad(bounds(160.0, 0.0, 64.0, 128.0), white(), fade));

    let image = crate::tests_scope::render(&mut scene, 224, 128);

    // (a) interior fast return: F once; border colour: 0.5 (blue channel, no red).
    assert!(
        (red(&image, 32, row) - 0.5).abs() <= TOLERANCE,
        "bordered interior fill"
    );
    assert!(
        (channel(&image, 1, row, 2) - 0.5).abs() <= TOLERANCE,
        "border colour receives F"
    );
    assert!(
        red(&image, 1, row) < TOLERANCE,
        "border keeps its own colour"
    );
    // (b) general route fill inside the corner region: F^2 = 0.25 before outer antialiasing.
    assert!(
        (red(&image, 92, row) - 0.25).abs() <= TOLERANCE,
        "rounded fill: {}",
        red(&image, 92, row)
    );
    // (c) unrounded borderless: 0.5.
    assert!(
        (red(&image, 192, row) - 0.5).abs() <= TOLERANCE,
        "unrounded borderless"
    );

    // Plain solid quad with no active band receives 1, even when a rounded clip scope that
    // would otherwise cut it is active (RND-002 R6).
    let rounded_only = ScopeParams {
        clip_bounds: [4.0, 4.0, 60.0, 60.0],
        clip_radius: 20.0,
        ..Default::default()
    };
    let mut plain_scene = Scene::default();
    plain_scene.insert_primitive(quad(
        bounds(0.0, 0.0, 64.0, 128.0),
        white(),
        ScopeParams::default(),
    ));
    plain_scene.insert_primitive(quad(bounds(80.0, 0.0, 64.0, 64.0), white(), rounded_only));
    let image = crate::tests_scope::render(&mut plain_scene, 144, 128);
    assert!(
        (red(&image, 32, row) - 1.0).abs() <= TOLERANCE,
        "plain solid, no scope"
    );
    assert!(
        (red(&image, 82, 2) - 1.0).abs() <= TOLERANCE,
        "plain solid ignores the clip"
    );
}

// --- production-path crossing of one band by every primitive class (A8) --- //

const A8_ICON: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="140" viewBox="0 0 40 140"><rect width="40" height="140" fill="black"/></svg>"#;

/// Column pitch and width, in logical pixels, of the A8 scene.
const COLUMN_PITCH: f32 = 70.0;
const COLUMN_WIDTH: f32 = 40.0;

/// Every primitive class painted through the real element and Window paint paths, one per
/// column, each spanning logical y = 0..140 of the container whose top edge is the fade
/// edge. With `faded` the container is wrapped so that its top edge fades over a 40 logical
/// pixel band (80 device pixels at scale factor 2); without it the same tree is painted
/// unscoped, giving the unfaded reference pixels.
fn a8_scene(image: std::sync::Arc<vespertine::RenderImage>, faded: bool) -> vespertine::AnyElement {
    use vespertine::{
        BoxShadow, IntoElement as _, ParentElement as _, PathBuilder, Styled as _, UnderlineStyle,
        canvas, div, edge_faded, img, point, size,
    };
    let column = |index: usize| px(10.0 + COLUMN_PITCH * index as f32);
    let content = div()
        .relative()
        .size_full()
        // 0: one large image.
        .child(
            img(image)
                .absolute()
                .left(column(0))
                .top(px(0.0))
                .w(px(COLUMN_WIDTH))
                .h(px(140.0)),
        )
        // 1: one tall glyph.
        .child(
            div()
                .absolute()
                .left(column(1))
                .top(px(0.0))
                .w(px(COLUMN_WIDTH))
                .h(px(140.0))
                .text_size(px(150.0))
                .line_height(px(140.0))
                .text_color(white())
                .child("I"),
        )
        // 2..7: a single bordered quad, vector icon, paths, shadow and underline through the
        // Window paint API (an element paints its fill and its border as two quads).
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, cx| {
                    let at = |index: usize, x: f32, y: f32| {
                        point(
                            bounds.origin.x + column(index) + px(x),
                            bounds.origin.y + px(y),
                        )
                    };
                    window.paint_quad(vespertine::quad(
                        vespertine::Bounds::new(at(2, 0.0, 0.0), size(px(COLUMN_WIDTH), px(140.0))),
                        vespertine::Corners::all(px(0.0)),
                        rgb(0xC03030),
                        vespertine::Edges::all(px(4.0)),
                        rgb(0x3050E0),
                        vespertine::BorderStyle::Solid,
                    ));
                    window
                        .paint_svg(
                            vespertine::Bounds::new(
                                at(3, 0.0, 0.0),
                                size(px(COLUMN_WIDTH), px(140.0)),
                            ),
                            "a8-icon".into(),
                            Some(A8_ICON.as_bytes()),
                            TransformationMatrix::unit(),
                            white(),
                            cx,
                        )
                        .expect("paint svg");
                    let triangle = |index: usize, top: f32| {
                        let mut builder = PathBuilder::fill();
                        builder.move_to(at(index, 0.0, top));
                        builder.line_to(at(index, COLUMN_WIDTH, top));
                        builder.line_to(at(index, 0.0, 140.0));
                        builder.close();
                        builder.build().expect("path")
                    };
                    // Box touching the fade edge, and a box 20 px (half a band) inside.
                    window.paint_path(triangle(4, 0.0), white());
                    window.paint_path(triangle(5, 20.0), white());
                    window.paint_drop_shadows(
                        vespertine::Bounds::new(at(6, 0.0, 0.0), size(px(30.0), px(140.0))),
                        vespertine::Corners::all(px(0.0)),
                        &[BoxShadow {
                            color: white(),
                            offset: point(px(0.0), px(0.0)),
                            blur_radius: px(8.0),
                            spread_radius: px(0.0),
                            inset: false,
                        }],
                    );
                    window.paint_underline(
                        at(7, 0.0, 20.0),
                        px(COLUMN_WIDTH),
                        &UnderlineStyle {
                            thickness: px(24.0),
                            color: Some(white()),
                            wavy: false,
                        },
                    );
                },
            )
            .absolute()
            .inset_0(),
        );
    let content = if faded {
        edge_faded(px(40.0), true, false, content).into_any_element()
    } else {
        content.into_any_element()
    };
    div()
        .size_full()
        .bg(vespertine::black())
        .child(
            div()
                .absolute()
                .top(px(20.0))
                .left(px(0.0))
                .w(px(10.0 + COLUMN_PITCH * 8.0))
                .h(px(140.0))
                .child(content),
        )
        .into_any_element()
}

#[test]
fn a8_every_primitive_class_crosses_one_band_per_fragment_or_conservatively() {
    use std::sync::Arc;
    let image = Arc::new(vespertine::RenderImage::new(vec![image::Frame::new(
        RgbaImage::from_pixel(40, 140, image::Rgba([255; 4])),
    )]));
    let mut host = crate::tests_host::RenderHost::new();
    let (width, height) = (10.0 + COLUMN_PITCH * 8.0, 160.0);
    let unfaded = host.render_logical(width, height, {
        let image = image.clone();
        move |_, _| a8_scene(image.clone(), false)
    });
    let faded = host.render_logical(width, height, move |_, _| a8_scene(image.clone(), true));
    assert_eq!(faded.dimensions(), ((2.0 * width) as u32, 320));

    // Container top edge: logical y = 20, device y = 40; band 40 logical = 80 device.
    let ramp = |y: u32| {
        let u = ((y as f32 + 0.5 - 40.0) / 80.0).clamp(0.0, 1.0);
        u * u
    };
    // Per-fragment classes follow the squared ramp pixel by pixel; paths and shadows whose
    // box touches the edge are fully transparent; a path half a band inside and a thick
    // underline starting half a band inside get one uniform factor 0.25 over rows whose
    // per-fragment factor would range from 0.25 to 1.
    // The bordered quad takes the general route: border and interior (the fast return)
    // receive F once, but the one-pixel ring just inside the border (where the border's
    // inner antialiasing term leaves the fill alone) goes through the double attenuation
    // of R11c and receives F squared. Quad: device y 40..320, border 8 device pixels.
    let quad_left = (2.0 * (10.0 + 2.0 * COLUMN_PITCH)) as u32;
    let quad_right = quad_left + (2.0 * COLUMN_WIDTH) as u32;
    let bordered = move |x: u32, y: u32| {
        let (inner_left, inner_right) = (quad_left + 8, quad_right - 8);
        let ring = (y >= 48 && y < 312 && (x == inner_left || x == inner_right - 1))
            || (x >= inner_left && x < inner_right && (y == 48 || y == 311));
        if ring { ramp(y) * ramp(y) } else { ramp(y) }
    };
    // The conservative factor 0.25 is multiplied into the underline color's alpha. The
    // renderer's non-wavy underline shader (unchanged upstream behavior) applies its color
    // alpha twice, so the composited result of alpha 0.25 on a white underline is 0.25^2.
    const UNDERLINE_RESPONSE: f32 = 0.25 * 0.25;
    let columns: [(&str, usize, &dyn Fn(u32, u32) -> f32); 8] = [
        ("image", 0, &|_, y| ramp(y)),
        ("glyph", 1, &|_, y| ramp(y)),
        ("bordered quad", 2, &bordered),
        ("vector icon", 3, &|_, y| ramp(y)),
        ("path touching the edge", 4, &|_, _| 0.0),
        ("path half a band inside", 5, &|_, _| 0.25),
        ("shadow of a box touching the edge", 6, &|_, _| 0.0),
        ("underline at 0.5 of the band", 7, &|_, _| {
            UNDERLINE_RESPONSE
        }),
    ];
    for (name, index, predicted) in columns {
        let left = ((10.0 + COLUMN_PITCH * index as f32 - 12.0) * 2.0) as u32;
        let right = ((10.0 + COLUMN_PITCH * index as f32 + COLUMN_WIDTH + 12.0) * 2.0) as u32;
        let mut visible_unfaded = 0usize;
        let mut worst = 0.0f32;
        for y in 40..320u32 {
            for x in left..right {
                let reference = unfaded.get_pixel(x, y).0;
                let actual = faded.get_pixel(x, y).0;
                if reference[0] > 64 {
                    visible_unfaded += 1;
                }
                for channel in 0..3 {
                    let want = f32::from(reference[channel]) / 255.0 * predicted(x, y);
                    let got = f32::from(actual[channel]) / 255.0;
                    worst = worst.max((got - want).abs());
                    assert!(
                        (got - want).abs() <= 3.5 / 255.0,
                        "{name}: pixel ({x}, {y}) channel {channel}: {got} vs {want}; faded {actual:?} unfaded {reference:?} predicted {}",
                        predicted(x, y)
                    );
                }
            }
        }
        assert!(
            visible_unfaded > 200,
            "{name}: the unfaded reference shows content"
        );
        eprintln!("a8 {name}: worst deviation {worst}");
    }

    // Continuity and visibility away from the edge for the image: a ramp inside the band
    // and full alpha below it.
    let image_column = 2 * 30;
    let value = |y: u32| f32::from(faded.get_pixel(image_column, y).0[0]) / 255.0;
    assert!(value(41) < 0.01);
    assert!((value(90) - ramp(90)).abs() <= TOLERANCE && value(90) > 0.3 && value(90) < 0.9);
    assert!(
        value(250) > 0.99,
        "the image stays visible away from the edge"
    );
    // The underline's rows: one uniform value where per-fragment evaluation would vary from
    // E = 0.25 to 1 over the same rows.
    let underline_x = (2.0 * (10.0 + 7.0 * COLUMN_PITCH + 20.0)) as u32;
    let rows: Vec<f32> = (80..128u32)
        .map(|y| f32::from(faded.get_pixel(underline_x, y).0[0]) / 255.0)
        .collect();
    assert!(
        rows.iter()
            .all(|row| (row - UNDERLINE_RESPONSE).abs() <= 3.5 / 255.0),
        "{rows:?}"
    );
}
