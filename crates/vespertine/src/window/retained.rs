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
use crate::{
    Bounds, ContentMask, EdgeFade, EntityId, GlobalElementId, HitboxId, LayoutId, Pixels,
    ScopeParams, TextStyle,
};
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
    /// A retained row of a virtual list (PRF-005): nested in the view that owns the list,
    /// built by the list rather than by a view element.
    Row,
    /// The content of a scroll block recorded as a layer (PRF-004): nested in the view that
    /// owns the block, built by the block rather than by a view element.
    Layer,
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
    /// The paint scopes (edge fade, rounded clip) the view inherits: the recorded primitives
    /// carry them, so a replay requires them unchanged.
    pub scope: ScopeParams,
    pub edge_fade: Option<EdgeFade>,
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
    /// The inherited scopes in force while the view was painted; `None` until it was painted.
    pub paint: Option<ScopeState>,
}

/// The scoped state the window carries into descendants: inertness and the paint scopes.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct ScopeState {
    pub inert_depth: u32,
    pub paint_scope: ScopeParams,
    pub edge_fade: Option<EdgeFade>,
    pub curved_clip_ancestors: u32,
}

#[derive(Clone)]
pub(crate) struct PrepaintAmbient {
    pub content_mask: ContentMask<Pixels>,
    pub opacity: f32,
    pub scope: ScopeState,
    pub element_offset: crate::Point<Pixels>,
    /// Index of the parent of the view's wrapper node in the dispatch tree of
    /// the frame the view was prepainted in.
    pub dispatch_parent: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AmbientPhase {
    Layout,
    Prepaint,
    Paint,
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
    /// The last rebuild of this view happened because the view itself was flagged dirty, and
    /// because reads of its own were stale (not merely because a nested view was dirty).
    pub rebuilt_dirty: bool,
    pub rebuilt_stale: bool,
    /// Length of the frame's accessed-element-state list when the row build began (rows only).
    pub row_mark: usize,
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
    /// Frame epoch in which the record was last built (not merely replayed).
    pub built_epoch: u64,
    /// What a row of a virtual list keeps besides the above (rows and layers only).
    pub row: Option<Box<RowData>>,
    /// A scroll layer whose block scrolled while its owner stayed clean: the layer is placed
    /// at the new offset without being rebuilt (layers only).
    pub moved: bool,
}

/// The content-space output of one retained list row apart from the ranges its record
/// keeps: the paint operations (recorded under the row's extent mask), where the row was
/// recorded and what decides whether it may be replayed.
pub(crate) struct RowData {
    /// Paint operations of the row, in window space as of `anchor`.
    pub scene: crate::Scene,
    /// The list's content origin (list origin plus snapped scroll offset) when recorded.
    pub anchor: crate::Point<Pixels>,
    /// The row's output can be moved: it holds no scope-dependent primitive, deferred draw
    /// or window-fixed state.
    pub replayable: bool,
    /// Rendered by a callback that may have changed (owner rebuilt ambiguously, row count or
    /// callback replaced): rebuilt as soon as the row is shown.
    pub suspect: bool,
    /// The row read hover state while it was built.
    pub hover_sensitive: bool,
    /// The row's hitboxes as built, with masks that cover the whole row: the frame holds copies
    /// clipped to the visible region, which cannot be moved back into view.
    pub hitboxes: Vec<crate::Hitbox>,
    /// Whether the pointer was over the row's hitboxes when it was last recorded or replayed.
    pub hovered: bool,
    /// The row accessed element states while it was built.
    pub has_states: bool,
    /// The row was kept in a frame without its element states (it was not shown): the states
    /// were dropped, so the row is built again before it is shown (PRF-005 R3).
    pub states_dropped: bool,
    /// The tab stops the row registered while it was painted; they enter the frame only
    /// while the row is shown.
    pub tab_ops: Vec<crate::tab_stop::TabStopOperation>,
    /// How far (device pixels) the accessibility nodes the record logged in the last frame
    /// are from where they were recorded: replaying them moves them by the difference to the
    /// placement the replay wants.
    pub a11y_shift: (f64, f64),
}

/// Where a replayed stretch of output lands (rows and scroll layers, PRF-004/PRF-005).
#[derive(Clone, Copy)]
pub(crate) struct Placement {
    /// Moves the recorded geometry (logical pixels).
    pub offset: crate::Point<Pixels>,
    /// The visible region; hitbox content masks are clipped to it after moving.
    pub clip: Bounds<Pixels>,
    /// Carry the element states the stretch accessed (rows that are shown).
    pub keep_states: bool,
    pub scale_factor: f32,
}

impl Placement {
    pub fn place_bounds(&self, bounds: Bounds<Pixels>) -> Bounds<Pixels> {
        Bounds {
            origin: bounds.origin + self.offset,
            size: bounds.size,
        }
    }

