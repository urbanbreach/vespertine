//! PLT-001 R8, R18: a renderer for an additional presentation surface shares the window
//! renderer's device and sprite atlas, and a renderer that lost its device or released its GPU
//! resources reports itself unusable so it is discarded rather than reused.
//!
//! The renderers present to plain `wl_surface`s of the Wayland session (no role, nothing is
//! shown). Without a Wayland session the tests report so and return, like the other host tests.

#![cfg(all(any(target_os = "linux", target_os = "freebsd"), feature = "wayland"))]

use std::{cell::RefCell, ffi::c_void, ptr::NonNull, rc::Rc};

use raw_window_handle as rwh;
use vespertine::DevicePixels;
use vespertine::Size;
use vespertine_wgpu::{GpuContext, WgpuRenderer, WgpuSurfaceConfig, gpu_creation_counts, wgpu};
use wayland_client::{
    Connection, Proxy, QueueHandle, delegate_noop,
    globals::registry_queue_init,
    protocol::{wl_compositor::WlCompositor, wl_surface::WlSurface},
};

struct Client;

delegate_noop!(Client: ignore WlCompositor);
delegate_noop!(Client: ignore WlSurface);
impl
    wayland_client::Dispatch<
        wayland_client::protocol::wl_registry::WlRegistry,
        wayland_client::globals::GlobalListContents,
    > for Client
{
    fn event(
        _: &mut Self,
        _: &wayland_client::protocol::wl_registry::WlRegistry,
        _: wayland_client::protocol::wl_registry::Event,
        _: &wayland_client::globals::GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

#[derive(Clone, Copy, Debug)]
struct RawSurface {
    surface: *mut c_void,
    display: *mut c_void,
}

// Safety: the pointers name objects of the test's live connection, which outlives every
// renderer the test creates.
unsafe impl Send for RawSurface {}
unsafe impl Sync for RawSurface {}

impl rwh::HasWindowHandle for RawSurface {
    fn window_handle(&self) -> Result<rwh::WindowHandle<'_>, rwh::HandleError> {
        let handle = rwh::WaylandWindowHandle::new(NonNull::new(self.surface).unwrap());
        Ok(unsafe { rwh::WindowHandle::borrow_raw(handle.into()) })
    }
}

impl rwh::HasDisplayHandle for RawSurface {
    fn display_handle(&self) -> Result<rwh::DisplayHandle<'_>, rwh::HandleError> {
        let handle = rwh::WaylandDisplayHandle::new(NonNull::new(self.display).unwrap());
        Ok(unsafe { rwh::DisplayHandle::borrow_raw(handle.into()) })
    }
}

/// The GPU creation counters are process-wide and Vulkan teardown is not safe to overlap, so
/// the tests of this file take turns.
static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A connection with a source of role-less surfaces, or `None` without a Wayland session.
struct Session {
    _connection: Connection,
    compositor: WlCompositor,
    queue: wayland_client::EventQueue<Client>,
}

impl Session {
    fn connect() -> Option<Self> {
        let connection = Connection::connect_to_env().ok()?;
        let (globals, queue) = registry_queue_init::<Client>(&connection).ok()?;
        let compositor = globals.bind(&queue.handle(), 4..=6, ()).ok()?;
        Some(Session {
            _connection: connection,
            compositor,
            queue,
        })
    }

    fn surface(&self) -> (WlSurface, RawSurface) {
        let surface = self.compositor.create_surface(&self.queue.handle(), ());
        let display = surface.backend().upgrade().unwrap().display_ptr().cast();
        let raw = RawSurface {
            surface: surface.id().as_ptr().cast(),
            display,
        };
        (surface, raw)
    }
}

fn config() -> WgpuSurfaceConfig {
    WgpuSurfaceConfig {
        size: Size {
            width: DevicePixels(64),
            height: DevicePixels(48),
        },
        transparent: true,
        preferred_present_mode: Some(wgpu::PresentMode::Mailbox),
    }
}

