//! Closable, reorderable tabs (CMP-017).
//!
//! [`TabStrip`] is a horizontally scrolling strip of tabs with stable [`TabId`]s, active
//! selection, keyboard cycling ([`NextTab`], [`PreviousTab`]), close buttons that share a slot
//! with an unsaved indicator, optional context menus, pointer-derived drag reordering with
//! sliding insertion feedback, a predictable surviving selection after a close
//! ([`ClosePolicy`]) and edge-hidden state that drives an overflow fade. The pure helpers of
//! [`model`] (`cycle_target`, `drop_index`, `slide_offset`, ...) carry the analytic rules.
//!
//! The strip owns display order, selection and visit history; the owner owns the content of
//! every tab by identity, so reordering, closing another tab, scrolling or switching never
//! recreates a surviving tab's content or its focus state.

pub mod chip;
pub mod model;
pub mod strip;

pub use chip::{
    CLOSE_LABEL, TabChip, TabIcon, TabSpec, UNSAVED_SUFFIX, close_slot_opacity, tab_chip,
    tab_strip_region,
};
pub use model::{
    CloseDecision, ClosePolicy, Slide, SlideSet, TabId, VisitHistory, active_after_close,
    cycle_target, drop_index, ease_out, edge_hidden, reorder, reveal_offset, slide_offset,
};
pub use strip::{NextTab, PreviousTab, TabDrag, TabStrip, bind_keys, cycle_on};
