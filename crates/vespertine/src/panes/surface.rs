//! A moving input surface (LAY-004 R5, R6).

use std::{cell::RefCell, rc::Rc, time::Duration};

use super::{motion::Glide, state::PaneMetrics};
use crate::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, Point, Window, point,
};

/// Duration of the glide toward a docked route (ANM-002 preset).
pub const DOCKING_GLIDE_DURATION: Duration = Duration::from_millis(420);
/// Duration of the glide back to the surface's own slot (ANM-002 preset).
pub const RETURN_GLIDE_DURATION: Duration = Duration::from_millis(470);
/// Duration over which a layout change is absorbed (the size transition).
pub const REFLOW_GLIDE_DURATION: Duration = Duration::from_millis(200);

struct Glides {
    route: [Glide; 2],
    reflow: [Glide; 2],
}

#[derive(Default)]
struct SurfaceInner {
    glides: Option<Glides>,
    route_target: Point<Pixels>,
    route_retargets: usize,
    layout_origin: Option<Point<Pixels>>,
    displayed: Point<Pixels>,
    clearance: Pixels,
    clearance_shift: Pixels,
    moving: bool,
}

/// Retained motion state of a [`MovingSurface`], readable by the caller. Keep
/// one per surface alongside the surface's entity.
#[derive(Clone, Default)]
pub struct MovingSurfaceState(Rc<RefCell<SurfaceInner>>);

impl MovingSurfaceState {
    /// The total displacement from the layout slot currently applied to the
    /// surface's prepaint origin: route travel plus reflow compensation plus
    /// bottom clearance.
    pub fn displayed_offset(&self) -> Point<Pixels> {
        self.0.borrow().displayed
    }

    /// The route travel component of the displacement.
    pub fn route_offset(&self) -> Point<Pixels> {
        let inner = self.0.borrow();
        inner.glides.as_ref().map_or_else(Point::default, |g| {
            point(Pixels(g.route[0].current()), Pixels(g.route[1].current()))
        })
    }

    /// The reflow compensation component of the displacement.
    pub fn reflow_offset(&self) -> Point<Pixels> {
        let inner = self.0.borrow();
        inner.glides.as_ref().map_or_else(Point::default, |g| {
            point(Pixels(g.reflow[0].current()), Pixels(g.reflow[1].current()))
        })
    }

    /// How many times the route target changed. Reflow never increments it.
    pub fn route_retargets(&self) -> usize {
        self.0.borrow().route_retargets
    }

    /// The bottom reservation the surface used in the last frame.
    pub fn bottom_clearance(&self) -> Pixels {
        self.0.borrow().clearance
    }

    /// The vertical shift applied in the last frame to clear the bottom surface.
    pub fn clearance_shift(&self) -> Pixels {
        self.0.borrow().clearance_shift
    }

    /// The layout slot origin seen in the last frame (unaffected by motion).
    pub fn layout_origin(&self) -> Option<Point<Pixels>> {
        self.0.borrow().layout_origin
    }

    /// Whether a glide was still running in the last frame.
    pub fn is_moving(&self) -> bool {
        self.0.borrow().moving
    }
}

/// Wraps an element so that it can travel to another position without
/// leaving its layout slot.
///
/// The element stays in one slot of the layout tree, so its entity, text,
/// selection, composition state and focus are untouched; only the origin used
/// in prepaint changes, so hitboxes, caret and input-method bounds travel
/// with its pixels. The displacement is the sum of two independent glides:
/// route travel (toward the offset given to [`route`](Self::route)) and
/// reflow compensation (absorbing changes of the slot's layout position,
/// which never restarts route travel). When it must clear the bottom surface,
/// the measured bottom reservation of a pane group is applied on top, in the
/// same frame.
pub struct MovingSurface {
    state: MovingSurfaceState,
    child: AnyElement,
    route: Point<Pixels>,
    metrics: Option<PaneMetrics>,
    docking: Duration,
    returning: Duration,
    reflow: Duration,
}

