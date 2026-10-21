//! Reproducible frame-cost benchmarks and retained-output verification (DEV-003).
//!
//! A *workload* scripts one model or input change per frame (a *step*). The headless
//! [`runner`] opens a fresh real [`vespertine::Window`] with real text shaping for each
//! [`harness::RetentionMode`], applies each step once, drains tasks and effects until the window
//! is parked, and measures the complete resulting frame with the step's own cost subtracted.
//! [`verify`] draws a retained and a retention-disabled window through identical steps and
//! compares them every frame.
//!
//! None of this is required by production rendering.

use vespertine_bench_metrics::CountingAllocator;

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator::new(std::alloc::System);

pub mod harness;
pub mod metrics;
pub mod platform_report;
pub mod report;
pub mod runner;
pub mod verify;
pub mod workload;
pub mod workloads;

pub use harness::{Driver, EventLog, FaultKind, FrameOutcome, Harness, RetentionMode, Stage};
pub use metrics::{Cost, Distribution, Instructions, Meter, Metric};
pub use runner::{RunOptions, ScenarioReport, run_scenario};
pub use workload::{Family, Workload, catalogue, select};
