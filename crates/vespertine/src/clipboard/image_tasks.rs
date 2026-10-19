//! Asynchronous clipboard image helpers.
//!
//! Producing a clipboard image from pixels (RGBA to encoded PNG) and decoding clipboard image
//! bytes for display run on the background executor, never on the UI thread. The format is
//! preserved: a JPEG stays a JPEG and an SVG stays an SVG; encoding always produces PNG.
//! For formats with alpha, straight alpha is preserved exactly; formats without alpha decode
//! as fully opaque.
//!
//! Both helpers return tasks that are cancelled by dropping them. To deliver a result to a
//! view, use [`deliver_when_current`] with a [`RequestGeneration`]: delivery to a view that was
//! released, or for a request that was superseded by a newer one, does nothing.

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use anyhow::{Result, bail};
use image::{ExtendedColorType, ImageEncoder, codecs::png::PngEncoder};

use crate::{App, Context, Image, ImageFormat, RenderImage, Task};

/// Encodes straight-alpha RGBA8 pixels (`width * height * 4` bytes, row-major, top row first)
/// as a PNG image. Blocking; see [`encode_rgba_png_async`].
pub fn encode_rgba_png(width: u32, height: u32, rgba: &[u8]) -> Result<Image> {
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4));
    if expected != Some(rgba.len()) {
        bail!(
            "expected {width} x {height} RGBA pixels ({expected:?} bytes), got {} bytes",
            rgba.len()
        );
    }
    let mut bytes = Vec::new();
    PngEncoder::new(&mut bytes).write_image(rgba, width, height, ExtendedColorType::Rgba8)?;
    Ok(Image::from_bytes(ImageFormat::Png, bytes))
}

/// Encodes RGBA8 pixels as a PNG [`Image`] on the background executor.
///
/// The task is cancelled by dropping it.
pub fn encode_rgba_png_async(
    cx: &App,
    width: u32,
    height: u32,
    rgba: Vec<u8>,
) -> Task<Result<Image>> {
    cx.background_executor()
        .spawn(async move { encode_rgba_png(width, height, &rgba) })
}

/// Decodes a clipboard image for display on the background executor.
///
/// Raster formats keep their straight alpha (formats without alpha decode as fully opaque),
/// SVG is rasterized by the application's SVG renderer, and animated GIFs keep their frames.
/// The task is cancelled by dropping it.
pub fn decode_image_async(cx: &App, image: Image) -> Task<Result<Arc<RenderImage>>> {
    let renderer = cx.svg_renderer();
    cx.background_executor()
        .spawn(async move { image.to_image_data(renderer) })
}

/// Counts the requests a view makes so that only the newest one is delivered.
///
/// Call [`begin`](Self::begin) for every request and hand the returned [`RequestToken`] to
/// [`deliver_when_current`]. Clones share one counter.
#[derive(Clone, Debug, Default)]
pub struct RequestGeneration(Arc<AtomicU64>);

impl RequestGeneration {
    /// Creates a counter with no requests yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts a request, superseding every earlier one.
    pub fn begin(&self) -> RequestToken {
        let id = self.0.fetch_add(1, Ordering::SeqCst) + 1;
        RequestToken {
            id,
            current: self.0.clone(),
        }
    }

