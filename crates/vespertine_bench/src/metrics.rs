//! Cost measurement for the headless runner (DEV-003 R3): per-thread CPU time, wall time,
//! process-wide allocation counts and retired instructions, with distributions over frames.
//!
//! Instructions are reported only when the platform's performance counters can be opened and
//! advance; any failure is carried as [`Metric::Unavailable`] with its reason, never as zero.

use std::ops::{Add, AddAssign, Sub};
use std::time::Instant;

/// A cost accumulated over an interval. Differences saturate at zero.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cost {
    /// CPU time of the measuring thread, nanoseconds (zero when the platform has no per-thread
    /// CPU clock; see [`thread_cpu_available`]).
    pub cpu_ns: u64,
    /// Wall-clock time, nanoseconds.
    pub wall_ns: u64,
    /// Allocation calls (including reallocations) of the whole process.
    pub allocations: u64,
    /// Bytes requested by those allocations.
    pub allocated_bytes: u64,
    /// Retired user-space instructions of the measuring thread; `None` when unavailable.
    pub instructions: Option<u64>,
}

impl Cost {
    /// No cost, with instructions counted (zero), for accumulating available measurements.
    pub const ZERO: Self = Self {
        cpu_ns: 0,
        wall_ns: 0,
        allocations: 0,
        allocated_bytes: 0,
        instructions: Some(0),
    };
}

impl Sub for Cost {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        Self {
            cpu_ns: self.cpu_ns.saturating_sub(other.cpu_ns),
            wall_ns: self.wall_ns.saturating_sub(other.wall_ns),
            allocations: self.allocations.saturating_sub(other.allocations),
            allocated_bytes: self.allocated_bytes.saturating_sub(other.allocated_bytes),
            instructions: self
                .instructions
                .zip(other.instructions)
                .map(|(total, part)| total.saturating_sub(part)),
        }
    }
}

impl Add for Cost {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            cpu_ns: self.cpu_ns + other.cpu_ns,
            wall_ns: self.wall_ns + other.wall_ns,
            allocations: self.allocations + other.allocations,
            allocated_bytes: self.allocated_bytes + other.allocated_bytes,
            instructions: self
                .instructions
                .zip(other.instructions)
                .map(|(first, second)| first + second),
        }
    }
}

impl AddAssign for Cost {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

/// Where instruction counts come from. The platform implementation reads a Linux performance
/// counter; tests substitute their own.
pub trait InstructionSource {
    /// The retired-instruction count so far, monotonic.
    fn read(&mut self) -> u64;
}

/// Whether instruction counts can be taken, and if not, why.
pub enum Instructions {
    /// A working counter.
    Available(Box<dyn InstructionSource>),
    /// The counter could not be opened or does not advance.
    Unavailable(String),
}

impl Instructions {
    /// Opens the platform's per-thread user-space instruction counter. A permission or
    /// availability failure, or a counter that opens but does not advance (a virtual machine
    /// without a PMU), yields [`Instructions::Unavailable`] with the reason.
    pub fn open() -> Self {
        match platform::open_instruction_counter() {
            Ok(source) => Self::from_source(source),
            Err(reason) => Self::Unavailable(reason),
        }
    }

    /// Wraps an opened source, checking that it advances while this thread does work.
    pub fn from_source(mut source: Box<dyn InstructionSource>) -> Self {
        // A counter may need a moment to be scheduled onto the PMU: give it a few chances.
        for _ in 0..4 {
            let before = source.read();
            std::hint::black_box(
                (0..200_000_u64).fold(0_u64, |total, value| total.wrapping_add(value)),
            );
            if source.read() > before {
                return Self::Available(source);
            }
        }
        Self::Unavailable(
            "the instruction counter opened but does not advance (no performance monitoring \
             unit available to this machine)"
                .to_owned(),
        )
    }

    /// The reason instructions are unavailable.
    pub fn unavailable_reason(&self) -> Option<&str> {
        match self {
            Self::Available(_) => None,
            Self::Unavailable(reason) => Some(reason),
        }
    }
}

/// A point of the cost counters; subtract two with [`Snapshot::since`].
#[derive(Clone, Copy)]
pub struct Snapshot {
    cpu_ns: u64,
    wall: Instant,
    allocations: u64,
    allocated_bytes: u64,
    instructions: Option<u64>,
}

impl Snapshot {
    /// The cost between `earlier` and this snapshot.
    pub fn since(&self, earlier: &Snapshot) -> Cost {
        Cost {
            cpu_ns: self.cpu_ns.saturating_sub(earlier.cpu_ns),
            wall_ns: u64::try_from(self.wall.duration_since(earlier.wall).as_nanos())
                .unwrap_or(u64::MAX),
            allocations: self.allocations.saturating_sub(earlier.allocations),
            allocated_bytes: self.allocated_bytes.saturating_sub(earlier.allocated_bytes),
            instructions: self
                .instructions
                .zip(earlier.instructions)
                .map(|(end, start)| end.saturating_sub(start)),
        }
    }
}

/// Takes snapshots. Taking one never allocates.
pub struct Meter {
    instructions: Instructions,
}

impl Meter {
    /// A meter with the platform's instruction counter if it can be opened.
    pub fn new() -> Self {
        Self::with_instructions(Instructions::open())
    }

    /// A meter with the given instruction availability.
    pub fn with_instructions(instructions: Instructions) -> Self {
        Self { instructions }
    }

    /// Why instructions are not measured, if they are not.
    pub fn instructions_unavailable(&self) -> Option<&str> {
        self.instructions.unavailable_reason()
    }

