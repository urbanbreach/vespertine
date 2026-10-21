//! A tooltip that also discloses for keyboard focus (CMP-004 R2, R3, R6, R10).
//!
//! Hover keeps the base mechanism and timing ([`TooltipSpec`]); [`FocusTooltip`] adds the rest of
//! the contract: a keyboard-focused trigger (keyboard modality, INP-001) opens the surface at once
//! with no delay and closes it on blur, Escape, a pointer press or a click, while a
//! pointer-initiated focus (a mouse press) does not leave it open. The surface never takes
//! keyboard focus, holds no focus handle and never blurs the focused element, and it is
//! pointer-transparent. While it is open it joins the window's dismissal stack (CMP-002 R6), so
//! Escape reaches it before a menu below it.
//!
//! [`FocusTooltip`] is the reusable piece: a control that already owns a focus handle and a keyed
//! state (the native button) embeds one and renders its surface without an extra wrapper element.
//! [`TooltipTrigger`] is the ready-made wrapper entity for any other element.

use super::text::{TextTooltip, TooltipSpec};
use crate::floating::{EscapeHandler, EscapeOutcome, LayerRegistration, TriggerBounds};
use crate::tokens::ComponentTokens;
use std::rc::Rc;
use vespertine::{
    Anchor, AnyElement, App, Bounds, Context, ElementId, FocusHandle, InteractiveElement,
    IntoElement, MouseButton, ParentElement, Pixels, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Window, anchored, deferred, div, point, px,
};

/// Clearance between the trigger and the surface.
pub const CLEARANCE: Pixels = px(6.);
/// Layer priority of the focus-opened surface: above menus (1) and nested menus (2).
pub const PRIORITY: usize = 3;
/// Snap margin to the window edges.
pub const WINDOW_MARGIN: Pixels = px(8.);

/// The element a [`TooltipTrigger`] wraps, built at every render.
pub type TriggerContent = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;

/// The focus-opened half of a tooltip: state and subscriptions for one trigger.
///
/// The owner stores it in its own entity, passes an accessor to it ([`FocusTooltip::new`]) and
/// renders [`FocusTooltip::probe`] (a zero-flow child of the trigger that records its bounds)
/// together with [`FocusTooltip::surface`] (nothing while closed) as children of the trigger. A
/// pointer press on the trigger calls [`FocusTooltip::hide`].
pub struct FocusTooltip {
    bounds: TriggerBounds,
    layer: Option<LayerRegistration>,
    escape: EscapeHandler,
    _subscriptions: Vec<Subscription>,
}

