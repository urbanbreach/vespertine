//! Retained view records (PRF-001).
//!
//! Every entity-backed view that is built gets a [`ViewRecord`] that survives
//! across frames. The record remembers what the view read, the ranges of
//! prepaint and paint output it produced in the previously drawn frame and the
//! layout nodes it owns. A later frame replays the output instead of building
//! the view again when nothing the view depends on changed.

use super::{
    ElementIdStack, OwnedLayoutNodes, PaintIndex, PrepaintStateIndex, Window,
    read_tracker::{ReadSet, ReadTracker, ViewKey, WriteTarget},
};
use crate::key_dispatch::DispatchNodeId;
use crate::taffy::LayoutKeyContext;
use crate::{Bounds, ContentMask, EntityId, GlobalElementId, LayoutId, Pixels, TextStyle};
use smallvec::SmallVec;
use std::{any::TypeId, ops::Range};
use vespertine_collections::{FxHashMap, FxHashSet};

/// Why a view is being embedded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ViewKind {
    /// An ordinary entity-backed view: automatic dependency tracking.
    Retained,
    /// An explicitly cached view (`cached(style)`): upstream invalidation
    /// rules (not invalidated by an unnotified update of an entity it read).
    Cached,
}

/// The ambient inputs a replay requires to be unchanged.
#[derive(Clone, PartialEq)]
pub(crate) struct CacheKey {
    pub bounds: Bounds<Pixels>,
    pub content_mask: ContentMask<Pixels>,
    pub text_style: TextStyle,
    pub opacity: f32,
    pub rem_size: Pixels,
    /// The retention context in force (see [`Window::with_retention_context`]).
    pub context: u64,
    /// Inside an inert subtree: such views contribute no input or accessibility output.
    pub inert: bool,
}

/// Ambient window state a view was built in, kept so the view can be built
/// again out of band when a clean ancestor is replayed around it (PRF-001 R6).
#[derive(Clone)]
pub(crate) struct Ambient {
    pub element_id_stack: ElementIdStack,
    pub text_style_stack: crate::text_system::TextStyleStack,
    pub rem_override: SmallVec<[Pixels; 8]>,
    pub rendered_entities: Vec<EntityId>,
    pub image_caches: Vec<crate::AnyImageCache>,
    pub retention_context: u64,
    /// Prepaint-phase state; `None` until the view was prepainted.
    pub prepaint: Option<PrepaintAmbient>,
}

#[derive(Clone)]
pub(crate) struct PrepaintAmbient {
    pub content_mask: ContentMask<Pixels>,
    pub opacity: f32,
    pub element_offset: crate::Point<Pixels>,
    /// Index of the parent of the view's wrapper node in the dispatch tree of
    /// the frame the view was prepainted in.
    pub dispatch_parent: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AmbientPhase {
    Layout,
    Prepaint,
}

pub(crate) struct ViewRecord {
    pub path: GlobalElementId,
    pub entity_id: EntityId,
    pub kind: ViewKind,
    /// The record of the nearest enclosing view in the tree.
    pub parent: Option<ViewKey>,
    /// Records of views nested directly in this view's output (in the order
    /// they were first built or visited).
    pub children: Vec<ViewKey>,
    /// Bumped every time this record starts a rebuild; a child registers
    /// itself with its parent once per parent build.
    pub build_epoch: u64,
    pub registered_in_parent_epoch: u64,
    /// This view's own inputs changed.
    pub dirty: bool,
    /// Some view nested in this one is dirty.
    pub dirty_descendants: bool,
    /// The view's output depends on all of its nested views being built together with it
    /// (see [`Window::require_whole_view_rebuild`]): it is never replayed around rebuilt views.
    pub splice_unsafe: bool,
    /// This view was notified since the preceding frame.
    pub notified: bool,
    /// Reads of this view alone (nested views excluded).
    pub reads: ReadSet,
    /// Logical time at which the reads were completed.
    pub built_clock: u64,
    pub layout: OwnedLayoutNodes,
    /// Element states the view's own layout phase accessed (nested views
    /// excluded). A replayed view does not run its layout phase, so these are
    /// carried into the new frame explicitly; the prepaint and paint phases
    /// carry theirs through their ranges.
    pub layout_states: Vec<(GlobalElementId, TypeId)>,
    pub layout_root: Option<LayoutId>,
    /// The layout key frame of the view when it was last built in the enclosing
    /// layout, used to request layout for it again in place.
    pub layout_key: Option<LayoutKeyContext>,
    /// The view handle, to render it again out of band.
    pub view: Option<crate::AnyView>,
    pub ambient: Option<std::rc::Rc<Ambient>>,
    pub prepaint: Option<Range<PrepaintStateIndex>>,
    pub paint: Option<Range<PaintIndex>>,
    pub cache_key: Option<CacheKey>,
    /// Frame epoch in which the record was last built or replayed.
    pub live_epoch: u64,
}

enum Slot {
    Occupied { generation: u32, record: ViewRecord },
    Free { generation: u32 },
}

/// Counters for tests and benchmarks (DEV-003).
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct RetentionStats {
    /// Views whose render ran.
    pub views_rendered: u64,
    /// Views whose output was replayed instead of rebuilt.
    pub views_reused: u64,
    /// Parents replayed around freshly rebuilt children.
    pub splice_successes: u64,
    /// Splice attempts that fell back to full reconstruction.
    pub splice_failures: u64,
}

#[derive(Default)]
pub(crate) struct RetainedViews {
    slots: Vec<Slot>,
    free: Vec<u32>,
    by_path: FxHashMap<GlobalElementId, ViewKey>,
    by_entity: FxHashMap<EntityId, SmallVec<[ViewKey; 1]>>,
    entity_readers: FxHashMap<EntityId, SmallVec<[ViewKey; 2]>>,
    global_readers: FxHashMap<TypeId, SmallVec<[ViewKey; 2]>>,
    presence_readers: FxHashMap<TypeId, SmallVec<[ViewKey; 2]>>,
    pointer_readers: FxHashSet<ViewKey>,
    modifier_readers: FxHashSet<ViewKey>,
    capslock_readers: FxHashSet<ViewKey>,
    pub frame_epoch: u64,
    pub stats: RetentionStats,
}

/// Inputs that changed since the preceding frame.
#[derive(Default)]
pub(crate) struct InputChanges {
    pub pointer: bool,
    pub modifiers: bool,
    pub capslock: bool,
}

fn remove_key<K: Eq + std::hash::Hash + Copy, const N: usize>(
    map: &mut FxHashMap<K, SmallVec<[ViewKey; N]>>,
    k: K,
    key: ViewKey,
) {
    if let Some(list) = map.get_mut(&k) {
        list.retain(|candidate| *candidate != key);
        if list.is_empty() {
            map.remove(&k);
        }
    }
}

fn add_key<K: Eq + std::hash::Hash + Copy, const N: usize>(
    map: &mut FxHashMap<K, SmallVec<[ViewKey; N]>>,
    k: K,
    key: ViewKey,
) {
    let list = map.entry(k).or_default();
    if !list.contains(&key) {
        list.push(key);
    }
}

impl RetainedViews {
    pub fn get(&self, key: ViewKey) -> Option<&ViewRecord> {
        match self.slots.get(key.index as usize)? {
            Slot::Occupied { generation, record } if *generation == key.generation => Some(record),
            _ => None,
        }
    }

