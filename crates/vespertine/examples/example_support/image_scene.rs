//! Headless rendering support shared by the RND-004 / RND-007 image examples and tests:
//! a window drawn through the real GPU renderer with an atlas that counts decodes and
//! uploads, generators for the procedural source images of the fidelity scenes, and the
//! fidelity scene painters themselves.
#![allow(dead_code)]

use std::{
    borrow::Cow,
    cell::Cell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use anyhow::Result;
use image::RgbaImage;
use vespertine::{
    AnyWindowHandle, App, AppContext as _, AtlasKey, AtlasTile, Bounds, Context, Corners,
    DevicePixels, Entity, HeadlessAppContext, ImageAlphaMask, IntoElement, ParentElement as _,
    Pixels, PlatformAtlas, PlatformHeadlessRenderer, Render, RenderImage, Scene, Size, Styled as _,
    Window, WindowHandle, canvas, cover_fit, div, point, px, rgb, size,
};

/// Counts the images the atlas had to build (decode and upload) and the bytes uploaded.
#[derive(Default)]
pub struct AtlasCounters {
    insertions: AtomicUsize,
    uploaded_bytes: AtomicUsize,
    failing: AtomicBool,
}

impl AtlasCounters {
    /// Number of atlas misses so far (images built into a tile).
    pub fn insertions(&self) -> usize {
        self.insertions.load(Ordering::SeqCst)
    }

    /// Makes every following atlas lookup fail with an error (an atlas that cannot allocate).
    pub fn set_failing(&self, failing: bool) {
        self.failing.store(failing, Ordering::SeqCst);
    }

    /// Number of bytes handed to the atlas for upload so far.
    pub fn uploaded_bytes(&self) -> usize {
        self.uploaded_bytes.load(Ordering::SeqCst)
    }
}

struct CountingAtlas {
    inner: Arc<dyn PlatformAtlas>,
    counters: Arc<AtlasCounters>,
}

impl PlatformAtlas for CountingAtlas {
    fn get_or_insert_with<'a>(
        &self,
        key: AtlasKey,
        build: &mut dyn FnMut() -> Result<Option<(Size<DevicePixels>, Cow<'a, [u8]>)>>,
    ) -> Result<Option<AtlasTile>> {
        if self.counters.failing.load(Ordering::SeqCst) {
            anyhow::bail!("the atlas cannot allocate the image");
        }
        let counters = self.counters.clone();
        self.inner.get_or_insert_with(key, &mut || {
            let built = build()?;
            if let Some((_, bytes)) = &built {
                counters.insertions.fetch_add(1, Ordering::SeqCst);
                counters
                    .uploaded_bytes
                    .fetch_add(bytes.len(), Ordering::SeqCst);
            }
            Ok(built)
        })
    }

    fn remove(&self, key: &AtlasKey) {
        self.inner.remove(key);
    }

    fn update(&self, key: AtlasKey, size: Size<DevicePixels>, bytes: &[u8]) -> Result<AtlasTile> {
        self.inner.update(key, size, bytes)
    }

    fn contains(&self, key: &AtlasKey) -> bool {
        self.inner.contains(key)
    }
}

/// The platform's headless GPU renderer behind an atlas that counts insertions.
struct CountingRenderer {
    inner: Box<dyn PlatformHeadlessRenderer>,
    atlas: Arc<CountingAtlas>,
}

impl PlatformHeadlessRenderer for CountingRenderer {
    fn render_scene_to_image(
        &mut self,
        scene: &Scene,
        size: Size<DevicePixels>,
    ) -> Result<RgbaImage> {
        self.inner.render_scene_to_image(scene, size)
    }

    fn render_scene(&mut self, scene: &Scene, size: Size<DevicePixels>) -> Result<()> {
        self.inner.render_scene(scene, size)
    }

    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.atlas.clone()
    }
}

/// A window on the real headless GPU renderer (scale factor 2), drawn and captured on demand.
pub struct HeadlessScene<V: 'static> {
    pub cx: HeadlessAppContext,
    pub window: WindowHandle<V>,
    pub counters: Arc<AtlasCounters>,
}

