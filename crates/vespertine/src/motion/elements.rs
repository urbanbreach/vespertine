//! Paint-local opacity and displacement wrappers (ANM-002 R8), the declarative motion host
//! element (R1-R6) and the route/reflow travel element (R10, R11).

use std::{rc::Rc, time::Duration};

use crate::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, ParentElement, Pixels, Point, Styled, Window, px,
};

use super::{MotionCompletion, MotionDescription, MotionTrack, glide::TravelGlide, now};

/// A transparent wrapper that applies paint-time state to everything inside it without
/// changing layout (ANM-002 R8):
///
/// - an opacity multiplier applied to each primitive painted inside (fills, borders and text
///   individually, not as a group),
/// - a displacement of the element origin applied in prepaint, so paint, hitboxes, the caret and
///   input-method bounds of descendants all move together and siblings never relayout,
/// - an inert flag that keeps the subtree painted but removes its hitboxes, focus, pointer,
///   keyboard and accessibility participation.
pub struct PaintScope<E> {
    element: Option<E>,
    opacity: Option<f32>,
    offset: Point<Pixels>,
    inert: bool,
}

impl<E: IntoElement + 'static> PaintScope<E> {
    /// Wraps `element` with no opacity, displacement or inertness.
    pub fn new(element: E) -> Self {
        Self {
            element: Some(element),
            opacity: None,
            offset: Point::default(),
            inert: false,
        }
    }

    /// Multiplies the opacity of everything painted inside by `opacity`.
    pub fn opacity(mut self, opacity: f32) -> Self {
        self.opacity = Some(opacity);
        self
    }

    /// Displaces the wrapped element's origin by `offset` in prepaint.
    pub fn offset(mut self, offset: Point<Pixels>) -> Self {
        self.offset = offset;
        self
    }

    /// Makes the wrapped subtree painted but inert (no hitboxes, focus, input or accessibility).
    pub fn inert(mut self, inert: bool) -> Self {
        self.inert = inert;
        self
    }
}

impl<E: ParentElement> ParentElement for PaintScope<E> {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        if let Some(element) = &mut self.element {
            element.extend(elements);
        }
    }
}

impl<E: IntoElement + 'static> IntoElement for PaintScope<E> {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl<E: IntoElement + 'static> Element for PaintScope<E> {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut element = self
            .element
            .take()
            .expect("request_layout is called once")
            .into_any_element();
        (element.request_layout(window, cx), element)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        element: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let (opacity, offset, inert) = (self.opacity, self.offset, self.inert);
        window.with_inert(inert, |window| {
            window.with_element_opacity(opacity, |window| {
                window.with_element_offset(offset, |window| element.prepaint(window, cx))
            })
        });
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        element: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (opacity, inert) = (self.opacity, self.inert);
        window.with_inert(inert, |window| {
            window.with_element_opacity(opacity, |window| element.paint(window, cx))
        });
    }
}

/// A callback receiving motion completion reports.
pub type MotionCompleteHandler = Rc<dyn Fn(MotionCompletion, &mut Window, &mut App)>;

/// Extension methods that wrap an element in the motion host and paint-local wrappers.
pub trait MotionExt: IntoElement + Sized + 'static {
    /// Drives this element's style from a declarative motion track (ANM-002 R1-R6). The element
    /// ID keeps the track's state across frames.
    fn with_motion(
        self,
        id: impl Into<ElementId>,
        description: MotionDescription,
    ) -> MotionElement<Self>
    where
        Self: Styled,
    {
        MotionElement {
            id: id.into(),
            element: Some(self),
            description,
            frozen_at: None,
            on_complete: None,
        }
    }

    /// Multiplies the opacity of every primitive painted inside this element by `opacity`,
    /// without relayout.
    fn with_paint_opacity(self, opacity: f32) -> PaintScope<Self> {
        PaintScope::new(self).opacity(opacity)
    }

    /// Displaces this element (its prepaint origin, hitboxes, caret and input bounds) by
    /// `offset`, without relayout of siblings.
    fn with_displacement(self, offset: Point<Pixels>) -> PaintScope<Self> {
        PaintScope::new(self).offset(offset)
    }

    /// Wraps this element in a route-plus-reflow glide (ANM-002 R10); see [`Travel`].
    fn with_travel(self, id: impl Into<ElementId>, route: impl std::hash::Hash) -> Travel<Self> {
        Travel::new(id, route, self)
    }
}

impl<E: IntoElement + 'static> MotionExt for E {}

/// The host element of a declarative motion track. See [`MotionExt::with_motion`].
pub struct MotionElement<E> {
    id: ElementId,
    element: Option<E>,
    description: MotionDescription,
    frozen_at: Option<Duration>,
    on_complete: Option<MotionCompleteHandler>,
}

impl<E> MotionElement<E> {
    /// Calls `handler` with each completion report (exactly once per owed completion). The call
    /// is deferred until after the current frame's element pass.
    pub fn on_motion_complete(
        mut self,
        handler: impl Fn(MotionCompletion, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_complete = Some(Rc::new(handler));
        self
    }

    /// Freezes the track at `elapsed` since its start: the description starts at time zero, is
    /// sampled at `elapsed`, and no frames are requested. This is the deterministic sampling
    /// used by tests and fidelity scenes.
    pub fn frozen_at(mut self, elapsed: Duration) -> Self {
        self.frozen_at = Some(elapsed);
        self
    }
}

impl<E: ParentElement> ParentElement for MotionElement<E> {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        if let Some(element) = &mut self.element {
            element.extend(elements);
        }
    }
}

