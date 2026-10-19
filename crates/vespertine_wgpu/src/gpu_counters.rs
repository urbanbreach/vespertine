//! Process-wide counters of GPU device and sprite atlas creations, an instrumentation hook
//! for PLT-001: creating an overlay renderer must share the window's device and atlas, so
//! tests and performance scenes watch these counters stay put per frame.

use std::sync::atomic::{AtomicUsize, Ordering};

static DEVICES: AtomicUsize = AtomicUsize::new(0);
static ATLASES: AtomicUsize = AtomicUsize::new(0);

/// How many GPU devices and glyph/sprite atlases this process created so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GpuCreationCounts {
    /// GPU devices created.
    pub devices: usize,
    /// Sprite atlases created.
    pub atlases: usize,
}

impl GpuCreationCounts {
    /// The creations since `earlier` was read.
    pub fn since(self, earlier: GpuCreationCounts) -> GpuCreationCounts {
        GpuCreationCounts {
            devices: self.devices - earlier.devices,
            atlases: self.atlases - earlier.atlases,
        }
    }
}

/// The current creation counts.
pub fn gpu_creation_counts() -> GpuCreationCounts {
    GpuCreationCounts {
        devices: DEVICES.load(Ordering::Relaxed),
        atlases: ATLASES.load(Ordering::Relaxed),
    }
}

pub(crate) fn note_device_created() {
    DEVICES.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn note_atlas_created() {
    ATLASES.fetch_add(1, Ordering::Relaxed);
}
