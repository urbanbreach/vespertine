//! Retained pane-group state, its events and the per-frame metrics exposed to
//! children (LAY-004 R1, R6, R7).

use std::{cell::RefCell, collections::HashMap, rc::Rc, time::Duration};

use super::fit::{
    MAIN_MIN_WIDTH, PaneDescriptor, PaneEntry, PaneFit, PaneId, PaneLimits, PaneSide, PaneState,
    fit,
};
use crate::{
    Bounds, Context, EventEmitter, Pixels, Size,
    motion::{MotionDescription, MotionProps, MotionTrack, MotionTransition},
};

/// Changes to a [`PaneGroupState`], emitted for every change however it was
/// made (seam drag, seam reset or a direct call) so callers can persist them.
#[derive(Clone, Debug, PartialEq)]
pub enum PaneEvent {
    /// A pane's retained size changed.
    Resized {
        /// The pane.
        pane: PaneId,
        /// The new retained size.
        size: Pixels,
    },
    /// A seam double-click reset a pane to its default size.
    Reset {
        /// The pane.
        pane: PaneId,
        /// The descriptor's default size.
        default: Pixels,
        /// The size applied after clamping to the current limits.
        size: Pixels,
    },
    /// A pane's open flag changed.
    CollapsedChanged {
        /// The pane.
        pane: PaneId,
        /// The new collapsed flag.
        collapsed: bool,
    },
}

/// Where a pane is placed in the most recently painted frame.
#[derive(Clone, Debug, PartialEq)]
pub struct PanePlacement {
    /// The pane.
    pub id: PaneId,
    /// The pane's side.
    pub side: PaneSide,
    /// Whether the open flag is on.
    pub open: bool,
    /// Whether fitting shows the pane (open and not hidden).
    pub visible: bool,
    /// Whether the pane's slot is closing: it still occupies space but is
    /// painted inert until the transition ends.
    pub closing: bool,
    /// The slot the pane currently occupies (animated during transitions).
    pub slot: Bounds<Pixels>,
    /// The size along the pane's axis its content is laid out at.
    pub content_extent: Pixels,
    /// The size limits that applied this frame.
    pub limits: PaneLimits,
}

/// The hitbox rectangle of a pane's seam in the last frame.
#[derive(Clone, Debug, PartialEq)]
pub struct PaneSeam {
    /// The pane the seam resizes.
    pub pane: PaneId,
    /// The seam's hitbox: it consumes pointer events inside it only.
    pub bounds: Bounds<Pixels>,
}

/// Geometry of the last prepaint of a pane group.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PaneFrame {
    /// The group's bounds.
    pub bounds: Bounds<Pixels>,
    /// The main content region.
    pub main: Bounds<Pixels>,
    /// The bottom-docked surface's slot (zero height when none).
    pub bottom: Bounds<Pixels>,
    /// The measured bottom reservation: measured once per frame, and used
    /// both to inset content above the bottom surface and for its hit
    /// testing.
    pub bottom_reservation: Pixels,
    /// One placement per pane in descriptor order.
    pub panes: Vec<PanePlacement>,
    /// The hitbox rectangle of each visible seam.
    pub seams: Vec<PaneSeam>,
}

/// A shared view of the group's measured geometry for children (R6).
///
/// The values are written once per frame while the group prepaints, before
/// any of its children prepaint, so children read the same numbers the group
/// uses for hit testing.
#[derive(Clone, Default)]
pub struct PaneMetrics(pub(super) Rc<RefCell<PaneFrame>>);

impl PaneMetrics {
    /// The bottom reservation measured for the current frame.
    pub fn bottom_reservation(&self) -> Pixels {
        self.0.borrow().bottom_reservation
    }

    /// A copy of the measured geometry.
    pub fn frame(&self) -> PaneFrame {
        self.0.borrow().clone()
    }

    /// The group's bounds in the last frame.
    pub fn bounds(&self) -> Bounds<Pixels> {
        self.0.borrow().bounds
    }

    /// The placement of `id` in the last frame.
    pub fn placement(&self, id: &PaneId) -> Option<PanePlacement> {
        self.0
            .borrow()
            .panes
            .iter()
            .find(|placement| &placement.id == id)
            .cloned()
    }
}

/// A size animated by an ANM-002 motion track: retargets from the visible value, snaps by
/// starting a fresh track at the new value.
pub(super) struct SizeTrack {
    track: MotionTrack,
    target: f32,
}

