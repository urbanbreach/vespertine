//! Incremental draw-order assignment for [`Scene`](crate::Scene) (PRF-002).
//!
//! Every inserted bounds receives the order `1 + max(order of every earlier overlapping
//! bounds)`. Two strategies produce exactly that value:
//!
//! * **Replay**: the previous frame's insertion stream (bounds and assigned order per
//!   position) is kept. An entry whose bounds equal the previous entry at the same stream
//!   position, and which overlaps none of the regions that changed earlier in this frame,
//!   inherits the previous order without any search.
//! * **Search**: everything else is resolved by scanning the earlier entries of the frame
//!   linearly until [`REPLAY_COMPARISON_BUDGET`] comparisons were spent, after which a uniform
//!   spatial grid is built from the frame's entries and used for the rest of the frame.

use crate::util::{ceil_fast, floor_fast};
use crate::{Bounds, ScaledPixels};

/// Side of one spatial grid cell in device pixels.
pub(crate) const GRID_CELL_SIZE: f32 = 64.0;
/// Maximum number of grid cells along one axis.
pub(crate) const GRID_MAX_CELLS_PER_AXIS: usize = 256;
/// Number of bit-grid cells along one axis of the changed-region filter.
pub(crate) const CHANGED_GRID_SIZE: usize = 64;
/// Exact comparisons spent on replay searches in one frame before the spatial grid is built.
pub(crate) const REPLAY_COMPARISON_BUDGET: u32 = 32_768;

const NIL: u32 = u32::MAX;

/// Counters describing how the draw orders of the current frame were assigned.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SceneOrderStats {
    /// Entries inserted into the order stream this frame.
    pub entries: u32,
    /// Entries whose order was copied from the previous frame's stream.
    pub replayed_entries: u32,
    /// Entries whose order had to be computed.
    pub recomputed_entries: u32,
    /// Exact overlap comparisons spent on replay checks and linear searches this frame.
    pub replay_comparisons: u32,
    /// Queries answered by the spatial grid this frame.
    pub spatial_queries: u32,
    /// Times the spatial grid was built this frame.
    pub grid_builds: u32,
    /// Old and new bounds recorded as changed regions this frame.
    pub changed_regions: u32,
}

/// Edges of a bounds, computed exactly as the overlap predicate computes them.
///
/// Upstream orders `ScaledPixels` with the IEEE total order, so `-0.0 < 0.0` and NaNs are
/// ordered; the predicate and equality below follow that order bit for bit.
#[derive(Clone, Copy, Debug)]
struct Rect {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    /// The edges as integers that compare in the IEEE total order (`x0`, `y0`, `x1`, `y1`).
    keys: [i32; 4],
}

/// The integer that orders like `value` does in the IEEE total order (see `f32::total_cmp`).
#[inline]
fn total_order_key(value: f32) -> i32 {
    let bits = value.to_bits() as i32;
    bits ^ (((bits >> 31) as u32) >> 1) as i32
}

impl Rect {
    fn new(bounds: &Bounds<ScaledPixels>) -> Self {
        let x0 = bounds.origin.x.0;
        let y0 = bounds.origin.y.0;
        let x1 = x0 + bounds.size.width.0;
        let y1 = y0 + bounds.size.height.0;
        Self {
            x0,
            y0,
            x1,
            y1,
            keys: [
                total_order_key(x0),
                total_order_key(y0),
                total_order_key(x1),
                total_order_key(y1),
            ],
        }
    }

    /// The upstream strict-inequality overlap predicate: each rect starts strictly before the
    /// other one ends, on both axes, in the IEEE total order.
    #[inline]
    fn intersects(&self, other: &Rect) -> bool {
        self.keys[0] < other.keys[2]
            && other.keys[0] < self.keys[2]
            && self.keys[1] < other.keys[3]
            && other.keys[1] < self.keys[3]
    }

    /// Finite edges with non-negative extent. Anything else is handled by exact scans only.
    #[inline]
    fn is_well_formed(&self) -> bool {
        self.x0.is_finite()
            && self.y0.is_finite()
            && self.x1.is_finite()
            && self.y1.is_finite()
            && self.x1 >= self.x0
            && self.y1 >= self.y0
    }