    pub fn place_hitbox(&self, hitbox: &crate::Hitbox) -> crate::Hitbox {
        let mut hitbox = hitbox.clone();
        hitbox.bounds = self.place_bounds(hitbox.bounds);
        hitbox.content_mask.bounds = self
            .place_bounds(hitbox.content_mask.bounds)
            .intersect(&self.clip);
        hitbox
    }

    /// The offset in device pixels, as accessibility bounds use it.
    pub fn device_translation(&self) -> (f64, f64) {
        (
            f64::from(self.offset.x.0 * self.scale_factor),
            f64::from(self.offset.y.0 * self.scale_factor),
        )
    }
}

#[derive(Default)]
pub(crate) struct RetainedViews {
    slots: slotmap::SlotMap<ViewKey, ViewRecord>,
    by_path: FxHashMap<GlobalElementId, ViewKey>,
    by_entity: FxHashMap<EntityId, SmallVec<[ViewKey; 1]>>,
    entity_readers: FxHashMap<EntityId, SmallVec<[ViewKey; 2]>>,
    global_readers: FxHashMap<TypeId, SmallVec<[ViewKey; 2]>>,
    presence_readers: FxHashMap<TypeId, SmallVec<[ViewKey; 2]>>,
    pointer_readers: FxHashSet<ViewKey>,
    modifier_readers: FxHashSet<ViewKey>,
    capslock_readers: FxHashSet<ViewKey>,
    hover_readers: FxHashMap<HitboxId, SmallVec<[ViewKey; 2]>>,
    pub frame_epoch: u64,
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
        self.slots.get(key)
    }

