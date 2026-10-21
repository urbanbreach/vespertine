//! Pixel tests of the composite-time glyph blur and transform (RND-005): primitive-level
//! tests through synthetic atlas tiles, and production-path tests that shape and paint real
//! glyphs through `Window::paint_glyph_transformed`.
//!
//! Coverage values are read from the red channel of white text over a black target; the
//! renderer's contrast/gamma correction is measured once per test as a 256-entry table so
//! expectations can be stated on coverage.

use crate::{
    WgpuHeadlessRenderer,
    tests_host::RenderHost,
    tests_scope::{bounds, device_size},
};
use image::RgbaImage;
use std::{borrow::Cow, sync::Arc};
use vespertine::{
    AtlasKey, AtlasTile, Bounds, ContentMask, FontId, GlyphId, Hsla, MonochromeSprite, Pixels,
    PlatformAtlas, PlatformHeadlessRenderer, Point, RenderSvgParams, ScaledPixels, Scene,
    ScopeParams, TransformationMatrix, blur_expanded_bounds, blur_halo, blur_kernel_radius,
    blurred_coverage, point, px, radians, white,
};

const BIG: f32 = 4096.0;

struct Harness {
    renderer: WgpuHeadlessRenderer,
    atlas: Arc<dyn PlatformAtlas>,
    serial: u32,
}

impl Harness {
    fn new() -> Self {
        let renderer = WgpuHeadlessRenderer::new().expect("headless renderer");
        let atlas = renderer.sprite_atlas();
        Self {
            renderer,
            atlas,
            serial: 0,
        }
    }

    /// Inserts a monochrome tile whose texel `(x, y)` is `texel(x, y)`.
    fn tile(&mut self, width: u32, height: u32, texel: impl Fn(u32, u32) -> u8) -> AtlasTile {
        self.serial += 1;
        let size = device_size(width as i32, height as i32);
        let key = AtlasKey::Svg(RenderSvgParams {
            path: format!("glyph-blur-test-tile-{}", self.serial).into(),
            size,
        });
        let mut bytes = Vec::new();
        for y in 0..height {
            for x in 0..width {
                bytes.push(texel(x, y));
            }
        }
        self.atlas
            .get_or_insert_with(key, &mut || Ok(Some((size, Cow::Owned(bytes.clone())))))
            .expect("insert tile")
            .expect("tile")
    }

    fn render(&mut self, sprites: &[MonochromeSprite], width: u32, height: u32) -> RgbaImage {
        let mut scene = Scene::default();
        for sprite in sprites {
            scene.insert_primitive(*sprite);
        }
        scene.finish();
        self.renderer
            .render_scene_to_image(&scene, device_size(width as i32, height as i32))
            .expect("render")
    }

    /// The measured response of the shader's contrast and gamma correction for white text:
    /// entry `i` is the output for mask coverage `i / 255`.
    fn correction_table(&mut self) -> Vec<f32> {
        let tile = self.tile(256, 1, |x, _| x as u8);
        let image = self.render(&[sprite(&tile, (0.0, 0.0), 0.0)], 256, 1);
        (0..256).map(|x| coverage(&image, x, 0)).collect()
    }
}

fn corrected(table: &[f32], coverage: f32) -> f32 {
    let position = coverage.clamp(0.0, 1.0) * 255.0;
    let index = position.floor() as usize;
    let next = (index + 1).min(255);
    table[index] + (table[next] - table[index]) * (position - index as f32)
}

fn coverage(image: &RgbaImage, x: u32, y: u32) -> f32 {
    f32::from(image.get_pixel(x, y).0[0]) / 255.0
}

/// A white sprite showing `tile` with its own top-left corner at `origin` (device pixels)
/// and blur `sigma`, with the halo-expanded bounds `Window::paint_glyph_transformed` records.
fn sprite(tile: &AtlasTile, origin: (f32, f32), sigma: f32) -> MonochromeSprite {
    let own = bounds(
        origin.0,
        origin.1,
        tile.bounds.size.width.0 as f32,
        tile.bounds.size.height.0 as f32,
    );
    MonochromeSprite {
        order: 0,
        blur: sigma,
        bounds: blur_expanded_bounds(own, sigma),
        content_mask: ContentMask {
            bounds: bounds(0.0, 0.0, BIG, BIG),
        },
        color: white(),
        tile: *tile,
        transformation: TransformationMatrix::unit(),
        scope: ScopeParams::default(),
    }
}

