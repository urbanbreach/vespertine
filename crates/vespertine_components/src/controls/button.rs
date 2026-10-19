//! The native button (CMP-001 R7-R11).
//!
//! A [`Button`] is a focusable control with role button and these states: default, hovered,
//! pressed, selected (toggled on), focused, disabled and busy.
//!
//! * **Precedence** of the surface paint: disabled, then busy, then pressed, then hovered, then
//!   default ([`SurfaceState::resolve`]). Selected and focused are independent overlays.
//! * **One box**: every state occupies the identical box. The button has a reserved border edge
//!   that is transparent in every state except focus-visible, where it takes the focus-ring
//!   color, so focusing never shifts the label or its neighbors; the busy indicator is drawn
//!   inside the content box as an overlay; hover and press change only the fill.
//! * **Focus indicator** only for keyboard-originated focus (INP-001 modality): a control focused
//!   by a mouse press shows none.
//! * **Busy indicator** drawn from the shared activity-indicator clock (ANM-004): it animates
//!   only while painted, shows a uniform brightness cue under system-only reduced motion and is
//!   still under forced reduction.
//! * **Disabled and busy**: a disabled button keeps its role, label and disabled state in the
//!   accessibility tree, leaves sequential focus traversal (unless
//!   [`Button::focusable_when_disabled`], which keeps it focusable but never activatable) and
//!   neither activates by pointer, key or assistive-technology action nor shows hover/pressed
//!   styling. A busy button exposes busy, stays focusable and starts no other activation.
//! * **Activation** follows [`crate::controls::activation`]: pointer click, Enter on the first
//!   non-held key-down and Space on key-up, exactly once per physical press.
//! * **Icon-only** actions are created with [`Button::icon_only`], which requires the
//!   accessible label; the label is also the tooltip text unless [`Button::tooltip_text`] gives
//!   another one. The tooltip is the CMP-004 text tooltip: hover shows it above the pointer after
//!   the default 500 ms ([`TooltipSpec`]), and keyboard focus (keyboard modality) shows it at once
//!   through the button's own [`FocusTooltip`]; it closes on blur, Escape or a press and never
//!   takes keyboard focus.
//! * **Disabled context**: a button inside a [`disabled_group`](super::disabled_group) is
//!   disabled whatever its own flag says.

use std::{cell::Cell, rc::Rc};

use vespertine::{
    ActivityIndicator, AnyElement, App, Bounds, ClickEvent, Context, ElementId, FocusHandle, Hsla,
    InteractiveElement, IntoElement, KeyDownEvent, KeyboardButton, KeyboardClickEvent, MouseButton,
    ParentElement, Pixels, Refineable, RenderOnce, Role, SharedString, StatefulInteractiveElement,
    StyleRefinement, Styled, Subscription, Toggled, Window, div, hsla, prelude::FluentBuilder, px,
};

use super::{ActivationKey, ActivationLatch, effective_disabled, track_bounds, with_opacity};
use crate::tokens::ComponentTokens;
use crate::tooltip::{text::TooltipSpec, trigger::FocusTooltip};

/// Default button height.
pub const BUTTON_HEIGHT: Pixels = px(36.);

/// Width of the reserved focus edge.
pub const FOCUS_EDGE: Pixels = px(2.);

/// The surface paint state of a button. Selected and focused are not part of it: they overlay
/// whichever state is current.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceState {
    /// Disabled: wins over everything else.
    Disabled,
    /// Busy: wins over pressed, hovered and default.
    Busy,
    /// A press is in progress on the control.
    Pressed,
    /// The pointer is over the control.
    Hovered,
    /// None of the above.
    Default,
}

impl SurfaceState {
    /// The paint state for the given inputs: disabled, then busy, then pressed, then hovered,
    /// then default.
    pub fn resolve(disabled: bool, busy: bool, pressed: bool, hovered: bool) -> Self {
        if disabled {
            Self::Disabled
        } else if busy {
            Self::Busy
        } else if pressed {
            Self::Pressed
        } else if hovered {
            Self::Hovered
        } else {
            Self::Default
        }
    }