    pub fn get_mut(&mut self, key: ViewKey) -> Option<&mut ViewRecord> {
        self.slots.get_mut(key)
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
            moved: false,
            splice_unsafe: false,
            notified: false,
            rebuilt_dirty: false,
            rebuilt_stale: false,
            row_mark: 0,
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
            built_epoch: 0,
            row: None,
        };
        let key = self.slots.insert(record);
        self.by_path.insert(path, key);
        add_key(&mut self.by_entity, entity_id, key);
        key
    }

    /// Creates the record of a list row nested in `parent`. Rows are invalidated by what they
    /// read, never by a notification of the owner's entity itself.
    pub fn create_row(
        &mut self,
        path: GlobalElementId,
        owner: EntityId,
        parent: Option<ViewKey>,
    ) -> ViewKey {
        let key = self.create(path, owner, ViewKind::Row);
        remove_key(&mut self.by_entity, owner, key);
        if let Some(record) = self.get_mut(key) {
            record.parent = parent;
        }
        self.register_with_parent(key);
        key
    }

    /// Replaces the dependency set of a record and keeps the reverse indices
    /// in sync.
    /// Creates the record of the layer of a scroll block nested in `parent`. Like a row, a
    /// layer is invalidated by what it read, never by a notification of the owner's entity.
    pub fn create_layer(
        &mut self,
        path: GlobalElementId,
        owner: EntityId,
        parent: Option<ViewKey>,
    ) -> ViewKey {
        let key = self.create(path, owner, ViewKind::Layer);
        remove_key(&mut self.by_entity, owner, key);
        if let Some(record) = self.get_mut(key) {
            record.parent = parent;
        }
        self.register_with_parent(key);
        key
    }

    /// Marks the layer `key` as moved: its owner is replayed around it, and the layer is
    /// placed at the block's new offset (PRF-004 R2).
    pub fn flag_moved(&mut self, key: ViewKey) {
        let Some(record) = self.get_mut(key) else {
            return;
        };
        record.moved = true;
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
        for hitbox in &reads.hovers {
            remove_key(&mut self.hover_readers, *hitbox, key);
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
        for hitbox in &reads.hovers {
            add_key(&mut self.hover_readers, *hitbox, key);
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

    /// The views that read the hover state of any of `hitboxes` while they were built.
    pub fn hover_readers_of(&self, hitboxes: &[HitboxId]) -> SmallVec<[ViewKey; 4]> {
        let mut readers = SmallVec::new();
        for hitbox in hitboxes {
            for key in self.hover_readers.get(hitbox).into_iter().flatten() {
                if !readers.contains(key) {
                    readers.push(*key);
                }
            }
        }
        readers
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
        for record in self.slots.values_mut() {
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
            if record.kind == ViewKind::Layer {
                // A layer is placed at its block's new offset when nothing it read changed;
                // otherwise the view that owns it is built again.
                if record.dirty {
                    return None;
                }
                if record.moved {
                    frontier.push(key);
                }
                continue;
            }
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
            record.rebuilt_dirty = record.dirty;
            record.rebuilt_stale = false;
            record.dirty = false;
            record.dirty_descendants = false;
            record.moved = false;
            record.splice_unsafe = false;
            record.parent = parent;
            record.live_epoch = epoch;
            record.built_epoch = epoch;
        }
        self.register_with_parent(key);
    }

    /// Records that the rebuild just begun is also due to reads of the view that went stale
    /// (see [`Self::reads_stale`]).
    pub fn mark_rebuilt_stale(&mut self, key: ViewKey) {
        if let Some(record) = self.get_mut(key) {
            record.rebuilt_stale = true;
        }
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
        let dead: Vec<ViewKey> = self
            .slots
            .iter()
            .filter(|(_, record)| record.live_epoch != epoch)
            .map(|(key, _)| key)
            .collect();
        for key in dead {
            self.remove(key);
        }
    }

    pub fn remove(&mut self, key: ViewKey) {
        let Some(record) = self.slots.remove(key) else {
            return;
        };
        self.unindex_reads(key, &record.reads);
        if self.by_path.get(&record.path) == Some(&key) {
            self.by_path.remove(&record.path);
        }
        remove_key(&mut self.by_entity, record.entity_id, key);
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn clear(&mut self) {
        *self = Self {
            frame_epoch: self.frame_epoch,
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
    /// Records of views drawn inside deferred draws whose owner was replayed: they keep the
    /// ranges of the previous frame until the replay of the draw that holds their output
    /// moves them (PRF-001 R2). The prepaint and paint lists are separate, each with the range
    /// the record held when it was carried.
    pub carried_prepaint: Vec<(ViewKey, Range<PrepaintStateIndex>)>,
    pub carried_paint: Vec<(ViewKey, Range<PaintIndex>)>,
    /// Views rebuilt in this frame inside a deferred draw of a replayed owner: each is placed
    /// into the replay of the draw that holds its previous output.
    pub deferred_splice: Vec<crate::view::SplicedChild>,
    /// Children built for a splice that was abandoned, waiting to be adopted
    /// by the fallback reconstruction of their parent.
    pub adopted: FxHashMap<ViewKey, (LayoutId, crate::AnyElement)>,
    /// Element states accessed by the layout phase of the views being built,
    /// innermost view last.
    pub layout_state_sink: Vec<Vec<(GlobalElementId, TypeId)>>,
    /// Everything must be reconstructed in the frame being drawn.
    pub blocked: bool,
    /// Phases of retained list rows being built (render and layout, prepaint) right now. The
    /// scene operations of a row are recorded apart from the frame's, so a view nested in a
    /// row that is built again is built again with it instead of replaying paint output that
    /// lives in the row's previous scene.
    pub row_builds: u32,
    /// Rows that are painted without being shown (retained outside the viewport) are being
    /// painted: nothing painted there is observed, as nothing of it reaches the screen.
    pub unshown_row_paints: u32,
    /// The retention context in force while a view is built or replayed (see
    /// [`Window::with_retention_context`]).
    pub context: u64,
    /// Per-window control (tests and benchmarks).
    pub window_disabled: bool,
    /// Whether accessibility was active when the previous frame started.
    pub a11y_was_active: bool,
    /// What caused the notifications this frame is drawn for.
    pub causes: super::read_tracker::FrameCauses,
    /// The notification counters as of the previous frame.
    pub causes_baseline: [u64; 3],
}

impl RetentionState {
    pub fn clear_carried(&mut self) {
        self.carried_prepaint.clear();
        self.carried_paint.clear();
    }

    pub fn new(tracker: std::rc::Rc<ReadTracker>) -> Self {
        Self {
            tracker,
            views: RetainedViews::default(),
            stack: Vec::new(),
            last_inputs: None,
            notified_ancestors: 0,
            carried_prepaint: Vec::new(),
            carried_paint: Vec::new(),
            deferred_splice: Vec::new(),
            adopted: FxHashMap::default(),
            layout_state_sink: Vec::new(),
            blocked: false,
            row_builds: 0,
            unshown_row_paints: 0,
            context: 0,
            window_disabled: false,
            a11y_was_active: false,
            causes: Default::default(),
            causes_baseline: [0; 3],
        }
    }
}

impl PrepaintStateIndex {
    /// Whether every component of `self` lies within `range`.
    pub(crate) fn within(&self, range: &Range<PrepaintStateIndex>) -> bool {
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
            && self.layer_frames_index >= range.start.layer_frames_index
            && self.layer_frames_index <= range.end.layer_frames_index
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
        !retained_frames_disabled() && !self.retention.window_disabled
    }

    /// Whether a recorded view may be replayed in this frame.
    pub(crate) fn retention_reuse_allowed(&self) -> bool {
        self.retention_tracking() && !self.retention.blocked
    }

    /// Whether a recorded view may be replayed here: reuse is allowed and the view is not nested
    /// in a list row that is being built.
    pub(crate) fn view_reuse_allowed(&self) -> bool {
        self.retention_reuse_allowed() && self.retention.row_builds == 0
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
            scope: self.paint_scope,
            edge_fade: self.edge_fade,
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
            paint: None,
        }
    }

    /// The prepaint-phase part of the ambient state, captured while the view
    /// is prepainted (its wrapper dispatch node is open).
    pub(crate) fn capture_prepaint_ambient(&self) -> PrepaintAmbient {
        let dispatch = &self.next_frame.dispatch_tree;
        PrepaintAmbient {
            content_mask: self.content_mask(),
            opacity: self.element_opacity,
            scope: self.scope_state(),
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
        if phase != AmbientPhase::Layout {
            // The prepaint and paint entry points push the view themselves.
            rendered.pop();
        }
        let rendered = std::mem::replace(&mut self.rendered_entity_stack, rendered);
        let images = std::mem::replace(&mut self.image_cache_stack, ambient.image_caches.clone());
        let context = std::mem::replace(&mut self.retention.context, ambient.retention_context);
        let prepaint_state = (phase != AmbientPhase::Layout)
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
        // What the view inherited from its clean ancestors, which a splice does not run again:
        // inertness and the paint scopes, as of the phase being run.
        let scope = match phase {
            AmbientPhase::Layout => None,
            AmbientPhase::Prepaint => ambient.prepaint.as_ref().map(|prepaint| prepaint.scope),
            AmbientPhase::Paint => ambient
                .paint
                .or_else(|| ambient.prepaint.as_ref().map(|prepaint| prepaint.scope)),
        }
        .map(|scope| self.replace_scope_state(scope));
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
        if let Some(scope) = scope {
            self.replace_scope_state(scope);
        }
        result
    }

    /// The inertness and paint scopes in force.
    pub(crate) fn scope_state(&self) -> ScopeState {
        ScopeState {
            inert_depth: self.inert_depth,
            paint_scope: self.paint_scope,
            edge_fade: self.edge_fade,
            curved_clip_ancestors: self.curved_clip_ancestors,
        }
    }

    /// Installs `scope` and returns the state it replaced.
    fn replace_scope_state(&mut self, scope: ScopeState) -> ScopeState {
        let previous = self.scope_state();
        self.inert_depth = scope.inert_depth;
        self.paint_scope = scope.paint_scope;
        self.edge_fade = scope.edge_fade;
        self.curved_clip_ancestors = scope.curved_clip_ancestors;
        previous
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
            let moved = range.as_ref().and_then(|range| {
                Some(map_point(&range.start, false)?..map_point(&range.end, true)?)
            });
            match (moved, range) {
                (Some(moved), _) => record.prepaint = Some(moved),
                // Output of a deferred draw: moved with the draw's replay.
                (None, Some(range)) => {
                    record.prepaint = Some(range.clone());
                    self.retention.carried_prepaint.push((child, range));
                }
                (None, None) => record.paint = None,
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
            let moved = range.as_ref().and_then(|range| {
                Some(map_point(&range.start, false)?..map_point(&range.end, true)?)
            });
            match (moved, range) {
                (Some(moved), _) => record.paint = Some(moved),
                (None, Some(range)) => {
                    record.paint = Some(range.clone());
                    self.retention.carried_paint.push((child, range));
                }
                (None, None) => record.prepaint = None,
            }
            stack.extend(record.children.iter().copied());
        }
    }

    /// The expanded primitives painted by the last frame, in paint order, as
    /// comparable text. Atlas texture and tile identities are omitted (they
    /// depend on packing history, not on what was drawn) and replaced by what the
    /// sprite shows (glyph, SVG or image key) when the atlas can tell; order,
    /// clipping, geometry, color and text layout are included.
    #[cfg(any(test, feature = "test-support"))]
    pub fn painted_primitives(&self) -> Vec<String> {
        use crate::scene::{PaintOperation, Primitive};
        let mut meanings: std::collections::HashMap<(u32, u32), String> = Default::default();
        let mut meaning = |tile: &crate::AtlasTile| -> String {
            meanings
                .entry((tile.texture_id.index, tile.tile_id.0))
                .or_insert_with(|| match self.sprite_atlas.key_for_tile(tile) {
                    Some(key) => format!(" shows={key:?}"),
                    None => format!(
                        " shows={:?} {:?} padding {}",
                        tile.texture_id.kind, tile.bounds.size, tile.padding
                    ),
                })
                .clone()
        };
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
                    Primitive::MonochromeSprite(p) => format!("{p:?}{}", meaning(&p.tile)),
                    Primitive::SubpixelSprite(p) => format!("{p:?}{}", meaning(&p.tile)),
                    Primitive::PolychromeSprite(p) => format!("{p:?}{}", meaning(&p.tile)),
                    Primitive::Surface(p) => format!("{p:?}"),
                }),
            })
            .collect()
    }

    /// What the last frame paints, as comparable text with the clipped bounds of each
    /// primitive: scroll layers (PRF-004) are expanded into the primitives their tiles bake,
    /// draw orders are left out, and so is everything that depends on atlas packing. Two frames
    /// that draw the same pixels list the same primitives; only primitives that do not overlap
    /// may differ in position (use [`painted_content_agrees`]).
    #[cfg(any(test, feature = "test-support"))]
    pub fn painted_content(&self) -> Vec<(String, Bounds<crate::ScaledPixels>)> {
        use crate::scene::{PaintOperation, Primitive};
        let describe = |primitive: &Primitive| -> (String, Bounds<crate::ScaledPixels>) {
            let mut primitive = primitive.clone();
            crate::scroll_layer::strip_identity(&mut primitive);
            let bounds = primitive
                .painted_bounds()
                .intersect(&primitive.content_mask().bounds);
            let meaning = |tile: &crate::AtlasTile| match self.sprite_atlas.key_for_tile(tile) {
                Some(key) => format!(" shows={key:?}"),
                None => format!(
                    " shows={:?} {:?} padding {}",
                    tile.texture_id.kind, tile.bounds.size, tile.padding
                ),
            };
            let text = match &mut primitive {
                Primitive::Shadow(p) => format!("{p:?}"),
                Primitive::Quad(p) => format!("{p:?}"),
                Primitive::Path(p) => format!("{p:?}"),
                Primitive::Underline(p) => format!("{p:?}"),
                Primitive::MonochromeSprite(p) => {
                    let shows = meaning(&p.tile);
                    p.tile = crate::scroll_layer::blank_tile();
                    format!("{p:?}{shows}")
                }
                Primitive::SubpixelSprite(p) => {
                    let shows = meaning(&p.tile);
                    p.tile = crate::scroll_layer::blank_tile();
                    format!("{p:?}{shows}")
                }
                Primitive::PolychromeSprite(p) => {
                    let shows = meaning(&p.tile);
                    p.tile = crate::scroll_layer::blank_tile();
                    format!("{p:?}{shows}")
                }
                Primitive::Surface(p) => format!("{p:?}"),
            };
            (text, bounds)
        };
        let frames = self.rendered_frame.scene.layers.frames();
        let mut expanded: Vec<crate::LayerId> = Vec::new();
        let mut content = Vec::new();
        for operation in &self.rendered_frame.scene.paint_operations {
            let PaintOperation::Primitive(primitive) = operation else {
                continue;
            };
            if let Primitive::PolychromeSprite(sprite) = primitive
                && let Some((layer, coord)) = crate::tile_identity(&sprite.tile)
            {
                if expanded.contains(&layer) {
                    continue;
                }
                expanded.push(layer);
                let Some(frame) = frames.iter().find(|frame| frame.id() == layer) else {
                    continue;
                };
                // The window position of content space follows from where this tile was drawn.
                let tile = coord.bounds();
                let offset = crate::point(
                    crate::ScaledPixels(sprite.bounds.origin.x.0 - tile.origin.x.0 as f32),
                    crate::ScaledPixels(sprite.bounds.origin.y.0 - tile.origin.y.0 as f32),
                );
                for baked in
                    frame
                        .content()
                        .baked()
                        .paint_operations
                        .iter()
                        .filter_map(|operation| match operation {
                            PaintOperation::Primitive(primitive) => Some(primitive),
                            _ => None,
                        })
                {
                    let moved = baked.translated(offset, Some(sprite.content_mask.bounds));
                    if moved
                        .painted_bounds()
                        .intersect(&moved.content_mask().bounds)
                        .is_empty()
                    {
                        continue;
                    }
                    content.push(describe(&moved));
                }
                continue;
            }
            content.push(describe(primitive));
        }
        content
    }

    /// The atlas identity (texture index, tile id) of every sprite painted by the last frame, in
    /// paint order. [`Self::painted_primitives`] leaves these out because they depend on packing
    /// history; this makes that dependence observable (DEV-003).
    #[cfg(any(test, feature = "test-support"))]
    pub fn painted_tile_ids(&self) -> Vec<(u32, u32)> {
        use crate::scene::{PaintOperation, Primitive};
        self.rendered_frame
            .scene
            .paint_operations
            .iter()
            .filter_map(|operation| match operation {
                PaintOperation::Primitive(Primitive::MonochromeSprite(sprite)) => Some(sprite.tile),
                PaintOperation::Primitive(Primitive::SubpixelSprite(sprite)) => Some(sprite.tile),
                PaintOperation::Primitive(Primitive::PolychromeSprite(sprite)) => Some(sprite.tile),
                _ => None,
            })
            .map(|tile| (tile.texture_id.index, tile.tile_id.0))
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
                // Output outside the replayed range is content drawn later by a deferred
                // draw: it keeps its range until the replay of that draw moves it, and is
                // built again when it is next reached if nothing does.
                Some(range) => {
                    record.prepaint = Some(range.clone());
                    self.retention.carried_prepaint.push((child, range));
                }
                None => record.paint = None,
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
                Some(range) => {
                    record.paint = Some(range.clone());
                    self.retention.carried_paint.push((child, range));
                }
                None => record.prepaint = None,
            }
            stack.extend(record.children.iter().copied());
        }
    }
}