fn rotation_about(pivot: (f32, f32), degrees: f32) -> TransformationMatrix {
    TransformationMatrix::unit()
        .translate(point(ScaledPixels(pivot.0), ScaledPixels(pivot.1)))
        .rotate(radians(degrees.to_radians()))
        .translate(point(ScaledPixels(-pivot.0), ScaledPixels(-pivot.1)))
}

fn assert_close(actual: f32, expected: f32, tolerance: f32, what: &str) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{what}: rendered {actual}, expected {expected}"
    );
}

const TOLERANCE: f32 = 3.0 / 255.0;

fn pattern(x: u32, y: u32) -> u8 {
    ((x * 53 + y * 97 + 31) % 211) as u8 + 20
}

#[test]
fn identity_transform_zero_blur_is_pixel_identical_to_ordinary_sampling() {
    // A1: blur 0 (and any negative blur) takes the ordinary path with the ordinary tile
    // mapping, whatever the transformation arithmetic around it.
    let mut harness = Harness::new();
    let tile = harness.tile(9, 7, pattern);
    let ordinary = harness.render(&[sprite(&tile, (5.0, 4.0), 0.0)], 24, 20);
    let mut negative = sprite(&tile, (5.0, 4.0), 0.0);
    negative.blur = -4.0;
    assert_eq!(harness.render(&[negative], 24, 20), ordinary);
    let mut composed = sprite(&tile, (5.0, 4.0), 0.0);
    composed.transformation = TransformationMatrix::unit()
        .translate(point(ScaledPixels(3.0), ScaledPixels(-2.0)))
        .translate(point(ScaledPixels(-3.0), ScaledPixels(2.0)));
    assert_eq!(harness.render(&[composed], 24, 20), ordinary);

    // The ink is where the unexpanded bounds say it is: one texel per pixel.
    let table = harness.correction_table();
    for y in 0..7 {
        for x in 0..9 {
            assert_close(
                coverage(&ordinary, 5 + x, 4 + y),
                corrected(&table, f32::from(pattern(x, y)) / 255.0),
                TOLERANCE,
                "ordinary texel",
            );
        }
    }
    assert_eq!(coverage(&ordinary, 4, 4), 0.0);
    assert_eq!(coverage(&ordinary, 14, 4), 0.0);
}

#[test]
fn oversampled_mask_keeps_its_mapping_without_and_with_blur() {
    // A1 / R6: a 16 by 16 texel tile shown at 8 by 8 pixels. Without blur the sampling is
    // the ordinary bilinear one; with blur the halo must not scale the content, and taps
    // are measured in tile texels (R8), so the apparent blur is sigma texels.
    let mut harness = Harness::new();
    let table = harness.correction_table();
    let tile = harness.tile(16, 16, pattern);
    let texel = |x: u32, y: u32| f32::from(pattern(x, y)) / 255.0;
    for sigma in [0.0_f32, 1.0, 2.0] {
        // A fractional origin keeps every tap off the tile boundary (the inclusive range
        // test is ill-conditioned exactly there).
        let origin = (20.25, 12.25);
        let (size, view) = (8.0, 48);
        let mut oversampled = sprite(&tile, origin, sigma);
        oversampled.bounds = blur_expanded_bounds(bounds(origin.0, origin.1, size, size), sigma);
        let image = harness.render(&[oversampled], view, view);
        let halo = blur_halo(sigma);
        let (left, top) = (origin.0 - halo, origin.1 - halo);
        let extent = size + 2.0 * halo;
        for y in 0..view {
            for x in 0..view {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let inside = px > left && px < left + extent && py > top && py < top + extent;
                let actual = coverage(&image, x, y);
                if !inside {
                    assert_eq!(actual, 0.0, "sigma {sigma}: ({x}, {y}) outside the quad");
                    continue;
                }
                // Tile texel coordinates of the fragment: the content keeps its size.
                let center = [(px - origin.0) * 2.0, (py - origin.1) * 2.0];
                let expected = corrected(&table, blurred_coverage(sigma, center, [16, 16], texel));
                assert_close(
                    actual,
                    expected,
                    TOLERANCE,
                    &format!("sigma {sigma} ({x}, {y})"),
                );
            }
        }
    }
}

