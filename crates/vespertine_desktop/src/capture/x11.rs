//! X server boundary of the native capture path (PLT-012 R3).
//!
//! [`X11Server`] is the injectable boundary (active window, process id,
//! properties, geometry, `GetImage`). The pure helpers here turn raw property
//! bytes and raw images into the values the service uses.

use thiserror::Error;

use super::budget::{BudgetError, validate_dimensions};

pub type WindowId = u32;

/// A failed X server request.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message}")]
pub struct X11Error {
    pub message: String,
    /// The server answered `BadWindow` or `BadDrawable`: the window id is stale.
    pub bad_window: bool,
}

impl X11Error {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            bad_window: false,
        }
    }

    pub fn bad_window(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            bad_window: true,
        }
    }
}

/// Window width and height as reported by the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowGeometry {
    pub width: u32,
    pub height: u32,
}

/// The title properties of a window.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TitleProperties {
    /// `_NET_WM_NAME` (UTF-8), when present.
    pub net_wm_name: Option<Vec<u8>>,
    /// `WM_NAME`, when present.
    pub wm_name: Option<Vec<u8>>,
}

/// A `GetImage` reply in ZPixmap format together with the pixel layout of the
/// window's visual.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawImage {
    pub width: u32,
    pub height: u32,
    pub bits_per_pixel: u32,
    /// Bytes per scanline, including padding.
    pub bytes_per_line: usize,
    /// Whether multi-byte pixels are stored most significant byte first.
    pub msb_first: bool,
    pub red_mask: u32,
    pub green_mask: u32,
    pub blue_mask: u32,
    pub data: Vec<u8>,
}

/// Read access to the X server needed by the capture service. Calls are
/// blocking.
pub trait X11Server: Send + Sync {
    /// `_NET_ACTIVE_WINDOW` on the root; `None` when absent or zero.
    fn active_window(&self) -> Result<Option<WindowId>, X11Error>;
    /// `_NET_WM_PID`; `None` when absent.
    fn window_pid(&self, window: WindowId) -> Result<Option<u32>, X11Error>;
    fn window_geometry(&self, window: WindowId) -> Result<WindowGeometry, X11Error>;
    fn window_title(&self, window: WindowId) -> Result<TitleProperties, X11Error>;
    /// Raw `WM_CLASS` bytes.
    fn window_class(&self, window: WindowId) -> Result<Option<Vec<u8>>, X11Error>;
    /// Raw `_NET_WM_ICON` cardinals.
    fn window_icon(&self, window: WindowId) -> Result<Option<Vec<u32>>, X11Error>;
    /// `_NET_CLIENT_LIST` on the root.
    fn client_list(&self) -> Result<Vec<WindowId>, X11Error>;
    /// Reads the window contents at (0, 0) with the given geometry.
    fn get_image(&self, window: WindowId, width: u32, height: u32) -> Result<RawImage, X11Error>;
}

/// The window title: `_NET_WM_NAME` when present, else `WM_NAME`; it must be
/// valid UTF-8 and not blank. The title is returned exactly as stored.
pub fn window_title_text(properties: &TitleProperties) -> Option<String> {
    let raw = properties
        .net_wm_name
        .as_ref()
        .or(properties.wm_name.as_ref())?;
    let text = std::str::from_utf8(raw).ok()?;
    if text.trim().is_empty() {
        None
    } else {
        Some(text.to_owned())
    }
}

/// The window class: the last non-empty NUL-separated part of `WM_CLASS`.
pub fn window_class_name(raw: &[u8]) -> Option<String> {
    raw.split(|byte| *byte == 0)
        .rev()
        .find(|part| !part.is_empty())
        .and_then(|part| std::str::from_utf8(part).ok())
        .map(str::to_owned)
}

/// An RGBA icon candidate parsed from `_NET_WM_ICON`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconCandidate {
    pub width: u32,
    pub height: u32,
    /// Tightly packed RGBA.
    pub rgba: Vec<u8>,
}

/// Largest icon side considered.
pub const MAX_ICON_SIDE: u32 = 512;
const PREFERRED_ICON_SIDE: u32 = 64;

