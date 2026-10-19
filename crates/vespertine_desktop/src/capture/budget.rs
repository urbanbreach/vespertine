//! Buffer budgets (PLT-012 R6).
//!
//! Every pixel buffer allocated or decoded by the capture service is checked
//! against these budgets first. The checks are pure functions so that nothing
//! is allocated when a budget is violated.

use std::io::Cursor;

use thiserror::Error;

/// Largest accepted width or height in pixels.
pub const MAX_DIMENSION: u64 = 8192;
/// Largest accepted pixel count (`32 x 1024 x 1024`).
pub const MAX_PIXELS: u64 = 32 * 1024 * 1024;
/// Largest accepted RGBA byte size (128 MiB).
pub const MAX_RGBA_BYTES: u64 = 128 * 1024 * 1024;
/// Default cap for the encoded image delivered by the portal (24 MiB).
pub const DEFAULT_ENCODED_IMAGE_CAP: u64 = 24 * 1024 * 1024;

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// A violated buffer budget or malformed image header. The message names the
/// offending dimensions.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BudgetError {
    #[error("image dimensions {width} x {height} include a zero side")]
    ZeroDimension { width: u64, height: u64 },
    #[error("image dimensions {width} x {height} exceed the {MAX_DIMENSION} pixel limit per side")]
    DimensionLimit { width: u64, height: u64 },
    #[error(
        "image dimensions {width} x {height} exceed the budget of {MAX_PIXELS} pixels in total"
    )]
    PixelBudget { width: u64, height: u64 },
    #[error("image dimensions {width} x {height} exceed the budget of {MAX_RGBA_BYTES} RGBA bytes")]
    ByteBudget { width: u64, height: u64 },
    #[error("image dimensions {width} x {height} overflow the buffer size computation")]
    Overflow { width: u64, height: u64 },
    #[error("PNG data is malformed: {0}")]
    MalformedPng(String),
    #[error(
        "decoded PNG frame is {frame_width} x {frame_height} but its header declares {header_width} x {header_height}"
    )]
    HeaderMismatch {
        header_width: u32,
        header_height: u32,
        frame_width: u32,
        frame_height: u32,
    },
}

/// Validates `width x height` against the budgets and returns the RGBA byte
/// size (`pixels * 4`).
///
/// Width and height must each be non-zero and at most 8192; the pixel count
/// (overflow-checked) at most 32 Mi; the RGBA size (overflow-checked) at most
/// 128 MiB.
pub fn validate_dimensions(width: u64, height: u64) -> Result<usize, BudgetError> {
    if width == 0 || height == 0 {
        return Err(BudgetError::ZeroDimension { width, height });
    }
    if width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(BudgetError::DimensionLimit { width, height });
    }
    let pixels = width
        .checked_mul(height)
        .ok_or(BudgetError::Overflow { width, height })?;
    if pixels > MAX_PIXELS {
        return Err(BudgetError::PixelBudget { width, height });
    }
    let bytes = pixels
        .checked_mul(4)
        .ok_or(BudgetError::Overflow { width, height })?;
    if bytes > MAX_RGBA_BYTES {
        return Err(BudgetError::ByteBudget { width, height });
    }
    usize::try_from(bytes).map_err(|_| BudgetError::Overflow { width, height })
}

/// Reads the pixel dimensions from a PNG header without decoding anything:
/// checks the 8-byte signature and the `IHDR` chunk tag, then reads the
/// big-endian width at bytes 16..20 and height at 20..24. Both must be
/// non-zero.
pub fn png_header_dimensions(data: &[u8]) -> Result<(u32, u32), BudgetError> {
    if data.len() < 24 {
        return Err(BudgetError::MalformedPng(format!(
            "{} bytes is too short for a PNG header",
            data.len()
        )));
    }
    if data[..8] != PNG_SIGNATURE {
        return Err(BudgetError::MalformedPng("missing PNG signature".into()));
    }
    if &data[12..16] != b"IHDR" {
        return Err(BudgetError::MalformedPng("first chunk is not IHDR".into()));
    }
    let width = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
    let height = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
    if width == 0 || height == 0 {
        return Err(BudgetError::ZeroDimension {
            width: u64::from(width),
            height: u64::from(height),
        });
    }
    Ok((width, height))
}

