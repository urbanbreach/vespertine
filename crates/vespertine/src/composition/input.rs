//! Pointer routing to cooperating windowless native producers (PLT-001 R16).
//!
//! A producer that does not receive input through the display server registers pointer and
//! wheel handlers for its surface and opts in with [`InputForwarding::Framework`]. The window
//! feeds every pointer event to a [`PointerRouter`] after framework capture has run; the
//! router decides which surface receives what, so the semantics live in one pure, tested
//! place.
//!
//! * Candidates are visible surfaces whose current bounds contain the pointer, topmost first
//!   (stacking order of the surface tree).
//! * A framework-captured event is delivered to no producer, and a pointer that becomes
//!   covered counts as leaving its recipient.
//! * Crossing from one surface to another sends a leave to the previous recipient (with the
//!   new pointer position) before the enter and move of the new one.
//! * A press keeps its recipient through the matching release even when the pointer moves
//!   over a different surface; moves while a button is held go to the owner, whose pointer
//!   position may lie outside its bounds.
//! * Hidden or destroyed surfaces stop participating immediately: they receive nothing,
//!   not even a leave.
//! * Wheel events go to the topmost candidate under the pointer; a handler reporting the
//!   event as unhandled lets the next candidate below see it.

use crate::{
    Bounds, MouseButton, MouseDownEvent, MouseExitEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    Point, ScrollWheelEvent, SurfaceId,
};

/// A pointer event delivered to a producer. Positions are in window-content logical pixels.
#[derive(Clone, Debug, PartialEq)]
pub enum NativePointerEvent {
    /// The pointer entered the surface.
    Enter(MouseMoveEvent),
    /// The pointer moved over the surface, or moved while the surface owns a press.
    Move(MouseMoveEvent),
    /// The pointer left the surface; the position is where the pointer is now.
    Leave(MouseExitEvent),
    /// A button was pressed over the surface.
    Down(MouseDownEvent),
    /// A button that the surface owns was released.
    Up(MouseUpEvent),
}

/// What a candidate surface looks like to the router.
#[derive(Clone, Copy, Debug)]
pub struct RouteSurface {
    /// The surface.
    pub id: SurfaceId,
    /// Its current bounds in window-content logical pixels.
    pub bounds: Bounds<Pixels>,
}

/// One event for one surface.
#[derive(Clone, Debug, PartialEq)]
pub struct PointerDelivery {
    /// The receiving surface.
    pub surface: SurfaceId,
    /// The event.
    pub event: NativePointerEvent,
}

/// The raw input the window feeds to the router.
#[derive(Clone, Debug)]
pub enum PointerInput {
    /// The pointer moved.
    Move(MouseMoveEvent),
    /// A button was pressed.
    Down(MouseDownEvent),
    /// A button was released.
    Up(MouseUpEvent),
    /// The pointer left the window.
    Exited(MouseExitEvent),
}

/// The routing state: the current recipient and the press owner.
#[derive(Debug, Default)]
pub struct PointerRouter {
    recipient: Option<SurfaceId>,
    owner: Option<SurfaceId>,
    buttons_down: usize,
}

impl PointerRouter {
    /// The surface that currently has the pointer.
    pub fn recipient(&self) -> Option<SurfaceId> {
        self.recipient
    }

    /// The surface that owns the press in progress.
    pub fn press_owner(&self) -> Option<SurfaceId> {
        self.owner
    }

    /// Forgets a surface that was hidden or destroyed: it receives nothing more.
    pub fn forget(&mut self, surface: SurfaceId) {
        if self.recipient == Some(surface) {
            self.recipient = None;
        }
        if self.owner == Some(surface) {
            self.owner = None;
            self.buttons_down = 0;
        }
    }

    /// The topmost candidate containing the position. `surfaces` is ordered topmost first.
    fn hit(surfaces: &[RouteSurface], position: Point<Pixels>) -> Option<SurfaceId> {
        surfaces
            .iter()
            .find(|surface| surface.bounds.contains(&position))
            .map(|surface| surface.id)
    }

    fn is_live(surfaces: &[RouteSurface], id: SurfaceId) -> bool {
        surfaces.iter().any(|surface| surface.id == id)
    }

