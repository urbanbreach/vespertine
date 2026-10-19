//! Shared fixtures of the CMP-020 media tests.

#![allow(dead_code)]

use image::{ExtendedColorType, ImageEncoder, Rgba, RgbaImage, codecs::png::PngEncoder};
use std::sync::Arc;
use vespertine::{RenderImage, SvgRenderer};

/// A solid BGRA test image.
pub fn solid_image(width: u32, height: u32) -> Arc<RenderImage> {
    let bytes = vec![0x80; (width * height * 4) as usize];
    Arc::new(RenderImage::from_bgra(width, height, bytes).unwrap())
}

/// A renderer with no bundled assets (system fonts only).
pub fn renderer() -> SvgRenderer {
    SvgRenderer::new(Arc::new(()))
}

/// A PNG of a gradient of the given size.
pub fn png(width: u32, height: u32) -> Vec<u8> {
    let image = RgbaImage::from_fn(width, height, |x, y| {
        Rgba([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8, 255])
    });
    let mut out = Vec::new();
    PngEncoder::new(&mut out)
        .write_image(image.as_raw(), width, height, ExtendedColorType::Rgba8)
        .unwrap();
    out
}

/// A JPEG of a gradient of the given size.
pub fn jpeg(width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 0])
    });
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut out)
        .write_image(image.as_raw(), width, height, ExtendedColorType::Rgb8)
        .unwrap();
    out
}

/// An animated GIF with three frames.
pub fn animated_gif(width: u16, height: u16) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = image::codecs::gif::GifEncoder::new(&mut out);
        for shade in [0u8, 100, 200] {
            let frame =
                RgbaImage::from_pixel(width as u32, height as u32, Rgba([shade, 0, 0, 255]));
            encoder.encode_frame(image::Frame::new(frame)).unwrap();
        }
    }
    out
}
