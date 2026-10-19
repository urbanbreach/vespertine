//! Retained virtual-list rows, the window side (PRF-005).
//!
//! A virtual list keeps the rows of the viewport and of two viewports on each side as
//! records nested in the view that owns the list. A row record keeps what the row read, the
//! layout nodes it owns, the ranges of non-scene output it produced (hitboxes, listeners,
//! dispatch nodes, cursor styles, tab stops, element states) and its paint operations in a
//! scene of its own. A frame replays the rows that did not change, moved to the current
//! scroll offset, and builds only the rows that are missing, changed or newly hovered.

use super::{
    Window,
    read_tracker::ViewKey,
    retained::{CacheKey, Placement, RowData},
};
use crate::{App, Bounds, ContentMask, ElementId, GlobalElementId, Pixels, Point, Scene};
use std::collections::BTreeMap;

/// Frames a row that left the retained region keeps its layout nodes, so that scrolling back
/// adopts the old text measurement (PRF-005 R7).
pub(crate) const ROW_LINGER_FRAMES: u32 = 240;

/// Viewports of rows kept on each side of the visible ones.
pub(crate) const OVERSCAN_VIEWPORTS: f32 = 2.0;

/// Viewport heights of row content a composite frame may spend on filling overscan rows.
pub(crate) const EXTENSION_BUDGET_VIEWPORTS: f32 = 0.75;

/// The measurement sample of a uniform list (the row measured to learn the common height).
pub(crate) struct MeasureSample {
    /// Record that holds what the sample read.
    pub key: ViewKey,
    pub item_count: usize,
    pub width: Option<Pixels>,
    pub size: crate::Size<Pixels>,
}

/// Everything about a list that, when it changes, makes every retained row obsolete.
#[derive(Clone, PartialEq)]
pub(crate) struct RowGeometry {
    pub item_count: usize,
    pub item_height: Pixels,
    pub bounds: Bounds<Pixels>,
    pub available_width: Pixels,
    pub scale_factor: f32,
    /// What rows inherit (text style, opacity, rem size, retention context): a change makes
    /// every recorded row obsolete.
    pub ambient: CacheKey,
}

/// The rows of one list element.
#[derive(Default)]
pub(crate) struct RowStore {
    pub rows: BTreeMap<usize, ViewKey>,
    pub geometry: Option<RowGeometry>,
    /// Frame epoch in which the list was last drawn.
    pub seen_epoch: u64,
    /// The retained region was filled once (initial construction).
    pub constructed: bool,
    /// Scroll top of the previous frame and the direction the list moved last.
    pub last_scroll_top: f64,
    pub moving_down: bool,
    /// The type of the callback that rendered the rows.
    pub render_type: Option<std::any::TypeId>,
    pub measure: Option<MeasureSample>,
    /// The list's structural revision the rows were recorded at (variable-height list).
    pub revision: u64,
    /// The list's count of scroll mutations as of the previous frame (variable-height list).
    pub scroll_marks: u64,
}

impl Window {
    /// Whether virtual lists retain their rows in this frame: retention is on, accessibility
    /// is not active (overscan rows would leak nodes into the tree) and the list is not
    /// drawn inside deferred content.
    pub(crate) fn rows_enabled(&self) -> bool {
        self.retention_tracking() && !self.a11y.is_active()
    }

    /// What rows inherit from the place their list is drawn in (text style, opacity, rem
    /// size, retention context), without the geometry that moves rows rather than
    /// invalidating them.
    pub(crate) fn row_ambient(&self) -> CacheKey {
        let mut key = self.ambient_cache_key(Bounds::default());
        key.content_mask = ContentMask {
            bounds: Bounds::default(),
        };
        key
    }

    /// The store of the list drawn at `id`, created on first use and marked as seen.
    pub(crate) fn row_store(&mut self, id: &GlobalElementId) -> &mut RowStore {
        let epoch = self.retention.views.frame_epoch;
        let store = self.row_stores.entry(id.clone()).or_default();
        store.seen_epoch = epoch;
        store
    }

