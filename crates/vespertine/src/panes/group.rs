//! The pane group element (LAY-004 R1, R3, R4, R6).

use std::time::Duration;

use vespertine_refineable::Refineable as _;

use super::{
    fit::{PaneEntry, PaneFit, PaneId, PaneSide, fit},
    handle::{ResizeHandle, seam_bounds},
    state::{
        GroupRuntime, PaneFrame, PaneGroupState, PanePlacement, PaneRuntime, PaneSeam, SizeTrack,
    },
};
use crate::{
    AnyElement, App, AvailableSpace, Bounds, ContentMask, Element, ElementId, Entity,
    GlobalElementId, HitboxBehavior, InspectorElementId, IntoElement, LayoutId, Pixels, Style,
    StyleRefinement, Styled, Window,
    motion::{self, MotionTransition, presets},
    point, size,
};

/// Creates a pane group laid out from `state`.
pub fn pane_group(id: impl Into<ElementId>, state: &Entity<PaneGroupState>) -> PaneGroup {
    PaneGroup::new(id, state)
}

/// Arranges a main content region, collapsible leading and trailing panes and
/// a bottom-docked pane according to a [`PaneGroupState`].
///
/// Layout, fitting and slot transitions are computed once per frame in
/// prepaint; each child is laid out with a definite size so that content does
/// not reflow while a slot grows or shrinks. Seams between panes are
/// [`ResizeHandle`]s placed on the pane edges.
pub struct PaneGroup {
    id: ElementId,
    state: Entity<PaneGroupState>,
    style: StyleRefinement,
    main: Option<AnyElement>,
    panes: Vec<(PaneId, AnyElement)>,
    seam_top_inset: Pixels,
    transition: MotionTransition,
}

impl PaneGroup {
    /// Creates a pane group that fills its parent.
    pub fn new(id: impl Into<ElementId>, state: &Entity<PaneGroupState>) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
            style: StyleRefinement::default(),
            main: None,
            panes: Vec::new(),
            seam_top_inset: Pixels::ZERO,
            transition: presets::SIZE_TRANSITION.transition(),
        }
        .size_full()
    }

    /// Sets the main content region's element.
    pub fn main(mut self, child: impl IntoElement) -> Self {
        self.main = Some(child.into_any_element());
        self
    }

    /// Sets the content of the pane `id`. Panes without content still
    /// reserve their slot.
    pub fn pane(mut self, id: impl Into<PaneId>, child: impl IntoElement) -> Self {
        self.panes.push((id.into(), child.into_any_element()));
        self
    }

    /// Insets the horizontal seams' hitboxes from the top of the group so they
    /// yield to overlapping chrome (the same inset for left and right seams).
    pub fn seam_top_inset(mut self, inset: Pixels) -> Self {
        self.seam_top_inset = inset;
        self
    }

    /// Replaces the slot transition (default: the 200 ms size transition of
    /// ANM-002, cubic-bezier 0, 0, 0.58, 1).
    pub fn slot_transition(mut self, transition: MotionTransition) -> Self {
        self.transition = transition;
        self
    }
}

