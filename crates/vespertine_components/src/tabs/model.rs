//! Pure, unit-testable logic of the tab strip (CMP-017): selection cycling, drop quantization,
//! slide offsets, the surviving selection after a close or a move, the visit history and the
//! retargetable slide tween. Nothing here touches the window system.

use std::collections::HashMap;
use vespertine::SharedString;

/// Width of a tab chip.
pub const TAB_WIDTH: f32 = 112.0;
/// Height of a tab chip.
pub const TAB_HEIGHT: f32 = 24.0;
/// Gap between chips.
pub const TAB_GAP: f32 = 4.0;
/// The uniform drop slot: chip width plus gap.
pub const TAB_SLOT: f32 = TAB_WIDTH + TAB_GAP;
/// Width of the overflow fade on a hidden edge.
pub const FADE_BAND: f32 = 36.0;
/// Hidden-edge tolerance in pixels.
pub const EDGE_TOLERANCE: f32 = 1.0;
/// Duration of a slide.
pub const SLIDE_MS: f64 = 150.0;
/// Hover time before a tab's detail tooltip appears.
pub const TOOLTIP_DELAY_MS: u64 = 350;

/// A stable, opaque tab identity. The display order of tabs is separate from their identity:
/// reordering never changes an id.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TabId(SharedString);

impl TabId {
    /// An identity from any text.
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self(id.into())
    }

    /// The identity as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for TabId {
    fn from(id: &str) -> Self {
        Self::new(id.to_owned())
    }
}

impl From<String> for TabId {
    fn from(id: String) -> Self {
        Self::new(id)
    }
}

impl From<u64> for TabId {
    fn from(id: u64) -> Self {
        Self::new(id.to_string())
    }
}

impl std::fmt::Display for TabId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// How the surviving tab is chosen after the selected tab is closed (R5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ClosePolicy {
    /// Index proximity ([`active_after_close`]).
    #[default]
    NearestIndex,
    /// The most recently visited live tab ([`VisitHistory`]), else the first tab.
    MostRecentlyVisited,
}

/// The owner's answer to `prepare_close` (R4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseDecision {
    /// Remove the tab now.
    Allow,
    /// The owner is asking the user (for example about unsaved changes); keep the tab.
    Pending,
    /// The close is refused; keep the tab.
    Blocked,
}

/// Index of the tab selected by one cycling step with wrap-around, or `None` when nothing
/// changes (at most one tab). With nothing selected, or a selection that is no longer in
/// `order`, forward selects the first tab and backward the last.
pub fn cycle_target<T: PartialEq>(
    order: &[T],
    selected: Option<&T>,
    forward: bool,
) -> Option<usize> {
    let len = order.len();
    if len <= 1 {
        return None;
    }
    let current = selected.and_then(|selected| order.iter().position(|item| item == selected));
    Some(match (current, forward) {
        (Some(ix), true) => (ix + 1) % len,
        (Some(ix), false) => (ix + len - 1) % len,
        (None, true) => 0,
        (None, false) => len - 1,
    })
}

/// The drop index of a pointer at `rel` content pixels from the start of the strip (scroll
/// offset already folded in): `clamp(floor(rel / slot), 0, count - 1)`. A negative `rel`, an
/// empty strip and a degenerate slot all yield 0.
pub fn drop_index(rel: f32, slot: f32, count: usize) -> usize {
    if count == 0 || slot <= 0.0 || rel.is_nan() || rel < 0.0 {
        return 0;
    }
    let ix = (rel / slot).floor();
    if ix >= (count - 1) as f32 {
        count - 1
    } else {
        ix as usize
    }
}

/// The slide, in slots, of the tab at `ix` while the tab at `from` is dragged over `over`:
/// -1 for tabs in `(from, over]`, +1 for tabs in `[over, from)`, else 0.
pub fn slide_offset(ix: usize, from: usize, over: usize) -> i32 {
    if from < over && from < ix && ix <= over {
        -1
    } else if over < from && over <= ix && ix < from {
        1
    } else {
        0
    }
}