/// Selects the `_NET_WM_ICON` entry closest to 64 x 64 (smallest
/// `|width-64| + |height-64|`) among entries with width and height at most 512
/// and at most 512 x 512 pixels, converting ARGB to RGBA.
pub fn select_icon(cardinals: &[u32]) -> Option<IconCandidate> {
    let mut best: Option<(u32, usize, u32, u32)> = None;
    let mut offset = 0usize;
    while offset + 2 <= cardinals.len() {
        let width = cardinals[offset];
        let height = cardinals[offset + 1];
        let pixels = (width as usize).checked_mul(height as usize)?;
        let end = (offset + 2).checked_add(pixels)?;
        if end > cardinals.len() {
            break;
        }
        if width > 0 && height > 0 && width <= MAX_ICON_SIDE && height <= MAX_ICON_SIDE {
            let distance =
                width.abs_diff(PREFERRED_ICON_SIDE) + height.abs_diff(PREFERRED_ICON_SIDE);
            if best.is_none_or(|(best_distance, ..)| distance < best_distance) {
                best = Some((distance, offset + 2, width, height));
            }
        }
        offset = end;
    }
    let (_, start, width, height) = best?;
    let pixels = (width * height) as usize;
    let mut rgba = Vec::with_capacity(pixels * 4);
    for argb in &cardinals[start..start + pixels] {
        let [a, r, g, b] = argb.to_be_bytes();
        rgba.extend_from_slice(&[r, g, b, a]);
    }
    Some(IconCandidate {
        width,
        height,
        rgba,
    })
}

/// Converts a ZPixmap image to RGBA with alpha 255 using the visual's
/// channel masks: each channel is scaled to 16 bits and the top 8 bits are
/// kept. The dimensions are validated first, so no buffer is allocated for an
/// over-budget image.
pub fn raw_image_to_rgba(image: &RawImage) -> Result<Vec<u8>, String> {
    let rgba_len = validate_dimensions(u64::from(image.width), u64::from(image.height))
        .map_err(|error: BudgetError| error.to_string())?;
    let bytes_per_pixel = match image.bits_per_pixel {
        16 => 2usize,
        24 => 3,
        32 => 4,
        other => return Err(format!("unsupported pixel depth of {other} bits per pixel")),
    };
    let width = image.width as usize;
    let height = image.height as usize;
    if image.bytes_per_line < width * bytes_per_pixel {
        return Err(format!(
            "scanline of {} bytes is too short for {} pixels",
            image.bytes_per_line, image.width
        ));
    }
    let needed = image
        .bytes_per_line
        .checked_mul(height)
        .ok_or_else(|| "image data size overflows".to_string())?;
    if image.data.len() < needed {
        return Err(format!(
            "image data holds {} bytes but {needed} are required",
            image.data.len()
        ));
    }
    let channels = [
        ChannelScale::new(image.red_mask)?,
        ChannelScale::new(image.green_mask)?,
        ChannelScale::new(image.blue_mask)?,
    ];

    let mut rgba = Vec::with_capacity(rgba_len);
    for row in 0..height {
        let line = &image.data[row * image.bytes_per_line..][..width * bytes_per_pixel];
        for pixel_bytes in line.chunks_exact(bytes_per_pixel) {
            let mut pixel = 0u32;
            if image.msb_first {
                for byte in pixel_bytes {
                    pixel = (pixel << 8) | u32::from(*byte);
                }
            } else {
                for byte in pixel_bytes.iter().rev() {
                    pixel = (pixel << 8) | u32::from(*byte);
                }
            }
            rgba.extend_from_slice(&[
                channels[0].top8(pixel),
                channels[1].top8(pixel),
                channels[2].top8(pixel),
                255,
            ]);
        }
    }
    Ok(rgba)
}

#[derive(Clone, Copy)]
struct ChannelScale {
    mask: u32,
    shift: u32,
    max: u64,
}

impl ChannelScale {
    fn new(mask: u32) -> Result<Self, String> {
        if mask == 0 {
            return Err("the window's visual has an empty channel mask".into());
        }
        let shift = mask.trailing_zeros();
        Ok(Self {
            mask,
            shift,
            max: u64::from(mask >> shift),
        })
    }

