//! GPU pixel tests of the rounded clip scope and the curved overflow clip chain (RND-002):
//! A1, A5, A9 to A12 and requirements R13 to R17, checked against the reference arithmetic of
//! `vespertine::ScopeParams` / `vespertine::CurvedClipShape` and against independent oracles.

use crate::WgpuHeadlessRenderer;
use crate::tests_scope::{bounds, coverage, device_size, gradient, quad};
use image::RgbaImage;
use std::borrow::Cow;
use vespertine::{
    AtlasKey, AtlasTile, Background, Bounds, ContentMask, Corners, CurvedClipShape, Edges, FontId,
    GlyphId, Hsla, ImageId, MonochromeSprite, Path, PlatformHeadlessRenderer, PolychromeSprite,
    RenderGlyphParams, RenderImageParams, RenderSvgParams, ScaledPixels, Scene, ScopeParams,
    Shadow, Size, SubpixelSprite, TransformationMatrix, Underline, black, curved_clip_coverage,
    point, px, white,
};

const TOLERANCE: f32 = 3.0 / 255.0;

/// A renderer plus helpers that hand out atlas tiles of the same renderer.
pub(crate) struct Gpu {
    pub(crate) renderer: WgpuHeadlessRenderer,
    next_tile: usize,
}

impl Gpu {
    pub(crate) fn new() -> Self {
        Self {
            renderer: WgpuHeadlessRenderer::new().expect("headless renderer"),
            next_tile: 0,
        }
    }

    pub(crate) fn render(&mut self, scene: &mut Scene, width: i32, height: i32) -> RgbaImage {
        scene.finish();
        self.renderer
            .render_scene_to_image(scene, device_size(width, height))
            .expect("render")
    }

    fn insert(&mut self, key: AtlasKey, size: i32, bytes: Vec<u8>) -> AtlasTile {
        let size = Size {
            width: vespertine::DevicePixels(size),
            height: vespertine::DevicePixels(size),
        };
        self.renderer
            .sprite_atlas()
            .get_or_insert_with(key, &mut || Ok(Some((size, Cow::Owned(bytes.clone())))))
            .expect("atlas insert")
            .expect("tile")
    }

    /// An 8 x 8 monochrome mask tile of full coverage (the vector-icon route).
    pub(crate) fn mono_tile(&mut self) -> AtlasTile {
        self.next_tile += 1;
        let key = AtlasKey::Svg(RenderSvgParams {
            path: format!("clip-test-mono-{}", self.next_tile).into(),
            size: Size {
                width: vespertine::DevicePixels(8),
                height: vespertine::DevicePixels(8),
            },
        });
        self.insert(key, 8, vec![255; 64])
    }

    /// An 8 x 8 opaque color tile (BGRA bytes).
    pub(crate) fn color_tile(&mut self, bgra: [u8; 4]) -> AtlasTile {
        self.next_tile += 1;
        let key = AtlasKey::Image(RenderImageParams {
            image_id: ImageId(1_000_000 + self.next_tile),
            frame_index: 0,
        });
        self.insert(key, 8, bgra.repeat(64))
    }

    /// An 8 x 8 subpixel coverage tile.
    pub(crate) fn subpixel_tile(&mut self) -> AtlasTile {
        self.next_tile += 1;
        let key = AtlasKey::Glyph(RenderGlyphParams {
            font_id: FontId(0),
            glyph_id: GlyphId(self.next_tile as u32),
            font_size: px(8.0),
            subpixel_variant: point(0, 0),
            scale_factor: 1.0,
            is_emoji: false,
            subpixel_rendering: true,
            dilation: 0,
        });
        self.insert(key, 8, vec![255; 256])
    }
}

/// A fixture of one probe per primitive class.
pub(crate) struct Tiles {
    pub(crate) mono: AtlasTile,
    pub(crate) color: AtlasTile,
    pub(crate) subpixel: AtlasTile,
}

impl Tiles {
    pub(crate) fn new(gpu: &mut Gpu) -> Self {
        Self {
            mono: gpu.mono_tile(),
            color: gpu.color_tile([60, 120, 240, 255]),
            subpixel: gpu.subpixel_tile(),
        }
    }
}

/// A rounded-clip scope whose coverage is 1 right of `left` and 0 left of it, with `K = 0.5`
/// on the pixel whose center lies exactly on `left`.
fn column_scope(left: f32) -> ScopeParams {
    ScopeParams {
        clip_bounds: [left, -1000.0, left + 2000.0, 1000.0],
        clip_radius: 0.0,
        ..Default::default()
    }
}

fn mask(area: Bounds<ScaledPixels>) -> ContentMask<ScaledPixels> {
    ContentMask { bounds: area }
}

pub(crate) fn mono_sprite(
    area: Bounds<ScaledPixels>,
    tile: AtlasTile,
    color: Hsla,
) -> MonochromeSprite {
    MonochromeSprite {
        order: 0,
        blur: 0.0,
        bounds: area,
        content_mask: mask(area),
        color,
        tile,
        transformation: TransformationMatrix::unit(),
        scope: Default::default(),
    }
}

pub(crate) fn poly_sprite(area: Bounds<ScaledPixels>, tile: AtlasTile) -> PolychromeSprite {
    PolychromeSprite {
        order: 0,
        pad: 0,
        grayscale: false.into(),
        opacity: 1.0,
        bounds: area,
        content_mask: mask(area),
        corner_radii: Corners::default(),
        tile,
        scope: Default::default(),
        effects: Default::default(),
    }
}

