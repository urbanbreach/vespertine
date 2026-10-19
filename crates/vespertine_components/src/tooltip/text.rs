//! Native text tooltips on the base show/hide mechanism (CMP-004 R11).
//!
//! The base element layer owns the timing: a per-element show delay (default 500 ms), a tooltip
//! that closes as soon as the pointer leaves a non-hoverable trigger, a hoverable tooltip that
//! stays while the pointer is over the trigger or the tooltip and otherwise hides after 500 ms,
//! and hiding on an outside press or wheel scroll. [`TextTooltip`] adds the token-driven chip, the
//! 320 px wrapping limit and the alternate placement above the pointer with a 20 px clearance so a
//! small control under the pointer is not covered. Passive tooltips do not join the dismissal
//! stack of CMP-002 (R6) and never take keyboard focus.

use crate::tokens::ComponentTokens;
use std::time::Duration;
use vespertine::{
    Anchor, AnyView, App, AppContext, Context, InteractiveElement, IntoElement, ParentElement,
    Pixels, Point, Render, SharedString, StatefulInteractiveElement, Styled, Window, anchored, div,
    point, px,
};

/// Maximum width of a text tooltip; longer text wraps.
pub const MAX_WIDTH: Pixels = px(320.);
/// Clearance between the pointer and an above-pointer tooltip.
pub const ABOVE_POINTER_CLEARANCE: Pixels = px(20.);
/// The base default show delay.
pub const DEFAULT_SHOW_DELAY: Duration = Duration::from_millis(500);
/// Line height of the chip: whole pixels, so the chip's height is whole.
pub const LINE_HEIGHT: Pixels = px(16.);
/// Text size of the chip.
pub const TEXT_SIZE: Pixels = px(12.);

/// Where a text tooltip appears relative to the pointer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Placement {
    /// Below and to the right of the pointer (the base placement).
    #[default]
    AtPointer,
    /// Above the pointer with [`ABOVE_POINTER_CLEARANCE`].
    AbovePointer,
}

/// The tooltip chip.
pub struct TextTooltip {
    text: SharedString,
    tokens: ComponentTokens,
    /// The pointer position captured when the tooltip was built, for the above placement.
    above: Option<Point<Pixels>>,
}

impl TextTooltip {
    /// The chip element for `text`: token-driven, 320 px maximum width, wrapping.
    pub fn chip_for(text: SharedString, tokens: ComponentTokens) -> impl IntoElement {
        div()
            .debug_selector(|| "text-tooltip".to_string())
            .max_w(MAX_WIDTH)
            .px(px(8.))
            .py(px(4.))
            .rounded(px(4.))
            .border_1()
            .border_color(tokens.border)
            .bg(tokens.surface_raised)
            .text_color(tokens.text_muted)
            .text_size(TEXT_SIZE)
            .line_height(LINE_HEIGHT)
            .child(text)
    }

    fn chip(&self) -> impl IntoElement {
        Self::chip_for(self.text.clone(), self.tokens.clone())
    }
}

impl Render for TextTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        match self.above {
            None => self.chip().into_any_element(),
            Some(pointer) => anchored()
                .position(point(pointer.x, pointer.y - ABOVE_POINTER_CLEARANCE))
                .anchor(Anchor::BottomLeft)
                .snap_to_window()
                .child(self.chip())
                .into_any_element(),
        }
    }
}

/// A text tooltip description.
#[derive(Clone, Debug)]
pub struct TooltipSpec {
    text: SharedString,
    placement: Placement,
    hoverable: bool,
    show_delay: Option<Duration>,
}

impl TooltipSpec {
    /// A non-hoverable tooltip at the pointer with the default delay.
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            placement: Placement::AtPointer,
            hoverable: false,
            show_delay: None,
        }
    }

    /// Shows the tooltip above the pointer.
    pub fn above_pointer(mut self) -> Self {
        self.placement = Placement::AbovePointer;
        self
    }

    /// Makes the tooltip hoverable: it stays while the pointer is over it.
    pub fn hoverable(mut self) -> Self {
        self.hoverable = true;
        self
    }

    /// Overrides the per-element show delay.
    pub fn show_delay(mut self, delay: Duration) -> Self {
        self.show_delay = Some(delay);
        self
    }

    fn builder(&self) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        let text = self.text.clone();
        let placement = self.placement;
        move |window, cx| {
            let above = (placement == Placement::AbovePointer).then(|| window.mouse_position());
            let tokens = ComponentTokens::current(cx);
            let text = text.clone();
            cx.new(|_| TextTooltip {
                text,
                tokens,
                above,
            })
            .into()
        }
    }

    /// Attaches the tooltip to an element.
    pub fn apply<E: StatefulInteractiveElement>(self, element: E) -> E {
        let element = match self.show_delay {
            Some(delay) => element.tooltip_show_delay(delay),
            None => element,
        };
        if self.hoverable {
            element.hoverable_tooltip(self.builder())
        } else {
            element.tooltip(self.builder())
        }
    }
}

/// `.text_tooltip("…")` on any stateful interactive element.
pub trait TextTooltipExt: StatefulInteractiveElement + Sized {
    /// Attaches a plain text tooltip.
    fn text_tooltip(self, text: impl Into<SharedString>) -> Self {
        TooltipSpec::new(text).apply(self)
    }

    /// Attaches a described tooltip.
    fn tooltip_spec(self, spec: TooltipSpec) -> Self {
        spec.apply(self)
    }
}

impl<E: StatefulInteractiveElement> TextTooltipExt for E {}
