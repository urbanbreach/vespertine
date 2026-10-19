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
    retained::{Placement, RowData},
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
}

impl Window {
    /// Whether virtual lists retain their rows in this frame: retention is on, accessibility
    /// is not active (overscan rows would leak nodes into the tree) and the list is not
    /// drawn inside deferred content.
    pub(crate) fn rows_enabled(&self) -> bool {
        self.retention_tracking() && !self.a11y.is_active()
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

    /// Whether the row's recorded output may be replayed in this frame.
    pub(crate) fn row_reusable(&self, key: ViewKey) -> bool {
        let Some(record) = self.retention.views.get(key) else {
            return false;
        };
        let Some(row) = record.row.as_deref() else {
            return false;
        };
        if record.dirty || record.prepaint.is_none() || record.paint.is_none() {
            return false;
        }
        if row.suspect || !row.replayable {
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

    /// Whether the owner of the list was rebuilt for a reason nothing tracked explains: a
    /// notification that is neither a scroll nor an animation frame, or a frame that
    /// reconstructs everything (PRF-005 R5).
    pub(crate) fn rows_owner_ambiguous(&self) -> bool {
        self.retention.causes.unexplained || !self.retention_reuse_allowed()
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

    /// Prepaints a freshly built row under the mask `extent` (which covers the whole row), clips
    /// the hitboxes it produced to `clip`, and records its prepaint range. Ends the build's
    /// read phase.
    pub(crate) fn row_prepaint<R>(
        &mut self,
        key: ViewKey,
        extent: ContentMask<Pixels>,
        clip: Bounds<Pixels>,
        anchor: Point<Pixels>,
        f: impl FnOnce(&mut Window) -> R,
    ) -> R {
        let hover_before = self.hover_reads.get();
        let start = self.prepaint_index();
        let result = self.with_content_mask_override(extent, f);
        let end = self.prepaint_index();
        let hitboxes = self.next_frame.hitboxes[start.hitboxes_index..].to_vec();
        for hitbox in &mut self.next_frame.hitboxes[start.hitboxes_index..] {
            hitbox.content_mask.bounds = hitbox.content_mask.bounds.intersect(&clip);
        }
        let hover_sensitive = self.hover_reads.get() != hover_before;
        // Content that switches the surface it is drawn into (PLT-001) is drawn in place.
        let deferred = end.deferred_draws_index != start.deferred_draws_index
            || end.hitbox_switches_index != start.hitbox_switches_index;
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
            }));
        }
        result
    }

    /// Paints a freshly built row under `extent`, into a scene of its own, and records its
    /// paint range.
    pub(crate) fn row_paint<R>(
        &mut self,
        key: ViewKey,
        extent: ContentMask<Pixels>,
        f: impl FnOnce(&mut Window) -> R,
    ) -> R {
        self.begin_view_phase(key);
        let hover_before = self.hover_reads.get();
        let scene_position = self.next_frame.scene.len();
        let main = std::mem::take(&mut self.next_frame.scene);
        let start = self.paint_index();
        let result = self.with_content_mask_override(extent, f);
        let end = self.paint_index();
        let scene = std::mem::replace(&mut self.next_frame.scene, main);
        // The row's scene operations are kept apart: as far as the frame's ranges are
        // concerned the row produced no scene output, at this position.
        let (mut start, mut end) = (start, end);
        start.scene_index = scene_position;
        end.scene_index = scene_position;
        self.end_view_phase(key);
        let hover_read = self.hover_reads.get() != hover_before;
        let hovered = self.row_hit_by_pointer(key, None);
        if let Some(record) = self.retention.views.get_mut(key) {
            record.paint = Some(start..end);
            if let Some(row) = record.row.as_deref_mut() {
                row.hover_sensitive |= hover_read;
                row.replayable &= !scene.has_active_scope()
                    && end.surface_switches_index == start.surface_switches_index;
                row.scene = scene;
                row.hovered = hovered;
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
        self.end_view_phase(key);
    }
}

/// Where a row recorded at `anchor` goes when the list's content origin is `now`.
pub(crate) fn row_offset(anchor: Point<Pixels>, now: Point<Pixels>) -> Point<Pixels> {
    now - anchor
}