pub(crate) fn subpixel_sprite(
    area: Bounds<ScaledPixels>,
    tile: AtlasTile,
    color: Hsla,
) -> SubpixelSprite {
    SubpixelSprite {
        order: 0,
        pad: 0,
        bounds: area,
        content_mask: mask(area),
        color,
        tile,
        transformation: TransformationMatrix::unit(),
        scope: Default::default(),
    }
}

fn shadow(element: Bounds<ScaledPixels>, area: Bounds<ScaledPixels>, color: Hsla) -> Shadow {
    Shadow {
        order: 0,
        blur_radius: ScaledPixels(4.0),
        bounds: element,
        corner_radii: Corners::all(ScaledPixels(3.0)),
        content_mask: mask(area),
        color,
        element_bounds: element,
        element_corner_radii: Corners::all(ScaledPixels(3.0)),
        inset: 0,
        curved_clip: 0,
    }
}

fn underline(area: Bounds<ScaledPixels>, color: Hsla) -> Underline {
    Underline {
        order: 0,
        curved_clip: 0,
        bounds: area,
        content_mask: mask(area),
        color,
        thickness: ScaledPixels(3.0),
        wavy: false.into(),
    }
}

fn triangle(area: Bounds<ScaledPixels>, color: Hsla) -> Path<ScaledPixels> {
    let left = area.origin.x.0;
    let top = area.origin.y.0;
    let right = left + area.size.width.0;
    let bottom = top + area.size.height.0;
    let mut path = Path::new(point(px(left), px(bottom)));
    path.line_to(point(px((left + right) / 2.0), px(top)));
    path.line_to(point(px(right), px(bottom)));
    path.content_mask = ContentMask {
        bounds: Bounds {
            origin: point(px(left), px(top)),
            size: vespertine::size(px(right - left), px(bottom - top)),
        },
    };
    path.color = Background::from(color);
    path.scale(1.0)
}

fn close(actual: f32, expected: f32, what: impl std::fmt::Debug) {
    assert!(
        (actual - expected).abs() <= TOLERANCE,
        "{what:?}: rendered {actual}, expected {expected}"
    );
}

fn red(image: &RgbaImage, x: u32, y: u32) -> f32 {
    coverage(image, x, y)
}

// ---- A1: rounded coverage K on the GPU ----

#[test]
fn rounded_scope_edge_distance_landmarks_on_gpu() {
    // D = -0.5, 0, +0.5 give K = 1, 0.5, 0 (R3), through the general quad route.
    let mut gpu = Gpu::new();
    for (left, pixel, expected) in [(10.0, 10, 1.0), (10.5, 10, 0.5), (10.0, 9, 0.0)] {
        let scope = ScopeParams {
            clip_bounds: [left, -100.0, 200.0, 200.0],
            ..Default::default()
        };
        let mut scene = Scene::default();
        scene.insert_primitive(quad(bounds(0.0, 0.0, 32.0, 8.0), gradient(white()), scope));
        let image = gpu.render(&mut scene, 32, 8);
        close(
            red(&image, pixel, 4),
            expected,
            format!("edge at {left}, pixel {pixel}"),
        );
    }
}

#[test]
fn rounded_scope_radius_larger_than_half_size_is_clamped_on_gpu() {
    // 32 x 16 scope with radius 1000: the radius clamps to the half height, a pill shape.
    let scope = ScopeParams {
        clip_bounds: [4.0, 4.0, 36.0, 20.0],
        clip_radius: 1000.0,
        ..Default::default()
    };
    let clamped = ScopeParams {
        clip_radius: 8.0,
        ..scope
    };
    let mut scene = Scene::default();
    scene.insert_primitive(quad(bounds(0.0, 0.0, 40.0, 24.0), gradient(white()), scope));
    let image = Gpu::new().render(&mut scene, 40, 24);
    for y in 0..24 {
        for x in 0..40 {
            let p = [x as f32 + 0.5, y as f32 + 0.5];
            assert_eq!(scope.rounded_coverage(p), clamped.rounded_coverage(p));
            close(red(&image, x, y), clamped.rounded_coverage(p), (x, y));
        }
    }
    // The pill's left cap is a half circle: the corner pixel is cut, the middle row is not.
    assert!(red(&image, 4, 4) < 0.05);
    assert!(red(&image, 4, 12) > 0.95);
}

// ---- A5 and RND-001 A7: the quad routes under K = 0.5 ----

