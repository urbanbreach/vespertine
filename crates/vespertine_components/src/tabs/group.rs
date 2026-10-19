//! Reorderable groups and pinned lists (CMP-017 R13): the identity and insertion-index concepts
//! of the tab strip for vertical lists, with uniform and variable-height slots, a lifted item
//! that follows the pointer, sibling sliding, edge autoscroll and a settle glide.
//!
//! [`ReorderableGroup`] is a pure state machine: callers pass pointer positions in content
//! coordinates (relative to the first slot's top edge, scroll offset folded in) and the motion
//! time in milliseconds, and read back the lifted item's top and every sibling's displacement
//! to paint.

use super::model::{SLIDE_MS, Slide, SlideSet, drop_index, ease_out, reorder, slide_offset};

/// Width of the autoscroll band at the top and bottom edge of the viewport.
pub const AUTOSCROLL_BAND: f32 = 48.0;
/// The largest autoscroll step, per frame.
pub const AUTOSCROLL_MAX_STEP: f32 = 12.0;
/// The frame period the autoscroll step is defined for.
pub const DRAG_FRAME_MS: f64 = 16.0;

/// The drop index of a lifted section of variable height (sections of a sidebar).
///
/// `top` is the lifted item's top measured from the first slot, `heights` the natural heights of
/// all items in order, `from` the lifted item's index. Slot tops are natural (the thresholds
/// stay still while siblings slide); `over` is the number of siblings whose midpoint the
/// lifted item's leading edge has crossed: for a sibling before `from`, `top` is past its
/// midpoint; for one after, the lifted bottom edge is.
pub fn section_drop_index(top: f32, heights: &[f32], from: usize, gap: f32) -> usize {
    let Some(&own) = heights.get(from) else {
        return 0;
    };
    let bottom = top + own;
    let mut slot_top = 0.0;
    let mut over = 0;
    for (ix, height) in heights.iter().enumerate() {
        if ix != from {
            let midpoint = slot_top + height / 2.0;
            let crossed = if ix < from {
                top > midpoint
            } else {
                bottom > midpoint
            };
            over += usize::from(crossed);
        }
        slot_top += height + gap;
    }
    over
}

/// The row under `y` for lists whose rows differ in height: the first row whose cumulative
/// bottom (heights plus gaps) exceeds `y`. In strict mode (`clamp == false`) a `y` outside
/// `[0, total height]` yields `None`; in clamp mode the first or last row.
pub fn row_drop_index(y: f32, heights: &[f32], gap: f32, clamp: bool) -> Option<usize> {
    if heights.is_empty() || y.is_nan() {
        return None;
    }
    let total: f32 = heights.iter().sum::<f32>() + gap * (heights.len() - 1) as f32;
    if !clamp && (y < 0.0 || y > total) {
        return None;
    }
    let mut bottom = 0.0;
    for (ix, height) in heights.iter().enumerate() {
        bottom += height + gap;
        if bottom > y {
            return Some(ix);
        }
    }
    Some(heights.len() - 1)
}

/// The full ordered pin list after moving, within the `visible` subset, the entry at `from` to
/// the visible position `to`. Hidden or archived entries keep their relative order: the moved
/// id is removed from `all`, the target visible id's position in the remainder is found, and
/// the moved id is inserted after it when moving down (`from < to`) or at it when moving up.
/// Out-of-range or equal indices return `all` unchanged.
pub fn reorder_visible_pins<T: Clone + PartialEq>(
    all: &[T],
    visible: &[T],
    from: usize,
    to: usize,
) -> Vec<T> {
    if from == to || from >= visible.len() || to >= visible.len() {
        return all.to_vec();
    }
    let moved = &visible[from];
    let target = &visible[to];
    let mut order: Vec<T> = all.iter().filter(|id| *id != moved).cloned().collect();
    let at = match order.iter().position(|id| id == target) {
        Some(position) if from < to => position + 1,
        Some(position) => position,
        None => order.len(),
    };
    order.insert(at, moved.clone());
    order
}

/// Pins whose identity the workspace no longer knows are dropped without disturbing the order
/// of the rest.
pub fn drop_unknown<T>(order: Vec<T>, known: impl Fn(&T) -> bool) -> Vec<T> {
    order.into_iter().filter(|id| known(id)).collect()
}