    /// Supersedes all outstanding requests without starting a new one.
    pub fn cancel_all(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// Identifies one request of a [`RequestGeneration`].
#[derive(Clone, Debug)]
pub struct RequestToken {
    id: u64,
    current: Arc<AtomicU64>,
}

impl RequestToken {
    /// Whether no newer request was started since this one.
    pub fn is_current(&self) -> bool {
        self.current.load(Ordering::SeqCst) == self.id
    }
}

/// Delivers the result of `task` to the view of `cx` through `on_ready`, unless the view was
/// released or `token` was superseded in the meantime, in which case nothing happens.
///
/// Dropping the returned task cancels the work and the delivery.
pub fn deliver_when_current<T: 'static, R: 'static>(
    cx: &mut Context<T>,
    token: RequestToken,
    task: Task<R>,
    on_ready: impl FnOnce(&mut T, R, &mut Context<T>) + 'static,
) -> Task<()> {
    cx.spawn(async move |this, cx| {
        let result = task.await;
        if !token.is_current() {
            return;
        }
        this.update(cx, |this, cx| {
            if token.is_current() {
                on_ready(this, result, cx);
            }
        })
        .ok();
    })
}

/// Decodes `image` for display and delivers it to the view of `cx` unless the view was
/// released or `generation` has a newer request by then. See [`deliver_when_current`].
pub fn decode_image_for<T: 'static>(
    cx: &mut Context<T>,
    generation: &RequestGeneration,
    image: Image,
    on_ready: impl FnOnce(&mut T, Result<Arc<RenderImage>>, &mut Context<T>) + 'static,
) -> Task<()> {
    let token = generation.begin();
    let decoding = decode_image_async(cx, image);
    deliver_when_current(cx, token, decoding, on_ready)
}

/// Encodes `rgba` pixels as PNG and delivers the image to the view of `cx` unless the view was
/// released or `generation` has a newer request by then. See [`deliver_when_current`].
pub fn encode_rgba_png_for<T: 'static>(
    cx: &mut Context<T>,
    generation: &RequestGeneration,
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    on_ready: impl FnOnce(&mut T, Result<Image>, &mut Context<T>) + 'static,
) -> Task<()> {
    let token = generation.begin();
    let encoding = encode_rgba_png_async(cx, width, height, rgba);
    deliver_when_current(cx, token, encoding, on_ready)
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use super::*;
    use crate::{AppContext as _, Entity, TestAppContext};

    struct Viewer {
        decoded: Vec<Result<Arc<RenderImage>>>,
    }

    fn partially_transparent_rgba() -> (u32, u32, Vec<u8>) {
        let (width, height) = (4, 3);
        let mut pixels = Vec::new();
        for index in 0..(width * height) as u8 {
            pixels.extend([index * 20, 255 - index * 10, index * 3, index * 23]);
        }
        (width, height, pixels)
    }

    fn straight_rgba(image: &RenderImage) -> Vec<u8> {
        let mut bytes = image.as_bytes(0).unwrap().to_vec();
        for pixel in bytes.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        bytes
    }

    #[test]
    fn png_alpha_round_trips_exactly_and_format_is_png() {
        let (width, height, pixels) = partially_transparent_rgba();
        let image = encode_rgba_png(width, height, &pixels).unwrap();
        assert_eq!(image.format, ImageFormat::Png);
        assert_eq!(&image.bytes[..8], b"\x89PNG\r\n\x1a\n");
        let decoded = image
            .to_image_data(crate::SvgRenderer::new(Arc::new(())))
            .unwrap();
        assert_eq!(straight_rgba(&decoded), pixels);
    }

    #[test]
    fn mismatched_pixel_buffer_is_an_error() {
        assert!(encode_rgba_png(2, 2, &[0; 15]).is_err());
    }

    #[crate::test]
    async fn async_encode_and_decode_run_off_the_ui_thread(cx: &mut TestAppContext) {
        let (width, height, pixels) = partially_transparent_rgba();
        let image = cx
            .update(|cx| encode_rgba_png_async(cx, width, height, pixels.clone()))
            .await
            .unwrap();
        let decoded = cx
            .update(|cx| decode_image_async(cx, image.clone()))
            .await
            .unwrap();
        assert_eq!(straight_rgba(&decoded), pixels);
    }

    #[crate::test]
    async fn jpeg_stays_jpeg_and_decodes_opaque(cx: &mut TestAppContext) {
        let image = Image::from_bytes(
            ImageFormat::Jpeg,
            include_bytes!("../../examples/image/exif-orientation-rotate-180.jpg").to_vec(),
        );
        assert_eq!(image.format, ImageFormat::Jpeg);
        let decoded = cx.update(|cx| decode_image_async(cx, image)).await.unwrap();
        assert!(
            straight_rgba(&decoded)
                .chunks_exact(4)
                .all(|pixel| pixel[3] == 255)
        );
    }

    fn png_image() -> Image {
        let (width, height, pixels) = partially_transparent_rgba();
        encode_rgba_png(width, height, &pixels).unwrap()
    }

    #[crate::test]
    fn decode_for_released_view_has_no_effect(cx: &mut TestAppContext) {
        let delivered = Rc::new(Cell::new(false));
        let viewer: Entity<Viewer> = cx.new(|_| Viewer {
            decoded: Vec::new(),
        });
        let generation = RequestGeneration::new();
        let task = viewer.update(cx, |_, cx| {
            let delivered = delivered.clone();
            decode_image_for(cx, &generation, png_image(), move |this, result, _| {
                delivered.set(true);
                this.decoded.push(result);
            })
        });
        // The view is released before the decode completes.
        drop(viewer);
        cx.run_until_parked();
        task.detach();
        cx.run_until_parked();
        assert!(!delivered.get());
    }

    #[crate::test]
    fn superseded_request_is_discarded(cx: &mut TestAppContext) {
        let viewer: Entity<Viewer> = cx.new(|_| Viewer {
            decoded: Vec::new(),
        });
        let generation = RequestGeneration::new();
        let first = viewer.update(cx, |_, cx| {
            decode_image_for(cx, &generation, png_image(), |this, result, _| {
                this.decoded.push(result)
            })
        });
        let second = viewer.update(cx, |_, cx| {
            decode_image_for(cx, &generation, png_image(), |this, result, _| {
                this.decoded.push(result)
            })
        });
        first.detach();
        second.detach();
        cx.run_until_parked();
        viewer.read_with(cx, |viewer, _| assert_eq!(viewer.decoded.len(), 1));
    }

    #[crate::test]
    fn dropping_the_returned_task_cancels_delivery(cx: &mut TestAppContext) {
        let viewer: Entity<Viewer> = cx.new(|_| Viewer {
            decoded: Vec::new(),
        });
        let generation = RequestGeneration::new();
        let task = viewer.update(cx, |_, cx| {
            decode_image_for(cx, &generation, png_image(), |this, result, _| {
                this.decoded.push(result)
            })
        });
        drop(task);
        cx.run_until_parked();
        viewer.read_with(cx, |viewer, _| assert!(viewer.decoded.is_empty()));
    }
}
