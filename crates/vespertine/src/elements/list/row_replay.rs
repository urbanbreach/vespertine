//! Retained rows of the variable-height list (PRF-005).
//!
//! The list keeps the rows of the viewport and of two viewports on each side as records
//! nested in the view that owns the list (see `window/rows.rs`). A frame replays the rows that
//! did not change, moved to the current scroll offset, and builds only the rows that are
//! missing, changed or newly hovered. Rows are measured during layout as ever; rows that are
//! only measured keep no record and what they read belongs to nobody.

use super::{ItemLayout, ListItem, RenderItemFn, StateInner};
use crate::{
    AnyElement, App, AvailableSpace, Bounds, ContentMask, GlobalElementId, Pixels, Point, Size,
    Window, point, px, size,
    window::{
        read_tracker::ViewKey,
        retained::Placement,
        rows::{EXTENSION_BUDGET_VIEWPORTS, OVERSCAN_VIEWPORTS, RowGeometry, row_offset},
    },
};
use std::{
    any::TypeId,
    collections::{BTreeMap, BTreeSet},
    ops::Range,
};

/// What a frame does with one retained row.
pub(super) struct RowPlan {
    ix: usize,
    key: ViewKey,
    /// The element of a row built in this frame; `None` for a replayed row.
    element: Option<AnyElement>,
    placement: Placement,
    /// The mask the row was built under.
    extent: ContentMask<Pixels>,
    /// The row reaches the visible viewport.
    shown: bool,
}

/// What painting the rows needs from prepaint.
pub(super) struct RowPaint {
    plans: Vec<RowPlan>,
}

/// The retained rows of one list in one frame.
pub(super) struct RowSession {
    list: GlobalElementId,
    /// The rows retained so far, taken from the store of the list.
    rows: BTreeMap<usize, ViewKey>,
    plans: Vec<RowPlan>,
    /// Rows built or replayed in the current pass (undone when the pass is retried).
    touched: Vec<usize>,
    /// Rows whose build began and that reached prepaint.
    begun: BTreeSet<usize>,
    prepared: BTreeSet<usize>,
    constructed: bool,
    last_scroll_px: f64,
    moving_down: bool,
    base_extent: Bounds<Pixels>,
    clip: Bounds<Pixels>,
    scale: f32,
    /// Height of the visible rows built (rather than replayed) in this frame.
    visible_built: Pixels,
}

