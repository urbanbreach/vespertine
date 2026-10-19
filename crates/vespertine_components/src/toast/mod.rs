//! Paused timed toast stacks with swipe dismissal (CMP-018).
//!
//! Any code with application access publishes notices ([`show`], [`error`], [`success`],
//! [`info`]); every window mounts one host with [`render_toaster`] at the root of its element
//! tree. Each notice has its own countdown that pauses while the pointer is over the stack or
//! the window is inactive; the collapsed stack shows at most three cards, hovering fans it out,
//! new cards rise and fade in, removed cards fade out and a rightward swipe dismisses a card.
//! The pure model ([`model`]) carries every rule with an explicit clock.

pub mod host;
pub mod model;

pub use host::{
    error, info, live_messages, render_toaster, scheduled_wakeup, show, success, take_error,
    update_queue, with_queue,
};
pub use model::{ToastKind, ToastQueue, WakePlan};