    pub fn get_mut(&mut self, key: ViewKey) -> Option<&mut ViewRecord> {
        match self.slots.get_mut(key.index as usize)? {
            Slot::Occupied { generation, record } if *generation == key.generation => Some(record),
            _ => None,
        }
    }

    pub fn lookup(&self, path: &GlobalElementId) -> Option<ViewKey> {
        self.by_path.get(path).copied()
    }

    pub fn create(
        &mut self,
        path: GlobalElementId,
        entity_id: EntityId,
        kind: ViewKind,
    ) -> ViewKey {
        let record = ViewRecord {
            path: path.clone(),
            entity_id,
            kind,
            parent: None,
            children: Vec::new(),
            build_epoch: 0,
            registered_in_parent_epoch: u64::MAX,
            dirty: true,
            dirty_descendants: false,
            splice_unsafe: false,
            notified: false,
            reads: ReadSet::default(),
            built_clock: 0,
            layout: OwnedLayoutNodes::default(),
            layout_states: Vec::new(),
            layout_root: None,
            layout_key: None,
            view: None,
            ambient: None,
            prepaint: None,
            paint: None,
            cache_key: None,
            live_epoch: self.frame_epoch,
        };
        let key = if let Some(index) = self.free.pop() {
            let generation = match &self.slots[index as usize] {
                Slot::Free { generation } => *generation,
                Slot::Occupied { .. } => unreachable!("free list points at an occupied slot"),
            };
            self.slots[index as usize] = Slot::Occupied { generation, record };
            ViewKey { index, generation }
        } else {
            let index = self.slots.len() as u32;
            self.slots.push(Slot::Occupied {
                generation: 0,
                record,
            });
            ViewKey {
                index,
                generation: 0,
            }
        };
        self.by_path.insert(path, key);
        add_key(&mut self.by_entity, entity_id, key);
        key
    }

    /// Replaces the dependency set of a record and keeps the reverse indices
    /// in sync.
    /// Forgets the reads of a record that is about to be built again.
    pub fn reset_reads(&mut self, key: ViewKey) {
        if let Some(record) = self.get_mut(key) {
            let old = std::mem::take(&mut record.reads);
            self.unindex_reads(key, &old);
        }
    }

    /// Adds reads made in a later build phase of the same view.
    pub fn append_reads(&mut self, key: ViewKey, reads: ReadSet, clock: u64) {
        self.index_reads(key, &reads);
        if let Some(record) = self.get_mut(key) {
            record.reads.extend_from(&reads);
            record.built_clock = clock;
        }
    }

    fn unindex_reads(&mut self, key: ViewKey, reads: &ReadSet) {
        for entity in &reads.entities {
            remove_key(&mut self.entity_readers, *entity, key);
        }
        for global in &reads.globals {
            remove_key(&mut self.global_readers, *global, key);
        }
        for global in &reads.presence {
            remove_key(&mut self.presence_readers, *global, key);
        }
        if reads.pointer {
            self.pointer_readers.remove(&key);
        }
        if reads.modifiers {
            self.modifier_readers.remove(&key);
        }
        if reads.capslock {
            self.capslock_readers.remove(&key);
        }
    }

    fn index_reads(&mut self, key: ViewKey, reads: &ReadSet) {
        for entity in &reads.entities {
            add_key(&mut self.entity_readers, *entity, key);
        }
        for global in &reads.globals {
            add_key(&mut self.global_readers, *global, key);
        }
        for global in &reads.presence {
            add_key(&mut self.presence_readers, *global, key);
        }
        if reads.pointer {
            self.pointer_readers.insert(key);
        }
        if reads.modifiers {
            self.modifier_readers.insert(key);
        }
        if reads.capslock {
            self.capslock_readers.insert(key);
        }
    }

