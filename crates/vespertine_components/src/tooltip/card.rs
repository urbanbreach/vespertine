//! The hover-card host: dwell, warm window, interactive content and contained placement
//! (CMP-004 R12-R14).
//!
//! One [`HoverCardHost`] entity serves every trigger of a container. Triggers are wrapped with
//! [`HoverCardHost::trigger`]; the container renders [`HoverCardHost::layer`] once. The host owns
//! the timers (the dwell and the interactive hide grace) and drives the pure controllers
//! [`HoverCards`] and [`InteractiveHide`], so the timing rules are the ones tested there. The
//! card never takes keyboard focus and holds no focus handle, so showing or hiding it never
//! blurs the focused element, an embedded page or an editor.

use super::hover_card::{HideAnswer, HoverCards, InteractiveHide, Reaction};
use crate::floating::{
    AnchoredMenu, BESIDE_OFFSET, ContainedAlign, Placement, TriggerBounds, containment_limits,
};
use crate::tokens::ComponentTokens;
use std::{collections::HashMap, hash::Hash, rc::Rc, time::Duration, time::Instant};
use vespertine::{
    AnyElement, App, Bounds, Context, Div, Entity, InteractiveElement, IntoElement, MouseMoveEvent,
    ParentElement, Pixels, Point, Render, ScrollWheelEvent, Stateful, StatefulInteractiveElement,
    Styled, Task, Window, canvas, div, prelude::FluentBuilder, px,
};

/// Builds the content of the card of a trigger key.
pub type CardContent<K> = Rc<dyn Fn(&K, &mut Window, &mut App) -> AnyElement>;

/// Where the card goes relative to its trigger.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CardPlacement {
    /// Below the trigger, contained inside the window (flips above when it does not fit).
    Below,
    /// Beside the trigger (right, or left when `left`), `clearance` px away.
    Beside {
        /// Opens to the left of the trigger.
        left: bool,
        /// Distance from the trigger.
        clearance: Pixels,
    },
}

/// The hover-card controller entity.
pub struct HoverCardHost<K: Clone + Eq + Hash + 'static> {
    cards: HoverCards<K>,
    hide: InteractiveHide,
    triggers: HashMap<K, TriggerBounds>,
    content: CardContent<K>,
    interactive: bool,
    placement: CardPlacement,
    max_height: Pixels,
    dwell: Option<Task<()>>,
    grace: Option<Task<()>>,
    card_bounds: TriggerBounds,
    origin: Instant,
    last_pointer: Point<Pixels>,
}

