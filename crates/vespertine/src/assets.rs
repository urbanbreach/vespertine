use crate::{AtlasKey, DevicePixels, Pixels, PlatformAtlas, Result, SharedString, Size, size};
use smallvec::SmallVec;

use image::{Delay, Frame};
use std::{
    borrow::Cow,
    fmt,
    hash::Hash,
    sync::atomic::{AtomicUsize, Ordering::SeqCst},
};

/// A source of assets for this app to use.
pub trait AssetSource: 'static + Send + Sync {
    /// Load the given asset from the source path.
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>>;

    /// List the assets at the given path.
    fn list(&self, path: &str) -> Result<Vec<SharedString>>;
}

impl AssetSource for () {
    fn load(&self, _path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(None)
    }

    fn list(&self, _path: &str) -> Result<Vec<SharedString>> {
        Ok(vec![])
    }
}

/// A unique identifier for the image cache
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ImageId(pub usize);

#[derive(PartialEq, Eq, Hash, Clone)]
#[expect(missing_docs)]
pub struct RenderImageParams {
    pub image_id: ImageId,
    pub frame_index: usize,
}

/// A cached and processed image, in BGRA format
pub struct RenderImage {
    /// The ID associated with this image
    pub id: ImageId,
    /// The scale factor of this image on render.
    pub(crate) scale_factor: f32,
    data: SmallVec<[Frame; 1]>,
}

impl PartialEq for RenderImage {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for RenderImage {}

impl RenderImage {
    /// Create a new image from the given data.
    pub fn new(data: impl Into<SmallVec<[Frame; 1]>>) -> Self {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

        Self {
            id: ImageId(NEXT_ID.fetch_add(1, SeqCst)),
            scale_factor: 1.0,
            data: data.into(),
        }
    }

    /// Creates a new image object with new pixel `data` that keeps this image's
    /// identity (and scale factor). Use it with [`Window::update_image`] to give
    /// an image already referenced by a rendered scene new pixels; the atlas tiles
    /// are keyed by the identity, so the update lands in the existing allocation
    /// when the frame sizes are unchanged. [`RenderImage::new`] always creates a
    /// fresh identity, even for equal bytes.
    ///
    /// [`Window::update_image`]: crate::Window::update_image
    pub fn with_updated_frames(&self, data: impl Into<SmallVec<[Frame; 1]>>) -> Self {
        Self {
            id: self.id,
            scale_factor: self.scale_factor,
            data: data.into(),
        }
    }

    /// Convert this image into a byte slice.
    pub fn as_bytes(&self, frame_index: usize) -> Option<&[u8]> {
        self.data
            .get(frame_index)
            .map(|frame| frame.buffer().as_raw().as_slice())
    }

    /// Get the size of this image, in pixels.
    pub fn size(&self, frame_index: usize) -> Size<DevicePixels> {
        self.data
            .get(frame_index)
            .map(|frame| {
                let (width, height) = frame.buffer().dimensions();
                size(width.into(), height.into())
            })
            .unwrap_or_default()
    }

    /// Get the size of this image, in pixels for display, adjusted for the scale factor.
    pub(crate) fn render_size(&self, frame_index: usize) -> Size<Pixels> {
        self.size(frame_index)
            .map(|v| (v.0 as f32 / self.scale_factor).into())
    }

    /// Get the delay of this frame from the previous
    pub fn delay(&self, frame_index: usize) -> Delay {
        self.data
            .get(frame_index)
            .map(|frame| frame.delay())
            .unwrap_or(Delay::from_numer_denom_ms(100, 1))
    }