    /// Every entity any live record read (the window's tracked set).
    pub fn tracked_entities(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.entity_readers
            .keys()
            .copied()
            .chain(self.by_entity.keys().copied())
    }

    /// Marks `key` dirty and every ancestor as having a dirty descendant.
    pub fn flag_dirty(&mut self, key: ViewKey) {
        let Some(record) = self.get_mut(key) else {
            return;
        };
        record.dirty = true;
        let mut parent = record.parent;
        while let Some(parent_key) = parent {
            let Some(parent_record) = self.get_mut(parent_key) else {
                break;
            };
            if parent_record.dirty_descendants {
                break;
            }
            parent_record.dirty_descendants = true;
            parent = parent_record.parent;
        }
    }

    /// Applies everything that happened since the preceding frame: notified
    /// entities, global writes and input changes.
    pub fn begin_frame(
        &mut self,
        tracker: &ReadTracker,
        notified: &FxHashSet<EntityId>,
        inputs: &InputChanges,
    ) {
        self.frame_epoch += 1;
        for record in self.slots.iter_mut().filter_map(|slot| match slot {
            Slot::Occupied { record, .. } => Some(record),
            Slot::Free { .. } => None,
        }) {
            record.notified = false;
        }
        for entity in notified {
            let views = self.by_entity.get(entity).cloned().unwrap_or_default();
            for key in views {
                if let Some(record) = self.get_mut(key) {
                    record.notified = true;
                }
                self.flag_dirty(key);
            }
            let updated_at = tracker.entity_updated_at(*entity);
            let readers = self.entity_readers.get(entity).cloned().unwrap_or_default();
            for key in readers {
                if self
                    .get(key)
                    .is_some_and(|record| updated_at > record.built_clock)
                {
                    self.flag_dirty(key);
                }
            }
        }
        // A notified view re-renders, and its ordinary descendants are then
        // re-evaluated against updates that were never notified; explicitly
        // cached views (and what they contain) keep the upstream rule.
        let notified_views: Vec<ViewKey> = notified
            .iter()
            .filter_map(|entity| self.by_entity.get(entity))
            .flatten()
            .copied()
            .collect();
        for root in notified_views {
            let mut stack: SmallVec<[ViewKey; 16]> = SmallVec::new();
            if let Some(record) = self.get(root) {
                stack.extend(record.children.iter().copied());
            }
            while let Some(key) = stack.pop() {
                let Some(record) = self.get(key) else {
                    continue;
                };
                if record.kind == ViewKind::Cached {
                    continue;
                }
                stack.extend(record.children.iter().copied());
                if Self::reads_stale(tracker, record) {
                    self.flag_dirty(key);
                }
            }
        }
        let changes = tracker.take_global_changes();
        for global in changes.values {
            let readers = self
                .global_readers
                .get(&global)
                .cloned()
                .unwrap_or_default();
            let written_at = tracker.global_value_written_at(global);
            for key in readers {
                if self
                    .get(key)
                    .is_some_and(|record| written_at > record.built_clock)
                {
                    self.flag_dirty(key);
                }
            }
        }
        for global in changes.presence {
            let readers = self
                .presence_readers
                .get(&global)
                .cloned()
                .unwrap_or_default();
            let written_at = tracker.global_presence_written_at(global);
            for key in readers {
                if self
                    .get(key)
                    .is_some_and(|record| written_at > record.built_clock)
                {
                    self.flag_dirty(key);
                }
            }
        }
        if inputs.pointer {
            for key in self.pointer_readers.iter().copied().collect::<Vec<_>>() {
                self.flag_dirty(key);
            }
        }
        if inputs.modifiers {
            for key in self.modifier_readers.iter().copied().collect::<Vec<_>>() {
                self.flag_dirty(key);
            }
        }
        if inputs.capslock {
            for key in self.capslock_readers.iter().copied().collect::<Vec<_>>() {
                self.flag_dirty(key);
            }
        }
    }

    /// Applies writes that happened while drawing: every reader other than
    /// the writing view is invalidated.
    pub fn flush_draw_writes(&mut self, tracker: &ReadTracker) {
        if !tracker.has_draw_writes() {
            return;
        }
        for write in tracker.take_draw_writes() {
            let readers: SmallVec<[ViewKey; 2]> = match write.target {
                WriteTarget::Entity(entity) => self
                    .entity_readers
                    .get(&entity)
                    .cloned()
                    .unwrap_or_default(),
                WriteTarget::Global(global) => self
                    .global_readers
                    .get(&global)
                    .cloned()
                    .unwrap_or_default(),
                WriteTarget::Presence(global) => self
                    .presence_readers
                    .get(&global)
                    .cloned()
                    .unwrap_or_default(),
            };
            for key in readers {
                if Some(key) != write.writer {
                    self.flag_dirty(key);
                }
            }
        }
    }

    /// Whether `reads` observed a global or entity write that happened after
    /// `built_clock`. Used for descendants of a notified ancestor (PRF-001
    /// R4: unnotified updates are re-evaluated when an ancestor re-renders).
    pub fn reads_stale(tracker: &ReadTracker, record: &ViewRecord) -> bool {
        record
            .reads
            .entities
            .iter()
            .any(|entity| tracker.entity_updated_at(*entity) > record.built_clock)
            || record.reads.versioned_changed()
    }

