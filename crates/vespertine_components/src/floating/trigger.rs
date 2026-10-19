//! Trigger bounds and the outside-press probe of a dismissal layer (CMP-002 R13, R14).

use super::dismiss::{LayerId, press_is_inside, set_layer_bounds};
use std::{cell::Cell, rc::Rc};
use vespertine::{
    App, Bounds, DispatchPhase, IntoElement, MouseDownEvent, Pixels, Point, Styled, Window, canvas,
};

/// The window bounds of a trigger as last painted, shared between the trigger's probe and the
/// floating layer attached to it. Clones share the cell.
#[derive(Clone, Default, Debug)]
pub struct TriggerBounds(Rc<Cell<Option<Bounds<Pixels>>>>);

impl TriggerBounds {
    /// An empty tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// The bounds of the last painted frame, if the trigger has painted.
    pub fn get(&self) -> Option<Bounds<Pixels>> {
        self.0.get()
    }

    /// Sets the bounds explicitly (a trigger whose geometry the caller knows).
    pub fn set(&self, bounds: Bounds<Pixels>) {
        self.0.set(Some(bounds));
    }

    /// Whether `point` lies inside the trigger.
    pub fn contains(&self, point: &Point<Pixels>) -> bool {
        self.get().is_some_and(|bounds| bounds.contains(point))
    }

    /// A zero-flow child to place inside the trigger element (which must be positioned
    /// relatively, or at least not static): it records the trigger's bounds during prepaint.
    pub fn probe(&self) -> impl IntoElement {
        let cell = self.0.clone();
        canvas(move |bounds, _, _| cell.set(Some(bounds)), |_, _, _, _| {})
            .absolute()
            .top_0()
            .left_0()
            .size_full()
    }
}

/// What dismisses a layer on an outside press and what that press may still reach.
#[derive(Clone)]
pub struct OutsidePress {
    /// The layer the probe belongs to.
    pub layer: LayerId,
    /// Called (capture phase) for a press outside the layer and every layer above it.
    pub on_dismiss: Rc<dyn Fn(&mut Window, &mut App)>,
    /// Top-level layers consume the press in the bubble phase so background controls do not
    /// activate; nested layers let their own outside presses reach sibling controls.
    pub consume: bool,
    /// Triggers that still receive the press (switching triggers takes one click, R13).
    pub exempt: Vec<TriggerBounds>,
}

impl OutsidePress {
    /// The probe element: place it inside the layer's surface (it fills it, recording its
    /// bounds and registering the press listeners while painting).
    pub fn probe(self) -> impl IntoElement {
        let layer = self.layer;
        canvas(
            move |bounds, window, cx| {
                set_layer_bounds(window, cx, layer, bounds);
            },
            move |_, _, window, _cx| {
                let this = self;
                let swallow = Rc::new(Cell::new(false));
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    match phase {
                        DispatchPhase::Capture => {
                            // `None`: the layer is closing or closed; its overlay swallows
                            // presses over the dying surface and everything else falls through.
                            if press_is_inside(window, cx, this.layer, event.position)
                                != Some(false)
                            {
                                return;
                            }
                            (this.on_dismiss)(window, cx);
                            let on_trigger =
                                this.exempt.iter().any(|t| t.contains(&event.position));
                            swallow.set(this.consume && !on_trigger);
                        }
                        DispatchPhase::Bubble => {
                            if swallow.replace(false) {
                                cx.stop_propagation();
                            }
                        }
                    }
                });
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }
}