    /// Reads every counter now.
    pub fn now(&mut self) -> Snapshot {
        let instructions = match &mut self.instructions {
            Instructions::Available(source) => Some(source.read()),
            Instructions::Unavailable(_) => None,
        };
        // The allocation counters are read last so that nothing the instruction read did is
        // counted inside an interval that ends here (and first at the start of one).
        let stats = vespertine_bench_metrics::allocation_stats();
        Snapshot {
            cpu_ns: thread_cpu_ns().unwrap_or(0),
            wall: Instant::now(),
            allocations: stats.map_or(0, |stats| stats.allocations),
            allocated_bytes: stats.map_or(0, |stats| stats.allocated_bytes),
            instructions,
        }
    }
}

impl Default for Meter {
    fn default() -> Self {
        Self::new()
    }
}

/// CPU time consumed by the calling thread, or `None` when the platform has no such clock.
pub fn thread_cpu_ns() -> Option<u64> {
    platform::thread_cpu_ns()
}

/// Whether the platform provides per-thread CPU time.
pub fn thread_cpu_available() -> bool {
    thread_cpu_ns().is_some()
}

/// Whether process allocation counting is installed (it is for every binary of this crate).
pub fn allocations_counted() -> bool {
    vespertine_bench_metrics::allocation_stats().is_some()
}

#[cfg(target_os = "linux")]
mod platform {
    use super::InstructionSource;
    use vespertine_bench_metrics::{HardwareCounter, HardwareEvent, MetricScope};

    pub(super) fn thread_cpu_ns() -> Option<u64> {
        let mut time = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: `time` is a valid, writable timespec and CLOCK_THREAD_CPUTIME_ID is a clock
        // the call accepts.
        let status = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) };
        (status == 0).then(|| time.tv_sec as u64 * 1_000_000_000 + time.tv_nsec as u64)
    }

    struct PerfInstructions(HardwareCounter);

    impl InstructionSource for PerfInstructions {
        fn read(&mut self) -> u64 {
            self.0.cumulative()
        }
    }

    pub(super) fn open_instruction_counter() -> Result<Box<dyn InstructionSource>, String> {
        HardwareCounter::new(HardwareEvent::Instructions, MetricScope::CallingThread)
            .map(|counter| Box::new(PerfInstructions(counter)) as Box<dyn InstructionSource>)
            .map_err(|error| format!("{error:#}"))
    }
}

#[cfg(not(target_os = "linux"))]
mod platform {
    use super::InstructionSource;

    pub(super) fn thread_cpu_ns() -> Option<u64> {
        None
    }

    pub(super) fn open_instruction_counter() -> Result<Box<dyn InstructionSource>, String> {
        Err("retired-instruction counting is implemented for Linux only".to_owned())
    }
}

/// Mean, median, 95th percentile and maximum of per-frame values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Distribution {
    /// Arithmetic mean.
    pub mean: f64,
    /// Median; the mean of the two middle values for an even count.
    pub median: f64,
    /// 95th percentile by nearest rank: the value at rank `ceil(0.95 n)` of the sorted values.
    pub p95: f64,
    /// Largest value.
    pub max: f64,
}

impl Distribution {
    /// The distribution of `values`, or `None` for no values.
    pub fn of(values: &[f64]) -> Option<Self> {
        if values.is_empty() {
            return None;
        }
        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        let count = sorted.len();
        let median = if count % 2 == 1 {
            sorted[count / 2]
        } else {
            (sorted[count / 2 - 1] + sorted[count / 2]) / 2.0
        };
        let rank = (count * 95).div_ceil(100).max(1);
        Some(Self {
            mean: sorted.iter().sum::<f64>() / count as f64,
            median,
            p95: sorted[rank - 1],
            max: sorted[count - 1],
        })
    }
}

/// A measured distribution, or the reason the quantity could not be measured. An unavailable
/// metric is never represented by zeros.
#[derive(Clone, Debug, PartialEq)]
pub enum Metric {
    /// Per-frame values were measured.
    Measured(Distribution),
    /// The quantity is not available, with the reason.
    Unavailable(String),
}

impl Metric {
    /// The distribution of `values`, or unavailable with `reason` when there are none.
    pub fn from_values(values: Option<Vec<f64>>, reason: &str) -> Self {
        match values.as_deref().and_then(Distribution::of) {
            Some(distribution) => Self::Measured(distribution),
            None => Self::Unavailable(reason.to_owned()),
        }
    }

    /// The distribution if measured.
    pub fn measured(&self) -> Option<&Distribution> {
        match self {
            Self::Measured(distribution) => Some(distribution),
            Self::Unavailable(_) => None,
        }
    }

    /// One report cell: `mean/median/p95/max` in `unit`, or `unavailable (reason)`.
    pub fn describe(&self, scale: f64, unit: &str) -> String {
        match self {
            Self::Measured(d) => format!(
                "mean {:.3} median {:.3} p95 {:.3} max {:.3} {unit}",
                d.mean / scale,
                d.median / scale,
                d.p95 / scale,
                d.max / scale
            ),
            Self::Unavailable(reason) => format!("unavailable ({reason})"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distribution_reports_mean_median_p95_and_max() {
        let values: Vec<f64> = (1..=100).map(f64::from).collect();
        let d = Distribution::of(&values).unwrap();
        assert_eq!(d.mean, 50.5);
        assert_eq!(d.median, 50.5);
        assert_eq!(d.p95, 95.0);
        assert_eq!(d.max, 100.0);
        let d = Distribution::of(&[7.0, 1.0, 3.0]).unwrap();
        assert_eq!((d.median, d.p95, d.max), (3.0, 7.0, 7.0));
        assert!(Distribution::of(&[]).is_none());
    }
}
