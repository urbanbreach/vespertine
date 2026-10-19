//! The per-frame record of what a host actually painted.
//!
//! The log is an application global installed by the host. It records, for the last painted
//! frame only: the strings painted in paint order, the painted border-box bounds of identified
//! elements and the painted highlight washes. The host clears it exactly once at the start of
//! every frame it draws ([`PaintLog::begin_frame`]); retained-tree element builders report into
//! it from their paint observers, so an element that was hidden, not painted, or only painted
//! speculatively has no bounds.

use crate::types::{PaintedWash, Rect};
use std::{cell::RefCell, rc::Rc};
use vespertine::{App, Bounds, Global, Pixels};

#[derive(Default)]
struct State {
    frames: u64,
    /// `(identity, bounds, stacking order)`; a higher order paints above a lower one.
    bounds: Vec<(u64, Rect, u64)>,
    next_order: u64,
    strings: Vec<String>,
    washes: Vec<PaintedWash>,
}

/// The last-frame paint log. Cloning shares the log.
#[derive(Clone, Default)]
pub struct PaintLog {
    state: Rc<RefCell<State>>,
}

impl Global for PaintLog {}

impl PaintLog {
    /// Clears the log for a new frame. The host calls this exactly once per drawn frame.
    pub fn begin_frame(&self) {
        let mut state = self.state.borrow_mut();
        state.frames += 1;
        state.bounds.clear();
        state.next_order = 0;
        state.strings.clear();
        state.washes.clear();
    }

    /// The number of frames begun so far.
    pub fn frames(&self) -> u64 {
        self.state.borrow().frames
    }

    /// Records the painted border-box bounds of element `id` (window coordinates, logical
    /// pixels) into the application's log; elements recorded later stack above earlier ones. A
    /// second record of the same identity within a frame (an element painted twice) replaces
    /// the first and takes the later paint position. Does nothing when the application has no
    /// log.
    ///
    /// An `on_painted` observer (DEV-005) reports after the element's descendants have painted,
    /// so a host whose elements nest should call [`PaintLog::record_bounds_ordered`] with the
    /// element's pre-order position instead.
    pub fn record_bounds(cx: &App, id: u64, bounds: Bounds<Pixels>) {
        if let Some(log) = cx.try_global::<PaintLog>() {
            let mut state = log.state.borrow_mut();
            let order = state.next_order;
            Self::insert(&mut state, id, order, bounds);
        }
    }

    /// Like [`PaintLog::record_bounds`] with an explicit stacking `order` (the element's
    /// pre-order position in the frame): a higher order is topmost in
    /// [`PaintLog::elements_at`].
    pub fn record_bounds_ordered(cx: &App, id: u64, order: u64, bounds: Bounds<Pixels>) {
        if let Some(log) = cx.try_global::<PaintLog>() {
            Self::insert(&mut log.state.borrow_mut(), id, order, bounds);
        }
    }

    fn insert(state: &mut State, id: u64, order: u64, bounds: Bounds<Pixels>) {
        state.bounds.retain(|(existing, ..)| *existing != id);
        state.bounds.push((id, bounds.into(), order));
        state.next_order = state.next_order.max(order) + 1;
    }

    /// Records a painted string that did not pass through the shaped-line painter (for example
    /// generated chrome text). Strings painted through shaped lines are recorded
    /// automatically. Does nothing when the application has no log.
    pub fn record_text(cx: &App, text: &str) {
        if let Some(log) = cx.try_global::<PaintLog>() {
            log.push_text(text);
        }
    }

    /// Records a painted highlight wash. Does nothing when the application has no log.
    pub fn record_wash(cx: &App, wash: PaintedWash) {
        if let Some(log) = cx.try_global::<PaintLog>() {
            log.state.borrow_mut().washes.push(wash);
        }
    }

    pub(crate) fn push_text(&self, text: &str) {
        self.state.borrow_mut().strings.push(text.to_owned());
    }

    /// The strings painted in the last frame, in paint order.
    pub fn painted_text(&self) -> Vec<String> {
        self.state.borrow().strings.clone()
    }

    /// The highlight washes painted in the last frame, in paint order.
    pub fn painted_washes(&self) -> Vec<PaintedWash> {
        self.state.borrow().washes.clone()
    }

    /// The last-painted bounds of element `id`.
    pub fn bounds_of(&self, id: u64) -> Option<Rect> {
        self.state
            .borrow()
            .bounds
            .iter()
            .find(|(existing, ..)| *existing == id)
            .map(|(_, rect, _)| *rect)
    }

    /// Identities of the elements whose last-painted bounds contain the point, topmost
    /// (painted last) first.
    pub fn elements_at(&self, x: f64, y: f64) -> Vec<u64> {
        let state = self.state.borrow();
        let mut hits: Vec<_> = state
            .bounds
            .iter()
            .filter(|(_, rect, _)| rect.contains(x, y))
            .map(|(id, _, order)| (*order, *id))
            .collect();
        hits.sort_by_key(|hit| std::cmp::Reverse(hit.0));
        hits.into_iter().map(|(_, id)| id).collect()
    }

    /// Every element painted in the last frame with its bounds, in stacking order (bottom
    /// first).
    pub fn all_bounds(&self) -> Vec<(u64, Rect)> {
        let state = self.state.borrow();
        let mut all: Vec<_> = state.bounds.iter().collect();
        all.sort_by_key(|(_, _, order)| *order);
        all.into_iter().map(|(id, rect, _)| (*id, *rect)).collect()
    }
}