    /// The fill of the surface in this state, for a selected (toggled on) or plain control.
    pub fn fill(self, tokens: &ComponentTokens, selected: bool) -> Hsla {
        if selected {
            match self {
                Self::Pressed => tokens.accent.blend(hsla(0., 0., 0., 0.2)),
                Self::Hovered => tokens.accent.blend(tokens.highlight),
                _ => tokens.accent,
            }
        } else {
            match self {
                Self::Pressed => tokens.control_surface_pressed,
                Self::Hovered => tokens.control_surface_hover,
                _ => tokens.control_surface,
            }
        }
    }
}

/// State a control keeps across frames: its focus handle, its activation latch and its bounds.
pub(crate) struct ControlState {
    pub(crate) focus: FocusHandle,
    pub(crate) latch: ActivationLatch,
    pub(crate) bounds: Rc<Cell<Bounds<Pixels>>>,
    _blur: Subscription,
}

impl ControlState {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle().tab_stop(true);
        let latch = ActivationLatch::new();
        let blur = latch.watch_blur(&focus, window, cx);
        Self {
            focus,
            latch,
            bounds: Rc::default(),
            _blur: blur,
        }
    }
}

/// The focus-opened tooltip a button with tooltip text keeps across frames.
struct TooltipState {
    tip: FocusTooltip,
}

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// The native button. See the [module documentation](self).
#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: Option<SharedString>,
    children: Vec<AnyElement>,
    accessible_label: Option<SharedString>,
    tooltip: Option<SharedString>,
    disabled: bool,
    busy: bool,
    selected: Option<bool>,
    focusable_when_disabled: bool,
    tab_index: isize,
    on_click: Option<ClickHandler>,
    on_key_down: Option<Rc<dyn Fn(&KeyDownEvent, &mut Window, &mut App)>>,
    icon_only: bool,
    tokens: Option<ComponentTokens>,
    style: StyleRefinement,
}

impl Button {
    /// A button with the given element identity (it keys the button's focus and state).
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            label: None,
            children: Vec::new(),
            accessible_label: None,
            tooltip: None,
            disabled: false,
            busy: false,
            selected: None,
            focusable_when_disabled: false,
            tab_index: 0,
            on_click: None,
            on_key_down: None,
            icon_only: false,
            tokens: None,
            style: StyleRefinement::default(),
        }
    }

    /// An icon-only action. `accessible_label` is required: it names the control for assistive
    /// technology and is the tooltip text unless [`Button::tooltip_text`] overrides it.
    pub fn icon_only(
        id: impl Into<ElementId>,
        accessible_label: impl Into<SharedString>,
        icon: impl IntoElement,
    ) -> Self {
        let mut button = Self::new(id);
        button.accessible_label = Some(accessible_label.into());
        button.icon_only = true;
        button.children.push(icon.into_any_element());
        button
    }

    /// The visible text label, which is also the accessible name unless
    /// [`Button::accessible_label`] is given.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// The accessible name, when it differs from the visible label.
    pub fn accessible_label(mut self, label: impl Into<SharedString>) -> Self {
        self.accessible_label = Some(label.into());
        self
    }

    /// The tooltip text. Without it an icon-only button uses its accessible label and other
    /// buttons show no tooltip.
    pub fn tooltip_text(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    /// Disables the button (see the module documentation).
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Marks the button busy: it exposes the busy state, shows the busy indicator inside its
    /// box, stays focusable and starts no activation.
    pub fn busy(mut self, busy: bool) -> Self {
        self.busy = busy;
        self
    }

    /// Marks the button selected (toggled on) or not. Calling it also exposes the toggled state
    /// to assistive technology.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = Some(selected);
        self
    }

    /// Keeps a disabled button focusable by pointer or programmatically (it still leaves the
    /// sequential traversal and never activates).
    pub fn focusable_when_disabled(mut self, focusable: bool) -> Self {
        self.focusable_when_disabled = focusable;
        self
    }

    /// The tab index of the button (default 0).
    pub fn tab_index(mut self, index: isize) -> Self {
        self.tab_index = index;
        self
    }

    /// The activation callback: called once per pointer click, Enter press or Space press (and
    /// per assistive-technology click action). Keyboard activations arrive as
    /// [`ClickEvent::Keyboard`].
    pub fn on_click(
        mut self,
        callback: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(callback));
        self
    }

    /// A key handler that runs before the internal activation logic and can veto an activation
    /// by preventing the event's default.
    pub fn on_key_down(
        mut self,
        handler: impl Fn(&KeyDownEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_key_down = Some(Rc::new(handler));
        self
    }

    /// Overrides the token roles of the button (the default is
    /// [`ComponentTokens::current`]).
    pub fn tokens(mut self, tokens: ComponentTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }
}

