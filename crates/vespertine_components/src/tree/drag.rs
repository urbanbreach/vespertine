//! Drag and drop of tree entries: payload, destination resolution, the gesture-level resolver
//! and the autoscroll / hover-expand tick (CMP-016 R18-R20). Everything here is pure.

use super::model::{EntryKind, RowKind, TreeModel, file_name, is_within, join_path, parent_path};
use std::collections::HashSet;
use std::rc::Rc;
use std::time::Duration;

/// The interval of the drag tick.
pub const DRAG_TICK: Duration = Duration::from_millis(16);
/// The widest autoscroll band, in logical pixels.
pub const EDGE_BAND_MAX: f32 = 28.0;
/// The autoscroll speed at the very edge, in logical pixels per second.
pub const EDGE_SPEED_MAX: f32 = 540.0;
/// The longest elapsed time one tick may scroll for.
pub const TICK_ELAPSED_CAP: Duration = Duration::from_millis(50);
/// How long the pointer must rest on a collapsed directory destination before it expands.
pub const HOVER_EXPAND_DELAY: Duration = Duration::from_millis(650);
/// Width of the right-hand strip reserved for the scroll rail, which is never a destination.
pub const RAIL_EXCLUSION: f32 = 12.0;
/// Height of the dedicated "move to workspace root" row.
pub const ROOT_ROW_HEIGHT: f32 = 24.0;

/// One dragged entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DragItem {
    /// The entry path.
    pub path: String,
    /// The entry kind.
    pub kind: EntryKind,
}

/// The payload of an internal entry drag (R18).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeDragPayload {
    /// The entry the drag started on.
    pub dragged: DragItem,
    /// What moves: the selection roots in visible order when the dragged row is selected and
    /// the selection has more than one entry, otherwise just the dragged row.
    pub items: Rc<[DragItem]>,
}

impl TreeDragPayload {
    /// The payload of a drag that starts on `path` in `model`.
    pub fn for_drag(model: &TreeModel, path: &str) -> Option<Self> {
        let dragged = Self::dragged_item(model, path)?;
        let items = if Self::drags_selection(model, path) {
            Self::selection_items(model)
        } else {
            Rc::from(vec![dragged.clone()])
        };
        Some(Self { dragged, items })
    }

    /// Whether a drag starting on `path` carries the whole selection: the row is selected and
    /// the selection has more than one entry.
    pub fn drags_selection(model: &TreeModel, path: &str) -> bool {
        model.is_selected(path) && model.selection_len() > 1
    }

    /// The dragged entry of a drag starting on `path`.
    pub fn dragged_item(model: &TreeModel, path: &str) -> Option<DragItem> {
        let node = model.node(path)?;
        Some(DragItem {
            path: path.to_owned(),
            kind: node.entry.kind,
        })
    }

    /// The selection roots in visible order, so selected descendants of a selected ancestor
    /// are not duplicated.
    pub fn selection_items(model: &TreeModel) -> Rc<[DragItem]> {
        model
            .selection_roots()
            .into_iter()
            .filter_map(|root| {
                model.node(&root).map(|node| DragItem {
                    path: root,
                    kind: node.entry.kind,
                })
            })
            .collect()
    }
}

/// What the pointer is over, as seen by the destination resolver (R19).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropHit {
    /// Outside the tree bounds.
    Outside,
    /// Over the strip reserved for the scroll rail.
    Rail,
    /// Over the visible row with this index.
    Row(usize),
    /// Over the empty area below the last row.
    Empty,
    /// Over the dedicated "Move to workspace root" row.
    RootRow,
}

/// The destination directory (the empty string is the workspace root) the pointer designates:
/// a directory row is its own destination, a file row's destination is its parent, a symlink is
/// none, a status row belongs to its directory, the empty area and the root row are the root,
/// and the outside or the rail have none.
pub fn resolve_destination(model: &TreeModel, hit: DropHit) -> Option<String> {
    match hit {
        DropHit::Outside | DropHit::Rail => None,
        DropHit::Empty | DropHit::RootRow => Some(String::new()),
        DropHit::Row(ix) => {
            let row = model.visible_rows().get(ix)?;
            match row.kind {
                RowKind::Entry => {
                    let node = model.node(&row.path)?;
                    match node.entry.kind {
                        EntryKind::Directory => Some(row.path.clone()),
                        EntryKind::File => Some(parent_path(&row.path).to_owned()),
                        EntryKind::Symlink => None,
                    }
                }
                _ => row.status_directory().map(str::to_owned),
            }
        }
    }
}

