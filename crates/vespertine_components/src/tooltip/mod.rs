//! Tooltips and hover cards (CMP-004): the native half.
//!
//! [`text`] holds the native text tooltips on the base show/hide mechanism; [`hover_card`] holds the dwell/warm-window controller and the interactive hide policy; both
//! are pure state machines driven by a monotonic clock.

pub mod card;
pub mod destination;
pub mod hover_card;
pub mod text;
pub mod trigger;