/// The selected index after closing the tab at `closed` when `active` was selected and
/// `len_after` tabs remain (NearestIndex, R5). `None` when no tab is left.
pub fn active_after_close(active: usize, closed: usize, len_after: usize) -> Option<usize> {
    if len_after == 0 {
        return None;
    }
    let ix = if closed < active { active - 1 } else { active };
    Some(ix.min(len_after - 1))
}

/// Moves the element at `from` to `to`. Returns whether anything changed: an out-of-range or
/// equal index leaves the vector untouched.
pub fn reorder<T>(items: &mut Vec<T>, from: usize, to: usize) -> bool {
    if from == to || from >= items.len() || to >= items.len() {
        return false;
    }
    let item = items.remove(from);
    items.insert(to, item);
    true
}

/// Visit histories of tab identities, scoped by strip key (R5, MostRecentlyVisited).
///
/// A history is unique with the most recent visit last. Only selecting a tab is a visit: the
/// empty or picker state, visibility toggles and reordering never touch it.
#[derive(Clone, Debug, Default)]
pub struct VisitHistory {
    scopes: HashMap<String, Vec<TabId>>,
}

impl VisitHistory {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a visit: the id is appended after removing earlier occurrences.
    pub fn visit(&mut self, scope: &str, id: &TabId) {
        let history = self.scopes.entry(scope.to_owned()).or_default();
        history.retain(|visited| visited != id);
        history.push(id.clone());
    }

    /// Forgets a closed tab. A scope whose history empties is dropped.
    pub fn forget(&mut self, scope: &str, id: &TabId) {
        if let Some(history) = self.scopes.get_mut(scope) {
            history.retain(|visited| visited != id);
            if history.is_empty() {
                self.scopes.remove(scope);
            }
        }
    }

    /// The tab to select when the selected one is gone: the most recent history entry that is
    /// in `live`, else the first live tab, else none.
    pub fn fallback(&self, scope: &str, live: &[TabId]) -> Option<TabId> {
        self.scopes
            .get(scope)
            .and_then(|history| history.iter().rev().find(|id| live.contains(id)))
            .or_else(|| live.first())
            .cloned()
    }

    /// The history of a scope, oldest first.
    pub fn history(&self, scope: &str) -> &[TabId] {
        self.scopes.get(scope).map_or(&[], Vec::as_slice)
    }

    /// Whether the scope has a history.
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.contains_key(scope)
    }
}

/// The overflow state of a horizontal scroll: `(hidden_start, hidden_end)` from the scrolled
/// distance and the maximum scroll, with the 1 px tolerance of R9.
pub fn edge_hidden(scrolled: f32, max_scroll: f32) -> (bool, bool) {
    (
        scrolled > EDGE_TOLERANCE,
        scrolled < max_scroll - EDGE_TOLERANCE,
    )
}

/// The scroll offset (<= 0) that brings content span `[left, right]` fully into a viewport of
/// `viewport` pixels with the minimal movement from `offset`; `offset` itself when the span is
/// already fully visible.
pub fn reveal_offset(offset: f32, left: f32, right: f32, viewport: f32) -> f32 {
    let start = -offset;
    if left < start {
        -left
    } else if right > start + viewport {
        -(right - viewport).max(0.0)
    } else {
        offset
    }
}

/// Ease-out (cubic) of a progress in `[0, 1]`.
pub fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// One retargetable tween of a displacement, in slots (or pixels).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slide {
    from: f32,
    to: f32,
    start_ms: f64,
}

impl Slide {
    /// A tween at rest on `value`.
    pub fn at(value: f32) -> Self {
        Self {
            from: value,
            to: value,
            start_ms: 0.0,
        }
    }

    /// The value at `now_ms`.
    pub fn value(&self, now_ms: f64) -> f32 {
        if self.from == self.to {
            return self.to;
        }
        let t = ((now_ms - self.start_ms) / SLIDE_MS).clamp(0.0, 1.0) as f32;
        self.from + (self.to - self.from) * ease_out(t)
    }