/// The autoscroll step of a drag with the pointer at vertical position `pointer` in a viewport
/// spanning `top..bottom`: within the 48 px band of an edge it scrolls up to 12 px per 16 ms
/// frame times the penetration `clamp(distance into the band / 48, 0, 1)`, negative upward.
/// A zero-height (or inverted) viewport scrolls nothing.
pub fn group_drag_scroll_delta(pointer: f32, top: f32, bottom: f32) -> f32 {
    if bottom <= top || bottom.is_nan() || top.is_nan() || pointer.is_nan() {
        return 0.0;
    }
    let up = ((top + AUTOSCROLL_BAND - pointer) / AUTOSCROLL_BAND).clamp(0.0, 1.0);
    let down = ((pointer - (bottom - AUTOSCROLL_BAND)) / AUTOSCROLL_BAND).clamp(0.0, 1.0);
    if up > down {
        -AUTOSCROLL_MAX_STEP * up
    } else {
        AUTOSCROLL_MAX_STEP * down
    }
}

/// The slot geometry of a group.
#[derive(Clone, Debug, PartialEq)]
pub enum GroupLayout {
    /// Every item has the same height.
    Uniform {
        /// Height of an item.
        item_height: f32,
        /// Gap between items.
        gap: f32,
    },
    /// Items of different heights (sections).
    Variable {
        /// Natural height of each item, in order.
        heights: Vec<f32>,
        /// Gap between items.
        gap: f32,
    },
}

impl GroupLayout {
    fn gap(&self) -> f32 {
        match self {
            Self::Uniform { gap, .. } | Self::Variable { gap, .. } => *gap,
        }
    }

    fn height(&self, ix: usize) -> f32 {
        match self {
            Self::Uniform { item_height, .. } => *item_height,
            Self::Variable { heights, .. } => heights.get(ix).copied().unwrap_or(0.0),
        }
    }

    /// The natural top of slot `ix`, measured from the first slot, for `count` items.
    pub fn slot_top(&self, ix: usize) -> f32 {
        match self {
            Self::Uniform { item_height, gap } => ix as f32 * (item_height + gap),
            Self::Variable { heights, gap } => {
                heights.iter().take(ix).map(|height| height + gap).sum()
            }
        }
    }

    /// The height of all items with their gaps.
    pub fn total_height(&self, count: usize) -> f32 {
        if count == 0 {
            return 0.0;
        }
        (0..count).map(|ix| self.height(ix)).sum::<f32>() + self.gap() * (count - 1) as f32
    }

    /// The same layout after moving the item at `from` to `to`.
    fn reordered(&self, from: usize, to: usize) -> Self {
        match self {
            Self::Uniform { .. } => self.clone(),
            Self::Variable { heights, gap } => {
                let mut heights = heights.clone();
                reorder(&mut heights, from, to);
                Self::Variable { heights, gap: *gap }
            }
        }
    }
}

/// The ordered move a committed drag produces, with the neighbours at the destination.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Move<K> {
    /// The moved item.
    pub id: K,
    /// The item that precedes it at the destination.
    pub after: Option<K>,
    /// The item that follows it at the destination.
    pub before: Option<K>,
}

#[derive(Clone, Debug)]
struct Lift<K> {
    ids: Vec<K>,
    layout: GroupLayout,
    source: usize,
    grab_offset: f32,
    top: f32,
    over: usize,
    slides: SlideSet,
}

impl<K> Lift<K> {
    /// The lifted top for a pointer position: `pointer - grab_offset` held within the list.
    fn held_top(&self, pointer: f32) -> f32 {
        let own = self.layout.height(self.source);
        let span = (self.layout.total_height(self.ids.len()) - own).max(0.0);
        (pointer - self.grab_offset).clamp(0.0, span)
    }
}

#[derive(Clone, Debug)]
struct Settle {
    start_ms: f64,
    /// Residual of the lifted item (painted top minus resting top) at the start.
    lifted_from: f32,
    lifted_index: usize,
    siblings: Vec<Slide>,
}

/// A reorderable vertical group: lift, over, commit, cancel and the settle glide.
///
/// A drag is valid only while the list identity (the ordered ids) is unchanged since it began.
#[derive(Clone, Debug)]
pub struct ReorderableGroup<K> {
    lift: Option<Lift<K>>,
    settle: Option<Settle>,
}