impl RowSession {
    /// Opens the rows of the list at `list` for a frame: forgets them when what they were
    /// recorded for changed, and marks them suspect when the owner was rebuilt for a reason
    /// nothing tracked explains or the row callback is not the one that rendered them.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn begin(
        remeasured: Vec<Range<usize>>,
        window: &mut Window,
        list: &GlobalElementId,
        bounds: Bounds<Pixels>,
        content_width: Pixels,
        count: usize,
        render_type: TypeId,
        revision: u64,
        marks: u64,
    ) -> Self {
        let scale = window.scale_factor();
        let geometry = RowGeometry {
            item_count: count,
            item_height: Pixels::ZERO,
            bounds,
            available_width: content_width,
            scale_factor: scale,
            ambient: window.row_ambient(),
        };
        let (stored_geometry, stored_revision, stored_marks, stored_type) = {
            let store = window.row_store(list);
            (
                store.geometry.clone(),
                store.revision,
                store.scroll_marks,
                store.render_type,
            )
        };
        if !window.retention_reuse_allowed()
            || stored_geometry.as_ref() != Some(&geometry)
            || stored_revision != revision
        {
            window.row_store_clear(list);
        }
        let ambiguous = window.rows_owner_ambiguous_with(marks.saturating_sub(stored_marks))
            || stored_type != Some(render_type);
        let (mut rows, constructed, last_scroll_px, moving_down) = {
            let store = window.row_store(list);
            store.geometry = Some(geometry);
            store.render_type = Some(render_type);
            store.revision = revision;
            store.scroll_marks = marks;
            (
                std::mem::take(&mut store.rows),
                store.constructed,
                store.last_scroll_top,
                store.moving_down,
            )
        };
        for range in remeasured {
            let stale: Vec<usize> = rows.range(range).map(|(ix, _)| *ix).collect();
            for ix in stale {
                if let Some(key) = rows.remove(&ix) {
                    window.row_drop(key);
                }
            }
        }
        if ambiguous {
            for key in rows.values() {
                window.row_mark_suspect(*key);
            }
        }
        Self {
            list: list.clone(),
            rows,
            plans: Vec::new(),
            touched: Vec::new(),
            begun: BTreeSet::new(),
            prepared: BTreeSet::new(),
            constructed,
            last_scroll_px,
            moving_down,
            base_extent: Bounds {
                origin: bounds.origin - point(bounds.size.width, bounds.size.height * 4.0),
                size: size(bounds.size.width * 3.0, bounds.size.height * 9.0),
            },
            clip: window.content_mask().bounds.intersect(&bounds),
            scale,
            visible_built: Pixels::ZERO,
        }
    }

    /// The row `ix` when its recorded output may be shown in this frame.
    pub(super) fn reusable(&self, window: &Window, ix: usize) -> Option<ViewKey> {
        let key = self.rows.get(&ix).copied()?;
        window.row_reusable_for(key, true).then_some(key)
    }

    /// Starts building row `ix`, creating its record when it has none.
    pub(super) fn begin_build(&mut self, window: &mut Window, ix: usize) -> ViewKey {
        let key = match self.rows.get(&ix).copied() {
            Some(key) => key,
            None => {
                let key = window.row_record(&self.list, ix);
                self.rows.insert(ix, key);
                key
            }
        };
        self.touched.push(ix);
        self.begun.insert(ix);
        window.row_begin(key);
        key
    }

    fn placement(
        &self,
        window: &Window,
        key: ViewKey,
        origin: Point<Pixels>,
        shown: bool,
    ) -> Placement {
        Placement {
            offset: row_offset(window.row_anchor(key), origin),
            clip: self.clip,
            keep_states: shown,
            scale_factor: self.scale,
        }
    }

    /// Whether the row can be replayed at `placement`: it moves by whole device pixels and its
    /// hover state does not change there.
    fn replayable(
        &self,
        window: &Window,
        key: ViewKey,
        placement: &Placement,
        shown: bool,
    ) -> bool {
        let on_grid = |value: f32| {
            let device = value * self.scale;
            (device - device.round()).abs() < 0.01
        };
        window.row_reusable_for(key, shown)
            && on_grid(placement.offset.x.0)
            && on_grid(placement.offset.y.0)
            && !window.row_hover_changed(key, placement)
    }

    fn extent_for(&self, row: Bounds<Pixels>) -> ContentMask<Pixels> {
        ContentMask {
            bounds: self.base_extent.union(&row),
        }
    }

    /// Forgets what the failed pass did: its rows are recorded at positions that were rolled
    /// back with the frame.
    pub(super) fn discard_pass(&mut self, window: &mut Window) {
        self.plans.clear();
        self.prepared.clear();
        self.begun.clear();
        self.visible_built = Pixels::ZERO;
        for ix in std::mem::take(&mut self.touched) {
            if let Some(key) = self.rows.remove(&ix) {
                window.row_drop(key);
            }
        }
    }

    /// Closes the frame: forgets the rows whose build never reached prepaint, hands the rows
    /// back to the store of the list and returns what painting needs.
    pub(super) fn finish(mut self, window: &mut Window, scroll_px: Pixels) -> RowPaint {
        for ix in std::mem::take(&mut self.begun) {
            if !self.prepared.contains(&ix)
                && let Some(key) = self.rows.remove(&ix)
            {
                window.row_drop(key);
            }
        }
        let mut plans = std::mem::take(&mut self.plans);
        plans.sort_by_key(|plan| plan.ix);
        let scroll_px = scroll_px.to_f64();
        let store = window.row_store(&self.list);
        store.rows = self.rows;
        store.constructed = true;
        store.last_scroll_top = scroll_px;
        store.moving_down = if scroll_px > self.last_scroll_px {
            true
        } else if scroll_px < self.last_scroll_px {
            false
        } else {
            self.moving_down
        };
        RowPaint { plans }
    }
}

impl RowPaint {
    /// Paints the rows of the frame: rows built in it are painted into scenes of their own,
    /// replayed rows reuse theirs; the rows that reach the viewport are shown, moved and
    /// clipped.
    pub(super) fn paint(&mut self, window: &mut Window, cx: &mut App) {
        for plan in &mut self.plans {
            match plan.element.as_mut() {
                Some(element) => {
                    window.row_paint_phase(plan.key, plan.extent, plan.shown, |window| {
                        element.paint(window, cx)
                    });
                }
                None => window.row_replay_paint(plan.key, &plan.placement, cx),
            }
            if plan.shown {
                window.row_show(plan.key, &plan.placement);
            }
        }
    }
}