#[test]
fn rounded_bordered_quad_fill_gets_k_squared_at_the_clip_edge() {
    let mut scene = Scene::default();

    // Region A (x 0..64): rounded fill-only quad, K = 0.5 down the pixel column x = 10.
    let mut rounded = quad(
        bounds(0.0, 0.0, 64.0, 64.0),
        gradient(white()),
        column_scope(10.5),
    );
    rounded.corner_radii = Corners::all(ScaledPixels(20.0));
    scene.insert_primitive(rounded);

    // Region B (x 64..128): rounded quad with a black fill and a white 4 px border; the
    // border color receives K once (RND-001 A7).
    let mut border_quad = quad(
        bounds(64.0, 0.0, 64.0, 64.0),
        gradient(black()),
        column_scope(65.5),
    );
    border_quad.corner_radii = Corners::all(ScaledPixels(8.0));
    border_quad.border_widths = Edges::all(ScaledPixels(4.0));
    border_quad.border_color = white();
    scene.insert_primitive(border_quad);

    // Region C (x 128..192): white fill, black 4 px border: the fill at the inner edge's
    // antialias band is outside the interior fast return and gets K twice.
    let mut fill_quad = quad(
        bounds(128.0, 0.0, 64.0, 64.0),
        gradient(white()),
        column_scope(132.5),
    );
    fill_quad.corner_radii = Corners::all(ScaledPixels(8.0));
    fill_quad.border_widths = Edges::all(ScaledPixels(4.0));
    fill_quad.border_color = black();
    scene.insert_primitive(fill_quad);

    // Region D (x 192..256): unrounded borderless general-route rectangle: K once.
    scene.insert_primitive(quad(
        bounds(192.0, 0.0, 64.0, 64.0),
        gradient(white()),
        column_scope(202.5),
    ));

    // Region E (x 256..320): a plain solid quad in a batch that also holds general-route quads
    // is drawn by the general route and receives K (RND-002 R6, PRF-008 R3).
    scene.insert_primitive(quad(
        bounds(256.0, 0.0, 64.0, 64.0),
        white(),
        column_scope(266.5),
    ));

    // Region F (x 320..384): the same rounded fill-only quad with no scope: RND-001 A7's
    // "a rounded quad's fill under no fade follows its own antialiasing" baseline.
    let mut baseline = quad(
        bounds(320.0, 0.0, 64.0, 64.0),
        gradient(white()),
        Default::default(),
    );
    baseline.corner_radii = Corners::all(ScaledPixels(20.0));
    scene.insert_primitive(baseline);

    let image = Gpu::new().render(&mut scene, 384, 64);
    // A: corner region, inside the curve: K^2.
    close(
        red(&image, 10, 10),
        0.25,
        "rounded fill in the corner region",
    );
    // A: straight left edge, interior fast return: K once.
    close(
        red(&image, 10, 32),
        0.5,
        "interior fast return keeps K once",
    );
    // B: border color K once.
    close(red(&image, 65, 32), 0.5, "border color receives K once");
    // C: fill in the inner antialias band: K^2.
    close(
        red(&image, 132, 32),
        0.25,
        "bordered fill receives K squared",
    );
    // D: unrounded borderless general route: K once.
    close(
        red(&image, 202, 32),
        0.5,
        "unrounded general route receives K once",
    );
    // E: the plain solid quad shares its batch with the gradient quads above: K = 0.5 there.
    close(
        red(&image, 266, 32),
        0.5,
        "plain solid in a mixed batch gets K",
    );
    close(
        red(&image, 265, 32),
        column_scope(266.5).rounded_coverage([265.5, 32.5]),
        "plain solid in a mixed batch gets K outside too",
    );
    // The same quad alone in its batch takes the specialized solid route and bypasses K
    // entirely, although K = 0.5 there (RND-002 R6).
    let mut alone = Scene::default();
    alone.insert_primitive(quad(
        bounds(256.0, 0.0, 64.0, 64.0),
        white(),
        column_scope(266.5),
    ));
    let image_alone = Gpu::new().render(&mut alone, 384, 64);
    close(
        red(&image_alone, 266, 32),
        1.0,
        "plain solid alone bypasses K",
    );
    close(
        red(&image_alone, 265, 32),
        1.0,
        "plain solid alone bypasses K outside too",
    );
    // F: no scope: the fill is fully opaque inside the curve and cut outside it.
    close(red(&image, 320 + 10, 10), 1.0, "unscoped rounded fill");
    assert!(red(&image, 320, 0) < 0.05, "outside the rounded corner");
}

// ---- A10 and R13: curved overflow clip coverage on the GPU ----

fn render_shape(gpu: &mut Gpu, shapes: &[CurvedClipShape], size: i32) -> RgbaImage {
    let mut scene = Scene::default();
    for shape in shapes {
        scene.push_curved_clip(*shape);
    }
    // A plain solid quad: curved clips apply to every route, plain solid included.
    scene.insert_primitive(quad(
        bounds(0.0, 0.0, size as f32, size as f32),
        white(),
        Default::default(),
    ));
    for _ in shapes {
        scene.pop_curved_clip();
    }
    gpu.render(&mut scene, size, size)
}

#[test]
fn curved_shape_extent_64_radii_24_matches_r13_and_the_exact_circle() {
    let shape = CurvedClipShape {
        bounds: [8.0, 8.0, 72.0, 72.0],
        radii_x: [24.0; 4],
        radii_y: [24.0; 4],
    };
    let image = render_shape(&mut Gpu::new(), &[shape], 80);
    let corners = [
        ([8.0f32 + 24.0, 8.0 + 24.0], [-1.0f32, -1.0f32]),
        ([72.0 - 24.0, 8.0 + 24.0], [1.0, -1.0]),
        ([72.0 - 24.0, 72.0 - 24.0], [1.0, 1.0]),
        ([8.0 + 24.0, 72.0 - 24.0], [-1.0, 1.0]),
    ];
    for y in 0..80u32 {
        for x in 0..80u32 {
            let p = [x as f32 + 0.5, y as f32 + 0.5];
            close(
                red(&image, x, y),
                shape.coverage(p),
                ("R13 reference", x, y),
            );
            // Equal radii reproduce the exact circular distance inside each corner box.
            for (center, direction) in corners {
                let in_box = (p[0] - center[0]) * direction[0] > 0.0
                    && (p[1] - center[1]) * direction[1] > 0.0;
                if in_box {
                    let distance =
                        ((p[0] - center[0]).powi(2) + (p[1] - center[1]).powi(2)).sqrt() - 24.0;
                    let expected = (0.5 - distance).clamp(0.0, 1.0);
                    close(red(&image, x, y), expected, ("exact circle", x, y));
                }
            }
        }
    }
    // Straight edges: D = -0.5 is covered, D = +0.5 is not.
    close(red(&image, 40, 8), 1.0, "just inside the top edge");
    close(red(&image, 40, 7), 0.0, "just outside the top edge");
    close(red(&image, 8, 40), 1.0, "just inside the left edge");
    close(red(&image, 71, 40), 1.0, "just inside the right edge");
    close(red(&image, 72, 40), 0.0, "just outside the right edge");
}