#[test]
fn an_additional_renderer_shares_the_device_and_the_atlas() {
    let _turn = TURN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(session) = Session::connect() else {
        eprintln!("no Wayland session: the renderer sharing test needs one");
        return;
    };
    let gpu: GpuContext = Rc::new(RefCell::new(None));
    let (_window_surface, window_raw) = session.surface();
    let window_renderer =
        WgpuRenderer::new(gpu.clone(), &window_raw, config(), None).expect("window renderer");
    assert!(window_renderer.is_reusable());

    let before = gpu_creation_counts();
    let (_overlay_surface, overlay_raw) = session.surface();
    let overlay = WgpuRenderer::new_sharing_atlas(
        gpu.clone(),
        &overlay_raw,
        config(),
        None,
        window_renderer.sprite_atlas().clone(),
    )
    .expect("overlay renderer");
    let created = gpu_creation_counts().since(before);
    assert_eq!(created.devices, 0, "the overlay renderer creates no device");
    assert_eq!(created.atlases, 0, "the overlay renderer creates no atlas");
    assert!(
        std::sync::Arc::ptr_eq(window_renderer.sprite_atlas(), overlay.sprite_atlas()),
        "primitives reference the same atlas in both planes"
    );
    assert!(overlay.is_reusable());
}

#[test]
fn sharing_an_atlas_needs_a_device_to_share() {
    let _turn = TURN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(session) = Session::connect() else {
        eprintln!("no Wayland session: the renderer sharing test needs one");
        return;
    };
    let gpu: GpuContext = Rc::new(RefCell::new(None));
    let (_surface, raw) = session.surface();
    let donor_gpu: GpuContext = Rc::new(RefCell::new(None));
    let (_donor_surface, donor_raw) = session.surface();
    let donor = WgpuRenderer::new(donor_gpu, &donor_raw, config(), None).expect("donor renderer");
    // The context that sharing is asked for holds no device: an atlas could not belong to it.
    let error =
        WgpuRenderer::new_sharing_atlas(gpu, &raw, config(), None, donor.sprite_atlas().clone())
            .err()
            .expect("sharing without a device fails");
    assert!(
        error.to_string().contains("needs the device"),
        "unexpected error: {error:#}"
    );
}

#[test]
fn a_renderer_that_lost_its_device_or_released_its_resources_is_not_reusable() {
    let _turn = TURN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(session) = Session::connect() else {
        eprintln!("no Wayland session: the renderer sharing test needs one");
        return;
    };
    // A private device: losing it must not disturb the other tests of this process.
    let gpu: GpuContext = Rc::new(RefCell::new(None));
    let (_window_surface, window_raw) = session.surface();
    let window_renderer =
        WgpuRenderer::new(gpu.clone(), &window_raw, config(), None).expect("window renderer");
    let (_overlay_surface, overlay_raw) = session.surface();
    let mut overlay = WgpuRenderer::new_sharing_atlas(
        gpu.clone(),
        &overlay_raw,
        config(),
        None,
        window_renderer.sprite_atlas().clone(),
    )
    .expect("overlay renderer");
    assert!(overlay.is_reusable());

    // Releasing the GPU resources makes the renderer unusable.
    let (_spare_surface, spare_raw) = session.surface();
    let mut spare = WgpuRenderer::new_sharing_atlas(
        gpu,
        &spare_raw,
        config(),
        None,
        window_renderer.sprite_atlas().clone(),
    )
    .expect("spare renderer");
    assert!(spare.is_reusable());
    spare.destroy();
    assert!(!spare.is_reusable(), "a released renderer is not reused");

    // The window renderer loses the device the overlay shares: both report it.
    window_renderer.force_device_loss();
    assert!(!window_renderer.is_reusable());
    assert!(
        !overlay.is_reusable(),
        "an overlay renderer of a lost device is discarded, not reused"
    );
    overlay.destroy();
}