impl SizeTrack {
    pub fn at_rest(value: f32, transition: &MotionTransition, now: Duration) -> Self {
        let mut track = Self {
            track: MotionTrack::new(),
            target: value,
        };
        track.update(value, transition, now);
        track
    }

    fn update(&mut self, value: f32, transition: &MotionTransition, now: Duration) {
        self.target = value;
        let description = MotionDescription::new(MotionProps::new().width(f64::from(value)))
            .transition(transition.clone());
        self.track.update(&description, now);
    }

    pub fn target(&self) -> f32 {
        self.target
    }

    /// Eases from the currently visible value to `value`.
    pub fn retarget(&mut self, value: f32, transition: &MotionTransition, now: Duration) {
        self.update(value, transition, now);
    }

    /// Jumps to `value` without animation.
    pub fn snap(&mut self, value: f32, transition: &MotionTransition, now: Duration) {
        self.track = MotionTrack::new();
        self.update(value, transition, now);
    }

    /// The visible value and whether the track is still running.
    pub fn sample(&mut self, now: Duration, reduced_motion: bool) -> (f32, bool) {
        let sample = self.track.sample(now, reduced_motion);
        (sample.style.width.unwrap_or(self.target), sample.active)
    }

    pub fn value(&mut self, now: Duration) -> f32 {
        self.sample(now, false).0
    }

    pub fn is_active(&mut self, now: Duration) -> bool {
        self.sample(now, false).1
    }
}

/// Transition bookkeeping for one pane's slot.
pub(super) struct PaneRuntime {
    pub slot: SizeTrack,
    pub content: SizeTrack,
    /// Whether `content` holds a width the content was last laid out at (the
    /// slot has not rested at zero since).
    pub content_valid: bool,
    pub last_open: bool,
    pub last_adjustment: u64,
}

/// Transition bookkeeping for a whole group.
#[derive(Default)]
pub(super) struct GroupRuntime {
    pub panes: HashMap<PaneId, PaneRuntime>,
    pub viewport: Option<Size<Pixels>>,
}

/// The retained state of a pane group: descriptors, per-pane size, collapsed
/// flag and opening order. Observable (`cx.observe`, `cx.subscribe` for
/// [`PaneEvent`]) and settable by the caller.
pub struct PaneGroupState {
    main_min: Pixels,
    entries: Vec<PaneEntry>,
    next_order: u64,
    next_adjustment: u64,
    metrics: PaneMetrics,
    pub(super) runtime: Rc<RefCell<GroupRuntime>>,
}

impl EventEmitter<PaneEvent> for PaneGroupState {}

impl PaneGroupState {
    /// Creates a state from descriptors; panes that start open are opened in
    /// the order given. At most one bottom pane is supported, and ids must be
    /// unique; violating either is a programming error and panics.
    pub fn new(descriptors: impl IntoIterator<Item = PaneDescriptor>) -> Self {
        let mut state = Self {
            main_min: MAIN_MIN_WIDTH,
            entries: Vec::new(),
            next_order: 0,
            next_adjustment: 0,
            metrics: PaneMetrics::default(),
            runtime: Rc::default(),
        };
        for descriptor in descriptors {
            state.push_pane(descriptor);
        }
        state
    }

    fn push_pane(&mut self, descriptor: PaneDescriptor) {
        assert!(
            self.entry(&descriptor.id).is_none(),
            "duplicate pane id {:?}",
            descriptor.id
        );
        assert!(
            descriptor.side != PaneSide::Bottom
                || self
                    .entries
                    .iter()
                    .all(|entry| entry.descriptor.side != PaneSide::Bottom),
            "a pane group supports one bottom pane"
        );
        let order = self.next_order;
        self.next_order += 1;
        self.entries.push(PaneEntry {
            state: PaneState {
                size: descriptor.clamp_size(descriptor.preferred_size),
                collapsed: descriptor.collapsed,
                opened_order: order,
                adjustment: 0,
            },
            descriptor,
        });
    }

    /// Sets the main region's minimum width (default 300).
    pub fn with_main_min(mut self, main_min: Pixels) -> Self {
        self.main_min = main_min;
        self
    }

    /// The main region's minimum width.
    pub fn main_min(&self) -> Pixels {
        self.main_min
    }

    /// Adds a pane; it is opened after every existing pane.
    pub fn add_pane(&mut self, descriptor: PaneDescriptor, cx: &mut Context<Self>) {
        self.push_pane(descriptor);
        cx.notify();
    }

    /// All panes in descriptor order.
    pub fn entries(&self) -> &[PaneEntry] {
        &self.entries
    }

