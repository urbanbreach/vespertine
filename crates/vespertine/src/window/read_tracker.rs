//! Per-view dependency tracking for retained frames (PRF-001 R3).
//!
//! While a view is being built (render, layout, prepaint, paint) the window
//! pushes a [`ReadFrame`] here; every entity read/update, global read, global
//! presence query, pointer/modifier/caps-lock query and versioned scroll/list
//! read made while that frame is on top of the stack is recorded into the
//! frame's [`ReadSet`]. Writes are versioned with a monotonic clock so the
//! window can later decide whether a view's recorded reads are stale.

use crate::{EntityId, HitboxId};
use std::{
    any::TypeId,
    cell::{Cell, RefCell},
    rc::Rc,
};
use vespertine_collections::{FxHashMap, FxHashSet};

slotmap::new_key_type! {
    /// Identity of a retained view record in the window's slot map; the key's
    /// version guards against recycled slots.
    pub(crate) struct ViewKey;
}

/// A shared counter that stateful scroll and list objects bump whenever the
/// data a view may have read from them changes (PRF-004/PRF-005 versioned
/// reads).
pub(crate) type VersionCell = Rc<Cell<u64>>;

/// Everything one view (excluding the views nested inside it) read while it
/// was built.
#[derive(Default, Clone)]
pub(crate) struct ReadSet {
    pub entities: FxHashSet<EntityId>,
    /// Globals whose value was read.
    pub globals: FxHashSet<TypeId>,
    /// Hitboxes whose hover state was queried: the view's output depends on whether they are
    /// hovered (PRF-001 R5).
    pub hovers: FxHashSet<HitboxId>,
    /// Globals whose presence (existence) alone was queried.
    pub presence: FxHashSet<TypeId>,
    pub pointer: bool,
    pub modifiers: bool,
    pub capslock: bool,
    /// Versioned reads: the counter and the value observed when it was read.
    pub versioned: Vec<(VersionCell, u64)>,
}

impl ReadSet {
    pub fn versioned_changed(&self) -> bool {
        self.versioned
            .iter()
            .any(|(cell, seen)| cell.get() != *seen)
    }

    pub fn extend_from(&mut self, other: &ReadSet) {
        self.entities.extend(other.entities.iter().copied());
        self.globals.extend(other.globals.iter().copied());
        self.presence.extend(other.presence.iter().copied());
        self.hovers.extend(other.hovers.iter().copied());
        self.pointer |= other.pointer;
        self.modifiers |= other.modifiers;
        self.capslock |= other.capslock;
        for (cell, seen) in &other.versioned {
            if !self.versioned.iter().any(|(c, _)| Rc::ptr_eq(c, cell)) {
                self.versioned.push((cell.clone(), *seen));
            }
        }
    }
}

struct ReadFrame {
    key: ViewKey,
    reads: ReadSet,
}

/// A write observed while a draw was in progress, attributed to the view that
/// was being built (if any).
#[derive(Clone, Copy, Debug)]
pub(crate) struct DrawWrite {
    pub target: WriteTarget,
    pub writer: Option<ViewKey>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WriteTarget {
    Entity(EntityId),
    Global(TypeId),
    Presence(TypeId),
}

#[derive(Default)]
pub(crate) struct ReadTracker {
    frames: RefCell<Vec<ReadFrame>>,
    drawing: Cell<bool>,
    clock: Cell<u64>,
    entity_clock: RefCell<FxHashMap<EntityId, u64>>,
    global_clock: RefCell<FxHashMap<TypeId, u64>>,
    presence_clock: RefCell<FxHashMap<TypeId, u64>>,
    draw_writes: RefCell<Vec<DrawWrite>>,
    /// The logical time of the latest global write (value or presence).
    last_global_write: Cell<u64>,
    /// Number of entity updates (any time), for tests and diagnostics.
    /// Depth of read-only platform queries (input-method queries): updates
    /// made by them do not count unless the entity notifies.
    query_depth: Cell<u32>,
    query_updates: RefCell<FxHashSet<EntityId>>,
    query_notifies: RefCell<FxHashSet<EntityId>>,
    /// Notifications since the preceding frame, and how many of them came from scroll
    /// listeners and animation-frame requests (PRF-004 R2, PRF-005 R5).
    notify_calls: Cell<u64>,
    scroll_notifies: Cell<u64>,
    animation_notifies: Cell<u64>,
}
/// What caused the notifications received since the preceding frame.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct FrameCauses {
    /// Some notification is neither a scroll nor an animation-frame notification: the
    /// notified views may render different output for reasons nothing tracked explains.
    #[allow(dead_code, reason = "read by scroll layers (PRF-004 R2)")]
    pub unexplained: bool,
    /// How many notifications were neither scroll nor animation-frame notifications.
    pub unexplained_count: u64,
    /// A scroll listener notified.
    #[allow(dead_code, reason = "read by scroll layers (PRF-004 R2)")]
    pub scrolled: bool,
}