impl<V: Render + 'static> HeadlessScene<V> {
    /// Opens a window of `window_size` logical pixels with the view built by `build_root`.
    pub fn new(
        window_size: Size<Pixels>,
        build_root: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
    ) -> Self {
        let counters = Arc::new(AtlasCounters::default());
        let factory_counters = counters.clone();
        let mut cx = HeadlessAppContext::with_platform(
            Arc::new(vespertine_wgpu::CosmicTextSystem::new("Noto Sans")),
            Arc::new(()),
            move || {
                let Some(inner) = vespertine_platform::current_headless_renderer()? else {
                    return Ok(None);
                };
                #[allow(clippy::arc_with_non_send_sync)]
                let atlas = Arc::new(CountingAtlas {
                    inner: inner.sprite_atlas(),
                    counters: factory_counters.clone(),
                });
                Ok(Some(Box::new(CountingRenderer { inner, atlas })
                    as Box<dyn PlatformHeadlessRenderer>))
            },
        );
        let window = cx
            .open_window(window_size, build_root)
            .expect("window opens");
        Self {
            cx,
            window,
            counters,
        }
    }

    /// Draws the window once without capturing it.
    pub fn draw(&mut self) {
        self.cx.run_until_parked();
        self.cx
            .update_window(self.window.into(), |_, window, cx| {
                window.draw(cx).clear(cx)
            })
            .expect("window draws");
    }

    /// Draws the window and returns what the GPU rendered, at scale factor 2.
    pub fn capture(&mut self) -> Capture {
        self.draw();
        let window: AnyWindowHandle = self.window.into();
        Capture {
            image: self.cx.capture_screenshot(window).expect("screenshot"),
            scale: 2.0,
        }
    }

    /// Runs `f` against the window's painted state.
    pub fn with_window<R>(&mut self, f: impl FnOnce(&mut Window, &mut App) -> R) -> R {
        self.cx
            .update_window(self.window.into(), |_, window, cx| f(window, cx))
            .expect("window update")
    }
}

/// A captured frame addressed in logical pixels.
pub struct Capture {
    pub image: RgbaImage,
    pub scale: f32,
}

impl Capture {
    /// The pixel containing the logical point.
    pub fn at(&self, x: f32, y: f32) -> [u8; 3] {
        let pixel = self
            .image
            .get_pixel((x * self.scale) as u32, (y * self.scale) as u32);
        [pixel[0], pixel[1], pixel[2]]
    }

    /// The pixel at device coordinates.
    pub fn device(&self, x: u32, y: u32) -> [u8; 3] {
        let pixel = self.image.get_pixel(x, y);
        [pixel[0], pixel[1], pixel[2]]
    }
}

pub fn rgb_bytes(color: u32) -> [u8; 3] {
    [(color >> 16) as u8, (color >> 8) as u8, color as u8]
}

/// Whether every channel of `actual` is within `tolerance` of the sRGB hex `expected`.
pub fn near(actual: [u8; 3], expected: u32, tolerance: u8) -> bool {
    actual
        .into_iter()
        .zip(rgb_bytes(expected))
        .all(|(a, e)| a.abs_diff(e) <= tolerance)
}

/// A procedurally generated opaque image: `color(x, y)` is the sRGB hex of each pixel.
pub fn generated_image(
    width: u32,
    height: u32,
    color: impl Fn(u32, u32) -> u32,
) -> Arc<RenderImage> {
    let mut bytes = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let [r, g, b] = rgb_bytes(color(x, y));
            bytes.extend_from_slice(&[b, g, r, 255]);
        }
    }
    Arc::new(RenderImage::from_bgra(width, height, bytes).expect("valid generated image"))
}

pub fn logical_bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
    Bounds {
        origin: point(px(x), px(y)),
        size: size(px(width), px(height)),
    }
}

/// A view that fills the window with `background` and runs `paint` in the paint phase of a
/// full-window canvas.
pub struct PaintView {
    background: u32,
    opacity: f32,
    paint: Rc<dyn Fn(&mut Window)>,
}

impl PaintView {
    pub fn scene(
        window_size: Size<Pixels>,
        background: u32,
        opacity: f32,
        paint: impl Fn(&mut Window) + 'static,
    ) -> HeadlessScene<PaintView> {
        let paint: Rc<dyn Fn(&mut Window)> = Rc::new(paint);
        HeadlessScene::new(window_size, move |_, cx| {
            cx.new(|_| PaintView {
                background,
                opacity,
                paint,
            })
        })
    }
}

impl Render for PaintView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let paint = self.paint.clone();
        div().size_full().bg(rgb(self.background)).child(
            div()
                .size_full()
                .opacity(self.opacity)
                .child(canvas(|_, _, _| (), move |_, _, window, _| paint(window)).size_full()),
        )
    }
}

// ---- FID-RND-004 --------------------------------------------------------------------

