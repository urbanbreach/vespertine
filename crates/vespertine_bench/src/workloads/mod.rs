//! The workload families (DEV-003 R4, R5). Every workload builds retained views over entity
//! models, so retention has something to reuse, and scripts its steps as pure functions of the
//! absolute frame index.

mod fields;
mod lists;
mod nested;
mod quotes;
mod scrolling;
mod settings;
mod transcripts;
mod workspace;

pub use quotes::{
    QuoteBook, QuoteProbe, Schedule, SortColumn, format_volume, quote_driver, virtual_driver,
};
pub use transcripts::TranscriptPattern;

use crate::workload::Workload;
use vespertine::{Hsla, rgb};

/// Body text color (#1f2328).
pub const INK: u32 = 0x1f2328;
/// Window background (#ffffff).
pub const WHITE: u32 = 0xffffff;
/// Zebra and panel background (#f6f8fa).
pub const SURFACE: u32 = 0xf6f8fa;
/// Hairline color.
pub const BORDER: u32 = 0xd0d7de;
/// Positive change (#1a7f37).
pub const GREEN: u32 = 0x1a7f37;
/// Negative change (#cf222e).
pub const RED: u32 = 0xcf222e;
/// The proportional font of every workload.
pub const SANS: &str = "Noto Sans";
/// The monospace font of code-like text.
pub const MONO: &str = "Liberation Mono";

/// An opaque sRGB color.
pub fn hex(value: u32) -> Hsla {
    rgb(value).into()
}

/// Adds every workload to the catalogue.
pub fn register(all: &mut Vec<Workload>) {
    all.push(quotes::idle());
    all.extend(fields::workloads());
    all.extend(lists::workloads());
    all.extend(quotes::workloads());
    all.extend(settings::workloads());
    all.extend(nested::workloads());
    all.extend(scrolling::workloads());
    all.extend(transcripts::workloads());
    all.extend(workspace::workloads());
}