    /// The topmost dirty views nested in `root`, or `None` if one of them
    /// cannot be rebuilt out of band.
    pub fn dirty_frontier(&self, root: ViewKey) -> Option<Vec<ViewKey>> {
        let mut frontier = Vec::new();
        let mut stack: SmallVec<[ViewKey; 16]> = SmallVec::new();
        stack.extend(self.get(root)?.children.iter().copied());
        while let Some(key) = stack.pop() {
            let record = self.get(key)?;
            // A view that cannot be replayed around rebuilt views is rebuilt itself when any
            // view nested in it is dirty.
            if record.dirty || (record.splice_unsafe && record.dirty_descendants) {
                let rebuildable = record.kind == ViewKind::Retained
                    && record.view.is_some()
                    && record.layout_root.is_some()
                    && record.layout_key.is_some()
                    && record.prepaint.is_some()
                    && record.paint.is_some()
                    && record
                        .ambient
                        .as_ref()
                        .is_some_and(|ambient| ambient.prepaint.is_some());
                if !rebuildable {
                    return None;
                }
                frontier.push(key);
            } else if record.dirty_descendants {
                stack.extend(record.children.iter().copied());
            }
        }
        Some(frontier)
    }

    /// Whether `key` is nested (at any depth) in `ancestor`.
    pub fn is_nested_in(&self, key: ViewKey, ancestor: ViewKey) -> bool {
        let mut current = self.get(key).and_then(|record| record.parent);
        while let Some(parent) = current {
            if parent == ancestor {
                return true;
            }
            current = self.get(parent).and_then(|record| record.parent);
        }
        false
    }

    /// Clears the dirty-descendant flags of `root` and of the views between it
    /// and the dirty views nested in it, after those were rebuilt.
    pub fn clear_dirty_descendants(&mut self, root: ViewKey) {
        let mut stack: SmallVec<[ViewKey; 16]> = SmallVec::new();
        stack.push(root);
        while let Some(key) = stack.pop() {
            let Some(record) = self.get_mut(key) else {
                continue;
            };
            if !std::mem::replace(&mut record.dirty_descendants, false) {
                continue;
            }
            stack.extend(record.children.iter().copied());
        }
    }

    /// Prepares `key` for a rebuild: forgets its children and clears its
    /// invalidation flags.
    pub fn begin_rebuild(&mut self, key: ViewKey, parent: Option<ViewKey>) {
        let epoch = self.frame_epoch;
        if let Some(record) = self.get_mut(key) {
            record.build_epoch += 1;
            record.children.clear();
            record.dirty = false;
            record.dirty_descendants = false;
            record.splice_unsafe = false;
            record.parent = parent;
            record.live_epoch = epoch;
        }
        self.register_with_parent(key);
    }

    /// Registers `key` as a child of its parent (once per parent build).
    pub fn register_with_parent(&mut self, key: ViewKey) {
        let Some(parent_key) = self.get(key).and_then(|record| record.parent) else {
            return;
        };
        let Some(parent_epoch) = self.get(parent_key).map(|record| record.build_epoch) else {
            return;
        };
        let record = self.get_mut(key).unwrap();
        if record.registered_in_parent_epoch == parent_epoch {
            return;
        }
        record.registered_in_parent_epoch = parent_epoch;
        self.get_mut(parent_key).unwrap().children.push(key);
    }

    /// Adopts a (clean) record as visited in this frame with `parent`.
    pub fn adopt(&mut self, key: ViewKey, parent: Option<ViewKey>) {
        let epoch = self.frame_epoch;
        if let Some(record) = self.get_mut(key) {
            record.parent = parent;
            record.live_epoch = epoch;
        }
        self.register_with_parent(key);
    }

    /// Marks the record and every descendant as live in this frame and
    /// returns their keys (the record first).
    pub fn touch_subtree(&mut self, key: ViewKey, visit: &mut dyn FnMut(&mut ViewRecord)) {
        let epoch = self.frame_epoch;
        let mut stack: SmallVec<[ViewKey; 16]> = SmallVec::new();
        stack.push(key);
        while let Some(key) = stack.pop() {
            let Some(record) = self.get_mut(key) else {
                continue;
            };
            record.live_epoch = epoch;
            visit(record);
            stack.extend(record.children.iter().copied());
        }
    }

    /// Removes every record that was neither built nor replayed in this
    /// frame.
    pub fn sweep(&mut self) {
        let epoch = self.frame_epoch;
        let mut dead = Vec::new();
        for (index, slot) in self.slots.iter().enumerate() {
            if let Slot::Occupied { generation, record } = slot
                && record.live_epoch != epoch
            {
                dead.push(ViewKey {
                    index: index as u32,
                    generation: *generation,
                });
            }
        }
        for key in dead {
            self.remove(key);
        }
    }