    /// Well-formed with positive extent on both axes.
    #[inline]
    fn has_area(&self) -> bool {
        self.is_well_formed() && self.x1 > self.x0 && self.y1 > self.y0
    }
}

impl PartialEq for Rect {
    /// Bitwise equality: rects that compare equal are indistinguishable to the predicate.
    fn eq(&self, other: &Self) -> bool {
        self.x0.to_bits() == other.x0.to_bits()
            && self.y0.to_bits() == other.y0.to_bits()
            && self.x1.to_bits() == other.x1.to_bits()
            && self.y1.to_bits() == other.y1.to_bits()
    }
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    rect: Rect,
    order: u32,
}

#[derive(Clone, Copy)]
struct Cell {
    /// Maximum order of any bounds touching the cell.
    max_order: u32,
    /// Maximum order of bounds that cover the whole cell.
    cover_max: u32,
    /// Newest partial node, or `NIL`.
    head: u32,
}

const EMPTY_CELL: Cell = Cell {
    max_order: 0,
    cover_max: 0,
    head: NIL,
};

struct PartialNode {
    rect: Rect,
    order: u32,
    /// Maximum order of this node and every older node in the same list.
    remaining_max: u32,
    next: u32,
}

/// Uniform grid. Cell `k` along an axis covers `[(k - 1) * 64, k * 64)`; cell 0 extends to
/// negative infinity and the last cell to positive infinity.
#[derive(Default)]
struct Grid {
    nx: usize,
    ny: usize,
    cells: Vec<Cell>,
    partial: Vec<PartialNode>,
    /// Entries that cannot be mapped to cells (non-finite or inverted edges).
    malformed: Vec<Entry>,
}

/// First cell an edge touches. Zero edges reach into the infinite cell below zero because the
/// total order distinguishes `-0.0` from `0.0`, which makes bounds that merely touch at zero
/// overlap for the predicate.
#[inline]
fn lower_cell(edge: f32, count: usize) -> usize {
    if edge == 0.0 {
        return 0;
    }
    (floor_fast(edge / GRID_CELL_SIZE) + 1.0).clamp(0.0, (count - 1) as f32) as usize
}

/// Last cell the upper edge of a bounds with positive extent touches (see [`lower_cell`]).
#[inline]
fn upper_cell(edge: f32, count: usize) -> usize {
    if edge == 0.0 {
        return 1;
    }
    ceil_fast(edge / GRID_CELL_SIZE).clamp(0.0, (count - 1) as f32) as usize
}

/// Cells of one axis that the interval touches, first to last inclusive.
#[inline]
fn axis_range(low: f32, high: f32, count: usize) -> (usize, usize) {
    let first = lower_cell(low, count);
    if high > low {
        (first, upper_cell(high, count).max(first))
    } else if high == 0.0 {
        (0, 1)
    } else {
        (first, first)
    }
}

/// Whether the interval spans all of interior cell `k` of an axis with `count` cells. The cell
/// starting at zero additionally requires the interval to start strictly below zero, so that
/// signed-zero touching is always decided by the exact predicate on the partial list.
#[inline]
fn axis_covers(low: f32, high: f32, k: usize, count: usize) -> bool {
    k >= 1
        && k + 1 < count
        && low <= (k - 1) as f32 * GRID_CELL_SIZE
        && high >= k as f32 * GRID_CELL_SIZE
        && (k > 1 || low < 0.0)
}

impl Grid {
    fn reset(&mut self, nx: usize, ny: usize) {
        self.nx = nx;
        self.ny = ny;
        self.cells.clear();
        self.cells.resize(nx * ny, EMPTY_CELL);
        self.partial.clear();
        self.malformed.clear();
    }

