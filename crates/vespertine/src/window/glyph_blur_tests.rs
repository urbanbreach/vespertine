//! Tests of `Window::paint_glyph_transformed` (RND-005): what it records in the scene, how
//! it shares raster identity with `paint_glyph`, and that animating its transformation or
//! blur never rasterizes or uploads anything. Pixels are checked in `vespertine_wgpu`.

use crate::{
    AppContext as _, AtlasTextureKind, Bounds, ContentMask, Context, DevicePixels, EdgeFade,
    FontId, GlyphId, Hsla, IntoElement, MonochromeSprite, NoopTextSystem, PlatformTextSystem,
    Render, RenderGlyphParams, ScaledPixels, Size, Styled as _, TestAppContext, TestDispatcher,
    TextRenderingMode, TransformationMatrix, Window, blur_expanded_bounds, canvas, hsla, point, px,
    radians, size, white,
};
use anyhow::{Result, anyhow};
use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

/// A glyph with a 10 by 12 device pixel raster whose box starts at (-1, -9) from the origin.
const GLYPH: GlyphId = GlyphId(1);
/// A glyph with empty raster bounds.
const EMPTY_GLYPH: GlyphId = GlyphId(2);
/// A glyph whose raster bounds fail.
const BAD_FONT_GLYPH: GlyphId = GlyphId(3);
/// A glyph with bounds whose rasterization fails.
const BAD_RASTER_GLYPH: GlyphId = GlyphId(4);
const FONT: FontId = FontId(1);

/// Counts every query and rasterization of the text system.
struct GlyphProbe {
    noop: NoopTextSystem,
    raster_bounds_calls: AtomicUsize,
    rasterize_calls: AtomicUsize,
    rasterized_bytes: AtomicUsize,
    params: parking_lot::Mutex<Vec<RenderGlyphParams>>,
}

impl Default for GlyphProbe {
    fn default() -> Self {
        Self {
            noop: NoopTextSystem::new(),
            raster_bounds_calls: AtomicUsize::new(0),
            rasterize_calls: AtomicUsize::new(0),
            rasterized_bytes: AtomicUsize::new(0),
            params: parking_lot::Mutex::new(Vec::new()),
        }
    }
}

impl GlyphProbe {
    fn counts(&self) -> (usize, usize, usize) {
        (
            self.raster_bounds_calls.load(Ordering::SeqCst),
            self.rasterize_calls.load(Ordering::SeqCst),
            self.rasterized_bytes.load(Ordering::SeqCst),
        )
    }
}

impl PlatformTextSystem for GlyphProbe {
    fn add_fonts(&self, fonts: Vec<std::borrow::Cow<'static, [u8]>>) -> Result<()> {
        self.noop.add_fonts(fonts)
    }

    fn all_font_names(&self) -> Vec<String> {
        self.noop.all_font_names()
    }

    fn font_id(&self, descriptor: &crate::Font) -> Result<FontId> {
        self.noop.font_id(descriptor)
    }

    fn font_metrics(&self, font_id: FontId) -> crate::FontMetrics {
        self.noop.font_metrics(font_id)
    }

