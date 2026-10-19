//! Columns, widths, resize and reorder math of the data table (CMP-019 R1, R3, R4, R9).
//!
//! Everything here is pure geometry on plain numbers so it can be tested without a window.

use std::collections::BTreeMap;

/// The default minimum width of the primary (remainder-absorbing) column.
pub const PRIMARY_MIN_WIDTH: f32 = 80.0;
/// The width of the resize hit area, centered on the divider.
pub const HANDLE_WIDTH: f32 = 6.0;
/// The width of the reorder insertion indicator.
pub const INDICATOR_WIDTH: f32 = 2.0;
/// How far the insertion indicator is inset from the top and bottom of the header.
pub const INDICATOR_INSET: f32 = 3.0;
/// The default horizontal cell padding.
pub const CELL_PADDING_X: f32 = 12.0;
/// The default vertical cell padding.
pub const CELL_PADDING_Y: f32 = 8.0;

/// Where a cell's content sits inside the cell.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    /// At the start (the default).
    #[default]
    Start,
    /// Centered.
    Center,
    /// At the end.
    End,
}

/// The alignment of a cell: the column's explicit alignment, or end alignment for a plain
/// number in a column whose alignment was left at its default.
pub fn resolve_align(column_align: Option<Align>, numeric_cell: bool) -> Align {
    match column_align {
        Some(align) => align,
        None if numeric_cell => Align::End,
        None => Align::Start,
    }
}

/// How a column gets its width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WidthMode {
    /// A pixel width (`flex: none`).
    Fixed(f32),
    /// Basis 0, sharing the remaining width equally with the other flexible columns.
    Flex,
}

/// A column descriptor (R1).
#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    /// The stable column id.
    pub id: String,
    /// The header label; may be empty.
    pub label: String,
    /// The explicit alignment; `None` is the default (numbers align to the end).
    pub align: Option<Align>,
    /// The width mode.
    pub width: WidthMode,
    /// The minimum width, if any.
    pub min_width: Option<f32>,
    /// The maximum width, if any.
    pub max_width: Option<f32>,
    /// Whether the header offers a resize handle next to this column.
    pub resizable: bool,
    /// Whether the header cell can be dragged to reorder.
    pub reorderable: bool,
    /// Whether the column is shown.
    pub visible: bool,
}

impl Column {
    /// A flexible, resizable, reorderable, visible column.
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            align: None,
            width: WidthMode::Flex,
            min_width: None,
            max_width: None,
            resizable: true,
            reorderable: true,
            visible: true,
        }
    }

    /// Sets the alignment.
    pub fn align(mut self, align: Align) -> Self {
        self.align = Some(align);
        self
    }

    /// A fixed pixel width (`flex: none`).
    pub fn width(mut self, width: f32) -> Self {
        self.width = WidthMode::Fixed(width);
        self
    }

    /// A flexible width.
    pub fn flex(mut self) -> Self {
        self.width = WidthMode::Flex;
        self
    }

    /// Sets the minimum width.
    pub fn min_width(mut self, min: f32) -> Self {
        self.min_width = Some(min);
        self
    }

    /// Sets the maximum width.
    pub fn max_width(mut self, max: f32) -> Self {
        self.max_width = Some(max);
        self
    }

    /// Sets whether the column can be resized.
    pub fn resizable(mut self, resizable: bool) -> Self {
        self.resizable = resizable;
        self
    }

    /// Sets whether the column can be reordered.
    pub fn reorderable(mut self, reorderable: bool) -> Self {
        self.reorderable = reorderable;
        self
    }

    /// Sets whether the column is shown.
    pub fn visible(mut self, visible: bool) -> Self {
        self.visible = visible;
        self
    }

    /// The minimum width: the descriptor's, else 80 px for the primary column, else 0.
    pub fn effective_min(&self, primary: bool) -> f32 {
        self.min_width
            .unwrap_or(if primary { PRIMARY_MIN_WIDTH } else { 0.0 })
    }

    /// The maximum width: the descriptor's, else unbounded (the primary column has none).
    pub fn effective_max(&self, primary: bool) -> f32 {
        if primary {
            f32::INFINITY
        } else {
            self.max_width.unwrap_or(f32::INFINITY)
        }
    }
}