pub const FID_004_1_BACKGROUND: u32 = 0x161B25;
pub const FID_004_2_BACKGROUND: u32 = 0x182838;
pub const FID_004_3_BACKGROUND: u32 = 0x181A1F;

/// FID-RND-004-1 source: 82 x 42 checkerboard of 4 x 4 cells, #70B8FF on even i + j and
/// #FFC878 on odd.
pub fn fid_004_1_image() -> Arc<RenderImage> {
    generated_image(82, 42, |x, y| {
        if (x / 4 + y / 4) % 2 == 0 {
            0x70B8FF
        } else {
            0xFFC878
        }
    })
}

/// FID-RND-004-1 mask: bounds (180,110) 520 x 230, radius 36, feather 24, clearance 8,
/// bottom fade at y = 440 with band height 36.
pub fn fid_004_1_mask() -> ImageAlphaMask {
    ImageAlphaMask {
        bounds: logical_bounds(180.0, 110.0, 520.0, 230.0),
        radius: px(36.0),
        feather: px(24.0),
        clearance: px(8.0),
        bottom_fade: Some((px(440.0), px(36.0))),
    }
}

/// Paints FID-RND-004-1 (window 900 x 500): the image at (40,40) 820 x 420 with radius 24 and
/// masked; the hosting element carries opacity [`FID_004_1_OPACITY`].
pub fn paint_fid_004_1(
    window: &mut Window,
    image: &Arc<RenderImage>,
    mask: Option<ImageAlphaMask>,
) {
    let area = logical_bounds(40.0, 40.0, 820.0, 420.0);
    window
        .paint_image_fitted_masked(
            area,
            area,
            Corners::all(px(24.0)),
            image.clone(),
            0,
            false,
            mask,
        )
        .expect("image paints");
}

/// Element opacity of FID-RND-004-1 (applied by the element hosting the painter).
pub const FID_004_1_OPACITY: f32 = 0.8;

/// FID-RND-004-2 source: 512 x 256, #E04040 left of x = 256 and #4080E0 right of it.
pub fn fid_004_2_image() -> Arc<RenderImage> {
    generated_image(512, 256, |x, _| if x < 256 { 0xE04040 } else { 0x4080E0 })
}

/// FID-RND-004-2 mask: bounds (240,150) 160 x 80, radius 20, clearance 8, feather 24.
pub fn fid_004_2_mask() -> ImageAlphaMask {
    ImageAlphaMask {
        bounds: logical_bounds(240.0, 150.0, 160.0, 80.0),
        radius: px(20.0),
        feather: px(24.0),
        clearance: px(8.0),
        bottom_fade: None,
    }
}

/// Paints FID-RND-004-2 (window 640 x 400): visible (120,60) 400 x 280 inside fitted
/// (40,60) 560 x 280, all corner radii 24.
pub fn paint_fid_004_2(
    window: &mut Window,
    image: &Arc<RenderImage>,
    mask: Option<ImageAlphaMask>,
) {
    window
        .paint_image_fitted_masked(
            logical_bounds(120.0, 60.0, 400.0, 280.0),
            logical_bounds(40.0, 60.0, 560.0, 280.0),
            Corners::all(px(24.0)),
            image.clone(),
            0,
            false,
            mask,
        )
        .expect("image paints");
}

/// FID-RND-004-3 source: 800 x 400, #8060A0 above y = 200 and #40A080 below.
pub fn fid_004_3_image() -> Arc<RenderImage> {
    generated_image(800, 400, |_, y| if y < 200 { 0x8060A0 } else { 0x40A080 })
}

/// FID-RND-004-3 mask: bounds (240,240) 320 x 200, radius 20, clearance 8, feather 120, bottom
/// fade at y = 440 with band height 400.
pub fn fid_004_3_mask() -> ImageAlphaMask {
    ImageAlphaMask {
        bounds: logical_bounds(240.0, 240.0, 320.0, 200.0),
        radius: px(20.0),
        feather: px(120.0),
        clearance: px(8.0),
        bottom_fade: Some((px(440.0), px(400.0))),
    }
}

