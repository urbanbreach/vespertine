//! The interactive virtual file tree (CMP-016).
//!
//! The pure parts are testable without a window: [`TreeModel`] (entries, lazy load states,
//! selection, cursor, anchor), [`sync_list_rows`] (path-attached scroll anchors), the drag
//! helpers ([`destination_paths`], [`edge_scroll_speed`], [`DragSession`]), the rename helpers
//! ([`basename_range`], [`validate_name`], [`RenameSession`], [`delete_subject`]), the context
//! menu model ([`ContextMenu`]) and the search-result tree ([`SearchTree`],
//! [`RevealCoordinator`]). The [`Tree`] view renders the model with the virtual list and wires
//! input, the context menu, inline rename and drag and drop to owner callbacks; it contains no
//! file-system access.

pub mod drag;
pub mod menu;
pub mod model;
pub mod rename;
pub mod rows;
pub mod search;
mod search_view;
pub mod style;
mod view;

pub use drag::{
    DRAG_TICK, DragItem, DragSession, DropHit, EDGE_BAND_MAX, EDGE_SPEED_MAX, HOVER_EXPAND_DELAY,
    RAIL_EXCLUSION, ROOT_ROW_HEIGHT, TICK_ELAPSED_CAP, TickOutput, TreeDragPayload,
    destination_paths, edge_scroll_speed, resolve_destination,
};
pub use menu::{ContextMenu, MenuItem, copy_path_text, is_mutable, menu_items};
pub use model::{
    Activation, Entry, EntryKind, LoadRequest, LoadState, Node, Outcome, Page, PageError, ROOT,
    RevealStep, RowKind, TreeModel, VisibleRow, file_name, is_within, join_path, parent_path,
    row_parent,
};
pub use rename::{
    ENTRY_CHANGED_MESSAGE, INVALID_NAME_MESSAGE, RenameCommit, RenameSession, basename_range,
    delete_subject, validate_name,
};
pub use rows::{fade_flags, list_fade_flags, resolve_anchor, splice_plan, sync_list_rows};
pub use search::{
    RESULT_LIMIT, RevealCoordinator, RevealEffect, RevealIntent, RevealProgress, RevealRequest,
    SEARCH_ROW_HEIGHT, SearchActivation, SearchNode, SearchResult, SearchRow, SearchTree,
};
pub use search_view::{SearchEvent, SearchResults};
pub use style::{RowVisual, TreeTheme, TreeWashes};
pub use view::{Tree, TreeEvent, default_identity};