impl Styled for PaneGroup {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl IntoElement for PaneGroup {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

struct PanePaint {
    child: usize,
    bounds: Bounds<Pixels>,
    /// A closing pane stays painted but registers no input or accessibility output.
    inert: bool,
}

/// Prepaint output of a [`PaneGroup`].
pub struct PaneGroupPrepaint {
    main: Bounds<Pixels>,
    panes: Vec<PanePaint>,
    seams: Vec<AnyElement>,
}

fn mask(bounds: Bounds<Pixels>) -> Option<ContentMask<Pixels>> {
    Some(ContentMask { bounds })
}

/// Per-frame result of the slot transitions of one pane.
struct SlotFrame {
    size: f32,
    content_extent: f32,
    closing: bool,
}

/// Advances every pane's slot transition to `now` against the freshly fitted
/// destination sizes (LAY-004 R4).
fn advance_slots(
    runtime: &mut GroupRuntime,
    entries: &[PaneEntry],
    fit: &PaneFit,
    viewport: crate::Size<Pixels>,
    now: Duration,
    reduce_motion: bool,
    transition: &MotionTransition,
) -> (Vec<SlotFrame>, bool) {
    let viewport_changed = runtime
        .viewport
        .is_some_and(|previous| previous != viewport);
    runtime.viewport = Some(viewport);
    let any_direct = entries.iter().any(|entry| {
        runtime
            .panes
            .get(&entry.descriptor.id)
            .is_some_and(|rt| rt.last_adjustment != entry.state.adjustment)
    });

    let mut active = false;
    let mut frames = Vec::with_capacity(entries.len());
    for (entry, result) in entries.iter().zip(&fit.panes) {
        let open = entry.is_open();
        let target = if result.visible { result.size.0 } else { 0.0 };
        let rt = runtime
            .panes
            .entry(entry.descriptor.id.clone())
            .or_insert_with(|| PaneRuntime {
                slot: SizeTrack::at_rest(target, transition, now),
                content: SizeTrack::at_rest(target, transition, now),
                content_valid: target > 0.0,
                last_open: open,
                last_adjustment: entry.state.adjustment,
            });
        let flag_changed = rt.last_open != open;
        let snap = reduce_motion || ((viewport_changed || any_direct) && !flag_changed);
        if target != rt.slot.target() {
            if snap {
                rt.slot.snap(target, transition, now);
                rt.content_valid = target > 0.0;
                if target > 0.0 {
                    rt.content.snap(target, transition, now);
                }
            } else {
                if target > 0.0 {
                    if rt.content_valid {
                        rt.content.retarget(target, transition, now);
                    } else {
                        // Opening from a closed slot: the content is laid out
                        // at the destination size from the first frame.
                        rt.content.snap(target, transition, now);
                        rt.content_valid = true;
                    }
                } else {
                    // Closing: the content keeps the size it had when
                    // closing began.
                    let current = rt.content.value(now);
                    rt.content.snap(current, transition, now);
                }
                rt.slot.retarget(target, transition, now);
            }
        } else if (viewport_changed || reduce_motion) && rt.slot.is_active(now) {
            // A window-driven change discards any departing allocation, and reduced motion
            // settles a transition that began before it was enabled.
            rt.slot.snap(target, transition, now);
            if target > 0.0 {
                rt.content.snap(target, transition, now);
            }
            rt.content_valid = target > 0.0;
        }
        rt.last_open = open;
        rt.last_adjustment = entry.state.adjustment;

        let slot = rt.slot.value(now).max(0.0);
        if target == 0.0 && !rt.slot.is_active(now) && slot == 0.0 {
            rt.content_valid = false;
        }
        active |= rt.slot.is_active(now) || rt.content.is_active(now);
        frames.push(SlotFrame {
            size: slot,
            content_extent: if rt.content_valid {
                rt.content.value(now).max(0.0)
            } else {
                slot
            },
            closing: rt.slot.target() == 0.0 && slot > 0.0,
        });
    }
    (frames, active)
}

impl Element for PaneGroup {
    type RequestLayoutState = ();
    type PrepaintState = PaneGroupPrepaint;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
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
        let mut style = Style::default();
        style.refine(&self.style);
        (window.request_layout(style, [], cx), ())
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
        let (entries, main_min, runtime, metrics) = {
            let state = self.state.read(cx);
            (
                state.entries().to_vec(),
                state.main_min(),
                state.runtime.clone(),
                state.metrics(),
            )
        };

        let fitted = fit(bounds.size, main_min, &entries);
        let (slots, active) = advance_slots(
            &mut runtime.borrow_mut(),
            &entries,
            &fitted,
            bounds.size,
            now,
            cx.reduce_motion(),
            &self.transition,
        );
        if active {
            window.request_animation_frame();
        }

