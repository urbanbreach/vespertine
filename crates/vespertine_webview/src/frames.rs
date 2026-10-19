//! Pixel frames received from the helper: inline copies and shared-memory buffers
//! (PLT-015 R7 to R13).

use crate::protocol::{MAX_DIMENSION, MAX_SCALE, MIN_SCALE};
use serde_json::{Map, Value};
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::Arc;

/// A rectangle of changed pixels.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Damage {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Damage {
    pub fn full(width: u32, height: u32) -> Self {
        Self {
            x: 0,
            y: 0,
            width,
            height,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// The bounding rectangle of two rectangles; an empty rectangle contributes nothing.
    pub fn union(self, other: Damage) -> Damage {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        let x0 = self.x.min(other.x);
        let y0 = self.y.min(other.y);
        let x1 = (self.x + self.width).max(other.x + other.width);
        let y1 = (self.y + self.height).max(other.y + other.height);
        Damage {
            x: x0,
            y: y0,
            width: x1 - x0,
            height: y1 - y0,
        }
    }

    /// Whether the rectangle lies inside a `width` x `height` frame.
    pub fn fits(&self, width: u32, height: u32) -> bool {
        u64::from(self.x) + u64::from(self.width) <= u64::from(width)
            && u64::from(self.y) + u64::from(self.height) <= u64::from(height)
    }
}

/// A read-only shared mapping of one pool slot.
struct Mapping {
    ptr: *const u8,
    len: usize,
}

// SAFETY: the mapping is read-only memory that is only unmapped on drop.
unsafe impl Send for Mapping {}
unsafe impl Sync for Mapping {}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: `ptr` and `len` come from a successful mmap.
        unsafe {
            libc::munmap(self.ptr as *mut libc::c_void, self.len);
        }
    }
}

/// A pool of shared-memory buffers announced by the helper (kind `P`).
///
/// Buffers are mapped read-only; a new generation invalidates all earlier buffers, which are
/// unmapped when their last user lets go of the `Arc<Pool>`.
pub struct Pool {
    pub generation: u64,
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub stride: u32,
    slots: Vec<Mapping>,
}

impl std::fmt::Debug for Pool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pool")
            .field("generation", &self.generation)
            .field("size", &(self.width, self.height))
            .field("scale", &self.scale)
            .field("stride", &self.stride)
            .field("slots", &self.slots.len())
            .finish()
    }
}

fn unsigned(object: &Map<String, Value>, key: &str) -> Option<u64> {
    object.get(key).and_then(Value::as_u64)
}

impl Pool {
    /// Number of descriptors a pool announcement `object` must carry (one per slot).
    pub fn expected_descriptors(object: &Map<String, Value>) -> usize {
        match object.get("slots") {
            Some(Value::Array(slots)) => slots.len(),
            _ => 0,
        }
    }