    fn typographic_bounds(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Bounds<f32>> {
        self.noop.typographic_bounds(font_id, glyph_id)
    }

    fn advance(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Size<f32>> {
        self.noop.advance(font_id, glyph_id)
    }

    fn glyph_for_char(&self, font_id: FontId, ch: char) -> Option<GlyphId> {
        self.noop.glyph_for_char(font_id, ch)
    }

    fn glyph_raster_bounds(&self, params: &RenderGlyphParams) -> Result<Bounds<DevicePixels>> {
        self.raster_bounds_calls.fetch_add(1, Ordering::SeqCst);
        self.params.lock().push(params.clone());
        match params.glyph_id {
            EMPTY_GLYPH => Ok(Bounds::default()),
            BAD_FONT_GLYPH => Err(anyhow!("font failure")),
            _ => Ok(Bounds {
                origin: point(DevicePixels(-1), DevicePixels(-9)),
                size: size(DevicePixels(10), DevicePixels(12)),
            }),
        }
    }

    fn rasterize_glyph(
        &self,
        params: &RenderGlyphParams,
        raster_bounds: Bounds<DevicePixels>,
    ) -> Result<(Size<DevicePixels>, Vec<u8>)> {
        self.rasterize_calls.fetch_add(1, Ordering::SeqCst);
        if params.glyph_id == BAD_RASTER_GLYPH {
            return Err(anyhow!("raster failure"));
        }
        let bytes = raster_bounds.size.width.0 as usize * raster_bounds.size.height.0 as usize;
        self.rasterized_bytes.fetch_add(bytes, Ordering::SeqCst);
        Ok((raster_bounds.size, vec![255; bytes]))
    }

    fn layout_line(
        &self,
        text: &str,
        font_size: crate::Pixels,
        runs: &[crate::FontRun],
    ) -> crate::LineLayout {
        self.noop.layout_line(text, font_size, runs)
    }

    fn recommended_rendering_mode(&self, _: FontId, _: crate::Pixels) -> TextRenderingMode {
        TextRenderingMode::Subpixel
    }

    fn glyph_dilation_for_color(&self, color: Hsla) -> u8 {
        if color.l > 0.5 { 2 } else { 0 }
    }
}

/// What one paint callback recorded.
#[derive(Default)]
struct Painted {
    mono: Vec<MonochromeSprite>,
    subpixel: usize,
    result: Option<Result<(), String>>,
}

struct PaintView(Rc<dyn Fn(&mut Window)>);

impl Render for PaintView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let paint = self.0.clone();
        canvas(|_, _, _| {}, move |_, _, window, _| paint(window)).size_full()
    }
}

fn new_cx(probe: &Arc<GlyphProbe>) -> TestAppContext {
    TestAppContext::build_with_text_system(TestDispatcher::new(0), None, probe.clone())
}

/// Opens a 600 by 360 logical window (scale factor 2) whose paint phase runs `paint` on an
/// emptied scene and records the sprites it produced.
fn open(
    cx: &mut TestAppContext,
    paint: impl Fn(&mut Window) -> Result<()> + 'static,
) -> (crate::AnyWindowHandle, Rc<RefCell<Painted>>) {
    let painted = Rc::new(RefCell::new(Painted::default()));
    let sink = painted.clone();
    let handle = cx.open_window(size(px(600.0), px(360.0)), move |_, _| {
        let sink = sink.clone();
        PaintView(Rc::new(move |window| {
            window.next_frame.scene.clear();
            let result = paint(window).map_err(|error| error.to_string());
            let mut painted = sink.borrow_mut();
            painted.mono = window.next_frame.scene.monochrome_sprites.clone();
            painted.subpixel = window.next_frame.scene.subpixel_sprites.len();
            painted.result = Some(result);
        }))
    });
    (handle.into(), painted)
}

/// Draws one frame. An unchanged view replays its retained output (PRF-001), so the refresh
/// makes the frame construct and paint the view again.
fn draw(cx: &mut TestAppContext, handle: crate::AnyWindowHandle) {
    cx.update_window(handle, |_, window, cx| {
        window.refresh();
        window.draw(cx).clear(cx)
    })
    .unwrap();
}

/// Paints once and returns what was recorded.
fn paint_once(
    cx: &mut TestAppContext,
    paint: impl Fn(&mut Window) -> Result<()> + 'static,
) -> Painted {
    let (handle, painted) = open(cx, paint);
    draw(cx, handle);
    painted.take()
}

fn glyph(
    window: &mut Window,
    origin: (f32, f32),
    glyph_id: GlyphId,
    color: Hsla,
    transformation: TransformationMatrix,
    blur: f32,
) -> Result<()> {
    window.paint_glyph_transformed(
        point(px(origin.0), px(origin.1)),
        FONT,
        glyph_id,
        px(16.0),
        color,
        transformation,
        px(blur),
    )
}