    pub fn remove(&mut self, key: ViewKey) {
        let Some(Slot::Occupied { generation, .. }) = self.slots.get_mut(key.index as usize) else {
            return;
        };
        if *generation != key.generation {
            return;
        }
        let generation = generation.wrapping_add(1);
        let slot = std::mem::replace(
            &mut self.slots[key.index as usize],
            Slot::Free { generation },
        );
        let Slot::Occupied { record, .. } = slot else {
            return;
        };
        self.unindex_reads(key, &record.reads);
        if self.by_path.get(&record.path) == Some(&key) {
            self.by_path.remove(&record.path);
        }
        remove_key(&mut self.by_entity, record.entity_id, key);
        self.free.push(key.index);
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn clear(&mut self) {
        *self = Self {
            frame_epoch: self.frame_epoch,
            stats: self.stats,
            ..Self::default()
        };
    }
}

static GLOBALLY_DISABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Process-wide diagnostic switch: when set, every frame of every window
/// follows the nonretained path (everything is constructed).
pub fn set_retained_frames_disabled(disabled: bool) {
    GLOBALLY_DISABLED.store(disabled, std::sync::atomic::Ordering::Relaxed);
}

/// Whether the process-wide switch disables retained frames. Initialized from
/// the `VESPERTINE_DISABLE_RETENTION` environment variable.
pub fn retained_frames_disabled() -> bool {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        if std::env::var_os("VESPERTINE_DISABLE_RETENTION")
            .is_some_and(|value| !value.is_empty() && value != "0")
        {
            GLOBALLY_DISABLED.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    });
    GLOBALLY_DISABLED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Window-level retention state.
pub(crate) struct RetentionState {
    pub tracker: std::rc::Rc<ReadTracker>,
    pub views: RetainedViews,
    /// Records of the views currently being built, innermost last.
    pub stack: Vec<ViewKey>,
    /// Pointer, modifiers and caps lock as of the last frame.
    pub last_inputs: Option<(crate::Point<Pixels>, crate::Modifiers, crate::Capslock)>,
    /// Number of views on the stack that were notified since the preceding
    /// frame (their ordinary descendants re-evaluate unnotified updates).
    pub notified_ancestors: u32,
    /// Greater than zero while deferred content is built: such views are not
    /// retained independently.
    pub in_deferred: u32,
    /// Children built for a splice that was abandoned, waiting to be adopted
    /// by the fallback reconstruction of their parent.
    pub adopted: FxHashMap<ViewKey, (LayoutId, crate::AnyElement)>,
    /// Element states accessed by the layout phase of the views being built,
    /// innermost view last.
    pub layout_state_sink: Vec<Vec<(GlobalElementId, TypeId)>>,
    /// Everything must be reconstructed in the frame being drawn.
    pub blocked: bool,
    /// The retention context in force while a view is built or replayed (see
    /// [`Window::with_retention_context`]).
    pub context: u64,
    /// Per-window control (tests and benchmarks).
    pub window_disabled: bool,
    /// Whether accessibility was active when the previous frame started.
    pub a11y_was_active: bool,
}

impl RetentionState {
    pub fn new(tracker: std::rc::Rc<ReadTracker>) -> Self {
        Self {
            tracker,
            views: RetainedViews::default(),
            stack: Vec::new(),
            last_inputs: None,
            notified_ancestors: 0,
            in_deferred: 0,
            adopted: FxHashMap::default(),
            layout_state_sink: Vec::new(),
            blocked: false,
            context: 0,
            window_disabled: false,
            a11y_was_active: false,
        }
    }
}

impl PrepaintStateIndex {
    /// Whether every component of `self` lies within `range`.
    fn within(&self, range: &Range<PrepaintStateIndex>) -> bool {
        self.hitboxes_index >= range.start.hitboxes_index
            && self.hitboxes_index <= range.end.hitboxes_index
            && self.tooltips_index >= range.start.tooltips_index
            && self.tooltips_index <= range.end.tooltips_index
            && self.deferred_draws_index >= range.start.deferred_draws_index
            && self.deferred_draws_index <= range.end.deferred_draws_index
            && self.dispatch_tree_index >= range.start.dispatch_tree_index
            && self.dispatch_tree_index <= range.end.dispatch_tree_index
            && self.accessed_element_states_index >= range.start.accessed_element_states_index
            && self.accessed_element_states_index <= range.end.accessed_element_states_index
            && self.hitbox_switches_index >= range.start.hitbox_switches_index
            && self.hitbox_switches_index <= range.end.hitbox_switches_index
            && self.effects_index >= range.start.effects_index
            && self.effects_index <= range.end.effects_index
            && self.a11y_index >= range.start.a11y_index
            && self.a11y_index <= range.end.a11y_index
            && self
                .line_layout_index
                .within(&range.start.line_layout_index, &range.end.line_layout_index)
    }
}

impl PaintIndex {
    /// Whether every component of `self` lies within `range`.
    fn within(&self, range: &Range<PaintIndex>) -> bool {
        #[cfg(any(test, feature = "test-support"))]
        let debug_bounds = self.debug_bounds_index >= range.start.debug_bounds_index
            && self.debug_bounds_index <= range.end.debug_bounds_index;
        #[cfg(not(any(test, feature = "test-support")))]
        let debug_bounds = true;
        debug_bounds
            && self.scene_index >= range.start.scene_index
            && self.scene_index <= range.end.scene_index
            && self.surface_switches_index >= range.start.surface_switches_index
            && self.surface_switches_index <= range.end.surface_switches_index
            && self.mouse_listeners_index >= range.start.mouse_listeners_index
            && self.mouse_listeners_index <= range.end.mouse_listeners_index
            && self.input_handlers_index >= range.start.input_handlers_index
            && self.input_handlers_index <= range.end.input_handlers_index
            && self.cursor_styles_index >= range.start.cursor_styles_index
            && self.cursor_styles_index <= range.end.cursor_styles_index
            && self.accessed_element_states_index >= range.start.accessed_element_states_index
            && self.accessed_element_states_index <= range.end.accessed_element_states_index
            && self.tab_handle_index >= range.start.tab_handle_index
            && self.tab_handle_index <= range.end.tab_handle_index
            && self.effects_index >= range.start.effects_index
            && self.effects_index <= range.end.effects_index
            && self.a11y_listeners_index >= range.start.a11y_listeners_index
            && self.a11y_listeners_index <= range.end.a11y_listeners_index
            && self
                .line_layout_index
                .within(&range.start.line_layout_index, &range.end.line_layout_index)
    }
}

impl Window {
    /// Whether views are recorded in this frame at all.
    pub(crate) fn retention_tracking(&self) -> bool {
        !retained_frames_disabled()
            && !self.retention.window_disabled
            && self.retention.in_deferred == 0
    }