#[test]
fn neighboring_atlas_tile_never_shows_in_the_halo() {
    // A3: a solid tile is allocated first; the transparent tile that follows it shares the
    // atlas texture and touches it. No pixel of the blurred transparent tile, including the
    // halo and the taps just beyond the tile boundary, may pick up the neighbor.
    let mut harness = Harness::new();
    let solid = harness.tile(8, 8, |_, _| 255);
    let empty = harness.tile(8, 8, |_, _| 0);
    assert_eq!(solid.texture_id, empty.texture_id);
    let expanded = |tile: &AtlasTile| {
        let rect = tile.bounds;
        (
            rect.origin.x.0 - 1,
            rect.origin.y.0 - 1,
            rect.origin.x.0 + rect.size.width.0 + 1,
            rect.origin.y.0 + rect.size.height.0 + 1,
        )
    };
    let (a, b) = (expanded(&solid), expanded(&empty));
    assert!(
        a.0 < b.2 && b.0 < a.2 && a.1 < b.3 && b.1 < a.3,
        "tiles must touch for the test to mean anything: {solid:?} {empty:?}"
    );
    for sigma in [0.5_f32, 2.0, 5.0, 8.0] {
        let image = harness.render(&[sprite(&empty, (40.0, 40.0), sigma)], 96, 96);
        assert!(
            image.pixels().all(|pixel| pixel.0[0] == 0),
            "sigma {sigma}: the neighbor leaked into the halo"
        );
    }
    // Control: the solid neighbor itself, blurred the same way, is visible.
    let image = harness.render(&[sprite(&solid, (40.0, 40.0), 2.0)], 96, 96);
    assert!(coverage(&image, 44, 44) > 0.3);
}

#[test]
fn edge_taps_fade_to_transparent_and_match_the_kernel_reference() {
    // A4 and R7-R8: every pixel of a blurred patterned tile matches the Gaussian reference,
    // which divides by the whole neighborhood's weight; the solid tile's border therefore
    // fades instead of brightening.
    let mut harness = Harness::new();
    let table = harness.correction_table();
    let origin = (24.0, 20.0);
    for sigma in [1.0_f32, 2.5] {
        let tile = harness.tile(8, 8, pattern);
        let image = harness.render(&[sprite(&tile, origin, sigma)], 64, 56);
        let halo = blur_halo(sigma);
        let texel = |x: u32, y: u32| f32::from(pattern(x, y)) / 255.0;
        for y in 0..56 {
            for x in 0..64 {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let inside = px > origin.0 - halo
                    && px < origin.0 + 8.0 + halo
                    && py > origin.1 - halo
                    && py < origin.1 + 8.0 + halo;
                let actual = coverage(&image, x, y);
                if !inside {
                    assert_eq!(actual, 0.0);
                    continue;
                }
                let center = [px - origin.0, py - origin.1];
                let expected = corrected(&table, blurred_coverage(sigma, center, [8, 8], texel));
                assert_close(
                    actual,
                    expected,
                    TOLERANCE,
                    &format!("sigma {sigma} ({x}, {y})"),
                );
            }
        }
    }

    let solid = harness.tile(8, 8, |_, _| 255);
    let sigma = 2.0;
    let image = harness.render(&[sprite(&solid, origin, sigma)], 64, 56);
    let center = coverage(&image, 28, 24);
    let edge_inside = coverage(&image, 24, 24);
    let corner_inside = coverage(&image, 24, 20);
    let beyond = coverage(&image, 22, 24);
    assert!(
        edge_inside < 0.8 * center,
        "edge {edge_inside}, center {center}"
    );
    assert!(corner_inside < 0.8 * edge_inside, "corner {corner_inside}");
    assert!(beyond > 0.0 && beyond < edge_inside, "halo {beyond}");
    assert_eq!(blur_kernel_radius(sigma), 6);
}

