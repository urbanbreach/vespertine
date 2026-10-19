//! Quality selection (CMP-025 R8).

use std::time::Duration;

/// Settled software renders faster than this keep interactive frames at settled quality.
pub const ADAPTIVE_THRESHOLD: Duration = Duration::from_millis(8);
/// Largest `device width x device height x 4` that still supersamples 2 x 2.
pub const SUPERSAMPLE_LIMIT: u64 = 6_000_000;

/// How finely a frame is sampled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Quality {
    /// One sample per device pixel.
    Draft,
    /// Supersampling: 2 x 2 samples per device pixel while `device w x h x 4` is at most
    /// 6,000,000, otherwise one sample per device pixel.
    Supersampled,
}

impl Quality {
    /// Samples per device pixel at this quality for a frame of `device_size` pixels.
    pub fn samples_per_pixel(self, device_size: (u32, u32)) -> u32 {
        match self {
            Quality::Draft => 1,
            Quality::Supersampled => {
                let area = device_size.0 as u64 * device_size.1 as u64 * 4;
                if area <= SUPERSAMPLE_LIMIT { 4 } else { 1 }
            }
        }
    }
}

/// The kind of renderer behind the producer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RendererKind {
    /// A GPU renderer: always the settled quality, so finishing a drag does not re-render an
    /// identical frame.
    Gpu,
    /// A software renderer: Draft while interacting unless the last settled render was fast.
    Software,
}

/// Chooses the quality of the next frame.
///
/// A GPU renderer always uses `settled`. A software renderer uses `interacting` while the
/// turntable runs or a drag is active, unless the scene's last settled render took less than
/// 8 ms (then it stays at `settled`).
pub fn select_quality(
    renderer: RendererKind,
    interacting_now: bool,
    last_settled_render: Option<Duration>,
    interacting: Quality,
    settled: Quality,
) -> Quality {
    match renderer {
        RendererKind::Gpu => settled,
        RendererKind::Software => {
            let fast = last_settled_render.is_some_and(|cost| cost < ADAPTIVE_THRESHOLD);
            if interacting_now && !fast {
                interacting
            } else {
                settled
            }
        }
    }
}
