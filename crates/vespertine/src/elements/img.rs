use crate::{
    AnyElement, AnyImageCache, App, Asset, AssetLogger, Bounds, Corners, DefiniteLength,
    DevicePixels, Element, ElementId, Entity, GlobalElementId, Hitbox, Image, ImageCache,
    InspectorElementId, InteractiveElement, Interactivity, IntoElement, LayoutId, Length,
    ObjectFit, Pixels, RenderImage, Resource, SharedString, SharedUri, StyleRefinement, Styled,
    Task, UntrustedImageLimits, Window, decode_image_bytes, decode_sniffed, decode_static_image,
    decode_untrusted_image_bytes, px, sniff_image_format, unrecognized_image_format,
};
use anyhow::Result;

use futures::Future;
use image::ImageError;
use std::{
    fs, io,
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
    time::Duration,
};
use thiserror::Error;
use vespertine_scheduler::Instant;
use vespertine_util::ResultExt;

use super::{Stateful, StatefulInteractiveElement};

/// Paints `data` with its fitted rectangle `fitted` cropped to `visible` (ELM-003 R2/R3)
/// through the fitted painter of RND-004.
fn paint_fitted_image(
    window: &mut Window,
    visible: Bounds<Pixels>,
    fitted: Bounds<Pixels>,
    corner_radii: Corners<Pixels>,
    data: Arc<RenderImage>,
    frame_index: usize,
    grayscale: bool,
) {
    window
        .paint_image_fitted(visible, fitted, corner_radii, data, frame_index, grayscale)
        .log_err();
}

/// The delay before showing the loading state.
pub const LOADING_DELAY: Duration = Duration::from_millis(200);

/// A type alias to the resource loader that the `img()` element uses.
///
/// Note: that this is only for Resources, like URLs or file paths.
/// Custom loaders, or external images will not use this asset loader
pub type ImgResourceLoader = AssetLogger<ImageAssetLoader>;

/// A source of image content.
#[derive(Clone)]
pub enum ImageSource {
    /// The image content will be loaded from some resource location
    Resource(Resource),
    /// Cached image data
    Render(Arc<RenderImage>),
    /// Cached image data
    Image(Arc<Image>),
    /// A custom loading function to use
    Custom(Arc<dyn Fn(&mut Window, &mut App) -> Option<Result<Arc<RenderImage>, ImageCacheError>>>),
}

fn is_uri(uri: &str) -> bool {
    url::Url::from_str(uri).is_ok()
}

impl From<SharedUri> for ImageSource {
    fn from(value: SharedUri) -> Self {
        Self::Resource(Resource::Uri(value))
    }
}

impl<'a> From<&'a str> for ImageSource {
    fn from(s: &'a str) -> Self {
        if is_uri(s) {
            Self::Resource(Resource::Uri(s.to_string().into()))
        } else {
            Self::Resource(Resource::Embedded(s.to_string().into()))
        }
    }
}

impl From<String> for ImageSource {
    fn from(s: String) -> Self {
        if is_uri(&s) {
            Self::Resource(Resource::Uri(s.into()))
        } else {
            Self::Resource(Resource::Embedded(s.into()))
        }
    }
}

impl From<SharedString> for ImageSource {
    fn from(s: SharedString) -> Self {
        s.as_ref().into()
    }
}

impl From<&Path> for ImageSource {
    fn from(value: &Path) -> Self {
        Self::Resource(value.to_path_buf().into())
    }
}

impl From<Arc<Path>> for ImageSource {
    fn from(value: Arc<Path>) -> Self {
        Self::Resource(value.into())
    }
}

impl From<PathBuf> for ImageSource {
    fn from(value: PathBuf) -> Self {
        Self::Resource(value.into())
    }
}

impl From<Arc<RenderImage>> for ImageSource {
    fn from(value: Arc<RenderImage>) -> Self {
        Self::Render(value)
    }
}

impl From<Arc<Image>> for ImageSource {
    fn from(value: Arc<Image>) -> Self {
        Self::Image(value)
    }
}

impl<F> From<F> for ImageSource
where
    F: Fn(&mut Window, &mut App) -> Option<Result<Arc<RenderImage>, ImageCacheError>> + 'static,
{
    fn from(value: F) -> Self {
        Self::Custom(Arc::new(value))
    }
}

/// The style of an image element.
pub struct ImageStyle {
    grayscale: bool,
    object_fit: ObjectFit,
    loading: Option<Box<dyn Fn() -> AnyElement>>,
    fallback: Option<Box<dyn Fn() -> AnyElement>>,
}

impl Default for ImageStyle {
    fn default() -> Self {
        Self {
            grayscale: false,
            object_fit: ObjectFit::Contain,
            loading: None,
            fallback: None,
        }
    }
}

/// Style an image element.
pub trait StyledImage: Sized {
    /// Get a mutable [ImageStyle] from the element.
    fn image_style(&mut self) -> &mut ImageStyle;

    /// Set the image to be displayed in grayscale.
    fn grayscale(mut self, grayscale: bool) -> Self {
        self.image_style().grayscale = grayscale;
        self
    }

    /// Set the object fit for the image.
    fn object_fit(mut self, object_fit: ObjectFit) -> Self {
        self.image_style().object_fit = object_fit;
        self
    }

    /// Set a fallback function that will be invoked to render an error view should
    /// the image fail to load.
    fn with_fallback(mut self, fallback: impl Fn() -> AnyElement + 'static) -> Self {
        self.image_style().fallback = Some(Box::new(fallback));
        self
    }

    /// Set a fallback function that will be invoked to render a view while the image
    /// is still being loaded.
    fn with_loading(mut self, loading: impl Fn() -> AnyElement + 'static) -> Self {
        self.image_style().loading = Some(Box::new(loading));
        self
    }
}

impl StyledImage for Img {
    fn image_style(&mut self) -> &mut ImageStyle {
        &mut self.style
    }
}

impl StyledImage for Stateful<Img> {
    fn image_style(&mut self) -> &mut ImageStyle {
        &mut self.element.style
    }
}

/// An image element.
pub struct Img {
    interactivity: Interactivity,
    source: ImageSource,
    style: ImageStyle,
    image_cache: Option<AnyImageCache>,
}

/// Create a new image element.
#[track_caller]
pub fn img(source: impl Into<ImageSource>) -> Img {
    Img {
        interactivity: Interactivity::new(),
        source: source.into(),
        style: ImageStyle::default(),
        image_cache: None,
    }
}