    /// The value the tween is heading to.
    pub fn target(&self) -> f32 {
        self.to
    }

    /// Whether the tween is still moving at `now_ms`.
    pub fn is_animating(&self, now_ms: f64) -> bool {
        self.from != self.to && now_ms - self.start_ms < SLIDE_MS
    }

    /// Restarts from the current displaced position toward `to`. With `snap` the value jumps.
    /// Returns whether the target changed.
    pub fn retarget(&mut self, to: f32, now_ms: f64, snap: bool) -> bool {
        if to == self.to {
            return false;
        }
        self.from = if snap { to } else { self.value(now_ms) };
        self.to = to;
        self.start_ms = now_ms;
        true
    }
}

/// The slides of every tab (or group item) during one drag, with an epoch counter that
/// increases whenever a retarget restarts any of them.
#[derive(Clone, Debug, Default)]
pub struct SlideSet {
    slides: Vec<Slide>,
    epoch: u64,
}

impl SlideSet {
    /// `count` slides at rest.
    pub fn new(count: usize) -> Self {
        Self {
            slides: vec![Slide::at(0.0); count],
            epoch: 0,
        }
    }

    /// Retargets every slide to `slide_offset(ix, from, over)`.
    pub fn retarget_for_drag(&mut self, from: usize, over: usize, now_ms: f64, snap: bool) {
        let mut changed = false;
        for (ix, slide) in self.slides.iter_mut().enumerate() {
            changed |= slide.retarget(slide_offset(ix, from, over) as f32, now_ms, snap);
        }
        if changed {
            self.epoch += 1;
        }
    }

    /// Retargets every slide to the given values (used by group settling).
    pub fn retarget_all(&mut self, targets: &[f32], now_ms: f64, snap: bool) {
        let mut changed = false;
        for (slide, target) in self.slides.iter_mut().zip(targets) {
            changed |= slide.retarget(*target, now_ms, snap);
        }
        if changed {
            self.epoch += 1;
        }
    }

    /// The displacement of slide `ix` at `now_ms`.
    pub fn value(&self, ix: usize, now_ms: f64) -> f32 {
        self.slides.get(ix).map_or(0.0, |slide| slide.value(now_ms))
    }

    /// The target of slide `ix`.
    pub fn target(&self, ix: usize) -> f32 {
        self.slides.get(ix).map_or(0.0, Slide::target)
    }

    /// Whether any slide is still moving.
    pub fn is_animating(&self, now_ms: f64) -> bool {
        self.slides.iter().any(|slide| slide.is_animating(now_ms))
    }

    /// The restart counter: it increments each time a retarget restarts the animation.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The number of slides.
    pub fn len(&self) -> usize {
        self.slides.len()
    }