impl<K> Default for ReorderableGroup<K> {
    fn default() -> Self {
        Self {
            lift: None,
            settle: None,
        }
    }
}

impl<K: Clone + PartialEq> ReorderableGroup<K> {
    /// An idle group.
    pub fn new() -> Self {
        Self::default()
    }

    /// Lifts the item at `source` of `ids`. `pointer` is the pointer's position and
    /// `grab_offset` the distance between the pointer and the item's top edge, both measured
    /// from the first slot. Returns whether a drag began (`source` in range).
    pub fn lift(
        &mut self,
        ids: &[K],
        layout: GroupLayout,
        source: usize,
        grab_offset: f32,
        pointer: f32,
    ) -> bool {
        if source >= ids.len() {
            return false;
        }
        self.settle = None;
        let mut lift = Lift {
            ids: ids.to_vec(),
            layout,
            source,
            grab_offset,
            top: 0.0,
            over: source,
            slides: SlideSet::new(ids.len()),
        };
        lift.top = lift.held_top(pointer);
        self.lift = Some(lift);
        true
    }

    /// Whether an item is lifted.
    pub fn is_lifted(&self) -> bool {
        self.lift.is_some()
    }

    /// The lifted item's index in the order at lift time.
    pub fn source(&self) -> Option<usize> {
        self.lift.as_ref().map(|lift| lift.source)
    }

    /// The index the lifted item would land on.
    pub fn over(&self) -> Option<usize> {
        self.lift.as_ref().map(|lift| lift.over)
    }

    /// The restart counter of the sibling slides.
    pub fn epoch(&self) -> u64 {
        self.lift.as_ref().map_or(0, |lift| lift.slides.epoch())
    }

    /// Whether the lifted ids still match `ids`; a drag whose list identity changed must be
    /// cancelled.
    pub fn is_valid_for(&self, ids: &[K]) -> bool {
        self.lift.as_ref().is_some_and(|lift| lift.ids == ids)
    }

    /// The pointer moved (or the scroll offset changed, which moves the pointer in content
    /// coordinates): repaints the lifted item at `pointer - grab_offset` held within the list
    /// and derives the drop index again, retargeting the sibling slides when it changes.
    pub fn pointer_moved(&mut self, pointer: f32, now_ms: f64, reduced_motion: bool) {
        let Some(lift) = &mut self.lift else {
            return;
        };
        lift.top = lift.held_top(pointer);
        let over = match &lift.layout {
            GroupLayout::Uniform { item_height, gap } => {
                drop_index(pointer, item_height + gap, lift.ids.len())
            }
            GroupLayout::Variable { heights, gap } => {
                section_drop_index(lift.top, heights, lift.source, *gap)
            }
        };
        let changed = over != lift.over;
        lift.over = over;
        if changed {
            lift.slides
                .retarget_for_drag(lift.source, over, now_ms, reduced_motion);
        }
    }

    /// The autoscroll step for the pointer at `(x, y)` in window coordinates over a viewport:
    /// zero unless the pointer is horizontally inside it. The caller scrolls by the returned
    /// amount and then calls [`Self::pointer_moved`] with the pointer in the new content
    /// coordinates, so the drop index follows the scroll offset.
    pub fn autoscroll_step(
        &self,
        x: f32,
        y: f32,
        viewport_left: f32,
        viewport_right: f32,
        viewport_top: f32,
        viewport_bottom: f32,
    ) -> f32 {
        if self.lift.is_none() || x < viewport_left || x > viewport_right {
            return 0.0;
        }
        group_drag_scroll_delta(y, viewport_top, viewport_bottom)
    }

    /// The lifted item's top, measured from the first slot (while lifted).
    pub fn lifted_top(&self) -> Option<f32> {
        self.lift.as_ref().map(|lift| lift.top)
    }

    /// The displacement of the sibling at `ix` (index at lift time) in pixels:
    /// `slot_offset(ix, from, over) x (lifted height + gap)`, animated.
    pub fn sibling_offset(&self, ix: usize, now_ms: f64) -> f32 {
        let Some(lift) = &self.lift else {
            return 0.0;
        };
        lift.slides.value(ix, now_ms) * (lift.layout.height(lift.source) + lift.layout.gap())
    }