#[test]
fn empty_curved_shapes_cover_nothing_and_nested_shapes_combine_by_minimum() {
    let mut gpu = Gpu::new();
    let full = CurvedClipShape {
        bounds: [0.0, 0.0, 32.0, 32.0],
        ..Default::default()
    };
    let half = CurvedClipShape {
        bounds: [16.0, 0.0, 48.0, 32.0],
        radii_x: [8.0; 4],
        radii_y: [8.0; 4],
    };
    // Zero extent in either component: coverage 0 everywhere (R13).
    for empty in [
        CurvedClipShape::default(),
        CurvedClipShape {
            bounds: [4.0, 4.0, 4.0, 28.0],
            ..Default::default()
        },
        CurvedClipShape {
            bounds: [4.0, 4.0, 28.0, 4.0],
            ..Default::default()
        },
        CurvedClipShape {
            bounds: [28.0, 4.0, 4.0, 28.0],
            ..Default::default()
        },
    ] {
        let image = render_shape(&mut gpu, &[empty], 32);
        assert!(image.pixels().all(|pixel| pixel.0[0] == 0), "{empty:?}");
        // A primitive hidden by an empty shape deep in a chain is not revealed either.
        let image = render_shape(&mut gpu, &[full, empty, full], 32);
        assert!(image.pixels().all(|pixel| pixel.0[0] == 0), "{empty:?}");
    }
    // Nested shapes combine by minimum, not product.
    let image = render_shape(&mut gpu, &[full, half], 48);
    for y in 0..48u32 {
        for x in 0..48u32 {
            let p = [x as f32 + 0.5, y as f32 + 0.5];
            close(
                red(&image, x, y),
                curved_clip_coverage([&full, &half], p),
                ("minimum", x, y),
            );
        }
    }
    // The coincident straight edge at x = 16: a product would give 0.25 at D = 0 (0.5 * 0.5).
    let coincident = CurvedClipShape {
        bounds: [16.5, 0.0, 48.0, 32.0],
        ..Default::default()
    };
    let image = render_shape(&mut gpu, &[coincident, coincident], 48);
    close(red(&image, 16, 16), 0.5, "coincident edges do not darken");
}

/// The corner of `shape` (0 top-left, 1 top-right, 2 bottom-right, 3 bottom-left) as the
/// inward distances of `p` from its two edges.
fn inward(shape: &CurvedClipShape, corner: usize, p: [f32; 2]) -> [f32; 2] {
    let [left, top, right, bottom] = shape.bounds;
    let x = if corner == 1 || corner == 2 {
        right - p[0]
    } else {
        p[0] - left
    };
    let y = if corner >= 2 {
        bottom - p[1]
    } else {
        p[1] - top
    };
    [x, y]
}

#[test]
fn every_corner_of_every_curved_shape_follows_its_own_ellipse() {
    // Border boxes, outer radii (tl, tr, br, bl) and border widths (top, right, bottom,
    // left) of two unrelated shapes with asymmetric thick borders. The left border of the
    // first shape (26) is thicker than its top-left outer radius could absorb, and the
    // second shape's left and bottom borders cut through the bottom-left and top-left
    // ellipses entirely.
    let first = CurvedClipShape::from_padding_edge(
        bounds(4.0, 4.0, 120.0, 100.0),
        [30.0, 20.0, 24.0, 40.0],
        [4.0, 18.0, 6.0, 26.0],
    );
    let second = CurvedClipShape::from_padding_edge(
        bounds(30.0, 20.0, 100.0, 90.0),
        [16.0, 16.0, 50.0, 8.0],
        [10.0, 2.0, 22.0, 30.0],
    );
    // R12, written out by hand: inner radii max(r - h, 0), max(r - v, 0).
    assert_eq!(first.bounds, [30.0, 8.0, 106.0, 98.0]);
    assert_eq!(first.radii_x, [4.0, 2.0, 6.0, 14.0]);
    assert_eq!(first.radii_y, [26.0, 16.0, 18.0, 34.0]);
    assert_eq!(second.bounds, [60.0, 30.0, 128.0, 88.0]);
    assert_eq!(second.radii_x, [0.0, 14.0, 48.0, 0.0]);
    assert_eq!(second.radii_y, [6.0, 6.0, 28.0, 0.0]);

    let mut gpu = Gpu::new();
    let size = 140;
    let alone: Vec<RgbaImage> = [first, second]
        .iter()
        .map(|shape| render_shape(&mut gpu, &[*shape], size))
        .collect();
    let nested = render_shape(&mut gpu, &[first, second], size);

    for (index, shape) in [first, second].iter().enumerate() {
        for corner in 0..4 {
            let rx = shape.radii_x[corner];
            let ry = shape.radii_y[corner];
            for y in 0..size as u32 {
                for x in 0..size as u32 {
                    let p = [x as f32 + 0.5, y as f32 + 0.5];
                    let [ix, iy] = inward(shape, corner, p);
                    // Only the corner box of a shape whose point is inside the rectangle.
                    let [left, top, right, bottom] = shape.bounds;
                    if p[0] < left || p[0] > right || p[1] < top || p[1] > bottom {
                        continue;
                    }
                    if !(ix < rx && iy < ry) && !(rx == 0.0 || ry == 0.0) {
                        continue;
                    }
                    let actual = red(&alone[index], x, y);
                    if rx > 0.0 && ry > 0.0 {
                        // Independent ellipse test with a 3 px margin along the radius.
                        let e = ((ix - rx) / rx).powi(2) + ((iy - ry) / ry).powi(2);
                        let margin = 3.0 / rx.min(ry);
                        if e.sqrt() <= 1.0 - margin {
                            close(actual, 1.0, ("inside the ellipse", index, corner, x, y));
                        } else if e.sqrt() >= 1.0 + margin {
                            close(actual, 0.0, ("outside the ellipse", index, corner, x, y));
                        }
                    } else if ix >= 1.0 && iy >= 1.0 && ix < 4.0 && iy < 4.0 {
                        // A square corner stays square.
                        close(actual, 1.0, ("square corner", index, corner, x, y));
                    }
                }
            }
        }
    }

    // No inherited or relocated corners: the nested render is the minimum of the two.
    for y in 0..size as u32 {
        for x in 0..size as u32 {
            let p = [x as f32 + 0.5, y as f32 + 0.5];
            let expected = first.coverage(p).min(second.coverage(p));
            close(red(&nested, x, y), expected, ("nested minimum", x, y));
            let lone_min = red(&alone[0], x, y).min(red(&alone[1], x, y));
            close(red(&nested, x, y), lone_min, ("nested vs separate", x, y));
        }
    }
}