impl ParentElement for Button {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl Styled for Button {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

/// What the busy indicator shows (ANM-004 R4).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum IndicatorFrame {
    /// A trail of brightness running around the dots; `lead` is the fraction of the circle at
    /// which it currently is (0 under forced reduction, which freezes it).
    Spinning(f32),
    /// Under system-only reduced motion: every dot in phase at one uniform opacity, with no
    /// spatial movement.
    Uniform(f32),
}

impl IndicatorFrame {
    /// The frame for the shared activity-indicator phase: two sweeps per 2.4 s period.
    pub(crate) fn from_activity(activity: ActivityIndicator) -> Self {
        if activity.gentle_cue {
            Self::Uniform(activity.opacity)
        } else {
            Self::Spinning((activity.phase * 2.0).fract())
        }
    }

    /// Opacity of dot `index` of `count`.
    pub(crate) fn dot_opacity(self, index: usize, count: usize) -> f32 {
        match self {
            Self::Uniform(opacity) => opacity,
            Self::Spinning(lead) => {
                let behind =
                    (lead * count as f32 - index as f32).rem_euclid(count as f32) / count as f32;
                0.25 + 0.75 * (1.0 - behind)
            }
        }
    }
}

/// Eight dots on a circle drawn inside the content box of a busy control.
fn busy_indicator(size: Pixels, frame: IndicatorFrame, color: Hsla) -> impl IntoElement {
    const DOTS: usize = 8;
    let dot = size * 0.2;
    let radius = (size - dot) * 0.5;
    let center = size * 0.5;
    let mut ring = div().relative().flex_none().size(size);
    for index in 0..DOTS {
        let angle = std::f32::consts::TAU * index as f32 / DOTS as f32;
        ring = ring.child(
            div()
                .absolute()
                .left(center + radius * angle.sin() - dot * 0.5)
                .top(center - radius * angle.cos() - dot * 0.5)
                .size(dot)
                .rounded_full()
                .bg(with_opacity(color, frame.dot_opacity(index, DOTS))),
        );
    }
    ring
}

impl RenderOnce for Button {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = self
            .tokens
            .clone()
            .unwrap_or_else(|| ComponentTokens::current(cx));
        let disabled = effective_disabled(self.disabled, cx);
        let busy = self.busy;
        let interactive = !disabled && !busy;
        let selected = self.selected == Some(true);

        let control = window.use_keyed_state((self.id.clone(), "control"), cx, ControlState::new);
        let (focus, latch, bounds) = {
            let state = control.read(cx);
            (
                state.focus.clone(),
                state.latch.clone(),
                state.bounds.clone(),
            )
        };

        let rest = SurfaceState::resolve(disabled, busy, false, false);
        let text_color = if selected {
            tokens.accent_contrast
        } else {
            tokens.text
        };
        let name = self.accessible_label.clone().or_else(|| self.label.clone());
        let tooltip = self.tooltip.clone().or_else(|| {
            self.icon_only
                .then(|| self.accessible_label.clone())
                .flatten()
        });

