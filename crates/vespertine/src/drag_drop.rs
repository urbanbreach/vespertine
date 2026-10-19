//! Typed drag-and-drop policy helpers.
//!
//! The base drag machinery (`on_drag`, `on_drop`, `on_drag_move`, `drag_over`, `can_drop`,
//! [`App::stop_active_drag`]) moves typed payloads between elements. This module adds the
//! consumer-generic policies that sit on top of it:
//!
//! * pure insertion geometry for reorderable lists ([`uniform_slot`], [`variable_slot`],
//!   [`column_slot`], [`insertion_index`], [`slide_transition`]) and the [`ReorderPreview`] state
//!   that tracks the hovered slot for the live gap preview;
//! * stable group identities and snapshot validation ([`DragGroup`], [`ReorderPayload`]);
//! * hierarchical (tree) move resolution ([`selection_roots`], [`destination_directory`],
//!   [`resolve_tree_moves`]);
//! * the three edge-autoscroll profiles as pure functions ([`linear_autoscroll_speed`],
//!   [`ramp_autoscroll_step`], [`selection_autoscroll_step`]) and a loop driver
//!   ([`AutoscrollLoop`]) that runs them on the framework's executor clock;
//! * [`DragSession`], which owns the destination, the dwell-to-expand timer and the autoscroll loop
//!   of one drag and cancels all of them together;
//! * classification and validation of externally dropped paths ([`PathKind`], [`classify_path`],
//!   [`ExternalPaths::require_regular_files`]);
//! * drag cursor helpers ([`set_drag_cursor`], [`clear_drag_cursor`]).

use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    fmt,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use crate::{
    App, Bounds, CursorStyle, ExternalPaths, KeyDownEvent, Pixels, Point, ScrollHandle,
    SharedString, Subscription, Task, Window, point, px,
};

/// Interval between autoscroll ticks.
pub const AUTOSCROLL_TICK: Duration = Duration::from_millis(16);
/// Upper bound on the elapsed time a single time-based autoscroll tick may account for.
pub const AUTOSCROLL_MAX_ELAPSED: Duration = Duration::from_millis(50);
/// Maximum band width of the time-based linear autoscroll profile.
pub const LINEAR_AUTOSCROLL_MAX_BAND: Pixels = px(28.);
/// Speed, in pixels per second, reached at the very edge by the time-based linear profile.
pub const LINEAR_AUTOSCROLL_MAX_SPEED: Pixels = px(540.);
/// Band width of the per-tick penetration ramp profile.
pub const RAMP_AUTOSCROLL_BAND: Pixels = px(48.);
/// Largest step, in pixels per tick, of the per-tick penetration ramp profile.
pub const RAMP_AUTOSCROLL_MAX_STEP: Pixels = px(12.);
/// Pixels scrolled per tick per pixel of distance for text-selection autoscroll.
pub const SELECTION_AUTOSCROLL_FACTOR: f32 = 0.2;
/// Inset applied to the right and bottom edges when clamping a selection end into an editor.
pub const SELECTION_EDGE_INSET: Pixels = px(0.5);
/// Width of the scroll rail at the right edge of a tree, which is never a drop target.
pub const SCROLL_RAIL_WIDTH: Pixels = px(12.);
/// How long a destination directory must stay unchanged before a collapsed one expands.
pub const DWELL_TO_EXPAND: Duration = Duration::from_millis(650);

// ---------------------------------------------------------------------------------------------
// Slot geometry
// ---------------------------------------------------------------------------------------------

/// Converts a pointer coordinate along the list axis into a content coordinate: the pointer
/// coordinate minus the target's origin coordinate, minus the scroll offset (negative once content
/// has moved toward the origin side, so subtracting it adds the scrolled-away distance), minus the
/// leading padding.
pub fn content_coordinate(
    pointer: Pixels,
    origin: Pixels,
    scroll_offset: Pixels,
    leading_padding: Pixels,
) -> Pixels {
    pointer - origin - scroll_offset - leading_padding
}

/// How a coordinate outside the rows of [`variable_slot`] is resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotMode {
    /// A coordinate outside `0..total` resolves to no slot.
    Strict,
    /// A coordinate outside `0..total` resolves to the nearest valid slot (first or last).
    Clamped,
}

/// Slot under `coordinate` for `count` uniform slots of size `slot_size`: `floor(coordinate /
/// slot_size)` clamped to `0..count`. Leading padding therefore belongs to slot 0, trailing padding
/// to the last slot, and a negative coordinate to slot 0. Returns `None` for an empty list or a
/// non-positive slot size.
pub fn uniform_slot(coordinate: Pixels, slot_size: Pixels, count: usize) -> Option<usize> {
    if count == 0 || slot_size.0.is_nan() || slot_size.0 <= 0. {
        return None;
    }
    let slot = (coordinate.0 / slot_size.0).floor();
    // Float to integer casts saturate, so negative and NaN coordinates land on slot 0.
    Some((slot as usize).min(count - 1))
}

