//! A reference native producer for X11 child windows (PLT-001 R12, R21).
//!
//! An embedder receives the child window id of a native surface through
//! [`vespertine::NativeLayer::X11`]. [`X11SolidProducer`] draws two opaque solid colors into
//! such a window through its own X connection, independently of the framework's UI thread and
//! connection, the way a real producer (a video surface, a web view, another toolkit's widget)
//! presents into its window.

use anyhow::{Context as _, Result, anyhow, bail};
use vespertine::{DevicePixels, Size};
use x11rb::{
    connection::Connection,
    protocol::{
        Event,
        xproto::{
            ChangeGCAux, ConnectionExt as _, CreateGCAux, EventMask, Gcontext, Rectangle,
            VisualClass, Visualid, Window,
        },
    },
    rust_connection::RustConnection,
};

/// Packs 8-bit sRGB colors into pixel values of the window's visual.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PixelFormat {
    red_mask: u32,
    green_mask: u32,
    blue_mask: u32,
    /// The bits the visual's depth holds beyond the color channels (an alpha channel). They are
    /// set so the pixels are opaque.
    alpha_mask: u32,
}

impl PixelFormat {
    fn new(depth: u8, red_mask: u32, green_mask: u32, blue_mask: u32) -> Self {
        let depth_mask = if depth >= 32 {
            u32::MAX
        } else {
            (1u32 << depth) - 1
        };
        PixelFormat {
            red_mask,
            green_mask,
            blue_mask,
            alpha_mask: depth_mask & !(red_mask | green_mask | blue_mask),
        }
    }

    fn channel(value: u8, mask: u32) -> u32 {
        if mask == 0 {
            return 0;
        }
        let bits = mask.count_ones();
        let scaled = if bits >= 8 {
            u32::from(value) << (bits - 8) | (u32::from(value) >> (16 - bits.min(16)))
        } else {
            u32::from(value) >> (8 - bits)
        };
        (scaled << mask.trailing_zeros()) & mask
    }

    /// The pixel value of an opaque sRGB color.
    fn pixel(&self, [red, green, blue]: [u8; 3]) -> u32 {
        Self::channel(red, self.red_mask)
            | Self::channel(green, self.green_mask)
            | Self::channel(blue, self.blue_mask)
            | self.alpha_mask
    }
}

/// What the producer last painted or was asked to paint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Request {
    size: Size<DevicePixels>,
    left: [u8; 3],
    right: [u8; 3],
}

/// Paints a native child window with two solid sRGB colors: the left half and the right half.
///
/// The producer opens its own X connection, so it presents independently of the framework
/// that owns the window's parent. Dropping it releases its resources; the window itself
/// belongs to the framework.
pub struct X11SolidProducer {
    connection: RustConnection,
    window: Window,
    gc: Gcontext,
    format: PixelFormat,
    /// The size the server reports for the window; the producer paints this, not what
    /// the caller thinks.
    server_size: (u16, u16),
    /// The last request, painted or not.
    request: Option<Request>,
    /// The window holds something other than the last request: nothing was painted yet, or it
    /// was exposed or resized since.
    stale: bool,
}

impl X11SolidProducer {
    /// Opens an independent connection to the display `window` lives on and prepares to paint
    /// it. `window` is the child window id of a native surface
    /// ([`vespertine::NativeLayer::X11`]).
    pub fn new(window: u32) -> Result<Self> {
        let (connection, _screen) =
            x11rb::connect(None).context("the X11 producer could not connect to the X server")?;
        let attributes = connection
            .get_window_attributes(window)?
            .reply()
            .with_context(|| format!("X11 window {window} does not exist"))?;
        let geometry = connection
            .get_geometry(window)?
            .reply()
            .with_context(|| format!("X11 window {window} has no geometry"))?;
        let format = pixel_format(&connection, attributes.visual, geometry.depth)?;
        connection
            .change_window_attributes(
                window,
                &x11rb::protocol::xproto::ChangeWindowAttributesAux::new()
                    .event_mask(EventMask::EXPOSURE | EventMask::STRUCTURE_NOTIFY),
            )?
            .check()
            .context("the X11 producer could not select events on its window")?;
        let gc = connection.generate_id()?;
        connection
            .create_gc(gc, window, &CreateGCAux::new())?
            .check()
            .context("the X11 producer could not create a graphics context")?;
        connection.flush()?;
        Ok(X11SolidProducer {
            connection,
            window,
            gc,
            format,
            server_size: (geometry.width, geometry.height),
            request: None,
            stale: true,
        })
    }