        // The tooltip: hover through the CMP-004 text tooltip (above the pointer, so the control
        // under it stays uncovered), keyboard focus through the button's own focus-opened surface.
        let tip = tooltip.as_ref().map(|_| {
            window.use_keyed_state((self.id.clone(), "tooltip"), cx, |window, cx| {
                TooltipState {
                    tip: FocusTooltip::new(&focus, window, cx, |state: &mut TooltipState| {
                        &mut state.tip
                    }),
                }
            })
        });
        let tip_parts = tip.as_ref().zip(tooltip.as_ref()).map(|(state, text)| {
            let state = state.read(cx);
            (
                state.tip.probe().into_any_element(),
                state.tip.surface(text.clone(), tokens.clone()),
            )
        });

        // The busy indicator draws from the shared activity-indicator clock (ANM-004): it
        // renews a lease only while painted, parks otherwise and honors reduced motion.
        let show_indicator = busy && !disabled;
        let indicator =
            show_indicator.then(|| IndicatorFrame::from_activity(window.activity_indicator(cx)));

        let mut content = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .gap(px(6.))
            .when(show_indicator, |content| content.opacity(0.35))
            .when_some(self.label.clone(), |content, label| content.child(label));
        content.extend(self.children);

        let on_click = self.on_click;
        let activate = {
            let bounds = bounds.clone();
            let on_click = on_click.clone();
            move |key: ActivationKey, window: &mut Window, cx: &mut App| {
                if let Some(on_click) = &on_click {
                    let event = ClickEvent::Keyboard(KeyboardClickEvent {
                        button: match key {
                            ActivationKey::Enter => KeyboardButton::Enter,
                            ActivationKey::Space => KeyboardButton::Space,
                        },
                        bounds: bounds.get(),
                    });
                    on_click(&event, window, cx);
                }
            }
        };