/// Slot under `coordinate` for rows of differing `heights` separated by a constant `gap`.
///
/// The cumulative bottom of row `k` is the sum of the heights of rows `0..=k` plus `(k + 1)` gaps;
/// the slot is the first row whose cumulative bottom exceeds the coordinate. In
/// [`SlotMode::Strict`] a coordinate outside `0..total` (the heights plus `n - 1` gaps) resolves to
/// `None`; in [`SlotMode::Clamped`] it resolves to the first or last row. An empty list is always
/// `None`.
pub fn variable_slot(
    coordinate: Pixels,
    heights: &[Pixels],
    gap: Pixels,
    mode: SlotMode,
) -> Option<usize> {
    if heights.is_empty() {
        return None;
    }
    let total =
        heights.iter().map(|height| height.0).sum::<f32>() + gap.0 * (heights.len() - 1) as f32;
    let coordinate = coordinate.0;
    if mode == SlotMode::Strict && !(0. ..total).contains(&coordinate) {
        return None;
    }
    if coordinate < 0. || coordinate.is_nan() {
        return Some(0);
    }
    let mut bottom = 0.;
    for (ix, height) in heights.iter().enumerate() {
        bottom += height.0 + gap.0;
        if bottom > coordinate {
            return Some(ix);
        }
    }
    Some(heights.len() - 1)
}

/// Slot under pointer position `x` when reordering columns of differing desired widths.
///
/// `x` is first mapped into desired-width space, `x' = clamp(x, 0, rendered_width) * sum(desired) /
/// rendered_width`. The slot is the first column for which `x'` is less than its cumulative start
/// plus half its width, or the last column if none. Returns `None` for no columns or a non-positive
/// rendered width.
pub fn column_slot(x: Pixels, desired_widths: &[Pixels], rendered_width: Pixels) -> Option<usize> {
    if desired_widths.is_empty() || rendered_width.0.is_nan() || rendered_width.0 <= 0. {
        return None;
    }
    let desired_total = desired_widths.iter().map(|width| width.0).sum::<f32>();
    let mapped = x.0.clamp(0., rendered_width.0) * desired_total / rendered_width.0;
    let mut start = 0.;
    for (ix, width) in desired_widths.iter().enumerate() {
        if mapped < start + width.0 / 2. {
            return Some(ix);
        }
        start += width.0;
    }
    Some(desired_widths.len() - 1)
}

// ---------------------------------------------------------------------------------------------
// Reordering
// ---------------------------------------------------------------------------------------------

/// A resolved reorder: the item at `from` is removed and re-inserted at index `to` of the list
/// without it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReorderMove {
    /// Index of the dragged item before the move.
    pub from: usize,
    /// Insertion index into the list after the dragged item was removed.
    pub to: usize,
}

impl ReorderMove {
    /// Applies the move to `items`. Does nothing when `from` is out of range.
    pub fn apply<T>(&self, items: &mut Vec<T>) {
        if self.from >= items.len() {
            return;
        }
        let item = items.remove(self.from);
        items.insert(self.to.min(items.len()), item);
    }
}

/// Insertion index for dropping the item at `from` onto the item at slot `over` in a list of `len`
/// items. The dragged item is removed; the new index is the position of the target item after
/// removal, plus 1 when `from < over` and plus 0 otherwise, clamped to the length of the list. A
/// drop onto itself (or an out-of-range `from`) changes nothing and returns `None`.
pub fn insertion_index(from: usize, over: usize, len: usize) -> Option<usize> {
    if from >= len || from == over {
        return None;
    }
    let target_after_removal = if over > from { over - 1 } else { over };
    let insertion = target_after_removal + usize::from(from < over);
    Some(insertion.min(len - 1))
}

/// Resolves the reorder for dropping the item at `from` on slot `over`, or `None` when nothing
/// changes.
pub fn reorder_move(from: usize, over: usize, len: usize) -> Option<ReorderMove> {
    insertion_index(from, over, len).map(|to| ReorderMove { from, to })
}

/// Shift, in whole slots, of row `ix` in the live gap preview while the row at `from` is dragged
/// over slot `over`.
///
/// The dragged row's shift is `over - from`. For `from < over` rows in `(from, over]` shift by
/// `-1`; for `from > over` rows in `[over, from)` shift by `+1`. Everything else stays at zero.
pub fn slide_shift(ix: usize, from: usize, over: usize) -> i32 {
    let (ix, from, over) = (ix as i64, from as i64, over as i64);
    let shift = if ix == from {
        over - from
    } else if from < over && ix > from && ix <= over {
        -1
    } else if from > over && ix >= over && ix < from {
        1
    } else {
        0
    };
    shift as i32
}

/// A row's slide from the shift of the previously hovered slot to the shift of the current one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlideTransition {
    /// Shift, in slots, the row is sliding from.
    pub from: i32,
    /// Shift, in slots, the row is sliding to.
    pub to: i32,
}

impl SlideTransition {
    /// Whether the row's destination changed, so its slide animation must restart. Rows with an
    /// unchanged destination keep their running animation.
    pub fn restarts(&self) -> bool {
        self.from != self.to
    }

    /// Offset in pixels at animation `progress` (clamped to `0..=1`) for slots of `slot_size`.
    pub fn offset(&self, progress: f32, slot_size: Pixels) -> Pixels {
        let progress = progress.clamp(0., 1.);
        slot_size * (self.from as f32 + (self.to - self.from) as f32 * progress)
    }
}

/// Slide of row `ix` when the hovered slot changes from `previous_over` to `over` while the row at
/// `from` is dragged.
pub fn slide_transition(
    ix: usize,
    from: usize,
    previous_over: usize,
    over: usize,
) -> SlideTransition {
    SlideTransition {
        from: slide_shift(ix, from, previous_over),
        to: slide_shift(ix, from, over),
    }
}