#[test]
fn large_sigma_keeps_the_tap_cap_while_the_halo_grows() {
    // A6: device sigma 8 (halo 24, radius 12) and 5 (halo 15, radius 12): the quad is
    // inflated by 3 sigma on every side while the kernel radius stays 12 (the reference
    // `blurred_coverage` uses the same cap, so agreement shows the shader does too).
    let mut harness = Harness::new();
    let table = harness.correction_table();
    for (sigma, halo) in [(8.0_f32, 24.0_f32), (5.0, 15.0)] {
        assert_eq!(blur_halo(sigma), halo);
        assert_eq!(blur_kernel_radius(sigma), 12);
        let tile = harness.tile(10, 10, pattern);
        let origin = (40.0, 40.0);
        let size = 100;
        let image = harness.render(&[sprite(&tile, origin, sigma)], size, size);
        let texel = |x: u32, y: u32| f32::from(pattern(x, y)) / 255.0;
        let (low, high) = (origin.0 - halo, origin.0 + 10.0 + halo);
        for y in 0..size {
            for x in 0..size {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let inside = px > low && px < high && py > low && py < high;
                let actual = coverage(&image, x, y);
                if !inside {
                    assert_eq!(actual, 0.0, "sigma {sigma}: ({x}, {y}) outside the halo");
                    continue;
                }
                let center = [px - origin.0, py - origin.1];
                let expected = corrected(&table, blurred_coverage(sigma, center, [10, 10], texel));
                assert_close(
                    actual,
                    expected,
                    TOLERANCE,
                    &format!("sigma {sigma} ({x}, {y})"),
                );
            }
        }
    }
}

fn scope_with_clip_and_right_band() -> ScopeParams {
    ScopeParams {
        fade_edges: [0.0, 0.0, 0.0, 120.0],
        fade_bands: [0.0, 0.0, 0.0, 40.0],
        clip_bounds: [30.0, 35.0, 120.0, 85.0],
        clip_radius: 14.0,
        ..Default::default()
    }
}

#[test]
fn rotated_glyph_is_cut_by_unrotated_device_space_scopes() {
    // A5 and R11: the scope factor depends on the final fragment position only.
    let mut harness = Harness::new();
    let tile = harness.tile(96, 16, |_, _| 255);
    let mut rotated = sprite(&tile, (30.0, 50.0), 3.0);
    rotated.transformation = rotation_about((78.0, 58.0), 8.0);
    let unscoped = harness.render(&[rotated], 160, 120);

    let scope = scope_with_clip_and_right_band();
    let mut scoped = rotated;
    scoped.scope = scope;
    let image = harness.render(&[scoped], 160, 120);

    let (mut faded, mut cut, mut kept) = (0, 0, 0);
    for y in 0..120 {
        for x in 0..160 {
            let factor = scope.factor([x as f32 + 0.5, y as f32 + 0.5]);
            let ink = coverage(&unscoped, x, y);
            assert_close(
                coverage(&image, x, y),
                ink * factor,
                TOLERANCE,
                &format!("({x}, {y})"),
            );
            if ink > 0.2 {
                if factor == 0.0 {
                    cut += 1;
                } else if factor < 0.9 {
                    faded += 1;
                } else {
                    kept += 1;
                }
            }
        }
    }
    assert!(
        faded >= 20 && cut >= 20 && kept >= 100,
        "{faded} {cut} {kept}"
    );
    // The rotation really happened: unrotated bounds do not contain the ink.
    let unrotated = harness.render(&[sprite(&tile, (30.0, 50.0), 3.0)], 160, 120);
    assert_ne!(unrotated, unscoped);
}

