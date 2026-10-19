//! A reference producer for native surfaces (PLT-001): it shows one or two solid colors on a
//! native attachment, through a `wl_shm` buffer on Wayland. Embedders with real content
//! (a video decoder, another toolkit) follow the same pattern: take the handles from
//! [`NativeSurface::layer`](vespertine::NativeSurface::layer) and present independently of the
//! framework.

use anyhow::{Result, bail};
use vespertine::{DevicePixels, NativeComposition, NativeLayer, Size};

/// Shows the left half of a native surface in one solid color and the right half in another,
/// presenting independently of the framework's frames.
///
/// Create it from the window's [`NativeComposition`] and the attachment's layer, call
/// [`Self::present`] whenever the surface's size or colors change, and drop it before the
/// attachment. After the compositor surface is recreated (see
/// [`NativeSurface::on_recreated`](vespertine::NativeSurface::on_recreated)) create a new one.
pub enum SolidProducer {
    /// A `wl_shm` producer on a Wayland connection.
    #[cfg(feature = "wayland")]
    Wayland(super::wayland::WaylandSolidProducer),
    /// A producer drawing into an X11 child window through its own X connection.
    #[cfg(feature = "x11")]
    X11(super::x11::X11SolidProducer),
}

impl SolidProducer {
    /// Attaches to the surface behind `layer`.
    pub fn new(composition: &NativeComposition, layer: NativeLayer) -> Result<Self> {
        match layer {
            #[cfg(feature = "wayland")]
            NativeLayer::Wayland { .. } => Ok(SolidProducer::Wayland(
                super::wayland::WaylandSolidProducer::new(composition, layer)?,
            )),
            #[cfg(feature = "x11")]
            NativeLayer::X11 { window } => Ok(SolidProducer::X11(
                super::x11::X11SolidProducer::new(window)?,
            )),
            other => {
                let _ = composition;
                bail!("no solid-color producer for {other:?}")
            }
        }
    }

    /// Shows `left` over the left half and `right` over the right half of a buffer of `size`
    /// device pixels (opaque sRGB). Does nothing when nothing changed since the last call.
    pub fn present(
        &mut self,
        size: Size<DevicePixels>,
        left: [u8; 3],
        right: [u8; 3],
    ) -> Result<()> {
        match self {
            #[cfg(feature = "wayland")]
            SolidProducer::Wayland(producer) => producer.present(size, left, right),
            #[cfg(feature = "x11")]
            SolidProducer::X11(producer) => producer.present(size, left, right),
        }
    }

    /// Processes the display server's events for the producer (buffer releases, repaints).
    pub fn pump(&mut self) -> Result<()> {
        match self {
            #[cfg(feature = "wayland")]
            SolidProducer::Wayland(producer) => producer.pump(),
            #[cfg(feature = "x11")]
            SolidProducer::X11(producer) => producer.pump(),
        }
    }
}

/// A native child that shows two solid colors: the native surface of a window's composition,
/// its [`SolidProducer`], and the layout slot that positions the surface.
///
/// It enables the window composition on first use and re-attaches the producer when the
/// compositor surface is recreated. Where composition is unavailable it reports so once and
/// the slot stays empty.
pub struct SolidChild {
    surface: Option<std::rc::Rc<vespertine::NativeSurface>>,
    producer: Option<SolidProducer>,
    /// The compositor surface behind the attachment was recreated: the producer is stale.
    stale: std::rc::Rc<std::cell::Cell<bool>>,
    unavailable: Option<String>,
}

impl Default for SolidChild {
    fn default() -> Self {
        Self::new()
    }
}

impl SolidChild {
    /// A child that is created on the first [`Self::prepare`].
    pub fn new() -> Self {
        SolidChild {
            surface: None,
            producer: None,
            stale: std::rc::Rc::new(std::cell::Cell::new(false)),
            unavailable: None,
        }
    }

    /// Why the child cannot be shown, once [`Self::prepare`] failed.
    pub fn unavailable(&self) -> Option<&str> {
        self.unavailable.as_deref()
    }

    /// The native surface, once created.
    pub fn surface(&self) -> Option<&std::rc::Rc<vespertine::NativeSurface>> {
        self.surface.as_ref()
    }

    /// Enables the window composition and creates the native surface and its producer on the
    /// first call, and re-attaches the producer after the compositor surface was recreated.
    pub fn prepare(&mut self, window: &mut vespertine::Window) {
        if self.unavailable.is_some() {
            return;
        }
        if self.surface.is_none() {
            let created = window
                .enable_window_composition()
                .map_err(|error| format!("{error:#}"))
                .and_then(|()| {
                    window
                        .composition()
                        .and_then(|controller| controller.create_native_surface())
                        .map_err(|error| error.to_string())
                });
            match created {
                Ok(surface) => {
                    let stale = self.stale.clone();
                    surface.on_recreated(move |_| stale.set(true));
                    self.surface = Some(std::rc::Rc::new(surface));
                    self.stale.set(true);
                }
                Err(error) => {
                    log::error!("the native child is unavailable: {error}");
                    self.unavailable = Some(error);
                    return;
                }
            }
        }
        if self.stale.replace(false) {
            // The producer presents to the old handle: drop it before attaching to the new one.
            self.producer = None;
            let (Some(surface), Some(composition)) =
                (self.surface.as_ref(), window.native_composition())
            else {
                return;
            };
            let Some(layer) = surface.layer() else {
                return;
            };
            match SolidProducer::new(&composition, layer) {
                Ok(producer) => self.producer = Some(producer),
                Err(error) => {
                    log::error!("the native child's producer is unavailable: {error:#}");
                    self.unavailable = Some(format!("{error:#}"));
                }
            }
        }
    }

    /// Shows `left` over the left half and `right` over the right half (0xRRGGBB) of the
    /// child, whose bounds in logical pixels are `bounds`.
    pub fn present(
        &mut self,
        window: &vespertine::Window,
        bounds: vespertine::Bounds<vespertine::Pixels>,
        left: u32,
        right: u32,
    ) {
        let Some(producer) = &mut self.producer else {
            return;
        };
        let rgb = |value: u32| [(value >> 16) as u8, (value >> 8) as u8, value as u8];
        let device = bounds.to_device_pixels(window.scale_factor());
        let size = Size {
            width: DevicePixels(device.size.width.0),
            height: DevicePixels(device.size.height.0),
        };
        if let Err(error) = producer.present(size, rgb(left), rgb(right)) {
            log::error!("presenting the native child failed: {error:#}");
        }
    }

    /// The layout slot that positions the native surface at `bounds` (absolutely, inside a
    /// relative parent). Painted in base content it stacks below the overlay; painted inside a
    /// deferred draw it is hosted above it (PLT-001 R11).
    pub fn slot(&self, bounds: vespertine::Bounds<vespertine::Pixels>) -> vespertine::AnyElement {
        use vespertine::{IntoElement as _, ParentElement as _, Styled as _};
        let Some(surface) = self.surface.clone() else {
            return vespertine::div().into_any_element();
        };
        vespertine::div()
            .absolute()
            .left(bounds.origin.x)
            .top(bounds.origin.y)
            .w(bounds.size.width)
            .h(bounds.size.height)
            .child(
                vespertine::canvas(
                    move |bounds, window, _| surface.position(window, bounds, true),
                    |_, _, _, _| {},
                )
                .size_full(),
            )
            .into_any_element()
    }
}