/// Paints FID-RND-004-3 (window 800 x 480): the image cover-fitted into the viewport
/// (80,40) 640 x 400 at zoom 1 and focal (0.5, 0.5).
pub fn paint_fid_004_3(
    window: &mut Window,
    image: &Arc<RenderImage>,
    mask: Option<ImageAlphaMask>,
) {
    let viewport = logical_bounds(80.0, 40.0, 640.0, 400.0);
    let fitted =
        cover_fit(viewport, size(800.0, 400.0), 1.0, point(0.5, 0.5)).expect("valid cover fit");
    window
        .paint_image_fitted_masked(
            viewport,
            fitted,
            Corners::default(),
            image.clone(),
            0,
            false,
            mask,
        )
        .expect("image paints");
}

// ---- FID-RND-007 --------------------------------------------------------------------

/// FID-RND-007 source: 2 x 2 raw BGRA, red / green over blue / white.
pub fn fid_007_image() -> Arc<RenderImage> {
    let bgra = [
        [0x00, 0x00, 0xFF, 0xFF],
        [0x00, 0xFF, 0x00, 0xFF],
        [0xFF, 0x00, 0x00, 0xFF],
        [0xFF, 0xFF, 0xFF, 0xFF],
    ];
    Arc::new(RenderImage::from_bgra(2, 2, bgra.concat()).expect("valid 2 x 2 image"))
}

/// Paints FID-RND-007 (window 400 x 200): the nearest copy at (40,40) 120 x 120 and the linear
/// copy at (220,40) 120 x 120.
pub fn paint_fid_007(window: &mut Window, image: &Arc<RenderImage>) {
    let nearest = logical_bounds(40.0, 40.0, 120.0, 120.0);
    let linear = logical_bounds(220.0, 40.0, 120.0, 120.0);
    window
        .paint_image_nearest(
            nearest,
            nearest,
            Corners::default(),
            image.clone(),
            0,
            false,
        )
        .expect("nearest paints");
    window
        .paint_image(linear, linear, Corners::default(), image.clone(), 0, false)
        .expect("linear paints");
}

// ---- RND-004 A13: a resizable excluded surface over an image ---------------------------

/// Window size of [`ResizeScene`].
pub const RESIZE_WINDOW: (f32, f32) = (640.0, 380.0);
/// The image region of [`ResizeScene`]: left, top, width, height in logical pixels.
pub const RESIZE_IMAGE_AREA: (f32, f32, f32, f32) = (40.0, 40.0, 560.0, 300.0);

/// An image region with an input-like surface laid over its bottom. The surface is excluded
/// from the image by a mask whose bounds come from the surface's bounds measured in the same
/// prepaint pass as the image region and which extends to the image bottom. Changing
/// `surface_height` resizes the surface; the image is never decoded again.
pub struct ResizeScene {
    pub image: Arc<RenderImage>,
    pub surface_height: Rc<Cell<f32>>,
    measured: Rc<Cell<Option<Bounds<Pixels>>>>,
}

impl ResizeScene {
    pub fn new(image: Arc<RenderImage>, surface_height: Rc<Cell<f32>>) -> Self {
        Self {
            image,
            surface_height,
            measured: Rc::new(Cell::new(None)),
        }
    }
}

impl Render for ResizeScene {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // A measurement left over from the previous frame must never be used: clear it so a
        // stale read fails loudly instead of lagging by one frame.
        self.measured.set(None);
        let measured = self.measured.clone();
        let measured_at_paint = self.measured.clone();
        let image = self.image.clone();
        let (left, top, width, height) = RESIZE_IMAGE_AREA;
        div().size_full().bg(rgb(0x181A1F)).child(
            div()
                .absolute()
                .left(px(left))
                .top(px(top))
                .w(px(width))
                .h(px(height))
                .on_children_prepainted(move |bounds, _, _| measured.set(bounds.get(1).copied()))
                .child(
                    canvas(
                        |_, _, _| (),
                        move |region, _, window, _| {
                            let surface = measured_at_paint
                                .get()
                                .expect("the surface was measured in this frame's prepaint");
                            let mask = ImageAlphaMask {
                                bounds: Bounds {
                                    origin: surface.origin,
                                    size: size(
                                        surface.size.width,
                                        region.bottom() - surface.origin.y,
                                    ),
                                },
                                radius: px(12.0),
                                feather: px(24.0),
                                clearance: px(8.0),
                                bottom_fade: Some((region.bottom(), px(40.0))),
                            };
                            let fitted =
                                cover_fit(region, size(800.0, 400.0), 1.0, point(0.5, 0.5))
                                    .expect("valid cover fit");
                            window
                                .paint_image_fitted_masked(
                                    region,
                                    fitted,
                                    Corners::all(px(16.0)),
                                    image,
                                    0,
                                    false,
                                    Some(mask),
                                )
                                .expect("image paints");
                        },
                    )
                    .size_full(),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(100.0))
                        .right(px(100.0))
                        .bottom(px(20.0))
                        .h(px(self.surface_height.get()))
                        .rounded(px(12.0))
                        .bg(rgb(0x2A2E38)),
                ),
        )
    }
}