// ---- R15: C multiplies every primitive class exactly once ----

/// Paints one probe per primitive class into non-overlapping rows of a 64 x 144 target:
/// plain solid quad, general rounded gradient quad, drop shadow, path, underline,
/// strikethrough-sized underline, monochrome, color and subpixel sprites.
fn paint_probes(scene: &mut Scene, tiles: &Tiles, shape: Option<CurvedClipShape>) {
    if let Some(shape) = shape {
        scene.push_curved_clip(shape);
    }
    let row = |index: usize| bounds(0.0, index as f32 * 16.0, 64.0, 16.0);
    let cell = |index: usize| bounds(8.0, index as f32 * 16.0 + 2.0, 48.0, 12.0);
    scene.insert_primitive(quad(cell(0), white(), Default::default()));
    let mut rounded = quad(cell(1), gradient(white()), Default::default());
    rounded.corner_radii = Corners::all(ScaledPixels(5.0));
    rounded.border_widths = Edges::all(ScaledPixels(2.0));
    rounded.border_color = vespertine::rgb(0x40a0e0).into();
    scene.insert_primitive(rounded);
    scene.insert_primitive(shadow(
        bounds(16.0, 2.0 * 16.0 + 4.0, 32.0, 8.0),
        row(2),
        white().opacity(0.8),
    ));
    scene.insert_primitive(triangle(cell(3), white()));
    scene.insert_primitive(underline(bounds(8.0, 4.0 * 16.0 + 6.0, 48.0, 3.0), white()));
    scene.insert_primitive(underline(
        bounds(8.0, 5.0 * 16.0 + 2.0, 48.0, 12.0),
        vespertine::rgb(0xe0a040).into(),
    ));
    scene.insert_primitive(mono_sprite(cell(6), tiles.mono, white()));
    scene.insert_primitive(poly_sprite(cell(7), tiles.color));
    scene.insert_primitive(subpixel_sprite(cell(8), tiles.subpixel, white()));
    if shape.is_some() {
        scene.pop_curved_clip();
    }
}

#[test]
fn curved_clip_multiplies_every_primitive_class_once() {
    let mut gpu = Gpu::new();
    let tiles = Tiles::new(&mut gpu);
    // Left edge at x = 24.5 (C = 0.5 on pixel column 24), arcs of different radii at the
    // corners and a curved bottom: exercises straight edges, arcs and full cover.
    let shape = CurvedClipShape {
        bounds: [24.5, 3.0, 62.0, 141.0],
        radii_x: [12.0, 6.0, 10.0, 4.0],
        radii_y: [9.0, 6.0, 14.0, 4.0],
    };
    let mut unclipped = Scene::default();
    paint_probes(&mut unclipped, &tiles, None);
    let reference = gpu.render(&mut unclipped, 64, 144);
    let mut clipped = Scene::default();
    paint_probes(&mut clipped, &tiles, Some(shape));
    let image = gpu.render(&mut clipped, 64, 144);

    let mut partially_covered = [0usize; 9];
    for y in 0..144u32 {
        for x in 0..64u32 {
            let c = shape.coverage([x as f32 + 0.5, y as f32 + 0.5]);
            for channel in 0..3 {
                let base = f32::from(reference.get_pixel(x, y).0[channel]) / 255.0;
                let actual = f32::from(image.get_pixel(x, y).0[channel]) / 255.0;
                close(actual, base * c, ("C multiplies", x, y, channel));
            }
            if (0.2..0.8).contains(&c) && reference.get_pixel(x, y).0[0] > 40 {
                partially_covered[(y / 16) as usize] += 1;
            }
        }
    }
    // Every class really met a partially covered edge, so single and double application
    // (C versus C squared) differ in what was checked: the path in particular.
    for (row, count) in partially_covered.iter().enumerate() {
        assert!(*count > 0, "row {row} never crossed the clip edge");
    }
}

// ---- R16: membership follows paint order, not GPU batches ----