    /// Get the number of frames for this image.
    pub fn frame_count(&self) -> usize {
        self.data.len()
    }
}

/// Updates every frame of an image in `atlas` under its existing per-frame key,
/// reporting whether the whole image was retained: `true` only when the image has
/// at least one frame and every frame had a tile that equals the tile the update
/// returned. A missing tile, a size change, a backend that reallocated, or zero
/// frames yield `false`. Errors from `frame` or the atlas propagate immediately.
pub(crate) fn update_image_frames<'a>(
    atlas: &dyn PlatformAtlas,
    image_id: ImageId,
    frame_count: usize,
    mut frame: impl FnMut(usize) -> Result<(Size<DevicePixels>, &'a [u8])>,
) -> Result<bool> {
    let mut retained = frame_count > 0;
    for frame_index in 0..frame_count {
        let key: AtlasKey = RenderImageParams {
            image_id,
            frame_index,
        }
        .into();
        let (size, bytes) = frame(frame_index)?;
        // A builder that yields nothing makes this a lookup that never inserts.
        let previous = atlas.get_or_insert_with(key.clone(), &mut || Ok(None))?;
        let updated = atlas.update(key, size, bytes)?;
        retained &= previous == Some(updated);
    }
    Ok(retained)
}

impl fmt::Debug for RenderImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImageData")
            .field("id", &self.id)
            .field("size", &self.data.first().map(|f| f.buffer().dimensions()))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smallvec::SmallVec;

    #[test]
    fn empty_render_image_does_not_panic() {
        let image = RenderImage::new(SmallVec::new());
        assert_eq!(image.frame_count(), 0);
        assert_eq!(image.size(0), Size::default());
        assert_eq!(image.as_bytes(0), None);
        assert_eq!(image.render_size(0), Size::default());
        assert_eq!(image.delay(0), Delay::from_numer_denom_ms(100, 1));
        let _ = format!("{image:?}");
    }

    fn frame(width: u32, height: u32) -> Frame {
        Frame::new(image::RgbaImage::new(width, height))
    }

    fn key(image: &RenderImage, frame_index: usize) -> AtlasKey {
        RenderImageParams {
            image_id: image.id,
            frame_index,
        }
        .into()
    }

    fn update(atlas: &crate::HeadlessAtlas, image: &RenderImage) -> Result<bool> {
        update_image_frames(atlas, image.id, image.frame_count(), |frame_index| {
            let bytes = image
                .as_bytes(frame_index)
                .ok_or_else(|| anyhow::anyhow!("image has no frame {frame_index}"))?;
            Ok((image.size(frame_index), bytes))
        })
    }

    #[test]
    fn update_image_frames_requires_every_frame_to_be_retained() {
        let atlas = crate::HeadlessAtlas::default();
        let image = RenderImage::new(SmallVec::from_vec(vec![frame(2, 2), frame(3, 3)]));

        // No prior tile for any frame: inserted, but not retained.
        assert!(!update(&atlas, &image).unwrap());
        assert!(update(&atlas, &image).unwrap());

        // One frame loses its tile: the others are retained, the whole image is not.
        atlas.remove(&key(&image, 1));
        assert!(!update(&atlas, &image).unwrap());
        assert!(update(&atlas, &image).unwrap());

        // A size change in any frame is never retained.
        let resized = image.with_updated_frames(SmallVec::from_vec(vec![frame(2, 2), frame(4, 4)]));
        assert_eq!(resized.id, image.id);
        assert!(!update(&atlas, &resized).unwrap());
        assert!(update(&atlas, &resized).unwrap());
    }

    #[test]
    fn update_image_frames_is_false_for_zero_frames() {
        let atlas = crate::HeadlessAtlas::default();
        let image = RenderImage::new(SmallVec::new());
        assert!(!update(&atlas, &image).unwrap());
    }

    #[test]
    fn update_image_frames_propagates_invalid_frame_access() {
        let atlas = crate::HeadlessAtlas::default();
        let image = RenderImage::new(SmallVec::from_vec(vec![frame(2, 2)]));

        // Asking for a frame the image does not have is an error, not `false`.
        let error = update_image_frames(&atlas, image.id, 2, |frame_index| {
            let bytes = image
                .as_bytes(frame_index)
                .ok_or_else(|| anyhow::anyhow!("image has no frame {frame_index}"))?;
            Ok((image.size(frame_index), bytes))
        })
        .unwrap_err();
        assert!(error.to_string().contains("no frame 1"));

        // Bytes that do not match the reported size are a backend error.
        let short = [0u8; 3];
        update_image_frames(&atlas, image.id, 1, |_| Ok((image.size(0), &short[..]))).unwrap_err();
    }
}