fn rotation_about(center: (f32, f32), degrees: f32) -> TransformationMatrix {
    TransformationMatrix::unit()
        .translate(point(ScaledPixels(center.0), ScaledPixels(center.1)))
        .rotate(radians(degrees.to_radians()))
        .translate(point(ScaledPixels(-center.0), ScaledPixels(-center.1)))
}

fn assert_ok(painted: &Painted) {
    assert_eq!(painted.result, Some(Ok(())));
}

#[test]
fn halo_expands_bounds_by_three_device_sigmas_and_ink_does_not_jump() {
    // A2: logical sigma 1 at scale factor 2 is device sigma 2: 6 device pixels per side.
    let probe = Arc::new(GlyphProbe::default());
    let mut cx = new_cx(&probe);
    let unit = TransformationMatrix::unit();
    let painted = paint_once(&mut cx, move |window| {
        glyph(window, (10.0, 20.0), GLYPH, white(), unit, 0.0)?;
        glyph(window, (10.0, 20.0), GLYPH, white(), unit, 1.0)?;
        glyph(window, (10.0, 20.0), GLYPH, white(), unit, 4.0)
    });
    assert_ok(&painted);
    let [plain, blurred, wide] = painted.mono[..] else {
        panic!("expected three sprites");
    };
    // Raster (-1, -9) 10 x 12 from the device origin (20, 40).
    assert_eq!(
        plain.bounds.origin,
        point(ScaledPixels(19.0), ScaledPixels(31.0))
    );
    assert_eq!(
        plain.bounds.size,
        size(ScaledPixels(10.0), ScaledPixels(12.0))
    );
    assert_eq!(plain.blur, 0.0);

    assert_eq!(blurred.blur, 2.0);
    assert_eq!(
        blurred.bounds.origin,
        point(ScaledPixels(13.0), ScaledPixels(25.0))
    );
    assert_eq!(
        blurred.bounds.size,
        size(ScaledPixels(22.0), ScaledPixels(24.0))
    );
    // The original ink rectangle inside the inflated one is where it always was.
    let halo = 6.0;
    assert_eq!(blurred.bounds.origin.x.0 + halo, plain.bounds.origin.x.0);
    assert_eq!(blurred.bounds.origin.y.0 + halo, plain.bounds.origin.y.0);
    assert_eq!(
        blurred.bounds.size.width.0 - 2.0 * halo,
        plain.bounds.size.width.0
    );
    assert_eq!(blurred.tile.bounds, plain.tile.bounds);

    // A6: logical 4 at scale 2 is device sigma 8: halo 24, tap radius capped at 12.
    assert_eq!(wide.blur, 8.0);
    assert_eq!(
        wide.bounds.origin,
        point(ScaledPixels(-5.0), ScaledPixels(7.0))
    );
    assert_eq!(
        wide.bounds.size,
        size(ScaledPixels(58.0), ScaledPixels(60.0))
    );
    assert_eq!(crate::blur_kernel_radius(wide.blur), 12);
    assert_eq!(wide.bounds, blur_expanded_bounds(plain.bounds, 8.0));
}

#[test]
fn zero_blur_identity_matches_ordinary_glyph_painting_and_negative_blur_is_zero() {
    // A1 (sprite level): the same sprite as `paint_glyph` records.
    let probe = Arc::new(GlyphProbe::default());
    let mut cx = new_cx(&probe);
    let unit = TransformationMatrix::unit();
    let color = hsla(0.1, 0.5, 0.3, 1.0);
    let painted = paint_once(&mut cx, move |window| {
        window.paint_glyph(point(px(10.3), px(20.6)), FONT, GLYPH, px(16.0), color)?;
        glyph(window, (10.3, 20.6), GLYPH, color, unit, 0.0)?;
        glyph(window, (10.3, 20.6), GLYPH, color, unit, -5.0)
    });
    assert_ok(&painted);
    let [ordinary, zero, negative] = painted.mono[..] else {
        panic!("expected three sprites");
    };
    for sprite in [zero, negative] {
        assert_eq!(sprite.blur, 0.0);
        assert_eq!(sprite.bounds, ordinary.bounds);
        assert_eq!(sprite.tile, ordinary.tile);
        assert_eq!(sprite.color, ordinary.color);
        assert_eq!(sprite.content_mask, ordinary.content_mask);
        assert_eq!(sprite.transformation, ordinary.transformation);
        assert_eq!(sprite.scope, ordinary.scope);
    }
    // One raster served all three paints.
    assert_eq!(probe.counts().1, 1);
}