    fn insert(&mut self, entry: Entry) {
        if !entry.rect.is_well_formed() {
            self.malformed.push(entry);
            return;
        }
        let rect = entry.rect;
        let (x_first, x_last) = axis_range(rect.x0, rect.x1, self.nx);
        let (y_first, y_last) = axis_range(rect.y0, rect.y1, self.ny);
        let has_area = rect.has_area();
        for y in y_first..=y_last {
            let covers_y = has_area && axis_covers(rect.y0, rect.y1, y, self.ny);
            for x in x_first..=x_last {
                let cell = &mut self.cells[y * self.nx + x];
                cell.max_order = cell.max_order.max(entry.order);
                if covers_y && axis_covers(rect.x0, rect.x1, x, self.nx) {
                    cell.cover_max = cell.cover_max.max(entry.order);
                } else {
                    let remaining_max = if cell.head == NIL {
                        entry.order
                    } else {
                        entry
                            .order
                            .max(self.partial[cell.head as usize].remaining_max)
                    };
                    let index = self.partial.len() as u32;
                    self.partial.push(PartialNode {
                        rect,
                        order: entry.order,
                        remaining_max,
                        next: cell.head,
                    });
                    cell.head = index;
                }
            }
        }
    }

    /// Maximum order among entries overlapping `query` (0 when none). `all` is every entry of
    /// the frame and serves queries that cannot be answered by cell lookups.
    fn query(&self, query: &Rect, all: &[Entry], global_max: u32) -> u32 {
        if !query.has_area() {
            return all
                .iter()
                .filter(|entry| query.intersects(&entry.rect))
                .map(|entry| entry.order)
                .max()
                .unwrap_or(0);
        }
        let mut best = 0u32;
        for entry in &self.malformed {
            if query.intersects(&entry.rect) {
                best = best.max(entry.order);
            }
        }
        let (x_first, x_last) = axis_range(query.x0, query.x1, self.nx);
        let (y_first, y_last) = axis_range(query.y0, query.y1, self.ny);
        for y in y_first..=y_last {
            for x in x_first..=x_last {
                let cell = &self.cells[y * self.nx + x];
                best = best.max(cell.cover_max);
                if cell.max_order <= best {
                    continue;
                }
                let mut node = cell.head;
                while node != NIL {
                    let node_ref = &self.partial[node as usize];
                    if node_ref.remaining_max <= best {
                        break;
                    }
                    if node_ref.order > best && query.intersects(&node_ref.rect) {
                        best = node_ref.order;
                    }
                    node = node_ref.next;
                }
                if best >= global_max {
                    return best;
                }
            }
        }
        best
    }
}

/// 64 x 64 bit filter over the scene marking where changed regions are.
struct ChangedBits {
    rows: [u64; CHANGED_GRID_SIZE],
    /// Device pixels per bit cell.
    side: f32,
}

impl ChangedBits {
    const fn new() -> Self {
        Self {
            rows: [0; CHANGED_GRID_SIZE],
            side: 32.0,
        }
    }

    fn clear(&mut self, extent: f32) {
        self.rows = [0; CHANGED_GRID_SIZE];
        let mut side = 32.0f32;
        while side * (CHANGED_GRID_SIZE as f32) < extent && side < 1.0e9 {
            side *= 2.0;
        }
        self.side = side;
    }

    /// Inclusive bit-cell ranges, clamped so that the border cells stand for everything
    /// beyond them. `None` when the rect has non-finite or inverted edges.
    fn ranges(&self, rect: &Rect) -> Option<(usize, usize, u64, usize, usize)> {
        if !rect.is_well_formed() {
            return None;
        }
        let cell = |edge: f32| {
            floor_fast(edge / self.side).clamp(0.0, (CHANGED_GRID_SIZE - 1) as f32) as usize
        };
        let (x_first, x_last) = (cell(rect.x0), cell(rect.x1));
        let (y_first, y_last) = (cell(rect.y0), cell(rect.y1));
        let width = x_last - x_first + 1;
        let mask = if width >= 64 {
            u64::MAX
        } else {
            ((1u64 << width) - 1) << x_first
        };
        Some((y_first, y_last, mask, x_first, x_last))
    }

    /// Returns `false` when the rect cannot be represented.
    fn mark(&mut self, rect: &Rect) -> bool {
        let Some((y_first, y_last, mask, ..)) = self.ranges(rect) else {
            return false;
        };
        for row in &mut self.rows[y_first..=y_last] {
            *row |= mask;
        }
        true
    }

