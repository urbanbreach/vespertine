//! The table state entity (CMP-019 R3-R7): column order and widths by id, the collapsed-group
//! set, the sort, the flattened rows and the scroll anchor of the virtual list.

use super::columns::{
    Column, IndicatorEdge, ResizeDrag, ResizeSide, drop_index, indicator, natural_widths,
    primary_index, reorder,
};
use super::gutter::{GutterGeometry, morph};
use super::rows::{
    GroupSpec, ScrollAnchor, TableRow, Transition, capture_anchor, flatten_groups, sync_rows,
};
use std::cell::Cell;
use std::collections::{BTreeMap, HashSet};
use std::time::{Duration, Instant};
use vespertine::{Context, EventEmitter, ListAlignment, ListState, px};

/// The direction of a sort.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortDirection {
    /// Smallest first.
    Ascending,
    /// Largest first.
    Descending,
}

/// The current sort, rendered as an indicator on the sorted column's header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sort {
    /// The sorted column id.
    pub column: String,
    /// The direction.
    pub direction: SortDirection,
}

/// Notifications of the table state.
#[derive(Clone, Debug, PartialEq)]
pub enum TableEvent {
    /// A column was resized; the width map is keyed by column id.
    ColumnResized(BTreeMap<String, f32>),
    /// The columns were reordered; the new order of all column ids.
    ColumnReordered(Vec<String>),
    /// Every column width was reset to its default.
    ColumnsReset,
    /// A header was clicked (outside a resize handle): the caller re-supplies the rows sorted.
    SortRequested(String),
    /// A group heading was toggled.
    GroupToggled(String),
    /// The paging footer was activated.
    LoadMoreRequested,
}

/// How long the gutter takes to morph between two geometries.
pub const GUTTER_MORPH_DURATION: Duration = Duration::from_millis(150);

#[derive(Clone, Copy, Debug)]
struct GutterMotion {
    from: GutterGeometry,
    target: GutterGeometry,
    started: Instant,
}

impl GutterMotion {
    fn progress(&self, now: Instant) -> f32 {
        let elapsed = now.saturating_duration_since(self.started);
        (elapsed.as_secs_f32() / GUTTER_MORPH_DURATION.as_secs_f32()).clamp(0.0, 1.0)
    }

    fn displayed(&self, now: Instant) -> GutterGeometry {
        let progress = self.progress(now);
        if progress >= 1.0 {
            return self.target;
        }
        let morphed = morph(&self.from, &self.target, progress);
        GutterGeometry {
            lanes: self.target.lanes,
            width: morphed.width,
            lane_spacing: morphed.lane_spacing,
            compact: morphed.compact_topology,
        }
    }
}

/// A column drag over the header: the dragged column and the visible column under the pointer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DragOver {
    /// The dragged column id.
    pub dragged: String,
    /// The id of the visible column the pointer is over.
    pub over: String,
}

/// The state of a data table.
pub struct TableState {
    columns: Vec<Column>,
    widths: BTreeMap<String, f32>,
    collapsed: HashSet<String>,
    sort: Option<Sort>,
    groups: Vec<GroupSpec>,
    has_more: bool,
    loading_more: bool,
    filter_text: String,
    active_filters: usize,
    rows: Vec<TableRow>,
    list: ListState,
    row_height: Option<f32>,
    transition: Option<Transition>,
    resize: Option<ResizeDrag>,
    drag_over: Option<DragOver>,
    header_width: Cell<f32>,
    gutter: Cell<Option<GutterGeometry>>,
    gutter_motion: Cell<Option<GutterMotion>>,
}

impl EventEmitter<TableEvent> for TableState {}

impl TableState {
    /// A state over `columns` (in their initial order). With `row_height` the list uses a
    /// uniform item height (36 px for a history-style table, 64 px for a grouped board);
    /// otherwise rows are measured.
    pub fn new(columns: Vec<Column>, row_height: Option<f32>) -> Self {
        let list = ListState::new(0, ListAlignment::Top, px(row_height.unwrap_or(40.0) * 4.0));
        if let Some(height) = row_height {
            list.set_estimated_item_height(Some(px(height)));
        }
        Self {
            columns,
            widths: BTreeMap::new(),
            collapsed: HashSet::new(),
            sort: None,
            groups: Vec::new(),
            has_more: false,
            loading_more: false,
            filter_text: String::new(),
            active_filters: 0,
            rows: Vec::new(),
            list,
            row_height,
            transition: None,
            resize: None,
            drag_over: None,
            header_width: Cell::new(0.0),
            gutter: Cell::new(None),
            gutter_motion: Cell::new(None),
        }
    }