impl MovingSurface {
    /// Wraps `child`, tracking motion in `state`. The route starts at no
    /// displacement.
    pub fn new(state: &MovingSurfaceState, child: impl IntoElement) -> Self {
        Self {
            state: state.clone(),
            child: child.into_any_element(),
            route: Point::default(),
            metrics: None,
            docking: DOCKING_GLIDE_DURATION,
            returning: RETURN_GLIDE_DURATION,
            reflow: REFLOW_GLIDE_DURATION,
        }
    }

    /// Sets the route: the displacement of the surface from its layout slot
    /// it should travel to. Changing it retargets the route glide from its
    /// current position and velocity. The first route a surface sees is
    /// displayed immediately.
    pub fn route(mut self, offset: Point<Pixels>) -> Self {
        self.route = offset;
        self
    }

    /// Makes the surface clear the bottom-docked surface of the group that
    /// owns `metrics`, using its measured reservation.
    pub fn clear_bottom(mut self, metrics: PaneMetrics) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Overrides the glide durations: toward a displaced route, back to the
    /// slot, and for reflow compensation.
    pub fn durations(mut self, docking: Duration, returning: Duration, reflow: Duration) -> Self {
        self.docking = docking;
        self.returning = returning;
        self.reflow = reflow;
        self
    }
}

impl IntoElement for MovingSurface {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for MovingSurface {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let now = cx.background_executor().now();
        let reduce_motion = cx.reduce_motion();
        let displayed = {
            let mut inner = self.state.0.borrow_mut();
            let inner = &mut *inner;
            let route = self.route;
            let glides = inner.glides.get_or_insert_with(|| {
                inner.route_target = route;
                Glides {
                    route: [
                        Glide::at_rest(route.x.0, now, self.docking),
                        Glide::at_rest(route.y.0, now, self.docking),
                    ],
                    reflow: [
                        Glide::at_rest(0.0, now, self.reflow),
                        Glide::at_rest(0.0, now, self.reflow),
                    ],
                }
            });

            if inner.route_target != route {
                inner.route_target = route;
                inner.route_retargets += 1;
                let duration = if route == Point::default() {
                    self.returning
                } else {
                    self.docking
                };
                glides.route[0].retarget(now, route.x.0, duration);
                glides.route[1].retarget(now, route.y.0, duration);
            }

            // A change of the slot's layout position (another pane opening,
            // closing or resizing) is absorbed by the reflow glide only.
            if let Some(previous) = inner.layout_origin {
                let delta = previous - bounds.origin;
                if delta.x != Pixels::ZERO {
                    glides.reflow[0].displace(now, delta.x.0);
                }
                if delta.y != Pixels::ZERO {
                    glides.reflow[1].displace(now, delta.y.0);
                }
            }
            inner.layout_origin = Some(bounds.origin);

            for glide in glides.reflow.iter_mut() {
                glide.retarget(now, 0.0, self.reflow);
            }
            if reduce_motion {
                for glide in glides.route.iter_mut().chain(glides.reflow.iter_mut()) {
                    glide.snap_to_target();
                }
            }
            let route_now = point(
                Pixels(glides.route[0].advance(now)),
                Pixels(glides.route[1].advance(now)),
            );
            let reflow_now = point(
                Pixels(glides.reflow[0].advance(now)),
                Pixels(glides.reflow[1].advance(now)),
            );
            inner.moving = glides
                .route
                .iter()
                .chain(glides.reflow.iter())
                .any(Glide::is_active);

            let mut shift = Pixels::ZERO;
            inner.clearance = Pixels::ZERO;
            if let Some(metrics) = &self.metrics {
                let reservation = metrics.bottom_reservation();
                inner.clearance = reservation;
                let group = metrics.bounds();
                if group.size.height > Pixels::ZERO {
                    let limit = group.bottom() - reservation;
                    let bottom = bounds.bottom() + route_now.y + reflow_now.y;
                    shift = (limit - bottom).min(Pixels::ZERO);
                }
            }
            inner.clearance_shift = shift;
            let displayed = route_now + reflow_now + point(Pixels::ZERO, shift);
            inner.displayed = displayed;
            displayed
        };
        if self.state.0.borrow().moving {
            window.request_animation_frame();
        }
        window.with_element_offset(displayed, |window| {
            self.child.prepaint(window, cx);
        });
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}