impl Window {
    /// After the output of a deferred draw was copied from `container` (its range in the
    /// previous frame) in `stretches` (copied ranges and where each begins now), moves the
    /// records of the views drawn inside it. A record whose output lies in a gap between the
    /// stretches belongs to a view built again in this frame, which assigns it its new range.
    pub(crate) fn rebase_carried_prepaint(
        &mut self,
        container: &Range<PrepaintStateIndex>,
        stretches: &[(Range<PrepaintStateIndex>, PrepaintStateIndex)],
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
        let carried = std::mem::take(&mut self.retention.carried_prepaint);
        let mut remaining = Vec::with_capacity(carried.len());
        for (key, range) in carried {
            if !(range.start.within(container) && range.end.within(container)) {
                remaining.push((key, range));
                continue;
            }
            let Some(record) = self.retention.views.get_mut(key) else {
                continue;
            };
            if record.built_epoch == epoch || record.prepaint.as_ref() != Some(&range) {
                continue;
            }
            if let Some(moved) = map_point(&range.start, false)
                .zip(map_point(&range.end, true))
                .map(|(start, end)| start..end)
            {
                record.prepaint = Some(moved);
                record.live_epoch = epoch;
            }
        }
        self.retention.carried_prepaint = remaining;
    }

    /// The paint-phase counterpart of [`Self::rebase_carried_prepaint`].
    pub(crate) fn rebase_carried_paint(
        &mut self,
        container: &Range<PaintIndex>,
        stretches: &[(Range<PaintIndex>, PaintIndex)],
    ) {
        let epoch = self.retention.views.frame_epoch;
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
        let carried = std::mem::take(&mut self.retention.carried_paint);
        let mut remaining = Vec::with_capacity(carried.len());
        for (key, range) in carried {
            if !(range.start.within(container) && range.end.within(container)) {
                remaining.push((key, range));
                continue;
            }
            let Some(record) = self.retention.views.get_mut(key) else {
                continue;
            };
            if record.built_epoch == epoch || record.paint.as_ref() != Some(&range) {
                continue;
            }
            if let Some(moved) = map_point(&range.start, false)
                .zip(map_point(&range.end, true))
                .map(|(start, end)| start..end)
            {
                record.paint = Some(moved);
            }
        }
        self.retention.carried_paint = remaining;
    }