#[test]
fn correction_is_applied_before_the_scoped_factor() {
    // A10 / R10: with a fade factor f over a blurred sample the result is
    // corrected(blurred) * f, not corrected(blurred * f).
    let mut harness = Harness::new();
    let table = harness.correction_table();
    let tile = harness.tile(20, 20, |_, _| 255);
    let origin = (30.0, 30.0);
    let sigma = 3.0;
    let plain = sprite(&tile, origin, sigma);
    let unscoped = harness.render(&[plain], 80, 80);
    let scope = ScopeParams {
        fade_edges: [0.0, 0.0, 0.0, 62.0],
        fade_bands: [0.0, 0.0, 0.0, 30.0],
        ..Default::default()
    };
    let mut scoped = plain;
    scoped.scope = scope;
    let image = harness.render(&[scoped], 80, 80);

    let texel = |_: u32, _: u32| 1.0;
    let mut distinguishing = 0;
    for y in 0..80 {
        for x in 0..80 {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let factor = scope.factor([px, py]);
            let blurred = blurred_coverage(sigma, [px - origin.0, py - origin.1], [20, 20], texel);
            let after = corrected(&table, blurred) * factor;
            let before = corrected(&table, blurred * factor);
            assert_close(
                coverage(&image, x, y),
                coverage(&unscoped, x, y) * factor,
                TOLERANCE,
                "factor",
            );
            if (after - before).abs() > 6.0 / 255.0
                && (coverage(&image, x, y) - after).abs() <= TOLERANCE
            {
                distinguishing += 1;
            }
        }
    }
    assert!(
        distinguishing >= 10,
        "too few pixels tell the two orders apart: {distinguishing}"
    );
}

#[test]
fn rectangular_content_mask_clips_the_transformed_blurred_quad() {
    // A8 (pixels): the mask rejects fragments by their transformed position.
    let mut harness = Harness::new();
    let tile = harness.tile(40, 12, |_, _| 255);
    let mut sprite = sprite(&tile, (20.0, 30.0), 2.0);
    sprite.transformation = rotation_about((40.0, 36.0), -12.0);
    let unmasked = harness.render(&[sprite], 100, 80);
    let mask = bounds(30.0, 28.0, 25.0, 17.0);
    sprite.content_mask = ContentMask { bounds: mask };
    let image = harness.render(&[sprite], 100, 80);
    let mut inside_ink = 0;
    for y in 0..80 {
        for x in 0..100 {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let inside = px >= mask.left().0
                && px <= mask.right().0
                && py >= mask.top().0
                && py <= mask.bottom().0;
            let expected = if inside {
                coverage(&unmasked, x, y)
            } else {
                0.0
            };
            assert_eq!(coverage(&image, x, y), expected, "({x}, {y})");
            inside_ink += usize::from(inside && expected > 0.2);
        }
    }
    assert!(inside_ink > 50);
}

// ----- production path: real shaping, real glyph raster -----

mod production {
    use super::*;
    use vespertine::{
        AnyElement, Font, IntoElement, ParentElement, Styled, TextRun, canvas, div, font, hsla, rgb,
    };

    pub(super) const FOREGROUND: u32 = 0xF0D080;

    /// Shapes `text` and returns each glyph's font, id and position relative to the
    /// baseline origin of the line.
    pub(super) fn shape(
        window: &mut vespertine::Window,
        text: &str,
        font: Font,
        size: f32,
    ) -> Vec<(FontId, GlyphId, Point<Pixels>)> {
        let run = TextRun {
            strikethrough: None,
            underline: None,
            background_color: None,
            color: hsla(0.0, 0.0, 1.0, 1.0),
            font,
            len: text.len(),
            letter_spacing: Pixels::ZERO,
        };
        let line = window
            .text_system()
            .shape_line(text.to_string().into(), px(size), &[run], None);
        line.runs
            .iter()
            .flat_map(|run| {
                run.glyphs
                    .iter()
                    .map(|glyph| (run.font_id, glyph.id, glyph.position))
            })
            .collect()
    }

    /// A full-window element with background `background` that runs `paint` in its paint
    /// phase with the shaped glyphs of `text`.
    pub(super) fn scene(
        background: u32,
        text: &'static str,
        font: Font,
        size: f32,
        paint: impl Fn(&mut vespertine::Window, &[(FontId, GlyphId, Point<Pixels>)]) + 'static,
    ) -> impl Fn(&mut vespertine::Window, &mut vespertine::App) -> AnyElement {
        let paint = Arc::new(paint);
        move |_, _| {
            let (paint, font) = (paint.clone(), font.clone());
            div()
                .size_full()
                .bg(rgb(background))
                .child(
                    canvas(
                        |_, _, _| {},
                        move |_, _, window, _| {
                            let glyphs = shape(window, text, font.clone(), size);
                            paint(window, &glyphs);
                        },
                    )
                    .size_full(),
                )
                .into_any_element()
        }
    }

    pub(super) fn noto() -> Font {
        font("Noto Sans")
    }
}