/// Hover state of an in-progress reorder: the dragged row, the hovered slot and the previously
/// hovered slot (so only rows whose destination changed restart their slide).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReorderPreview {
    from: Option<usize>,
    over: Option<usize>,
    previous_over: Option<usize>,
}

impl ReorderPreview {
    /// An idle preview with no drag in progress.
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts previewing a drag of the row at `from`, with no hovered slot yet.
    pub fn begin(&mut self, from: usize) {
        self.from = Some(from);
        self.over = None;
        self.previous_over = None;
    }

    /// The dragged row, while a drag is being previewed.
    pub fn from(&self) -> Option<usize> {
        self.from
    }

    /// The hovered slot, if the pointer is over the list.
    pub fn over(&self) -> Option<usize> {
        self.over
    }

    /// Whether any drag-derived visual state is present.
    pub fn is_active(&self) -> bool {
        self.from.is_some() || self.over.is_some()
    }

    /// Records the hovered slot. Returns `true` when it changed.
    pub fn hover(&mut self, over: usize) -> bool {
        if self.over == Some(over) {
            return false;
        }
        self.previous_over = self.over.or(self.from);
        self.over = Some(over);
        true
    }

    /// The pointer left the list: collapses the gap back to the source row. Returns `true` when a
    /// slot was hovered.
    pub fn leave(&mut self) -> bool {
        if self.over.is_none() {
            return false;
        }
        self.previous_over = self.over;
        self.over = None;
        true
    }

    /// Clears every drag-derived value (gap, highlight, hovered slot).
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Clears the preview when no drag is active. Consumers MUST call this while rendering, since a
    /// mouse-up outside every accepting target ends the drag without any drop callback. Returns
    /// `true` when state was cleared.
    pub fn clear_if_idle(&mut self, cx: &App) -> bool {
        if !cx.has_active_drag() && self.is_active() {
            self.clear();
            return true;
        }
        false
    }

    /// Slide of row `ix` for the current hover, or `None` when there is no drag.
    pub fn transition(&self, ix: usize) -> Option<SlideTransition> {
        let from = self.from?;
        let over = self.over.unwrap_or(from);
        let previous_over = self.previous_over.unwrap_or(from);
        Some(slide_transition(ix, from, previous_over, over))
    }

    /// The reorder a drop at the current hover would apply in a list of `len` items.
    pub fn pending_move(&self, len: usize) -> Option<ReorderMove> {
        reorder_move(self.from?, self.over?, len)
    }
}

// ---------------------------------------------------------------------------------------------
// Group identity and snapshot validation
// ---------------------------------------------------------------------------------------------

/// Stable identity of the list a reorder payload came from.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DragGroup {
    /// Identifier of the list.
    pub list_id: SharedString,
    /// The active filter of the list.
    pub filter: SharedString,
    /// Key of the owner the list belongs to.
    pub owner_key: SharedString,
}

impl DragGroup {
    /// Builds a group identity from its three parts.
    pub fn new(
        list_id: impl Into<SharedString>,
        filter: impl Into<SharedString>,
        owner_key: impl Into<SharedString>,
    ) -> Self {
        Self {
            list_id: list_id.into(),
            filter: filter.into(),
            owner_key: owner_key.into(),
        }
    }
}

/// Why a reorder drop was refused. A refused drop must not reorder anything and must clear the
/// drag state; it MAY be shown as rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReorderRejection {
    /// The payload came from a list with a different group identity.
    ForeignGroup,
    /// The dragged id is not part of the payload's snapshot, so the payload is invalid.
    DraggedNotInSnapshot,
    /// The visible ids changed since the drag started.
    StaleSnapshot,
}

impl fmt::Display for ReorderRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ForeignGroup => "the dragged item belongs to a different list",
            Self::DraggedNotInSnapshot => "the dragged item is not part of the drag snapshot",
            Self::StaleSnapshot => "the list changed while the item was being dragged",
        })
    }
}

impl std::error::Error for ReorderRejection {}

/// Typed payload of a reorder drag: the identity of the source list, the dragged id and a snapshot
/// of the ordered visible ids at drag start.
#[derive(Clone, Debug)]
pub struct ReorderPayload<Id> {
    /// Identity of the list the drag started in.
    pub group: DragGroup,
    /// The dragged item.
    pub dragged: Id,
    /// Ordered visible ids when the drag started.
    pub snapshot: Arc<[Id]>,
}

impl<Id: PartialEq> ReorderPayload<Id> {
    /// Builds a payload for dragging `dragged` out of the list whose ordered visible ids are
    /// `snapshot`.
    pub fn new(group: DragGroup, dragged: Id, snapshot: impl Into<Arc<[Id]>>) -> Self {
        Self {
            group,
            dragged,
            snapshot: snapshot.into(),
        }
    }

    /// Index of the dragged id in the snapshot.
    pub fn source_index(&self) -> Option<usize> {
        self.snapshot.iter().position(|id| *id == self.dragged)
    }

    /// Checks the payload against a target: the group identities must be equal, the snapshot must
    /// contain the dragged id and it must equal `current_ids` exactly. Returns the dragged id's
    /// source index.
    pub fn validate(
        &self,
        target_group: &DragGroup,
        current_ids: &[Id],
    ) -> Result<usize, ReorderRejection> {
        if self.group != *target_group {
            return Err(ReorderRejection::ForeignGroup);
        }
        let Some(from) = self.source_index() else {
            return Err(ReorderRejection::DraggedNotInSnapshot);
        };
        if *self.snapshot != *current_ids {
            return Err(ReorderRejection::StaleSnapshot);
        }
        Ok(from)
    }

