//! Streaming bottom-follow, anchored scrolling and bidirectional history (CMP-029).
//!
//! A policy layer over the variable-height virtual list for chat-like and log-like content that
//! grows at its end:
//!
//! - [`follow`]: the pure policy (pinned versus detached, the velocity-spring [`Stepper`], long
//!   jumps, reservations, selection autoscroll, the navigation timeline).
//! - [`FollowController`]: the controller attached to a [`vespertine::ListState`].
//! - [`overview`]: the overview rail (markers, buckets, previews, navigation).
//! - [`history`]: the bounded bidirectional paging pattern.

pub mod controller;
pub mod follow;
pub mod history;
pub mod overview;

pub use controller::{Commit, FollowController};
pub use follow::{
    FollowPolicy, NavigationTimeline, PinChange, Stepper, autoscroll_step, list_viewport_height,
    reservation_changed, reserved_height, teleport_excess,
};
pub use history::{
    Direction, History, HistoryCommit, HistoryRow, Landing, Page, ReaderSample, Request, Restore,
};
pub use overview::{
    Marker, OverviewState, Preview, active_marker, bucket_of, bucket_preview, rail_slots,
    rail_visible, reading_row, render_overview_rail, tick_buckets, truncate_preview,
};