/// The moves a drop of `items` into `directory` issues, in the order of `items`, or `None` when
/// the destination is invalid for the gesture (R19): an item already inside the directory is
/// skipped; a directory dropped into itself or its own subtree, or two items resolving to the
/// same destination name, invalidate the whole gesture; nothing left to move is invalid.
pub fn destination_paths(items: &[DragItem], directory: &str) -> Option<Vec<(String, String)>> {
    let mut seen = HashSet::new();
    let mut moves = Vec::new();
    for item in items {
        if item.kind == EntryKind::Directory && is_within(directory, &item.path) {
            return None;
        }
        let destination = join_path(directory, file_name(&item.path));
        if !seen.insert(destination.clone()) {
            return None;
        }
        if parent_path(&item.path) == directory {
            continue;
        }
        moves.push((item.path.clone(), destination));
    }
    (!moves.is_empty()).then_some(moves)
}

/// The autoscroll speed, in logical pixels per second (negative scrolls up), for a pointer at
/// `y` in a viewport spanning `top..bottom`: `540 x (1 - distance_from_edge / band)` within a
/// band of `min(28, height / 2)` of an edge, zero elsewhere and outside the viewport.
pub fn edge_scroll_speed(y: f32, top: f32, bottom: f32) -> f32 {
    if !(y >= top && y <= bottom) {
        return 0.0;
    }
    let band = EDGE_BAND_MAX.min((bottom - top) / 2.0);
    if band <= 0.0 {
        return 0.0;
    }
    let from_top = y - top;
    let from_bottom = bottom - y;
    if from_top < band {
        -EDGE_SPEED_MAX * (1.0 - from_top / band)
    } else if from_bottom < band {
        EDGE_SPEED_MAX * (1.0 - from_bottom / band)
    } else {
        0.0
    }
}

/// What one tick of a drag asks the view to do.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TickOutput {
    /// Logical pixels to scroll by (positive scrolls down).
    pub scroll: f32,
    /// A collapsed directory destination to expand.
    pub expand: Option<String>,
}

/// The state of an internal drag inside the tree.
#[derive(Clone, Debug)]
pub struct DragSession {
    /// The payload being dragged.
    pub payload: TreeDragPayload,
    destination: Option<String>,
    valid: bool,
    expandable: bool,
    hover: Duration,
    expanded: bool,
    pointer_y: Option<f32>,
}

impl DragSession {
    /// A session for `payload`.
    pub fn new(payload: TreeDragPayload) -> Self {
        Self {
            payload,
            destination: None,
            valid: false,
            expandable: false,
            hover: Duration::ZERO,
            expanded: false,
            pointer_y: None,
        }
    }

    /// The destination under the pointer, valid or not.
    pub fn destination(&self) -> Option<&str> {
        self.destination.as_deref()
    }

    /// Whether the destination under the pointer accepts the payload.
    pub fn is_valid(&self) -> bool {
        self.destination.is_some() && self.valid
    }

    /// The pointer's vertical position in window coordinates; `None` outside the viewport.
    pub fn set_pointer_y(&mut self, y: Option<f32>) {
        self.pointer_y = y;
    }

    /// Changes the highlighted destination immediately. `expandable` is whether it is a
    /// collapsed directory (the hover-to-expand candidate). A changed destination restarts
    /// the hover timer. Returns whether anything changed.
    pub fn set_destination(
        &mut self,
        destination: Option<String>,
        valid: bool,
        expandable: bool,
    ) -> bool {
        let changed = self.destination != destination || self.valid != valid;
        if self.destination != destination || self.expandable != expandable {
            self.hover = Duration::ZERO;
            self.expanded = false;
        }
        self.destination = destination;
        self.valid = valid;
        self.expandable = expandable;
        changed
    }

    /// Advances the drag by `elapsed` for a viewport spanning `top..bottom`: the scroll is the
    /// edge speed times the elapsed time capped at 0.05 s; the hover timer expands a collapsed
    /// directory destination once after 650 ms.
    pub fn tick(&mut self, elapsed: Duration, top: f32, bottom: f32) -> TickOutput {
        let mut output = TickOutput::default();
        if let Some(y) = self.pointer_y {
            let speed = edge_scroll_speed(y, top, bottom);
            output.scroll = speed * elapsed.min(TICK_ELAPSED_CAP).as_secs_f32();
        }
        if self.expandable && !self.expanded {
            self.hover += elapsed;
            if self.hover >= HOVER_EXPAND_DELAY {
                self.expanded = true;
                output.expand = self.destination.clone();
            }
        }
        output
    }
}