    // ---- queries ----------------------------------------------------------------------------

    /// The virtual list of the rows.
    pub fn list_state(&self) -> &ListState {
        &self.list
    }

    /// The uniform row height, when the caller gave one.
    pub fn row_height(&self) -> Option<f32> {
        self.row_height
    }

    /// Every column in order, hidden ones included.
    pub fn columns(&self) -> &[Column] {
        &self.columns
    }

    /// The ids of every column in order.
    pub fn column_order(&self) -> Vec<String> {
        self.columns
            .iter()
            .map(|column| column.id.clone())
            .collect()
    }

    /// The visible columns in order.
    pub fn visible_columns(&self) -> Vec<&Column> {
        self.columns
            .iter()
            .filter(|column| column.visible)
            .collect()
    }

    /// The widths stored by resizes, keyed by column id.
    pub fn widths(&self) -> &BTreeMap<String, f32> {
        &self.widths
    }

    /// The flattened rows currently in the list.
    pub fn rows(&self) -> &[TableRow] {
        &self.rows
    }

    /// The collapsed group keys.
    pub fn collapsed(&self) -> &HashSet<String> {
        &self.collapsed
    }

    /// The current sort.
    pub fn sort(&self) -> Option<&Sort> {
        self.sort.as_ref()
    }

    /// The number of active filters (shown in the header's filter slot).
    pub fn active_filters(&self) -> usize {
        self.active_filters
    }

    /// Whether more data exists (the paging footer is shown).
    pub fn has_more(&self) -> bool {
        self.has_more
    }

    /// Whether the footer shows its loading state.
    pub fn loading_more(&self) -> bool {
        self.loading_more
    }

    /// The column drag in progress over the header.
    pub fn drag_over(&self) -> Option<&DragOver> {
        self.drag_over.as_ref()
    }

    /// The width of the header at the last paint (for resize and reorder math).
    pub fn header_width(&self) -> f32 {
        self.header_width.get()
    }

    /// Records the width of the header's column area (the table measures it at every paint;
    /// hosts that lay the header out themselves may supply it).
    pub fn record_header_width(&self, width: f32) {
        self.header_width.set(width);
    }

    /// The gutter geometry the table is morphing to (or has reached) at `now`: when `target`
    /// differs from the previous target the width and lane spacing interpolate linearly from the
    /// geometry shown so far over [`GUTTER_MORPH_DURATION`] (the compact rail takes over at
    /// progress 0.999 when entering and the full topology returns at the first frame when
    /// leaving). Returns the geometry to draw and whether the morph is still running.
    pub fn step_gutter(&self, target: GutterGeometry, now: Instant) -> (GutterGeometry, bool) {
        let motion = match self.gutter_motion.get() {
            None => GutterMotion {
                from: target,
                target,
                started: now,
            },
            Some(motion) if motion.target != target => GutterMotion {
                from: motion.displayed(now),
                target,
                started: now,
            },
            Some(motion) => motion,
        };
        self.gutter_motion.set(Some(motion));
        let moving = motion.from != motion.target && motion.progress(now) < 1.0;
        (motion.displayed(now), moving)
    }

    /// The geometry of the graph gutter at the last paint, when the table has one.
    pub fn gutter_geometry(&self) -> Option<GutterGeometry> {
        self.gutter.get()
    }

    pub(crate) fn record_gutter(&self, geometry: GutterGeometry) {
        self.gutter.set(Some(geometry));
    }

    /// The natural widths of the visible columns for the last painted header width.
    pub fn rendered_widths(&self) -> Vec<f32> {
        natural_widths(
            &self.visible_columns(),
            &self.widths,
            self.header_width.get(),
        )
    }

    // ---- columns ----------------------------------------------------------------------------

    /// Shows or hides a column.
    pub fn set_column_visible(&mut self, id: &str, visible: bool, cx: &mut Context<Self>) {
        if let Some(column) = self.columns.iter_mut().find(|column| column.id == id) {
            column.visible = visible;
            cx.notify();
        }
    }

