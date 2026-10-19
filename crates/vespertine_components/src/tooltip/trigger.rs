//! A tooltip trigger that also discloses for keyboard focus (CMP-004 R2, R3, R6, R10).
//!
//! Hover keeps the base mechanism and timing ([`TooltipSpec`]); this entity adds the rest of the
//! contract: a keyboard-focused trigger (keyboard modality, INP-001) opens the surface at once
//! with no delay and closes it on blur, Escape, a pointer press or a click, while a
//! pointer-initiated focus (a mouse press) does not leave it open. The surface never takes
//! keyboard focus, holds no focus handle and never blurs the focused element, and it is
//! pointer-transparent. While it is open it joins the window's dismissal stack (CMP-002 R6), so
//! Escape reaches it before a menu below it.

use super::text::{TextTooltip, TooltipSpec};
use crate::floating::{
    EscapeHandler, EscapeOutcome, LayerId, TriggerBounds, register_layer, unregister_layer,
};
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

/// A trigger with a hover tooltip (base timing) and a focus-opened surface.
pub struct TooltipTrigger {
    id: ElementId,
    text: SharedString,
    spec: TooltipSpec,
    content: TriggerContent,
    focus: FocusHandle,
    bounds: TriggerBounds,
    layer: Option<LayerId>,
    _subscriptions: Vec<Subscription>,
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
        let focus_in = cx.on_focus_in(&focus, window, |_, window, cx| {
            // Only keyboard modality opens (R3): a mouse press that focuses the trigger does not.
            // Focus listeners run while the frame is finishing: a change they make draws only
            // when it is applied after it.
            if window.last_input_was_keyboard() {
                cx.defer_in(window, |this, window, cx| this.show(window, cx));
            }
        });
        let focus_out = cx.on_focus_out(&focus, window, |_, _, window, cx| {
            cx.defer_in(window, |this, window, cx| this.hide(window, cx));
        });
        Self {
            id: id.into(),
            spec: TooltipSpec::new(text.clone()),
            text,
            content: Rc::new(content),
            focus,
            bounds: TriggerBounds::new(),
            layer: None,
            _subscriptions: vec![focus_in, focus_out],
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
        self.layer.is_some()
    }

    /// Opens the surface now (no delay) and joins the dismissal stack.
    pub fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.layer.is_some() {
            return;
        }
        let view = cx.entity().downgrade();
        let escape: EscapeHandler = Rc::new(move |window, cx| {
            if let Some(view) = view.upgrade() {
                view.update(cx, |this, cx| this.hide(window, cx));
            }
            EscapeOutcome::Dismissed
        });
        self.layer = Some(register_layer(window, cx, escape));
        cx.notify();
        window.refresh();
    }

    /// Closes the surface and leaves the dismissal stack.
    pub fn hide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(layer) = self.layer.take() {
            unregister_layer(window, cx, layer);
            cx.notify();
            window.refresh();
        }
    }

    fn surface(&self, trigger: Bounds<Pixels>, tokens: ComponentTokens) -> AnyElement {
        let chip = TextTooltip::chip_for(self.text.clone(), tokens);
        // Above the trigger, centered, inside the window with the snap margin. Pointer-transparent:
        // no `occlude`, no handlers.
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
        .into_any_element()
    }
}

impl Render for TooltipTrigger {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = ComponentTokens::current(cx);
        let content = (self.content)(window, cx);
        let surface = self
            .layer
            .and_then(|_| self.bounds.get())
            .map(|trigger| self.surface(trigger, tokens));
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
            .child(self.bounds.probe())
            .children(surface);
        self.spec.clone().apply(wrapper)
    }
}