#[test]
fn raster_identity_and_origin_quantization_match_ordinary_painting() {
    let probe = Arc::new(GlyphProbe::default());
    let mut cx = new_cx(&probe);
    let unit = TransformationMatrix::unit();
    let bright = hsla(0.0, 0.0, 0.9, 1.0);
    let dark = hsla(0.0, 0.0, 0.1, 1.0);
    // Scale factor 2. Logical origin -> device origin -> (integer x, x variant, integer y):
    // quarter pixels horizontally and whole pixels vertically, half toward zero.
    let cases = [
        ((10.3, 20.6), (20.0, 2, 41.0)), // (20.6, 41.2): 82.4 quarters -> 82
        ((1.0625, 3.5), (2.0, 0, 7.0)),  // (2.125, 7.0): 8.5 quarters -> 8
        ((3.0625, 2.75), (6.0, 0, 5.0)), // (6.125, 5.5): 24.5 quarters -> 24; 5.5 -> 5
        ((0.1875, 0.25), (0.0, 1, 0.0)), // (0.375, 0.5): 1.5 quarters -> 1; 0.5 -> 0
    ];
    for color in [bright, dark] {
        for (origin, (integer_x, variant_x, integer_y)) in cases {
            probe.params.lock().clear();
            let painted = paint_once(&mut cx, move |window| {
                window.paint_glyph(
                    point(px(origin.0), px(origin.1)),
                    FONT,
                    GLYPH,
                    px(16.0),
                    color,
                )?;
                glyph(window, origin, GLYPH, color, unit, 3.0)
            });
            assert_ok(&painted);
            // The transformed painter hit the raster-bounds cache entry the ordinary painter
            // made (or both hit an entry of an earlier case with the same raster identity),
            // so it asked for no other raster identity, and it got the same atlas tile.
            let params = probe.params.lock().clone();
            assert!(params.len() <= 1, "{origin:?}");
            if let Some(ordinary) = params.first() {
                assert_eq!(ordinary.dilation, if color.l > 0.5 { 2 } else { 0 });
                assert!(!ordinary.is_emoji && !ordinary.subpixel_rendering);
                assert_eq!(ordinary.subpixel_variant.x, variant_x, "{origin:?}");
                assert_eq!(ordinary.subpixel_variant.y, 0);
            }
            let [plain, blurred] = painted.mono[..] else {
                panic!("expected two sprites");
            };
            assert_eq!(plain.tile, blurred.tile);
            // Raster box (-1, -9) from the integer origin; blur 3 logical = 6 device, halo 18.
            assert_eq!(plain.bounds.origin.x.0, integer_x - 1.0, "{origin:?}");
            assert_eq!(plain.bounds.origin.y.0, integer_y - 9.0, "{origin:?}");
            assert_eq!(blurred.bounds.origin.x.0, integer_x - 1.0 - 18.0);
            assert_eq!(blurred.bounds.origin.y.0, integer_y - 9.0 - 18.0);
        }
    }
}