    /// Validates and maps a shared-memory pool announcement.
    ///
    /// The announcement is rejected (and its descriptors closed) when: the kind is not "shm"; the
    /// format is not "BGRA8888"; the generation is zero; a dimension is outside 1..8192; the scale
    /// is outside 0.5..4; the stride is below width x 4; there is not exactly one descriptor per
    /// slot; a slot's size is below stride x height; or a descriptor is not a memory file sealed
    /// against shrinking and at least as large as announced (so reading it can never fault).
    pub fn from_announcement(
        object: &Map<String, Value>,
        fds: Vec<OwnedFd>,
    ) -> Result<Pool, String> {
        if object.get("kind").and_then(Value::as_str) != Some("shm") {
            return Err("pool kind is not shm".into());
        }
        if object.get("format").and_then(Value::as_str) != Some("BGRA8888") {
            return Err("pool format is not BGRA8888".into());
        }
        let generation = unsigned(object, "generation")
            .filter(|g| *g > 0)
            .ok_or("pool generation is missing")?;
        let dimension = |key: &str| {
            unsigned(object, key)
                .filter(|v| (1..=u64::from(MAX_DIMENSION)).contains(v))
                .map(|v| v as u32)
                .ok_or_else(|| format!("pool {key} is out of range"))
        };
        let width = dimension("width")?;
        let height = dimension("height")?;
        let scale = object
            .get("scale")
            .and_then(Value::as_f64)
            .map(|scale| scale as f32)
            .filter(|scale| scale.is_finite() && (MIN_SCALE..=MAX_SCALE).contains(scale))
            .ok_or("pool scale is out of range")?;
        let stride = unsigned(object, "stride")
            .filter(|stride| *stride >= u64::from(width) * 4 && *stride <= u64::from(u32::MAX))
            .ok_or("pool stride is smaller than a row")? as u32;
        let Some(Value::Array(slots)) = object.get("slots") else {
            return Err("pool has no slots".into());
        };
        if slots.is_empty() || slots.len() != fds.len() {
            return Err("pool descriptors do not match its slots".into());
        }
        let needed = u64::from(stride) * u64::from(height);
        let mut mappings = Vec::with_capacity(slots.len());
        for (index, (slot, fd)) in slots.iter().zip(fds).enumerate() {
            let slot = slot.as_object().ok_or("pool slot is not an object")?;
            if unsigned(slot, "slot") != Some(index as u64) {
                return Err("pool slots are not numbered in order".into());
            }
            let size = unsigned(slot, "size").ok_or("pool slot has no size")?;
            if size < needed {
                return Err("pool slot is smaller than stride x height".into());
            }
            mappings.push(map_slot(fd, needed as usize)?);
        }
        Ok(Pool {
            generation,
            width,
            height,
            scale,
            stride,
            slots: mappings,
        })
    }

    /// Number of slots in the pool.
    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }

    /// The full contents of `slot` (stride x height bytes), or `None` when out of range.
    pub fn slot_bytes(&self, slot: u32) -> Option<&[u8]> {
        let mapping = self.slots.get(slot as usize)?;
        // SAFETY: the mapping is `len` readable bytes that stay mapped as long as `self` lives,
        // and the memory file is sealed against shrinking (checked in `map_slot`).
        Some(unsafe { std::slice::from_raw_parts(mapping.ptr, mapping.len) })
    }

    /// A pool backed by anonymous memory files created here, for scripted helpers in tests.
    #[doc(hidden)]
    pub fn announcement_for_tests(
        generation: u64,
        width: u32,
        height: u32,
        stride: u32,
        scale: f32,
        slots: usize,
    ) -> (Map<String, Value>, Vec<OwnedFd>) {
        let size = u64::from(stride) * u64::from(height);
        let mut fds = Vec::new();
        let mut descriptors = Vec::new();
        for slot in 0..slots {
            fds.push(memfd_sealed(size as usize));
            descriptors.push(serde_json::json!({"slot": slot, "size": size}));
        }
        let object = serde_json::json!({
            "generation": generation, "kind": "shm", "width": width, "height": height,
            "scale": f64::from(scale), "stride": stride, "format": "BGRA8888", "slots": descriptors,
        });
        (object.as_object().unwrap().clone(), fds)
    }
}

fn map_slot(fd: OwnedFd, len: usize) -> Result<Mapping, String> {
    let raw = fd.as_raw_fd();
    // SAFETY: plain syscalls on a descriptor we own.
    unsafe {
        let mut stat: libc::stat = std::mem::zeroed();
        if libc::fstat(raw, &mut stat) != 0 {
            return Err(format!(
                "could not inspect a pool descriptor: {}",
                std::io::Error::last_os_error()
            ));
        }
        if (stat.st_size as u64) < len as u64 {
            return Err("pool descriptor is smaller than the announced size".into());
        }
        let seals = libc::fcntl(raw, libc::F_GET_SEALS);
        if seals < 0 || seals & libc::F_SEAL_SHRINK == 0 {
            return Err("pool descriptor is not sealed against shrinking".into());
        }
        let ptr = libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ,
            libc::MAP_SHARED,
            raw,
            0,
        );
        if ptr == libc::MAP_FAILED {
            return Err(format!(
                "could not map a pool descriptor: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(Mapping {
            ptr: ptr.cast(),
            len,
        })
    }
}