    /// Restores the widths stored by the host (keyed by column id; unknown ids are ignored).
    pub fn restore_widths(&mut self, widths: &BTreeMap<String, f32>, cx: &mut Context<Self>) {
        for (id, width) in widths {
            if self.columns.iter().any(|column| &column.id == id) {
                self.widths.insert(id.clone(), *width);
            }
        }
        cx.notify();
    }

    /// Restores the column order stored by the host; ids it does not mention keep their
    /// relative place after the mentioned ones, unknown ids are ignored.
    pub fn restore_order(&mut self, order: &[String], cx: &mut Context<Self>) {
        let mut next: Vec<Column> = Vec::with_capacity(self.columns.len());
        for id in order {
            if let Some(ix) = self.columns.iter().position(|column| &column.id == id) {
                next.push(self.columns.remove(ix));
            }
        }
        next.append(&mut self.columns);
        self.columns = next;
        cx.notify();
    }

    /// Resets every column width to its default and notifies the host.
    pub fn reset_widths(&mut self, cx: &mut Context<Self>) {
        self.widths.clear();
        self.resize = None;
        cx.emit(TableEvent::ColumnsReset);
        cx.notify();
    }

    /// Starts a resize drag at the divider right of the visible column `left_id` at pointer x
    /// `press_x` (R3), recording the starting widths of the two adjacent columns.
    pub fn begin_resize(&mut self, left_id: &str, press_x: f32, cx: &mut Context<Self>) {
        let visible = self.visible_columns();
        let Some(left_ix) = visible.iter().position(|column| column.id == left_id) else {
            return;
        };
        if left_ix + 1 >= visible.len() {
            return;
        }
        let widths = natural_widths(&visible, &self.widths, self.header_width.get());
        let primary = primary_index(&visible);
        let side = |ix: usize| ResizeSide {
            start: widths[ix],
            min: visible[ix].effective_min(primary == Some(ix)),
            max: visible[ix].effective_max(primary == Some(ix)),
        };
        self.resize = Some(ResizeDrag::new(
            &visible[left_ix].id,
            &visible[left_ix + 1].id,
            press_x,
            side(left_ix),
            side(left_ix + 1),
            primary == Some(left_ix),
        ));
        cx.notify();
    }

    /// Continues the resize drag with the pointer at `pointer_x`; the host is notified of the
    /// new width map on each change.
    pub fn update_resize(&mut self, pointer_x: f32, cx: &mut Context<Self>) {
        let Some(drag) = &self.resize else {
            return;
        };
        let updates = drag.apply(pointer_x);
        let mut changed = false;
        for (id, width) in updates {
            if self.widths.get(&id) != Some(&width) {
                self.widths.insert(id, width);
                changed = true;
            }
        }
        if changed {
            cx.emit(TableEvent::ColumnResized(self.widths.clone()));
            cx.notify();
        }
    }

    /// Ends the resize drag (releasing the handle).
    pub fn end_resize(&mut self, cx: &mut Context<Self>) {
        if self.resize.take().is_some() {
            cx.notify();
        }
    }

    /// Whether a resize drag is in progress.
    pub fn is_resizing(&self) -> bool {
        self.resize.is_some()
    }

    /// The pointer of a column drag is at `x` relative to the header's left edge: finds the
    /// visible column under it (R4).
    pub fn drag_column_over(&mut self, dragged: &str, x: f32, cx: &mut Context<Self>) {
        let visible = self.visible_columns();
        if visible.is_empty() {
            return;
        }
        let preferred = natural_widths(&visible, &self.widths, self.header_width.get());
        let over = visible[drop_index(x, self.header_width.get(), &preferred)]
            .id
            .clone();
        let next = Some(DragOver {
            dragged: dragged.to_owned(),
            over,
        });
        if self.drag_over != next {
            self.drag_over = next;
            cx.notify();
        }
    }

    /// Where the insertion indicator sits: the visible column index and edge, `None` when the
    /// target is the dragged column or no drag is in progress.
    pub fn insertion_indicator(&self) -> Option<(usize, IndicatorEdge)> {
        let drag = self.drag_over.as_ref()?;
        let visible = self.visible_columns();
        let ids: Vec<&str> = visible.iter().map(|column| column.id.as_str()).collect();
        let over_ix = ids.iter().position(|id| *id == drag.over)?;
        indicator(&ids, &drag.dragged, over_ix)
    }

    /// Cancels a column drag.
    pub fn clear_drag_over(&mut self, cx: &mut Context<Self>) {
        if self.drag_over.take().is_some() {
            cx.notify();
        }
    }

