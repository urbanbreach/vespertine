//! The headless runner (DEV-003 R2, R3): one fresh application per workload and retention
//! mode, one measured frame per scripted step.

use crate::harness::{Driver, FrameOutcome, Harness, RetentionMode};
use crate::metrics::{Meter, Metric, allocations_counted, thread_cpu_available};
use crate::workload::Workload;
use anyhow::{Result, anyhow};
use std::{cell::RefCell, rc::Rc, sync::Arc};
use vespertine::{FrameStats, PlatformTextSystem};

/// Default warm-up frames (not recorded).
pub const DEFAULT_WARMUP: u64 = 30;
/// Default measured frames.
pub const DEFAULT_FRAMES: u64 = 200;

/// How to run a workload.
#[derive(Clone, Debug, Default)]
pub struct RunOptions {
    /// Warm-up steps applied before measuring; `None` takes the workload's own default.
    pub warmup: Option<u64>,
    /// Measured steps; `None` takes the workload's own default.
    pub frames: Option<u64>,
    /// Whether phase and shaping timers run (off by default; counters always run).
    pub phase_times: bool,
}

/// Everything recorded about one measured frame.
#[derive(Clone, Copy, Debug)]
pub struct FrameSample {
    /// Absolute frame index of the step (warm-up frames included).
    pub frame: u64,
    /// What the step cost and how many draws it took.
    pub outcome: FrameOutcome,
}

/// The result of running one workload in one retention mode.
pub struct ScenarioReport {
    /// Workload name.
    pub name: &'static str,
    /// Retention mode the window ran in.
    pub mode: RetentionMode,
    /// Warm-up steps applied.
    pub warmup: u64,
    /// Measured steps.
    pub frames: u64,
    /// The measured frames, in order.
    pub samples: Vec<FrameSample>,
    /// Natural draws over the measured frames.
    pub natural_draws: u64,
    /// Forced draws over the measured frames, counted separately.
    pub forced_draws: u64,
    /// The window's accumulated statistics over the measured frames.
    pub stats: FrameStats,
    /// Why instructions were not measured, if they were not.
    pub instructions_unavailable: Option<String>,
}

impl ScenarioReport {
    fn metric(&self, per_frame: impl Fn(&FrameSample) -> Option<f64>, reason: &str) -> Metric {
        let values: Option<Vec<f64>> = self.samples.iter().map(per_frame).collect();
        Metric::from_values(values, reason)
    }

    /// Per-frame CPU time of the measuring thread, nanoseconds.
    pub fn cpu(&self) -> Metric {
        if !thread_cpu_available() {
            return Metric::Unavailable("no per-thread CPU clock on this platform".to_owned());
        }
        self.metric(
            |sample| Some(sample.outcome.frame.cpu_ns as f64),
            "no frames",
        )
    }

    /// Per-frame wall time, nanoseconds.
    pub fn wall(&self) -> Metric {
        self.metric(
            |sample| Some(sample.outcome.frame.wall_ns as f64),
            "no frames",
        )
    }

    /// Per-frame allocation calls of the whole process.
    pub fn allocations(&self) -> Metric {
        if !allocations_counted() {
            return Metric::Unavailable("the counting allocator is not installed".to_owned());
        }
        self.metric(
            |sample| Some(sample.outcome.frame.allocations as f64),
            "no frames",
        )
    }

    /// Per-frame bytes requested by allocations of the whole process.
    pub fn allocated_bytes(&self) -> Metric {
        if !allocations_counted() {
            return Metric::Unavailable("the counting allocator is not installed".to_owned());
        }
        self.metric(
            |sample| Some(sample.outcome.frame.allocated_bytes as f64),
            "no frames",
        )
    }

    /// Per-frame retired instructions, or `unavailable` with the reason.
    pub fn instructions(&self) -> Metric {
        let reason = self
            .instructions_unavailable
            .as_deref()
            .unwrap_or("instructions were not measured");
        self.metric(
            |sample| sample.outcome.frame.instructions.map(|count| count as f64),
            reason,
        )
    }
}

/// Runs `workload` in a fresh application with `mode`, measuring one frame per step.
pub fn run_scenario(
    text_system: &Arc<dyn PlatformTextSystem>,
    meter: &Rc<RefCell<Meter>>,
    workload: &Workload,
    mode: RetentionMode,
    options: &RunOptions,
) -> Result<ScenarioReport> {
    let warmup = options.warmup.unwrap_or(workload.warmup);
    let frames = options.frames.unwrap_or(workload.frames);
    let mut harness = Harness::new(text_system.clone(), meter.clone(), false);
    let mut driver: Box<dyn Driver> = (workload.setup)(&mut harness)?;
    harness.set_retention(mode)?;
    harness.settle()?;

    let mut next_frame = 0;
    for _ in 0..warmup {
        harness.run_frame(next_frame, &mut *driver)?;
        next_frame += 1;
    }
    harness.reset_frame_stats(options.phase_times)?;
    let mut samples = Vec::with_capacity(frames as usize);
    let (mut natural_draws, mut forced_draws) = (0, 0);
    for _ in 0..frames {
        let outcome = harness.run_frame(next_frame, &mut *driver)?;
        natural_draws += u64::from(outcome.natural_draws);
        forced_draws += u64::from(outcome.forced_draws);
        samples.push(FrameSample {
            frame: next_frame,
            outcome,
        });
        next_frame += 1;
    }
    let stats = harness.frame_stats()?;
    if stats.draws != natural_draws + forced_draws {
        return Err(anyhow!(
            "draw accounting is off for {}: the window drew {} times but {} natural and {} forced \
             draws were counted",
            workload.name,
            stats.draws,
            natural_draws,
            forced_draws
        ));
    }
    Ok(ScenarioReport {
        name: workload.name,
        mode,
        warmup,
        frames,
        samples,
        natural_draws,
        forced_draws,
        stats,
        instructions_unavailable: meter.borrow().instructions_unavailable().map(str::to_owned),
    })
}