    /// Routes one pointer event. `surfaces` are the live candidates, topmost first;
    /// `captured` says that the framework captured the event (an overlay region or a handler
    /// that stopped propagation covers the pointer).
    pub fn route(
        &mut self,
        input: &PointerInput,
        surfaces: &[RouteSurface],
        captured: bool,
    ) -> Vec<PointerDelivery> {
        if let Some(owner) = self.owner
            && !Self::is_live(surfaces, owner)
        {
            self.owner = None;
            self.buttons_down = 0;
        }
        if let Some(recipient) = self.recipient
            && !Self::is_live(surfaces, recipient)
        {
            self.recipient = None;
        }
        let mut out = Vec::new();
        match input {
            PointerInput::Move(event) => {
                let target = if let Some(owner) = self.owner {
                    // A press in progress keeps every move with its owner.
                    Some(owner)
                } else if captured {
                    None
                } else {
                    Self::hit(surfaces, event.position)
                };
                self.cross_to(
                    target,
                    event.position,
                    event.pressed_button,
                    event.modifiers,
                    &mut out,
                );
                if let Some(target) = target {
                    out.push(PointerDelivery {
                        surface: target,
                        event: NativePointerEvent::Move(event.clone()),
                    });
                }
            }
            PointerInput::Down(event) => {
                let target = if captured {
                    None
                } else {
                    Self::hit(surfaces, event.position)
                };
                self.cross_to(
                    target,
                    event.position,
                    Some(event.button),
                    event.modifiers,
                    &mut out,
                );
                if let Some(target) = target {
                    if self.owner.is_none() {
                        self.owner = Some(target);
                    }
                    self.buttons_down += 1;
                    out.push(PointerDelivery {
                        surface: target,
                        event: NativePointerEvent::Down(event.clone()),
                    });
                }
            }
            PointerInput::Up(event) => {
                if let Some(owner) = self.owner {
                    self.buttons_down = self.buttons_down.saturating_sub(1);
                    if self.buttons_down == 0 {
                        self.owner = None;
                    }
                    out.push(PointerDelivery {
                        surface: owner,
                        event: NativePointerEvent::Up(event.clone()),
                    });
                    if self.owner.is_none() {
                        // The press is over: the pointer now belongs to whatever is under it.
                        let target = if captured {
                            None
                        } else {
                            Self::hit(surfaces, event.position)
                        };
                        self.cross_to(target, event.position, None, event.modifiers, &mut out);
                    }
                } else {
                    let target = if captured {
                        None
                    } else {
                        Self::hit(surfaces, event.position)
                    };
                    self.cross_to(target, event.position, None, event.modifiers, &mut out);
                }
            }
            PointerInput::Exited(event) => {
                if self.owner.is_none() {
                    self.cross_to(
                        None,
                        event.position,
                        event.pressed_button,
                        event.modifiers,
                        &mut out,
                    );
                }
            }
        }
        out
    }

    /// The candidates that receive a wheel event at `position`, topmost first. A framework
    /// capture delivers to none.
    pub fn wheel_targets(
        &self,
        event: &ScrollWheelEvent,
        surfaces: &[RouteSurface],
        captured: bool,
    ) -> Vec<SurfaceId> {
        if captured {
            return Vec::new();
        }
        surfaces
            .iter()
            .filter(|surface| surface.bounds.contains(&event.position))
            .map(|surface| surface.id)
            .collect()
    }

    /// Moves the pointer to `target`: a leave for the previous recipient (carrying the new
    /// position) and an enter for the new one.
    fn cross_to(
        &mut self,
        target: Option<SurfaceId>,
        position: Point<Pixels>,
        pressed_button: Option<MouseButton>,
        modifiers: crate::Modifiers,
        out: &mut Vec<PointerDelivery>,
    ) {
        if self.recipient == target {
            return;
        }
        if let Some(previous) = self.recipient.take() {
            out.push(PointerDelivery {
                surface: previous,
                event: NativePointerEvent::Leave(MouseExitEvent {
                    position,
                    pressed_button,
                    modifiers,
                }),
            });
        }
        if let Some(target) = target {
            out.push(PointerDelivery {
                surface: target,
                event: NativePointerEvent::Enter(MouseMoveEvent {
                    position,
                    pressed_button,
                    modifiers,
                }),
            });
        }
        self.recipient = target;
    }
}