    /// Forgets every row of the list at `id`.
    pub(crate) fn row_store_clear(&mut self, id: &GlobalElementId) {
        let Some(store) = self.row_stores.get_mut(id) else {
            return;
        };
        let keys: Vec<ViewKey> = std::mem::take(&mut store.rows).into_values().collect();
        let sample = store.measure.take().map(|sample| sample.key);
        store.constructed = false;
        store.geometry = None;
        for key in keys.into_iter().chain(sample) {
            self.row_drop(key);
        }
    }

    /// Drops the record of a row; its layout nodes stay alive for a while.
    pub(crate) fn row_drop(&mut self, key: ViewKey) {
        if let Some(record) = self.retention.views.get(key) {
            let nodes = record.layout.clone();
            self.retain_layout_nodes(&nodes, ROW_LINGER_FRAMES);
        }
        self.retention.views.remove(key);
    }

    /// Forgets the rows whose records were swept and the stores of lists that are gone. A list
    /// drawn through a replayed view is not drawn in this frame but keeps its rows alive.
    pub(crate) fn sweep_row_stores(&mut self) {
        let epoch = self.retention.views.frame_epoch;
        let views = &self.retention.views;
        self.row_stores.retain(|_, store| {
            store.rows.retain(|_, key| views.get(*key).is_some());
            if store
                .measure
                .as_ref()
                .is_some_and(|sample| views.get(sample.key).is_none())
            {
                store.measure = None;
            }
            store.seen_epoch == epoch || !store.rows.is_empty() || store.measure.is_some()
        });
    }