#[test]
fn scope_membership_follows_paint_order_not_batches() {
    let mut gpu = Gpu::new();
    let tiles = Tiles::new(&mut gpu);
    let mut state = 0x2545_f491u32;
    let mut random = move |bound: u32| {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (state >> 16) % bound
    };
    let (columns, rows, cell) = (8u32, 6u32, 16.0f32);
    let mut scene = Scene::default();
    // The same primitives without any shape, to know every tile's unclipped level.
    let mut base_scene = Scene::default();
    let mut stack: Vec<CurvedClipShape> = Vec::new();
    // The shapes open when each tile was painted: membership by paint order.
    let mut members: Vec<Vec<CurvedClipShape>> = Vec::new();
    for index in 0..columns * rows {
        // Randomly close or open scopes between tiles; depth is bounded by four.
        while !stack.is_empty() && random(3) == 0 {
            stack.pop();
            scene.pop_curved_clip();
        }
        if stack.len() < 4 && random(2) == 0 {
            let left = random(columns * 16) as f32;
            let top = random(rows * 16) as f32;
            let shape = CurvedClipShape {
                bounds: [
                    left,
                    top,
                    left + 24.0 + random(80) as f32,
                    top + 24.0 + random(60) as f32,
                ],
                radii_x: [random(12) as f32; 4],
                radii_y: [random(12) as f32; 4],
            };
            stack.push(shape);
            scene.push_curved_clip(shape);
        }
        let (column, row) = (index % columns, index / columns);
        let area = bounds(column as f32 * cell, row as f32 * cell, cell, cell);
        let kind = random(4);
        for target in [&mut scene, &mut base_scene] {
            match kind {
                0 => target.insert_primitive(quad(area, white(), Default::default())),
                1 => target.insert_primitive(quad(area, gradient(white()), Default::default())),
                2 => target.insert_primitive(mono_sprite(area, tiles.mono, white())),
                _ => target.insert_primitive(poly_sprite(area, tiles.color)),
            }
        }
        members.push(stack.clone());
    }
    while stack.pop().is_some() {
        scene.pop_curved_clip();
    }
    let (width, height) = ((columns * 16) as i32, (rows * 16) as i32);
    let image = gpu.render(&mut scene, width, height);
    let base = gpu.render(&mut base_scene, width, height);

    let mut partially_cut = 0;
    for index in 0..columns * rows {
        let (column, row) = (index % columns, index / columns);
        for dy in 0..16 {
            for dx in 0..16 {
                let (x, y) = (column * 16 + dx, row * 16 + dy);
                let p = [x as f32 + 0.5, y as f32 + 0.5];
                let c = curved_clip_coverage(members[index as usize].iter(), p);
                for channel in 0..3 {
                    let unclipped = f32::from(base.get_pixel(x, y).0[channel]) / 255.0;
                    let actual = f32::from(image.get_pixel(x, y).0[channel]) / 255.0;
                    close(actual, unclipped * c, ("membership", index, x, y, channel));
                }
                if c < 0.99 && base.get_pixel(x, y).0[0] > 0 {
                    partially_cut += 1;
                }
            }
        }
    }
    assert!(partially_cut > 200, "the random scene must actually clip");
}

// ---- R17: empty scopes and deep nesting ----

#[test]
fn deep_nesting_and_empty_scopes_keep_order_and_never_reveal_clipped_content() {
    let mut gpu = Gpu::new();
    let shape = CurvedClipShape {
        bounds: [4.0, 4.0, 60.0, 60.0],
        radii_x: [10.0; 4],
        radii_y: [10.0; 4],
    };
    let single = render_shape(&mut gpu, &[shape], 64);
    // 300 identical nested shapes: the minimum of equal coverages, no shape dropped.
    let deep = render_shape(&mut gpu, &[shape; 300], 64);
    assert_eq!(single.as_raw(), deep.as_raw());
    // 300 nested shapes whose middle one is empty: everything stays hidden.
    let mut chain = vec![shape; 150];
    chain.push(CurvedClipShape::default());
    chain.extend(vec![shape; 149]);
    let hidden = render_shape(&mut gpu, &chain, 64);
    assert!(hidden.pixels().all(|pixel| pixel.0[0] == 0));

    // Empty scopes between primitives change nothing: same pixels, same chain table use.
    let plain = {
        let mut scene = Scene::default();
        scene.push_curved_clip(shape);
        scene.insert_primitive(quad(
            bounds(0.0, 0.0, 64.0, 32.0),
            white(),
            Default::default(),
        ));
        scene.pop_curved_clip();
        scene.insert_primitive(quad(
            bounds(0.0, 32.0, 64.0, 32.0),
            gradient(white()),
            Default::default(),
        ));
        gpu.render(&mut scene, 64, 64)
    };
    let with_empty_scopes = {
        let mut scene = Scene::default();
        scene.push_curved_clip(CurvedClipShape::default());
        scene.pop_curved_clip();
        scene.push_curved_clip(shape);
        scene.push_curved_clip(CurvedClipShape::default());
        scene.pop_curved_clip();
        scene.insert_primitive(quad(
            bounds(0.0, 0.0, 64.0, 32.0),
            white(),
            Default::default(),
        ));
        scene.pop_curved_clip();
        scene.push_curved_clip(shape);
        scene.pop_curved_clip();
        scene.insert_primitive(quad(
            bounds(0.0, 32.0, 64.0, 32.0),
            gradient(white()),
            Default::default(),
        ));
        gpu.render(&mut scene, 64, 64)
    };
    assert_eq!(plain.as_raw(), with_empty_scopes.as_raw());
    // Content after an empty scope closes is visible again.
    assert!(red(&with_empty_scopes, 32, 48) > 0.95);
}

// ---- A11: replay equivalence at the scene level ----

