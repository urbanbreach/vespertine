//! Keeping the virtual list attached to a path while the visible rows change (CMP-016 R10),
//! and the fade-edge flags (R9).

use super::model::{VisibleRow, parent_path, row_parent};
use std::collections::HashMap;
use std::ops::Range;
use vespertine::{ListOffset, ListState, Pixels, px};

/// The tolerance, in logical pixels, of the overflow flags of the edge fade.
pub const FADE_TOLERANCE: f32 = 0.5;

/// Which edges of a scrolled list have hidden rows: `(top, bottom)` for the scroll `offset`
/// (distance scrolled from the top) and the maximum offset `max`. The top flag is
/// `offset > 0.5`, the bottom flag `offset < max - 0.5`; reading both from the list's own
/// offset at paint time makes a clamped scroll clear them in the same frame.
pub fn fade_flags(offset: Pixels, max: Pixels) -> (bool, bool) {
    (
        f32::from(offset) > FADE_TOLERANCE,
        f32::from(offset) < f32::from(max) - FADE_TOLERANCE,
    )
}

/// The fade flags of `list` read from its current scroll geometry.
pub fn list_fade_flags(list: &ListState) -> (bool, bool) {
    let offset = -list.scroll_px_offset_for_scrollbar().y;
    let max = list.max_offset_for_scrollbar().y;
    fade_flags(offset.max(px(0.)), max)
}

/// The splice that turns `old` into `new`: the range of `old` between the longest common prefix
/// and the longest common suffix, and the number of rows that replace it. `None` for equal
/// sequences.
pub fn splice_plan(old: &[VisibleRow], new: &[VisibleRow]) -> Option<(Range<usize>, usize)> {
    if old == new {
        return None;
    }
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let max_suffix = old.len().min(new.len()) - prefix;
    let suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    Some((prefix..old.len() - suffix, new.len() - prefix - suffix))
}

/// Where the viewport anchor goes when the rows change: the path of the row at the old scroll
/// top (`old_ix`, clamped by the caller) resolves to the new index of that path; else of its
/// nearest surviving ancestor; else of the first surviving row scanning forward from the old
/// index and then backward; else `(0, 0)`. The sub-row offset is kept only when the anchor row
/// itself survives.
pub fn resolve_anchor(
    old: &[VisibleRow],
    new: &[VisibleRow],
    old_ix: usize,
    offset: Pixels,
) -> (usize, Pixels) {
    if new.is_empty() || old.is_empty() {
        return (0, px(0.));
    }
    let positions: HashMap<&str, usize> = new
        .iter()
        .enumerate()
        .map(|(ix, row)| (row.path.as_str(), ix))
        .collect();
    let old_ix = old_ix.min(old.len() - 1);
    let anchor = old[old_ix].path.as_str();
    if let Some(&ix) = positions.get(anchor) {
        return (ix, offset);
    }
    let mut ancestor = row_parent(anchor);
    while !ancestor.is_empty() {
        if let Some(&ix) = positions.get(ancestor) {
            return (ix, px(0.));
        }
        ancestor = parent_path(ancestor);
    }
    let forward = old[old_ix + 1..].iter();
    let backward = old[..old_ix].iter().rev();
    for row in forward.chain(backward) {
        if let Some(&ix) = positions.get(row.path.as_str()) {
            return (ix, px(0.));
        }
    }
    (0, px(0.))
}

/// Updates `list` from the rows `old` to the rows `new` (R10): splices only the range between
/// the common prefix and suffix and restores the viewport onto the same path. An empty `new`
/// sets the count to zero and the scroll top to 0.
pub fn sync_list_rows(list: &ListState, old: &[VisibleRow], new: &[VisibleRow]) {
    let Some((range, count)) = splice_plan(old, new) else {
        return;
    };
    let top = list.logical_scroll_top();
    list.splice(range, count);
    let (item_ix, offset_in_item) = resolve_anchor(old, new, top.item_ix, top.offset_in_item);
    list.scroll_to(ListOffset {
        item_ix,
        offset_in_item,
    });
}
