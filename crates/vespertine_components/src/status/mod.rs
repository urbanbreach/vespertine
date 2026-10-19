//! Loading placeholders, progress and collapsible status (CMP-023).
//!
//! - [`loadable`]: a state wrapper (idle, loading, ready, error with inline retry) whose slot
//!   keeps one outer size in all states.
//! - [`skeleton`]: non-interactive placeholder bars and an accessible block.
//! - [`progress`]: determinate bars and rings in fixed slots.
//! - [`indicators`]: indeterminate wave, matrix, mini-glyph and glow indicators driven by the
//!   shared parked pulse clock, and capped count labels.
//! - [`checklist`] and [`section`]: the checklist model (summary, folding, expanded-state
//!   policy, dismissal) and the collapsible status section.
//! - [`motion`]: the pure functions behind all of the above.

mod button;
pub mod checklist;
pub mod indicators;
pub mod loadable;
pub mod motion;
pub mod progress;
pub mod section;
pub mod skeleton;
pub mod theme;

pub use checklist::{
    FOLD_THRESHOLD, Item, ItemStatus, Row, SectionState, SectionStates, Side, Summary, WINDOW,
    focus_window, rows, signature, summary,
};
pub use indicators::{
    IndicatorKind, MatrixSpinner, MiniGlyph, PulseGlow, WaveLoader, capped_label, count_slot_width,
    matrix_spinner, mini_glyph_spinner, pulse_glow, wave_loader, widest_capped_label,
};
pub use loadable::{Loadable, LoadableSlot, Presentation, loadable};
pub use progress::{ProgressBar, ProgressRing, progress_bar, progress_ring};
pub use section::{StatusSection, status_section};
pub use skeleton::{
    BarWidth, SkeletonBar, SkeletonBlock, SkeletonRow, skeleton_bar, skeleton_block, skeleton_rows,
};
