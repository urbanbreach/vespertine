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

use vespertine::{
    AnyView, App, AppContext, Bounds, Context, Hsla, IntoElement, ParentElement, Pixels, Render,
    SharedString, Styled, Window, canvas, div, px,
};

use crate::tokens::ComponentTokens;

/// An invisible child that records the bounds its parent has after layout, so event handlers
/// (which do not receive the element's bounds) can map pointer positions onto the control.
pub(crate) fn track_bounds(cell: Rc<Cell<Bounds<Pixels>>>) -> impl IntoElement {
    canvas(move |bounds, _, _| cell.set(bounds), |_, _, _, _| {})
        .absolute()
        .top_0()
        .left_0()
        .size_full()
}

/// The tooltip card of controls with a tooltip text, in token colors. Tooltips do not take
/// keyboard focus. CMP-004 provides the shared tooltip; until components adopt it this card
/// renders the text.
pub(crate) struct TextTooltip {
    text: SharedString,
    tokens: ComponentTokens,
}

impl Render for TextTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.))
            .py(px(4.))
            .rounded(px(4.))
            .border_1()
            .border_color(self.tokens.border_strong)
            .bg(self.tokens.surface_raised)
            .text_color(self.tokens.text)
            .text_size(px(12.))
            .child(self.text.clone())
    }
}

/// A `.tooltip(..)` builder showing `text`.
pub(crate) fn text_tooltip(
    text: SharedString,
    tokens: ComponentTokens,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    move |_, cx| {
        cx.new(|_| TextTooltip {
            text: text.clone(),
            tokens: tokens.clone(),
        })
        .into()
    }
}

/// `color` with its opacity multiplied by `factor`.
pub(crate) fn with_opacity(color: Hsla, factor: f32) -> Hsla {
    Hsla {
        a: color.a * factor,
        ..color
    }
}