fn rgb_of(image: &RgbaImage, x: u32, y: u32) -> [f32; 3] {
    let p = image.get_pixel(x, y).0;
    [f32::from(p[0]), f32::from(p[1]), f32::from(p[2])]
}

fn luminance_difference(image: &RgbaImage, background: [f32; 3], x: u32, y: u32) -> f32 {
    let p = rgb_of(image, x, y);
    (0..3)
        .map(|c| (p[c] - background[c]).abs())
        .fold(0.0, f32::max)
}

/// Bounding box `(left, top, right, bottom)` and ink-weighted centroid of the pixels that
/// differ from the background by more than `threshold`.
fn ink(
    image: &RgbaImage,
    background: [f32; 3],
    threshold: f32,
) -> ((u32, u32, u32, u32), [f32; 2]) {
    let (mut left, mut top, mut right, mut bottom) = (u32::MAX, u32::MAX, 0, 0);
    let (mut sum, mut sx, mut sy) = (0.0, 0.0, 0.0);
    for y in 0..image.height() {
        for x in 0..image.width() {
            let weight = luminance_difference(image, background, x, y);
            if weight > threshold {
                left = left.min(x);
                right = right.max(x);
                top = top.min(y);
                bottom = bottom.max(y);
            }
            sum += weight;
            sx += weight * (x as f32 + 0.5);
            sy += weight * (y as f32 + 0.5);
        }
    }
    ((left, top, right, bottom), [sx / sum, sy / sum])
}

fn hsla_of(rgb_hex: u32) -> Hsla {
    vespertine::rgb(rgb_hex).into()
}

#[test]
fn ordinary_and_identity_zero_blur_glyphs_are_pixel_identical_end_to_end() {
    // A1 on real glyphs through the production path; also FID-RND-005-1 glyph one.
    use production::*;
    let color = hsla_of(FOREGROUND);
    let mut host = RenderHost::new();
    let ordinary = host.render_logical(
        600.0,
        360.0,
        scene(0x202838, "A", noto(), 80.0, move |window, glyphs| {
            let (font_id, glyph_id, _) = glyphs[0];
            window
                .paint_glyph(
                    point(px(70.0), px(210.0)),
                    font_id,
                    glyph_id,
                    px(80.0),
                    color,
                )
                .unwrap();
        }),
    );
    let transformed = host.render_logical(
        600.0,
        360.0,
        scene(0x202838, "A", noto(), 80.0, move |window, glyphs| {
            let (font_id, glyph_id, _) = glyphs[0];
            window
                .paint_glyph_transformed(
                    point(px(70.0), px(210.0)),
                    font_id,
                    glyph_id,
                    px(80.0),
                    color,
                    TransformationMatrix::unit(),
                    px(0.0),
                )
                .unwrap();
        }),
    );
    let background = [32.0, 40.0, 56.0];
    let (extent, _) = ink(&ordinary, background, 8.0);
    assert!(
        extent.2 - extent.0 > 40,
        "the glyph was painted: {extent:?}"
    );
    assert_eq!(ordinary, transformed);
}