impl<K: Clone + Eq + Hash + 'static> HoverCardHost<K> {
    /// A host whose cards show `content(key)`.
    pub fn new(
        content: impl Fn(&K, &mut Window, &mut App) -> AnyElement + 'static,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            cards: HoverCards::new(),
            hide: InteractiveHide::new(),
            triggers: HashMap::new(),
            content: Rc::new(content),
            interactive: false,
            placement: CardPlacement::Below,
            max_height: px(320.),
            dwell: None,
            grace: None,
            card_bounds: TriggerBounds::new(),
            origin: cx.background_executor().now(),
            last_pointer: Point::default(),
        }
    }

    /// A card with interactive content is hoverable: leaving the trigger starts the 80 ms hide
    /// grace unless the pointer travels the corridor toward the card (R14).
    pub fn interactive(mut self, interactive: bool) -> Self {
        self.interactive = interactive;
        self
    }

    /// Where the card goes (default: below, contained).
    pub fn placement(mut self, placement: CardPlacement) -> Self {
        self.placement = placement;
        self
    }

    /// The card's maximum height; longer content scrolls inside it (default 320 px).
    pub fn max_height(mut self, height: Pixels) -> Self {
        self.max_height = height;
        self
    }

    /// The trigger whose card is shown.
    pub fn shown(&self) -> Option<&K> {
        self.cards.shown()
    }

    /// The trigger with a pending dwell.
    pub fn target(&self) -> Option<&K> {
        self.cards.target()
    }

    /// The painted bounds of the shown card.
    pub fn card_bounds(&self) -> Option<Bounds<Pixels>> {
        self.card_bounds.get()
    }

    fn now(&self, cx: &App) -> Duration {
        cx.background_executor()
            .now()
            .saturating_duration_since(self.origin)
    }

    /// Wraps `trigger` as the hover trigger of `key`. The trigger must be a positioned element
    /// (relative or absolute): its painted bounds are recorded by a probe child.
    ///
    /// Entering the trigger starts the dwell (or shows at once when warm): leaving hides, a press or a wheel scroll dismisses cold.
    pub fn trigger(
        host: &Entity<Self>,
        key: K,
        trigger: Stateful<Div>,
        cx: &mut App,
    ) -> Stateful<Div> {
        let bounds = host.update(cx, |this, _| {
            this.triggers.entry(key.clone()).or_default().clone()
        });
        let on_hover = {
            let host = host.clone();
            let key = key.clone();
            move |hovered: &bool, window: &mut Window, cx: &mut App| {
                let key = key.clone();
                let hovered = *hovered;
                host.update(cx, |this, cx| {
                    if hovered {
                        this.enter(&key, window, cx);
                    } else {
                        this.leave(&key, window, cx);
                    }
                });
            }
        };
        let dismiss = {
            let host = host.clone();
            move |window: &mut Window, cx: &mut App| {
                host.update(cx, |this, cx| this.dismiss_cold(window, cx));
            }
        };
        let press = dismiss.clone();
        trigger
            .on_hover(on_hover)
            .on_mouse_down(vespertine::MouseButton::Left, move |_, window, cx| {
                press(window, cx)
            })
            .on_scroll_wheel(move |_, window, cx| dismiss(window, cx))
            .child(bounds.probe())
    }

    /// The pointer entered `key`'s trigger.
    pub fn enter(&mut self, key: &K, window: &mut Window, cx: &mut Context<Self>) {
        self.last_pointer = window.mouse_position();
        if let Some(bounds) = self.triggers.get(key) {
            if self.interactive && bounds.contains(&self.last_pointer) {
                self.hide.record_origin(self.last_pointer);
            }
        }
        if self.interactive {
            self.hide.trigger_entered();
            self.grace = None;
        }
        let now = self.now(cx);
        match self.cards.enter(now, key, false) {
            Reaction::None => {}
            Reaction::Show(_) => {
                self.dwell = None;
                self.after_show(cx);
            }
            Reaction::StartDwell { target, due } => {
                let wait = due.saturating_sub(now);
                self.dwell = Some(cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(wait).await;
                    this.update_in(cx, |this, window, cx| this.settle(&target, window, cx))
                        .ok();
                }));
            }
            Reaction::Hide(_) => {}
        }
        cx.notify();
    }

    /// The dwell timer fired: the card shows when the pointer is still on `key`'s trigger. The
    /// hit test against the trigger's last painted bounds also cancels a dwell whose trigger moved
    /// or was clipped under a stationary pointer (R13).
    fn settle(&mut self, key: &K, window: &mut Window, cx: &mut Context<Self>) {
        let pointer = window.mouse_position();
        let still_on = self
            .triggers
            .get(key)
            .is_some_and(|bounds| bounds.contains(&pointer));
        if !still_on {
            self.cards.leave(self.now(cx), key);
            return;
        }
        let now = self.now(cx);
        if let Reaction::Show(_) = self.cards.settle(now, key) {
            self.after_show(cx);
            cx.notify();
        }
    }

    fn after_show(&mut self, _cx: &mut Context<Self>) {
        self.grace = None;
    }

    /// The pointer left `key`'s trigger.
    pub fn leave(&mut self, key: &K, window: &mut Window, cx: &mut Context<Self>) {
        let pointer = window.mouse_position();
        self.last_pointer = pointer;
        let now = self.now(cx);
        if self.cards.shown() == Some(key) && self.interactive {
            let card = self.card_bounds.get();
            let left = matches!(self.placement, CardPlacement::Beside { left: true, .. });
            match card {
                Some(card) => {
                    let answer = self.hide.trigger_left(now, pointer, card, left);
                    self.arm_grace(answer, window, cx);
                }
                None => self.hide_now(key, cx),
            }
            return;
        }
        if self.cards.target() == Some(key) {
            self.dwell = None;
        }
        if let Reaction::Hide(_) = self.cards.leave(now, key) {
            cx.notify();
        }
    }

    fn arm_grace(&mut self, answer: HideAnswer, window: &mut Window, cx: &mut Context<Self>) {
        match answer {
            HideAnswer::StartGrace => {
                let due = self.hide.grace_due().unwrap_or_default();
                let wait = due.saturating_sub(self.now(cx));
                self.grace = Some(cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(wait).await;
                    this.update(cx, |this, cx| {
                        let now = this.now(cx);
                        if this.hide.expired(now) {
                            if let Some(key) = this.cards.shown().cloned() {
                                this.hide_now(&key, cx);
                            }
                        }
                    })
                    .ok();
                }));
            }
            HideAnswer::Close => {
                self.grace = None;
                if let Some(key) = self.cards.shown().cloned() {
                    self.hide_now(&key, cx);
                }
            }
            HideAnswer::CancelGrace => self.grace = None,
            HideAnswer::None => {}
        }
    }

    fn hide_now(&mut self, key: &K, cx: &mut Context<Self>) {
        let now = self.now(cx);
        self.grace = None;
        self.cards.leave(now, key);
        cx.notify();
    }

    fn pointer_moved(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.last_pointer = position;
        if !self.interactive || self.cards.shown().is_none() {
            return;
        }
        let Some(card) = self.card_bounds.get() else {
            return;
        };
        let left = matches!(self.placement, CardPlacement::Beside { left: true, .. });
        let now = self.now(cx);
        let answer = self.hide.moved(now, position, card, left);
        self.arm_grace(answer, window, cx);
    }

    /// A click, menu opening, drag start or scroll: cancel pending, hide the shown card and
    /// clear hidden-at, so the card does not chase the pointer straight back.
    pub fn dismiss_cold(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.dwell = None;
        self.grace = None;
        if let Reaction::Hide(_) = self.cards.dismiss_cold() {
            cx.notify();
        }
    }

    /// Render-time reconciliation for the container (R13): dismisses cold a card whose trigger is
    /// not mounted or visible according to `trigger_visible`, while `editing`, or during a drag.
    pub fn reconcile(
        &mut self,
        trigger_visible: impl Fn(&K) -> bool,
        editing: bool,
        drag_active: bool,
        cx: &mut Context<Self>,
    ) {
        if let Reaction::Hide(_) = self.cards.reconcile(trigger_visible, editing, drag_active) {
            self.dwell = None;
            self.grace = None;
            cx.notify();
        }
    }

    fn card_body(&self, key: &K, window: &mut Window, cx: &mut App) -> AnyElement {
        let tokens = ComponentTokens::current(cx);
        let content = (self.content)(key, window, cx);
        let viewport_height = f32::from(window.viewport_size().height);
        let max_height = px(f32::from(self.max_height).min((viewport_height - 16.).max(1.)));
        div()
            .id("hover-card")
            .max_h(max_height)
            .overflow_y_scroll()
            .px(px(10.))
            .py(px(8.))
            .rounded(px(6.))
            .border_1()
            .border_color(tokens.border_strong)
            .bg(tokens.surface_raised)
            .text_color(tokens.text)
            .child(content)
            .into_any_element()
    }
}

