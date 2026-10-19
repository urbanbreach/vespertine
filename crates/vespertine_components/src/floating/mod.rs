//! Floating surfaces, anchored menus, popovers and dismissal layers (CMP-002).
//!
//! The foundation under every popup control. The pure, reusable rules live in their own
//! modules and carry no framework state:
//!
//! * [`stepping`]: keyboard stepping over lists with unavailable rows (R23).
//! * [`keys`]: key classification shared by menus, selects, comboboxes and pickers (R22).
//! * [`rank`]: search match rank and filter-and-rank returning original indices (R25).
//! * [`hover_intent`]: submenu hover intent with the pointer corridor (R26, R27).
//! * [`placement`]: contained placement inside caller-supplied bounds (R18, R19).
//! * [`controlled`]: the controlled-state helper (R12).

pub mod controlled;
pub mod hover_intent;
pub mod keys;
pub mod placement;
pub mod rank;
pub mod stepping;

pub use controlled::ControlledState;
pub use hover_intent::{
    CORRIDOR_TOLERANCE, HOVER_GRACE, HoverIntent, IntentAnswer, OpenSubmenu, RENEW_THRESHOLD,
    TURN_AWAY_THRESHOLD, in_corridor,
};
pub use keys::{ListKey, classify_key};
pub use placement::{
    CONTAINED_GAP, CONTAINMENT_INSET, ContainedAlign, ContainedPlacement, MAX_CONTAINED_HEIGHT,
    contained_max_size, containment_limits, place_contained, rows_max_height,
};
pub use rank::{filter_and_rank, match_rank};
pub use stepping::{Edge, StepDirection, edge_available, step_available, step_slice};