#[test]
fn fid_rnd_005_1_rotation_pivot_and_blur_halo() {
    // FID-RND-005-1: window 600 x 360 logical, scale 2, background #202838, "A" in Noto
    // Sans 400 at 80 logical, color #F0D080, baseline origins (70,210), (250,210), (430,210):
    // identity/blur 0; 15 degrees clockwise about its own origin (device pivot (500,420));
    // identity with logical blur 4 (device sigma 8: halo 24, radius 12).
    use production::*;
    let color = hsla_of(FOREGROUND);
    let background = [32.0, 40.0, 56.0];
    let mut host = RenderHost::new();
    let mut render = |origin_x: f32, transformation: TransformationMatrix, blur: f32| {
        host.render_logical(
            600.0,
            360.0,
            scene(0x202838, "A", noto(), 80.0, move |window, glyphs| {
                let (font_id, glyph_id, _) = glyphs[0];
                window
                    .paint_glyph_transformed(
                        point(px(origin_x), px(210.0)),
                        font_id,
                        glyph_id,
                        px(80.0),
                        color,
                        transformation,
                        px(blur),
                    )
                    .unwrap();
            }),
        )
    };
    let unit = TransformationMatrix::unit();
    let plain = render(250.0, unit, 0.0);
    let rotated = render(250.0, rotation_about((500.0, 420.0), 15.0), 0.0);
    let blurred = render(430.0, unit, 4.0);
    let blurred_reference = render(430.0, unit, 0.0);

    // Pivot: the centroid of the rotated glyph is the clockwise (y down) rotation of the
    // unrotated centroid about the device pivot.
    let (plain_box, plain_centroid) = ink(&plain, background, 8.0);
    let (_, rotated_centroid) = ink(&rotated, background, 8.0);
    let (sin, cos) = 15.0_f32.to_radians().sin_cos();
    let (dx, dy) = (plain_centroid[0] - 500.0, plain_centroid[1] - 420.0);
    let expected = [500.0 + cos * dx - sin * dy, 420.0 + sin * dx + cos * dy];
    assert!(
        (rotated_centroid[0] - expected[0]).abs() < 1.0
            && (rotated_centroid[1] - expected[1]).abs() < 1.0,
        "rotated centroid {rotated_centroid:?}, expected {expected:?}"
    );
    assert_ne!(plain_box, ink(&rotated, background, 8.0).0);

    // Blur: the ink footprint grows by a margin that stays within the 24 pixel halo, the
    // glyph's own ink does not move (centroid) and no pixel beyond the halo is touched.
    let (sharp_box, sharp_centroid) = ink(&blurred_reference, background, 8.0);
    let (blur_box, blur_centroid) = ink(&blurred, background, 1.0);
    assert!(
        blur_box.0 + 10 <= sharp_box.0 && blur_box.2 >= sharp_box.2 + 10,
        "{blur_box:?} {sharp_box:?}"
    );
    assert!(
        blur_box.0 + 24 >= sharp_box.0 && blur_box.2 <= sharp_box.2 + 24,
        "{blur_box:?} {sharp_box:?}"
    );
    assert!(
        blur_box.1 + 24 >= sharp_box.1 && blur_box.3 <= sharp_box.3 + 24,
        "{blur_box:?} {sharp_box:?}"
    );
    assert!(
        (blur_centroid[0] - sharp_centroid[0]).abs() < 1.0
            && (blur_centroid[1] - sharp_centroid[1]).abs() < 1.0,
        "{blur_centroid:?} {sharp_centroid:?}"
    );
    // Nothing outside the halo box of the unblurred glyph's own bounds.
    let limit = (sharp_box.0 - 24 - 3, sharp_box.2 + 24 + 3);
    for y in 0..blurred.height() {
        for x in (0..limit.0).chain(limit.1..blurred.width()) {
            assert!(
                luminance_difference(&blurred, background, x, y) < 0.5,
                "({x}, {y})"
            );
        }
    }
}