    /// Validates the payload and resolves a drop on slot `over`. `Ok(None)` means the drop is valid
    /// but changes nothing (a drop onto itself).
    pub fn resolve_drop(
        &self,
        target_group: &DragGroup,
        current_ids: &[Id],
        over: usize,
    ) -> Result<Option<ReorderMove>, ReorderRejection> {
        let from = self.validate(target_group, current_ids)?;
        Ok(reorder_move(from, over, current_ids.len()))
    }
}

// ---------------------------------------------------------------------------------------------
// Hierarchical move resolution
// ---------------------------------------------------------------------------------------------

/// A dragged tree entry: its path relative to the tree root and whether it is a directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeEntry {
    /// Path relative to the tree root.
    pub path: PathBuf,
    /// Whether the entry is a directory.
    pub is_directory: bool,
}

impl TreeEntry {
    /// A directory entry.
    pub fn directory(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            is_directory: true,
        }
    }

    /// A file entry.
    pub fn file(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            is_directory: false,
        }
    }
}

/// What a visible tree row is, for drop-target resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeRowKind {
    /// A directory row; it is its own destination.
    Directory,
    /// A file row; its destination is its parent directory.
    File,
    /// A link row; it is not a target.
    Link,
    /// An explicit root row; its destination is the root.
    Root,
}

/// A visible tree row under the pointer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeRow {
    /// Path relative to the tree root (empty for the root row).
    pub path: PathBuf,
    /// The row's kind.
    pub kind: TreeRowKind,
}

/// What the pointer is over inside a tree's drag region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeHit<'a> {
    /// A visible row.
    Row(&'a TreeRow),
    /// The empty area below the last row.
    BelowLastRow,
}

/// The selection roots of a multi-selection: the entries of `visible` (in visible order) for which
/// `is_selected` holds and none of whose ancestors is selected, so nothing is moved twice.
pub fn selection_roots(
    visible: &[TreeEntry],
    is_selected: impl Fn(&TreeEntry) -> bool,
) -> Vec<&TreeEntry> {
    let selected: HashSet<&Path> = visible
        .iter()
        .filter(|entry| is_selected(entry))
        .map(|entry| entry.path.as_path())
        .collect();
    visible
        .iter()
        .filter(|entry| selected.contains(entry.path.as_path()))
        .filter(|entry| {
            !entry
                .path
                .ancestors()
                .skip(1)
                .any(|ancestor| selected.contains(ancestor))
        })
        .collect()
}

/// Resolves the destination directory (relative to the root; the empty path is the root) for a
/// pointer position over a tree.
///
/// The pointer must be inside `region`; the rightmost [`SCROLL_RAIL_WIDTH`] pixels are never a
/// target. A directory row is its own destination, a file row targets its parent directory, a link
/// row is not a target, a root row targets the root, and the empty area below the last row targets
/// the root.
pub fn destination_directory(
    region: Bounds<Pixels>,
    pointer: Point<Pixels>,
    hit: TreeHit<'_>,
) -> Option<PathBuf> {
    if !region.contains(&pointer) || pointer.x >= region.right() - SCROLL_RAIL_WIDTH {
        return None;
    }
    match hit {
        TreeHit::BelowLastRow => Some(PathBuf::new()),
        TreeHit::Row(row) => match row.kind {
            TreeRowKind::Directory => Some(row.path.clone()),
            TreeRowKind::File => Some(row.path.parent().map(Path::to_path_buf).unwrap_or_default()),
            TreeRowKind::Link => None,
            TreeRowKind::Root => Some(PathBuf::new()),
        },
    }
}

/// One resolved move of a tree drop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeMove {
    /// Current path of the entry.
    pub source: PathBuf,
    /// New path: `destination_directory/name`, or just `name` for the root.
    pub destination: PathBuf,
}

/// Why a tree drop is invalid: there is no destination, hide drop feedback and use the prohibited
/// cursor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeMoveError {
    /// A directory would move into itself or one of its descendants.
    IntoSelf {
        /// The offending directory.
        source: PathBuf,
    },
    /// Two items would land on the same destination path.
    NameCollision {
        /// The contested destination path.
        destination: PathBuf,
    },
    /// An entry has no last path component to name it by.
    MissingName {
        /// The unnameable entry.
        source: PathBuf,
    },
    /// Every item is skipped (its parent already is the destination), or there are no items.
    NothingToMove,
}

impl fmt::Display for TreeMoveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IntoSelf { source } => {
                write!(f, "cannot move {} into itself", source.display())
            }
            Self::NameCollision { destination } => {
                write!(f, "several items would become {}", destination.display())
            }
            Self::MissingName { source } => write!(f, "{} has no name", source.display()),
            Self::NothingToMove => f.write_str("nothing to move"),
        }
    }
}

impl std::error::Error for TreeMoveError {}