/// Creates a sealed memory file of `size` bytes (scripted helpers in tests).
#[doc(hidden)]
pub fn memfd_sealed(size: usize) -> OwnedFd {
    use std::os::fd::FromRawFd;
    // SAFETY: plain syscalls; the descriptor is wrapped immediately.
    unsafe {
        let raw = libc::memfd_create(
            c"vsp-test-slot".as_ptr(),
            libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
        );
        assert!(raw >= 0, "memfd_create failed");
        let fd = OwnedFd::from_raw_fd(raw);
        assert_eq!(libc::ftruncate(raw, size as libc::off_t), 0);
        assert_eq!(
            libc::fcntl(
                raw,
                libc::F_ADD_SEALS,
                libc::F_SEAL_SHRINK | libc::F_SEAL_GROW
            ),
            0
        );
        fd
    }
}

/// Writes `bytes` into a memory file at `offset` (scripted helpers in tests).
#[doc(hidden)]
pub fn write_memfd(fd: &impl AsRawFd, offset: usize, bytes: &[u8]) {
    // SAFETY: plain pwrite on a valid descriptor.
    let written = unsafe {
        libc::pwrite(
            fd.as_raw_fd(),
            bytes.as_ptr().cast(),
            bytes.len(),
            offset as libc::off_t,
        )
    };
    assert_eq!(written, bytes.len() as isize);
}

/// Returns a buffer slot to the helper when dropped (PLT-015 R10).
pub struct Lease {
    release: Arc<dyn Fn(u64, u32) + Send + Sync>,
    generation: u64,
    slot: u32,
}

impl Lease {
    pub(crate) fn new(
        release: Arc<dyn Fn(u64, u32) + Send + Sync>,
        generation: u64,
        slot: u32,
    ) -> Self {
        Self {
            release,
            generation,
            slot,
        }
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        (self.release)(self.generation, self.slot);
    }
}

/// A frame whose pixels live in a shared-memory slot owned by the application until released.
pub struct BufferFrame {
    pool: Arc<Pool>,
    slot: u32,
    pub seq: u64,
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    /// Bounding rectangle of every pixel that changed since the previously uploaded frame,
    /// including the damage of buffer frames dropped in between.
    pub damage: Damage,
    lease: Option<Lease>,
}

impl BufferFrame {
    pub(crate) fn new(pool: Arc<Pool>, slot: u32, seq: u64, damage: Damage, lease: Lease) -> Self {
        Self {
            width: pool.width,
            height: pool.height,
            scale: pool.scale,
            pool,
            slot,
            seq,
            damage,
            lease: Some(lease),
        }
    }

    pub fn generation(&self) -> u64 {
        self.pool.generation
    }

    pub fn slot(&self) -> u32 {
        self.slot
    }

    pub fn stride(&self) -> u32 {
        self.pool.stride
    }

    /// The whole slot: `stride x height` bytes of BGRA straight-alpha rows.
    pub fn pixels(&self) -> &[u8] {
        self.pool
            .slot_bytes(self.slot)
            .expect("slot validated at arrival")
    }

    /// Returns the slot to the helper now instead of at drop.
    pub fn release(&mut self) {
        self.lease = None;
    }
}

/// An inline frame: tightly packed BGRA straight-alpha pixels (stride width x 4).
#[derive(Clone)]
pub struct InlineFrame {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub pixels: Arc<Vec<u8>>,
}

/// The single latest frame of a page.
pub enum Frame {
    Inline(InlineFrame),
    Buffer(BufferFrame),
}

impl Frame {
    pub fn width(&self) -> u32 {
        match self {
            Frame::Inline(frame) => frame.width,
            Frame::Buffer(frame) => frame.width,
        }
    }

