//! A moving input surface (LAY-004 R5, R6).

use std::{cell::RefCell, rc::Rc, time::Duration};

use super::state::PaneMetrics;
use crate::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, Point, Window,
    motion::{self, TravelGlide, presets},
    point,
};

#[derive(Default)]
struct SurfaceInner {
    travel: TravelGlide,
    route: Point<Pixels>,
    route_retargets: usize,
    layout_origin: Option<Point<Pixels>>,
    last_time: Option<Duration>,
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
        point(
            inner.route.x + Pixels(inner.travel.route[0].value),
            inner.route.y + Pixels(inner.travel.route[1].value),
        )
    }

    /// The reflow compensation component of the displacement.
    pub fn reflow_offset(&self) -> Point<Pixels> {
        let inner = self.0.borrow();
        point(
            Pixels(inner.travel.reflow[0].value),
            Pixels(inner.travel.reflow[1].value),
        )
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
            docking: presets::DOCKING_GLIDE.duration(),
            returning: presets::RETURN_GLIDE.duration(),
            reflow: presets::SIZE_TRANSITION.duration(),
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

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn request_layout(
        &mut self,
        _global_id: Option<&GlobalElementId>,
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
        let now = motion::now(cx);
        let reduced = cx.reduce_motion();
        let displayed = {
            let mut inner = self.state.0.borrow_mut();
            let inner = &mut *inner;
            let route = self.route;

            // The surface's natural position is its slot origin plus the route it
            // is meant to rest at. A route change displaces the route glide by the
            // natural-position delta; a change of the slot's layout position
            // (another pane opening, closing or resizing) displaces only the
            // reflow glide, so it never restarts or retargets the route motion.
            if let Some(previous) = inner.layout_origin {
                if inner.route != route {
                    inner.route_retargets += 1;
                    let delta = inner.route - route;
                    inner.travel.displace([delta.x.0, delta.y.0], true);
                }
                let delta = previous - bounds.origin;
                if delta != Point::default() {
                    inner.travel.displace([delta.x.0, delta.y.0], false);
                }
            }
            inner.route = route;
            inner.layout_origin = Some(bounds.origin);

            let dt = inner
                .last_time
                .map_or(0.0, |last| now.saturating_sub(last).as_secs_f32());
            inner.last_time = Some(now);
            let route_duration = if route == Point::default() {
                self.returning
            } else {
                self.docking
            };
            inner.moving = inner
                .travel
                .advance(route_duration, self.reflow, dt, reduced);
            let [dx, dy] = inner.travel.displacement();
            let travel_now = point(route.x + Pixels(dx), route.y + Pixels(dy));

            let mut shift = Pixels::ZERO;
            inner.clearance = Pixels::ZERO;
            if let Some(metrics) = &self.metrics {
                let reservation = metrics.bottom_reservation();
                inner.clearance = reservation;
                let group = metrics.bounds();
                if group.size.height > Pixels::ZERO {
                    let limit = group.bottom() - reservation;
                    let bottom = bounds.bottom() + travel_now.y;
                    shift = (limit - bottom).min(Pixels::ZERO);
                }
            }
            inner.clearance_shift = shift;
            let displayed = travel_now + point(Pixels::ZERO, shift);
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