    /// Whether a recorded view may be replayed in this frame.
    pub(crate) fn retention_reuse_allowed(&self) -> bool {
        self.retention_tracking() && !self.retention.blocked
    }

    /// The ambient inputs a replay of the view being drawn depends on.
    pub(crate) fn ambient_cache_key(&self, bounds: Bounds<Pixels>) -> CacheKey {
        CacheKey {
            bounds,
            content_mask: self.content_mask(),
            text_style: self.text_style(),
            opacity: self.element_opacity,
            rem_size: self.rem_size(),
            context: self.retention.context,
            inert: self.inert_depth > 0,
        }
    }

    /// Runs `f` with `token` mixed into the retention context: a view built inside `f` is replayed
    /// only while the context it is reached in is unchanged, so an element whose output depends
    /// on something its descendants inherit (a scoped setting, a declaration it owns) passes a
    /// token that changes with that something. A token that is unique per construction of the
    /// element makes every view nested in it rebuild whenever the element itself is rebuilt.
    pub fn with_retention_context<R>(&mut self, token: u64, f: impl FnOnce(&mut Self) -> R) -> R {
        let outer = self.retention.context;
        self.retention.context =
            (outer.rotate_left(13) ^ token).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let result = f(self);
        self.retention.context = outer;
        result
    }

    /// Declares that the output of the view being built depends on every view nested in it being
    /// built together with it (for example because an element of the view computes state over
    /// its whole subtree): when any nested view is dirty the view is built again instead of being
    /// replayed around the rebuilt one (PRF-001 R6). A no-op outside a view.
    pub fn require_whole_view_rebuild(&mut self) {
        if let Some(key) = self.retention.stack.last().copied()
            && let Some(record) = self.retention.views.get_mut(key)
        {
            record.splice_unsafe = true;
        }
    }

    /// Enables or disables retained frames for this window. With retention
    /// disabled every frame constructs every view (test and benchmark
    /// control).
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_retention_enabled(&mut self, enabled: bool) {
        if self.retention.window_disabled == enabled {
            self.retention.window_disabled = !enabled;
            if !enabled {
                self.retention.views.clear();
            }
        }
    }

    /// Whether retained frames are enabled for this window.
    #[cfg(any(test, feature = "test-support"))]
    pub fn retention_enabled(&self) -> bool {
        !self.retention.window_disabled
    }

    /// Counters of retained-frame activity since the last reset.
    #[cfg(any(test, feature = "test-support"))]
    pub fn retention_stats(&self) -> RetentionStats {
        self.retention.views.stats
    }

    /// Resets the retained-frame counters.
    #[cfg(any(test, feature = "test-support"))]
    pub fn reset_retention_stats(&mut self) {
        self.retention.views.stats = RetentionStats::default();
    }

    /// Captures the ambient state of the view being built.
    pub(crate) fn capture_ambient(&self) -> Ambient {
        Ambient {
            element_id_stack: self.element_id_stack.clone(),
            text_style_stack: self.text_style_stack.clone(),
            rem_override: self.rem_size_override_stack.clone(),
            rendered_entities: self.rendered_entity_stack.clone(),
            image_caches: self.image_cache_stack.clone(),
            retention_context: self.retention.context,
            prepaint: None,
        }
    }

    /// The prepaint-phase part of the ambient state, captured while the view
    /// is prepainted (its wrapper dispatch node is open).
    pub(crate) fn capture_prepaint_ambient(&self) -> PrepaintAmbient {
        let dispatch = &self.next_frame.dispatch_tree;
        PrepaintAmbient {
            content_mask: self.content_mask(),
            opacity: self.element_opacity,
            element_offset: self.element_offset(),
            dispatch_parent: dispatch
                .active_node_id()
                .and_then(|node| dispatch.parent_of(node))
                .map(DispatchNodeId::index),
        }
    }

    /// Runs `f` with the window state of `ambient` installed.
    pub(crate) fn with_ambient<R>(
        &mut self,
        ambient: &Ambient,
        phase: AmbientPhase,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let ids = std::mem::replace(&mut self.element_id_stack, ambient.element_id_stack.clone());
        let text = std::mem::replace(&mut self.text_style_stack, ambient.text_style_stack.clone());
        let rem = std::mem::replace(
            &mut self.rem_size_override_stack,
            ambient.rem_override.clone(),
        );
        let mut rendered = ambient.rendered_entities.clone();
        if phase == AmbientPhase::Prepaint {
            // The prepaint and paint entry points push the view themselves.
            rendered.pop();
        }
        let rendered = std::mem::replace(&mut self.rendered_entity_stack, rendered);
        let images = std::mem::replace(&mut self.image_cache_stack, ambient.image_caches.clone());
        let context = std::mem::replace(&mut self.retention.context, ambient.retention_context);
        let prepaint_state = (phase == AmbientPhase::Prepaint)
            .then(|| ambient.prepaint.as_ref())
            .flatten()
            .map(|prepaint| {
                (
                    std::mem::replace(&mut self.content_mask_stack, vec![prepaint.content_mask]),
                    std::mem::replace(
                        &mut self.element_offset_stack,
                        vec![prepaint.element_offset],
                    ),
                    std::mem::replace(&mut self.element_opacity, prepaint.opacity),
                )
            });
        let result = f(self);
        self.element_id_stack = ids;
        self.text_style_stack = text;
        self.rem_size_override_stack = rem;
        self.rendered_entity_stack = rendered;
        self.image_cache_stack = images;
        self.retention.context = context;
        if let Some((mask, offset, opacity)) = prepaint_state {
            self.content_mask_stack = mask;
            self.element_offset_stack = offset;
            self.element_opacity = opacity;
        }
        result
    }