    pub fn height(&self) -> u32 {
        match self {
            Frame::Inline(frame) => frame.height,
            Frame::Buffer(frame) => frame.height,
        }
    }

    /// The scale the frame was captured at ("frame scale" used for placement).
    pub fn scale(&self) -> f32 {
        match self {
            Frame::Inline(frame) => frame.scale,
            Frame::Buffer(frame) => frame.scale,
        }
    }

    /// Bytes per row of [`Frame::pixels`].
    pub fn stride(&self) -> u32 {
        match self {
            Frame::Inline(frame) => frame.width * 4,
            Frame::Buffer(frame) => frame.stride(),
        }
    }

    /// The frame's rows, `stride x height` bytes.
    pub fn pixels(&self) -> &[u8] {
        match self {
            Frame::Inline(frame) => &frame.pixels,
            Frame::Buffer(frame) => frame.pixels(),
        }
    }

    /// The accumulated damage; an inline frame is always fully damaged.
    pub fn damage(&self) -> Damage {
        match self {
            Frame::Inline(frame) => Damage::full(frame.width, frame.height),
            Frame::Buffer(frame) => frame.damage,
        }
    }

    /// Red, green and blue of the top-left pixel (the page's own background colour).
    pub fn top_left_rgb(&self) -> [u8; 3] {
        let pixels = self.pixels();
        [pixels[2], pixels[1], pixels[0]]
    }