        let mut button = div()
            .id(self.id.clone())
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .h(BUTTON_HEIGHT)
            .px(px(12.))
            .when(self.icon_only, |button| button.w(BUTTON_HEIGHT).px(px(0.)))
            .rounded(tokens.button_radius)
            // The reserved focus edge: transparent unless focus-visible.
            .border(FOCUS_EDGE)
            .border_color(hsla(0., 0., 0., 0.))
            .bg(rest.fill(&tokens, selected))
            .text_color(text_color)
            .text_size(px(13.))
            .when(disabled, |button| button.opacity(tokens.disabled_opacity))
            .role(Role::Button)
            .aria_disabled(disabled)
            .aria_busy(busy)
            .when_some(name, |button, name| button.aria_label(name))
            .when_some(self.selected, |button, selected| {
                button.aria_toggled(if selected {
                    Toggled::True
                } else {
                    Toggled::False
                })
            })
            .when_some(tooltip, |button, text| {
                TooltipSpec::new(text)
                    .above_pointer()
                    .tokens(tokens.clone())
                    .apply(button)
            })
            .when_some(tip, |button, state| {
                // A pointer press closes the focus-opened surface (R3).
                button.on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    state.update(cx, |state, cx| state.tip.hide(window, cx));
                })
            })
            .child(content)
            .child(track_bounds(bounds))
            .when_some(tip_parts, |button, (probe, surface)| {
                button.child(probe).children(surface)
            });

        if let Some(frame) = indicator {
            button = button.child(
                div()
                    .size_full()
                    .absolute()
                    .top_0()
                    .left_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(busy_indicator(px(14.), frame, text_color)),
            );
        }

        if !disabled || self.focusable_when_disabled {
            let focus = focus.tab_index(self.tab_index).tab_stop(!disabled);
            button = button
                .track_focus(&focus)
                .focus_visible(|style| style.border_color(tokens.focus_ring));
        }

        if interactive {
            button = button
                .cursor_pointer()
                .hover(|style| style.bg(SurfaceState::Hovered.fill(&tokens, selected)))
                .active(|style| style.bg(SurfaceState::Pressed.fill(&tokens, selected)));
            if let Some(on_click) = on_click {
                button = button.on_click(move |event, window, cx| {
                    // Keyboard activation belongs to the latch; the base keyboard-click of the
                    // same press must not activate a second time.
                    if matches!(event, ClickEvent::Keyboard(_)) || !event.standard_click() {
                        return;
                    }
                    on_click(event, window, cx);
                });
            }
        }

        if let Some(on_key_down) = self.on_key_down {
            button = button.on_key_down(move |event, window, cx| on_key_down(event, window, cx));
        }
        button.style().refine(&self.style);
        latch.bind(button, interactive, activate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_precedence_is_disabled_then_busy_then_pressed_then_hovered_then_default() {
        use SurfaceState::*;
        for (disabled, busy, pressed, hovered, expected) in [
            (true, true, true, true, Disabled),
            (true, false, true, true, Disabled),
            (false, true, true, true, Busy),
            (false, true, false, true, Busy),
            (false, false, true, true, Pressed),
            (false, false, true, false, Pressed),
            (false, false, false, true, Hovered),
            (false, false, false, false, Default),
        ] {
            assert_eq!(
                SurfaceState::resolve(disabled, busy, pressed, hovered),
                expected,
                "disabled={disabled} busy={busy} pressed={pressed} hovered={hovered}"
            );
        }
    }

    #[test]
    fn selected_fill_is_the_accent_and_keeps_hover_and_press_distinguishable() {
        let tokens = ComponentTokens::neutral();
        assert_eq!(SurfaceState::Default.fill(&tokens, true), tokens.accent);
        assert_eq!(SurfaceState::Busy.fill(&tokens, true), tokens.accent);
        assert_eq!(SurfaceState::Disabled.fill(&tokens, true), tokens.accent);
        let hovered = SurfaceState::Hovered.fill(&tokens, true);
        let pressed = SurfaceState::Pressed.fill(&tokens, true);
        assert_ne!(hovered, tokens.accent);
        assert_ne!(pressed, tokens.accent);
        assert_ne!(hovered, pressed);
        assert_eq!(
            SurfaceState::Pressed.fill(&tokens, false),
            tokens.control_surface_pressed
        );
        assert_eq!(
            SurfaceState::Hovered.fill(&tokens, false),
            tokens.control_surface_hover
        );
        assert_eq!(
            SurfaceState::Default.fill(&tokens, false),
            tokens.control_surface
        );
    }

    #[test]
    fn busy_indicator_spins_normally_and_pulses_uniformly_under_the_gentle_cue() {
        let spinning = IndicatorFrame::from_activity(ActivityIndicator {
            phase: 0.25,
            gentle_cue: false,
            opacity: 1.0,
        });
        assert_eq!(spinning, IndicatorFrame::Spinning(0.5));
        let opacities: Vec<f32> = (0..8).map(|index| spinning.dot_opacity(index, 8)).collect();
        assert!(
            opacities.windows(2).any(|pair| pair[0] != pair[1]),
            "a trail"
        );
        assert!(
            opacities
                .iter()
                .all(|opacity| (0.25..=1.0).contains(opacity))
        );
        // Under system-only reduction every dot has the same opacity: no spatial chase.
        let cue = IndicatorFrame::from_activity(ActivityIndicator {
            phase: 0.5,
            gentle_cue: true,
            opacity: 0.8,
        });
        assert!((0..8).all(|index| cue.dot_opacity(index, 8) == 0.8));
        // Frozen (forced reduction): the trail at phase zero, still.
        let frozen = IndicatorFrame::from_activity(ActivityIndicator {
            phase: 0.0,
            gentle_cue: false,
            opacity: 1.0,
        });
        assert_eq!(frozen, IndicatorFrame::Spinning(0.0));
    }
}