/// Whether a header row is drawn: at least one visible column has a non-empty label.
pub fn header_visible(columns: &[Column]) -> bool {
    columns
        .iter()
        .any(|column| column.visible && !column.label.is_empty())
}

/// The index of the primary column among `columns`: the first flexible one.
pub fn primary_index(columns: &[&Column]) -> Option<usize> {
    columns
        .iter()
        .position(|column| column.width == WidthMode::Flex)
}

/// The natural widths of the visible `columns` in a container `container` pixels wide: fixed
/// columns (or the width stored in `widths` by a resize) keep their width clamped to their
/// bounds; flexible columns share the remainder equally, honoring minimum and maximum widths.
/// The sum may exceed `container` when minimum widths cannot be honored.
pub fn natural_widths(
    columns: &[&Column],
    widths: &BTreeMap<String, f32>,
    container: f32,
) -> Vec<f32> {
    let primary = primary_index(columns);
    let mut result = vec![0.0f32; columns.len()];
    let mut flexible: Vec<usize> = Vec::new();
    let mut fixed_sum = 0.0;
    for (index, column) in columns.iter().enumerate() {
        let is_primary = primary == Some(index);
        let explicit = widths.get(&column.id).copied().or(match column.width {
            WidthMode::Fixed(width) => Some(width),
            WidthMode::Flex => None,
        });
        match explicit {
            Some(width) => {
                let width = width
                    .max(column.effective_min(is_primary))
                    .min(column.effective_max(is_primary));
                result[index] = width;
                fixed_sum += width;
            }
            None => flexible.push(index),
        }
    }
    let mut remaining = (container - fixed_sum).max(0.0);
    let mut open = flexible;
    loop {
        if open.is_empty() {
            break;
        }
        let share = remaining / open.len() as f32;
        let violation = open.iter().copied().find(|&index| {
            let is_primary = primary == Some(index);
            let column = columns[index];
            share < column.effective_min(is_primary) || share > column.effective_max(is_primary)
        });
        match violation {
            Some(index) => {
                let is_primary = primary == Some(index);
                let column = columns[index];
                let clamped = share
                    .max(column.effective_min(is_primary))
                    .min(column.effective_max(is_primary));
                result[index] = clamped;
                remaining = (remaining - clamped).max(0.0);
                open.retain(|candidate| *candidate != index);
            }
            None => {
                for index in open.drain(..) {
                    result[index] = share;
                }
            }
        }
    }
    result
}

/// One side of a resize handle: the width at press time and its bounds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResizeSide {
    /// The width when the handle was pressed.
    pub start: f32,
    /// The minimum width.
    pub min: f32,
    /// The maximum width.
    pub max: f32,
}

/// The widths of the two columns around a handle after a drag of `delta` pixels (R3): the pair
/// keeps its total width; `delta` is clamped to
/// `[max(left_min - left_start, right_start - right_max), min(left_max - left_start, right_start - right_min)]`.
pub fn resize_pair(left: ResizeSide, right: ResizeSide, delta: f32) -> (f32, f32) {
    let low = (left.min - left.start).max(right.start - right.max);
    let high = (left.max - left.start).min(right.start - right.min);
    let delta = if low > high {
        0.0
    } else {
        delta.clamp(low, high)
    };
    (left.start + delta, right.start - delta)
}

/// The width of the right column when the left neighbor is the flexible primary column: only
/// the right column changes, `clamp(right_start - delta, right_min, right_max)`.
pub fn resize_after_primary(right: ResizeSide, delta: f32) -> f32 {
    (right.start - delta).clamp(right.min, right.max.max(right.min))
}

/// A resize drag in progress: the pair of columns around the pressed handle.
#[derive(Clone, Debug, PartialEq)]
pub struct ResizeDrag {
    /// The id of the column left of the handle.
    pub left: String,
    /// The id of the column right of the handle.
    pub right: String,
    /// The pointer x at press time.
    pub press_x: f32,
    left_side: ResizeSide,
    right_side: ResizeSide,
    left_primary: bool,
}