thread_local! {
    /// The tracker of the draw in progress on this thread, for reads made by state that has
    /// no window at hand (versioned list and scroll queries).
    static DRAWING: RefCell<Option<Rc<ReadTracker>>> = const { RefCell::new(None) };
}

/// Records that the view being built read the versioned counter `cell`: a no-op outside a draw.
pub(crate) fn read_versioned_in_draw(cell: &VersionCell) {
    DRAWING.with(|drawing| {
        if let Some(tracker) = drawing.borrow().as_ref() {
            tracker.read_versioned(cell);
        }
    });
}

impl ReadTracker {
    pub fn new() -> Rc<Self> {
        Rc::new(Self::default())
    }

    /// The current logical time. Every write advances it.
    pub fn now(&self) -> u64 {
        self.clock.get()
    }

    fn tick(&self) -> u64 {
        let next = self.clock.get() + 1;
        self.clock.set(next);
        next
    }

    /// Starts a draw: reads and writes are attributed to views until [`Self::end_draw`].
    pub fn begin_draw(this: &Rc<Self>) {
        this.drawing.set(true);
        DRAWING.with(|drawing| *drawing.borrow_mut() = Some(this.clone()));
    }

    /// Ends the draw started by [`Self::begin_draw`].
    pub fn end_draw(this: &Rc<Self>) {
        this.drawing.set(false);
        DRAWING.with(|drawing| *drawing.borrow_mut() = None);
    }

    pub fn push_frame(&self, key: ViewKey, reads: ReadSet) {
        self.frames.borrow_mut().push(ReadFrame { key, reads });
    }

    pub fn pop_frame(&self) -> ReadSet {
        self.frames
            .borrow_mut()
            .pop()
            .expect("read frame stack underflow")
            .reads
    }

    pub fn current_view(&self) -> Option<ViewKey> {
        self.frames.borrow().last().map(|frame| frame.key)
    }

    #[inline]
    pub fn read_entity(&self, id: EntityId) {
        if let Some(frame) = self.frames.borrow_mut().last_mut() {
            frame.reads.entities.insert(id);
        }
    }

    #[inline]
    pub fn read_global(&self, type_id: TypeId) {
        if let Some(frame) = self.frames.borrow_mut().last_mut() {
            frame.reads.globals.insert(type_id);
        }
    }

    #[inline]
    pub fn read_presence(&self, type_id: TypeId) {
        if let Some(frame) = self.frames.borrow_mut().last_mut() {
            frame.reads.presence.insert(type_id);
        }
    }

    #[inline]
    pub fn read_pointer(&self) {
        if let Some(frame) = self.frames.borrow_mut().last_mut() {
            frame.reads.pointer = true;
        }
    }

    /// Records that the view being built asked whether `hitbox` is hovered.
    #[inline]
    pub fn read_hover(&self, hitbox: HitboxId) {
        if let Some(frame) = self.frames.borrow_mut().last_mut() {
            frame.reads.hovers.insert(hitbox);
        }
    }

    #[inline]
    pub fn read_modifiers(&self) {
        if let Some(frame) = self.frames.borrow_mut().last_mut() {
            frame.reads.modifiers = true;
        }
    }

    #[inline]
    pub fn read_capslock(&self) {
        if let Some(frame) = self.frames.borrow_mut().last_mut() {
            frame.reads.capslock = true;
        }
    }