    fn top8(self, pixel: u32) -> u8 {
        let value = u64::from((pixel & self.mask) >> self.shift);
        let sixteen = value * 0xffff / self.max;
        (sixteen >> 8) as u8
    }
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub use real::X11rbServer;

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod real {
    use x11rb::{
        connection::Connection as _,
        errors::ReplyError,
        protocol::{
            ErrorKind,
            xproto::{
                Atom, AtomEnum, ConnectionExt as _, ImageFormat, ImageOrder, Visualtype, Window,
            },
        },
        rust_connection::RustConnection,
    };

    use super::*;

    /// Upper bound for one property read, in 32-bit units.
    const MAX_PROPERTY_UNITS: u32 = 1 << 20;

    /// [`X11Server`] over a native X11 connection.
    pub struct X11rbServer {
        connection: RustConnection,
        root: Window,
        net_active_window: Atom,
        net_wm_pid: Atom,
        net_wm_name: Atom,
        utf8_string: Atom,
        net_client_list: Atom,
        net_wm_icon: Atom,
    }

    fn error(error: impl std::fmt::Display) -> X11Error {
        X11Error::new(error.to_string())
    }

    fn reply_error(error: ReplyError) -> X11Error {
        match &error {
            ReplyError::X11Error(x11)
                if matches!(x11.error_kind, ErrorKind::Window | ErrorKind::Drawable) =>
            {
                X11Error::bad_window(error.to_string())
            }
            _ => X11Error::new(error.to_string()),
        }
    }

    impl X11rbServer {
        /// Connects to the display named by `DISPLAY`.
        pub fn connect() -> Result<Self, X11Error> {
            let (connection, screen) = RustConnection::connect(None).map_err(error)?;
            let root = connection.setup().roots[screen].root;
            let intern = |name: &str| -> Result<Atom, X11Error> {
                Ok(connection
                    .intern_atom(false, name.as_bytes())
                    .map_err(error)?
                    .reply()
                    .map_err(reply_error)?
                    .atom)
            };
            Ok(Self {
                root,
                net_active_window: intern("_NET_ACTIVE_WINDOW")?,
                net_wm_pid: intern("_NET_WM_PID")?,
                net_wm_name: intern("_NET_WM_NAME")?,
                utf8_string: intern("UTF8_STRING")?,
                net_client_list: intern("_NET_CLIENT_LIST")?,
                net_wm_icon: intern("_NET_WM_ICON")?,
                connection,
            })
        }

        fn property(
            &self,
            window: Window,
            property: Atom,
            property_type: impl Into<Atom>,
        ) -> Result<Option<(Vec<u8>, u8)>, X11Error> {
            let reply = self
                .connection
                .get_property(
                    false,
                    window,
                    property,
                    property_type,
                    0,
                    MAX_PROPERTY_UNITS,
                )
                .map_err(error)?
                .reply()
                .map_err(reply_error)?;
            if reply.type_ == x11rb::NONE || reply.value.is_empty() {
                Ok(None)
            } else {
                Ok(Some((reply.value, reply.format)))
            }
        }

        fn cardinals(
            &self,
            window: Window,
            property: Atom,
            property_type: impl Into<Atom>,
        ) -> Result<Option<Vec<u32>>, X11Error> {
            Ok(self
                .property(window, property, property_type)?
                .filter(|(_, format)| *format == 32)
                .map(|(value, _)| {
                    value
                        .chunks_exact(4)
                        .map(|chunk| u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                        .collect()
                }))
        }

        fn find_visual(&self, visual_id: u32) -> Option<Visualtype> {
            self.connection
                .setup()
                .roots
                .iter()
                .flat_map(|screen| screen.allowed_depths.iter())
                .flat_map(|depth| depth.visuals.iter())
                .find(|visual| visual.visual_id == visual_id)
                .copied()
        }
    }

    impl X11Server for X11rbServer {
        fn active_window(&self) -> Result<Option<WindowId>, X11Error> {
            Ok(self
                .cardinals(self.root, self.net_active_window, AtomEnum::WINDOW)?
                .and_then(|values| values.first().copied())
                .filter(|window| *window != 0))
        }

        fn window_pid(&self, window: WindowId) -> Result<Option<u32>, X11Error> {
            Ok(self
                .cardinals(window, self.net_wm_pid, AtomEnum::CARDINAL)?
                .and_then(|values| values.first().copied()))
        }

        fn window_geometry(&self, window: WindowId) -> Result<WindowGeometry, X11Error> {
            let reply = self
                .connection
                .get_geometry(window)
                .map_err(error)?
                .reply()
                .map_err(reply_error)?;
            Ok(WindowGeometry {
                width: u32::from(reply.width),
                height: u32::from(reply.height),
            })
        }

        fn window_title(&self, window: WindowId) -> Result<TitleProperties, X11Error> {
            Ok(TitleProperties {
                net_wm_name: self
                    .property(window, self.net_wm_name, self.utf8_string)?
                    .map(|(value, _)| value),
                wm_name: self
                    .property(window, AtomEnum::WM_NAME.into(), AtomEnum::ANY)?
                    .map(|(value, _)| value),
            })
        }

        fn window_class(&self, window: WindowId) -> Result<Option<Vec<u8>>, X11Error> {
            Ok(self
                .property(window, AtomEnum::WM_CLASS.into(), AtomEnum::STRING)?
                .map(|(value, _)| value))
        }

        fn window_icon(&self, window: WindowId) -> Result<Option<Vec<u32>>, X11Error> {
            self.cardinals(window, self.net_wm_icon, AtomEnum::CARDINAL)
        }

        fn client_list(&self) -> Result<Vec<WindowId>, X11Error> {
            Ok(self
                .cardinals(self.root, self.net_client_list, AtomEnum::WINDOW)?
                .unwrap_or_default())
        }

        fn get_image(
            &self,
            window: WindowId,
            width: u32,
            height: u32,
        ) -> Result<RawImage, X11Error> {
            let width16 = u16::try_from(width).map_err(error)?;
            let height16 = u16::try_from(height).map_err(error)?;
            let reply = self
                .connection
                .get_image(ImageFormat::Z_PIXMAP, window, 0, 0, width16, height16, !0)
                .map_err(error)?
                .reply()
                .map_err(reply_error)?;
            let setup = self.connection.setup();
            let format = setup
                .pixmap_formats
                .iter()
                .find(|format| format.depth == reply.depth)
                .ok_or_else(|| error(format!("no pixmap format for depth {}", reply.depth)))?;
            let visual = self
                .find_visual(reply.visual)
                .ok_or_else(|| error(format!("unknown visual {:#x}", reply.visual)))?;
            let bits_per_pixel = u32::from(format.bits_per_pixel);
            let pad = u32::from(format.scanline_pad);
            let bytes_per_line = ((width * bits_per_pixel).div_ceil(pad) * pad / 8) as usize;
            Ok(RawImage {
                width,
                height,
                bits_per_pixel,
                bytes_per_line,
                msb_first: setup.image_byte_order == ImageOrder::MSB_FIRST,
                red_mask: visual.red_mask,
                green_mask: visual.green_mask,
                blue_mask: visual.blue_mask,
                data: reply.data,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn titles(net: Option<&[u8]>, plain: Option<&[u8]>) -> TitleProperties {
        TitleProperties {
            net_wm_name: net.map(<[u8]>::to_vec),
            wm_name: plain.map(<[u8]>::to_vec),
        }
    }

    #[test]
    fn title_prefers_net_wm_name_and_rejects_invalid_or_blank() {
        assert_eq!(
            window_title_text(&titles(Some(b"Net"), Some(b"Plain"))).as_deref(),
            Some("Net")
        );
        assert_eq!(
            window_title_text(&titles(None, Some(b"Plain"))).as_deref(),
            Some("Plain")
        );
        assert_eq!(window_title_text(&titles(Some(b"  \t"), Some(b"x"))), None);
        assert_eq!(
            window_title_text(&titles(Some(&[0xff, 0xfe]), Some(b"x"))),
            None
        );
        assert_eq!(window_title_text(&titles(None, None)), None);
        assert_eq!(
            window_title_text(&titles(Some(b"  padded  "), None)).as_deref(),
            Some("  padded  ")
        );
    }

    #[test]
    fn class_is_the_last_non_empty_part() {
        assert_eq!(
            window_class_name(b"navigator\0Firefox\0").as_deref(),
            Some("Firefox")
        );
        assert_eq!(window_class_name(b"only").as_deref(), Some("only"));
        assert_eq!(window_class_name(b"a\0\0\0"), Some("a".to_owned()));
        assert_eq!(window_class_name(b"\0\0"), None);
    }

    #[test]
    fn icon_closest_to_64_wins_and_is_converted_to_rgba() {
        let mut data = Vec::new();
        // 16x16 (distance 96), 48x48 (distance 32), 600x2 (rejected), 64x72.
        data.extend([16, 16]);
        data.extend(std::iter::repeat_n(0xff00_0000u32, 16 * 16));
        data.extend([48, 48]);
        data.extend(std::iter::repeat_n(0x8011_2233u32, 48 * 48));
        data.extend([600, 2]);
        data.extend(std::iter::repeat_n(0xffff_ffffu32, 1200));
        data.extend([64, 72]);
        data.extend(std::iter::repeat_n(0x0102_0304u32, 64 * 72));
        let icon = select_icon(&data).unwrap();
        // 64x72 has distance 8, the best candidate.
        assert_eq!((icon.width, icon.height), (64, 72));
        assert_eq!(&icon.rgba[..4], &[0x02, 0x03, 0x04, 0x01]);

        let only_48 = &data[..2 + 16 * 16 + 2 + 48 * 48];
        let icon = select_icon(only_48).unwrap();
        assert_eq!((icon.width, icon.height), (48, 48));
        assert_eq!(&icon.rgba[..4], &[0x11, 0x22, 0x33, 0x80]);
    }

    #[test]
    fn icons_over_the_limits_or_truncated_are_ignored() {
        assert!(select_icon(&[513, 1, 0]).is_none());
        let mut too_big = vec![512, 512];
        too_big.extend(std::iter::repeat_n(0, 512 * 512));
        assert!(select_icon(&too_big).is_some());
        assert!(select_icon(&[4, 4, 1, 2, 3]).is_none());
        assert!(select_icon(&[]).is_none());
    }

    fn bgrx_image(pixels: &[[u8; 4]], width: u32, height: u32) -> RawImage {
        RawImage {
            width,
            height,
            bits_per_pixel: 32,
            bytes_per_line: width as usize * 4,
            msb_first: false,
            red_mask: 0x00ff_0000,
            green_mask: 0x0000_ff00,
            blue_mask: 0x0000_00ff,
            data: pixels.iter().flatten().copied().collect(),
        }
    }

    #[test]
    fn channel_masks_are_applied_with_alpha_255() {
        // Little-endian 0x00RRGGBB stored as B, G, R, X.
        let image = bgrx_image(&[[0x30, 0x20, 0x10, 0x00], [0xff, 0x00, 0x80, 0x77]], 2, 1);
        let rgba = raw_image_to_rgba(&image).unwrap();
        assert_eq!(rgba, vec![0x10, 0x20, 0x30, 255, 0x80, 0x00, 0xff, 255]);
    }

    #[test]
    fn narrow_masks_scale_to_full_range() {
        // RGB565, little endian.
        let value: u16 = 0b11111_000000_00000;
        let green: u16 = 0b00000_111111_00000;
        let image = RawImage {
            width: 2,
            height: 1,
            bits_per_pixel: 16,
            bytes_per_line: 4,
            msb_first: false,
            red_mask: 0xf800,
            green_mask: 0x07e0,
            blue_mask: 0x001f,
            data: [value.to_le_bytes(), green.to_le_bytes()].concat(),
        };
        assert_eq!(
            raw_image_to_rgba(&image).unwrap(),
            vec![255, 0, 0, 255, 0, 255, 0, 255]
        );
    }

    #[test]
    fn scanline_padding_and_big_endian_are_honoured() {
        let image = RawImage {
            width: 1,
            height: 2,
            bits_per_pixel: 32,
            bytes_per_line: 8,
            msb_first: true,
            red_mask: 0x00ff_0000,
            green_mask: 0x0000_ff00,
            blue_mask: 0x0000_00ff,
            data: vec![0, 1, 2, 3, 9, 9, 9, 9, 0, 4, 5, 6, 9, 9, 9, 9],
        };
        assert_eq!(
            raw_image_to_rgba(&image).unwrap(),
            vec![1, 2, 3, 255, 4, 5, 6, 255]
        );
    }

    #[test]
    fn short_data_and_over_budget_images_fail_before_allocating() {
        let mut image = bgrx_image(&[[0; 4]], 2, 1);
        assert!(raw_image_to_rgba(&image).is_err());
        image.width = 8193;
        image.height = 1;
        let message = raw_image_to_rgba(&image).unwrap_err();
        assert!(message.contains("8193 x 1"), "{message}");
        let mut empty_mask = bgrx_image(&[[0; 4]], 1, 1);
        empty_mask.green_mask = 0;
        assert!(raw_image_to_rgba(&empty_mask).is_err());
        let mut odd_depth = bgrx_image(&[[0; 4]], 1, 1);
        odd_depth.bits_per_pixel = 8;
        assert!(raw_image_to_rgba(&odd_depth).is_err());
    }
}