    /// After the prepaint output of `key` was copied from `old` in stretches
    /// around rebuilt children, moves the records of the views nested in the
    /// copied stretches along with it.
    pub(crate) fn rebase_spliced_prepaint(
        &mut self,
        key: ViewKey,
        new: Range<PrepaintStateIndex>,
        stretches: &[(Range<PrepaintStateIndex>, PrepaintStateIndex)],
        rebuilt: &[ViewKey],
    ) {
        let epoch = self.retention.views.frame_epoch;
        let map_point = |point: &PrepaintStateIndex, prefer_last: bool| {
            let mut found = None;
            for (old, new_start) in stretches {
                if point.within(old) {
                    found = Some(point.rebased(&old.start, new_start));
                    if !prefer_last {
                        break;
                    }
                }
            }
            found
        };
        let mut stack: SmallVec<[ViewKey; 16]> = SmallVec::new();
        if let Some(record) = self.retention.views.get_mut(key) {
            record.prepaint = Some(new);
            record.live_epoch = epoch;
            stack.extend(record.children.iter().copied());
        }
        while let Some(child) = stack.pop() {
            if rebuilt.contains(&child) {
                continue;
            }
            let Some(record) = self.retention.views.get_mut(child) else {
                continue;
            };
            record.live_epoch = epoch;
            let range = record.prepaint.take();
            match range.and_then(|range| {
                Some(map_point(&range.start, false)?..map_point(&range.end, true)?)
            }) {
                Some(range) => record.prepaint = Some(range),
                None => record.paint = None,
            }
            stack.extend(record.children.iter().copied());
        }
    }

    /// The paint-phase counterpart of [`Self::rebase_spliced_prepaint`].
    pub(crate) fn rebase_spliced_paint(
        &mut self,
        key: ViewKey,
        new: Range<PaintIndex>,
        stretches: &[(Range<PaintIndex>, PaintIndex)],
        rebuilt: &[ViewKey],
    ) {
        let map_point = |point: &PaintIndex, prefer_last: bool| {
            let mut found = None;
            for (old, new_start) in stretches {
                if point.within(old) {
                    found = Some(point.rebased(&old.start, new_start));
                    if !prefer_last {
                        break;
                    }
                }
            }
            found
        };
        let mut stack: SmallVec<[ViewKey; 16]> = SmallVec::new();
        if let Some(record) = self.retention.views.get_mut(key) {
            record.paint = Some(new);
            stack.extend(record.children.iter().copied());
        }
        while let Some(child) = stack.pop() {
            if rebuilt.contains(&child) {
                continue;
            }
            let Some(record) = self.retention.views.get_mut(child) else {
                continue;
            };
            let range = record.paint.take();
            match range.and_then(|range| {
                Some(map_point(&range.start, false)?..map_point(&range.end, true)?)
            }) {
                Some(range) => record.paint = Some(range),
                None => record.prepaint = None,
            }
            stack.extend(record.children.iter().copied());
        }
    }