        // Geometry. Slots are clamped cumulatively so that mid-transition
        // sums can never exceed the group.
        let total_width = bounds.size.width.0.max(0.0);
        let total_height = bounds.size.height.0.max(0.0);
        let mut remaining = total_width;
        let mut widths = vec![0.0f32; entries.len()];
        for side in [PaneSide::Leading, PaneSide::Trailing] {
            for (i, entry) in entries.iter().enumerate() {
                if entry.descriptor.side == side {
                    widths[i] = slots[i].size.min(remaining);
                    remaining -= widths[i];
                }
            }
        }
        let mut reservation = 0.0;
        for (i, entry) in entries.iter().enumerate() {
            if entry.descriptor.side == PaneSide::Bottom {
                reservation = slots[i].size.min(total_height);
            }
        }
        let row_height = total_height - reservation;
        let leading_total: f32 = entries
            .iter()
            .zip(&widths)
            .filter(|(e, _)| e.descriptor.side == PaneSide::Leading)
            .map(|(_, w)| *w)
            .sum();
        let trailing_total: f32 = entries
            .iter()
            .zip(&widths)
            .filter(|(e, _)| e.descriptor.side == PaneSide::Trailing)
            .map(|(_, w)| *w)
            .sum();

        let origin = bounds.origin;
        let main_bounds = Bounds::new(
            point(origin.x + Pixels(leading_total), origin.y),
            size(
                Pixels((total_width - leading_total - trailing_total).max(0.0)),
                Pixels(row_height),
            ),
        );
        let mut leading_x = 0.0;
        let mut trailing_x = total_width - trailing_total;
        let mut placements = Vec::with_capacity(entries.len());
        let mut bottom_bounds = Bounds::new(
            point(origin.x, origin.y + Pixels(row_height)),
            size(bounds.size.width, Pixels(reservation)),
        );
        for (i, entry) in entries.iter().enumerate() {
            let slot = match entry.descriptor.side {
                PaneSide::Leading => {
                    let slot = Bounds::new(
                        point(origin.x + Pixels(leading_x), origin.y),
                        size(Pixels(widths[i]), Pixels(row_height)),
                    );
                    leading_x += widths[i];
                    slot
                }
                PaneSide::Trailing => {
                    let slot = Bounds::new(
                        point(origin.x + Pixels(trailing_x), origin.y),
                        size(Pixels(widths[i]), Pixels(row_height)),
                    );
                    trailing_x += widths[i];
                    slot
                }
                PaneSide::Bottom => {
                    bottom_bounds = Bounds::new(
                        point(origin.x, origin.y + Pixels(row_height)),
                        size(bounds.size.width, Pixels(reservation)),
                    );
                    bottom_bounds
                }
            };
            placements.push(PanePlacement {
                id: entry.descriptor.id.clone(),
                side: entry.descriptor.side,
                open: entry.is_open(),
                visible: fitted.panes[i].visible,
                closing: slots[i].closing,
                slot,
                content_extent: Pixels(slots[i].content_extent),
                limits: fitted.panes[i].limits,
            });
        }

        // The reservation is measured exactly once here; children and hit
        // testing below all read these numbers.
        *metrics.0.borrow_mut() = PaneFrame {
            bounds,
            main: main_bounds,
            bottom: bottom_bounds,
            bottom_reservation: Pixels(reservation),
            panes: placements.clone(),
            seams: Vec::new(),
        };

        if let Some(main) = &mut self.main {
            window.with_content_mask(mask(main_bounds), |window| {
                main.prepaint_as_root(
                    main_bounds.origin,
                    size(
                        AvailableSpace::Definite(main_bounds.size.width),
                        AvailableSpace::Definite(main_bounds.size.height),
                    ),
                    window,
                    cx,
                );
            });
        }

