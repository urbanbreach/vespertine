//! Tooltips and hover cards (CMP-004): the native half.
//!
//! [`hover_card`] holds the dwell/warm-window controller and the interactive hide policy; both
//! are pure state machines driven by a monotonic clock.

pub mod hover_card;