impl Img {
    /// A list of all format extensions currently supported by this img element
    pub fn extensions() -> &'static [&'static str] {
        // This is the list in [image::ImageFormat::from_extension] + `svg`
        &[
            "avif", "jpg", "jpeg", "png", "gif", "webp", "tif", "tiff", "tga", "dds", "bmp", "ico",
            "hdr", "exr", "pbm", "pam", "ppm", "pgm", "ff", "farbfeld", "qoi", "svg",
        ]
    }

    /// Sets the image alternative text. It becomes the accessible label only
    /// when no `aria-label` was supplied (not even an invalid or empty one) and
    /// the text is nonempty.
    pub fn alt(mut self, alt: impl Into<SharedString>) -> Self {
        self.interactivity.aria.alt = Some(alt.into());
        self
    }

    /// Sets the image cache for the current node.
    ///
    /// If the `image_cache` is not explicitly provided, the function will determine the image cache by:
    ///
    /// 1. Checking if any ancestor node of the current node contains an `ImageCacheElement`, If such a node exists, the image cache specified by that ancestor will be used.
    /// 2. If no ancestor node contains an `ImageCacheElement`, the global image cache will be used as a fallback.
    ///
    /// This mechanism provides a flexible way to manage image caching, allowing precise control when needed,
    /// while ensuring a default behavior when no cache is explicitly specified.
    #[inline]
    pub fn image_cache<I: ImageCache>(self, image_cache: &Entity<I>) -> Self {
        Self {
            image_cache: Some(image_cache.clone().into()),
            ..self
        }
    }
}

impl Stateful<Img> {
    /// Sets the image alternative text; see [`Img::alt`].
    pub fn alt(mut self, alt: impl Into<SharedString>) -> Self {
        self.element.interactivity.aria.alt = Some(alt.into());
        self
    }
}

impl Deref for Stateful<Img> {
    type Target = Img;

    fn deref(&self) -> &Self::Target {
        &self.element
    }
}

impl DerefMut for Stateful<Img> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.element
    }
}

/// The image state between frames
pub(crate) struct ImgState {
    pub(crate) frame_index: usize,
    last_frame_time: Option<Instant>,
    started_loading: Option<(Instant, Task<()>)>,
}

/// The image layout state between frames
pub struct ImgLayoutState {
    frame_index: usize,
    replacement: Option<AnyElement>,
}

impl Element for Img {
    type RequestLayoutState = ImgLayoutState;
    type PrepaintState = Option<Hitbox>;

    fn id(&self) -> Option<ElementId> {
        self.interactivity.element_id.clone()
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        self.interactivity.source_location()
    }

    fn a11y_role(&self) -> Option<accesskit::Role> {
        self.interactivity.a11y_role()
    }

    fn write_a11y_info(&self, node: &mut accesskit::Node) {
        self.interactivity.write_a11y_info(node);
        self.interactivity.write_alt_fallback(node);
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut layout_state = ImgLayoutState {
            frame_index: 0,
            replacement: None,
        };

        window.with_optional_element_state(global_id, |state, window| {
            let mut state = state.map(|state| {
                state.unwrap_or(ImgState {
                    frame_index: 0,
                    last_frame_time: None,
                    started_loading: None,
                })
            });

            let mut frame_index = state.as_ref().map(|state| state.frame_index).unwrap_or(0);

            let layout_id = self.interactivity.request_layout(
                global_id,
                inspector_id,
                window,
                cx,
                |mut style, window, cx| {
                    let mut replacement_id = None;

                    match self.source.use_data(
                        self.image_cache
                            .clone()
                            .or_else(|| window.image_cache_stack.last().cloned()),
                        window,
                        cx,
                    ) {
                        Some(Ok(data)) => {
                            let frame_count = data.frame_count();
                            let max_frame_index = frame_count.saturating_sub(1);

                            if let Some(state) = &mut state {
                                state.frame_index = state.frame_index.min(max_frame_index);
                                if frame_count > 1 && !cx.reduce_motion() {
                                    if window.is_window_active() {
                                        let current_time = Instant::now();
                                        if let Some(last_frame_time) = state.last_frame_time {
                                            let elapsed = current_time - last_frame_time;
                                            let frame_duration =
                                                Duration::from(data.delay(state.frame_index));

                                            if elapsed >= frame_duration {
                                                state.frame_index =
                                                    (state.frame_index + 1) % frame_count;
                                                state.last_frame_time =
                                                    Some(current_time - (elapsed - frame_duration));
                                            }
                                        } else {
                                            state.last_frame_time = Some(current_time);
                                        }
                                    } else {
                                        state.last_frame_time = None;
                                    }
                                } else {
                                    state.last_frame_time = None;
                                }
                                state.started_loading = None;
                                frame_index = state.frame_index;
                            }

                            let image_size = data.render_size(frame_index);

                            // R1: the intrinsic ratio only applies when a dimension is auto
                            // and the style carries no explicit aspect ratio.
                            if style.aspect_ratio.is_none()
                                && (matches!(style.size.width, Length::Auto)
                                    || matches!(style.size.height, Length::Auto))
                            {
                                style.aspect_ratio = Some(image_size.width / image_size.height);
                            }

                            if let Length::Auto = style.size.width {
                                style.size.width = match style.size.height {
                                    Length::Definite(DefiniteLength::Absolute(abs_length)) => {
                                        let height_px = abs_length.to_pixels(window.rem_size());
                                        Length::Definite(
                                            px(image_size.width.0 * height_px.0
                                                / image_size.height.0)
                                            .into(),
                                        )
                                    }
                                    _ => Length::Definite(image_size.width.into()),
                                };
                            }

                            if let Length::Auto = style.size.height {
                                style.size.height = match style.size.width {
                                    Length::Definite(DefiniteLength::Absolute(abs_length)) => {
                                        let width_px = abs_length.to_pixels(window.rem_size());
                                        Length::Definite(
                                            px(image_size.height.0 * width_px.0
                                                / image_size.width.0)
                                            .into(),
                                        )
                                    }
                                    _ => Length::Definite(image_size.height.into()),
                                };
                            }

                            if global_id.is_some()
                                && data.frame_count() > 1
                                && window.is_window_active()
                                && !cx.reduce_motion()
                            {
                                window.request_animation_frame();
                            }
                        }
                        Some(_err) => {
                            if let Some(fallback) = self.style.fallback.as_ref() {
                                let mut element = fallback();
                                replacement_id = Some(element.request_layout(window, cx));
                                layout_state.replacement = Some(element);
                            }
                            if let Some(state) = &mut state {
                                state.started_loading = None;
                            }
                        }
                        None => {
                            if let Some(state) = &mut state {
                                if let Some((started_loading, _)) = state.started_loading {
                                    if started_loading.elapsed() > LOADING_DELAY
                                        && let Some(loading) = self.style.loading.as_ref()
                                    {
                                        let mut element = loading();
                                        replacement_id = Some(element.request_layout(window, cx));
                                        layout_state.replacement = Some(element);
                                    }
                                } else {
                                    let current_view = window.current_view();
                                    let task = window.spawn(cx, async move |cx| {
                                        cx.background_executor().timer(LOADING_DELAY).await;
                                        cx.update(move |_, cx| {
                                            cx.notify(current_view);
                                        })
                                        .ok();
                                    });
                                    state.started_loading = Some((Instant::now(), task));
                                }
                            }
                        }
                    }

                    window.request_layout(style, replacement_id, cx)
                },
            );

            layout_state.frame_index = frame_index;

            ((layout_id, layout_state), state)
        })
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.interactivity.prepaint(
            global_id,
            inspector_id,
            bounds,
            bounds.size,
            window,
            cx,
            |_, _, hitbox, window, cx| {
                if let Some(replacement) = &mut request_layout.replacement {
                    replacement.prepaint(window, cx);
                }

                hitbox
            },
        )
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout_state: &mut Self::RequestLayoutState,
        hitbox: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let source = self.source.clone();
        self.interactivity.paint(
            global_id,
            inspector_id,
            bounds,
            hitbox.as_ref(),
            window,
            cx,
            |style, window, cx| {
                if let Some(Ok(data)) = source.use_data(
                    self.image_cache
                        .clone()
                        .or_else(|| window.image_cache_stack.last().cloned()),
                    window,
                    cx,
                ) {
                    if data.frame_count() == 0 {
                        return;
                    }
                    let fitted = self.style.object_fit.get_bounds(
                        bounds,
                        data.render_size(layout_state.frame_index)
                            .map(|dimension| DevicePixels(dimension.0.round() as i32)),
                    );
                    let visible = bounds.intersect(&fitted);
                    if visible.size.width <= Pixels::ZERO || visible.size.height <= Pixels::ZERO {
                        return;
                    }
                    let corner_radii = style.corner_radii.to_pixels(window.rem_size());
                    paint_fitted_image(
                        window,
                        visible,
                        fitted,
                        corner_radii,
                        data,
                        layout_state.frame_index,
                        self.style.grayscale,
                    );
                } else if let Some(replacement) = &mut layout_state.replacement {
                    replacement.paint(window, cx);
                }
            },
        )
    }
}

