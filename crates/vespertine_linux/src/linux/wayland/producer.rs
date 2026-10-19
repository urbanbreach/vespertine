//! A reference producer for native surfaces on Wayland (PLT-001): it shows one or two solid
//! colors on a native attachment's `wl_surface` through `wl_shm` buffers, presenting
//! independently of the framework (R12).

use std::{
    ffi::c_void,
    os::fd::{AsFd, FromRawFd, OwnedFd},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::{Context as _, Result, bail};
use vespertine::{DevicePixels, NativeComposition, NativeDevice, NativeLayer, Size};
use wayland_backend::client::{Backend, ObjectId};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_buffer, wl_registry, wl_shm, wl_shm_pool, wl_surface::WlSurface},
};

/// The most buffers the producer keeps in flight; a compositor that holds all of them without
/// releasing any makes presenting an error.
const MAX_BUFFERS: usize = 4;

struct ProducerState;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for ProducerState {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_shm::WlShm, ()> for ProducerState {
    fn event(
        _: &mut Self,
        _: &wl_shm::WlShm,
        _: wl_shm::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_shm_pool::WlShmPool, ()> for ProducerState {
    fn event(
        _: &mut Self,
        _: &wl_shm_pool::WlShmPool,
        _: wl_shm_pool::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_buffer::WlBuffer, Arc<AtomicBool>> for ProducerState {
    fn event(
        _: &mut Self,
        _: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        busy: &Arc<AtomicBool>,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            busy.store(false, Ordering::Relaxed);
        }
    }
}

/// One shared-memory buffer with its pool and mapping.
struct Slot {
    pool: wl_shm_pool::WlShmPool,
    buffer: wl_buffer::WlBuffer,
    map: *mut u8,
    capacity: usize,
    size: Size<i32>,
    busy: Arc<AtomicBool>,
}

impl Slot {
    fn new(shm: &wl_shm::WlShm, qh: &QueueHandle<ProducerState>, size: Size<i32>) -> Result<Self> {
        let capacity = size.width as usize * size.height as usize * 4;
        let fd = unsafe { libc::memfd_create(c"vespertine-native".as_ptr(), libc::MFD_CLOEXEC) };
        if fd < 0 {
            bail!("memfd_create failed: {}", std::io::Error::last_os_error());
        }
        // Safety: `fd` is a freshly created descriptor nothing else owns.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        if unsafe {
            libc::ftruncate(
                std::os::fd::AsRawFd::as_raw_fd(&fd),
                capacity as libc::off_t,
            )
        } != 0
        {
            bail!("ftruncate failed: {}", std::io::Error::last_os_error());
        }
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                capacity,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                std::os::fd::AsRawFd::as_raw_fd(&fd),
                0,
            )
        };
        if map == libc::MAP_FAILED {
            bail!("mmap failed: {}", std::io::Error::last_os_error());
        }
        let pool = shm.create_pool(fd.as_fd(), capacity as i32, qh, ());
        let busy = Arc::new(AtomicBool::new(false));
        let buffer = pool.create_buffer(
            0,
            size.width,
            size.height,
            size.width * 4,
            wl_shm::Format::Xrgb8888,
            qh,
            busy.clone(),
        );
        Ok(Slot {
            pool,
            buffer,
            map: map.cast(),
            capacity,
            size,
            busy,
        })
    }