impl<K: Clone + Eq + Hash + 'static> Render for HoverCardHost<K> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(key) = self.cards.shown().cloned() else {
            return div().into_any_element();
        };
        let Some(trigger) = self.triggers.get(&key).cloned() else {
            return div().into_any_element();
        };
        let body = self.card_body(&key, window, cx);
        let placement = match self.placement {
            CardPlacement::Below => Placement::Contained {
                limits: containment_limits(Bounds::new(Point::default(), window.viewport_size())),
                align: ContainedAlign::Start,
            },
            CardPlacement::Beside { left, clearance } => Placement::Beside {
                left,
                offset: if clearance > px(0.) {
                    clearance
                } else {
                    BESIDE_OFFSET
                },
            },
        };
        let host = cx.entity();
        let tracker = self.card_bounds.clone();
        let interactive = self.interactive;
        let mut card = AnchoredMenu::new("hover-card-layer", &trigger, placement)
            .p(px(0.))
            .bg(vespertine::transparent_black())
            .child(
                div()
                    .id("hover-card-frame")
                    .when(interactive, {
                        let host = host.clone();
                        move |frame| {
                            frame.on_hover(move |hovered, window, cx| {
                                let hovered = *hovered;
                                host.update(cx, |this, cx| {
                                    let now = this.now(cx);
                                    let answer = if hovered {
                                        this.hide.card_entered()
                                    } else {
                                        this.hide.card_left(now)
                                    };
                                    this.arm_grace(answer, window, cx);
                                });
                            })
                        }
                    })
                    .child(body)
                    .child(tracker.probe()),
            );
        // Window-level pointer tracking while the card is shown: the interactive corridor and
        // the cold dismissal by a wheel scroll.
        let tracker_host = host.clone();
        card = card.child(
            canvas(
                |_, _, _| {},
                move |_, _, window, _| {
                    let moved = tracker_host.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, _, window, cx| {
                        moved.update(cx, |this, cx| {
                            this.pointer_moved(event.position, window, cx)
                        });
                    });
                    let scrolled = tracker_host.clone();
                    window.on_mouse_event(move |_: &ScrollWheelEvent, _, window, cx| {
                        scrolled.update(cx, |this, cx| this.dismiss_cold(window, cx));
                    });
                },
            )
            .absolute()
            .size_full(),
        );
        card.into_any_element()
    }
}