impl Styled for Img {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.interactivity.base_style
    }
}

impl InteractiveElement for Img {
    fn interactivity(&mut self) -> &mut Interactivity {
        &mut self.interactivity
    }
}

impl IntoElement for Img {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl StatefulInteractiveElement for Img {}

impl ImageSource {
    pub(crate) fn use_data(
        &self,
        cache: Option<AnyImageCache>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        match self {
            ImageSource::Resource(resource) => {
                if let Some(cache) = cache {
                    cache.load(resource, window, cx)
                } else {
                    window.use_asset::<ImgResourceLoader>(resource, cx)
                }
            }
            ImageSource::Custom(loading_fn) => loading_fn(window, cx),
            ImageSource::Render(data) => Some(Ok(data.to_owned())),
            ImageSource::Image(data) => window.use_asset::<AssetLogger<ImageDecoder>>(data, cx),
        }
    }

    pub(crate) fn get_data(
        &self,
        cache: Option<AnyImageCache>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        match self {
            ImageSource::Resource(resource) => {
                if let Some(cache) = cache {
                    cache.load(resource, window, cx)
                } else {
                    window.get_asset::<ImgResourceLoader>(resource, cx)
                }
            }
            ImageSource::Custom(loading_fn) => loading_fn(window, cx),
            ImageSource::Render(data) => Some(Ok(data.to_owned())),
            ImageSource::Image(data) => window.get_asset::<AssetLogger<ImageDecoder>>(data, cx),
        }
    }

    /// A source for an image from an untrusted origin: bytes are bounded and decoded with the
    /// [`UntrustedImageLimits`] defaults and SVG is rebuilt before rendering (ELM-004 R28).
    pub fn untrusted(resource: Resource) -> Self {
        ImageSource::Custom(Arc::new(move |window, cx| {
            window.use_asset::<AssetLogger<UntrustedImageAssetLoader>>(&resource, cx)
        }))
    }

    /// Remove this image source from the asset system
    pub fn remove_asset(&self, cx: &mut App) {
        match self {
            ImageSource::Resource(resource) => {
                cx.remove_asset::<ImgResourceLoader>(resource);
            }
            ImageSource::Custom(_) | ImageSource::Render(_) => {}
            ImageSource::Image(data) => cx.remove_asset::<AssetLogger<ImageDecoder>>(data),
        }
    }

    /// Evict this image: remove its cached asset (as [`Self::remove_asset`] does) and
    /// release the decoded image's atlas tiles in every window.
    ///
    /// The decoded image is obtained without starting any load; a pending or failed
    /// load frees no tiles but its cache entry is still removed. Pass the currently
    /// updating window as `window`, since it is absent from the application's window
    /// collection while being updated. Rendered-image sources have no cache entry and
    /// only release tiles; custom sources are caller-owned and are left untouched.
    pub fn evict(&self, window: Option<&mut Window>, cx: &mut App) {
        let image = match self {
            ImageSource::Resource(resource) => {
                let image = cx
                    .peek_asset::<ImgResourceLoader>(resource)
                    .and_then(Result::ok);
                cx.remove_asset::<ImgResourceLoader>(resource);
                image
            }
            ImageSource::Image(data) => {
                let image = cx
                    .peek_asset::<AssetLogger<ImageDecoder>>(data)
                    .and_then(Result::ok);
                cx.remove_asset::<AssetLogger<ImageDecoder>>(data);
                image
            }
            ImageSource::Render(data) => Some(data.clone()),
            ImageSource::Custom(_) => None,
        };
        if let Some(image) = image {
            cx.drop_image(image, window);
        }
    }