impl ResizeDrag {
    /// Starts a drag at `press_x` over the pair with its starting widths and bounds.
    pub fn new(
        left: &str,
        right: &str,
        press_x: f32,
        left_side: ResizeSide,
        right_side: ResizeSide,
        left_primary: bool,
    ) -> Self {
        Self {
            left: left.to_owned(),
            right: right.to_owned(),
            press_x,
            left_side,
            right_side,
            left_primary,
        }
    }

    /// The widths to store for pointer position `pointer_x`: only the right column when the
    /// left neighbor is the primary column, otherwise both.
    pub fn apply(&self, pointer_x: f32) -> Vec<(String, f32)> {
        let delta = pointer_x - self.press_x;
        if self.left_primary {
            vec![(
                self.right.clone(),
                resize_after_primary(self.right_side, delta),
            )]
        } else {
            let (left, right) = resize_pair(self.left_side, self.right_side, delta);
            vec![(self.left.clone(), left), (self.right.clone(), right)]
        }
    }
}

/// The divider positions of the visible columns: for every adjacent pair whose columns are both
/// resizable, `(index of the left column, x of the divider)` where `x` is the sum of the widths
/// up to and including the left column.
pub fn handle_positions(widths: &[f32], resizable: &[bool]) -> Vec<(usize, f32)> {
    let mut positions = Vec::new();
    let mut x = 0.0;
    for index in 0..widths.len().saturating_sub(1) {
        x += widths[index];
        if resizable[index] && resizable[index + 1] {
            positions.push((index, x));
        }
    }
    positions
}

/// The drop index among the visible columns for a pointer at `x` relative to the header's left
/// edge (R4): `x' = clamp(x, 0, rendered) x desired / rendered` with `desired` the sum of the
/// columns' preferred widths; the first column whose `start + width / 2` exceeds `x'`, else the
/// last.
pub fn drop_index(x: f32, rendered_width: f32, preferred: &[f32]) -> usize {
    if preferred.is_empty() {
        return 0;
    }
    let desired: f32 = preferred.iter().sum();
    let mapped = if rendered_width > 0.0 {
        x.clamp(0.0, rendered_width) * desired / rendered_width
    } else {
        0.0
    };
    let mut start = 0.0;
    for (index, width) in preferred.iter().enumerate() {
        if start + width / 2.0 > mapped {
            return index;
        }
        start += width;
    }
    preferred.len() - 1
}

/// The new column order after dragging `dragged` over `over` (R4): the dragged column is
/// removed, then reinserted at the position of the target after removal, plus one when it moved
/// later. `None` (no change, no event) when they are equal or either id is unknown.
pub fn reorder(order: &[String], dragged: &str, over: &str) -> Option<Vec<String>> {
    if dragged == over {
        return None;
    }
    let from = order.iter().position(|id| id == dragged)?;
    let to = order.iter().position(|id| id == over)?;
    let mut next: Vec<String> = order.to_vec();
    let moved = next.remove(from);
    let target = next.iter().position(|id| id == over)?;
    let at = if from < to { target + 1 } else { target };
    next.insert(at, moved);
    Some(next)
}

/// Which edge of the target column the insertion indicator sits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndicatorEdge {
    /// The leading edge (moving the dragged column earlier).
    Leading,
    /// The trailing edge (moving it later).
    Trailing,
}

/// The insertion indicator while `dragged` hovers over the visible column at `over_index`:
/// the leading edge of the target when moving earlier, the trailing edge when moving later,
/// nothing when the target is the dragged column.
pub fn indicator(
    visible_ids: &[&str],
    dragged: &str,
    over_index: usize,
) -> Option<(usize, IndicatorEdge)> {
    let from = visible_ids.iter().position(|id| *id == dragged)?;
    match over_index.cmp(&from) {
        std::cmp::Ordering::Less => Some((over_index, IndicatorEdge::Leading)),
        std::cmp::Ordering::Greater => Some((over_index, IndicatorEdge::Trailing)),
        std::cmp::Ordering::Equal => None,
    }
}
