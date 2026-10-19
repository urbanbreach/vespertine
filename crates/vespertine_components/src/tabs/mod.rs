//! Closable, reorderable tabs and reorderable groups (CMP-017).
//!
//! [`TabStrip`] is a horizontally scrolling strip of tabs with stable [`TabId`]s, active
//! selection, keyboard cycling ([`NextTab`], [`PreviousTab`]), close buttons that share a slot
//! with an unsaved indicator, optional context menus, pointer-derived drag reordering with
//! sliding insertion feedback, a predictable surviving selection after a close
//! ([`ClosePolicy`]) and edge-hidden state that drives an overflow fade. The pure helpers of
//! [`model`] (`cycle_target`, `drop_index`, `slide_offset`, ...) carry the analytic rules; the
//! [`group`] module reuses the identity and insertion-index concepts for reorderable vertical
//! groups and pinned lists ([`ReorderableGroup`]).
//!
//! The strip owns display order, selection and visit history; the owner owns the content of
//! every tab by identity, so reordering, closing another tab, scrolling or switching never
//! recreates a surviving tab's content or its focus state.

pub mod chip;
pub mod group;
pub mod model;
pub mod strip;

pub use chip::{
    CLOSE_LABEL, TabChip, TabIcon, TabSpec, UNSAVED_SUFFIX, close_slot_opacity, tab_chip,
    tab_strip_region,
};
pub use group::{
    GroupLayout, Move, ReorderableGroup, drop_unknown, group_drag_scroll_delta,
    reorder_visible_pins, row_drop_index, section_drop_index,
};
pub use model::{
    CloseDecision, ClosePolicy, Slide, SlideSet, TabId, VisitHistory, active_after_close,
    active_after_reorder, cycle_target, drop_index, ease_out, edge_hidden, reorder, reveal_offset,
    slide_offset,
};
pub use strip::{NextTab, PreviousTab, TabDrag, TabStrip, bind_keys, cycle_on};
