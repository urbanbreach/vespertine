//! Bounded image preview and zoomable viewer (CMP-020).
//!
//! * [`viewer`]: the zoom/pan geometry and the viewport element ([`ImageView`]).
//! * [`decode`] and [`svg`]: bounded raster decoding, SVG sanitizing and raster sizing.
//! * [`cache`]: the media cache with budgets, LRU eviction, retry ladder and renderer release.
//! * [`preview`]: the inline preview element.
//! * [`lightbox`]: the shared full-view overlay.

pub mod cache;
pub mod decode;
pub mod lightbox;
pub mod preview;
pub mod svg;
pub mod viewer;

pub use cache::{MediaCache, MediaClass, MediaKey, MediaStatus, ValidationPolicy, release};
pub use decode::{
    DecodedMedia, MediaError, decode_generated_image, decode_icon, decode_image,
    decode_image_with_cap, decode_thumbnail, raster_retained_bytes,
};
pub use lightbox::{Lightbox, PreviewTarget, lightbox};
pub use preview::{PreviewMedia, preview_element};
pub use viewer::{ImageClickHandler, ImageView};