    /// Fills the buffer: `left` over `[0, width / 2)` and `right` over the rest.
    fn fill(&mut self, left: [u8; 3], right: [u8; 3]) {
        let width = self.size.width as usize;
        let pixel = |color: [u8; 3]| {
            0xFF00_0000u32
                | u32::from(color[0]) << 16
                | u32::from(color[1]) << 8
                | u32::from(color[2])
        };
        let (left, right) = (pixel(left), pixel(right));
        let split = width / 2;
        // Safety: `map` covers `capacity` bytes, which is `width * height * 4`.
        let pixels =
            unsafe { std::slice::from_raw_parts_mut(self.map.cast::<u32>(), self.capacity / 4) };
        let (first_row, rest) = pixels.split_at_mut(width);
        first_row[..split].fill(left);
        first_row[split..].fill(right);
        for row in rest.chunks_exact_mut(width) {
            row.copy_from_slice(first_row);
        }
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.buffer.destroy();
        self.pool.destroy();
        // Safety: `map` and `capacity` come from the `mmap` in `Slot::new`.
        unsafe { libc::munmap(self.map.cast::<c_void>(), self.capacity) };
    }
}

/// Shows two solid colors on a native attachment's surface: the left half of its width in one
/// color and the right half in another (the same color twice for a single color).
///
/// The producer opens its own event queue on the window's Wayland connection and presents
/// whenever [`Self::present`] says the content changed, independently of the framework's
/// frames. Drop it before the attachment it presents to.
pub struct WaylandSolidProducer {
    _connection: Connection,
    queue: EventQueue<ProducerState>,
    state: ProducerState,
    shm: wl_shm::WlShm,
    surface: WlSurface,
    slots: Vec<Slot>,
    last: Option<(Size<DevicePixels>, [u8; 3], [u8; 3])>,
}

impl WaylandSolidProducer {
    /// Attaches to the native attachment whose `layer` is `layer`, on the connection named by
    /// `composition`.
    pub fn new(composition: &NativeComposition, layer: NativeLayer) -> Result<Self> {
        let NativeDevice::Wayland { display } = composition.device else {
            bail!("the window is not on a Wayland connection");
        };
        let NativeLayer::Wayland { surface } = layer else {
            bail!("the surface is not a Wayland surface");
        };
        // Safety: the pointer is the `wl_display` of the live window connection.
        let backend = unsafe { Backend::from_foreign_display(display.as_ptr().cast()) };
        let connection = Connection::from_backend(backend);
        // Safety: the pointer is a live `wl_surface` of this connection; the producer is
        // dropped before its attachment.
        let id = unsafe { ObjectId::from_ptr(WlSurface::interface(), surface.as_ptr().cast()) }
            .context("the native surface is not a wl_surface of this connection")?;
        let surface = WlSurface::from_id(&connection, id)
            .context("the native surface is not a wl_surface of this connection")?;
        let (globals, queue) = registry_queue_init::<ProducerState>(&connection)
            .context("reading the Wayland globals failed")?;
        let shm: wl_shm::WlShm = globals
            .bind(&queue.handle(), 1..=1, ())
            .context("the compositor does not provide wl_shm")?;
        Ok(WaylandSolidProducer {
            _connection: connection,
            queue,
            state: ProducerState,
            shm,
            surface,
            slots: Vec::new(),
            last: None,
        })
    }

    /// Processes buffer releases. Call it regularly; [`Self::present`] does it as well.
    pub fn pump(&mut self) -> Result<()> {
        self.queue
            .dispatch_pending(&mut self.state)
            .context("dispatching producer events failed")?;
        Ok(())
    }

    fn free_slot(&mut self, size: Size<i32>) -> Result<usize> {
        let qh = self.queue.handle();
        let free = self
            .slots
            .iter()
            .position(|slot| !slot.busy.load(Ordering::Relaxed));
        let index = match free {
            Some(index) => index,
            None if self.slots.len() < MAX_BUFFERS => {
                self.slots.push(Slot::new(&self.shm, &qh, size)?);
                return Ok(self.slots.len() - 1);
            }
            None => {
                // Every buffer is held: wait for the compositor to release one.
                self.queue
                    .roundtrip(&mut self.state)
                    .context("waiting for a buffer release failed")?;
                self.slots
                    .iter()
                    .position(|slot| !slot.busy.load(Ordering::Relaxed))
                    .context("the compositor holds every buffer of the producer")?
            }
        };
        if self.slots[index].size != size {
            self.slots[index] = Slot::new(&self.shm, &qh, size)?;
        }
        Ok(index)
    }

    /// Shows `left` over the left half and `right` over the right half of a buffer of `size`
    /// device pixels, as opaque sRGB colors. Does nothing when nothing changed since the last
    /// call. The compositor scales the buffer to the attachment's bounds.
    pub fn present(
        &mut self,
        size: Size<DevicePixels>,
        left: [u8; 3],
        right: [u8; 3],
    ) -> Result<()> {
        self.pump()?;
        if self.last == Some((size, left, right)) {
            return Ok(());
        }
        let buffer_size = Size {
            width: size.width.0.max(1),
            height: size.height.0.max(1),
        };
        let index = self.free_slot(buffer_size)?;
        let slot = &mut self.slots[index];
        slot.fill(left, right);
        slot.busy.store(true, Ordering::Relaxed);
        self.surface.attach(Some(&slot.buffer), 0, 0);
        self.surface
            .damage_buffer(0, 0, buffer_size.width, buffer_size.height);
        self.surface.commit();
        self._connection
            .flush()
            .context("flushing the producer's frame failed")?;
        self.last = Some((size, left, right));
        Ok(())
    }
}

impl Drop for WaylandSolidProducer {
    fn drop(&mut self) {
        // Detach so the compositor does not keep showing a buffer that is destroyed now.
        self.surface.attach(None, 0, 0);
        self.surface.commit();
        self.slots.clear();
        let _ = self._connection.flush();
    }
}