/// One subtree under its own curved shape: a probe of every class, positioned from `ox`.
fn paint_subtree(scene: &mut Scene, ox: f32, tiles: &Tiles) {
    scene.push_curved_clip(CurvedClipShape {
        bounds: [ox + 6.0, 5.0, ox + 94.0, 135.0],
        radii_x: [14.0, 6.0, 20.0, 3.0],
        radii_y: [10.0, 6.0, 22.0, 3.0],
    });
    let cell = |index: usize| bounds(ox + 8.0, index as f32 * 15.0 + 2.0, 84.0, 12.0);
    scene.insert_primitive(quad(cell(0), white(), Default::default()));
    let mut rounded = quad(cell(1), gradient(white()), Default::default());
    rounded.corner_radii = Corners::all(ScaledPixels(5.0));
    rounded.border_widths = Edges::all(ScaledPixels(2.0));
    rounded.border_color = vespertine::rgb(0x40a0e0).into();
    scene.insert_primitive(rounded);
    scene.insert_primitive(shadow(
        bounds(ox + 20.0, 2.0 * 15.0 + 4.0, 56.0, 6.0),
        bounds(ox, 2.0 * 15.0, 100.0, 15.0),
        vespertine::rgb(0xffffff).into(),
    ));
    scene.insert_primitive(triangle(cell(3), vespertine::rgb(0x8fd16a).into()));
    scene.insert_primitive(underline(
        bounds(ox + 8.0, 4.0 * 15.0 + 6.0, 84.0, 3.0),
        white(),
    ));
    scene.insert_primitive(mono_sprite(cell(5), tiles.mono, white()));
    scene.insert_primitive(poly_sprite(cell(6), tiles.color));
    scene.insert_primitive(subpixel_sprite(cell(7), tiles.subpixel, white()));
    scene.pop_curved_clip();
}

fn ancestor(radius: f32) -> CurvedClipShape {
    CurvedClipShape {
        bounds: [2.0, 2.0, 198.0, 138.0],
        radii_x: [radius; 4],
        radii_y: [radius; 4],
    }
}

#[test]
fn replayed_subtrees_render_identically_to_direct_painting() {
    let mut gpu = Gpu::new();
    let tiles = Tiles::new(&mut gpu);
    let (width, height) = (200, 140);

    // Direct painting under an ancestor shape.
    let direct_under = |radius: f32| {
        let mut scene = Scene::default();
        scene.push_curved_clip(ancestor(radius));
        paint_subtree(&mut scene, 0.0, &tiles);
        paint_subtree(&mut scene, 100.0, &tiles);
        scene.pop_curved_clip();
        scene
    };
    let mut direct = direct_under(30.0);
    let direct_image = gpu.render(&mut direct, width, height);
    assert!(direct.curved_clips.len() >= 3);

    // Retained: the whole recorded range replayed into a new frame.
    let mut retained = Scene::default();
    retained.replay(0..direct.len(), &direct);
    let retained_image = gpu.render(&mut retained, width, height);
    assert_eq!(direct_image.as_raw(), retained_image.as_raw(), "retained");
    assert_eq!(retained.curved_clips, direct.curved_clips);

    // Spliced: the subtree ranges of a frame recorded under one ancestor are replayed
    // under a different destination ancestor and equal direct painting under it.
    let mut recorded = Scene::default();
    recorded.push_curved_clip(ancestor(8.0));
    let first_start = recorded.len();
    paint_subtree(&mut recorded, 0.0, &tiles);
    let first_end = recorded.len();
    paint_subtree(&mut recorded, 100.0, &tiles);
    let second_end = recorded.len();
    recorded.pop_curved_clip();
    let mut spliced = Scene::default();
    spliced.push_curved_clip(ancestor(30.0));
    spliced.replay_translated(first_start..first_end, &recorded, Default::default());
    spliced.replay_translated(first_end..second_end, &recorded, Default::default());
    spliced.pop_curved_clip();
    let spliced_image = gpu.render(&mut spliced, width, height);
    assert_eq!(direct_image.as_raw(), spliced_image.as_raw(), "spliced");
    // The ancestor really matters: the two ancestors differ in the corner pixels.
    let mut other = direct_under(8.0);
    let other_image = gpu.render(&mut other, width, height);
    assert_ne!(direct_image.as_raw(), other_image.as_raw());

    // Layer replay: a layered frame replayed into a new frame equals direct layered painting.
    let layered = |scene: &mut Scene, ox: f32| {
        scene.push_layer(bounds(ox, 0.0, 100.0, 140.0));
        paint_subtree(scene, ox, &tiles);
        scene.pop_layer();
    };
    let mut direct_layers = Scene::default();
    layered(&mut direct_layers, 0.0);
    layered(&mut direct_layers, 100.0);
    let direct_layers_image = gpu.render(&mut direct_layers, width, height);
    let mut layered_replay = Scene::default();
    layered_replay.replay(0..direct_layers.len(), &direct_layers);
    let layered_replay_image = gpu.render(&mut layered_replay, width, height);
    assert_eq!(
        direct_layers_image.as_raw(),
        layered_replay_image.as_raw(),
        "layer replay"
    );
    // Every primitive class really painted something under the shapes.
    assert!(direct_image.pixels().filter(|pixel| pixel.0[0] > 0).count() > 3000);
}

// ---- A12: no extra passes, targets or curved-clip data ----

fn quad_grid(scene: &mut Scene, with_clips: bool, with_path: bool) {
    for index in 0..20 {
        let (column, row) = (index % 5, index / 5);
        let area = bounds(column as f32 * 20.0, row as f32 * 20.0, 18.0, 18.0);
        let shape = CurvedClipShape {
            bounds: [
                area.origin.x.0,
                area.origin.y.0,
                area.origin.x.0 + 18.0,
                area.origin.y.0 + 18.0,
            ],
            radii_x: [6.0; 4],
            radii_y: [6.0; 4],
        };
        if with_clips {
            scene.push_curved_clip(shape);
        }
        scene.insert_primitive(quad(area, white(), Default::default()));
        if with_path && index % 7 == 0 {
            scene.insert_primitive(triangle(area, white()));
        }
        if with_clips {
            scene.pop_curved_clip();
        }
    }
}

