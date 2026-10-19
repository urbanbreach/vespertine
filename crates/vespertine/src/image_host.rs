//! Retained state for the declarative media hosts `img` and `svg` (ELM-004).
//!
//! The adapter registry of ELM-007 creates one [`ImageHostState`] / [`SvgHostState`] per live
//! host identity, forwards the host properties (`src`, `objectFit`, `alt`; `src`, `source`)
//! to the setters, the ref methods (`setImagePixels`, `setImage`) to the live-update methods
//! and calls `render` every frame with the host-derived element id and the host style.

use crate::{
    AnyElement, App, ElementId, Hsla, Image, ImageCacheError, ImageFormat, ImageSource, ImageSrc,
    ImageUpdateError, Img, InteractiveElement, IntoElement, ObjectFit, ParentElement, PixelFormat,
    RenderImage, Resource, SharedString, Stateful, StatefulInteractiveElement, StyleRefinement,
    Styled, StyledImage, Svg, Window, classify_image_src, decode_image_bytes, div, img,
    parse_data_url, render_image_from_pixels, rgb, svg,
};
use std::{path::Path, sync::Arc};

/// Placeholder label for an empty source.
pub const IMAGE_NO_SRC_LABEL: &str = "img: no src";
/// Placeholder label for an invalid source or a failed load.
pub const IMAGE_LOAD_FAILED_LABEL: &str = "img: load failed";
/// Placeholder fill.
pub const IMAGE_PLACEHOLDER_FILL: u32 = 0x1F2230;
/// Placeholder border color (1 px).
pub const IMAGE_PLACEHOLDER_BORDER: u32 = 0x5D6481;
/// Placeholder text color.
pub const IMAGE_PLACEHOLDER_TEXT: u32 = 0xA4ACCD;
/// Default tint of the monochrome vector host.
pub const SVG_HOST_DEFAULT_TINT: u32 = 0xE2E2E2;

/// Parses the host property `objectFit`: exactly `fill`, `cover`, `scaleDown` or `none`;
/// anything else, including absent, is `contain`.
pub fn parse_object_fit(value: Option<&str>) -> ObjectFit {
    match value {
        Some("fill") => ObjectFit::Fill,
        Some("cover") => ObjectFit::Cover,
        Some("scaleDown") => ObjectFit::ScaleDown,
        Some("none") => ObjectFit::None,
        _ => ObjectFit::Contain,
    }
}

enum ResolvedSrc {
    /// No source: shows [`IMAGE_NO_SRC_LABEL`].
    Empty,
    /// The source cannot be used: shows [`IMAGE_LOAD_FAILED_LABEL`].
    Invalid,
    /// A loadable source.
    Source(ImageSource),
}

fn resolve_src(src: &str) -> ResolvedSrc {
    match classify_image_src(src) {
        ImageSrc::Empty => ResolvedSrc::Empty,
        ImageSrc::DataUrl(text) => match parse_data_url(text) {
            Ok(data) => ResolvedSrc::Source(ImageSource::Image(Arc::new(Image::from_bytes(
                data.format,
                data.bytes,
            )))),
            Err(_) => ResolvedSrc::Invalid,
        },
        ImageSrc::Network(uri) => {
            ResolvedSrc::Source(ImageSource::Resource(Resource::Uri(uri.to_string().into())))
        }
        ImageSrc::Path(path) => ResolvedSrc::Source(ImageSource::Resource(Resource::Path(
            Arc::from(Path::new(path)),
        ))),
    }
}

/// Retained state of one raster image host.
pub struct ImageHostState {
    src: Option<SharedString>,
    resolved: ResolvedSrc,
    object_fit: ObjectFit,
    alt: Option<SharedString>,
    live: Option<Arc<RenderImage>>,
    pending_release: Vec<Arc<RenderImage>>,
}

impl Default for ImageHostState {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageHostState {
    /// A host with no source.
    pub fn new() -> Self {
        Self {
            src: None,
            resolved: ResolvedSrc::Empty,
            object_fit: ObjectFit::Contain,
            alt: None,
            live: None,
            pending_release: Vec::new(),
        }
    }

    /// Applies the `src` property. `None` is the reset value (null): while a live image is
    /// shown it is ignored; otherwise it clears the source. A non-null value replaces the
    /// live image (scheduling it for release) and becomes the declarative source. Returns the
    /// replaced live image, if any.
    pub fn set_src(&mut self, src: Option<&str>) -> Option<Arc<RenderImage>> {
        match src {
            None if self.live.is_some() => None,
            None => {
                self.src = None;
                self.resolved = ResolvedSrc::Empty;
                None
            }
            Some(text) => {
                if self.src.as_deref() != Some(text) {
                    self.src = Some(SharedString::from(text.to_string()));
                    self.resolved = resolve_src(text);
                }
                self.take_live_for_release()
            }
        }
    }

    /// Applies the `objectFit` property.
    pub fn set_object_fit(&mut self, value: Option<&str>) {
        self.object_fit = parse_object_fit(value);
    }

    /// Applies the `alt` property (the accessible label).
    pub fn set_alt(&mut self, value: Option<&str>) {
        self.alt = value.map(|alt| SharedString::from(alt.to_string()));
    }

    /// The label of the placeholder this host shows when it has no image: [`IMAGE_NO_SRC_LABEL`]
    /// for an empty source, [`IMAGE_LOAD_FAILED_LABEL`] for an invalid source or a failed load.
    pub fn placeholder_label(&self) -> &'static str {
        match (&self.live, &self.resolved) {
            (None, ResolvedSrc::Empty) => IMAGE_NO_SRC_LABEL,
            _ => IMAGE_LOAD_FAILED_LABEL,
        }
    }

    /// The object fit currently in effect.
    pub fn object_fit(&self) -> ObjectFit {
        self.object_fit
    }