/// Resolves the moves for dropping `items` (normally [`selection_roots`]) into `destination`
/// (relative to the root; empty for the root).
///
/// Each item lands on `destination/name` where name is the last component of its source path. An
/// item whose current parent equals the destination is skipped. The whole gesture is invalid if a
/// directory would go into itself or a descendant, or if two items would land on the same
/// destination; otherwise the moves are the non-skipped items, and an empty list is invalid.
pub fn resolve_tree_moves(
    items: &[TreeEntry],
    destination: &Path,
) -> Result<Vec<TreeMove>, TreeMoveError> {
    let mut landed = HashSet::new();
    let mut moves = Vec::new();
    for item in items {
        if item.is_directory && destination.starts_with(&item.path) {
            return Err(TreeMoveError::IntoSelf {
                source: item.path.clone(),
            });
        }
        let Some(name) = item.path.file_name() else {
            return Err(TreeMoveError::MissingName {
                source: item.path.clone(),
            });
        };
        let target = destination.join(name);
        if !landed.insert(target.clone()) {
            return Err(TreeMoveError::NameCollision {
                destination: target,
            });
        }
        if item.path.parent() == Some(destination) {
            continue;
        }
        moves.push(TreeMove {
            source: item.path.clone(),
            destination: target,
        });
    }
    if moves.is_empty() {
        return Err(TreeMoveError::NothingToMove);
    }
    Ok(moves)
}

// ---------------------------------------------------------------------------------------------
// Edge autoscroll profiles (pure)
// ---------------------------------------------------------------------------------------------

/// Profile (a), time-based linear ramp for trees and long lists. Returns the scroll speed in
/// pixels per second for pointer coordinate `y` and viewport edges `top`/`bottom`.
///
/// The band is `min(28 px, height / 2)`. The speed is 0 outside the viewport or when
/// `bottom <= top`; in the top band it is `-540 * (1 - (y - top) / band)`, in the bottom band
/// `+540 * (1 - (bottom - y) / band)`, and 0 elsewhere.
pub fn linear_autoscroll_speed(y: Pixels, top: Pixels, bottom: Pixels) -> Pixels {
    let (y, top, bottom) = (y.0, top.0, bottom.0);
    if y.is_nan() || y < top || y > bottom || bottom <= top {
        return px(0.);
    }
    let band = LINEAR_AUTOSCROLL_MAX_BAND.0.min((bottom - top) / 2.);
    let top_depth = y - top;
    let bottom_depth = bottom - y;
    if top_depth < band {
        px(-LINEAR_AUTOSCROLL_MAX_SPEED.0 * (1. - top_depth / band))
    } else if bottom_depth < band {
        px(LINEAR_AUTOSCROLL_MAX_SPEED.0 * (1. - bottom_depth / band))
    } else {
        px(0.)
    }
}

/// Distance a time-based tick moves the offset: `speed` times the elapsed time, with the elapsed
/// time capped at [`AUTOSCROLL_MAX_ELAPSED`].
pub fn linear_autoscroll_delta(speed: Pixels, elapsed: Duration) -> Pixels {
    speed * elapsed.min(AUTOSCROLL_MAX_ELAPSED).as_secs_f32()
}

/// Profile (b), per-tick penetration ramp for sidebars and constrained sections. Returns the step
/// in pixels for one tick.
///
/// In the top zone (`y < top + 48`, including positions beyond the edge) the step is
/// `-12 * clamp((top + 48 - y) / 48, 0, 1)`; in the bottom zone (`y > bottom - 48`) it is
/// `+12 * clamp((y - (bottom - 48)) / 48, 0, 1)`; it is zero when `bottom <= top` or between
/// the zones. The top zone wins where the zones overlap.
pub fn ramp_autoscroll_step(y: Pixels, top: Pixels, bottom: Pixels) -> Pixels {
    let (y, top, bottom) = (y.0, top.0, bottom.0);
    if y.is_nan() || bottom <= top {
        return px(0.);
    }
    let band = RAMP_AUTOSCROLL_BAND.0;
    let max = RAMP_AUTOSCROLL_MAX_STEP.0;
    if y < top + band {
        px(-max * ((top + band - y) / band).clamp(0., 1.))
    } else if y > bottom - band {
        px(max * ((y - (bottom - band)) / band).clamp(0., 1.))
    } else {
        px(0.)
    }
}

/// Profile (c), drag selection of text outside an editor's vertical bounds. Returns the step in
/// pixels for one tick: `sign(distance) * clamp(|distance| * 0.2, 1, line_height)` where distance is
/// `y - top` above the editor and `y - bottom` below it; zero inside.
pub fn selection_autoscroll_step(
    y: Pixels,
    top: Pixels,
    bottom: Pixels,
    line_height: Pixels,
) -> Pixels {
    let distance = if y < top {
        y.0 - top.0
    } else if y > bottom {
        y.0 - bottom.0
    } else {
        return px(0.);
    };
    let magnitude = (distance.abs() * SELECTION_AUTOSCROLL_FACTOR)
        .max(1.)
        .min(line_height.0.max(1.));
    px(magnitude.copysign(distance))
}

/// Clamps a selection end into `bounds`, with the right and bottom edges inset by
/// [`SELECTION_EDGE_INSET`].
pub fn clamp_selection_end(position: Point<Pixels>, bounds: Bounds<Pixels>) -> Point<Pixels> {
    let max_x = (bounds.right() - SELECTION_EDGE_INSET).max(bounds.left());
    let max_y = (bounds.bottom() - SELECTION_EDGE_INSET).max(bounds.top());
    point(
        position.x.max(bounds.left()).min(max_x),
        position.y.max(bounds.top()).min(max_y),
    )
}

/// Whether a content scroll position in `0..=max` can still move in `direction` (the sign of the
/// intended movement; zero never can).
pub fn scroll_can_change(position: Pixels, direction: f32, max: Pixels) -> bool {
    if direction < 0. {
        position.0 > 0.
    } else if direction > 0. {
        position.0 < max.0
    } else {
        false
    }
}