#[test]
fn subpixel_capable_window_paints_transformed_glyphs_monochrome_only() {
    // A11: ordinary text stays on its (subpixel) pipeline; transformed or blurred glyphs
    // use the monochrome mask.
    let probe = Arc::new(GlyphProbe::default());
    let mut cx = new_cx(&probe);
    let unit = TransformationMatrix::unit();
    let (handle, painted) = open(&mut cx, move |window| {
        window.text_rendering_mode.set(TextRenderingMode::Subpixel);
        window.paint_glyph(point(px(10.0), px(20.0)), FONT, GLYPH, px(16.0), white())?;
        glyph(window, (30.0, 20.0), GLYPH, white(), unit, 0.0)?;
        glyph(
            window,
            (50.0, 20.0),
            GLYPH,
            white(),
            rotation_about((100.0, 40.0), 8.0),
            2.0,
        )
    });
    cx.test_window(handle)
        .set_subpixel_rendering_supported(true);
    draw(&mut cx, handle);
    let painted = painted.take();
    assert_ok(&painted);
    assert_eq!(
        painted.subpixel, 1,
        "ordinary text keeps the subpixel pipeline"
    );
    assert_eq!(painted.mono.len(), 2);
    for sprite in &painted.mono {
        assert_eq!(sprite.tile.texture_id.kind, AtlasTextureKind::Monochrome);
    }
    let params = probe.params.lock().clone();
    assert!(params.iter().any(|p| p.subpixel_rendering));
    assert!(params.iter().any(|p| !p.subpixel_rendering && !p.is_emoji));
}

#[test]
fn empty_bounds_paint_nothing_and_errors_propagate() {
    // A8.
    let probe = Arc::new(GlyphProbe::default());
    let mut cx = new_cx(&probe);
    let unit = TransformationMatrix::unit();

    let painted = paint_once(&mut cx, move |window| {
        glyph(window, (10.0, 20.0), EMPTY_GLYPH, white(), unit, 2.0)
    });
    assert_ok(&painted);
    assert!(painted.mono.is_empty() && painted.subpixel == 0);
    assert_eq!(
        probe.counts().1,
        0,
        "nothing is rasterized for empty bounds"
    );

    let painted = paint_once(&mut cx, move |window| {
        glyph(window, (10.0, 20.0), BAD_FONT_GLYPH, white(), unit, 2.0)
    });
    assert_eq!(painted.result, Some(Err("font failure".to_string())));
    assert!(painted.mono.is_empty());

    let painted = paint_once(&mut cx, move |window| {
        glyph(window, (10.0, 20.0), BAD_RASTER_GLYPH, white(), unit, 2.0)
    });
    assert_eq!(painted.result, Some(Err("raster failure".to_string())));
    assert!(painted.mono.is_empty());
}

#[test]
fn rectangular_content_mask_culls_and_clips_by_the_transformed_quad() {
    // A8: the rectangular content mask stays effective for transformed glyphs.
    let probe = Arc::new(GlyphProbe::default());
    let mut cx = new_cx(&probe);
    let unit = TransformationMatrix::unit();
    let mask = ContentMask {
        bounds: Bounds {
            origin: point(px(100.0), px(100.0)),
            size: size(px(50.0), px(50.0)),
        },
    };
    let painted = paint_once(&mut cx, move |window| {
        window.with_content_mask(Some(mask), |window| {
            // Entirely outside the mask: culled.
            glyph(window, (10.0, 20.0), GLYPH, white(), unit, 0.0)?;
            // Straddling the mask: kept, carrying the mask.
            glyph(window, (100.0, 110.0), GLYPH, white(), unit, 0.0)?;
            // Recorded outside the mask but translated into it (device px): kept.
            let into_mask = unit.translate(point(ScaledPixels(200.0), ScaledPixels(160.0)));
            glyph(window, (10.0, 20.0), GLYPH, white(), into_mask, 0.0)?;
            // Inside the mask but translated away from it: culled.
            let away = unit.translate(point(ScaledPixels(900.0), ScaledPixels(0.0)));
            glyph(window, (110.0, 110.0), GLYPH, white(), away, 0.0)
        })
    });
    assert_ok(&painted);
    assert_eq!(painted.mono.len(), 2);
    for sprite in &painted.mono {
        assert_eq!(
            sprite.content_mask.bounds,
            Bounds {
                origin: point(ScaledPixels(200.0), ScaledPixels(200.0)),
                size: size(ScaledPixels(100.0), ScaledPixels(100.0)),
            }
        );
    }
}