    /// Ref method `setImagePixels(width, height, bytes, format)`: installs packed
    /// straight-alpha pixels as the live image. Returns the previous live image.
    pub fn set_image_pixels(
        &mut self,
        width: f64,
        height: f64,
        bytes: &[u8],
        format: Option<&str>,
    ) -> Result<Option<Arc<RenderImage>>, ImageUpdateError> {
        let format = PixelFormat::parse(format)?;
        let image = render_image_from_pixels(width, height, bytes, format)?;
        Ok(self.replace_live_image(Arc::new(image)))
    }

    /// Ref method `setImage(encodedBytes)`: decodes any supported format (sniffed by magic
    /// bytes) and installs it as the live image. Returns the previous live image. The decode
    /// runs on the calling thread.
    pub fn set_image(
        &mut self,
        bytes: &[u8],
        cx: &App,
    ) -> Result<Option<Arc<RenderImage>>, ImageUpdateError> {
        let image = decode_image_bytes(bytes, &cx.svg_renderer())
            .map_err(|error| ImageUpdateError::Decode(live_decode_message(&error)))?;
        Ok(self.replace_live_image(image))
    }

    /// Installs `image` as the live image and returns the previous one. The previous image is
    /// also scheduled for release: the window's cached copy is dropped on the next render.
    pub fn replace_live_image(&mut self, image: Arc<RenderImage>) -> Option<Arc<RenderImage>> {
        let previous = self.live.replace(image);
        if let Some(previous) = &previous {
            self.pending_release.push(previous.clone());
        }
        previous
    }

    fn take_live_for_release(&mut self) -> Option<Arc<RenderImage>> {
        let previous = self.live.take();
        if let Some(previous) = &previous {
            self.pending_release.push(previous.clone());
        }
        previous
    }

    /// The live image currently shown, if any.
    pub fn live_image(&self) -> Option<&Arc<RenderImage>> {
        self.live.as_ref()
    }

    /// Replaced images that still await release from the window's cache.
    pub fn pending_release(&self) -> &[Arc<RenderImage>] {
        &self.pending_release
    }

    /// Drops the window's cached copies of replaced images.
    pub fn release_pending(&mut self, window: &mut Window, cx: &mut App) {
        for image in self.pending_release.drain(..) {
            cx.drop_image(image, Some(window));
        }
    }

    /// Destroys the host's native image resources: the live image and every image replaced
    /// but not yet released are dropped from the window caches.
    pub fn release(&mut self, window: &mut Window, cx: &mut App) {
        self.release_pending(window, cx);
        if let Some(live) = self.live.take() {
            cx.drop_image(live, Some(window));
        }
    }

    /// Builds the native element for this frame.
    ///
    /// `id` must be the host-derived stable element id: it keys animation progress and
    /// delayed-loading state across re-renders. `style` is the host's style description; a
    /// definite width and height stay in force after the image has loaded. The element has
    /// image accessibility semantics (role image, `alt` as the label). The adapter wires the
    /// standard events and bounds tracking on the returned element.
    pub fn element(
        &mut self,
        id: impl Into<ElementId>,
        style: StyleRefinement,
        window: &mut Window,
        cx: &mut App,
    ) -> Stateful<Img> {
        self.release_pending(window, cx);
        let id = id.into();
        let label = self.placeholder_label();
        let source = if let Some(live) = &self.live {
            ImageSource::Render(live.clone())
        } else {
            match &self.resolved {
                ResolvedSrc::Source(source) => source.clone(),
                ResolvedSrc::Empty => failing_source("no source"),
                ResolvedSrc::Invalid => failing_source("invalid source"),
            }
        };
        let alt = self.alt.clone();
        let placeholder_alt = alt.clone();
        let placeholder_id = ElementId::from((id.clone(), "placeholder"));
        let mut element = img(source)
            .object_fit(self.object_fit)
            .with_fallback(move || {
                image_placeholder(placeholder_id.clone(), label, placeholder_alt.clone())
            })
            .id(id)
            .role(crate::Role::Image);
        if let Some(alt) = alt {
            element = element.aria_label(alt);
        }
        *element.style() = style;
        element
    }

    /// [`Self::element`] as an [`AnyElement`].
    pub fn render(
        &mut self,
        id: impl Into<ElementId>,
        style: StyleRefinement,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        self.element(id, style, window, cx).into_any_element()
    }
}

fn failing_source(message: &'static str) -> ImageSource {
    ImageSource::Custom(Arc::new(move |_, _| {
        Some(Err(ImageCacheError::Asset(message.into())))
    }))
}

fn live_decode_message(error: &ImageCacheError) -> String {
    match error {
        ImageCacheError::Other(error) => error.to_string(),
        other => other.to_string(),
    }
}

/// The diagnostic placeholder of the raster host: a centered label in a 1 px bordered box
/// with image accessibility semantics (role image, `alt` as the accessible label).
pub fn image_placeholder(
    id: impl Into<ElementId>,
    label: &'static str,
    alt: Option<SharedString>,
) -> AnyElement {
    let mut placeholder = div()
        .id(id)
        .role(crate::Role::Image)
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgb(IMAGE_PLACEHOLDER_FILL))
        .border_1()
        .border_color(rgb(IMAGE_PLACEHOLDER_BORDER))
        .text_color(rgb(IMAGE_PLACEHOLDER_TEXT));
    if let Some(alt) = alt {
        placeholder = placeholder.aria_label(alt);
    }
    placeholder.child(label).into_any_element()
}

/// Retained state of one monochrome vector host (type `svg`).
///
/// The host renders only the SVG's coverage, filled with the style color
/// (default [`SVG_HOST_DEFAULT_TINT`]). A missing, unreadable or invalid source renders an
/// empty element with the styled bounds.
#[derive(Default)]
pub struct SvgHostState {
    src: Option<SharedString>,
    source: Option<SharedString>,
    loaded: Option<Arc<[u8]>>,
    loaded_for: Option<(Option<SharedString>, Option<SharedString>)>,
}