    /// Whether anything is still moving: slides while lifted or the settle glide.
    pub fn is_animating(&self, now_ms: f64) -> bool {
        self.lift
            .as_ref()
            .is_some_and(|lift| lift.slides.is_animating(now_ms))
            || self
                .settle
                .as_ref()
                .is_some_and(|settle| now_ms - settle.start_ms < SLIDE_MS)
    }

    /// Ends the drag with the pointer released over the list. When `current_ids` differ from
    /// the ids at lift time the drag is cancelled; dropping on the source slot also cancels.
    /// Otherwise the move is returned and the settle glide starts: the placed item glides from
    /// its last painted position to the landing slot of the **new** order and every sibling
    /// from its current displacement to its resting place.
    pub fn commit(
        &mut self,
        current_ids: &[K],
        now_ms: f64,
        reduced_motion: bool,
    ) -> Option<Move<K>> {
        if !self.is_valid_for(current_ids) {
            self.cancel(now_ms, reduced_motion);
            return None;
        }
        let lift = self.lift.take()?;
        if lift.over == lift.source {
            self.lift = Some(lift);
            self.cancel(now_ms, reduced_motion);
            return None;
        }
        let unit = lift.layout.height(lift.source) + lift.layout.gap();
        let new_layout = lift.layout.reordered(lift.source, lift.over);
        let landing = new_layout.slot_top(lift.over);
        // Siblings keep their painted place; after the reorder their resting place has moved
        // by exactly their target displacement, so the residual is current minus target.
        let mut siblings = vec![Slide::at(0.0); lift.ids.len()];
        let mut new_ids = lift.ids.clone();
        reorder(&mut new_ids, lift.source, lift.over);
        for ix in 0..lift.ids.len() {
            if ix == lift.source {
                continue;
            }
            let new_ix = new_index(ix, lift.source, lift.over);
            let residual = (lift.slides.value(ix, now_ms)
                - slide_offset(ix, lift.source, lift.over) as f32)
                * unit;
            let mut slide = Slide::at(residual);
            slide.retarget(0.0, now_ms, reduced_motion);
            siblings[new_ix] = slide;
        }
        self.settle = Some(Settle {
            start_ms: now_ms,
            lifted_from: if reduced_motion {
                0.0
            } else {
                lift.top - landing
            },
            lifted_index: lift.over,
            siblings,
        });
        let after = lift.over.checked_sub(1).map(|ix| new_ids[ix].clone());
        let before = new_ids.get(lift.over + 1).cloned();
        Some(Move {
            id: lift.ids[lift.source].clone(),
            after,
            before,
        })
    }

    /// Cancels the drag (Escape, a release outside the list, a changed list identity): the
    /// lifted item glides home and every sibling glides back.
    pub fn cancel(&mut self, now_ms: f64, reduced_motion: bool) {
        let Some(lift) = self.lift.take() else {
            return;
        };
        let unit = lift.layout.height(lift.source) + lift.layout.gap();
        let home = lift.layout.slot_top(lift.source);
        let siblings = (0..lift.ids.len())
            .map(|ix| {
                let mut slide = Slide::at(lift.slides.value(ix, now_ms) * unit);
                slide.retarget(0.0, now_ms, reduced_motion);
                slide
            })
            .collect();
        self.settle = Some(Settle {
            start_ms: now_ms,
            lifted_from: if reduced_motion { 0.0 } else { lift.top - home },
            lifted_index: lift.source,
            siblings,
        });
    }

    /// The settle residuals at `now_ms`: the placed (or returning) item's index and its offset
    /// from its resting slot, if a settle glide is running.
    pub fn settling_item(&self, now_ms: f64) -> Option<(usize, f32)> {
        let settle = self.settle.as_ref()?;
        let t = ((now_ms - settle.start_ms) / SLIDE_MS).clamp(0.0, 1.0) as f32;
        (t < 1.0).then(|| {
            (
                settle.lifted_index,
                settle.lifted_from * (1.0 - ease_out(t)),
            )
        })
    }

