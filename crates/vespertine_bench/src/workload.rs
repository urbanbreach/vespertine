//! The catalogue of deterministic workloads (DEV-003 R4, R5).

use crate::harness::{Driver, Host};
use anyhow::Result;

/// A scripted scenario.
#[derive(Clone, Copy)]
pub struct Workload {
    /// Name, matched exactly or by substring from the command line.
    pub name: &'static str,
    /// The R4 family the workload belongs to.
    pub family: Family,
    /// One line describing what each frame does.
    pub description: &'static str,
    /// Default warm-up frames.
    pub warmup: u64,
    /// Default measured frames.
    pub frames: u64,
    /// Builds the application state and opens the window; returns the step script.
    pub setup: fn(&mut dyn Host) -> Result<Box<dyn Driver>>,
}

/// The workload families of R4.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    /// An unchanged window and a scrolled sidebar (published orientation workloads).
    Baseline,
    /// Field typing and validation.
    Fields,
    /// Selection and toggle changes.
    Selection,
    /// Uniform- and variable-height list scrolling and live updates.
    Lists,
    /// 200-row quote tables with sparse and dense updates and periodic sorting.
    Quotes,
    /// The 10,000-row virtual quote table.
    VirtualTable,
    /// Settings navigation and theme changes.
    Settings,
    /// Nested layout changes.
    NestedLayout,
    /// Wheel scrolling.
    WheelScroll,
    /// Scrollbar dragging.
    ScrollbarDrag,
    /// 160- and 200-message rich transcripts.
    Transcripts,
    /// A docked workspace with hidden and visible subscribers and streaming quotes.
    Workspace,
}

impl Family {
    /// Every family R4 lists, in order (the baseline family is additional).
    pub const R4: [Self; 11] = [
        Self::Fields,
        Self::Selection,
        Self::Lists,
        Self::Quotes,
        Self::VirtualTable,
        Self::Settings,
        Self::NestedLayout,
        Self::WheelScroll,
        Self::ScrollbarDrag,
        Self::Transcripts,
        Self::Workspace,
    ];
}

/// Every workload, in catalogue order.
pub fn catalogue() -> Vec<Workload> {
    let mut all = Vec::new();
    crate::workloads::register(&mut all);
    all
}

/// The workloads whose name equals or contains one of `filters` (all of them when there is no
/// filter), in catalogue order.
pub fn select(filters: &[String]) -> Vec<Workload> {
    catalogue()
        .into_iter()
        .filter(|workload| {
            filters.is_empty()
                || filters.iter().any(|filter| {
                    workload.name == filter || workload.name.contains(filter.as_str())
                })
        })
        .collect()
}