    /// Check whether this image source is present in the asset system (loading
    /// or loaded), without fetching it.
    #[cfg(any(test, feature = "test-support"))]
    pub fn is_asset_cached(&self, cx: &App) -> bool {
        match self {
            ImageSource::Resource(resource) => cx.has_asset::<ImgResourceLoader>(resource),
            ImageSource::Custom(_) | ImageSource::Render(_) => false,
            ImageSource::Image(data) => cx.has_asset::<AssetLogger<ImageDecoder>>(data),
        }
    }
}

#[derive(Clone)]
enum ImageDecoder {}

impl Asset for ImageDecoder {
    type Source = Arc<Image>;
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    fn load(
        source: Self::Source,
        cx: &mut App,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        let renderer = cx.svg_renderer();
        // The bytes decide the format, not the declared one (ELM-004 R3).
        async move { decode_image_bytes(&source.bytes, &renderer) }
    }
}

/// Reads the bytes behind a resource (disk, network through the application's HTTP client,
/// or an embedded asset).
async fn read_resource_bytes(
    source: Resource,
    client: Arc<dyn vespertine_http_client::HttpClient>,
    asset_source: Arc<dyn crate::AssetSource>,
) -> Result<Vec<u8>, ImageCacheError> {
    Ok(match source {
        Resource::Path(uri) => fs::read(uri.as_ref())?,
        Resource::Uri(uri) => {
            use anyhow::Context as _;
            use futures::AsyncReadExt as _;

            let mut response = client
                .get(uri.as_ref(), ().into(), true)
                .await
                .with_context(|| format!("loading image asset from {uri:?}"))?;
            let mut body = Vec::new();
            response.body_mut().read_to_end(&mut body).await?;
            if !response.status().is_success() {
                let mut body = String::from_utf8_lossy(&body).into_owned();
                let first_line = body.lines().next().unwrap_or("").trim_end();
                body.truncate(first_line.len());
                return Err(ImageCacheError::BadStatus {
                    uri,
                    status: response.status(),
                    body,
                });
            }
            body
        }
        Resource::Embedded(path) => {
            let data = asset_source.load(&path).ok().flatten();
            if let Some(data) = data {
                data.to_vec()
            } else {
                return Err(ImageCacheError::Asset(
                    format!("Embedded resource not found: {}", path).into(),
                ));
            }
        }
    })
}

/// An image loader for the VESPERTINE asset system
#[derive(Clone)]
pub enum ImageAssetLoader {}

impl Asset for ImageAssetLoader {
    type Source = Resource;
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    fn load(
        source: Self::Source,
        cx: &mut App,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        let client = cx.http_client();
        // SVGs render at twice their declared size (ELM-004 R17).
        let svg_renderer = cx.svg_renderer();
        let asset_source = cx.asset_source().clone();
        async move {
            let bytes = read_resource_bytes(source, client, asset_source).await?;
            match sniff_image_format(&bytes) {
                Some(format) => decode_sniffed(&bytes, format, &svg_renderer),
                // Formats outside the sniffing table that the decoder crate still knows.
                None => match image::guess_format(&bytes) {
                    Ok(format) => Ok(Arc::new(RenderImage::new(decode_static_image(
                        &bytes, format,
                    )?))),
                    Err(_) => Err(unrecognized_image_format()),
                },
            }
        }
    }
}

/// The resource loader for images from untrusted sources: the input length is bounded before
/// decoding, rasters are decoded with the [`UntrustedImageLimits`] and keep one static
/// frame, and SVG is rebuilt so no script, HTML or external reference reaches the renderer
/// (ELM-004 R28). Use it through [`ImageSource::untrusted`].
#[derive(Clone)]
pub enum UntrustedImageAssetLoader {}

impl Asset for UntrustedImageAssetLoader {
    type Source = Resource;
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    fn load(
        source: Self::Source,
        cx: &mut App,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        let client = cx.http_client();
        let svg_renderer = cx.svg_renderer();
        let asset_source = cx.asset_source().clone();
        async move {
            let bytes = read_resource_bytes(source, client, asset_source).await?;
            decode_untrusted_image_bytes(&bytes, &svg_renderer, &UntrustedImageLimits::default())
        }
    }
}

/// An error that can occur when interacting with the image cache.
#[derive(Debug, Error, Clone)]
pub enum ImageCacheError {
    /// Some other kind of error occurred
    #[error("error: {0}")]
    Other(#[from] Arc<anyhow::Error>),
    /// An error that occurred while reading the image from disk.
    #[error("IO error: {0}")]
    Io(Arc<std::io::Error>),
    /// An error that occurred while processing an image.
    #[error("unexpected http status for {uri}: {status}, body: {body}")]
    BadStatus {
        /// The URI of the image.
        uri: SharedUri,
        /// The HTTP status code.
        status: vespertine_http_client::StatusCode,
        /// The HTTP response body.
        body: String,
    },
    /// An error that occurred while processing an asset.
    #[error("asset error: {0}")]
    Asset(SharedString),
    /// An error that occurred while processing an image.
    #[error("image error: {0}")]
    Image(Arc<ImageError>),
    /// An error that occurred while processing an SVG.
    #[error("svg error: {0}")]
    Usvg(Arc<usvg::Error>),
}

impl From<anyhow::Error> for ImageCacheError {
    fn from(value: anyhow::Error) -> Self {
        Self::Other(Arc::new(value))
    }
}

impl From<io::Error> for ImageCacheError {
    fn from(value: io::Error) -> Self {
        Self::Io(Arc::new(value))
    }
}

impl From<usvg::Error> for ImageCacheError {
    fn from(value: usvg::Error) -> Self {
        Self::Usvg(Arc::new(value))
    }
}

impl From<image::ImageError> for ImageCacheError {
    fn from(value: image::ImageError) -> Self {
        Self::Image(Arc::new(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ParentElement as _, TestAppContext, canvas, div, point, px, size};
    use image::{Frame, ImageBuffer, Rgba};
    use smallvec::SmallVec;

    const TEST_IMG_ID: &str = "test-img";

    fn test_image(frame_count: usize) -> Arc<RenderImage> {
        let frame = Frame::new(ImageBuffer::from_pixel(1, 1, Rgba([0, 0, 0, 0])));
        Arc::new(RenderImage::new(SmallVec::from_iter(
            (0..frame_count).map(|_| frame.clone()),
        )))
    }

    fn test_image_with_size(width: u32, height: u32) -> Arc<RenderImage> {
        let frame = Frame::new(ImageBuffer::from_pixel(width, height, Rgba([0, 0, 0, 0])));
        Arc::new(RenderImage::new(SmallVec::from_elem(frame, 1)))
    }

    /// Overwrites the cached `frame_index` of the sibling `img` during paint.
    fn seed_frame_index(frame_index: usize) -> impl IntoElement {
        canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                window.with_global_id(TEST_IMG_ID.into(), |id, window| {
                    window.with_element_state::<ImgState, _>(id, |state, _| {
                        let mut state = state.expect("img state should be initialized");
                        state.frame_index = frame_index;
                        ((), state)
                    });
                });
            },
        )
    }

    #[vespertine::test]
    fn zero_frame_image_does_not_panic_on_paint(cx: &mut TestAppContext) {
        cx.add_empty_window()
            .draw(point(px(0.), px(0.)), size(px(100.), px(100.)), |_, _| {
                img(ImageSource::Render(test_image(0))).into_any_element()
            });
    }

    #[vespertine::test]
    fn image_object_fit_cover_crops_to_element_bounds(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        let image = test_image_with_size(200, 100);
        window.draw(point(px(0.), px(0.)), size(px(100.), px(100.)), |_, _| {
            img(ImageSource::Render(image.clone()))
                .size_full()
                .object_fit(ObjectFit::Fill)
                .into_any_element()
        });
        let full_tile_bounds = window.update(|window, _| {
            window
                .rendered_frame
                .scene
                .polychrome_sprites
                .last()
                .expect("fill image should paint a sprite")
                .tile
                .bounds
        });

        window.draw(point(px(10.), px(20.)), size(px(100.), px(100.)), |_, _| {
            img(ImageSource::Render(image))
                .size_full()
                .object_fit(ObjectFit::Cover)
                .into_any_element()
        });

        let (rendered_bounds, rendered_tile_bounds, scale_factor) = window.update(|window, _| {
            let sprite = window
                .rendered_frame
                .scene
                .polychrome_sprites
                .last()
                .expect("cover image should paint a sprite");
            (sprite.bounds, sprite.tile.bounds, window.scale_factor())
        });
        assert_eq!(
            rendered_bounds,
            Bounds {
                origin: point(px(10.).scale(scale_factor), px(20.).scale(scale_factor)),
                size: size(px(100.).scale(scale_factor), px(100.).scale(scale_factor)),
            }
        );
        assert_eq!(
            (
                rendered_tile_bounds.origin.x.0 - full_tile_bounds.origin.x.0,
                rendered_tile_bounds.origin.y.0 - full_tile_bounds.origin.y.0,
                rendered_tile_bounds.size.width.0,
                rendered_tile_bounds.size.height.0,
            ),
            (50, 0, 100, 100),
        );
    }