#[test]
fn fid_rnd_005_2_blurred_text_in_rounded_clip_and_right_fade() {
    // FID-RND-005-2: 600 x 500 logical, background #111823, "Blurred glyphs 0123" in Noto
    // Sans 700 at 40 logical, color #E9F4FF, three 420 x 80 boxes at (60,50), (60,190),
    // (60,330) with baselines 40 below the box tops; logical blur 0, 1, 2.5 (device 0, 2,
    // 5); only the middle box is rotated 8 degrees clockwise about its center (device
    // (540,460)); all clipped by a rounded scope at (40,30) 420 x 430 radius 24 with an
    // edge fade of only a 48 logical right band on the same bounds.
    use production::*;
    use vespertine::{EdgeFade, size};
    let color = hsla_of(0xE9F4FF);
    let background = [17.0, 24.0, 35.0];
    let region = Bounds {
        origin: point(px(40.0), px(30.0)),
        size: size(px(420.0), px(430.0)),
    };
    let paint = move |scoped: bool| {
        move |window: &mut vespertine::Window, glyphs: &[(FontId, GlyphId, Point<Pixels>)]| {
            let mut fade = EdgeFade::new(region, px(48.0));
            fade.right = true;
            let body = |window: &mut vespertine::Window| {
                for (top, blur, degrees) in [(50.0, 0.0, 0.0), (190.0, 1.0, 8.0), (330.0, 2.5, 0.0)]
                {
                    let transformation = if degrees == 0.0 {
                        TransformationMatrix::unit()
                    } else {
                        rotation_about((540.0, 460.0), degrees)
                    };
                    for (font_id, glyph_id, position) in glyphs {
                        window
                            .paint_glyph_transformed(
                                point(px(60.0) + position.x, px(top + 40.0) + position.y),
                                *font_id,
                                *glyph_id,
                                px(40.0),
                                color,
                                transformation,
                                px(blur),
                            )
                            .unwrap();
                    }
                }
            };
            if scoped {
                window.with_rounded_clip(region, px(24.0), |window| {
                    window.with_edge_fade(Some(fade), body)
                });
            } else {
                body(window);
            }
        }
    };
    let bold = || noto().bold();
    let mut host = RenderHost::new();
    let unscoped = host.render_logical(
        600.0,
        500.0,
        scene(0x111823, "Blurred glyphs 0123", bold(), 40.0, paint(false)),
    );
    let scoped = host.render_logical(
        600.0,
        500.0,
        scene(0x111823, "Blurred glyphs 0123", bold(), 40.0, paint(true)),
    );

    let scope = ScopeParams {
        fade_edges: [0.0, 0.0, 0.0, 920.0],
        fade_bands: [0.0, 0.0, 0.0, 96.0],
        clip_bounds: [80.0, 60.0, 920.0, 920.0],
        clip_radius: 48.0,
        ..Default::default()
    };
    let (mut faded, mut cut) = (0, 0);
    for y in 0..scoped.height() {
        for x in 0..scoped.width() {
            let factor = scope.factor([x as f32 + 0.5, y as f32 + 0.5]);
            let base = rgb_of(&unscoped, x, y);
            let actual = rgb_of(&scoped, x, y);
            // Sharp and lightly blurred strings (device rows above 620) do not overlap
            // enough for the blend to be nonlinear: the scoped pixel is the unscoped one
            // pulled toward the background by the unrotated device-space factor. In the
            // heavily blurred string neighboring glyph quads overlap, and compositing two
            // faded layers is not linear in the factor, so only its bounds are checked.
            for channel in 0..3 {
                let expected = background[channel] + factor * (base[channel] - background[channel]);
                if y < 620 {
                    assert!(
                        (actual[channel] - expected).abs() <= 4.0,
                        "({x}, {y}) channel {channel}: {} vs {expected}",
                        actual[channel]
                    );
                } else {
                    let (low, high) = (
                        background[channel].min(base[channel]) - 1.0,
                        background[channel].max(base[channel]) + 1.0,
                    );
                    assert!(
                        actual[channel] >= low && actual[channel] <= high,
                        "({x}, {y})"
                    );
                    if factor == 0.0 {
                        assert!(
                            (actual[channel] - background[channel]).abs() <= 1.0,
                            "({x}, {y})"
                        );
                    }
                    if factor == 1.0 {
                        assert!((actual[channel] - base[channel]).abs() <= 1.0, "({x}, {y})");
                    }
                }
            }
            if luminance_difference(&unscoped, background, x, y) > 20.0 {
                if factor == 0.0 {
                    cut += 1;
                } else if factor < 0.95 {
                    faded += 1;
                }
            }
        }
    }
    // The strings run into the right band: a good deal of ink is faded. (With this
    // shaping the ink ends at device x 908, just inside the band's end at 920, so little
    // or none of it is cut away entirely.)
    assert!(faded > 50, "faded {faded}, cut {cut}");
    let (_, scoped_centroid) = ink(&scoped, background, 20.0);
    let (_, unscoped_centroid) = ink(&unscoped, background, 20.0);
    assert!(
        scoped_centroid[0] < unscoped_centroid[0],
        "the fade removes ink on the right"
    );
    // The blurred strings spread ink over more pixels than the sharp one.
    let spread = |from: u32, to: u32| {
        (from..to)
            .flat_map(|y| (0..unscoped.width()).map(move |x| (x, y)))
            .filter(|&(x, y)| luminance_difference(&unscoped, background, x, y) > 4.0)
            .count()
    };
    let (sharp, medium, wide) = (spread(60, 220), spread(340, 520), spread(640, 840));
    assert!(wide > sharp && medium > 0, "{sharp} {medium} {wide}");
}