impl StateInner {
    /// Prepares a visible row of the pass: replays it, or builds it when it has no usable
    /// record (or hover changed under the pointer). Returns the autoscroll request a freshly
    /// prepainted row made.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn prepare_visible_row(
        &mut self,
        rows: &mut RowSession,
        item: &mut ItemLayout,
        key: ViewKey,
        origin: Point<Pixels>,
        space: Size<AvailableSpace>,
        render_item: &mut RenderItemFn,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        let ix = item.index;
        let mut element = item.element.take();
        let mut key = key;
        if element.is_none() {
            let placement = rows.placement(window, key, origin, true);
            if rows.replayable(window, key, &placement, true) {
                window.row_replay_prepaint(key, &placement, cx);
                rows.touched.push(ix);
                rows.prepared.insert(ix);
                rows.plans.push(RowPlan {
                    ix,
                    key,
                    element: None,
                    placement,
                    extent: rows.extent_for(Bounds::default()),
                    shown: true,
                });
                return None;
            }
            // The row changed hover state or does not land on the device pixel grid.
            let (built, built_size, built_key) =
                self.build_row(ix, space, Some(&mut *rows), render_item, window, cx);
            self.adopt_remeasured(ix, built_size);
            element = Some(built);
            key = built_key.unwrap_or(key);
        }
        let mut element = element?;
        let row_size = self.mounted_item(ix).map_or(Size::default(), |item| {
            size(item.size().map_or(px(0.), |size| size.width), item.height())
        });
        let extent = rows.extent_for(Bounds {
            origin,
            size: row_size,
        });
        window.row_prepaint_phase(key, extent, rows.clip, origin, true, |window| {
            element.prepaint_at(origin, window, cx)
        });
        let request = window.take_autoscroll();
        if request.is_some() {
            window.row_forbid_replay(key);
        }
        rows.prepared.insert(ix);
        rows.visible_built += row_size.height;
        rows.plans.push(RowPlan {
            ix,
            key,
            element: Some(element),
            placement: Placement {
                offset: Point::default(),
                clip: rows.clip,
                keep_states: true,
                scale_factor: rows.scale,
            },
            extent,
            shown: true,
        });
        request
    }

    /// Stores the size a row was measured at, as the layout of a visible row does.
    fn adopt_remeasured(&mut self, ix: usize, measured: Size<Pixels>) {
        let mounted = self.mounted_range();
        let Some(item) = self.mounted_item(ix) else {
            return;
        };
        if item.size() != Some(measured) {
            self.replace_mounted_item(
                ix - mounted.start,
                ListItem::Measured {
                    size: measured,
                    focus_handle: item.focus_handle(),
                },
            );
        }
    }

    /// Keeps the rows retained around the viewport alive in the frame, drops those that left
    /// the retained region (in batches), and fills the missing ones within the frame's budget:
    /// everything on the first frame, afterwards the soft budget of PRF-005 R6 (three
    /// quarters of it toward the scroll direction, one quarter against it).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn prepare_overscan(
        &mut self,
        rows: &mut RowSession,
        bounds: Bounds<Pixels>,
        content_origin: Point<Pixels>,
        scroll_px: Pixels,
        space: Size<AvailableSpace>,
        render_item: &mut RenderItemFn,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (Some(&first), Some(&last)) = (rows.prepared.first(), rows.prepared.last()) else {
            return;
        };
        let viewport = bounds.size.height;
        let region_top = bounds.top() - viewport * OVERSCAN_VIEWPORTS;
        let region_bottom = bounds.bottom() + viewport * OVERSCAN_VIEWPORTS;
        // Rows leave in batches of at least a quarter of a viewport.
        let batch = viewport * 0.25;
        let top_of = |state: &Self, ix: usize| content_origin.y + state.row_top(ix) - scroll_px;
        let measured_height = |state: &Self, ix: usize| {
            state
                .mounted_item(ix)
                .and_then(|item| item.size())
                .map(|size| size.height)
        };

        // The rows retained so far outside the viewport.
        let others: Vec<usize> = rows
            .rows
            .keys()
            .copied()
            .filter(|ix| !rows.prepared.contains(ix))
            .collect();
        for ix in others {
            let Some(key) = rows.rows.get(&ix).copied() else {
                continue;
            };
            let top = top_of(self, ix);
            let kept = measured_height(self, ix).is_some_and(|height| {
                top + height >= region_top - batch && top <= region_bottom + batch
            });
            let origin = point(content_origin.x, top);
            let placement = rows.placement(window, key, origin, false);
            if kept && rows.replayable(window, key, &placement, false) {
                window.row_replay_prepaint(key, &placement, cx);
                rows.touched.push(ix);
                rows.prepared.insert(ix);
                rows.plans.push(RowPlan {
                    ix,
                    key,
                    element: None,
                    placement,
                    extent: rows.extent_for(Bounds::default()),
                    shown: false,
                });
            } else {
                // Gone from the region, or changed: built when it is shown.
                rows.rows.remove(&ix);
                window.row_drop(key);
            }
        }

        // The rows missing from the retained region, nearest first.
        let count = self.logical_count();
        let mut above: Vec<(usize, Pixels)> = Vec::new();
        for ix in (0..first).rev() {
            let Some(height) = measured_height(self, ix) else {
                break;
            };
            let top = top_of(self, ix);
            if top + height < region_top {
                break;
            }
            if !rows.rows.contains_key(&ix) {
                above.push((ix, height));
            }
        }
        let mut below: Vec<(usize, Pixels)> = Vec::new();
        for ix in last + 1..count {
            let Some(height) = measured_height(self, ix) else {
                break;
            };
            if top_of(self, ix) > region_bottom {
                break;
            }
            if !rows.rows.contains_key(&ix) {
                below.push((ix, height));
            }
        }
        let (toward, against) = if rows.moving_down_now(scroll_px) {
            (below, above)
        } else {
            (above, below)
        };
        let fill: Vec<usize> = if rows.constructed {
            let budget = viewport * EXTENSION_BUDGET_VIEWPORTS;
            let remaining = (budget - rows.visible_built).max(Pixels::ZERO);
            // Rows are taken while they fit in the share of their side; what the rows against
            // the scroll direction leave of their quarter goes to the rows toward it.
            let take = |candidates: Vec<(usize, Pixels)>, share: Pixels| {
                let mut spent = Pixels::ZERO;
                let mut taken = Vec::new();
                for (ix, height) in candidates {
                    if spent + height > share {
                        break;
                    }
                    spent += height;
                    taken.push(ix);
                }
                (taken, spent)
            };
            let (against_rows, against_spent) = take(against, remaining * 0.25);
            let (mut fill, _) = take(toward, remaining - against_spent);
            fill.extend(against_rows);
            fill
        } else {
            toward
                .into_iter()
                .chain(against)
                .map(|(ix, _)| ix)
                .collect()
        };
        for ix in fill {
            self.build_overscan_row(
                rows,
                ix,
                point(content_origin.x, top_of(self, ix)),
                space,
                render_item,
                window,
                cx,
            );
        }
    }

    /// Builds a row outside the viewport that is only kept: its size is the one the list
    /// knows, or it is not kept at all.
    #[allow(clippy::too_many_arguments)]
    fn build_overscan_row(
        &mut self,
        rows: &mut RowSession,
        ix: usize,
        origin: Point<Pixels>,
        space: Size<AvailableSpace>,
        render_item: &mut RenderItemFn,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(known) = self.mounted_item(ix).and_then(|item| item.size()) else {
            return;
        };
        let (mut element, built_size, key) =
            self.build_row(ix, space, Some(&mut *rows), render_item, window, cx);
        let Some(key) = key else {
            return;
        };
        if built_size != known {
            // Measuring is the business of the rows that are shown: leave the list's
            // geometry as it is and build the row when it is shown.
            rows.rows.remove(&ix);
            rows.begun.remove(&ix);
            window.row_drop(key);
            return;
        }
        let extent = rows.extent_for(Bounds {
            origin,
            size: known,
        });
        window.row_prepaint_phase(key, extent, rows.clip, origin, false, |window| {
            element.prepaint_at(origin, window, cx)
        });
        // An overscan row cannot scroll the list.
        if window.take_autoscroll().is_some() {
            window.row_forbid_replay(key);
        }
        rows.prepared.insert(ix);
        rows.plans.push(RowPlan {
            ix,
            key,
            element: Some(element),
            placement: Placement {
                offset: Point::default(),
                clip: rows.clip,
                keep_states: false,
                scale_factor: rows.scale,
            },
            extent,
            shown: false,
        });
    }
}

impl RowSession {
    fn moving_down_now(&self, scroll_px: Pixels) -> bool {
        let scroll_px = scroll_px.to_f64();
        if scroll_px > self.last_scroll_px {
            true
        } else if scroll_px < self.last_scroll_px {
            false
        } else {
            self.moving_down
        }
    }
}