/// Applies `delta` to a content scroll `position` clamped to `0..=max`. Positive scroll positions
/// move content toward larger positions.
pub fn advance_scroll_position(position: Pixels, delta: Pixels, max: Pixels) -> Pixels {
    px((position.0 + delta.0).clamp(0., max.0.max(0.)))
}

// ---------------------------------------------------------------------------------------------
// Autoscroll loop
// ---------------------------------------------------------------------------------------------

/// Timing information handed to each autoscroll tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AutoscrollTick {
    /// Time since the previous tick, capped at [`AUTOSCROLL_MAX_ELAPSED`]; zero on the first tick.
    pub elapsed: Duration,
}

/// Edge profile used by [`AutoscrollLoop::start_vertical`].
#[derive(Clone)]
pub enum EdgeAutoscroll {
    /// Profile (a): time-based linear ramp; the loop ends when the pointer leaves the viewport.
    TimeLinear,
    /// Profile (b): per-tick penetration ramp; the loop ends when the pointer leaves the section
    /// whose rows can be reordered, even though the edge zones extend beyond it.
    PenetrationRamp {
        /// Bounds of the reorderable section, evaluated on every tick.
        section: Rc<dyn Fn() -> Bounds<Pixels>>,
    },
}

struct AutoscrollState {
    generation: Rc<Cell<u64>>,
    running: Rc<Cell<bool>>,
    task: RefCell<Option<Task<()>>>,
}

/// Drives edge autoscroll on a 16 ms tick on the framework's executor clock.
///
/// A loop runs only while its drag is active (for [`start`](Self::start)), its tick function keeps
/// returning `true`, and its generation is current. [`cancel`](Self::cancel) and
/// [`invalidate`](Self::invalidate) advance the generation so a loop belonging to a finished drag
/// can never act on a later one. Cloning shares the same loop.
#[derive(Clone)]
pub struct AutoscrollLoop {
    state: Rc<AutoscrollState>,
}

impl Default for AutoscrollLoop {
    fn default() -> Self {
        Self::new()
    }
}

impl AutoscrollLoop {
    /// Creates an idle loop.
    pub fn new() -> Self {
        Self {
            state: Rc::new(AutoscrollState {
                generation: Rc::new(Cell::new(0)),
                running: Rc::new(Cell::new(false)),
                task: RefCell::new(None),
            }),
        }
    }

    /// Whether a loop is currently ticking.
    pub fn is_running(&self) -> bool {
        self.state.running.get()
    }

    /// The current generation. It advances every time a loop is started, cancelled or invalidated.
    pub fn generation(&self) -> u64 {
        self.state.generation.get()
    }

    /// Marks the running loop (if any) as belonging to a finished drag. It exits at its next tick
    /// without acting.
    pub fn invalidate(&self) {
        self.state
            .generation
            .set(self.state.generation.get().wrapping_add(1));
        self.state.running.set(false);
    }

    /// Invalidates the loop and drops its task immediately.
    pub fn cancel(&self) {
        self.invalidate();
        // Take first so a task dropped from inside its own tick does not hold the borrow.
        let task = self.state.task.borrow_mut().take();
        drop(task);
    }

    /// Starts a loop that ticks every [`AUTOSCROLL_TICK`] while a drag is active, unless one is
    /// already running. `tick` returns whether the loop continues; it ends the moment `tick`
    /// returns `false`, the drag ends, or the generation is no longer current. The first tick
    /// runs as soon as the executor polls the task and reports an elapsed time of zero.
    pub fn start(
        &self,
        window: &Window,
        cx: &App,
        tick: impl FnMut(AutoscrollTick, &mut Window, &mut App) -> bool + 'static,
    ) {
        self.spawn(window, cx, true, tick);
    }

    /// Like [`start`](Self::start) but not tied to an active typed drag, for dragging a text
    /// selection (profile (c)). `tick` is responsible for ending the loop when the selection
    /// gesture stops.
    pub fn start_for_selection(
        &self,
        window: &Window,
        cx: &App,
        tick: impl FnMut(AutoscrollTick, &mut Window, &mut App) -> bool + 'static,
    ) {
        self.spawn(window, cx, false, tick);
    }

    fn spawn(
        &self,
        window: &Window,
        cx: &App,
        requires_active_drag: bool,
        mut tick: impl FnMut(AutoscrollTick, &mut Window, &mut App) -> bool + 'static,
    ) {
        if self.is_running() {
            return;
        }
        let mine = self.state.generation.get().wrapping_add(1);
        self.state.generation.set(mine);
        self.state.running.set(true);
        let generation = self.state.generation.clone();
        let running = self.state.running.clone();
        let task = window.spawn(cx, async move |cx| {
            let mut previous: Option<Instant> = None;
            loop {
                if generation.get() != mine {
                    return;
                }
                let now = cx.background_executor().now();
                let elapsed = previous.map_or(Duration::ZERO, |previous| {
                    now.saturating_duration_since(previous)
                        .min(AUTOSCROLL_MAX_ELAPSED)
                });
                previous = Some(now);
                let keep_going = cx
                    .update(|window, cx| {
                        generation.get() == mine
                            && (!requires_active_drag || cx.has_active_drag())
                            && tick(AutoscrollTick { elapsed }, window, cx)
                    })
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
                cx.background_executor().timer(AUTOSCROLL_TICK).await;
            }
            if generation.get() == mine {
                running.set(false);
            }
        });
        *self.state.task.borrow_mut() = Some(task);
    }