impl SvgHostState {
    /// A host with no source.
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies the `src` property (a local path or an SVG data URL); `None` resets it.
    pub fn set_src(&mut self, src: Option<&str>) {
        self.src = src.map(|text| SharedString::from(text.to_string()));
    }

    /// Applies the `source` property (inline markup); a nonempty source after trimming takes
    /// precedence over `src`. `None` resets it.
    pub fn set_source(&mut self, source: Option<&str>) {
        self.source = source.map(|text| SharedString::from(text.to_string()));
    }

    /// The SVG bytes in effect, reading and validating the source when a property changed.
    pub fn bytes(&mut self, cx: &App) -> Option<Arc<[u8]>> {
        let key = (self.src.clone(), self.source.clone());
        if self.loaded_for.as_ref() != Some(&key) {
            self.loaded = self.load(cx);
            self.loaded_for = Some(key);
        }
        self.loaded.clone()
    }

    fn load(&self, cx: &App) -> Option<Arc<[u8]>> {
        let bytes: Vec<u8> = if let Some(markup) = self
            .source
            .as_deref()
            .filter(|text| !text.trim().is_empty())
        {
            markup.as_bytes().to_vec()
        } else {
            match classify_image_src(self.src.as_deref()?) {
                ImageSrc::Empty => return None,
                ImageSrc::DataUrl(text) => {
                    let data = parse_data_url(text).ok()?;
                    if data.format != ImageFormat::Svg {
                        return None;
                    }
                    data.bytes
                }
                ImageSrc::Network(_) => return None,
                ImageSrc::Path(path) => std::fs::read(path).ok()?,
            }
        };
        cx.svg_renderer().parse_svg(&bytes).ok()?;
        Some(Arc::from(bytes))
    }

    /// Builds the native element for this frame: the monochrome vector element, filled with
    /// the style color (default [`SVG_HOST_DEFAULT_TINT`]), taking its styled size without
    /// shrinking or growing by flex. Without a usable source it paints nothing and keeps its
    /// styled bounds. The adapter wires the standard events and bounds tracking on the
    /// returned element.
    pub fn element(
        &mut self,
        id: impl Into<ElementId>,
        style: StyleRefinement,
        cx: &App,
    ) -> Stateful<Svg> {
        let mut element = match self.bytes(cx) {
            Some(bytes) => svg().data(&bytes),
            None => svg(),
        }
        .flex_none()
        .id(id);
        *element.style() = style;
        if element.style().text.color.is_none() {
            element.style().text.color = Some(Hsla::from(rgb(SVG_HOST_DEFAULT_TINT)));
        }
        element
    }

    /// [`Self::element`] as an [`AnyElement`].
    pub fn render(
        &mut self,
        id: impl Into<ElementId>,
        style: StyleRefinement,
        cx: &App,
    ) -> AnyElement {
        self.element(id, style, cx).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AppContext as _, Bounds, Context, Hsla, ImgState, Pixels, Render, ScaledPixels,
        TestAppContext, VisualTestContext, canvas, point, px, size,
    };
    use image::{ImageBuffer, Rgba};
    use std::{
        cell::{Cell, RefCell},
        io::Cursor,
        rc::Rc,
    };
    use vespertine_http_client::{AsyncBody, FakeHttpClient, Response};

    type Host = Rc<RefCell<ImageHostState>>;

    fn new_host() -> Host {
        Rc::new(RefCell::new(ImageHostState::new()))
    }

    fn box_style(width: f32, height: f32) -> StyleRefinement {
        StyleRefinement::default().w(px(width)).h(px(height))
    }

    /// The root of a test frame: asset loading needs a current view.
    struct HostView {
        host: Host,
        style: StyleRefinement,
        probe: Option<Rc<Probe>>,
    }

    /// Reads (and optionally overwrites) the animation frame index kept for the host image.
    struct Probe {
        seed: Cell<Option<usize>>,
        observed: Cell<usize>,
    }