    /// Whether the row's recorded output may be replayed in this frame, whether it is shown
    /// or kept without being shown.
    #[allow(
        dead_code,
        reason = "the recording of scroll layers shares the row helpers (PRF-004)"
    )]
    pub(crate) fn row_reusable(&self, key: ViewKey) -> bool {
        self.row_reusable_for(key, false)
    }

    /// Whether the row's recorded output may be replayed in this frame. A row that was kept
    /// without its element states (it was not shown) is built again before it is shown.
    pub(crate) fn row_reusable_for(&self, key: ViewKey, shown: bool) -> bool {
        let Some(record) = self.retention.views.get(key) else {
            return false;
        };
        let Some(row) = record.row.as_deref() else {
            return false;
        };
        // A dirty nested view cannot be spliced into a replayed row: the row is built again.
        if record.dirty
            || record.dirty_descendants
            || record.prepaint.is_none()
            || record.paint.is_none()
        {
            return false;
        }
        if row.suspect || !row.replayable || (shown && row.states_dropped) {
            return false;
        }
        // Ordinary descendants of a notified ancestor re-evaluate updates that were never
        // notified (PRF-001 R4).
        !(self.retention.notified_ancestors > 0
            && super::retained::RetainedViews::reads_stale(&self.retention.tracker, record))
    }

    /// Marks every row of the list suspect: its callback may have changed, so a row is built
    /// again as soon as it is shown.
    pub(crate) fn row_mark_suspect(&mut self, key: ViewKey) {
        if let Some(row) = self
            .retention
            .views
            .get_mut(key)
            .and_then(|record| record.row.as_deref_mut())
        {
            row.suspect = true;
        }
    }

    /// Marks the row as one that cannot be replayed (it asked for an autoscroll while it was
    /// prepainted, which only a build can repeat).
    pub(crate) fn row_forbid_replay(&mut self, key: ViewKey) {
        if let Some(row) = self
            .retention
            .views
            .get_mut(key)
            .and_then(|record| record.row.as_deref_mut())
        {
            row.replayable = false;
        }
    }

    /// Whether the owner of the list was rebuilt for a reason nothing tracked explains (PRF-005
    /// R5), for a list that knows `scrolls` scroll mutations (scrollbar offsets and
    /// programmatic scrolls) since the previous frame: each explains one notification of the
    /// owner.
    ///
    /// The owner is rebuilt for an unexplained reason when it was notified itself (rather than
    /// being rebuilt only because a row nested in it changed) while some notification is
    /// neither a scroll nor an animation frame, or when reads of its own went stale.
    pub(crate) fn rows_owner_ambiguous_with(&self, scrolls: u64) -> bool {
        if !self.retention_reuse_allowed() {
            return true;
        }
        let Some(owner) = self
            .retention
            .stack
            .last()
            .and_then(|key| self.retention.views.get(*key))
        else {
            return true;
        };
        owner.rebuilt_stale
            || (owner.rebuilt_dirty && self.retention.causes.unexplained_count > scrolls)
    }

    /// Runs `f`, which renders and measures a row that is not retained, so that what it reads
    /// belongs to no row and to no owner (PRF-005 R4), and counts the measurement.
    pub(crate) fn row_measure_scope<R>(&mut self, f: impl FnOnce(&mut Window) -> R) -> R {
        self.frame_stats_mut().rows_measured += 1;
        let Some(owner) = self.retention.stack.last().copied() else {
            return f(self);
        };
        let tracker = self.retention.tracker.clone();
        tracker.push_frame(owner, super::read_tracker::ReadSet::default());
        let result = f(self);
        tracker.pop_frame();
        result
    }

    /// Creates or returns the record of row `ix` of the list at `list`.
    pub(crate) fn row_record(&mut self, list: &GlobalElementId, ix: usize) -> ViewKey {
        let mut ids = list.to_vec();
        ids.push(ElementId::named_usize("retained-row", ix));
        let path = GlobalElementId::new(&ids);
        if let Some(key) = self.retention.views.lookup(&path) {
            return key;
        }
        let owner = self.current_view();
        let parent = self.retention.stack.last().copied();
        self.retention.views.create_row(path, owner, parent)
    }

    /// Adopts a row record as live in this frame under the view being built.
    pub(crate) fn row_adopt(&mut self, key: ViewKey) {
        let parent = self.retention.stack.last().copied();
        self.retention.views.adopt(key, parent);
    }

    /// Starts building the row: forgets its reads and children and records the reads, the
    /// layout nodes and the element states of its render and layout from now on.
    pub(crate) fn row_begin(&mut self, key: ViewKey) {
        let parent = self.retention.stack.last().copied();
        self.retention.views.begin_rebuild(key, parent);
        self.retention.views.reset_reads(key);
        let mark = self.next_frame.accessed_element_states.len();
        if let Some(record) = self.retention.views.get_mut(key) {
            record.row_mark = mark;
            // The output of the build before is obsolete until this build produces new.
            record.prepaint = None;
            record.paint = None;
        }
        self.begin_view_phase(key);
        self.begin_layout_owner();
        self.retention.layout_state_sink.push(Vec::new());
    }

    /// Ends the render and layout part of a row build.
    pub(crate) fn row_layout_done(&mut self, key: ViewKey) {
        let states = self.retention.layout_state_sink.pop().unwrap_or_default();
        let owned = self.end_layout_owner();
        if let Some(record) = self.retention.views.get_mut(key) {
            record.layout = owned;
            record.layout_states = states;
        }
    }

    /// Ends the render and layout part of a row build and its read phase, for rows that are
    /// prepainted later (see [`Self::row_prepaint_phase`]).
    pub(crate) fn row_layout_end(&mut self, key: ViewKey) {
        self.row_layout_done(key);
        self.end_view_phase(key);
    }

    /// Prepaints a freshly built row under the mask `extent` (which covers the whole row), clips
    /// the hitboxes it produced to `clip`, and records its prepaint range. Ends the build's
    /// read phase.
    #[allow(
        dead_code,
        reason = "the recording of scroll layers shares the row helpers (PRF-004)"
    )]
    pub(crate) fn row_prepaint<R>(
        &mut self,
        key: ViewKey,
        extent: ContentMask<Pixels>,
        clip: Bounds<Pixels>,
        anchor: Point<Pixels>,
        f: impl FnOnce(&mut Window) -> R,
    ) -> R {
        self.row_prepaint_ex(key, extent, clip, anchor, true, false, f)
    }

    /// Like [`Self::row_prepaint`] for a row whose layout phase ended with
    /// [`Self::row_layout_end`]: the read phase starts again here. A row that is not `shown`
    /// keeps none of the element states of its build.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn row_prepaint_phase<R>(
        &mut self,
        key: ViewKey,
        extent: ContentMask<Pixels>,
        clip: Bounds<Pixels>,
        anchor: Point<Pixels>,
        shown: bool,
        f: impl FnOnce(&mut Window) -> R,
    ) -> R {
        self.row_prepaint_ex(key, extent, clip, anchor, shown, true, f)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn row_prepaint_ex<R>(
        &mut self,
        key: ViewKey,
        extent: ContentMask<Pixels>,
        clip: Bounds<Pixels>,
        anchor: Point<Pixels>,
        shown: bool,
        begin_phase: bool,
        f: impl FnOnce(&mut Window) -> R,
    ) -> R {
        if begin_phase {
            self.begin_view_phase(key);
        }
        self.frame_stats_mut().rows_built += 1;
        let hover_before = self.hover_reads.get();
        let mut start = self.prepaint_index();
        let result = self.with_content_mask_override(extent, f);
        let mut end = self.prepaint_index();
        let hitboxes = self.next_frame.hitboxes[start.hitboxes_index..].to_vec();
        for hitbox in &mut self.next_frame.hitboxes[start.hitboxes_index..] {
            hitbox.content_mask.bounds = hitbox.content_mask.bounds.intersect(&clip);
        }
        let hover_sensitive = self.hover_reads.get() != hover_before;
        // Content that switches the surface it is drawn into (PLT-001) is drawn in place.
        let deferred = end.deferred_draws_index != start.deferred_draws_index
            || end.hitbox_switches_index != start.hitbox_switches_index;
        let mark = self
            .retention
            .views
            .get(key)
            .map_or(start.accessed_element_states_index, |record| {
                record.row_mark
            });
        let mut has_states = end.accessed_element_states_index
            > start.accessed_element_states_index
            || self
                .retention
                .views
                .get(key)
                .is_some_and(|record| !record.layout_states.is_empty());
        if !shown {
            // Element states live only while the row is shown.
            has_states |= self.next_frame.accessed_element_states.len() > mark;
            self.forget_accessed_states(mark);
            start.accessed_element_states_index = mark;
            end.accessed_element_states_index = mark;
        }
        self.end_view_phase(key);
        if let Some(record) = self.retention.views.get_mut(key) {
            record.prepaint = Some(start..end);
            record.row = Some(Box::new(RowData {
                scene: Scene::default(),
                anchor,
                replayable: !deferred,
                suspect: false,
                hover_sensitive,
                hitboxes,
                hovered: false,
                has_states,
                states_dropped: false,
                tab_ops: Vec::new(),
            }));
        }
        result
    }

    /// Forgets the element states the frame recorded as accessed from `mark` on, and their
    /// values.
    fn forget_accessed_states(&mut self, mark: usize) {
        if self.next_frame.accessed_element_states.len() <= mark {
            return;
        }
        let keys: Vec<_> = self
            .next_frame
            .accessed_element_states
            .drain(mark..)
            .collect();
        for key in keys {
            self.next_frame.element_states.remove(&key);
        }
    }

    /// Paints a freshly built row under `extent`, into a scene of its own, and records its
    /// paint range.
    #[allow(
        dead_code,
        reason = "the recording of scroll layers shares the row helpers (PRF-004)"
    )]
    pub(crate) fn row_paint<R>(
        &mut self,
        key: ViewKey,
        extent: ContentMask<Pixels>,
        f: impl FnOnce(&mut Window) -> R,
    ) -> R {
        self.row_paint_phase(key, extent, true, f)
    }

    /// Like [`Self::row_paint`]; a row that is not `shown` keeps none of the element states
    /// of its paint.
    pub(crate) fn row_paint_phase<R>(
        &mut self,
        key: ViewKey,
        extent: ContentMask<Pixels>,
        shown: bool,
        f: impl FnOnce(&mut Window) -> R,
    ) -> R {
        self.begin_view_phase(key);
        let hover_before = self.hover_reads.get();
        let scene_position = self.next_frame.scene.len();
        let main = std::mem::take(&mut self.next_frame.scene);
        let mut start = self.paint_index();
        let result = self.with_content_mask_override(extent, f);
        let mut end = self.paint_index();
        let scene = std::mem::replace(&mut self.next_frame.scene, main);
        // The row's scene operations are kept apart: as far as the frame's ranges are
        // concerned the row produced no scene output, at this position.
        let switched = end.surface_switches_index != start.surface_switches_index;
        start.scene_index = scene_position;
        end.scene_index = scene_position;
        let mut has_states =
            end.accessed_element_states_index > start.accessed_element_states_index;
        if !shown {
            self.forget_accessed_states(start.accessed_element_states_index);
            end.accessed_element_states_index = start.accessed_element_states_index;
        }
        // The tab stops the row registered enter the frame only while it is shown.
        let tab_ops: Vec<_> = self.next_frame.tab_stops.insertion_history
            [start.tab_handle_index..end.tab_handle_index]
            .to_vec();
        if !tab_ops.is_empty() {
            self.next_frame.tab_stops.truncate(start.tab_handle_index);
            end.tab_handle_index = start.tab_handle_index;
        }
        self.end_view_phase(key);
        let hover_read = self.hover_reads.get() != hover_before;
        let hovered = self.row_hit_by_pointer(key, None);
        if let Some(record) = self.retention.views.get_mut(key) {
            record.paint = Some(start..end);
            if let Some(row) = record.row.as_deref_mut() {
                has_states |= row.has_states;
                row.has_states = has_states;
                row.states_dropped = !shown && has_states;
                row.hover_sensitive |= hover_read;
                row.replayable &= !scene.has_active_scope() && !switched;
                row.scene = scene;
                row.hovered = hovered;
                row.tab_ops = tab_ops;
            }
        }
        result
    }

    /// Runs `f` with `mask` as the content mask, without intersecting it with the mask in
    /// effect: rows are recorded under a mask that covers the whole row and clipped to the
    /// visible region when they are shown.
    pub(crate) fn with_content_mask_override<R>(
        &mut self,
        mask: ContentMask<Pixels>,
        f: impl FnOnce(&mut Window) -> R,
    ) -> R {
        self.content_mask_stack.push(mask);
        let result = f(self);
        self.content_mask_stack.pop();
        result
    }

    /// Whether the pointer is over a hitbox of the row, either at its recorded position
    /// (`None`) or moved by `placement`.
    pub(crate) fn row_hit_by_pointer(&self, key: ViewKey, placement: Option<&Placement>) -> bool {
        // Hover is painted only while the pointer is present and not suppressed by a wheel
        // scroll, and the last input was not the keyboard or a touch.
        if self.pointer.absent
            || self.pointer.paint_hover_suppressed
            || self.last_input_was_keyboard()
            || self.last_input_was_touch()
        {
            return false;
        }
        let Some(record) = self.retention.views.get(key) else {
            return false;
        };
        let (Some(range), Some(row)) = (record.prepaint.as_ref(), record.row.as_deref()) else {
            return false;
        };
        let position = self.mouse_position;
        let contains = |hitbox: &crate::Hitbox| {
            hitbox
                .bounds
                .intersect(&hitbox.content_mask.bounds)
                .contains(&position)
        };
        match placement {
            Some(placement) => row
                .hitboxes
                .iter()
                .any(|hitbox| contains(&placement.place_hitbox(hitbox))),
            None => self.next_frame.hitboxes[range.start.hitboxes_index..range.end.hitboxes_index]
                .iter()
                .any(contains),
        }
    }

    /// Whether a replay of the row at `placement` would show different hover state than the
    /// one it was recorded with.
    pub(crate) fn row_hover_changed(&self, key: ViewKey, placement: &Placement) -> bool {
        let Some(row) = self
            .retention
            .views
            .get(key)
            .and_then(|record| record.row.as_deref())
        else {
            return true;
        };
        row.hover_sensitive
            && (self.captured_hitbox.is_some()
                || self.row_hit_by_pointer(key, Some(placement)) != row.hovered)
    }

    /// Replays the non-scene prepaint output of a recorded row at `placement`.
    pub(crate) fn row_replay_prepaint(
        &mut self,
        key: ViewKey,
        placement: &Placement,
        cx: &mut App,
    ) {
        let Some(old) = self
            .retention
            .views
            .get(key)
            .and_then(|record| record.prepaint.clone())
        else {
            return;
        };
        self.frame_stats_mut().rows_replayed += 1;
        let hovered = self.row_hit_by_pointer(key, Some(placement));
        self.row_adopt(key);
        let start = self.prepaint_index();
        // The hitboxes come from the row's own unclipped copies, not from the frame.
        if let Some(row) = self
            .retention
            .views
            .get(key)
            .and_then(|record| record.row.as_deref())
        {
            self.next_frame.hitboxes.extend(
                row.hitboxes
                    .iter()
                    .map(|hitbox| placement.place_hitbox(hitbox)),
            );
        }
        let mut copied = old.clone();
        copied.end.hitboxes_index = copied.start.hitboxes_index;
        self.reuse_prepaint_mapped(copied, None, Some(placement), cx);
        let end = self.prepaint_index();
        self.rebase_prepaint_subtree(key, old, start..end);
        if let Some(record) = self.retention.views.get(key) {
            let nodes = record.layout.clone();
            if placement.keep_states {
                let states = record.layout_states.clone();
                self.next_frame.accessed_element_states.extend(states);
            }
            self.mark_layout_nodes_live(&nodes);
        }
        if let Some(row) = self
            .retention
            .views
            .get_mut(key)
            .and_then(|record| record.row.as_deref_mut())
        {
            row.hovered = hovered;
            if !placement.keep_states && row.has_states {
                row.states_dropped = true;
            }
        }
    }

    /// Replays the non-scene paint output of a recorded row at `placement`.
    pub(crate) fn row_replay_paint(&mut self, key: ViewKey, placement: &Placement, cx: &mut App) {
        let Some(old) = self
            .retention
            .views
            .get(key)
            .and_then(|record| record.paint.clone())
        else {
            return;
        };
        let start = self.paint_index();
        self.reuse_paint_placed(old.clone(), Some(placement), false, cx);
        let end = self.paint_index();
        self.rebase_paint_subtree(key, old, start..end);
    }

    /// Shows the paint operations of a row in the frame: moved by `placement` and clipped to
    /// the visible region.
    pub(crate) fn row_show(&mut self, key: ViewKey, placement: &Placement) {
        let Some(record) = self.retention.views.get(key) else {
            return;
        };
        let Some(row) = record.row.as_deref() else {
            return;
        };
        let scale = placement.scale_factor;
        // Rows move by whole device pixels: both positions are on the grid, so rounding only
        // removes the rounding error of the logical-pixel difference.
        let offset = crate::point(
            crate::ScaledPixels((placement.offset.x.0 * scale).round()),
            crate::ScaledPixels((placement.offset.y.0 * scale).round()),
        );
        let clip = placement.clip.scale(scale);
        let len = row.scene.len();
        self.next_frame
            .scene
            .replay_moved(0..len, &row.scene, offset, Some(clip));
        self.next_frame.tab_stops.replay(&row.tab_ops);
    }

    /// The content origin a row was recorded at.
    pub(crate) fn row_anchor(&self, key: ViewKey) -> Point<Pixels> {
        self.retention
            .views
            .get(key)
            .and_then(|record| record.row.as_deref())
            .map_or_else(Point::default, |row| row.anchor)
    }

    /// Whether a measurement sample may still be used.
    pub(crate) fn row_sample_valid(&self, key: ViewKey) -> bool {
        self.retention.views.get(key).is_some_and(|record| {
            !record.dirty
                && !(self.retention.notified_ancestors > 0
                    && super::retained::RetainedViews::reads_stale(&self.retention.tracker, record))
        })
    }

    /// Keeps a record (and its layout nodes) alive in this frame without building it.
    pub(crate) fn row_touch(&mut self, key: ViewKey) {
        self.row_adopt(key);
        if let Some(record) = self.retention.views.get(key) {
            let nodes = record.layout.clone();
            self.mark_layout_nodes_live(&nodes);
        }
    }

    /// Ends the read phase of a measurement sample.
    pub(crate) fn row_finish_measure(&mut self, key: ViewKey) {
        self.frame_stats_mut().rows_measured += 1;
        self.end_view_phase(key);
    }
}

/// Where a row recorded at `anchor` goes when the list's content origin is `now`.
pub(crate) fn row_offset(anchor: Point<Pixels>, now: Point<Pixels>) -> Point<Pixels> {
    now - anchor
}