    /// Starts vertical autoscroll of `handle` with the given edge `profile`, driven by the
    /// pointer position, unless a loop is already running.
    ///
    /// The viewport edges are the handle's bounds. The loop ends when the pointer leaves the
    /// region of the profile, when the drag ends, or when the content is already at the limit in
    /// the direction the pointer asks for. Offsets of a [`ScrollHandle`] are negative as content
    /// scrolls; this converts to the positive scroll positions of the profiles and clamps to
    /// `0..=max`.
    pub fn start_vertical(
        &self,
        window: &Window,
        cx: &App,
        handle: &ScrollHandle,
        profile: EdgeAutoscroll,
    ) {
        let handle = handle.clone();
        self.start(window, cx, move |tick, window, _| {
            let pointer = window.mouse_position();
            let viewport = handle.bounds();
            let (top, bottom) = (viewport.top(), viewport.bottom());
            let (direction, delta) = match &profile {
                EdgeAutoscroll::TimeLinear => {
                    if !viewport.contains(&pointer) {
                        return false;
                    }
                    let speed = linear_autoscroll_speed(pointer.y, top, bottom);
                    (speed.0, linear_autoscroll_delta(speed, tick.elapsed))
                }
                EdgeAutoscroll::PenetrationRamp { section } => {
                    if !section().contains(&pointer) {
                        return false;
                    }
                    let step = ramp_autoscroll_step(pointer.y, top, bottom);
                    (step.0, step)
                }
            };
            let offset = handle.offset();
            let position = -offset.y;
            let max = handle.max_offset().y;
            if direction != 0. && !scroll_can_change(position, direction, max) {
                return false;
            }
            let next = advance_scroll_position(position, delta, max);
            if next != position {
                handle.set_offset(point(offset.x, -next));
                window.refresh();
            }
            true
        });
    }
}

// ---------------------------------------------------------------------------------------------
// Drag cursor
// ---------------------------------------------------------------------------------------------

fn set_drag_cursor_style(window: &mut Window, cx: &mut App, style: CursorStyle) -> bool {
    if !cx.has_active_drag() {
        return false;
    }
    if cx.active_drag_cursor_style() != Some(style) {
        cx.set_active_drag_cursor_style(style, window);
    }
    true
}

/// Sets the active drag's cursor: a closed hand when the current target would accept the drop,
/// the prohibited-operation cursor when it would not. Returns `false` when no drag is active.
pub fn set_drag_cursor(window: &mut Window, cx: &mut App, accepts: bool) -> bool {
    let style = if accepts {
        CursorStyle::ClosedHand
    } else {
        CursorStyle::OperationNotAllowed
    };
    set_drag_cursor_style(window, cx, style)
}

/// Sets the active drag's cursor back to the arrow, as when the drag state is cleared. Returns
/// `false` when no drag is active.
pub fn clear_drag_cursor(window: &mut Window, cx: &mut App) -> bool {
    set_drag_cursor_style(window, cx, CursorStyle::Arrow)
}

// ---------------------------------------------------------------------------------------------
// Drag session: destination, dwell timer and autoscroll of one drag
// ---------------------------------------------------------------------------------------------

struct SessionState<K> {
    destination: RefCell<Option<K>>,
    dwell: RefCell<Option<Task<()>>>,
    autoscroll: AutoscrollLoop,
}

/// The transient state of one hierarchical drag: the resolved destination `K`, the dwell timer
/// that expands a collapsed destination after [`DWELL_TO_EXPAND`], and the autoscroll loop.
/// Cloning shares the state, so it can be captured by listeners.
///
/// Escape ([`cancel_on_escape`](Self::cancel_on_escape)), window deactivation
/// ([`cancel_on_deactivation`](Self::cancel_on_deactivation)), the pointer leaving the drag region
/// ([`pointer_moved`](Self::pointer_moved)), an incompatible payload
/// ([`retain_if_compatible`](Self::retain_if_compatible)) and the end of the drag
/// ([`clear_if_idle`](Self::clear_if_idle)) all clear the destination, the dwell timer and the
/// autoscroll loop together.
pub struct DragSession<K> {
    state: Rc<SessionState<K>>,
}

impl<K> Clone for DragSession<K> {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
        }
    }
}

impl<K: Clone + PartialEq + 'static> Default for DragSession<K> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Clone + PartialEq + 'static> DragSession<K> {
    /// Creates an idle session.
    pub fn new() -> Self {
        Self {
            state: Rc::new(SessionState {
                destination: RefCell::new(None),
                dwell: RefCell::new(None),
                autoscroll: AutoscrollLoop::new(),
            }),
        }
    }

    /// The autoscroll loop of this session.
    pub fn autoscroll(&self) -> &AutoscrollLoop {
        &self.state.autoscroll
    }

    /// The currently resolved destination, if any.
    pub fn destination(&self) -> Option<K> {
        self.state.destination.borrow().clone()
    }

    /// Whether a dwell timer is waiting to fire.
    pub fn is_dwelling(&self) -> bool {
        self.state
            .dwell
            .borrow()
            .as_ref()
            .is_some_and(|task| !task.is_ready())
    }