impl<E: IntoElement + Styled + 'static> IntoElement for MotionElement<E> {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl<E: IntoElement + Styled + 'static> Element for MotionElement<E> {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        window.with_element_state(global_id.unwrap(), |state: Option<MotionTrack>, window| {
            let mut track = state.unwrap_or_default();
            let (update_at, sample_at) = match self.frozen_at {
                Some(elapsed) => (Duration::ZERO, elapsed),
                None => {
                    let now = now(cx);
                    (now, now)
                }
            };
            track.update(&self.description, update_at);
            let sample = track.sample(sample_at, cx.reduce_motion());

            let mut element = self.element.take().expect("request_layout is called once");
            sample.style.apply(element.style());
            let mut element = element.into_any_element();

            if sample.active && self.frozen_at.is_none() {
                window.request_animation_frame();
            }
            if let (Some(completion), Some(handler)) = (sample.completion, self.on_complete.clone())
            {
                window.defer(cx, move |window, cx| handler(completion, window, cx));
            }
            ((element.request_layout(window, cx), element), track)
        })
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        element: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        element.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        element: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        element.paint(window, cx);
    }
}

struct TravelState {
    glide: TravelGlide,
    natural: Option<Point<Pixels>>,
    route: u64,
    updated_at: Duration,
}

/// Route plus reflow glide of a travelling component (ANM-002 R10, R11).
///
/// The element measures its natural (layout) position during prepaint of the same frame. When
/// the `route` key changes, the position delta is added to an independent route glide; when the
/// position changes under the same route (for example because an unrelated pane resized), the
/// delta is added to a separate reflow glide. Both decay critically damped to zero with their own
/// duration, a layout change never restarts or retargets the route glide, and the sum is applied
/// as a prepaint displacement, so paint, hitboxes, caret and input bounds follow the displayed
/// surface and siblings are not relaid out. Reduced motion snaps both glides.
pub struct Travel<E> {
    id: ElementId,
    route: u64,
    route_duration: Duration,
    reflow_duration: Duration,
    element: Option<E>,
}

fn hash_route(route: impl std::hash::Hash) -> u64 {
    use std::hash::Hasher;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    route.hash(&mut hasher);
    hasher.finish()
}

impl<E: IntoElement + 'static> Travel<E> {
    /// Creates a travel wrapper identified by `id`; `route` identifies the component's current
    /// route (any hashable value; a changed value is a route change). Defaults: route glide
    /// [`ROUTE_GLIDE_DURATION`], reflow glide [`SIZE_TRANSITION`].
    pub fn new(id: impl Into<ElementId>, route: impl std::hash::Hash, element: E) -> Self {
        Self {
            id: id.into(),
            route: hash_route(route),
            route_duration: super::presets::DOCKING_GLIDE.duration(),
            reflow_duration: super::presets::SIZE_TRANSITION.duration(),
            element: Some(element),
        }
    }

    /// Sets the route glide duration (for example the 420 ms docking or 470 ms return glide).
    pub fn route_duration(mut self, duration: Duration) -> Self {
        self.route_duration = duration;
        self
    }

    /// Sets the reflow glide duration.
    pub fn reflow_duration(mut self, duration: Duration) -> Self {
        self.reflow_duration = duration;
        self
    }
}

impl<E: ParentElement> ParentElement for Travel<E> {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        if let Some(element) = &mut self.element {
            element.extend(elements);
        }
    }
}

impl<E: IntoElement + 'static> IntoElement for Travel<E> {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl<E: IntoElement + 'static> Element for Travel<E> {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut element = self
            .element
            .take()
            .expect("request_layout is called once")
            .into_any_element();
        (element.request_layout(window, cx), element)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        element: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let current = now(cx);
        let reduced = cx.reduce_motion();
        let (route, route_duration, reflow_duration) =
            (self.route, self.route_duration, self.reflow_duration);
        let displacement =
            window.with_element_state(id.unwrap(), |state: Option<TravelState>, window| {
                let mut state = state.unwrap_or(TravelState {
                    glide: TravelGlide::default(),
                    natural: None,
                    route,
                    updated_at: current,
                });
                let dt = current.saturating_sub(state.updated_at).as_secs_f32();
                state.updated_at = current;
                let mut active = state
                    .glide
                    .advance(route_duration, reflow_duration, dt, reduced);

                // The natural position is measured now, in prepaint of this frame, never guessed.
                let natural = bounds.origin;
                if let Some(previous) = state.natural {
                    let delta = [
                        f32::from(previous.x - natural.x),
                        f32::from(previous.y - natural.y),
                    ];
                    if delta != [0.0, 0.0] {
                        state.glide.displace(delta, state.route != route);
                        active |= !reduced;
                        if reduced {
                            state
                                .glide
                                .advance(route_duration, reflow_duration, 0.0, true);
                        }
                    }
                }
                state.natural = Some(natural);
                state.route = route;

                if active {
                    window.request_animation_frame();
                }
                let [x, y] = state.glide.displacement();
                (Point::new(px(x), px(y)), state)
            });
        window.with_element_offset(displacement, |window| element.prepaint(window, cx));
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        element: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        element.paint(window, cx);
    }
}
