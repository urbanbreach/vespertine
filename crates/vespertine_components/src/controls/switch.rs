//! The native switch (CMP-001 R12-R14).
//!
//! A [`Switch`] exposes role switch and its checked state to assistive technology, takes its
//! accessible name from its label (or an explicit accessible label) and is focusable (tab index
//! 0 by default; disabled switches leave the sequential traversal). It activates on click, on
//! Enter or on Space with the base keyboard-click behavior (once, on release); an activation asks
//! the owner for the opposite state through the change callback. The owner owns the state: the
//! switch is controlled.
//!
//! The thumb position is a scalar from 0 (off) to 1 (on). On first appearance it equals the
//! target; when the checked state changes it travels from its current value to the new target in
//! 180 ms with an ease-out-cubic curve (see [`crate::controls::motion`]), restarts from the
//! current position when the target changes mid-travel, snaps under reduced motion and requests
//! frames only while it differs from the target by more than 0.001. Track and thumb colors
//! change immediately with the state; only the thumb interpolates. The optional label belongs to
//! the activation target and sits after the track.
//!
//! Default metrics: track 36 x 20 px, fully rounded, thumb diameter 16 px, side inset 2 px.

use std::rc::Rc;

use vespertine::{
    App, ClickEvent, ElementId, InteractiveElement, IntoElement, ParentElement, Pixels, RenderOnce,
    Role, SharedString, StatefulInteractiveElement, Styled, Toggled, Window, div, hsla,
    prelude::FluentBuilder, px,
};

use super::{
    ScalarMotion,
    button::{ControlState, FOCUS_EDGE},
    effective_disabled,
    motion::SWITCH_THUMB_DURATION,
    with_opacity,
};
use crate::tokens::ComponentTokens;

/// Metrics of a switch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SwitchMetrics {
    /// Track width.
    pub track_width: Pixels,
    /// Track height.
    pub track_height: Pixels,
    /// Thumb diameter.
    pub thumb: Pixels,
    /// Distance between the thumb and the track edge.
    pub inset: Pixels,
}

impl Default for SwitchMetrics {
    fn default() -> Self {
        Self {
            track_width: px(36.),
            track_height: px(20.),
            thumb: px(16.),
            inset: px(2.),
        }
    }
}

impl SwitchMetrics {
    /// Distance the thumb centre travels between off and on.
    pub fn travel(&self) -> Pixels {
        self.track_width - self.thumb - self.inset * 2.0
    }

    /// Left edge of the thumb inside the track for a thumb position between 0 and 1.
    pub fn thumb_left(&self, position: f32) -> Pixels {
        self.inset + self.travel() * position
    }
}

type ChangeHandler = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// The native switch. See the [module documentation](self).
#[derive(IntoElement)]
pub struct Switch {
    id: ElementId,
    checked: bool,
    on_change: Option<ChangeHandler>,
    label: Option<SharedString>,
    accessible_label: Option<SharedString>,
    description: Option<SharedString>,
    disabled: bool,
    tab_index: isize,
    metrics: SwitchMetrics,
    tokens: Option<ComponentTokens>,
}

impl Switch {
    /// A switch with the given identity, off.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            checked: false,
            on_change: None,
            label: None,
            accessible_label: None,
            description: None,
            disabled: false,
            tab_index: 0,
            metrics: SwitchMetrics::default(),
            tokens: None,
        }
    }

    /// The checked state, owned by the caller.
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    /// Called with the *requested* state (the opposite of the current one) when the user
    /// activates the switch. The switch itself does not change until the owner passes the new
    /// state to [`Switch::checked`].
    pub fn on_change(mut self, handler: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }

    /// The visible label, placed after the track and part of the activation target. It is the
    /// accessible name unless [`Switch::accessible_label`] is given.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// An explicit accessible name.
    pub fn accessible_label(mut self, label: impl Into<SharedString>) -> Self {
        self.accessible_label = Some(label.into());
        self
    }

    /// The accessible description.
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Disables the switch: it keeps its role, label and state in the accessibility tree but
    /// does not activate and leaves the sequential focus traversal.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// The tab index (default 0).
    pub fn tab_index(mut self, index: isize) -> Self {
        self.tab_index = index;
        self
    }

    /// Overrides the metrics (default 36 x 20 track, 16 thumb, 2 inset).
    pub fn metrics(mut self, metrics: SwitchMetrics) -> Self {
        self.metrics = metrics;
        self
    }

    /// Overrides the token roles (the default is [`ComponentTokens::current`]).
    pub fn tokens(mut self, tokens: ComponentTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }
}

impl RenderOnce for Switch {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = self
            .tokens
            .clone()
            .unwrap_or_else(|| ComponentTokens::current(cx));
        let disabled = effective_disabled(self.disabled, cx);
        let metrics = self.metrics;
        let target = if self.checked { 1.0 } else { 0.0 };

        let control = window.use_keyed_state((self.id.clone(), "control"), cx, ControlState::new);
        let focus = control
            .read(cx)
            .focus
            .clone()
            .tab_index(self.tab_index)
            .tab_stop(!disabled);
        let thumb_motion = window.use_keyed_state((self.id.clone(), "thumb"), cx, move |_, _| {
            ScalarMotion::new(target, SWITCH_THUMB_DURATION)
        });
        let position = thumb_motion.update(cx, |motion, cx| motion.advance(target, window, cx));

        let track_fill = if self.checked {
            tokens.accent
        } else {
            tokens.border
        };
        let name = self.accessible_label.clone().or_else(|| self.label.clone());
        let checked = self.checked;
        let on_change = self.on_change;

        let track = div()
            .relative()
            .flex_none()
            .w(metrics.track_width)
            .h(metrics.track_height)
            .rounded_full()
            .bg(track_fill)
            .child(
                div()
                    .absolute()
                    .left(metrics.thumb_left(position))
                    .top((metrics.track_height - metrics.thumb) * 0.5)
                    .size(metrics.thumb)
                    .rounded_full()
                    .bg(tokens.accent_contrast),
            );

        let root = div()
            .id(self.id.clone())
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            // The reserved focus edge, transparent unless focus-visible.
            .border(FOCUS_EDGE)
            .border_color(hsla(0., 0., 0., 0.))
            .rounded((metrics.track_height + FOCUS_EDGE * 2.0) * 0.5)
            .text_color(if disabled {
                with_opacity(tokens.text, tokens.disabled_opacity)
            } else {
                tokens.text
            })
            .text_size(px(13.))
            .when(disabled, |root| root.opacity(tokens.disabled_opacity))
            .role(Role::Switch)
            .aria_toggled(if checked {
                Toggled::True
            } else {
                Toggled::False
            })
            .aria_disabled(disabled)
            .when_some(name, |root, name| root.aria_label(name))
            .when_some(self.description, |root, description| {
                root.aria_description(description)
            })
            .child(track)
            .when_some(self.label, |root, label| root.child(label))
            .track_focus(&focus)
            .focus_visible(|style| style.border_color(tokens.focus_ring));

        // The base keyboard-click: Enter or Space activates once, on release.
        root.when(!disabled, |root| {
            root.cursor_pointer()
                .on_click(move |event: &ClickEvent, window, cx| {
                    if !event.standard_click() {
                        return;
                    }
                    if let Some(on_change) = &on_change {
                        on_change(!checked, window, cx);
                    }
                })
        })
    }
}