/// Converts a pointer position reported local to a native slot (including during a grab) to
/// window-content logical pixels by adding the slot's origin.
pub fn slot_local_to_window(slot_origin: Point<Pixels>, local: Point<Pixels>) -> Point<Pixels> {
    Point::new(slot_origin.x + local.x, slot_origin.y + local.y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Modifiers, SurfaceTree, point, px, size};

    fn bounds(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds {
            origin: point(px(x), px(y)),
            size: size(px(w), px(h)),
        }
    }

    fn surfaces() -> (SurfaceId, SurfaceId, Vec<RouteSurface>) {
        let mut tree = SurfaceTree::new();
        let lower = tree.create_below_overlay(crate::SurfaceKind::Native);
        let upper = tree.create_below_overlay(crate::SurfaceKind::Native);
        // Topmost first: upper covers the right half of lower.
        let list = vec![
            RouteSurface {
                id: upper,
                bounds: bounds(50.0, 0.0, 100.0, 100.0),
            },
            RouteSurface {
                id: lower,
                bounds: bounds(0.0, 0.0, 100.0, 100.0),
            },
        ];
        (lower, upper, list)
    }

    fn mv(x: f32, y: f32, pressed: Option<MouseButton>) -> PointerInput {
        PointerInput::Move(MouseMoveEvent {
            position: point(px(x), px(y)),
            pressed_button: pressed,
            modifiers: Modifiers::default(),
        })
    }

    fn down(x: f32, y: f32) -> PointerInput {
        PointerInput::Down(MouseDownEvent {
            button: MouseButton::Left,
            position: point(px(x), px(y)),
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        })
    }

    fn up(x: f32, y: f32) -> PointerInput {
        PointerInput::Up(MouseUpEvent {
            button: MouseButton::Left,
            position: point(px(x), px(y)),
            modifiers: Modifiers::default(),
            click_count: 1,
        })
    }

    fn kinds(deliveries: &[PointerDelivery]) -> Vec<(SurfaceId, &'static str)> {
        deliveries
            .iter()
            .map(|delivery| {
                let kind = match delivery.event {
                    NativePointerEvent::Enter(_) => "enter",
                    NativePointerEvent::Move(_) => "move",
                    NativePointerEvent::Leave(_) => "leave",
                    NativePointerEvent::Down(_) => "down",
                    NativePointerEvent::Up(_) => "up",
                };
                (delivery.surface, kind)
            })
            .collect()
    }

    #[test]
    fn a11_the_topmost_surface_under_the_pointer_receives_enter_then_move() {
        let (lower, upper, list) = surfaces();
        let mut router = PointerRouter::default();
        assert_eq!(
            kinds(&router.route(&mv(10.0, 10.0, None), &list, false)),
            [(lower, "enter"), (lower, "move")]
        );
        assert_eq!(
            kinds(&router.route(&mv(12.0, 10.0, None), &list, false)),
            [(lower, "move")]
        );
        // Over the overlap the upper surface wins.
        let crossing = router.route(&mv(70.0, 10.0, None), &list, false);
        assert_eq!(
            kinds(&crossing),
            [(lower, "leave"), (upper, "enter"), (upper, "move")]
        );
        // The leave to the previous recipient carries the new pointer position.
        let NativePointerEvent::Leave(leave) = &crossing[0].event else {
            panic!("expected a leave");
        };
        assert_eq!(leave.position, point(px(70.0), px(10.0)));
        assert_eq!(router.recipient(), Some(upper));
        // Leaving every surface sends one leave and nothing else.
        assert_eq!(
            kinds(&router.route(&mv(500.0, 10.0, None), &list, false)),
            [(upper, "leave")]
        );
        assert!(
            router
                .route(&mv(501.0, 10.0, None), &list, false)
                .is_empty()
        );
    }

    #[test]
    fn a11_framework_capture_runs_first_and_counts_as_leaving() {
        let (lower, _, list) = surfaces();
        let mut router = PointerRouter::default();
        router.route(&mv(10.0, 10.0, None), &list, false);
        // An overlay captures the pointer: the producer receives a leave, then nothing.
        assert_eq!(
            kinds(&router.route(&mv(11.0, 10.0, None), &list, true)),
            [(lower, "leave")]
        );
        assert!(router.route(&down(11.0, 10.0), &list, true).is_empty());
        assert_eq!(router.press_owner(), None);
        // Uncovered again: enter anew.
        assert_eq!(
            kinds(&router.route(&mv(12.0, 10.0, None), &list, false)),
            [(lower, "enter"), (lower, "move")]
        );
    }

    #[test]
    fn a11_a_press_keeps_its_recipient_through_the_release_across_surfaces() {
        let (lower, upper, list) = surfaces();
        let mut router = PointerRouter::default();
        assert_eq!(
            kinds(&router.route(&down(10.0, 10.0), &list, false)),
            [(lower, "enter"), (lower, "down")]
        );
        assert_eq!(router.press_owner(), Some(lower));
        // Dragging over the other surface and outside every bounds still goes to the owner.
        assert_eq!(
            kinds(&router.route(&mv(70.0, 10.0, Some(MouseButton::Left)), &list, false)),
            [(lower, "move")]
        );
        assert_eq!(
            kinds(&router.route(&mv(900.0, 10.0, Some(MouseButton::Left)), &list, false)),
            [(lower, "move")]
        );
        // The release goes to the owner; the pointer then belongs to what is under it.
        let release = router.route(&up(70.0, 10.0), &list, false);
        assert_eq!(
            kinds(&release),
            [(lower, "up"), (lower, "leave"), (upper, "enter")]
        );
        assert_eq!(router.press_owner(), None);
        assert_eq!(router.recipient(), Some(upper));
    }

    #[test]
    fn a11_hidden_or_destroyed_surfaces_receive_nothing_after_they_stop_participating() {
        let (lower, upper, list) = surfaces();
        let mut router = PointerRouter::default();
        router.route(&down(10.0, 10.0), &list, false);
        // The owner is hidden: it is not among the candidates any more.
        let without_lower: Vec<RouteSurface> = list
            .iter()
            .copied()
            .filter(|surface| surface.id != lower)
            .collect();
        assert!(
            router
                .route(
                    &mv(10.0, 10.0, Some(MouseButton::Left)),
                    &without_lower,
                    false
                )
                .is_empty()
        );
        assert!(
            router
                .route(&up(10.0, 10.0), &without_lower, false)
                .is_empty()
        );
        assert_eq!(router.recipient(), None);
        assert_eq!(router.press_owner(), None);
        // Forgetting a surface clears the state that refers to it.
        router.route(&mv(70.0, 10.0, None), &list, false);
        assert_eq!(router.recipient(), Some(upper));
        router.forget(upper);
        assert_eq!(router.recipient(), None);
        // A destroyed surface is not a candidate any more: nothing reaches it.
        assert!(router.route(&mv(71.0, 10.0, None), &[], false).is_empty());
    }

    #[test]
    fn a11_wheel_events_visit_candidates_topmost_first_unless_captured() {
        let (lower, upper, list) = surfaces();
        let router = PointerRouter::default();
        let wheel = ScrollWheelEvent {
            position: point(px(70.0), px(10.0)),
            ..Default::default()
        };
        assert_eq!(router.wheel_targets(&wheel, &list, false), [upper, lower]);
        assert!(router.wheel_targets(&wheel, &list, true).is_empty());
        let outside = ScrollWheelEvent {
            position: point(px(500.0), px(10.0)),
            ..Default::default()
        };
        assert!(router.wheel_targets(&outside, &list, false).is_empty());
    }

    #[test]
    fn a11_slot_local_positions_gain_the_slot_origin() {
        assert_eq!(
            slot_local_to_window(point(px(180.0), px(40.0)), point(px(5.0), px(7.0))),
            point(px(185.0), px(47.0))
        );
    }
}
