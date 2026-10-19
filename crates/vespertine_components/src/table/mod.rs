//! Resizable, reorderable data tables and graphs (CMP-019).
//!
//! The pure parts are testable without a window: column descriptors and widths with the resize
//! and reorder math ([`columns`]), row identity, group flattening, the splice planner and the
//! scroll anchor of the virtual list ([`rows`]) and the graph gutter geometry ([`gutter`]).
//! [`TableState`] is the entity that holds column order and widths by id, the collapsed-group
//! set, the rows and the anchor and emits [`TableEvent`]s; [`Table`] renders it.

pub mod columns;
pub mod gutter;
pub mod rows;
pub mod state;
mod view;

pub use columns::{
    Align, Column, IndicatorEdge, ResizeDrag, ResizeSide, WidthMode, drop_index, handle_positions,
    header_visible, indicator, natural_widths, reorder, resize_after_primary, resize_pair,
    resolve_align,
};
pub use gutter::{
    GraphGutter, GutterGeometry, GutterPaint, MorphedGutter, fit_gutter, fit_width, graph_gutter,
    morph, natural_width, snap_width,
};
pub use rows::{
    FOOTER_ID, GroupSpec, ScrollAnchor, TableRow, TableRowKind, Transition, capture_anchor,
    flatten_groups, plan_splice, resolve_anchor, sync_rows,
};
pub use state::{DragOver, GUTTER_MORPH_DURATION, Sort, SortDirection, TableEvent, TableState};
pub use view::{Cell, CellContent, ColumnDrag, GraphColumn, Table, TableTheme};