/// The source image of [`ResizeScene`]: 800 x 400, a diagonal two-tone pattern.
pub fn resize_scene_image() -> Arc<RenderImage> {
    generated_image(800, 400, |x, y| {
        if ((x / 50) + (y / 50)) % 2 == 0 {
            0x8060A0
        } else {
            0x40A080
        }
    })
}

/// Surface bounds, in device pixels, of the quad the surface of [`ResizeScene`] painted in the
/// most recently drawn frame: the quad with the surface's fill color.
pub fn painted_surface_quad(window: &Window) -> [f32; 4] {
    let target = vespertine::Hsla::from(rgb(0x2A2E38));
    let quad = window
        .painted_quads()
        .into_iter()
        .find(|quad| quad.background.as_solid() == Some(target))
        .expect("the surface quad was painted");
    [
        quad.bounds.origin.x.0,
        quad.bounds.origin.y.0,
        quad.bounds.size.width.0,
        quad.bounds.size.height.0,
    ]
}

// ---- FID-RND-007 checks -----------------------------------------------------------------

/// Asserts the FID-RND-007 behavior of a capture of [`paint_fid_007`]: four flat nearest blocks
/// with hard edges at the 60-logical-pixel boundaries, and a smooth linear copy.
pub fn check_fid_007(capture: &Capture) {
    let expected = |x: u32, y: u32| match (x >= 200, y >= 200) {
        (false, false) => 0xFF0000,
        (true, false) => 0x00FF00,
        (false, true) => 0x0000FF,
        (true, true) => 0xFFFFFF,
    };
    // Block interiors, one device pixel away from the outer boundary: exactly the texel color.
    for y in 81..319 {
        for x in 81..319 {
            assert!(
                near(capture.device(x, y), expected(x, y), 1),
                "nearest copy at device ({x}, {y}) is {:?}, not the flat block color",
                capture.device(x, y)
            );
        }
    }
    // The block edges are hard: adjacent device pixels carry the two block colors.
    assert!(near(capture.device(199, 120), 0xFF0000, 1));
    assert!(near(capture.device(200, 120), 0x00FF00, 1));
    assert!(near(capture.device(120, 199), 0xFF0000, 1));
    assert!(near(capture.device(120, 200), 0x0000FF, 1));
    // The linear copy at device (440, 80): linear blends and the average color in the center.
    // Its outer half-texel ring is the unchanged shared-atlas sampler's (no clamping at the
    // tile edge, RND-007); the landmarks below are those measured for that sampler over the
    // white window background, on a freshly started window where the image is the first atlas
    // tile, each within 8 per channel.
    let landmark = |x: u32, y: u32, expected: [u8; 3]| {
        let actual = capture.device(x, y);
        assert!(
            actual
                .into_iter()
                .zip(expected)
                .all(|(a, e)| a.abs_diff(e) <= 8),
            "linear copy at device ({x}, {y}) is {actual:?}, expected {expected:?}"
        );
    };
    for y in [80, 100, 139] {
        landmark(500, y, [254, 1, 0]);
        landmark(560, y, [127, 128, 0]);
        landmark(620, y, [1, 254, 1]);
        landmark(440, y, [191, 127, 127]);
        landmark(679, y, [127, 191, 127]);
    }
    landmark(500, 319, [128, 128, 191]);
    landmark(560, 319, [160, 160, 191]);
    landmark(620, 319, [191, 191, 191]);
    landmark(440, 200, [159, 127, 160]);
    let center = capture.device(560, 200);
    assert!(
        near(center, 0x808080, 3),
        "center of the linear copy is {center:?}"
    );
    let mut previous = 0u8;
    let mut intermediate = 0;
    for x in 500..620 {
        let [r, g, b] = capture.device(x, 100);
        assert!(g >= previous, "green decreases at device x = {x}");
        previous = g;
        assert!(
            r.abs_diff(255 - g) <= 4 && b <= 3,
            "device x = {x}: ({r}, {g}, {b})"
        );
        if (9..247).contains(&g) {
            intermediate += 1;
        }
    }
    assert!(
        intermediate > 100,
        "the linear copy has only {intermediate} blended columns"
    );
}
