//! Interactive terminal grid surfaces (CMP-021).
//!
//! A [`TerminalPanel`] folds terminal output bytes into retained, tabbed cell grids with an
//! [`Emulator`] (the `alacritty_terminal` state machine), paints the active grid with a
//! fixed-width cell model on the framework's text and quad primitives, encodes focused keyboard
//! input and paste into bytes, supports pointer selection, copy, scrolling and edge auto-scroll,
//! draws its tabs as a closable, reorderable strip (CMP-017) above the grid, and can be hosted
//! in a [`BottomDock`] whose height is shared with the content above it.
//!
//! The byte transport (sessions, pseudo-terminals, reconnection) is the host's: the panel
//! consumes bytes through [`TerminalPanel::feed`] and emits bytes through the `on_input`
//! callback and grid sizes through `on_resize`.

mod dock;
mod element;
mod emulator;
mod geometry;
mod input;
mod panel;
mod theme;

pub use dock::{BottomDock, DockGeometry};
pub use element::{
    CellRun, Segment, TerminalElement, measure_cell_width, merge_runs, row_segments, terminal_font,
    terminal_font_features,
};
pub use emulator::{
    CellColor, CellSnapshot, CursorSnapshot, Emulator, GridPoint, GridSize, MIN_COLS, MIN_ROWS,
    SCROLLBACK_LINES, SelectionType, Side,
};
pub use geometry::{
    CellHit, DEFAULT_FONT_SIZE, DOCK_MAX_FRACTION, DOCK_MIN_HEIGHT, DOCK_TOLERANCE, DRAG_THRESHOLD,
    FALLBACK_CELL_WIDTH_RATIO, LINE_HEIGHT_RATIO, MAX_MEASURED, PADDING, ScrollbarMetrics,
    WheelAccumulator, cell_at, default_line_height, dock_height, edge_scroll_lines,
    grid_dimensions, scrollbar_metrics,
};
pub use input::{
    ClipboardShortcut, InputCoalescer, PASTE_END, PASTE_START, clipboard_shortcut,
    keystroke_to_bytes, paste_bytes,
};
pub use panel::{
    AUTOSCROLL_TICK, GridGeometry, INPUT_COALESCE, RESIZE_DEBOUNCE, TabId, TerminalPanel,
    active_after_close, active_after_move, exit_status_line,
};
pub use theme::{CUBE_LEVELS, CURSOR_BLOCK_ALPHA, DIM_ALPHA, TerminalTheme, indexed_color, rgb8};
