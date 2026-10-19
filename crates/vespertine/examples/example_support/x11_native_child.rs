//! A solid-colored native X11 child window of a framework window, produced by a
//! separate X connection. Used by examples that show framework pixels and native
//! surfaces staying aligned (PLT-002, PLT-004).

use std::cell::Cell;

use raw_window_handle::RawWindowHandle;
use vespertine::Window;
use x11rb::{
    connection::Connection as _,
    protocol::xproto::{ConfigureWindowAux, ConnectionExt as _, CreateWindowAux, WindowClass},
    rust_connection::RustConnection,
};

pub struct NativeChild {
    connection: RustConnection,
    window: u32,
    mapped: Cell<bool>,
}

impl NativeChild {
    /// Creates an unmapped child of `window` filled with `argb`, or `None` with a
    /// message on stderr when the window is not an X11 window.
    pub fn for_window(window: &Window, argb: u32) -> Option<Self> {
        match raw_window_handle::HasWindowHandle::window_handle(window).map(|h| h.as_raw()) {
            Ok(RawWindowHandle::Xcb(handle)) => Self::new(handle.window.get(), argb)
                .map_err(|error| eprintln!("cannot create the native child: {error}"))
                .ok(),
            _ => {
                eprintln!("this example needs an X11 window (run with WAYLAND_DISPLAY unset)");
                None
            }
        }
    }

    fn new(parent: u32, argb: u32) -> anyhow::Result<Self> {
        let (connection, _) = RustConnection::connect(None)?;
        let window = connection.generate_id()?;
        connection.create_window(
            0,
            window,
            parent,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new().background_pixel(argb),
        )?;
        connection.flush()?;
        Ok(Self {
            connection,
            window,
            mapped: Cell::new(false),
        })
    }

    /// The child's current size in device pixels as the X server reports it.
    #[allow(dead_code)]
    pub fn server_size(&self) -> Option<(u16, u16)> {
        let reply = self
            .connection
            .get_geometry(self.window)
            .ok()?
            .reply()
            .ok()?;
        Some((reply.width, reply.height))
    }

    /// Applies geometry in device pixels and maps the window on first use, in one
    /// flushed batch.
    pub fn place(&self, x: i32, y: i32, width: u32, height: u32) {
        let aux = ConfigureWindowAux::new()
            .x(x)
            .y(y)
            .width(width.max(1))
            .height(height.max(1));
        self.connection.configure_window(self.window, &aux).ok();
        if !self.mapped.replace(true) {
            self.connection.map_window(self.window).ok();
        }
        self.connection.flush().ok();
    }
}