#[test]
fn opted_in_scenes_add_no_pass_target_or_allocation_beyond_the_shape_records() {
    let counters = |gpu: &mut Gpu, with_clips: bool, with_path: bool| {
        gpu.renderer.reset_record_stats();
        let mut scene = Scene::default();
        quad_grid(&mut scene, with_clips, with_path);
        let shape_count = scene.curved_clips.len() as u64;
        gpu.render(&mut scene, 100, 80);
        (
            gpu.renderer.record_stats(),
            gpu.renderer.clip_stats(),
            shape_count,
        )
    };

    // The first frame of a fresh renderer: a scene without opted-in elements allocates and
    // uploads no curved-clip data.
    let (rectangular, rectangular_clip, shapes) = counters(&mut Gpu::new(), false, false);
    assert_eq!(shapes, 0);
    assert_eq!(rectangular_clip.buffer_allocations, 0);
    assert_eq!(rectangular_clip.upload_bytes, 0);
    assert_eq!(rectangular.render_passes, 1);

    // The same scene with 20 opted-in shapes on a fresh renderer: the same passes and
    // offscreen targets, and 48 bytes of shape plus a 4 byte chain link per shape.
    let mut gpu = Gpu::new();
    let (curved, curved_clip, shapes) = counters(&mut gpu, true, false);
    assert_eq!(shapes, 20);
    assert_eq!(curved.render_passes, rectangular.render_passes);
    assert_eq!(
        curved.path_intermediate_allocations,
        rectangular.path_intermediate_allocations
    );
    assert_eq!(curved_clip.upload_bytes, 20 * (48 + 4));
    assert_eq!(curved_clip.buffer_allocations, 1);
    // A second frame reuses the buffers: no allocation, same upload, no new offscreen target.
    let (again, again_clip, _) = counters(&mut gpu, true, false);
    assert_eq!(again_clip.buffer_allocations, 0);
    assert_eq!(again.path_intermediate_allocations, 0);
    assert_eq!(again_clip.upload_bytes, 20 * (48 + 4));
    // Back to a rectangular scene: nothing uploaded, nothing allocated.
    let (_, rectangular_again, _) = counters(&mut gpu, false, false);
    assert_eq!(rectangular_again.buffer_allocations, 0);
    assert_eq!(rectangular_again.upload_bytes, 0);

    // With paths: opting in adds no pass over the rectangular scene.
    let (rect_paths, _, _) = counters(&mut Gpu::new(), false, true);
    let (curved_paths, _, _) = counters(&mut Gpu::new(), true, true);
    assert!(rect_paths.render_passes > 1);
    assert_eq!(curved_paths.render_passes, rect_paths.render_passes);
    assert_eq!(
        curved_paths.path_intermediate_allocations,
        rect_paths.path_intermediate_allocations
    );
}

#[test]
fn paths_of_one_order_under_different_shapes_share_a_pass_and_each_follow_their_own() {
    let mut gpu = Gpu::new();
    let cells: Vec<(Bounds<ScaledPixels>, CurvedClipShape)> = (0..6)
        .map(|index| {
            let (column, row) = (index % 3, index / 3);
            let left = column as f32 * 40.0;
            let top = row as f32 * 40.0;
            let area = bounds(left + 2.0, top + 2.0, 36.0, 36.0);
            let shape = CurvedClipShape {
                bounds: [
                    left + 3.5 + index as f32 * 0.5,
                    top + 4.0,
                    left + 38.0,
                    top + 37.0,
                ],
                radii_x: [4.0 + index as f32 * 2.0; 4],
                radii_y: [3.0 + index as f32 * 2.0; 4],
            };
            (area, shape)
        })
        .collect();
    let build = |clipped: bool| {
        let mut scene = Scene::default();
        for (area, shape) in &cells {
            if clipped {
                scene.push_curved_clip(*shape);
            }
            scene.insert_primitive(triangle(*area, white()));
            if clipped {
                scene.pop_curved_clip();
            }
        }
        scene
    };
    gpu.renderer.reset_record_stats();
    let reference = gpu.render(&mut build(false), 120, 80);
    let unclipped = gpu.renderer.record_stats();
    gpu.renderer.reset_record_stats();
    let image = gpu.render(&mut build(true), 120, 80);
    let clipped = gpu.renderer.record_stats();
    // Six paths of one draw order under six different chains: still one rasterization pass
    // and one composite, as without the shapes.
    assert_eq!(clipped.render_passes, unclipped.render_passes);
    // Grouped path rasterization (PRF-007 R4): one rasterization pass and the main pass.
    assert_eq!(clipped.render_passes, 2);
    // Each triangle is cut once, at composite, by its own shape: C and not C squared.
    let mut partial = 0;
    for (area, shape) in &cells {
        for y in 0..80u32 {
            for x in 0..120u32 {
                let p = [x as f32 + 0.5, y as f32 + 0.5];
                let inside_cell = p[0] >= area.origin.x.0
                    && p[0] < area.origin.x.0 + 36.0
                    && p[1] >= area.origin.y.0
                    && p[1] < area.origin.y.0 + 36.0;
                if !inside_cell {
                    continue;
                }
                let c = shape.coverage(p);
                let base = f32::from(reference.get_pixel(x, y).0[0]) / 255.0;
                close(red(&image, x, y), base * c, ("own shape", x, y));
                if c > 0.1 && c < 0.9 && base > 0.5 {
                    partial += 1;
                }
            }
        }
    }
    assert!(partial > 10, "paths must meet the curved edges");
}