    /// The pane with the given id.
    pub fn entry(&self, id: &PaneId) -> Option<&PaneEntry> {
        self.entries.iter().find(|entry| &entry.descriptor.id == id)
    }

    fn entry_mut(&mut self, id: &PaneId) -> &mut PaneEntry {
        self.entries
            .iter_mut()
            .find(|entry| &entry.descriptor.id == id)
            .unwrap_or_else(|| panic!("unknown pane id {id:?}"))
    }

    /// The retained size preference of a pane.
    pub fn size(&self, id: &PaneId) -> Option<Pixels> {
        self.entry(id).map(|entry| entry.state.size)
    }

    /// Whether the pane's open flag is off.
    pub fn is_collapsed(&self, id: &PaneId) -> Option<bool> {
        self.entry(id).map(|entry| entry.state.collapsed)
    }

    /// The pane's opening order serial (lower means opened earlier).
    pub fn opened_order(&self, id: &PaneId) -> Option<u64> {
        self.entry(id).map(|entry| entry.state.opened_order)
    }

    /// The geometry measured by the group, readable by children.
    pub fn metrics(&self) -> PaneMetrics {
        self.metrics.clone()
    }

    /// The fit of this state's panes into `viewport`.
    pub fn fit(&self, viewport: Size<Pixels>) -> PaneFit {
        fit(viewport, self.main_min, &self.entries)
    }

    /// Sets a pane's retained size, clamped to the pane's own minimum and
    /// maximum (restored values are bounded by the descriptor, which for the
    /// bottom pane defaults to the 2000 pixel absolute maximum). The change
    /// applies directly, without a slot transition. Panics on an unknown id.
    pub fn set_size(&mut self, id: &PaneId, size: Pixels, cx: &mut Context<Self>) {
        let size = self.entry(id).map(|e| e.descriptor.clamp_size(size));
        let Some(size) = size else {
            panic!("unknown pane id {id:?}");
        };
        self.store_size(id, size, None, cx);
    }

    /// Applies a seam-derived size that was already clamped to the current
    /// limits. `reset_default` marks a seam reset.
    pub(super) fn store_size(
        &mut self,
        id: &PaneId,
        size: Pixels,
        reset_default: Option<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let size = self
            .entry(id)
            .map_or(size, |e| e.descriptor.clamp_size(size));
        self.next_adjustment += 1;
        let adjustment = self.next_adjustment;
        let entry = self.entry_mut(id);
        let changed = entry.state.size != size;
        entry.state.size = size;
        entry.state.adjustment = adjustment;
        if let Some(default) = reset_default {
            cx.emit(PaneEvent::Reset {
                pane: id.clone(),
                default,
                size,
            });
        } else if changed {
            cx.emit(PaneEvent::Resized {
                pane: id.clone(),
                size,
            });
        }
        if changed || reset_default.is_some() {
            cx.notify();
        }
    }

    /// Resets a pane to its default size, re-clamped to the limits of the
    /// last painted frame.
    pub fn reset_size(&mut self, id: &PaneId, cx: &mut Context<Self>) {
        let Some(entry) = self.entry(id) else {
            panic!("unknown pane id {id:?}");
        };
        let default = entry.descriptor.default_size;
        let limits = self
            .metrics
            .placement(id)
            .map(|placement| placement.limits)
            .unwrap_or(PaneLimits {
                min: entry.descriptor.min_size,
                max: entry.descriptor.max_size,
            });
        self.store_size(id, limits.clamp(default), Some(default), cx);
    }

    /// Opens or collapses a pane. Opening records a new opening order.
    /// Panics on an unknown id.
    pub fn set_collapsed(&mut self, id: &PaneId, collapsed: bool, cx: &mut Context<Self>) {
        let order = self.next_order;
        let entry = self.entry_mut(id);
        if entry.state.collapsed == collapsed {
            return;
        }
        entry.state.collapsed = collapsed;
        if !collapsed {
            entry.state.opened_order = order;
            self.next_order += 1;
        }
        cx.emit(PaneEvent::CollapsedChanged {
            pane: id.clone(),
            collapsed,
        });
        cx.notify();
    }

    /// Flips a pane's open flag.
    pub fn toggle_collapsed(&mut self, id: &PaneId, cx: &mut Context<Self>) {
        let collapsed = self
            .is_collapsed(id)
            .unwrap_or_else(|| panic!("unknown pane id {id:?}"));
        self.set_collapsed(id, !collapsed, cx);
    }
}
