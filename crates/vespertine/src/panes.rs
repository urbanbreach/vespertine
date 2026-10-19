//! Collapsible, resizable pane composition (LAY-004).
//!
//! A [`PaneGroup`] arranges one main content region with collapsible leading
//! and trailing panes and one bottom-docked pane. Per-pane preferences live in
//! a [`PaneGroupState`] entity that callers observe and set; [`fit`] maps a
//! viewport and pane states to visible panes and size limits; [`ResizeHandle`]
//! seams drag with direct pointer tracking; slot changes animate with
//! retargetable transitions; and [`MovingSurface`] lets an input travel
//! without leaving its layout slot.

mod fit;
mod group;
mod handle;
mod state;
mod surface;

pub use fit::*;
pub use group::*;
pub use handle::*;
pub use state::*;
pub use surface::*;

#[cfg(test)]
mod tests;