    /// `Some(false)` means the rect cannot touch any marked region, `Some(true)` that it
    /// might, `None` that the rect cannot be represented.
    fn may_touch(&self, rect: &Rect) -> Option<bool> {
        let (y_first, y_last, mask, ..) = self.ranges(rect)?;
        Some(
            self.rows[y_first..=y_last]
                .iter()
                .any(|row| row & mask != 0),
        )
    }
}

enum ChangedCheck {
    Clear,
    Hit,
    BudgetExceeded,
}

/// Assigns draw orders for one scene and keeps the previous frame's stream for replay.
pub(crate) struct OrderIndex {
    previous: Vec<Entry>,
    current: Vec<Entry>,
    /// Largest finite lower-right edge (on either axis, at least zero) among `previous`.
    previous_extent: f32,
    /// The same for `current`, kept up to date by [`insert`](Self::insert).
    current_extent: f32,
    max_order: u32,
    changed: Vec<Rect>,
    changed_unmarkable: u32,
    changed_bits: ChangedBits,
    comparisons: u32,
    grid_active: bool,
    grid: Grid,
    stats: SceneOrderStats,
}

impl Default for OrderIndex {
    fn default() -> Self {
        Self {
            previous: Vec::new(),
            current: Vec::new(),
            previous_extent: 0.0,
            current_extent: 0.0,
            max_order: 0,
            changed: Vec::new(),
            changed_unmarkable: 0,
            changed_bits: ChangedBits::new(),
            comparisons: 0,
            grid_active: false,
            grid: Grid::default(),
            stats: SceneOrderStats::default(),
        }
    }
}

impl OrderIndex {
    /// Starts a new frame. The stream of the frame that just ended becomes the replay
    /// baseline (unless [`adopt_baseline`](Self::adopt_baseline) replaces it).
    pub fn begin_frame(&mut self) {
        std::mem::swap(&mut self.previous, &mut self.current);
        self.previous_extent = self.current_extent;
        self.reset_frame_state();
    }

    /// Uses the stream of `previous_frame` as the replay baseline. A scene that alternates
    /// with another one calls this so that the baseline is the immediately preceding frame.
    pub fn adopt_baseline(&mut self, previous_frame: &OrderIndex) {
        self.previous.clear();
        self.previous.extend_from_slice(&previous_frame.current);
        self.previous_extent = previous_frame.current_extent;
        self.reset_frame_state();
    }

    fn reset_frame_state(&mut self) {
        self.current.clear();
        self.current_extent = 0.0;
        self.max_order = 0;
        self.changed.clear();
        self.changed_unmarkable = 0;
        self.changed_bits.clear(self.previous_extent);
        self.comparisons = 0;
        self.grid_active = false;
        self.stats = SceneOrderStats::default();
    }

    pub fn stats(&self) -> SceneOrderStats {
        self.stats
    }

    /// Bounds recorded as changed regions in this frame (old and new bounds).
    #[cfg(test)]
    pub fn changed_regions(&self) -> Vec<Bounds<ScaledPixels>> {
        self.changed
            .iter()
            .map(|rect| Bounds {
                origin: crate::point(ScaledPixels(rect.x0), ScaledPixels(rect.y0)),
                size: crate::size(
                    ScaledPixels(rect.x1 - rect.x0),
                    ScaledPixels(rect.y1 - rect.y0),
                ),
            })
            .collect()
    }

    /// Inserts `bounds` into the stream and returns its order.
    pub fn insert(&mut self, bounds: Bounds<ScaledPixels>) -> u32 {
        let rect = Rect::new(&bounds);
        let position = self.current.len();
        self.stats.entries += 1;

        let order = if self.grid_active {
            self.grid_order(&rect)
        } else if self.comparisons >= REPLAY_COMPARISON_BUDGET {
            self.build_grid();
            self.grid_order(&rect)
        } else {
            self.replay_or_search(position, rect)
        };

        let entry = Entry { rect, order };
        if self.grid_active {
            self.grid.insert(entry);
        }
        self.current.push(entry);
        self.max_order = self.max_order.max(order);
        if rect.x1.is_finite() {
            self.current_extent = self.current_extent.max(rect.x1);
        }
        if rect.y1.is_finite() {
            self.current_extent = self.current_extent.max(rect.y1);
        }
        order
    }