    #[vespertine::test]
    fn explicit_aspect_ratio_is_not_overridden_by_intrinsic_ratio(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();

        // A portrait image in a square container
        window.draw(point(px(0.), px(0.)), size(px(100.), px(100.)), |_, _| {
            div()
                .size(px(100.))
                .overflow_hidden()
                .child(
                    img(ImageSource::Render(test_image_with_size(100, 200)))
                        .size_full()
                        .aspect_square()
                        .object_fit(ObjectFit::Contain),
                )
                .into_any_element()
        });

        let (rendered_bounds, scale_factor) = window.update(|window, _| {
            let sprite = window
                .rendered_frame
                .scene
                .polychrome_sprites
                .last()
                .expect("contained image should paint a sprite");
            (sprite.bounds, window.scale_factor())
        });

        // The element stays 100x100, so the image is letterboxed horizontally
        assert_eq!(
            rendered_bounds,
            Bounds {
                origin: point(px(25.).scale(scale_factor), px(0.).scale(scale_factor)),
                size: size(px(50.).scale(scale_factor), px(100.).scale(scale_factor)),
            }
        );
    }

    #[vespertine::test]
    fn image_object_fit_cover_clamps_corner_radii_to_visible_bounds(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.draw(point(px(0.), px(0.)), size(px(100.), px(100.)), |_, _| {
            img(ImageSource::Render(test_image_with_size(200, 100)))
                .size_full()
                .rounded(px(100.))
                .object_fit(ObjectFit::Cover)
                .into_any_element()
        });

        let (corner_radius, expected_corner_radius) = window.update(|window, _| {
            (
                window
                    .rendered_frame
                    .scene
                    .polychrome_sprites
                    .last()
                    .map(|sprite| sprite.corner_radii.top_left),
                px(50.).scale(window.scale_factor()),
            )
        });
        assert_eq!(corner_radius, Some(expected_corner_radius));
    }

    #[vespertine::test]
    fn stale_frame_index_is_clamped_when_image_changes(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();

        // Assert that a cached frame_index from a previous multi-frame image
        // does not cause an out-of-bounds panic when the image is replaced
        // with one that has fewer frames.
        window.draw(point(px(0.), px(0.)), size(px(100.), px(100.)), |_, _| {
            div()
                .child(img(ImageSource::Render(test_image(5))).id(TEST_IMG_ID))
                .child(seed_frame_index(4))
                .into_any_element()
        });
        window.draw(point(px(0.), px(0.)), size(px(100.), px(100.)), |_, _| {
            img(ImageSource::Render(test_image(1)))
                .id(TEST_IMG_ID)
                .into_any_element()
        });
    }

    // --- ELM-003: layout rule R1 and element-space rounded corners ---------------------

    fn element_bounds(
        cx: &mut TestAppContext,
        build: impl FnOnce() -> AnyElement,
    ) -> Bounds<Pixels> {
        let window = cx.add_empty_window();
        window.draw(point(px(0.), px(0.)), size(px(500.), px(500.)), |_, _| {
            build()
        });
        window
            .debug_bounds("subject")
            .expect("the subject image should have recorded bounds")
    }

    fn subject(width: u32, height: u32) -> Img {
        img(ImageSource::Render(test_image_with_size(width, height)))
            .debug_selector(|| "subject".into())
            .flex_none()
    }

    #[vespertine::test]
    fn explicit_size_is_not_overridden_by_intrinsic_ratio(cx: &mut TestAppContext) {
        // 2:1 image, every kind of explicit length: the element stays square.
        let absolute = element_bounds(cx, || {
            subject(200, 100).w(px(100.)).h(px(100.)).into_any_element()
        });
        assert_eq!(absolute.size, size(px(100.), px(100.)));

        let rem = element_bounds(cx, || {
            subject(200, 100)
                .w(crate::rems(6.25))
                .h(crate::rems(6.25))
                .into_any_element()
        });
        assert_eq!(rem.size, size(px(100.), px(100.)));

        let relative = element_bounds(cx, || {
            div()
                .w(px(100.))
                .h(px(100.))
                .child(
                    subject(200, 100)
                        .w(crate::relative(1.))
                        .h(crate::relative(1.)),
                )
                .into_any_element()
        });
        assert_eq!(relative.size, size(px(100.), px(100.)));
    }

    #[vespertine::test]
    fn explicit_aspect_ratio_wins_over_intrinsic_ratio_in_layout(cx: &mut TestAppContext) {
        // Width is explicit and the height is auto: the explicit 1:1 ratio is kept even
        // though the image is 2:1.
        let bounds = element_bounds(cx, || {
            subject(200, 100)
                .w(px(100.))
                .aspect_square()
                .into_any_element()
        });
        assert_eq!(bounds.size, size(px(100.), px(100.)));
    }

    #[vespertine::test]
    fn auto_dimensions_follow_the_intrinsic_size(cx: &mut TestAppContext) {
        // Width auto with an absolute height: width follows the intrinsic ratio.
        let width_auto = element_bounds(cx, || subject(200, 100).h(px(50.)).into_any_element());
        assert_eq!(width_auto.size, size(px(100.), px(50.)));

        // Height auto with an absolute width.
        let height_auto = element_bounds(cx, || subject(200, 100).w(px(50.)).into_any_element());
        assert_eq!(height_auto.size, size(px(50.), px(25.)));

        // Both auto: the intrinsic size.
        let both_auto = element_bounds(cx, || subject(200, 100).into_any_element());
        assert_eq!(both_auto.size, size(px(200.), px(100.)));

        // Both explicit.
        let both_explicit = element_bounds(cx, || {
            subject(200, 100).w(px(30.)).h(px(70.)).into_any_element()
        });
        assert_eq!(both_explicit.size, size(px(30.), px(70.)));

        // Rem-based lengths convert with the rem size (16 px by default).
        let rem_height = element_bounds(cx, || {
            subject(200, 100).h(crate::rems(2.)).into_any_element()
        });
        assert_eq!(rem_height.size, size(px(64.), px(32.)));
    }

    struct PaintedSprite {
        bounds: Bounds<crate::ScaledPixels>,
        tile: crate::AtlasTile,
        corner_radii: Corners<crate::ScaledPixels>,
        scale_factor: f32,
    }

    fn painted_sprites(
        cx: &mut TestAppContext,
        build: impl FnOnce() -> AnyElement,
    ) -> Vec<PaintedSprite> {
        let window = cx.add_empty_window();
        window.draw(point(px(0.), px(0.)), size(px(100.), px(100.)), |_, _| {
            build()
        });
        window.update(|window, _| {
            let scale_factor = window.scale_factor();
            window
                .rendered_frame
                .scene
                .polychrome_sprites
                .iter()
                .map(|sprite| PaintedSprite {
                    bounds: sprite.bounds,
                    tile: sprite.tile,
                    corner_radii: sprite.corner_radii,
                    scale_factor,
                })
                .collect()
        })
    }