    /// Releases a column drag: commits the reorder (R4) and reports the new order; nothing is
    /// reported when the order does not change.
    pub fn commit_reorder(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(drag) = self.drag_over.take() else {
            return false;
        };
        let order = self.column_order();
        let Some(next) = reorder(&order, &drag.dragged, &drag.over) else {
            cx.notify();
            return false;
        };
        let mut columns = std::mem::take(&mut self.columns);
        columns.sort_by_key(|column| next.iter().position(|id| *id == column.id));
        self.columns = columns;
        cx.emit(TableEvent::ColumnReordered(next));
        cx.notify();
        true
    }

    // ---- sort, filters, paging --------------------------------------------------------------

    /// Sets the sort shown on the headers.
    pub fn set_sort(&mut self, sort: Option<Sort>, cx: &mut Context<Self>) {
        self.sort = sort;
        cx.notify();
    }

    /// Reports a header click on column `id` (outside a resize handle).
    pub fn request_sort(&mut self, id: &str, cx: &mut Context<Self>) {
        cx.emit(TableEvent::SortRequested(id.to_owned()));
    }

    /// Sets the number of active filters for display.
    pub fn set_active_filters(&mut self, count: usize, cx: &mut Context<Self>) {
        self.active_filters = count;
        cx.notify();
    }

    /// Sets the filter text; a change clears the collapsed-group set (R6) and re-flattens.
    pub fn set_filter_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.filter_text != text {
            self.filter_text = text.to_owned();
            self.collapsed.clear();
            self.reflatten(false, cx);
        }
    }

    /// Shows the footer's loading state.
    pub fn set_loading_more(&mut self, loading: bool, cx: &mut Context<Self>) {
        self.loading_more = loading;
        cx.notify();
    }

    /// Reports the paging footer activated.
    pub fn request_load_more(&mut self, cx: &mut Context<Self>) {
        cx.emit(TableEvent::LoadMoreRequested);
    }

    // ---- rows -------------------------------------------------------------------------------

    /// Supplies the grouped data (R6): the rows are the groups flattened with the collapsed set
    /// and a paging footer when `has_more`. `animated` keeps the anchor alive until
    /// [`Self::settle_transition`].
    pub fn set_data(
        &mut self,
        groups: Vec<GroupSpec>,
        has_more: bool,
        animated: bool,
        cx: &mut Context<Self>,
    ) {
        self.groups = groups;
        self.has_more = has_more;
        self.reflatten(animated, cx);
    }

    /// Toggles a group heading: the collapsed set changes, the rows are re-flattened with the
    /// R5 splice and anchor logic and [`TableEvent::GroupToggled`] is emitted.
    pub fn toggle_group(&mut self, key: &str, animated: bool, cx: &mut Context<Self>) {
        if !self.collapsed.remove(key) {
            self.collapsed.insert(key.to_owned());
        }
        self.reflatten(animated, cx);
        cx.emit(TableEvent::GroupToggled(key.to_owned()));
    }

    /// Replaces the row sequence directly (instead of [`Self::set_data`]).
    pub fn set_rows(&mut self, rows: Vec<TableRow>, animated: bool, cx: &mut Context<Self>) {
        self.apply_rows(rows, animated);
        cx.notify();
    }

    fn reflatten(&mut self, animated: bool, cx: &mut Context<Self>) {
        let rows = flatten_groups(&self.groups, &self.collapsed, self.has_more);
        self.apply_rows(rows, animated);
        cx.notify();
    }

    fn apply_rows(&mut self, rows: Vec<TableRow>, animated: bool) {
        if let Some(anchor) = sync_rows(&self.list, &self.rows, &rows) {
            if animated {
                self.transition = Some(Transition::begin(&self.list, anchor));
            }
        }
        self.rows = rows;
    }

    /// The animated transition has settled: resolves the anchor again with the scroll position
    /// at this moment; a newer user scroll wins over the anchor captured at click time.
    pub fn settle_transition(&mut self, cx: &mut Context<Self>) {
        if let Some(transition) = self.transition.take() {
            transition.settle(&self.list, &self.rows);
            cx.notify();
        }
    }

    /// The anchor of the list right now.
    pub fn anchor(&self) -> Option<ScrollAnchor> {
        capture_anchor(&self.list, &self.rows)
    }
}