/// Fails when a decoded frame's size disagrees with the PNG header.
pub fn ensure_frame_matches_header(
    header: (u32, u32),
    frame: (u32, u32),
) -> Result<(), BudgetError> {
    if header == frame {
        Ok(())
    } else {
        Err(BudgetError::HeaderMismatch {
            header_width: header.0,
            header_height: header.1,
            frame_width: frame.0,
            frame_height: frame.1,
        })
    }
}

/// A decoded image: tightly packed RGBA8 pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedRgba {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Validates PNG `data` from its header, then decodes it to RGBA8.
///
/// The header dimensions pass [`validate_dimensions`] before the decoder is
/// created; the decoder is limited to 128 MiB, its output buffer size must be
/// at most 128 MiB, and the decoded frame must match the header.
pub fn decode_png_rgba(data: &[u8]) -> Result<DecodedRgba, BudgetError> {
    let header = png_header_dimensions(data)?;
    validate_dimensions(u64::from(header.0), u64::from(header.1))?;

    let mut decoder = png::Decoder::new_with_limits(
        Cursor::new(data),
        png::Limits {
            bytes: MAX_RGBA_BYTES as usize,
        },
    );
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder
        .read_info()
        .map_err(|error| BudgetError::MalformedPng(error.to_string()))?;
    let buffer_size = reader
        .output_buffer_size()
        .filter(|size| *size as u64 <= MAX_RGBA_BYTES)
        .ok_or(BudgetError::ByteBudget {
            width: u64::from(header.0),
            height: u64::from(header.1),
        })?;
    let mut buffer = vec![0; buffer_size];
    let frame = reader
        .next_frame(&mut buffer)
        .map_err(|error| BudgetError::MalformedPng(error.to_string()))?;
    ensure_frame_matches_header(header, (frame.width, frame.height))?;
    buffer.truncate(frame.buffer_size());

    let rgba = match frame.color_type {
        png::ColorType::Rgba => buffer,
        png::ColorType::Rgb => buffer
            .chunks_exact(3)
            .flat_map(|px| [px[0], px[1], px[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => buffer
            .chunks_exact(2)
            .flat_map(|px| [px[0], px[0], px[0], px[1]])
            .collect(),
        png::ColorType::Grayscale => buffer
            .iter()
            .flat_map(|&gray| [gray, gray, gray, 255])
            .collect(),
        png::ColorType::Indexed => {
            return Err(BudgetError::MalformedPng(
                "indexed PNG was not expanded by the decoder".into(),
            ));
        }
    };
    Ok(DecodedRgba {
        width: frame.width,
        height: frame.height,
        rgba,
    })
}

/// Encodes tightly packed RGBA8 pixels as a PNG after validating the size.
pub fn encode_png_rgba(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, BudgetError> {
    let expected = validate_dimensions(u64::from(width), u64::from(height))?;
    if rgba.len() != expected {
        return Err(BudgetError::MalformedPng(format!(
            "pixel buffer holds {} bytes but {width} x {height} RGBA needs {expected}",
            rgba.len()
        )));
    }
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|error| BudgetError::MalformedPng(error.to_string()))?;
        writer
            .write_image_data(rgba)
            .map_err(|error| BudgetError::MalformedPng(error.to_string()))?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dimension_budgets_match_the_spec_examples() {
        assert_eq!(validate_dimensions(8192, 4096), Ok(128 * 1024 * 1024));
        assert!(matches!(
            validate_dimensions(8192, 4097),
            Err(BudgetError::PixelBudget { .. })
        ));
        assert!(matches!(
            validate_dimensions(8192, 8192),
            Err(BudgetError::PixelBudget { .. })
        ));
        assert!(matches!(
            validate_dimensions(8193, 1),
            Err(BudgetError::DimensionLimit { .. })
        ));
        assert!(matches!(
            validate_dimensions(0, 10),
            Err(BudgetError::ZeroDimension { .. })
        ));
        assert!(matches!(
            validate_dimensions(10, 0),
            Err(BudgetError::ZeroDimension { .. })
        ));
    }

    #[test]
    fn overflow_sized_values_fail_without_arithmetic_panics() {
        for (w, h) in [
            (u64::MAX, u64::MAX),
            (u64::MAX, 1),
            (1, u64::MAX),
            (u64::from(u32::MAX), u64::from(u32::MAX)),
        ] {
            assert!(validate_dimensions(w, h).is_err(), "{w} x {h}");
        }
    }

    #[test]
    fn violation_messages_name_the_dimensions() {
        let message = validate_dimensions(8192, 4097).unwrap_err().to_string();
        assert!(message.contains("8192 x 4097"), "{message}");
        let message = validate_dimensions(8193, 1).unwrap_err().to_string();
        assert!(message.contains("8193 x 1"), "{message}");
    }

    #[test]
    fn png_header_reader_uses_bytes_16_to_24() {
        let png = encode_png_rgba(3, 2, &[7; 3 * 2 * 4]).unwrap();
        assert_eq!(png_header_dimensions(&png), Ok((3, 2)));
        assert_eq!(&png[16..20], &3u32.to_be_bytes());
        assert_eq!(&png[20..24], &2u32.to_be_bytes());

        assert!(png_header_dimensions(&png[..23]).is_err());
        let mut bad_signature = png.clone();
        bad_signature[1] = b'X';
        assert!(png_header_dimensions(&bad_signature).is_err());
        let mut zero_width = png.clone();
        zero_width[16..20].copy_from_slice(&0u32.to_be_bytes());
        assert!(matches!(
            png_header_dimensions(&zero_width),
            Err(BudgetError::ZeroDimension { .. })
        ));
        let mut zero_height = png;
        zero_height[20..24].copy_from_slice(&0u32.to_be_bytes());
        assert!(matches!(
            png_header_dimensions(&zero_height),
            Err(BudgetError::ZeroDimension { .. })
        ));
    }

    #[test]
    fn png_round_trip_decodes_to_the_same_pixels() {
        let rgba: Vec<u8> = (0..4 * 3 * 4).map(|v| v as u8).collect();
        let png = encode_png_rgba(4, 3, &rgba).unwrap();
        let decoded = decode_png_rgba(&png).unwrap();
        assert_eq!((decoded.width, decoded.height), (4, 3));
        assert_eq!(decoded.rgba, rgba);
    }

    #[test]
    fn oversized_png_header_is_rejected_before_decoding() {
        let mut png = encode_png_rgba(2, 2, &[0; 16]).unwrap();
        png[16..20].copy_from_slice(&9000u32.to_be_bytes());
        png[20..24].copy_from_slice(&1u32.to_be_bytes());
        // The corrupted header also breaks the CRC, so only a pre-decode
        // budget check can produce this specific error.
        assert!(matches!(
            decode_png_rgba(&png),
            Err(BudgetError::DimensionLimit { .. })
        ));
    }

    #[test]
    fn frame_disagreeing_with_header_fails() {
        assert_eq!(ensure_frame_matches_header((4, 3), (4, 3)), Ok(()));
        assert_eq!(
            ensure_frame_matches_header((4, 3), (4, 2)),
            Err(BudgetError::HeaderMismatch {
                header_width: 4,
                header_height: 3,
                frame_width: 4,
                frame_height: 2,
            })
        );
    }

    #[test]
    fn corrupted_header_dimensions_fail_decoding() {
        let mut png = encode_png_rgba(4, 3, &[1; 4 * 3 * 4]).unwrap();
        png[16..20].copy_from_slice(&5u32.to_be_bytes());
        assert!(decode_png_rgba(&png).is_err());
    }

    #[test]
    fn encode_rejects_wrong_buffer_length_and_budget() {
        assert!(encode_png_rgba(2, 2, &[0; 15]).is_err());
        assert!(matches!(
            encode_png_rgba(8193, 1, &[]),
            Err(BudgetError::DimensionLimit { .. })
        ));
    }
}