    /// Records the destination resolved for the pointer and updates the drag cursor (closed hand
    /// for a destination, prohibited for none). When the destination changed, the dwell timer
    /// restarts: after [`DWELL_TO_EXPAND`] unchanged, `on_dwell` is called once with the
    /// destination. Returns whether the destination changed.
    pub fn set_destination(
        &self,
        destination: Option<K>,
        window: &mut Window,
        cx: &mut App,
        on_dwell: impl Fn(&K, &mut Window, &mut App) + 'static,
    ) -> bool {
        set_drag_cursor(window, cx, destination.is_some());
        if *self.state.destination.borrow() == destination {
            return false;
        }
        *self.state.dwell.borrow_mut() = None;
        *self.state.destination.borrow_mut() = destination.clone();
        if let Some(destination) = destination {
            let task = window.spawn(cx, async move |cx| {
                cx.background_executor().timer(DWELL_TO_EXPAND).await;
                cx.update(|window, cx| on_dwell(&destination, window, cx))
                    .ok();
            });
            *self.state.dwell.borrow_mut() = Some(task);
        }
        true
    }

    /// Drops the destination, the dwell timer and the autoscroll loop without touching the drag.
    pub fn clear(&self) {
        *self.state.destination.borrow_mut() = None;
        let dwell = self.state.dwell.borrow_mut().take();
        drop(dwell);
        self.state.autoscroll.cancel();
    }

    /// Clears the session when no drag is active (the drag ended by drop or by a mouse-up outside
    /// every target). Consumers MUST call this while rendering. Returns whether no drag is active.
    pub fn clear_if_idle(&self, cx: &App) -> bool {
        if cx.has_active_drag() {
            return false;
        }
        self.clear();
        true
    }

    /// Cancels the session and ends the drag: clears every piece of state, sets the drag cursor
    /// back to the arrow and stops the active drag. Returns whether a drag was active.
    pub fn cancel(&self, window: &mut Window, cx: &mut App) -> bool {
        self.clear();
        clear_drag_cursor(window, cx);
        cx.stop_active_drag(window)
    }

    /// Cancels the drag when `event` is the Escape key. Returns whether it did.
    pub fn cancel_on_escape(
        &self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        event.keystroke.key == "escape" && cx.has_active_drag() && self.cancel(window, cx)
    }

    /// Cancels the drag when the pointer leaves the drag `region`. Returns whether the pointer is
    /// inside the region (and the drag therefore continues).
    pub fn pointer_moved(
        &self,
        region: Bounds<Pixels>,
        pointer: Point<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        if region.contains(&pointer) {
            return true;
        }
        self.cancel(window, cx);
        false
    }

    /// Cancels the drag when the payload stopped being compatible. Returns `compatible`.
    pub fn retain_if_compatible(
        &self,
        compatible: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        if !compatible {
            self.cancel(window, cx);
        }
        compatible
    }

    /// Cancels the session whenever the window is deactivated, for as long as the returned
    /// subscription lives.
    pub fn cancel_on_deactivation(&self, window: &mut Window) -> Subscription {
        let session = self.clone();
        let (subscription, activate) = window.activation_observers.insert(
            (),
            Box::new(move |window, cx| {
                if !window.is_window_active() {
                    session.cancel(window, cx);
                }
                true
            }),
        );
        activate();
        subscription
    }
}

// ---------------------------------------------------------------------------------------------
// External file drops
// ---------------------------------------------------------------------------------------------

/// What a dropped path turned out to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathKind {
    /// A regular file (after following symbolic links).
    File,
    /// A directory (after following symbolic links).
    Directory,
    /// Anything else: missing, unreadable, a dangling link, a device or a socket.
    Other,
}

/// Classifies `path` as a regular file, a directory or neither. Metadata is read through symbolic
/// links; a path whose metadata cannot be read (missing or unreadable) is [`PathKind::Other`].
pub fn classify_path(path: &Path) -> PathKind {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => PathKind::File,
        Ok(metadata) if metadata.is_dir() => PathKind::Directory,
        _ => PathKind::Other,
    }
}

/// A whole external drop was rejected because some entries are not regular files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDropRejection {
    /// The entries that are not regular files, with what they are.
    pub rejected: Vec<(PathBuf, PathKind)>,
}

impl fmt::Display for FileDropRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Only files can be dropped here; not accepted:")?;
        for (ix, (path, kind)) in self.rejected.iter().enumerate() {
            let what = match kind {
                PathKind::Directory => "folder",
                _ => "not a readable file",
            };
            write!(
                f,
                "{} {} ({what})",
                if ix == 0 { "" } else { "," },
                path.display()
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for FileDropRejection {}

impl ExternalPaths {
    /// Classifies every dropped path. Reads file system metadata, so call it when the drop is
    /// delivered rather than on every pointer move.
    pub fn classified(&self) -> Vec<(&Path, PathKind)> {
        self.paths()
            .iter()
            .map(|path| (path.as_path(), classify_path(path)))
            .collect()
    }

    /// Accepts the drop only if every entry is a regular file; otherwise the whole drop is
    /// rejected, naming every entry that is not one, so the consumer can show the message and
    /// attach nothing.
    pub fn require_regular_files(&self) -> Result<&[PathBuf], FileDropRejection> {
        let rejected: Vec<_> = self
            .classified()
            .into_iter()
            .filter(|(_, kind)| *kind != PathKind::File)
            .map(|(path, kind)| (path.to_path_buf(), kind))
            .collect();
        if rejected.is_empty() {
            Ok(self.paths())
        } else {
            Err(FileDropRejection { rejected })
        }
    }
}

#[cfg(test)]
mod tests;
