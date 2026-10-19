//! Buttons, switches and sliders (CMP-001).
//!
//! The native half of the activation controls shared by every higher-level component:
//!
//! * [`activation`] — the per-control keyboard activation latch (R2): exactly one activation per
//!   physical press, Enter on key-down, Space on key-up, repeat-proof on X11.
//! * [`disabled_context`] — control groups that disable everything inside them (R11).
//! * [`button`] — the native button with default, hovered, pressed, selected, focused, disabled
//!   and busy states in one fixed box, and the icon-only action (R7-R10).
//! * [`switch`] — the switch with a retargetable 180 ms thumb (R12-R14).
//! * [`slider`] — the continuous or discrete slider: pure normalization and geometry helpers, a
//!   pointer-capture drag with per-frame coalescing, keyboard stepping, reveal-on-interaction
//!   furniture and stop presentation (R15-R21).
//! * [`motion`] — the retargetable scalar the switch and the discrete slider share.
//!
//! All colors come from [`crate::tokens::ComponentTokens`]; the metrics are the themeable
//! defaults of the specification.

pub mod activation;
pub mod button;
pub mod disabled_context;
pub mod motion;
pub mod slider;
pub mod switch;

pub use activation::{ActivationKey, ActivationLatch, KeyOutcome};
pub use button::{Button, SurfaceState};
pub use disabled_context::{DisabledGroup, disabled_group, effective_disabled, group_disabled};
pub use motion::{ScalarMotion, ease_out_cubic};
pub use slider::{Slider, SliderRange};
pub use switch::Switch;

use std::{cell::Cell, rc::Rc};

use vespertine::{Bounds, Hsla, IntoElement, Pixels, Styled, canvas};

/// An invisible child that records the bounds its parent has after layout, so event handlers
/// (which do not receive the element's bounds) can map pointer positions onto the control.
pub(crate) fn track_bounds(cell: Rc<Cell<Bounds<Pixels>>>) -> impl IntoElement {
    canvas(move |bounds, _, _| cell.set(bounds), |_, _, _, _| {})
        .absolute()
        .top_0()
        .left_0()
        .size_full()
}

/// `color` with its opacity multiplied by `factor`.
pub(crate) fn with_opacity(color: Hsla, factor: f32) -> Hsla {
    Hsla {
        a: color.a * factor,
        ..color
    }
}