    /// Fills `[0, width / 2)` of the window with `left` and the rest with `right`, as opaque
    /// solid sRGB colors. Does nothing when `size`, `left` and `right` equal the last call's
    /// and the window was not exposed or resized since. The window is painted with the size the
    /// server reports for it when that differs from `size`.
    pub fn present(
        &mut self,
        size: Size<DevicePixels>,
        left: [u8; 3],
        right: [u8; 3],
    ) -> Result<()> {
        let request = Request { size, left, right };
        if self.request == Some(request) && !self.stale {
            return Ok(());
        }
        self.request = Some(request);
        self.drain_events()?;
        // The size the server holds now, which may be newer than any event received.
        let geometry = self
            .connection
            .get_geometry(self.window)?
            .reply()
            .with_context(|| format!("X11 window {} is gone", self.window))?;
        self.server_size = (geometry.width, geometry.height);
        self.paint(request)
    }

    /// Drains this connection's events and repaints the last request after an expose or a
    /// resize of the window.
    pub fn pump(&mut self) -> Result<()> {
        self.drain_events()?;
        if self.stale
            && let Some(request) = self.request
        {
            self.paint(request)?;
        }
        Ok(())
    }

    fn drain_events(&mut self) -> Result<()> {
        while let Some(event) = self.connection.poll_for_event()? {
            match event {
                Event::Expose(expose) if expose.window == self.window => self.stale = true,
                Event::ConfigureNotify(configure) if configure.window == self.window => {
                    self.server_size = (configure.width, configure.height);
                    self.stale = true;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn paint(&mut self, request: Request) -> Result<()> {
        let (width, height) = self.server_size;
        let half = width / 2;
        for (rectangle, color) in [
            (
                Rectangle {
                    x: 0,
                    y: 0,
                    width: half,
                    height,
                },
                request.left,
            ),
            (
                Rectangle {
                    x: half as i16,
                    y: 0,
                    width: width - half,
                    height,
                },
                request.right,
            ),
        ] {
            if rectangle.width == 0 || rectangle.height == 0 {
                continue;
            }
            self.connection.change_gc(
                self.gc,
                &ChangeGCAux::new().foreground(self.format.pixel(color)),
            )?;
            self.connection
                .poly_fill_rectangle(self.window, self.gc, &[rectangle])?;
        }
        self.connection.flush()?;
        self.stale = false;
        Ok(())
    }
}

impl Drop for X11SolidProducer {
    fn drop(&mut self) {
        // The connection closes right after, which releases the graphics context and the event
        // selection anyway; freeing it explicitly keeps the server state tidy for a producer
        // that is dropped while its window lives on.
        if self.connection.free_gc(self.gc).is_ok() {
            let _ = self.connection.flush();
        }
    }
}

fn pixel_format(connection: &RustConnection, visual: Visualid, depth: u8) -> Result<PixelFormat> {
    let visual_type = connection
        .setup()
        .roots
        .iter()
        .flat_map(|screen| screen.allowed_depths.iter())
        .flat_map(|allowed| allowed.visuals.iter())
        .find(|candidate| candidate.visual_id == visual)
        .ok_or_else(|| anyhow!("the window's visual {visual} is not offered by the X server"))?;
    if visual_type.class != VisualClass::TRUE_COLOR
        && visual_type.class != VisualClass::DIRECT_COLOR
    {
        bail!(
            "the window's visual {visual} is not a TrueColor visual ({:?})",
            visual_type.class
        );
    }
    Ok(PixelFormat::new(
        depth,
        visual_type.red_mask,
        visual_type.green_mask,
        visual_type.blue_mask,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_opaque_colors_into_a_24_bit_visual() {
        let format = PixelFormat::new(24, 0xFF0000, 0x00FF00, 0x0000FF);
        assert_eq!(format.pixel([0x20, 0x60, 0xC0]), 0x2060C0);
    }

    #[test]
    fn packs_opaque_colors_into_an_argb_visual_with_full_alpha() {
        let format = PixelFormat::new(32, 0xFF0000, 0x00FF00, 0x0000FF);
        assert_eq!(format.pixel([0x23, 0x68, 0xB4]), 0xFF2368B4);
    }

    #[test]
    fn scales_channels_of_narrow_visuals() {
        // RGB565.
        let format = PixelFormat::new(16, 0xF800, 0x07E0, 0x001F);
        assert_eq!(format.pixel([0xFF, 0xFF, 0xFF]), 0xFFFF);
        assert_eq!(format.pixel([0x00, 0x00, 0x00]), 0x0000);
        assert_eq!(format.pixel([0xFF, 0x00, 0x00]), 0xF800);
    }
}

#[cfg(test)]
mod server_tests {
    use super::*;
    use x11rb::protocol::xproto::{CreateWindowAux, EventMask, ImageFormat, MapState, WindowClass};
    use x11rb::wrapper::ConnectionExt as _;

    /// Creates and maps a plain top-level window on a fresh connection and returns it once the
    /// server holds it viewable.
    fn mapped_window(connection: &RustConnection, screen: usize, size: (u16, u16)) -> Window {
        let root = connection.setup().roots[screen].root;
        let window = connection.generate_id().unwrap();
        connection
            .create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                window,
                root,
                0,
                0,
                size.0,
                size.1,
                0,
                WindowClass::INPUT_OUTPUT,
                x11rb::COPY_FROM_PARENT,
                &CreateWindowAux::new()
                    .override_redirect(1)
                    .event_mask(EventMask::STRUCTURE_NOTIFY),
            )
            .unwrap()
            .check()
            .unwrap();
        connection.map_window(window).unwrap().check().unwrap();
        loop {
            if let Event::MapNotify(event) = connection.wait_for_event().unwrap()
                && event.window == window
            {
                break;
            }
        }
        window
    }

    /// The pixel, or `None` while the server cannot read the window yet.
    fn pixel(connection: &RustConnection, window: Window, x: i16, y: i16) -> Option<[u8; 3]> {
        let image = connection
            .get_image(ImageFormat::Z_PIXMAP, window, x, y, 1, 1, u32::MAX)
            .ok()?
            .reply()
            .ok()?;
        // Little-endian 32 bits per pixel: blue, green, red, padding or alpha.
        Some([image.data[2], image.data[1], image.data[0]])
    }

    /// The server composes the window's pixels asynchronously on some setups (XWayland): wait
    /// for the expected pixel instead of reading it once.
    fn assert_pixel(
        connection: &RustConnection,
        window: Window,
        x: i16,
        y: i16,
        expected: [u8; 3],
    ) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let actual = pixel(connection, window, x, y);
            if actual == Some(expected) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "pixel ({x}, {y}) is {actual:?}, expected {expected:?}"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[test]
    fn paints_two_solid_halves_into_a_real_window_and_follows_its_size() {
        let (connection, screen) = x11rb::connect(None)
            .expect("this test needs a reachable X server (DISPLAY, e.g. XWayland)");
        let window = mapped_window(&connection, screen, (64, 32));
        let attributes = connection
            .get_window_attributes(window)
            .unwrap()
            .reply()
            .unwrap();
        assert_eq!(attributes.map_state, MapState::VIEWABLE);

        let mut producer = X11SolidProducer::new(window).unwrap();
        let left = [0x23, 0x68, 0xB4];
        let right = [0x68, 0xB0, 0xE8];
        let size = Size {
            width: DevicePixels(64),
            height: DevicePixels(32),
        };
        producer.present(size, left, right).unwrap();
        producer.connection.sync().unwrap();
        assert_pixel(&connection, window, 5, 5, left);
        assert_pixel(&connection, window, 31, 31, left);
        assert_pixel(&connection, window, 32, 0, right);
        assert_pixel(&connection, window, 63, 31, right);

        // Nothing changed: no request at all is sent, so the window keeps its pixels even if
        // another client scribbles over them before the next call.
        let scribble = connection.generate_id().unwrap();
        connection
            .create_gc(scribble, window, &CreateGCAux::new().foreground(0x00FF00))
            .unwrap();
        connection
            .poly_fill_rectangle(
                window,
                scribble,
                &[Rectangle {
                    x: 0,
                    y: 0,
                    width: 64,
                    height: 32,
                }],
            )
            .unwrap();
        connection.sync().unwrap();
        producer.present(size, left, right).unwrap();
        producer.connection.sync().unwrap();
        assert_pixel(&connection, window, 5, 5, [0, 0xFF, 0]);

        // The window is resized behind the caller's back: the producer paints the size the
        // server reports, not the stale one it is told.
        connection
            .configure_window(
                window,
                &x11rb::protocol::xproto::ConfigureWindowAux::new()
                    .width(100)
                    .height(40),
            )
            .unwrap()
            .check()
            .unwrap();
        connection.sync().unwrap();
        producer.present(size, right, left).unwrap();
        producer.connection.sync().unwrap();
        assert_pixel(&connection, window, 49, 39, right);
        assert_pixel(&connection, window, 50, 39, left);
        assert_pixel(&connection, window, 99, 0, left);

        // An expose repaints the last request without a new call to present.
        connection
            .poly_fill_rectangle(
                window,
                scribble,
                &[Rectangle {
                    x: 0,
                    y: 0,
                    width: 100,
                    height: 40,
                }],
            )
            .unwrap();
        connection
            .clear_area(true, window, 0, 0, 0, 0)
            .unwrap()
            .check()
            .unwrap();
        connection.sync().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        producer.pump().unwrap();
        producer.connection.sync().unwrap();
        assert_pixel(&connection, window, 10, 10, right);
        assert_pixel(&connection, window, 90, 10, left);

        drop(producer);
        connection.destroy_window(window).unwrap().check().unwrap();
    }
}