    /// Whether there are no slides.
    pub fn is_empty(&self) -> bool {
        self.slides.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(names: &[&str]) -> Vec<TabId> {
        names.iter().map(|name| TabId::from(*name)).collect()
    }

    #[test]
    fn cycle_target_wraps_and_handles_missing_selection() {
        let order = ids(&["a", "b", "c"]);
        let b = TabId::from("b");
        let a = TabId::from("a");
        let c = TabId::from("c");
        assert_eq!(cycle_target(&order, Some(&b), true), Some(2));
        assert_eq!(cycle_target(&order, Some(&c), true), Some(0));
        assert_eq!(cycle_target(&order, Some(&a), false), Some(2));
        assert_eq!(cycle_target(&order, Some(&b), false), Some(0));
        // Nothing selected, or a selection that is no longer listed.
        assert_eq!(cycle_target(&order, None, true), Some(0));
        assert_eq!(cycle_target(&order, None, false), Some(2));
        let stale = TabId::from("gone");
        assert_eq!(cycle_target(&order, Some(&stale), true), Some(0));
        assert_eq!(cycle_target(&order, Some(&stale), false), Some(2));
        // At most one tab: nothing changes.
        assert_eq!(cycle_target(&order[..1], Some(&a), true), None);
        assert_eq!(cycle_target::<TabId>(&[], None, true), None);
    }

    #[test]
    fn drop_index_quantizes_and_clamps() {
        let cases = [
            (f32::NAN, 150.0, 3, 0),
            (-10.0, 150.0, 3, 0),
            (0.0, 150.0, 3, 0),
            (149.0, 150.0, 3, 0),
            (150.0, 150.0, 3, 1),
            (700.0, 150.0, 3, 2),
            (10.0, 150.0, 0, 0),
            (120.0, TAB_SLOT, 5, 1),
        ];
        for (x, slot, count, expected) in cases {
            assert_eq!(drop_index(x, slot, count), expected, "x = {x}");
        }
    }

    #[test]
    fn slide_offset_tables() {
        // Source 0 over 2: tabs 1 and 2 slide -1, tab 3 stays.
        let offsets: Vec<i32> = (0..4).map(|ix| slide_offset(ix, 0, 2)).collect();
        assert_eq!(offsets, [0, -1, -1, 0]);
        // Source 3 over 1: tabs 1 and 2 slide +1.
        let offsets: Vec<i32> = (0..4).map(|ix| slide_offset(ix, 3, 1)).collect();
        assert_eq!(offsets, [0, 1, 1, 0]);
        // Hovering the origin moves nothing.
        assert!((0..4).all(|ix| slide_offset(ix, 2, 2) == 0));
    }

    #[test]
    fn active_after_close_table() {
        // Close left of selected (selected 2, closed 0, 3 remain) -> 1.
        assert_eq!(active_after_close(2, 0, 3), Some(1));
        // Close selected mid-list (selected 1, closed 1, 2 remain) -> 1.
        assert_eq!(active_after_close(1, 1, 2), Some(1));
        // Close selected at the tail (selected 2, closed 2, 2 remain) -> 1.
        assert_eq!(active_after_close(2, 2, 2), Some(1));
        // The last tab closed -> none.
        assert_eq!(active_after_close(0, 0, 0), None);
        // Closing a tab right of the selection keeps the index.
        assert_eq!(active_after_close(0, 2, 3), Some(0));
    }

    #[test]
    fn reorder_moves_and_ignores_invalid_indices() {
        let mut items = vec!['a', 'b', 'c', 'd'];
        assert!(reorder(&mut items, 0, 2));
        assert_eq!(items, ['b', 'c', 'a', 'd']);
        assert!(!reorder(&mut items, 1, 1));
        assert!(!reorder(&mut items, 4, 0));
        assert!(!reorder(&mut items, 0, 9));
        assert_eq!(items, ['b', 'c', 'a', 'd']);
        let mut empty: Vec<char> = Vec::new();
        assert!(!reorder(&mut empty, 0, 0));
    }

    #[test]
    fn visit_history_keeps_unique_most_recent_last() {
        let mut history = VisitHistory::new();
        for name in ["a", "b", "c", "d", "c", "b", "d", "d"] {
            history.visit("main", &TabId::from(name));
        }
        assert_eq!(history.history("main"), ids(&["a", "c", "b", "d"]));
        history.forget("main", &TabId::from("d"));
        let live = ids(&["a", "b", "c"]);
        assert_eq!(history.fallback("main", &live), Some(TabId::from("b")));
        history.forget("main", &TabId::from("b"));
        let live = ids(&["a", "c"]);
        assert_eq!(history.fallback("main", &live), Some(TabId::from("c")));
    }

    #[test]
    fn visit_history_skips_stale_ids_and_falls_back_to_the_first_tab() {
        let mut history = VisitHistory::new();
        history.visit("s", &TabId::from("x"));
        history.visit("s", &TabId::from("y"));
        // Neither id is live: the first live tab.
        let live = ids(&["p", "q"]);
        assert_eq!(history.fallback("s", &live), Some(TabId::from("p")));
        // Stale entries are skipped, the newest live entry wins.
        history.visit("s", &TabId::from("q"));
        history.visit("s", &TabId::from("zzz"));
        assert_eq!(history.fallback("s", &live), Some(TabId::from("q")));
        assert_eq!(history.fallback("s", &[]), None);
        assert_eq!(history.fallback("unknown", &live), Some(TabId::from("p")));
    }

    #[test]
    fn visit_history_is_scoped_and_drops_emptied_scopes() {
        let mut history = VisitHistory::new();
        history.visit("one", &TabId::from("a"));
        history.visit("two", &TabId::from("b"));
        assert_eq!(history.history("one"), ids(&["a"]));
        assert_eq!(history.history("two"), ids(&["b"]));
        // Forgetting in one scope never touches another.
        history.forget("one", &TabId::from("b"));
        assert!(history.has_scope("two"));
        history.forget("two", &TabId::from("b"));
        assert!(!history.has_scope("two"));
        assert!(history.has_scope("one"));
        assert!(history.history("two").is_empty());
    }

    #[test]
    fn edge_hidden_uses_a_one_pixel_tolerance() {
        assert_eq!(edge_hidden(0.0, 100.0), (false, true));
        assert_eq!(edge_hidden(1.0, 100.0), (false, true));
        assert_eq!(edge_hidden(1.5, 100.0), (true, true));
        assert_eq!(edge_hidden(50.0, 100.0), (true, true));
        assert_eq!(edge_hidden(99.0, 100.0), (true, false));
        assert_eq!(edge_hidden(100.0, 100.0), (true, false));
        assert_eq!(edge_hidden(0.0, 0.0), (false, false));
    }

    #[test]
    fn reveal_offset_is_minimal() {
        // Fully visible: unchanged.
        assert_eq!(reveal_offset(0.0, 0.0, 112.0, 300.0), 0.0);
        assert_eq!(reveal_offset(-50.0, 100.0, 212.0, 300.0), -50.0);
        // Hidden to the right: right edge lands on the viewport edge.
        assert_eq!(reveal_offset(0.0, 464.0, 576.0, 300.0), -276.0);
        // Hidden to the left: left edge lands on the viewport edge.
        assert_eq!(reveal_offset(-400.0, 116.0, 228.0, 300.0), -116.0);
    }

    #[test]
    fn slide_retargets_from_the_current_displacement() {
        let mut slide = Slide::at(0.0);
        assert!(slide.retarget(-1.0, 1000.0, false));
        assert_eq!(slide.value(1000.0), 0.0);
        let mid = slide.value(1075.0);
        assert!(
            mid < -0.5 && mid > -1.0,
            "ease-out is past halfway at 75 ms: {mid}"
        );
        assert_eq!(slide.value(1150.0), -1.0);
        assert!(!slide.is_animating(1150.0));
        // Restart mid-flight: the new tween begins at the displaced position.
        assert!(slide.retarget(0.0, 1075.0, false));
        assert!((slide.value(1075.0) - mid).abs() < 1e-6);
        assert_eq!(slide.value(1225.0), 0.0);
        // The same target does not restart.
        assert!(!slide.retarget(0.0, 1100.0, false));
        // Snap jumps.
        assert!(slide.retarget(1.0, 1300.0, true));
        assert_eq!(slide.value(1300.0), 1.0);
    }

    #[test]
    fn slide_set_counts_restarts_by_epoch() {
        let mut set = SlideSet::new(4);
        set.retarget_for_drag(0, 2, 0.0, false);
        assert_eq!(set.epoch(), 1);
        assert_eq!(
            (set.target(1), set.target(2), set.target(3)),
            (-1.0, -1.0, 0.0)
        );
        // Same over: nothing restarts.
        set.retarget_for_drag(0, 2, 50.0, false);
        assert_eq!(set.epoch(), 1);
        // Mid-flight change of over: restarts from the current displacement.
        let before = set.value(1, 75.0);
        set.retarget_for_drag(0, 0, 75.0, false);
        assert_eq!(set.epoch(), 2);
        assert!((set.value(1, 75.0) - before).abs() < 1e-6);
        assert_eq!(set.value(1, 75.0 + SLIDE_MS), 0.0);
        assert!(set.is_animating(100.0));
        assert!(!set.is_animating(75.0 + SLIDE_MS));
    }
}