impl FocusTooltip {
    /// Watches `focus` (focus on it or on any descendant counts as focus on the trigger) for the
    /// owner entity `T`, which stores the returned value where `this` finds it.
    pub fn new<T: 'static>(
        focus: &FocusHandle,
        window: &mut Window,
        cx: &mut Context<T>,
        this: fn(&mut T) -> &mut FocusTooltip,
    ) -> Self {
        let focus_in = cx.on_focus_in(focus, window, move |_, window, cx| {
            // Only keyboard modality opens (R3): a mouse press that focuses the trigger does not.
            // Focus listeners run while the frame is finishing: a change they make draws only
            // when it is applied after it.
            if window.last_input_was_keyboard() {
                cx.defer_in(window, move |owner, window, cx| {
                    this(owner).show(window, cx)
                });
            }
        });
        let focus_out = cx.on_focus_out(focus, window, move |_, _, window, cx| {
            cx.defer_in(window, move |owner, window, cx| {
                this(owner).hide(window, cx)
            });
        });
        let owner = cx.weak_entity();
        let escape: EscapeHandler = Rc::new(move |window, cx| {
            if let Some(owner) = owner.upgrade() {
                owner.update(cx, |owner, cx| this(owner).hide(window, cx));
            }
            EscapeOutcome::Dismissed
        });
        Self {
            bounds: TriggerBounds::new(),
            layer: None,
            escape,
            _subscriptions: vec![focus_in, focus_out],
        }
    }

    /// Whether the surface is shown.
    pub fn is_open(&self) -> bool {
        self.layer.is_some()
    }

    /// Opens the surface now (no delay) and joins the dismissal stack.
    pub fn show<T: 'static>(&mut self, window: &mut Window, cx: &mut Context<T>) {
        if self.layer.is_some() {
            return;
        }
        self.layer = Some(LayerRegistration::new(window, cx, self.escape.clone()));
        cx.notify();
        window.refresh();
    }

    /// Closes the surface and leaves the dismissal stack.
    pub fn hide<T: 'static>(&mut self, window: &mut Window, cx: &mut Context<T>) {
        // Dropping the registration unregisters the layer.
        if self.layer.take().is_some() {
            cx.notify();
            window.refresh();
        }
    }

    /// A zero-flow child to place inside the trigger element: it records the trigger's bounds,
    /// which place the surface.
    pub fn probe(&self) -> impl IntoElement + use<'_> {
        self.bounds.probe()
    }

    /// The surface for `text` while open: above the trigger, centered, inside the window with the
    /// snap margin, pointer-transparent (no `occlude`, no handlers) and without a focus handle.
    pub fn surface(&self, text: SharedString, tokens: ComponentTokens) -> Option<AnyElement> {
        let trigger: Bounds<Pixels> = self.layer.as_ref().and_then(|_| self.bounds.get())?;
        let chip = TextTooltip::chip_for(text, tokens);
        Some(
            deferred(
                anchored()
                    .anchor(Anchor::BottomCenter)
                    .position(point(
                        trigger.origin.x + trigger.size.width / 2.,
                        trigger.origin.y,
                    ))
                    .offset(point(px(0.), -CLEARANCE))
                    .snap_to_window_with_margin(WINDOW_MARGIN)
                    .child(chip),
            )
            .with_priority(PRIORITY)
            .into_any_element(),
        )
    }
}

/// A trigger with a hover tooltip (base timing) and a focus-opened surface.
pub struct TooltipTrigger {
    id: ElementId,
    text: SharedString,
    spec: TooltipSpec,
    content: TriggerContent,
    focus: FocusHandle,
    tip: FocusTooltip,
}

impl TooltipTrigger {
    /// A trigger showing `text`. `content` builds the wrapped element; focus inside it (the
    /// wrapper's own focus handle or any descendant's) counts as focus on the trigger.
    pub fn new(
        id: impl Into<ElementId>,
        text: impl Into<SharedString>,
        content: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        let text = text.into();
        let tip = FocusTooltip::new(&focus, window, cx, |this: &mut Self| &mut this.tip);
        Self {
            id: id.into(),
            spec: TooltipSpec::new(text.clone()),
            text,
            content: Rc::new(content),
            focus,
            tip,
        }
    }

    /// Replaces the hover description (delay, placement, hoverability).
    pub fn spec(mut self, spec: TooltipSpec) -> Self {
        self.spec = spec;
        self
    }

    /// The wrapper's focus handle; focusing it (or a descendant) is focusing the trigger.
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// Whether the focus-opened surface is shown.
    pub fn is_open(&self) -> bool {
        self.tip.is_open()
    }

    /// Opens the surface now (no delay) and joins the dismissal stack.
    pub fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tip.show(window, cx);
    }

    /// Closes the surface and leaves the dismissal stack.
    pub fn hide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tip.hide(window, cx);
    }
}

impl Render for TooltipTrigger {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = ComponentTokens::current(cx);
        let content = (self.content)(window, cx);
        let surface = self.tip.surface(self.text.clone(), tokens);
        let wrapper = div()
            .id(self.id.clone())
            .relative()
            .track_focus(&self.focus)
            // A pointer press or click closes the surface (R3).
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.hide(window, cx)),
            )
            .on_click(cx.listener(|this, _, window, cx| this.hide(window, cx)))
            .child(content)
            .child(self.tip.probe())
            .children(surface);
        self.spec.clone().apply(wrapper)
    }
}