    /// The expanded primitives painted by the last frame, in paint order, as
    /// comparable text. Atlas texture and tile identities are omitted (they
    /// depend on packing history, not on what was drawn); order, clipping,
    /// geometry, color and text layout are included.
    #[cfg(any(test, feature = "test-support"))]
    pub fn painted_primitives(&self) -> Vec<String> {
        use crate::scene::{PaintOperation, Primitive};
        fn strip_atlas(mut text: String) -> String {
            while let Some(start) = text.find("tile: AtlasTile {") {
                let mut depth = 0usize;
                let mut end = start;
                for (offset, ch) in text[start..].char_indices() {
                    match ch {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                end = start + offset + 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                let mut cut_end = end;
                if text[cut_end..].starts_with(',') {
                    cut_end += 1;
                }
                text.replace_range(start..cut_end, "tile: _,");
            }
            text
        }
        self.rendered_frame
            .scene
            .paint_operations
            .iter()
            .map(|operation| match operation {
                PaintOperation::StartLayer(bounds) => format!("StartLayer({bounds:?})"),
                PaintOperation::EndLayer => "EndLayer".to_string(),
                PaintOperation::BackdropBlur(blur) => format!("BackdropBlur({blur:?})"),
                PaintOperation::PushCurvedClip(shape) => format!("PushCurvedClip({shape:?})"),
                PaintOperation::PopCurvedClip => "PopCurvedClip".to_string(),
                PaintOperation::Primitive(primitive) => strip_atlas(match primitive {
                    Primitive::Shadow(p) => format!("{p:?}"),
                    Primitive::Quad(p) => format!("{p:?}"),
                    Primitive::Path(p) => format!("{p:?}"),
                    Primitive::Underline(p) => format!("{p:?}"),
                    Primitive::MonochromeSprite(p) => format!("{p:?}"),
                    Primitive::SubpixelSprite(p) => format!("{p:?}"),
                    Primitive::PolychromeSprite(p) => format!("{p:?}"),
                    Primitive::Surface(p) => format!("{p:?}"),
                }),
            })
            .collect()
    }

    /// The bounds of the hitboxes under `position` in the last frame,
    /// topmost first.
    #[cfg(any(test, feature = "test-support"))]
    pub fn hit_test_bounds(&self, position: crate::Point<Pixels>) -> Vec<Bounds<Pixels>> {
        let hit_test = self.rendered_frame.hit_test(position);
        hit_test
            .ids
            .iter()
            .filter_map(|id| {
                self.rendered_frame
                    .hitboxes
                    .iter()
                    .find(|hitbox| hitbox.id == *id)
                    .map(|hitbox| hitbox.bounds)
            })
            .collect()
    }

    /// Starts recording the reads of the view `key` (one build phase).
    pub(crate) fn begin_view_phase(&mut self, key: ViewKey) {
        let notified = self
            .retention
            .views
            .get(key)
            .is_some_and(|record| record.notified);
        self.retention.stack.push(key);
        self.retention.notified_ancestors += notified as u32;
        self.retention.tracker.push_frame(key, ReadSet::default());
    }

    /// Ends the build phase started by [`Self::begin_view_phase`].
    pub(crate) fn end_view_phase(&mut self, key: ViewKey) {
        let reads = self.retention.tracker.pop_frame();
        let popped = self.retention.stack.pop();
        debug_assert_eq!(popped, Some(key));
        let tracker = self.retention.tracker.clone();
        let notified = self
            .retention
            .views
            .get(key)
            .is_some_and(|record| record.notified);
        self.retention.notified_ancestors -= notified as u32;
        let clock = tracker.now();
        self.retention.views.append_reads(key, reads, clock);
        self.retention.views.flush_draw_writes(&tracker);
    }

    /// Marks the layout nodes of the view `key` and every view nested in it
    /// as live in this frame.
    pub(crate) fn touch_layout_subtree(&mut self, key: ViewKey) {
        let mut owned = Vec::new();
        let accessed = &mut self.next_frame.accessed_element_states;
        self.retention.views.touch_subtree(key, &mut |record| {
            owned.push(record.layout.clone());
            accessed.extend(record.layout_states.iter().cloned());
        });
        for nodes in owned {
            self.mark_layout_nodes_live(&nodes);
        }
    }

    /// After the prepaint output of `key` was copied from `old` (its range in
    /// the previous frame) to a range starting at `new_start` and ending at
    /// `new_end`, moves the records of the nested views along with it.
    pub(crate) fn rebase_prepaint_subtree(
        &mut self,
        key: ViewKey,
        old: Range<PrepaintStateIndex>,
        new: Range<PrepaintStateIndex>,
    ) {
        let epoch = self.retention.views.frame_epoch;
        let mut stack: SmallVec<[ViewKey; 16]> = SmallVec::new();
        if let Some(record) = self.retention.views.get_mut(key) {
            record.prepaint = Some(new.clone());
            record.live_epoch = epoch;
            stack.extend(record.children.iter().copied());
        }
        while let Some(child) = stack.pop() {
            let Some(record) = self.retention.views.get_mut(child) else {
                continue;
            };
            record.live_epoch = epoch;
            match record.prepaint.take() {
                Some(range) if range.start.within(&old) && range.end.within(&old) => {
                    record.prepaint = Some(
                        range.start.rebased(&old.start, &new.start)
                            ..range.end.rebased(&old.start, &new.start),
                    );
                }
                // Output outside the replayed range (content drawn later,
                // such as deferred draws) cannot be carried: the view is
                // built again when it is next reached.
                _ => record.paint = None,
            }
            stack.extend(record.children.iter().copied());
        }
    }

    /// The paint-phase counterpart of [`Self::rebase_prepaint_subtree`].
    pub(crate) fn rebase_paint_subtree(
        &mut self,
        key: ViewKey,
        old: Range<PaintIndex>,
        new: Range<PaintIndex>,
    ) {
        let mut stack: SmallVec<[ViewKey; 16]> = SmallVec::new();
        if let Some(record) = self.retention.views.get_mut(key) {
            record.paint = Some(new.clone());
            stack.extend(record.children.iter().copied());
        }
        while let Some(child) = stack.pop() {
            let Some(record) = self.retention.views.get_mut(child) else {
                continue;
            };
            match record.paint.take() {
                Some(range) if range.start.within(&old) && range.end.within(&old) => {
                    record.paint = Some(
                        range.start.rebased(&old.start, &new.start)
                            ..range.end.rebased(&old.start, &new.start),
                    );
                }
                _ => record.prepaint = None,
            }
            stack.extend(record.children.iter().copied());
        }
    }
}

/// Maps dispatch node ids of the previous frame to the ids of their copies
/// while one view's output is copied stretch by stretch around rebuilt
/// children.
#[derive(Default)]
pub(crate) struct SpliceMap {
    /// Copied stretches: the range of old node indices and the index its
    /// first node received in the new tree.
    stretches: Vec<(Range<usize>, usize)>,
}

impl SpliceMap {
    pub fn push(&mut self, old: Range<usize>, new_start: usize) {
        self.stretches.push((old, new_start));
    }

    pub fn resolve(&self, old: DispatchNodeId) -> Option<DispatchNodeId> {
        self.stretches
            .iter()
            .find(|(range, _)| range.contains(&old.index()))
            .map(|(range, new_start)| {
                DispatchNodeId::from_index(new_start + old.index() - range.start)
            })
    }
}