    /// Tightly packed BGRA pixels (stride width x 4): a copy for padded or shared buffers.
    pub fn packed_pixels(&self) -> Vec<u8> {
        match self {
            Frame::Inline(frame) => frame.pixels.as_ref().clone(),
            Frame::Buffer(frame) => {
                let row = frame.width as usize * 4;
                let stride = frame.stride() as usize;
                let source = frame.pixels();
                let mut packed = Vec::with_capacity(row * frame.height as usize);
                for y in 0..frame.height as usize {
                    packed.extend_from_slice(&source[y * stride..y * stride + row]);
                }
                packed
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn damage_union_is_the_bounding_rectangle() {
        let a = Damage {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        let b = Damage {
            x: 20,
            y: 20,
            width: 10,
            height: 10,
        };
        assert_eq!(
            a.union(b),
            Damage {
                x: 0,
                y: 0,
                width: 30,
                height: 30
            }
        );
        let empty = Damage {
            x: 5,
            y: 5,
            width: 0,
            height: 0,
        };
        assert_eq!(a.union(empty), a);
        assert_eq!(empty.union(b), b);
        assert!(a.fits(10, 10) && !a.fits(9, 10));
        assert!(
            !Damage {
                x: u32::MAX,
                y: 0,
                width: 2,
                height: 1
            }
            .fits(10, 10)
        );
    }

    #[test]
    fn pools_validate_and_map_their_slots() {
        let (object, fds) = Pool::announcement_for_tests(1, 8, 4, 64, 2.0, 2);
        assert_eq!(Pool::expected_descriptors(&object), 2);
        write_memfd(&fds[0], 64 + 4 * 7, &[1, 2, 3, 4]);
        let pool = Pool::from_announcement(&object, fds).unwrap();
        assert_eq!((pool.slot_count(), pool.stride, pool.scale), (2, 64, 2.0));
        let bytes = pool.slot_bytes(0).unwrap();
        assert_eq!(bytes.len(), 64 * 4);
        assert_eq!(&bytes[64 + 28..64 + 32], &[1, 2, 3, 4]);
        assert!(pool.slot_bytes(2).is_none());
    }

    #[test]
    fn invalid_announcements_are_rejected() {
        let invalid = |edit: &dyn Fn(&mut Map<String, Value>)| {
            let (mut object, fds) = Pool::announcement_for_tests(1, 8, 4, 64, 1.0, 2);
            edit(&mut object);
            Pool::from_announcement(&object, fds).is_err()
        };
        assert!(invalid(&|o| {
            o.insert("kind".into(), "dmabuf".into());
        }));
        assert!(invalid(&|o| {
            o.insert("format".into(), "RGBA".into());
        }));
        assert!(invalid(&|o| {
            o.insert("generation".into(), 0.into());
        }));
        assert!(invalid(&|o| {
            o.insert("width".into(), 0.into());
        }));
        assert!(invalid(&|o| {
            o.insert("width".into(), 9000.into());
        }));
        assert!(invalid(&|o| {
            o.insert("scale".into(), 0.1.into());
        }));
        // Stride below width x 4.
        assert!(invalid(&|o| {
            o.insert("stride".into(), 16.into());
        }));
        // Size smaller than stride x height.
        assert!(invalid(&|o| {
            let slots = o.get_mut("slots").unwrap().as_array_mut().unwrap();
            slots[1]["size"] = 100.into();
        }));
        // The descriptor count must match the slots.
        let (object, mut fds) = Pool::announcement_for_tests(1, 8, 4, 64, 1.0, 2);
        fds.pop();
        assert!(Pool::from_announcement(&object, fds).is_err());
        // A descriptor that is too small or unsealed is refused.
        let (object, mut fds) = Pool::announcement_for_tests(1, 8, 4, 64, 1.0, 1);
        fds[0] = memfd_sealed(16);
        assert!(Pool::from_announcement(&object, fds).is_err());
        let (object, mut fds) = Pool::announcement_for_tests(1, 8, 4, 64, 1.0, 1);
        // SAFETY: plain syscalls creating an unsealed memory file.
        fds[0] = unsafe {
            use std::os::fd::FromRawFd;
            let raw = libc::memfd_create(c"unsealed".as_ptr(), libc::MFD_CLOEXEC);
            libc::ftruncate(raw, 4096);
            OwnedFd::from_raw_fd(raw)
        };
        assert!(Pool::from_announcement(&object, fds).is_err());
    }

    #[test]
    fn leases_release_exactly_once_on_drop() {
        let released = Arc::new(Mutex::new(Vec::new()));
        let sink = released.clone();
        let release: Arc<dyn Fn(u64, u32) + Send + Sync> =
            Arc::new(move |generation, slot| sink.lock().unwrap().push((generation, slot)));
        let (object, fds) = Pool::announcement_for_tests(3, 2, 2, 64, 1.0, 2);
        let pool = Arc::new(Pool::from_announcement(&object, fds).unwrap());
        let mut frame = BufferFrame::new(
            pool.clone(),
            1,
            1,
            Damage::full(2, 2),
            Lease::new(release.clone(), 3, 1),
        );
        frame.release();
        frame.release();
        drop(frame);
        let frame = BufferFrame::new(pool, 0, 2, Damage::full(2, 2), Lease::new(release, 3, 0));
        assert_eq!(*released.lock().unwrap(), vec![(3, 1)]);
        drop(frame);
        assert_eq!(*released.lock().unwrap(), vec![(3, 1), (3, 0)]);
    }

    #[test]
    fn packed_pixels_remove_the_row_padding() {
        let (object, fds) = Pool::announcement_for_tests(1, 1000, 2, 4096, 1.0, 1);
        write_memfd(&fds[0], 4096 + 3996, &[9, 8, 7, 6]);
        let pool = Arc::new(Pool::from_announcement(&object, fds).unwrap());
        let release: Arc<dyn Fn(u64, u32) + Send + Sync> = Arc::new(|_, _| {});
        let frame = Frame::Buffer(BufferFrame::new(
            pool,
            0,
            1,
            Damage::full(1000, 2),
            Lease::new(release, 1, 0),
        ));
        let packed = frame.packed_pixels();
        assert_eq!(packed.len(), 1000 * 2 * 4);
        assert_eq!(&packed[4000 + 3996..4000 + 4000], &[9, 8, 7, 6]);
    }
}