        let mut pane_paints = Vec::new();
        for (child_index, (id, child)) in self.panes.iter_mut().enumerate() {
            let Some(placement) = placements.iter().find(|p| &p.id == id) else {
                panic!("PaneGroup child for unknown pane id {id:?}");
            };
            let along = match placement.side {
                PaneSide::Bottom => placement.slot.size.height,
                _ => placement.slot.size.width,
            };
            if along <= Pixels::ZERO {
                continue;
            }
            let slot = placement.slot;
            let extent = placement.content_extent;
            let (content_origin, content_size) = match placement.side {
                PaneSide::Leading => (slot.origin, size(extent, slot.size.height)),
                PaneSide::Trailing => (
                    point(slot.origin.x + slot.size.width - extent, slot.origin.y),
                    size(extent, slot.size.height),
                ),
                PaneSide::Bottom => (
                    point(slot.origin.x, slot.origin.y + slot.size.height - extent),
                    size(slot.size.width, extent),
                ),
            };
            window.with_content_mask(mask(slot), |window| {
                window.with_inert(placement.closing, |window| {
                    child.prepaint_as_root(
                        content_origin,
                        size(
                            AvailableSpace::Definite(content_size.width),
                            AvailableSpace::Definite(content_size.height),
                        ),
                        window,
                        cx,
                    );
                });
                if placement.closing {
                    // The pointer does not reach what lies beneath a closing pane either.
                    window.insert_hitbox(slot, HitboxBehavior::BlockMouse);
                }
            });
            pane_paints.push(PanePaint {
                child: child_index,
                bounds: slot,
                inert: placement.closing,
            });
        }

        // Seams are inserted last so they sit above all content.
        let mut seams = Vec::new();
        let mut seam_rects = Vec::new();
        for (i, entry) in entries.iter().enumerate() {
            let placement = &placements[i];
            if !placement.visible || !placement.open || placement.closing || slots[i].size <= 0.0 {
                continue;
            }
            let side = entry.descriptor.side;
            let slot = placement.slot;
            let (seam, edge) = match side {
                PaneSide::Leading => (slot.right(), slot.left()),
                PaneSide::Trailing => (slot.left(), slot.right()),
                PaneSide::Bottom => (slot.top(), slot.bottom()),
            };
            let rect = seam_bounds(side, bounds, seam, Pixels(reservation), self.seam_top_inset);
            let id = entry.descriptor.id.clone();
            let default = entry.descriptor.default_size;
            let state = self.state.clone();
            let reset_state = self.state.clone();
            let reset_id = id.clone();
            let mut handle =
                ResizeHandle::new(ElementId::NamedInteger("pane-seam".into(), i as u64), side)
                    .with_size(rect.size)
                    .outer_edge(edge)
                    .limits(placement.limits)
                    .default_size(default)
                    .on_drag(move |size, _, cx| {
                        state.update(cx, |state, cx| state.store_size(&id, size, None, cx));
                    })
                    .on_reset(move |size, _, cx| {
                        reset_state.update(cx, |state, cx| {
                            state.store_size(&reset_id, size, Some(default), cx)
                        });
                    })
                    .into_any_element();
            handle.prepaint_as_root(
                rect.origin,
                size(
                    AvailableSpace::Definite(rect.size.width),
                    AvailableSpace::Definite(rect.size.height),
                ),
                window,
                cx,
            );
            seams.push(handle);
            seam_rects.push(PaneSeam {
                pane: entry.descriptor.id.clone(),
                bounds: rect,
            });
        }
        metrics.0.borrow_mut().seams = seam_rects;

        PaneGroupPrepaint {
            main: main_bounds,
            panes: pane_paints,
            seams,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(main) = &mut self.main {
            window.with_content_mask(mask(prepaint.main), |window| main.paint(window, cx));
        }
        for pane in &prepaint.panes {
            let child = &mut self.panes[pane.child].1;
            window.with_content_mask(mask(pane.bounds), |window| {
                window.with_inert(pane.inert, |window| child.paint(window, cx))
            });
        }
        for seam in &mut prepaint.seams {
            seam.paint(window, cx);
        }
    }
}