#[test]
fn sprite_carries_opacity_edge_fade_and_rounded_clip_scopes() {
    // A8: combination with element opacity, scoped fading and rounded clips.
    let probe = Arc::new(GlyphProbe::default());
    let mut cx = new_cx(&probe);
    let region = Bounds {
        origin: point(px(40.0), px(30.0)),
        size: size(px(420.0), px(430.0)),
    };
    let painted = paint_once(&mut cx, move |window| {
        let mut fade = EdgeFade::new(region, px(48.0));
        fade.right = true;
        window.with_element_opacity(Some(0.5), |window| {
            window.with_rounded_clip(region, px(24.0), |window| {
                window.with_edge_fade(Some(fade), |window| {
                    glyph(
                        window,
                        (70.0, 100.0),
                        GLYPH,
                        hsla(0.5, 1.0, 0.5, 0.8),
                        rotation_about((140.0, 100.0), 8.0),
                        2.5,
                    )
                })
            })
        })
    });
    assert_ok(&painted);
    let [sprite] = painted.mono[..] else {
        panic!("expected one sprite");
    };
    assert!((sprite.color.a - 0.4).abs() < 1e-6);
    // Device-space (scale factor 2) scope parameters.
    assert_eq!(sprite.scope.clip_bounds, [80.0, 60.0, 920.0, 920.0]);
    assert_eq!(sprite.scope.clip_radius, 48.0);
    assert_eq!(sprite.scope.fade_bands, [0.0, 0.0, 0.0, 96.0]);
    assert_eq!(sprite.scope.fade_edges[3], 920.0);
    assert!(sprite.scope.fade_active() && sprite.scope.rounded_clip_active());
    assert_eq!(sprite.blur, 5.0);
    assert_eq!(sprite.transformation, rotation_about((140.0, 100.0), 8.0));
    // The rounded clip narrows the rectangular mask to its bounds.
    assert_eq!(
        sprite.content_mask.bounds,
        Bounds {
            origin: point(ScaledPixels(80.0), ScaledPixels(60.0)),
            size: size(ScaledPixels(840.0), ScaledPixels(660.0)),
        }
    );
}