    /// The residual displacement of the sibling at `ix` (index in the order after the drag)
    /// during the settle glide, in pixels.
    pub fn settling_sibling(&self, ix: usize, now_ms: f64) -> f32 {
        self.settle
            .as_ref()
            .and_then(|settle| settle.siblings.get(ix))
            .map_or(0.0, |slide| slide.value(now_ms))
    }

    /// Whether a settle glide is still running at `now_ms`.
    pub fn is_settling(&self, now_ms: f64) -> bool {
        self.settle
            .as_ref()
            .is_some_and(|settle| now_ms - settle.start_ms < SLIDE_MS)
    }

    /// Drops a finished settle glide.
    pub fn finish_settle(&mut self, now_ms: f64) {
        if !self.is_settling(now_ms) {
            self.settle = None;
        }
    }
}

/// The index of the item formerly at `ix` after the item at `from` moved to `to`.
fn new_index(ix: usize, from: usize, to: usize) -> usize {
    if from < to && from < ix && ix <= to {
        ix - 1
    } else if to < from && to <= ix && ix < from {
        ix + 1
    } else {
        ix
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEIGHTS: [f32; 3] = [50.0, 300.0, 50.0];

    #[test]
    fn section_drop_index_table() {
        // Heights [50, 300, 50], gap 2: slot tops 0, 52, 354.
        let over = |top: f32, from: usize| section_drop_index(top, &HEIGHTS, from, 2.0);
        assert_eq!(over(0.0, 0), 0);
        assert_eq!(over(52.0, 1), 1);
        assert_eq!(over(99.0, 0), 0);
        assert_eq!(over(142.0, 0), 0);
        assert_eq!(over(162.0, 0), 1);
        assert_eq!(over(400.0, 0), 2);
        assert_eq!(over(254.0, 2), 2);
        assert_eq!(over(150.0, 2), 1);
        assert_eq!(over(-10.0, 2), 0);
        assert_eq!(section_drop_index(0.0, &[], 0, 2.0), 0);
    }

    #[test]
    fn row_drop_index_uses_cumulative_bottoms() {
        let heights = [20.0, 40.0, 20.0];
        // Bottoms with a 4 px gap: 24, 68, 92; total height 92 - 4 = 88.
        assert_eq!(row_drop_index(0.0, &heights, 4.0, false), Some(0));
        assert_eq!(row_drop_index(23.9, &heights, 4.0, false), Some(0));
        assert_eq!(row_drop_index(24.0, &heights, 4.0, false), Some(1));
        assert_eq!(row_drop_index(67.0, &heights, 4.0, false), Some(1));
        assert_eq!(row_drop_index(70.0, &heights, 4.0, false), Some(2));
        assert_eq!(row_drop_index(88.0, &heights, 4.0, false), Some(2));
        // Strict mode: outside [0, total] yields none; clamp mode the last row.
        assert_eq!(row_drop_index(-1.0, &heights, 4.0, false), None);
        assert_eq!(row_drop_index(89.0, &heights, 4.0, false), None);
        assert_eq!(row_drop_index(500.0, &heights, 4.0, true), Some(2));
        assert_eq!(row_drop_index(-5.0, &heights, 4.0, true), Some(0));
        assert_eq!(row_drop_index(5.0, &[], 4.0, true), None);
    }

    #[test]
    fn pins_with_hidden_entries_keep_their_relative_order() {
        let all = ["a", "b", "c", "d", "e"];
        let visible = ["a", "c", "e"];
        // Moving c up to a: it lands before a.
        assert_eq!(
            reorder_visible_pins(&all, &visible, 1, 0),
            ["c", "a", "b", "d", "e"]
        );
        // Moving a down to e: it lands after e.
        assert_eq!(
            reorder_visible_pins(&all, &visible, 0, 2),
            ["b", "c", "d", "e", "a"]
        );
        // Equal or out-of-range indices return the original order.
        assert_eq!(reorder_visible_pins(&all, &visible, 1, 1), all);
        assert_eq!(reorder_visible_pins(&all, &visible, 0, 3), all);
        assert_eq!(reorder_visible_pins(&all, &visible, 7, 0), all);
        // Unknown entries are dropped without disturbing the rest.
        let order = reorder_visible_pins(&["a", "ghost", "b", "c"], &["a", "b", "c"], 2, 0);
        assert_eq!(order, ["c", "a", "ghost", "b"]);
        assert_eq!(drop_unknown(order, |id| *id != "ghost"), ["c", "a", "b"]);
    }

    #[test]
    fn autoscroll_delta_scales_with_penetration() {
        assert_eq!(group_drag_scroll_delta(100.0, 100.0, 500.0), -12.0);
        assert_eq!(group_drag_scroll_delta(60.0, 100.0, 500.0), -12.0);
        assert_eq!(group_drag_scroll_delta(124.0, 100.0, 500.0), -6.0);
        assert_eq!(group_drag_scroll_delta(300.0, 100.0, 500.0), 0.0);
        assert_eq!(group_drag_scroll_delta(476.0, 100.0, 500.0), 6.0);
        assert_eq!(group_drag_scroll_delta(500.0, 100.0, 500.0), 12.0);
        assert_eq!(group_drag_scroll_delta(900.0, 100.0, 500.0), 12.0);
        // A zero-height viewport scrolls nothing.
        assert_eq!(group_drag_scroll_delta(100.0, 100.0, 100.0), 0.0);
    }

    fn ids() -> Vec<&'static str> {
        vec!["a", "b", "c", "d"]
    }

    fn uniform() -> GroupLayout {
        GroupLayout::Uniform {
            item_height: 20.0,
            gap: 4.0,
        }
    }

    #[test]
    fn lifted_item_follows_the_pointer_within_the_list() {
        let mut group = ReorderableGroup::new();
        assert!(group.lift(&ids(), uniform(), 1, 8.0, 32.0));
        // Pointer 32, grab offset 8 -> top 24 (the item's own slot top).
        assert_eq!(group.lifted_top(), Some(24.0));
        group.pointer_moved(-50.0, 0.0, false);
        assert_eq!(group.lifted_top(), Some(0.0));
        // Held at first slot top + total height - own height = 20*4 + 12 - 20 = 72.
        group.pointer_moved(900.0, 0.0, false);
        assert_eq!(group.lifted_top(), Some(72.0));
        assert!(!group.lift(&ids(), uniform(), 9, 0.0, 0.0));
    }

    #[test]
    fn uniform_over_slides_siblings_and_retargets() {
        let mut group = ReorderableGroup::new();
        group.lift(&ids(), uniform(), 0, 4.0, 4.0);
        assert_eq!(group.over(), Some(0));
        group.pointer_moved(52.0, 1000.0, false);
        assert_eq!(group.over(), Some(2));
        // Slots are 24 px: siblings 1 and 2 slide one lifted height + gap upward, eased.
        assert_eq!(group.sibling_offset(1, 1000.0), 0.0);
        assert_eq!(group.sibling_offset(1, 1150.0), -24.0);
        assert_eq!(group.sibling_offset(3, 1150.0), 0.0);
        let mid = group.sibling_offset(1, 1075.0);
        let epoch = group.epoch();
        // Reversing mid-flight continues from the displaced position.
        group.pointer_moved(4.0, 1075.0, false);
        assert!(group.epoch() > epoch);
        assert!((group.sibling_offset(1, 1075.0) - mid).abs() < 1e-4);
        assert_eq!(group.sibling_offset(1, 1225.0), 0.0);
        assert!(group.is_animating(1100.0));
    }

    #[test]
    fn commit_sends_the_move_with_its_destination_neighbours() {
        let mut group = ReorderableGroup::new();
        group.lift(&ids(), uniform(), 0, 4.0, 4.0);
        group.pointer_moved(52.0, 0.0, false);
        let moved = group.commit(&ids(), 100.0, false).expect("a move");
        // New order: b c a d.
        assert_eq!(
            moved,
            Move {
                id: "a",
                after: Some("c"),
                before: Some("d")
            }
        );
        assert!(!group.is_lifted());
    }

    #[test]
    fn dropping_at_the_source_slot_cancels() {
        let mut group = ReorderableGroup::new();
        group.lift(&ids(), uniform(), 2, 4.0, 52.0);
        group.pointer_moved(55.0, 0.0, false);
        assert_eq!(group.commit(&ids(), 10.0, false), None);
        assert!(!group.is_lifted());
        assert!(group.is_settling(10.0));
    }

    #[test]
    fn a_changed_id_list_cancels_the_drag() {
        let mut group = ReorderableGroup::new();
        group.lift(&ids(), uniform(), 0, 4.0, 4.0);
        group.pointer_moved(52.0, 0.0, false);
        let changed = vec!["a", "b", "c"];
        assert!(!group.is_valid_for(&changed));
        assert_eq!(group.commit(&changed, 10.0, false), None);
        assert!(!group.is_lifted());
        // A reordered list is also a changed identity.
        group.lift(&ids(), uniform(), 0, 4.0, 4.0);
        group.pointer_moved(52.0, 0.0, false);
        assert_eq!(group.commit(&["b", "a", "c", "d"], 10.0, false), None);
    }

    #[test]
    fn settle_glide_ends_at_the_landing_slot() {
        let mut group = ReorderableGroup::new();
        group.lift(&ids(), uniform(), 0, 4.0, 4.0);
        group.pointer_moved(70.0, 0.0, false);
        // Lifted top 66 held within the list; over = floor(70 / 24) = 2.
        assert_eq!(group.over(), Some(2));
        group.commit(&ids(), 1000.0, false).expect("a move");
        // Landing slot top of index 2 is 48: residual 66 - 48 = 18 at the start.
        let (index, start) = group.settling_item(1000.0).expect("settling");
        assert_eq!((index, start), (2, 18.0));
        let (_, mid) = group.settling_item(1075.0).expect("settling");
        assert!(mid > 0.0 && mid < 18.0);
        assert!(group.settling_item(1150.0).is_none());
        assert!(!group.is_settling(1150.0));
        assert_eq!(group.settling_sibling(0, 1150.0), 0.0);
        group.finish_settle(1150.0);
        assert!(group.settling_item(1000.0).is_none());
    }

    #[test]
    fn cancel_glides_home_and_reduced_motion_snaps() {
        let mut group = ReorderableGroup::new();
        group.lift(&ids(), uniform(), 1, 4.0, 40.0);
        group.pointer_moved(70.0, 0.0, false);
        group.cancel(500.0, false);
        // Home slot top of index 1 is 24; the item was painted at 66.
        assert_eq!(group.settling_item(500.0), Some((1, 42.0)));
        // With reduced motion nothing animates.
        let mut group = ReorderableGroup::new();
        group.lift(&ids(), uniform(), 1, 4.0, 40.0);
        group.pointer_moved(80.0, 0.0, true);
        group.cancel(500.0, true);
        assert_eq!(group.settling_item(500.0), Some((1, 0.0)));
        assert_eq!(group.sibling_offset(0, 0.0), 0.0);
    }

    #[test]
    fn variable_height_group_uses_the_midpoint_rule() {
        let layout = GroupLayout::Variable {
            heights: HEIGHTS.to_vec(),
            gap: 2.0,
        };
        let mut group = ReorderableGroup::new();
        group.lift(&["s0", "s1", "s2"], layout, 0, 10.0, 10.0);
        assert_eq!(group.over(), Some(0));
        group.pointer_moved(172.0, 0.0, false);
        // Top 162: crosses the midpoint of the 300 px section.
        assert_eq!(group.lifted_top(), Some(162.0));
        assert_eq!(group.over(), Some(1));
        group.pointer_moved(900.0, 0.0, false);
        // Held at total height 404 - own height 50 = 354 at most.
        assert_eq!(group.lifted_top(), Some(354.0));
        assert_eq!(group.over(), Some(2));
        assert_eq!(group.sibling_offset(2, 150.0), -52.0);
    }

    #[test]
    fn autoscroll_step_requires_the_pointer_inside_horizontally() {
        let mut group = ReorderableGroup::new();
        assert_eq!(
            group.autoscroll_step(50.0, 100.0, 0.0, 200.0, 100.0, 500.0),
            0.0
        );
        group.lift(&ids(), uniform(), 0, 4.0, 4.0);
        assert_eq!(
            group.autoscroll_step(50.0, 100.0, 0.0, 200.0, 100.0, 500.0),
            -12.0
        );
        assert_eq!(
            group.autoscroll_step(250.0, 100.0, 0.0, 200.0, 100.0, 500.0),
            0.0
        );
    }
}