    impl Render for HostView {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let element = self
                .host
                .borrow_mut()
                .render("host", self.style.clone(), window, cx);
            let mut root = div().child(element);
            if let Some(probe) = self.probe.clone() {
                root = root.child(canvas(
                    |_, _, _| (),
                    move |_, _, window, _| {
                        window.with_global_id("host".into(), |id, window| {
                            window.with_element_state::<ImgState, _>(id, |state, _| {
                                let mut state = state.expect("host image state exists");
                                probe.observed.set(state.frame_index);
                                if let Some(frame) = probe.seed.take() {
                                    state.frame_index = frame;
                                }
                                ((), state)
                            })
                        });
                    },
                ));
            }
            root
        }
    }

    /// Starts a fresh scene: the test window keeps the primitives of earlier draws otherwise.
    fn clear_scene(window: &mut VisualTestContext) {
        window.update(|window, _| {
            window.rendered_frame.scene.clear();
            window.next_frame.scene.clear();
        });
    }

    fn draw(window: &mut VisualTestContext, host: &Host, style: StyleRefinement) {
        clear_scene(window);
        let host = host.clone();
        window.draw(
            point(px(0.), px(0.)),
            size(px(300.), px(300.)),
            move |_, cx| {
                cx.new(|_| HostView {
                    host,
                    style,
                    probe: None,
                })
                .into_any_element()
            },
        );
    }

    #[derive(Clone)]
    struct Sprite {
        bounds: Bounds<Pixels>,
        tile_size: (i32, i32),
        radius: ScaledPixels,
        opacity: f32,
    }

    struct Painted {
        sprites: Vec<Sprite>,
        /// Quads filled with the placeholder fill.
        placeholder_quads: usize,
        /// Quads with the placeholder border color and a visible border.
        placeholder_borders: usize,
        scale: f32,
    }

    fn painted(window: &mut VisualTestContext) -> Painted {
        window.update(|window, _| {
            let scale = window.scale_factor();
            let scene = &window.rendered_frame.scene;
            let fill = Hsla::from(rgb(IMAGE_PLACEHOLDER_FILL));
            let border = Hsla::from(rgb(IMAGE_PLACEHOLDER_BORDER));
            Painted {
                sprites: scene
                    .polychrome_sprites
                    .iter()
                    .map(|sprite| Sprite {
                        bounds: Bounds {
                            origin: point(
                                px(sprite.bounds.origin.x.0 / scale),
                                px(sprite.bounds.origin.y.0 / scale),
                            ),
                            size: size(
                                px(sprite.bounds.size.width.0 / scale),
                                px(sprite.bounds.size.height.0 / scale),
                            ),
                        },
                        tile_size: (
                            sprite.tile.bounds.size.width.0,
                            sprite.tile.bounds.size.height.0,
                        ),
                        radius: sprite.corner_radii.top_left,
                        opacity: sprite.opacity,
                    })
                    .collect(),
                placeholder_quads: scene
                    .quads
                    .iter()
                    .filter(|quad| quad.background.as_solid() == Some(fill))
                    .count(),
                placeholder_borders: scene
                    .quads
                    .iter()
                    .filter(|quad| quad.border_color == border && quad.border_widths.top.0 > 0.)
                    .count(),
                scale,
            }
        })
    }

    fn encode(format: image::ImageFormat, width: u32, height: u32) -> Vec<u8> {
        let buffer = ImageBuffer::from_fn(width, height, |x, _| {
            if x < width / 2 {
                Rgba([255, 0, 0, 255])
            } else {
                Rgba([0, 0, 255, 255])
            }
        });
        let image = image::DynamicImage::ImageRgba8(buffer);
        let image = if format == image::ImageFormat::Jpeg {
            image::DynamicImage::ImageRgb8(image.to_rgb8())
        } else {
            image
        };
        let mut out = Cursor::new(Vec::new());
        image.write_to(&mut out, format).expect("encode");
        out.into_inner()
    }

    fn base64_encode(bytes: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let value = (chunk[0] as u32) << 16
                | (*chunk.get(1).unwrap_or(&0) as u32) << 8
                | *chunk.get(2).unwrap_or(&0) as u32;
            for index in 0..4 {
                if index <= chunk.len() {
                    out.push(ALPHABET[(value >> (18 - 6 * index) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    fn pixels(width: u32, height: u32) -> Vec<u8> {
        vec![200; (width * height * 4) as usize]
    }

    fn settle(window: &mut VisualTestContext, host: &Host, style: &StyleRefinement) {
        draw(window, host, style.clone());
        window.run_until_parked();
        draw(window, host, style.clone());
    }

    #[test]
    fn object_fit_strings() {
        assert_eq!(parse_object_fit(None), ObjectFit::Contain);
        assert_eq!(parse_object_fit(Some("fill")), ObjectFit::Fill);
        assert_eq!(parse_object_fit(Some("cover")), ObjectFit::Cover);
        assert_eq!(parse_object_fit(Some("scaleDown")), ObjectFit::ScaleDown);
        assert_eq!(parse_object_fit(Some("none")), ObjectFit::None);
        for other in ["contain", "Fill", "scale-down", "", "stretch"] {
            assert_eq!(parse_object_fit(Some(other)), ObjectFit::Contain, "{other}");
        }
    }

    #[vespertine::test]
    fn empty_and_failed_sources_show_the_exact_placeholders(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        let style = box_style(100., 60.);
        let host = new_host();

        // Empty source: "img: no src".
        assert_eq!(host.borrow().placeholder_label(), "img: no src");
        assert_eq!(IMAGE_NO_SRC_LABEL, "img: no src");
        draw(window, &host, style.clone());
        let painted_empty = painted(window);
        assert_eq!(painted_empty.placeholder_quads, 1);
        assert!(painted_empty.placeholder_borders >= 1);
        assert!(painted_empty.sprites.is_empty());
        host.borrow_mut().set_src(Some("   "));
        assert_eq!(host.borrow().placeholder_label(), "img: no src");

        // Invalid data URL, unsupported MIME and bad base64: "img: load failed".
        for src in [
            "data:image/png",
            "data:text/plain,hello",
            "data:image/png;base64,@@@@",
        ] {
            host.borrow_mut().set_src(Some(src));
            assert_eq!(
                host.borrow().placeholder_label(),
                "img: load failed",
                "{src}"
            );
            assert_eq!(IMAGE_LOAD_FAILED_LABEL, "img: load failed");
            draw(window, &host, style.clone());
            let invalid = painted(window);
            assert_eq!(invalid.placeholder_quads, 1, "{src}");
            assert!(invalid.sprites.is_empty(), "{src}");
        }

        // A missing file fails asynchronously and shows the same placeholder.
        host.borrow_mut()
            .set_src(Some("/nonexistent-dir/missing.png"));
        settle(window, &host, &style);
        assert_eq!(painted(window).placeholder_quads, 1);
        assert_eq!(host.borrow().placeholder_label(), "img: load failed");

        // Bytes that are not an image fail the same way (decoded by content).
        let dir = temp_dir("not-an-image");
        let path = dir.path().join("fake.png");
        std::fs::write(&path, b"this is not a png").unwrap();
        host.borrow_mut().set_src(Some(path.to_str().unwrap()));
        settle(window, &host, &style);
        let failed = painted(window);
        assert_eq!(failed.placeholder_quads, 1);
        assert!(failed.sprites.is_empty());
    }

    /// A directory removed when dropped.
    fn temp_dir(label: &str) -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix(&format!("vespertine-image-host-{label}-"))
            .tempdir()
            .unwrap()
    }

    #[vespertine::test]
    fn placeholder_keeps_the_styled_box_and_has_image_semantics(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        let host = new_host();
        host.borrow_mut().set_alt(Some("a cat"));
        draw(window, &host, box_style(120., 80.));
        let (fill_bounds, border_widths, scale) = window.update(|window, _| {
            let scene = &window.rendered_frame.scene;
            let fill = scene
                .quads
                .iter()
                .find(|quad| {
                    quad.background.as_solid() == Some(Hsla::from(rgb(IMAGE_PLACEHOLDER_FILL)))
                })
                .expect("placeholder fill quad");
            let border = scene
                .quads
                .iter()
                .find(|quad| quad.border_color == Hsla::from(rgb(IMAGE_PLACEHOLDER_BORDER)))
                .expect("placeholder border quad");
            (fill.bounds, border.border_widths, window.scale_factor())
        });
        // The box keeps the styled size; the border is 1 px wide on every side.
        assert_eq!(fill_bounds.size.width.0, 120. * scale);
        assert_eq!(fill_bounds.size.height.0, 80. * scale);
        for width in [
            border_widths.top,
            border_widths.right,
            border_widths.bottom,
            border_widths.left,
        ] {
            assert_eq!(width.0, scale, "1 px border");
        }
    }

    #[vespertine::test]
    fn data_url_source_keeps_the_explicit_box_after_load(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        let png = encode(image::ImageFormat::Png, 2, 1);
        let host = new_host();
        host.borrow_mut().set_src(Some(&format!(
            "DATA:IMAGE/PNG;Base64,{}",
            base64_encode(&png)
        )));
        let style = box_style(80., 80.);

        // Before the load completes the box already has its styled size.
        let wrapper = |window: &mut VisualTestContext, host: &Host| {
            let host = host.clone();
            let style = style.clone();
            window.draw(
                point(px(0.), px(0.)),
                size(px(300.), px(300.)),
                move |_, cx| {
                    let view = cx.new(|_| HostView {
                        host,
                        style,
                        probe: None,
                    });
                    crate::div()
                        .debug_selector(|| "wrapper".into())
                        .flex()
                        .child(view.into_any_element())
                },
            );
            window.debug_bounds("wrapper").expect("wrapper bounds")
        };
        assert_eq!(wrapper(window, &host).size, size(px(80.), px(80.)));
        window.run_until_parked();
        assert_eq!(wrapper(window, &host).size, size(px(80.), px(80.)));

        // After the load the 2:1 image is contained inside the unchanged 80 x 80 box.
        let loaded = painted(window);
        let sprite = loaded.sprites.last().expect("image painted");
        assert_eq!(
            sprite.bounds,
            Bounds {
                origin: point(px(0.), px(20.)),
                size: size(px(80.), px(40.)),
            }
        );
        assert_eq!(loaded.placeholder_quads, 0);
    }

    #[vespertine::test]
    fn disk_images_decode_by_content_not_extension(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        let dir = temp_dir("misleading");
        // A JPEG named .png, a BMP named .gif and a GIF without extension.
        let cases = [
            ("photo.png", encode(image::ImageFormat::Jpeg, 8, 4), 8),
            ("bitmap.gif", encode(image::ImageFormat::Bmp, 6, 4), 6),
            ("noext", encode(image::ImageFormat::Gif, 4, 4), 4),
        ];
        for (name, bytes, width) in cases {
            let path = dir.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            let host = new_host();
            host.borrow_mut().set_src(Some(path.to_str().unwrap()));
            host.borrow_mut().set_object_fit(Some("fill"));
            settle(window, &host, &box_style(50., 50.));
            let result = painted(window);
            let sprite = result
                .sprites
                .last()
                .unwrap_or_else(|| panic!("{name} painted"));
            assert_eq!(sprite.tile_size.0, width, "{name}");
            assert_eq!(result.placeholder_quads, 0, "{name}");
        }
    }

    #[vespertine::test]
    fn every_object_fit_paints_inside_the_box(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        let host = new_host();
        host.borrow_mut()
            .set_image_pixels(200., 100., &pixels(200, 100), None)
            .unwrap();
        let style = box_style(100., 100.);
        let check = |window: &mut VisualTestContext,
                     fit: Option<&str>,
                     expected: Bounds<Pixels>,
                     tile: (i32, i32)| {
            host.borrow_mut().set_object_fit(fit);
            draw(window, &host, style.clone());
            let result = painted(window);
            let sprite = result.sprites.last().expect("sprite").clone();
            assert_eq!(sprite.bounds, expected, "{fit:?}");
            assert_eq!(sprite.tile_size, tile, "{fit:?}");
        };
        let bounds = |x, y, w, h| Bounds {
            origin: point(px(x), px(y)),
            size: size(px(w), px(h)),
        };
        check(window, None, bounds(0., 25., 100., 50.), (200, 100));
        check(
            window,
            Some("contain"),
            bounds(0., 25., 100., 50.),
            (200, 100),
        );
        check(
            window,
            Some("anything-else"),
            bounds(0., 25., 100., 50.),
            (200, 100),
        );
        check(window, Some("fill"), bounds(0., 0., 100., 100.), (200, 100));
        // Cover crops the middle half of the columns into the box.
        check(
            window,
            Some("cover"),
            bounds(0., 0., 100., 100.),
            (100, 100),
        );
        // The image is larger than the box: scale down like contain.
        check(
            window,
            Some("scaleDown"),
            bounds(0., 25., 100., 50.),
            (200, 100),
        );
        // Original size at the box origin, cropped to the box.
        check(window, Some("none"), bounds(0., 0., 100., 100.), (100, 100));

        // A small image keeps its original size, centered, with scaleDown.
        host.borrow_mut()
            .set_image_pixels(20., 10., &pixels(20, 10), None)
            .unwrap();
        check(
            window,
            Some("scaleDown"),
            bounds(40., 45., 20., 10.),
            (20, 10),
        );
        check(window, Some("none"), bounds(0., 0., 20., 10.), (20, 10));
        check(
            window,
            Some("contain"),
            bounds(0., 25., 100., 50.),
            (20, 10),
        );
    }

    #[vespertine::test]
    fn style_radii_and_opacity_apply_to_the_image(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        let host = new_host();
        host.borrow_mut()
            .set_image_pixels(40., 40., &pixels(40, 40), None)
            .unwrap();
        let mut style = box_style(100., 100.).rounded(px(12.));
        style.opacity = Some(0.5);
        draw(window, &host, style);
        let result = painted(window);
        let sprite = result.sprites.last().expect("sprite");
        assert_eq!(sprite.radius, px(12.).scale(result.scale));
        assert!((sprite.opacity - 0.5).abs() < 1e-6);
    }

    #[vespertine::test]
    fn live_pixel_updates_validate_and_replace_in_place(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        let host = new_host();
        let mut state = host.borrow_mut();
        // Rejections do not disturb the (absent) live image.
        for (width, height, len) in [(0., 1., 0), (-1., 1., 4), (1.5, 1., 6), (f64::NAN, 1., 4)] {
            assert!(
                state
                    .set_image_pixels(width, height, &vec![0; len], None)
                    .is_err()
            );
        }
        assert!(state.set_image_pixels(1., 1., &[0; 3], None).is_err());
        assert!(
            state
                .set_image_pixels(1., 1., &[0; 4], Some("argb"))
                .is_err()
        );
        assert!(state.live_image().is_none());
        drop(state);
        let error = window
            .update(|_, cx| host.borrow_mut().set_image(b"garbage", cx))
            .unwrap_err();
        assert_eq!(error.to_string(), "unrecognized image format");
        let truncated = encode(image::ImageFormat::Png, 8, 8);
        let error = window
            .update(|_, cx| host.borrow_mut().set_image(&truncated[..20], cx))
            .unwrap_err();
        assert!(!error.to_string().is_empty());
        assert!(host.borrow().live_image().is_none(), "no partial image");

        // Installing images returns the previous one; the window drops it on the next render.
        let first = {
            let mut state = host.borrow_mut();
            assert!(
                state
                    .set_image_pixels(4., 4., &pixels(4, 4), None)
                    .unwrap()
                    .is_none()
            );
            state.live_image().unwrap().clone()
        };
        draw(window, &host, box_style(40., 40.));
        assert!(window.update(|window, _| window.has_image_atlas_entry(&first)));

        let previous = host
            .borrow_mut()
            .set_image_pixels(4., 4., &pixels(4, 4), Some("bgra"))
            .unwrap()
            .expect("previous live image");
        assert!(Arc::ptr_eq(&previous, &first));
        assert_eq!(host.borrow().pending_release().len(), 1);
        let second = host.borrow().live_image().unwrap().clone();
        draw(window, &host, box_style(40., 40.));
        assert!(host.borrow().pending_release().is_empty());
        window.update(|window, _| {
            assert!(
                !window.has_image_atlas_entry(&first),
                "replaced image released"
            );
            assert!(window.has_image_atlas_entry(&second));
        });

        // Removing the host releases the live image too.
        window.update(|window, cx| host.borrow_mut().release(window, cx));
        window.update(|window, _| assert!(!window.has_image_atlas_entry(&second)));
        assert!(host.borrow().live_image().is_none());
    }

    #[vespertine::test]
    fn clearing_src_keeps_a_live_image_but_a_new_src_replaces_it(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        let host = new_host();
        host.borrow_mut().set_src(Some("/nonexistent/declared.png"));
        host.borrow_mut()
            .set_image_pixels(4., 4., &pixels(4, 4), None)
            .unwrap();
        let live = host.borrow().live_image().unwrap().clone();
        draw(window, &host, box_style(40., 40.));
        assert!(window.update(|window, _| window.has_image_atlas_entry(&live)));

        // src reset to null while a live image is shown is ignored.
        assert!(host.borrow_mut().set_src(None).is_none());
        draw(window, &host, box_style(40., 40.));
        assert!(Arc::ptr_eq(host.borrow().live_image().unwrap(), &live));
        assert_eq!(painted(window).sprites.len(), 1);

        // A new non-null src replaces the live image and schedules it for release.
        let replaced = host
            .borrow_mut()
            .set_src(Some(""))
            .expect("live image replaced");
        assert!(Arc::ptr_eq(&replaced, &live));
        draw(window, &host, box_style(40., 40.));
        assert!(window.update(|window, _| !window.has_image_atlas_entry(&live)));
        assert_eq!(host.borrow().placeholder_label(), "img: no src");
        assert_eq!(painted(window).placeholder_quads, 1);

        // With no live image a null src clears the declared source.
        host.borrow_mut().set_src(Some("/nonexistent/declared.png"));
        host.borrow_mut().set_src(None);
        assert_eq!(host.borrow().placeholder_label(), "img: no src");
    }

    #[vespertine::test]
    fn animation_progress_survives_rerenders(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        let host = new_host();
        let mut out = Cursor::new(Vec::new());
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut out);
            for shade in [0u8, 255, 128] {
                // Ten minutes per frame: the animation never advances on its own in the test.
                let frame = image::Frame::from_parts(
                    ImageBuffer::from_pixel(4, 4, Rgba([shade, 0, 0, 255])),
                    0,
                    0,
                    image::Delay::from_numer_denom_ms(600_000, 1),
                );
                encoder.encode_frame(frame).unwrap();
            }
        }
        let gif = out.into_inner();
        window
            .update(|_, cx| host.borrow_mut().set_image(&gif, cx))
            .unwrap();
        assert_eq!(host.borrow().live_image().unwrap().frame_count(), 3);

        let probe = Rc::new(Probe {
            seed: Cell::new(Some(2)),
            observed: Cell::new(usize::MAX),
        });
        let view = window.update(|_, cx| {
            cx.new(|_| HostView {
                host: host.clone(),
                style: box_style(40., 40.),
                probe: Some(probe.clone()),
            })
        });
        let frame = |window: &mut VisualTestContext| {
            let view = view.clone();
            window.draw(
                point(px(0.), px(0.)),
                size(px(300.), px(300.)),
                move |_, _| view.into_any_element(),
            );
            probe.observed.get()
        };
        // The first frame starts at frame zero and the probe moves the animation to frame two;
        // every later re-render of the host (a fresh element each frame) continues from it.
        assert_eq!(frame(window), 0);
        assert_eq!(frame(window), 2);
        assert_eq!(frame(window), 2);
    }

    #[vespertine::test]
    fn network_sources_load_through_the_http_client(cx: &mut TestAppContext) {
        let png = encode(image::ImageFormat::Png, 4, 2);
        let requested = Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
        let log = requested.clone();
        let client = FakeHttpClient::create(move |request| {
            let png = png.clone();
            let log = log.clone();
            async move {
                log.lock().push(request.uri().to_string());
                Ok(Response::builder()
                    .status(200)
                    .body(AsyncBody::from(png))
                    .unwrap())
            }
        });
        cx.update(|cx| cx.set_http_client(client));
        let window = cx.add_empty_window();
        let host = new_host();
        host.borrow_mut()
            .set_src(Some(" HTTPS://img.example/cat.png "));
        host.borrow_mut().set_object_fit(Some("fill"));
        settle(window, &host, &box_style(40., 40.));
        let result = painted(window);
        assert_eq!(
            result
                .sprites
                .last()
                .expect("network image painted")
                .tile_size,
            (4, 2)
        );
        let requested: Vec<String> = requested
            .lock()
            .iter()
            .map(|uri| uri.to_ascii_lowercase())
            .collect();
        assert_eq!(requested, ["https://img.example/cat.png".to_string()]);
    }

    #[vespertine::test]
    fn failing_network_loaders_show_the_placeholder_without_panicking(cx: &mut TestAppContext) {
        let fail = FakeHttpClient::create(|_| async move { anyhow::bail!("network down") });
        cx.update(|cx| cx.set_http_client(fail));
        let window = cx.add_empty_window();
        let host = new_host();
        host.borrow_mut()
            .set_src(Some("https://img.example/cat.png"));
        settle(window, &host, &box_style(40., 40.));
        assert_eq!(painted(window).placeholder_quads, 1);

        let not_found = FakeHttpClient::with_404_response();
        window.update(|_, cx| cx.set_http_client(not_found));
        let host = new_host();
        host.borrow_mut()
            .set_src(Some("http://img.example/missing.png"));
        settle(window, &host, &box_style(40., 40.));
        assert_eq!(painted(window).placeholder_quads, 1);

        // Garbage with a success status is a decode failure.
        let garbage = FakeHttpClient::create(|_| async move {
            Ok(Response::builder()
                .status(200)
                .body(AsyncBody::from(b"<html>not an image</html>".to_vec()))
                .unwrap())
        });
        window.update(|_, cx| cx.set_http_client(garbage));
        let host = new_host();
        host.borrow_mut()
            .set_src(Some("https://img.example/page.html"));
        settle(window, &host, &box_style(40., 40.));
        assert_eq!(painted(window).placeholder_quads, 1);
    }

    #[test]
    fn polychrome_svg_preserves_authored_colors_and_alpha() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="2">
            <rect x="0" width="2" height="2" fill="#7DCFFF"/>
            <rect x="2" width="2" height="2" fill="#D0A060"/></svg>"##;
        let renderer = crate::SvgRenderer::new(Arc::new(()));
        let image = crate::decode_image_bytes(svg, &renderer).unwrap();
        // Rendered at twice the declared size, both authored colors exact (stored BGRA).
        assert_eq!(
            image.size(0),
            size(crate::DevicePixels(8), crate::DevicePixels(4))
        );
        let bytes = image.as_bytes(0).unwrap();
        assert_eq!(&bytes[4..8], &[0xFF, 0xCF, 0x7D, 0xFF]);
        assert_eq!(&bytes[24..28], &[0x60, 0xA0, 0xD0, 0xFF]);

        let half = br##"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2">
            <rect width="2" height="2" fill="#FF0000" fill-opacity="0.5"/></svg>"##;
        let image = crate::decode_image_bytes(half, &renderer).unwrap();
        let pixel = &image.as_bytes(0).unwrap()[0..4];
        assert!((127..=129).contains(&pixel[3]), "alpha {pixel:?}");
        assert!(
            pixel[2] >= 250 && pixel[0] == 0 && pixel[1] == 0,
            "straight color {pixel:?}"
        );
    }

    const SQUARE_WITH_HOLE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="80" height="80" viewBox="0 0 80 80"><path fill-rule="evenodd" d="M0 0H80V80H0Z M20 20V60H60V20Z"/></svg>"#;

    #[test]
    fn monochrome_mask_exposes_even_odd_holes() {
        let renderer = crate::SvgRenderer::new(Arc::new(()));
        let params = crate::RenderSvgParams {
            path: "hole.svg".into(),
            size: size(crate::DevicePixels(80), crate::DevicePixels(80)),
        };
        let (mask_size, alpha) = renderer
            .render_alpha_mask(&params, Some(SQUARE_WITH_HOLE.as_bytes()))
            .unwrap()
            .unwrap();
        assert_eq!((mask_size.width.0, mask_size.height.0), (80, 80));
        let at = |x: usize, y: usize| alpha[y * 80 + x];
        // Alpha, not a background-colored fill: the hole is fully transparent.
        assert_eq!(at(40, 40), 0);
        assert_eq!(at(25, 25), 0);
        assert_eq!(at(10, 10), 255);
        assert_eq!(at(70, 70), 255);
    }

    #[vespertine::test]
    fn svg_host_sources_follow_the_precedence_rules(cx: &mut TestAppContext) {
        let dir = temp_dir("svg-host");
        let path = dir.path().join("icon.svg");
        std::fs::write(&path, SQUARE_WITH_HOLE).unwrap();
        let inline = "<svg xmlns='http://www.w3.org/2000/svg' width='2' height='2'/>";
        cx.update(|cx| {
            let mut host = SvgHostState::new();
            assert!(host.bytes(cx).is_none(), "no source");

            // A data URL with an SVG MIME type (percent-decoded payload).
            host.set_src(Some(&format!(
                "data:image/svg+xml,{}",
                inline.replace('<', "%3C")
            )));
            assert_eq!(host.bytes(cx).unwrap().as_ref(), inline.as_bytes());
            // Other MIME types are not accepted by the vector host.
            host.set_src(Some("data:image/png;base64,AA=="));
            assert!(host.bytes(cx).is_none());

            // A local path.
            host.set_src(Some(path.to_str().unwrap()));
            assert_eq!(
                host.bytes(cx).unwrap().as_ref(),
                SQUARE_WITH_HOLE.as_bytes()
            );
            // Missing and invalid sources render empty.
            host.set_src(Some("/nonexistent/icon.svg"));
            assert!(host.bytes(cx).is_none());
            host.set_src(Some("https://example.com/icon.svg"));
            assert!(host.bytes(cx).is_none());

            // Inline markup wins over src when nonempty after trimming.
            host.set_src(Some(path.to_str().unwrap()));
            host.set_source(Some(inline));
            assert_eq!(host.bytes(cx).unwrap().as_ref(), inline.as_bytes());
            host.set_source(Some("   \n "));
            assert_eq!(
                host.bytes(cx).unwrap().as_ref(),
                SQUARE_WITH_HOLE.as_bytes()
            );
            // A nonempty but invalid inline source is invalid, not a fallback to src.
            host.set_source(Some("this is not svg"));
            assert!(host.bytes(cx).is_none());
            host.set_source(None);
            assert!(host.bytes(cx).is_some());
        });
    }

    #[vespertine::test]
    fn svg_host_paints_a_tinted_mask_and_keeps_its_bounds(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        let mut host = SvgHostState::new();
        host.set_source(Some(SQUARE_WITH_HOLE));
        let host = Rc::new(RefCell::new(host));
        let draw_svg = |window: &mut VisualTestContext, style: StyleRefinement| {
            let host = host.clone();
            window.draw(
                point(px(0.), px(0.)),
                size(px(300.), px(300.)),
                move |_, cx| {
                    crate::div()
                        .debug_selector(|| "wrapper".into())
                        .flex()
                        .child(host.borrow_mut().render("svg", style, cx))
                },
            );
            window.update(|window, _| {
                window
                    .rendered_frame
                    .scene
                    .monochrome_sprites
                    .iter()
                    .map(|sprite| sprite.color)
                    .collect::<Vec<_>>()
            })
        };
        // The default tint is #E2E2E2 when the style sets no color.
        let colors = draw_svg(window, box_style(80., 80.));
        assert_eq!(colors, vec![Hsla::from(rgb(SVG_HOST_DEFAULT_TINT))]);
        // The style color is the tint.
        let colors = draw_svg(window, box_style(80., 80.).text_color(rgb(0x80C090)));
        assert_eq!(colors, vec![Hsla::from(rgb(0x80C090))]);
        // The host takes its styled size and does not shrink by flex.
        let wrapper = window.debug_bounds("wrapper").unwrap();
        assert_eq!(wrapper.size, size(px(80.), px(80.)));

        // An invalid source paints nothing but keeps the styled bounds.
        host.borrow_mut().set_source(Some("garbage"));
        let colors = draw_svg(window, box_style(64., 32.));
        assert!(colors.is_empty());
        assert_eq!(
            window.debug_bounds("wrapper").unwrap().size,
            size(px(64.), px(32.))
        );
    }

    struct SourceView(ImageSource);

    impl Render for SourceView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            img(self.0.clone())
                .size(px(40.))
                .object_fit(ObjectFit::Fill)
        }
    }

    fn sprite_tile_widths(window: &mut VisualTestContext, source: ImageSource) -> Vec<i32> {
        for _ in 0..2 {
            clear_scene(window);
            let source = source.clone();
            window.draw(
                point(px(0.), px(0.)),
                size(px(100.), px(100.)),
                move |_, cx| cx.new(|_| SourceView(source)).into_any_element(),
            );
            window.run_until_parked();
        }
        painted(window)
            .sprites
            .iter()
            .map(|sprite| sprite.tile_size.0)
            .collect()
    }

    #[vespertine::test]
    fn untrusted_sources_are_bounded_and_sanitized_before_decoding(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        let dir = temp_dir("untrusted");
        let untrusted = |name: &str, bytes: &[u8]| {
            let path = dir.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            ImageSource::untrusted(Resource::Path(Arc::from(path.as_path())))
        };

        let ok = untrusted("ok.png", &encode(image::ImageFormat::Png, 16, 4));
        assert_eq!(sprite_tile_widths(window, ok), vec![16]);

        // Wider than 4096 pixels: refused before it reaches a pixel buffer.
        let wide = untrusted("wide.png", &encode(image::ImageFormat::Png, 4097, 1));
        assert!(sprite_tile_widths(window, wide).is_empty());

        // Scripts and external references are dropped; the sanitized SVG renders at twice its size.
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
            <script>alert(1)</script><image href="http://example.com/x.png" width="5" height="5"/>
            <rect width="10" height="10" fill="#00ff00"/></svg>"##;
        let sanitized = untrusted("evil.svg", svg);
        assert_eq!(sprite_tile_widths(window, sanitized), vec![20]);
    }
}