    /// Invalidates the records no deferred draw moved: the views are built again when they are
    /// next reached.
    pub(crate) fn finish_carried_prepaint(&mut self) {
        let epoch = self.retention.views.frame_epoch;
        for (key, range) in std::mem::take(&mut self.retention.carried_prepaint) {
            if let Some(record) = self.retention.views.get_mut(key)
                && record.built_epoch != epoch
                && record.prepaint.as_ref() == Some(&range)
            {
                record.paint = None;
            }
        }
    }

    /// The paint-phase counterpart of [`Self::finish_carried_prepaint`].
    pub(crate) fn finish_carried_paint(&mut self) {
        let epoch = self.retention.views.frame_epoch;
        for (key, range) in std::mem::take(&mut self.retention.carried_paint) {
            if let Some(record) = self.retention.views.get_mut(key)
                && record.built_epoch != epoch
                && record.paint.as_ref() == Some(&range)
            {
                record.prepaint = None;
            }
        }
    }

    /// The ranges of the deferred draws whose output the replay of `range` of the previous
    /// frame copies, nested draws included.
    pub(crate) fn deferred_ranges_within(
        &self,
        range: &Range<PrepaintStateIndex>,
    ) -> Vec<Range<PrepaintStateIndex>> {
        let mut found = Vec::new();
        let mut pending = vec![range.clone()];
        while let Some(range) = pending.pop() {
            let draws = self
                .rendered_frame
                .deferred_draws
                .get(range.start.deferred_draws_index..range.end.deferred_draws_index)
                .unwrap_or_default();
            for draw in draws {
                found.push(draw.prepaint_range.clone());
                pending.push(draw.prepaint_range.clone());
            }
        }
        found
    }
}

impl RetentionState {
    /// Takes the views rebuilt for a deferred draw whose previous output is `range`.
    pub(crate) fn take_deferred_children(
        &mut self,
        range: &Range<PrepaintStateIndex>,
    ) -> Vec<crate::view::SplicedChild> {
        let (taken, kept) = std::mem::take(&mut self.deferred_splice)
            .into_iter()
            .partition(|child| {
                self.views
                    .get(child.key)
                    .and_then(|record| record.prepaint.as_ref())
                    .is_some_and(|prepaint| {
                        prepaint.start.within(range) && prepaint.end.within(range)
                    })
            });
        self.deferred_splice = kept;
        taken
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
