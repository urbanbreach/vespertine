//! Scrollbars (CMP-031).
//!
//! Two independent treatments share one geometry vocabulary:
//!
//! 1. **Classic thumb indicators** are paint-only thumbs of a scrollable element or virtual list,
//!    enabled by the style color [`vespertine::Styled::scrollbar_color`] (and sized by
//!    `scrollbar_width`). They live in the framework core (see [`vespertine::classic_thumb_rects`])
//!    because they are painted by the element itself, in the frame the content scrolls.
//! 2. **Overlay rails** are draggable, auto-hiding strips outside the content clip: [`RailMetrics`]
//!    (geometry), [`RailState`] (visibility policy, press/drag, hide scheduler), the [`RailHost`]
//!    trait and the one-call [`render_rail`] helper.
//!
//! A host picks classic indicators or the overlay, or switches between them according to
//! the platform's auto-hide preference.

mod metrics;
mod rail;
mod state;

pub use metrics::{
    RAIL_HIT_THICKNESS, RAIL_MIN_THUMB, RAIL_THUMB_ACTIVE_THICKNESS, RAIL_THUMB_EDGE_OFFSET,
    RAIL_THUMB_THICKNESS, RAIL_TRACK_INSET, RailMetrics, RailOrientation,
};
pub use rail::{
    HandleRail, ListRail, RailBar, RailHost, begin_press, drag_to, end_press, render_rail,
    track_hover,
};
pub use state::{
    RAIL_FADE_MS, RAIL_FADE_WAKE_MS, RAIL_LINGER_MS, RAIL_OPACITY_ACTIVE, RAIL_OPACITY_REST,
    RAIL_OPACITY_THUMB_HOVER, RailLook, RailReveal, RailState,
};