    /// Records that the current view read a versioned counter at `seen`.
    #[allow(
        dead_code,
        reason = "the versioned scroll and list reads of PRF-004 and PRF-005 call this"
    )]
    pub fn read_versioned(&self, cell: &VersionCell) {
        if let Some(frame) = self.frames.borrow_mut().last_mut() {
            let versioned = &mut frame.reads.versioned;
            if !versioned.iter().any(|(c, _)| Rc::ptr_eq(c, cell)) {
                versioned.push((cell.clone(), cell.get()));
            }
        }
    }

    /// Called when an entity lease ends: the entity may have been mutated.
    pub fn entity_updated(&self, id: EntityId) {
        if self.query_depth.get() > 0 {
            self.query_updates.borrow_mut().insert(id);
            return;
        }
        let now = self.tick();
        self.entity_clock.borrow_mut().insert(id, now);
        if self.drawing.get() {
            let writer = self.current_view();
            self.draw_writes.borrow_mut().push(DrawWrite {
                target: WriteTarget::Entity(id),
                writer,
            });
        }
    }

    /// Called when an entity notifies.
    pub fn entity_notified(&self, id: EntityId) {
        self.notify_calls.set(self.notify_calls.get() + 1);
        if self.query_depth.get() > 0 {
            self.query_notifies.borrow_mut().insert(id);
        }
    }

    /// Records that a scroll listener is about to notify (once per notification).
    pub fn note_scroll_notification(&self) {
        self.scroll_notifies.set(self.scroll_notifies.get() + 1);
    }

    /// Records that a hover transition is about to notify: rows predict hover themselves, so
    /// the notification does not make the owner of a list suspicious (PRF-005 R3).
    pub fn note_hover_notification(&self) {
        self.animation_notifies
            .set(self.animation_notifies.get() + 1);
    }

    /// Records that an animation-frame request is about to notify.
    pub fn note_animation_notification(&self) {
        self.animation_notifies
            .set(self.animation_notifies.get() + 1);
    }

    /// Classifies the notifications since `baseline` (the counters the caller saw last) and
    /// moves `baseline` to the current counters. The counters are shared by every window of
    /// the app and never reset, so each window keeps its own baseline.
    pub fn causes_since(&self, baseline: &mut [u64; 3]) -> FrameCauses {
        let now = [
            self.notify_calls.get(),
            self.scroll_notifies.get(),
            self.animation_notifies.get(),
        ];
        let [calls, scrolls, animations] = [
            now[0] - baseline[0],
            now[1] - baseline[1],
            now[2] - baseline[2],
        ];
        *baseline = now;
        FrameCauses {
            unexplained: calls > scrolls + animations,
            unexplained_count: calls.saturating_sub(scrolls + animations),
            scrolled: scrolls > 0,
        }
    }

    /// Runs a read-only platform query (selected range, marked range, text
    /// for range, bounds for range, ...): entity updates made by it are not
    /// counted unless the updated entity notifies.
    pub fn with_query<R>(&self, query: impl FnOnce() -> R) -> R {
        self.query_depth.set(self.query_depth.get() + 1);
        let result = query();
        let depth = self.query_depth.get() - 1;
        self.query_depth.set(depth);
        if depth == 0 {
            let updates = std::mem::take(&mut *self.query_updates.borrow_mut());
            let notifies = std::mem::take(&mut *self.query_notifies.borrow_mut());
            for id in updates.intersection(&notifies) {
                self.entity_updated(*id);
            }
        }
        result
    }

    /// Called when the value of a global may have changed. `presence_changed`
    /// is true when the global was inserted or removed.
    pub fn global_written(&self, type_id: TypeId, presence_changed: bool) {
        let now = self.tick();
        self.global_clock.borrow_mut().insert(type_id, now);
        if presence_changed {
            self.presence_clock.borrow_mut().insert(type_id, now);
        }
        self.last_global_write.set(now);
        if self.drawing.get() {
            let writer = self.current_view();
            let mut writes = self.draw_writes.borrow_mut();
            writes.push(DrawWrite {
                target: WriteTarget::Global(type_id),
                writer,
            });
            if presence_changed {
                writes.push(DrawWrite {
                    target: WriteTarget::Presence(type_id),
                    writer,
                });
            }
        }
    }

    /// The logical time of the latest update of `id` (0 if never updated).
    pub fn entity_updated_at(&self, id: EntityId) -> u64 {
        self.entity_clock.borrow().get(&id).copied().unwrap_or(0)
    }

    pub fn global_value_written_at(&self, type_id: TypeId) -> u64 {
        self.global_clock
            .borrow()
            .get(&type_id)
            .copied()
            .unwrap_or(0)
    }

    pub fn global_presence_written_at(&self, type_id: TypeId) -> u64 {
        self.presence_clock
            .borrow()
            .get(&type_id)
            .copied()
            .unwrap_or(0)
    }

    pub fn forget_entity(&self, id: EntityId) {
        self.entity_clock.borrow_mut().remove(&id);
    }

    pub fn take_draw_writes(&self) -> Vec<DrawWrite> {
        std::mem::take(&mut *self.draw_writes.borrow_mut())
    }

    pub fn has_draw_writes(&self) -> bool {
        !self.draw_writes.borrow().is_empty()
    }

    /// The logical time of the latest write of any global (0 if none was ever written).
    pub fn last_global_write(&self) -> u64 {
        self.last_global_write.get()
    }
}
