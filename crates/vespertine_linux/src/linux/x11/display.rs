use anyhow::Context as _;
use uuid::Uuid;
use x11rb::{connection::Connection as _, xcb_ffi::XCBConnection};

use vespertine::{Bounds, DisplayId, Pixels, PlatformDisplay, Size, px};

use super::read_work_area;

#[derive(Debug)]
pub(crate) struct X11Display {
    x_screen_index: usize,
    bounds: Bounds<Pixels>,
    /// The work area: `_NET_WORKAREA` of the current desktop within `bounds`.
    visible_bounds: Bounds<Pixels>,
    uuid: Uuid,
}

impl X11Display {
    pub(crate) fn new(
        xcb: &XCBConnection,
        scale_factor: f32,
        x_screen_index: usize,
    ) -> anyhow::Result<Self> {
        let screen = xcb
            .setup()
            .roots
            .get(x_screen_index)
            .with_context(|| format!("No screen found with index {x_screen_index}"))?;
        let bounds = Bounds {
            origin: Default::default(),
            size: Size {
                width: px(screen.width_in_pixels as f32 / scale_factor),
                height: px(screen.height_in_pixels as f32 / scale_factor),
            },
        };
        Ok(Self {
            x_screen_index,
            bounds,
            visible_bounds: read_work_area(xcb, screen.root, scale_factor, bounds),
            // Stable across restarts and distinct per screen; screen 0 keeps the nil uuid.
            uuid: Uuid::from_u128(x_screen_index as u128),
        })
    }
}

impl PlatformDisplay for X11Display {
    // An X screen spans every monitor, so it has no single refresh rate.
    fn refresh_interval(&self) -> Option<std::time::Duration> {
        None
    }

    fn id(&self) -> DisplayId {
        DisplayId::new(self.x_screen_index as u64)
    }

    fn uuid(&self) -> anyhow::Result<Uuid> {
        Ok(self.uuid)
    }

    fn bounds(&self) -> Bounds<Pixels> {
        self.bounds
    }

    fn visible_bounds(&self) -> Bounds<Pixels> {
        self.visible_bounds
    }
}