    fn logical(bounds: Bounds<crate::ScaledPixels>, scale_factor: f32) -> Bounds<Pixels> {
        Bounds {
            origin: point(
                px(bounds.origin.x.0 / scale_factor),
                px(bounds.origin.y.0 / scale_factor),
            ),
            size: size(
                px(bounds.size.width.0 / scale_factor),
                px(bounds.size.height.0 / scale_factor),
            ),
        }
    }

    #[vespertine::test]
    fn cover_rounds_the_visible_rectangle_and_clamps_radius(cx: &mut TestAppContext) {
        // 200x100 image, 100x100 element, cover fit, radius 100.
        let sprites = painted_sprites(cx, || {
            subject(200, 100)
                .size(px(100.))
                .rounded(px(100.))
                .object_fit(ObjectFit::Cover)
                .into_any_element()
        });
        let sprite = sprites.last().expect("cover image paints one sprite");
        // The sprite is exactly the element bounds, not the oversized fitted rectangle.
        assert_eq!(
            logical(sprite.bounds, sprite.scale_factor),
            Bounds {
                origin: point(px(0.), px(0.)),
                size: size(px(100.), px(100.)),
            }
        );
        // The corner radius is clamped to half the visible rectangle: 50 logical pixels.
        let expected = px(50.).scale(sprite.scale_factor);
        assert_eq!(sprite.corner_radii.top_left, expected);
        assert_eq!(sprite.corner_radii.top_right, expected);
        assert_eq!(sprite.corner_radii.bottom_left, expected);
        assert_eq!(sprite.corner_radii.bottom_right, expected);
        // The crop keeps the middle half of the source columns (25%..75%).
        assert_eq!(sprite.tile.bounds.size.width.0, 100);
        assert_eq!(sprite.tile.bounds.size.height.0, 100);
    }

    #[vespertine::test]
    fn contain_rounds_the_letterboxed_rectangle(cx: &mut TestAppContext) {
        // A 2:1 image in a 100x100 element is a 100x50 rectangle at y = 25 with its own
        // rounded corners: the radius is clamped to half of the shorter side, 25.
        let sprites = painted_sprites(cx, || {
            subject(200, 100)
                .size(px(100.))
                .rounded(px(100.))
                .object_fit(ObjectFit::Contain)
                .into_any_element()
        });
        let sprite = sprites.last().expect("contained image paints one sprite");
        assert_eq!(
            logical(sprite.bounds, sprite.scale_factor),
            Bounds {
                origin: point(px(0.), px(25.)),
                size: size(px(100.), px(50.)),
            }
        );
        assert_eq!(
            sprite.corner_radii.top_left,
            px(25.).scale(sprite.scale_factor)
        );
        // Uncropped: the whole tile is used.
        assert_eq!(sprite.tile.bounds.size.width.0, 200);
        assert_eq!(sprite.tile.bounds.size.height.0, 100);
    }

    #[vespertine::test]
    fn portrait_in_square_aspect_contains_horizontally(cx: &mut TestAppContext) {
        let sprites = painted_sprites(cx, || {
            div()
                .size(px(100.))
                .child(
                    subject(100, 200)
                        .size_full()
                        .aspect_square()
                        .object_fit(ObjectFit::Contain),
                )
                .into_any_element()
        });
        let sprite = sprites.last().expect("contained image paints one sprite");
        assert_eq!(
            logical(sprite.bounds, sprite.scale_factor),
            Bounds {
                origin: point(px(25.), px(0.)),
                size: size(px(50.), px(100.)),
            }
        );
    }

    #[vespertine::test]
    fn zero_frame_paints_nothing_and_uncropped_uses_the_whole_tile(cx: &mut TestAppContext) {
        let none = painted_sprites(cx, || {
            img(ImageSource::Render(test_image(0)))
                .size(px(100.))
                .into_any_element()
        });
        assert!(none.is_empty());

        let uncropped = painted_sprites(cx, || {
            subject(200, 100)
                .size(px(100.))
                .object_fit(ObjectFit::Fill)
                .into_any_element()
        });
        let sprite = uncropped.last().expect("fill image paints one sprite");
        assert_eq!(
            logical(sprite.bounds, sprite.scale_factor),
            Bounds {
                origin: point(px(0.), px(0.)),
                size: size(px(100.), px(100.)),
            }
        );
        assert_eq!(
            (
                sprite.tile.bounds.size.width.0,
                sprite.tile.bounds.size.height.0
            ),
            (200, 100)
        );
    }
}

#[cfg(test)]
mod eviction_tests {
    use super::*;
    use crate::{
        AnyView, AppContext as _, Context, Empty, Render, TestAppContext, VisualTestContext, point,
        size,
    };
    use image::{Frame, ImageBuffer, Rgba};
    use smallvec::SmallVec;
    use std::cell::Cell;
    use std::rc::Rc;

    const PNG_FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/examples/image/app-icon.png");
    const JPEG_FIXTURE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/examples/image/exif-orientation-rotate-180.jpg"
    );
    const MISSING_FIXTURE: &str = "/nonexistent/app-001/missing.png";

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum LoadState {
        Pending,
        Failed,
        Completed,
    }

    type Decoded = Option<Result<Arc<RenderImage>, ImageCacheError>>;

    fn path_source(path: &str) -> ImageSource {
        ImageSource::Resource(Resource::Path(Path::new(path).into()))
    }

    fn encoded_source(bytes: Vec<u8>) -> ImageSource {
        ImageSource::Image(Arc::new(Image::from_bytes(crate::ImageFormat::Png, bytes)))
    }

    fn png_bytes() -> Vec<u8> {
        fs::read(PNG_FIXTURE).expect("PNG fixture should be readable")
    }

    fn new_window(cx: &mut TestAppContext) -> VisualTestContext {
        let handle = cx.add_window(|_, _| Empty);
        VisualTestContext::from_window(handle.into(), cx)
    }

    /// A view that paints one image, so draws have a rendering view for asset use.
    struct ImageView(ImageSource);