#[test]
fn animating_transform_and_blur_never_rasterizes_or_uploads() {
    // A7: 100 copies of one glyph in a 600 by 360 logical window for 600 frames, angle
    // varying continuously and logical sigma between 0 and 4.
    let probe = Arc::new(GlyphProbe::default());
    let mut cx = new_cx(&probe);
    let frame = Rc::new(Cell::new(0usize));
    let tiles = Rc::new(RefCell::new(HashSet::new()));
    let per_frame = Rc::new(RefCell::new(Vec::new()));
    let (handle, painted) = open(&mut cx, {
        let (tiles, per_frame, probe) = (tiles.clone(), per_frame.clone(), probe.clone());
        move |window| {
            let n = frame.get();
            frame.set(n + 1);
            let angle = n as f32 * 0.7;
            // 0 at every 50th frame, otherwise sweeping up to 4.
            let sigma = if n.is_multiple_of(50) {
                0.0
            } else {
                2.0 + 2.0 * (n as f32 * 0.05).sin()
            };
            for copy in 0..100 {
                let origin = (
                    20.0 + 50.0 * (copy % 10) as f32,
                    30.0 + 30.0 * (copy / 10) as f32,
                );
                let pivot = (origin.0 * 2.0, origin.1 * 2.0);
                glyph(
                    window,
                    origin,
                    GLYPH,
                    white(),
                    rotation_about(pivot, angle),
                    sigma,
                )?;
            }
            let sprites = &window.next_frame.scene.monochrome_sprites;
            let mut tiles = tiles.borrow_mut();
            for sprite in sprites {
                tiles.insert((sprite.tile.texture_id.index, sprite.tile.tile_id.0));
            }
            per_frame.borrow_mut().push((sprites.len(), probe.counts()));
            Ok(())
        }
    });
    drop(painted);
    for _ in 0..600 {
        draw(&mut cx, handle);
    }
    let per_frame = per_frame.take();
    assert!(per_frame.len() >= 600);
    let warm = per_frame[0].1;
    assert_eq!(warm.1, 1, "the glyph is rasterized once");
    assert!(per_frame.iter().all(|(sprites, _)| *sprites == 100));
    for (_, counts) in &per_frame[1..] {
        assert_eq!(
            *counts, warm,
            "no query, rasterization or upload after warm-up"
        );
    }
    assert_eq!(
        tiles.borrow().len(),
        1,
        "one atlas entry for every copy and frame"
    );

    // Contrast: changing the font size through ordinary painting rasterizes anew.
    let before = probe.counts();
    let frame = Cell::new(0usize);
    let (handle, _) = open(&mut cx, {
        move |window| {
            let n = frame.get();
            frame.set(n + 1);
            for copy in 0..100 {
                let origin = point(
                    px(20.0 + 50.0 * (copy % 10) as f32),
                    px(30.0 + 30.0 * (copy / 10) as f32),
                );
                window.paint_glyph(origin, FONT, GLYPH, px(14.0 + 0.01 * n as f32), white())?;
            }
            Ok(())
        }
    });
    for _ in 0..600 {
        draw(&mut cx, handle);
    }
    let after = probe.counts();
    assert!(after.1 - before.1 >= 600, "one raster per distinct size");
    assert!(after.2 > before.2);
}

/// ANM-002 R13: the label roll paints tilted, scaled and blurred glyphs through the transformed
/// painter (the same raster tile, sampled with a transform and a blur) and keeps plain translation
/// on the ordinary glyph path.
#[test]
fn rolling_label_glyphs_paint_through_the_transformed_painter() {
    use crate::motion::label::{RollGlyphPaint, glyph_transformation, paint_roll_glyph};

    let probe = Arc::new(GlyphProbe::default());
    let mut cx = new_cx(&probe);
    let pivot = point(px(40.0), px(30.0));
    let rolled = glyph_transformation(pivot, px(-5.0), 0.6, 2.0, 2.0);
    let retilted = glyph_transformation(pivot, px(-2.0), 0.8, 1.0, 2.0);
    let rising = glyph_transformation(pivot, px(-5.0), 1.0, 0.0, 2.0);
    let painted = paint_once(&mut cx, move |window| {
        let roll = |transformation, blur: f32| RollGlyphPaint {
            origin: point(px(36.0), px(34.0)),
            font_id: FONT,
            glyph_id: GLYPH,
            font_size: px(16.0),
            color: white(),
            opacity: 0.5,
            transformation,
            blur: px(blur),
            is_emoji: false,
        };
        paint_roll_glyph(window, &roll(rolled, 1.6))?;
        paint_roll_glyph(window, &roll(retilted, 0.8))?;
        paint_roll_glyph(window, &roll(rising, 0.0))
    });
    assert_ok(&painted);
    let [first, second, plain] = painted.mono[..] else {
        panic!("expected three sprites");
    };
    // Logical sigma times the scale factor of 2; the transform is attached to the sprite as is.
    assert_eq!((first.blur, first.transformation), (3.2, rolled));
    assert_eq!((second.blur, second.transformation), (1.6, retilted));
    // A pure translation without blur takes the ordinary path at the translated origin.
    assert_eq!(plain.blur, 0.0);
    assert_eq!(plain.transformation, TransformationMatrix::unit());
    assert_eq!(
        probe.counts().1,
        1,
        "tilt, scale and blur reuse the one raster tile"
    );
}