    fn replay_or_search(&mut self, position: usize, rect: Rect) -> u32 {
        let previous = self.previous.get(position).copied();
        let mut search_needed = true;
        if let Some(previous) = previous {
            if previous.rect == rect {
                match self.check_changed(&rect) {
                    ChangedCheck::Clear => {
                        self.stats.replayed_entries += 1;
                        return previous.order;
                    }
                    ChangedCheck::Hit => {}
                    ChangedCheck::BudgetExceeded => {
                        self.build_grid();
                        search_needed = false;
                    }
                }
            }
        }

        self.stats.recomputed_entries += 1;
        let order = if !search_needed {
            self.grid_order(&rect)
        } else {
            match self.linear_search(&rect) {
                Some(max_found) => max_found + 1,
                None => {
                    self.build_grid();
                    self.grid_order(&rect)
                }
            }
        };

        if let Some(previous) = previous {
            if previous.rect != rect || previous.order != order {
                self.mark_changed(previous.rect);
                self.mark_changed(rect);
            }
        }
        order
    }

    fn mark_changed(&mut self, rect: Rect) {
        self.stats.changed_regions += 1;
        if !self.changed_bits.mark(&rect) {
            self.changed_unmarkable += 1;
        }
        self.changed.push(rect);
    }

    /// Does `rect` overlap any changed region recorded so far in this frame?
    fn check_changed(&mut self, rect: &Rect) -> ChangedCheck {
        if self.changed.is_empty() {
            return ChangedCheck::Clear;
        }
        if self.changed_unmarkable == 0 && self.changed_bits.may_touch(rect) == Some(false) {
            return ChangedCheck::Clear;
        }
        for changed in &self.changed {
            if self.comparisons >= REPLAY_COMPARISON_BUDGET {
                return ChangedCheck::BudgetExceeded;
            }
            self.comparisons += 1;
            self.stats.replay_comparisons += 1;
            if rect.intersects(changed) {
                return ChangedCheck::Hit;
            }
        }
        ChangedCheck::Clear
    }

    /// Maximum order among earlier entries overlapping `rect`, scanning linearly. `None`
    /// when the comparison budget ran out.
    fn linear_search(&mut self, rect: &Rect) -> Option<u32> {
        let mut best = 0u32;
        for entry in self.current.iter().rev() {
            if self.comparisons >= REPLAY_COMPARISON_BUDGET {
                return None;
            }
            self.comparisons += 1;
            self.stats.replay_comparisons += 1;
            if entry.order > best && rect.intersects(&entry.rect) {
                best = entry.order;
                if best >= self.max_order {
                    break;
                }
            }
        }
        Some(best)
    }

    fn build_grid(&mut self) {
        let extent = |edge: fn(&Rect) -> f32| {
            self.previous
                .iter()
                .chain(self.current.iter())
                .map(|entry| edge(&entry.rect))
                .filter(|edge| edge.is_finite())
                .fold(0.0f32, f32::max)
        };
        let cells = |extent: f32| {
            (((extent / GRID_CELL_SIZE).ceil() as usize).min(GRID_MAX_CELLS_PER_AXIS) + 2)
                .min(GRID_MAX_CELLS_PER_AXIS)
        };
        let nx = cells(extent(|rect| rect.x1));
        let ny = cells(extent(|rect| rect.y1));
        self.grid.reset(nx, ny);
        for entry in &self.current {
            self.grid.insert(*entry);
        }
        self.grid_active = true;
        self.stats.grid_builds += 1;
    }

    fn grid_order(&mut self, rect: &Rect) -> u32 {
        self.stats.spatial_queries += 1;
        self.grid.query(rect, &self.current, self.max_order) + 1
    }
}

#[cfg(test)]
mod tests;