    impl Render for ImageView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            img(self.0.clone()).size(px(64.))
        }
    }

    fn draw(window: &mut VisualTestContext, source: &ImageSource) {
        window.draw(point(px(0.), px(0.)), size(px(64.), px(64.)), |_, cx| {
            AnyView::from(cx.new(|_| ImageView(source.clone()))).into_any_element()
        });
    }

    /// Draws `source` until its load reaches `state`, leaving it painted when completed.
    fn drive(window: &mut VisualTestContext, source: &ImageSource, state: LoadState) {
        draw(window, source);
        if state != LoadState::Pending {
            window.run_until_parked();
            draw(window, source);
        }
    }

    /// Inspects the cached decode of a cache-backed source without loading it.
    fn peek(cx: &TestAppContext, source: &ImageSource) -> Decoded {
        cx.read(|cx| match source {
            ImageSource::Resource(resource) => cx.peek_asset::<ImgResourceLoader>(resource),
            ImageSource::Image(data) => cx.peek_asset::<AssetLogger<ImageDecoder>>(data),
            ImageSource::Render(_) | ImageSource::Custom(_) => {
                panic!("rendered and custom sources have no cache entry")
            }
        })
    }

    fn decoded_image(cx: &TestAppContext, source: &ImageSource) -> Arc<RenderImage> {
        match peek(cx, source) {
            Some(Ok(image)) => image,
            Some(Err(error)) => panic!("source should have decoded, got {error:?}"),
            None => panic!("source should be completed"),
        }
    }

    fn has_tiles(window: &mut VisualTestContext, image: &RenderImage) -> bool {
        window.update(|window, _| window.has_image_atlas_entry(image))
    }

    fn is_cached(cx: &TestAppContext, source: &ImageSource) -> bool {
        cx.read(|cx| source.is_asset_cached(cx))
    }

    fn evict_in(window: &mut VisualTestContext, source: &ImageSource) {
        window.update(|window, cx| source.evict(Some(window), cx));
    }

    fn render_image() -> Arc<RenderImage> {
        let frame = Frame::new(ImageBuffer::from_pixel(8, 8, Rgba([1, 2, 3, 255])));
        Arc::new(RenderImage::new(SmallVec::from_elem(frame, 1)))
    }

    /// Evicts a cache-backed source in `state` and checks the full R4/R5/R6/R7 contract
    /// while a bystander image in the same window stays untouched.
    fn assert_cache_backed_eviction(
        cx: &mut TestAppContext,
        source: ImageSource,
        state: LoadState,
    ) {
        let mut window = new_window(cx);
        let bystander = path_source(JPEG_FIXTURE);
        drive(&mut window, &bystander, LoadState::Completed);
        let bystander_image = decoded_image(cx, &bystander);
        assert!(has_tiles(&mut window, &bystander_image));

        drive(&mut window, &source, state);
        assert!(is_cached(cx, &source));
        let decoded = match (state, peek(cx, &source)) {
            (LoadState::Pending, None) => None,
            (LoadState::Failed, Some(Err(_))) => None,
            (LoadState::Completed, Some(Ok(image))) => Some(image),
            (state, other) => panic!("unexpected {state:?} cache contents: {other:?}"),
        };
        if let Some(image) = &decoded {
            assert!(has_tiles(&mut window, image));
        }

        evict_in(&mut window, &source);
        cx.run_until_parked();

        assert!(!is_cached(cx, &source));
        assert!(peek(cx, &source).is_none());
        if let Some(image) = &decoded {
            assert!(!has_tiles(&mut window, image));
        }
        assert!(is_cached(cx, &bystander));
        assert!(has_tiles(&mut window, &bystander_image));

        // A second eviction changes nothing and does not panic.
        evict_in(&mut window, &source);
        assert!(!is_cached(cx, &source));
        assert!(is_cached(cx, &bystander));
        assert!(has_tiles(&mut window, &bystander_image));
    }

    #[vespertine::test]
    fn app_001_a3_eviction_releases_tiles_in_every_window_including_passed_current_window(
        cx: &mut TestAppContext,
    ) {
        let mut first = new_window(cx);
        let mut second = new_window(cx);
        let source = path_source(PNG_FIXTURE);

        draw(&mut first, &source);
        draw(&mut second, &source);
        cx.run_until_parked();
        draw(&mut first, &source);
        draw(&mut second, &source);

        let image = decoded_image(cx, &source);
        assert!(has_tiles(&mut first, &image));
        assert!(has_tiles(&mut second, &image));
        assert!(is_cached(cx, &source));

        // `first` is the currently updating window and is passed explicitly.
        evict_in(&mut first, &source);

        assert!(!has_tiles(&mut first, &image));
        assert!(!has_tiles(&mut second, &image));
        assert!(!is_cached(cx, &source));
        assert!(peek(cx, &source).is_none());
    }

    #[vespertine::test]
    fn app_001_a3_current_window_keeps_its_tiles_when_not_passed(cx: &mut TestAppContext) {
        let mut first = new_window(cx);
        let mut second = new_window(cx);
        let source = path_source(PNG_FIXTURE);
        for window in [&mut first, &mut second] {
            drive(window, &source, LoadState::Completed);
        }
        // `second` finished its draw before `first` loaded; make sure both have tiles.
        draw(&mut second, &source);
        let image = decoded_image(cx, &source);
        assert!(has_tiles(&mut first, &image));
        assert!(has_tiles(&mut second, &image));

        // While `first` is being updated it is absent from the window collection, so
        // evicting without passing it releases only the other window's tiles.
        first.update(|_, cx| source.evict(None, cx));

        assert!(has_tiles(&mut first, &image));
        assert!(!has_tiles(&mut second, &image));
        assert!(!is_cached(cx, &source));
    }

    #[vespertine::test]
    fn app_001_a3_encoded_image_eviction_releases_tiles_in_every_window(cx: &mut TestAppContext) {
        let mut first = new_window(cx);
        let mut second = new_window(cx);
        let source = encoded_source(png_bytes());

        draw(&mut first, &source);
        draw(&mut second, &source);
        cx.run_until_parked();
        draw(&mut first, &source);
        draw(&mut second, &source);
        let image = decoded_image(cx, &source);
        assert!(has_tiles(&mut first, &image));
        assert!(has_tiles(&mut second, &image));

        evict_in(&mut second, &source);

        assert!(!has_tiles(&mut first, &image));
        assert!(!has_tiles(&mut second, &image));
        assert!(!is_cached(cx, &source));
    }

    #[vespertine::test]
    fn app_001_a4_resource_pending_eviction_removes_entry_and_frees_no_tiles(
        cx: &mut TestAppContext,
    ) {
        assert_cache_backed_eviction(cx, path_source(PNG_FIXTURE), LoadState::Pending);
    }

    #[vespertine::test]
    fn app_001_a4_resource_failed_eviction_removes_entry_and_frees_no_tiles(
        cx: &mut TestAppContext,
    ) {
        assert_cache_backed_eviction(cx, path_source(MISSING_FIXTURE), LoadState::Failed);
    }

    #[vespertine::test]
    fn app_001_a4_embedded_resource_failed_eviction_removes_entry(cx: &mut TestAppContext) {
        let source = ImageSource::Resource(Resource::Embedded("app-001/missing.png".into()));
        assert_cache_backed_eviction(cx, source, LoadState::Failed);
    }

    #[vespertine::test]
    fn app_001_a4_resource_completed_eviction_removes_entry_and_frees_tiles(
        cx: &mut TestAppContext,
    ) {
        assert_cache_backed_eviction(cx, path_source(PNG_FIXTURE), LoadState::Completed);
    }

    #[vespertine::test]
    fn app_001_a4_encoded_image_pending_eviction_removes_entry_and_frees_no_tiles(
        cx: &mut TestAppContext,
    ) {
        assert_cache_backed_eviction(cx, encoded_source(png_bytes()), LoadState::Pending);
    }

    #[vespertine::test]
    fn app_001_a4_encoded_image_failed_eviction_removes_entry_and_frees_no_tiles(
        cx: &mut TestAppContext,
    ) {
        let source = encoded_source(b"not a png".to_vec());
        assert_cache_backed_eviction(cx, source, LoadState::Failed);
    }

    #[vespertine::test]
    fn app_001_a4_encoded_image_completed_eviction_removes_entry_and_frees_tiles(
        cx: &mut TestAppContext,
    ) {
        assert_cache_backed_eviction(cx, encoded_source(png_bytes()), LoadState::Completed);
    }

    #[vespertine::test]
    fn app_001_a4_rendered_image_eviction_frees_tiles_without_touching_the_cache(
        cx: &mut TestAppContext,
    ) {
        let mut first = new_window(cx);
        let mut second = new_window(cx);
        let bystander = path_source(JPEG_FIXTURE);
        drive(&mut first, &bystander, LoadState::Completed);
        let bystander_image = decoded_image(cx, &bystander);

        let image = render_image();
        let source = ImageSource::Render(image.clone());
        draw(&mut first, &source);
        draw(&mut second, &source);
        assert!(has_tiles(&mut first, &image));
        assert!(has_tiles(&mut second, &image));

        evict_in(&mut first, &source);

        assert!(!has_tiles(&mut first, &image));
        assert!(!has_tiles(&mut second, &image));
        assert!(!is_cached(cx, &source));
        assert!(is_cached(cx, &bystander));
        assert!(has_tiles(&mut first, &bystander_image));

        // Rendered sources evict without a window too, and repeatedly.
        draw(&mut second, &source);
        assert!(has_tiles(&mut second, &image));
        cx.update(|cx| source.evict(None, cx));
        assert!(!has_tiles(&mut second, &image));
        cx.update(|cx| source.evict(None, cx));
    }

    #[vespertine::test]
    fn app_001_a4_custom_source_eviction_does_nothing(cx: &mut TestAppContext) {
        let mut window = new_window(cx);
        let image = render_image();
        let calls = Rc::new(Cell::new(0));
        let source = ImageSource::from({
            let image = image.clone();
            let calls = calls.clone();
            move |_: &mut Window, _: &mut App| {
                calls.set(calls.get() + 1);
                Some(Ok(image.clone()))
            }
        });

        draw(&mut window, &source);
        assert!(has_tiles(&mut window, &image));
        let calls_before_eviction = calls.get();
        assert!(calls_before_eviction > 0);

        evict_in(&mut window, &source);
        cx.update(|cx| source.evict(None, cx));

        // The caller-owned loader is not invoked and its image keeps its tiles.
        assert_eq!(calls.get(), calls_before_eviction);
        assert!(has_tiles(&mut window, &image));
        assert!(!is_cached(cx, &source));
    }

    #[vespertine::test]
    fn app_001_a5_repeated_eviction_changes_nothing_after_the_first(cx: &mut TestAppContext) {
        let mut first = new_window(cx);
        let mut second = new_window(cx);
        let source = path_source(PNG_FIXTURE);
        let bystander = encoded_source(png_bytes());
        drive(&mut first, &bystander, LoadState::Completed);
        let bystander_image = decoded_image(cx, &bystander);
        // `second` never draws the evicted image and never draws the bystander.
        drive(&mut first, &source, LoadState::Completed);
        let image = decoded_image(cx, &source);
        assert!(!has_tiles(&mut second, &image));

        evict_in(&mut first, &source);
        let after_first = (
            is_cached(cx, &source),
            has_tiles(&mut first, &image),
            has_tiles(&mut second, &image),
            is_cached(cx, &bystander),
            has_tiles(&mut first, &bystander_image),
        );
        assert_eq!(after_first, (false, false, false, true, true));

        evict_in(&mut first, &source);
        cx.update(|cx| source.evict(None, cx));
        let after_more = (
            is_cached(cx, &source),
            has_tiles(&mut first, &image),
            has_tiles(&mut second, &image),
            is_cached(cx, &bystander),
            has_tiles(&mut first, &bystander_image),
        );
        assert_eq!(after_more, after_first);
    }

    #[vespertine::test]
    fn app_001_a5_evicting_an_uncached_source_succeeds_without_any_window(cx: &mut TestAppContext) {
        let mut window = new_window(cx);
        let bystander = path_source(JPEG_FIXTURE);
        drive(&mut window, &bystander, LoadState::Completed);
        let bystander_image = decoded_image(cx, &bystander);

        for source in [
            path_source(PNG_FIXTURE),
            encoded_source(png_bytes()),
            ImageSource::Resource(Resource::Embedded("app-001/never-requested".into())),
        ] {
            cx.update(|cx| source.evict(None, cx));
            evict_in(&mut window, &source);
            assert!(!is_cached(cx, &source));
        }

        assert!(is_cached(cx, &bystander));
        assert!(has_tiles(&mut window, &bystander_image));
    }

    #[vespertine::test]
    fn app_001_a5_evicting_while_pending_without_a_window_then_completing_is_safe(
        cx: &mut TestAppContext,
    ) {
        let mut window = new_window(cx);
        let source = path_source(PNG_FIXTURE);
        drive(&mut window, &source, LoadState::Pending);
        assert!(is_cached(cx, &source));

        cx.update(|cx| source.evict(None, cx));
        cx.run_until_parked();

        assert!(!is_cached(cx, &source));
        assert!(peek(cx, &source).is_none());
    }

    #[vespertine::test]
    fn app_001_a6_reuse_after_eviction_starts_exactly_one_new_load(cx: &mut TestAppContext) {
        for source in [path_source(PNG_FIXTURE), encoded_source(png_bytes())] {
            let mut window = new_window(cx);
            drive(&mut window, &source, LoadState::Completed);
            let first_image = decoded_image(cx, &source);
            assert!(has_tiles(&mut window, &first_image));

            evict_in(&mut window, &source);
            assert!(!is_cached(cx, &source));
            assert!(!has_tiles(&mut window, &first_image));

            // The next use starts a fresh load: pending first, nothing painted yet.
            draw(&mut window, &source);
            assert!(is_cached(cx, &source));
            assert!(peek(cx, &source).is_none());
            cx.run_until_parked();

            // Every decode creates a new image id, so an unchanged id across further
            // draws shows no additional load was started.
            let second_image = decoded_image(cx, &source);
            assert_ne!(second_image.id, first_image.id);
            for _ in 0..3 {
                draw(&mut window, &source);
                cx.run_until_parked();
                assert_eq!(decoded_image(cx, &source).id, second_image.id);
            }
            assert!(has_tiles(&mut window, &second_image));
            assert!(!has_tiles(&mut window, &first_image));
        }
    }

    #[vespertine::test]
    fn app_001_a6_cache_only_removal_leaves_allocated_tiles_in_place(cx: &mut TestAppContext) {
        for source in [path_source(PNG_FIXTURE), encoded_source(png_bytes())] {
            let mut first = new_window(cx);
            let mut second = new_window(cx);
            drive(&mut first, &source, LoadState::Completed);
            draw(&mut second, &source);
            let image = decoded_image(cx, &source);
            assert!(has_tiles(&mut first, &image));
            assert!(has_tiles(&mut second, &image));

            cx.update(|cx| source.remove_asset(cx));

            assert!(!is_cached(cx, &source));
            assert!(has_tiles(&mut first, &image));
            assert!(has_tiles(&mut second, &image));
        }
    }
}
