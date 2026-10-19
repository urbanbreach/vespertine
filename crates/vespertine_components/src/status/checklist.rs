//! The checklist model (CMP-023 R8-R11): summary, folding, expanded-state policy and
//! dismissal. Pure: no rendering, no time.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;

use vespertine::{App, Global, SharedString};

/// Lists of at most this many items are never folded.
pub const FOLD_THRESHOLD: usize = 6;
/// Items shown in the window of a folded list.
pub const WINDOW: usize = 3;

/// Status of one checklist item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ItemStatus {
    /// Not started.
    Pending,
    /// Being worked on.
    InProgress,
    /// Done.
    Completed,
}

/// One checklist item, supplied by the owner.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Item {
    /// Text.
    pub text: SharedString,
    /// Status.
    pub status: ItemStatus,
}

impl Item {
    /// An item.
    pub fn new(text: impl Into<SharedString>, status: ItemStatus) -> Self {
        Self {
            text: text.into(),
            status,
        }
    }
}

/// The summary of a list (R8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Summary {
    /// Item count.
    pub total: usize,
    /// Completed items.
    pub done: usize,
    /// Index of the first in-progress item.
    pub active: Option<usize>,
    /// Index of the first item not completed.
    pub next: Option<usize>,
    /// `total > 0` and every item completed.
    pub finished: bool,
}

impl Summary {
    /// The headline item: the active one, else the next.
    pub fn headline(&self) -> Option<usize> {
        self.active.or(self.next)
    }
}

/// Summarizes `items`.
pub fn summary(items: &[Item]) -> Summary {
    let total = items.len();
    let done = items
        .iter()
        .filter(|item| item.status == ItemStatus::Completed)
        .count();
    Summary {
        total,
        done,
        active: items
            .iter()
            .position(|item| item.status == ItemStatus::InProgress),
        next: items
            .iter()
            .position(|item| item.status != ItemStatus::Completed),
        finished: total > 0 && done == total,
    }
}

/// The folding window (R10): all items for lists of up to 6; otherwise 3 items starting at
/// `min(max(focus - 1, 0), total - 3)` where `focus` is the headline or the last item.
pub fn focus_window(items: &[Item]) -> Range<usize> {
    let total = items.len();
    if total <= FOLD_THRESHOLD {
        return 0..total;
    }
    let focus = summary(items).headline().unwrap_or(total - 1);
    let start = focus.saturating_sub(1).min(total - WINDOW);
    start..start + WINDOW
}

/// Which side of the window a fold row stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    /// Items before the window.
    Earlier,
    /// Items after the window.
    Later,
}

/// One row of the expanded list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    /// An item, by index in the owner's list.
    Item(usize),
    /// A fold row: how many items it stands for and whether they are shown.
    Fold {
        /// The side.
        side: Side,
        /// Hidden-or-shown item count.
        count: usize,
        /// Whether the side is open (its items are listed).
        open: bool,
    },
}

/// The expanded rows (R10): the earlier fold row (when items precede the window) then, when
/// open, the earlier items; the window items; then, when items follow the window, the later
/// items when open and the later fold row, so each fold row sits at the outer edge of the list
/// it stands for.
pub fn rows(items: &[Item], show_earlier: bool, show_later: bool) -> Vec<Row> {
    let window = focus_window(items);
    let mut rows = Vec::with_capacity(items.len() + 2);
    if window.start > 0 {
        rows.push(Row::Fold {
            side: Side::Earlier,
            count: window.start,
            open: show_earlier,
        });
        if show_earlier {
            rows.extend((0..window.start).map(Row::Item));
        }
    }
    rows.extend(window.clone().map(Row::Item));
    if window.end < items.len() {
        if show_later {
            rows.extend((window.end..items.len()).map(Row::Item));
        }
        rows.push(Row::Fold {
            side: Side::Later,
            count: items.len() - window.end,
            open: show_later,
        });
    }
    rows
}

/// A hash over the sequence of (text, status), used to remember a dismissal.
pub fn signature(items: &[Item]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    items.len().hash(&mut hasher);
    for item in items {
        item.hash(&mut hasher);
    }
    hasher.finish()
}

/// Presentation state of one section instance (R9, R11): kept in memory for the lifetime of
/// the process, independent per owner key.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SectionState {
    explicit: Option<bool>,
    show_earlier: bool,
    show_later: bool,
    dismissed: Option<u64>,
    was_settled: bool,
    epoch: u64,
}

impl SectionState {
    /// The default state: no explicit choice, folds closed, not dismissed.
    pub fn new() -> Self {
        Self::default()
    }

    /// Observes whether the section is settled (finished and work not live). On the frame it
    /// becomes settled after having been unsettled, the explicit choice is cleared once so the
    /// section tidies itself to compact; a later manual expansion is respected.
    pub fn observe(&mut self, settled: bool) {
        if settled && !self.was_settled {
            self.explicit = None;
        }
        self.was_settled = settled;
    }

    /// Effective expansion: the explicit choice if set, otherwise not finished.
    pub fn is_expanded(&self, finished: bool) -> bool {
        self.explicit.unwrap_or(!finished)
    }

    /// Toggles: the explicit choice becomes the opposite of the effective value. Bumps the
    /// epoch that replays the rows' fade-in.
    pub fn toggle(&mut self, finished: bool) {
        self.explicit = Some(!self.is_expanded(finished));
        self.epoch += 1;
    }

    /// The explicit choice, when set.
    pub fn explicit(&self) -> Option<bool> {
        self.explicit
    }

    /// Toggles one fold row.
    pub fn toggle_fold(&mut self, side: Side) {
        match side {
            Side::Earlier => self.show_earlier = !self.show_earlier,
            Side::Later => self.show_later = !self.show_later,
        }
    }

    /// Whether the earlier items are shown.
    pub fn show_earlier(&self) -> bool {
        self.show_earlier
    }

    /// Whether the later items are shown.
    pub fn show_later(&self) -> bool {
        self.show_later
    }

    /// The toggle epoch.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// A section can be dismissed when its work is not live (finished or not).
    pub fn can_dismiss(live: bool) -> bool {
        !live
    }

    /// Records a dismissal against the signature of `items`.
    pub fn dismiss(&mut self, items: &[Item]) {
        self.dismissed = Some(signature(items));
    }

    /// Whether the section stays hidden: the owner's list has the dismissed signature.
    pub fn is_dismissed(&self, items: &[Item]) -> bool {
        self.dismissed == Some(signature(items))
    }
}

/// The per-owner presentation states of the application.
#[derive(Default)]
pub struct SectionStates {
    states: HashMap<SharedString, SectionState>,
}

impl Global for SectionStates {}

impl SectionStates {
    /// The state of `owner`, created on first use.
    pub fn state<'a>(cx: &'a mut App, owner: &SharedString) -> &'a mut SectionState {
        cx.default_global::<SectionStates>()
            .states
            .entry(owner.clone())
            .or_default()
    }

    /// A copy of the state of `owner`, when one exists.
    pub fn peek(cx: &App, owner: &SharedString) -> Option<SectionState> {
        cx.try_global::<SectionStates>()
            .and_then(|states| states.states.get(owner).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(statuses: &[ItemStatus]) -> Vec<Item> {
        statuses
            .iter()
            .enumerate()
            .map(|(i, status)| Item::new(format!("item {i}"), *status))
            .collect()
    }

    fn ten(in_progress: Option<usize>, completed_before: usize) -> Vec<Item> {
        (0..10)
            .map(|i| {
                let status = if in_progress == Some(i) {
                    ItemStatus::InProgress
                } else if i < completed_before {
                    ItemStatus::Completed
                } else {
                    ItemStatus::Pending
                };
                Item::new(format!("item {i}"), status)
            })
            .collect()
    }

    #[test]
    fn a6_six_items_never_fold() {
        let items = list(&[ItemStatus::Pending; 6]);
        let rows = rows(&items, false, false);
        assert_eq!(rows, (0..6).map(Row::Item).collect::<Vec<_>>());
    }

    #[test]
    fn a6_window_follows_the_headline() {
        let items = ten(Some(5), 0);
        assert_eq!(focus_window(&items), 4..7);
        assert_eq!(
            rows(&items, false, false),
            [
                Row::Fold {
                    side: Side::Earlier,
                    count: 4,
                    open: false
                },
                Row::Item(4),
                Row::Item(5),
                Row::Item(6),
                Row::Fold {
                    side: Side::Later,
                    count: 3,
                    open: false
                },
            ]
        );
        let items = ten(Some(0), 0);
        assert_eq!(focus_window(&items), 0..3);
        assert_eq!(
            rows(&items, false, false),
            [
                Row::Item(0),
                Row::Item(1),
                Row::Item(2),
                Row::Fold {
                    side: Side::Later,
                    count: 7,
                    open: false
                },
            ]
        );
        let finished = ten(None, 10);
        assert!(summary(&finished).finished);
        assert_eq!(focus_window(&finished), 7..10);
        let rows = rows(&finished, false, false);
        assert_eq!(
            rows[0],
            Row::Fold {
                side: Side::Earlier,
                count: 7,
                open: false
            }
        );
        assert_eq!(rows.len(), 4);
    }

    #[test]
    fn a6_open_folds_list_hidden_items_next_to_their_fold_row() {
        let items = ten(Some(5), 0);
        let open = rows(&items, true, true);
        assert_eq!(
            open,
            [
                Row::Fold {
                    side: Side::Earlier,
                    count: 4,
                    open: true
                },
                Row::Item(0),
                Row::Item(1),
                Row::Item(2),
                Row::Item(3),
                Row::Item(4),
                Row::Item(5),
                Row::Item(6),
                Row::Item(7),
                Row::Item(8),
                Row::Item(9),
                Row::Fold {
                    side: Side::Later,
                    count: 3,
                    open: true
                },
            ]
        );
    }

    #[test]
    fn summary_picks_active_then_next() {
        let items = list(&[
            ItemStatus::Completed,
            ItemStatus::Pending,
            ItemStatus::InProgress,
        ]);
        let summary = summary(&items);
        assert_eq!(summary.done, 1);
        assert_eq!(summary.active, Some(2));
        assert_eq!(summary.next, Some(1));
        assert_eq!(summary.headline(), Some(2));
        assert!(!summary.finished);
        let empty = super::summary(&[]);
        assert!(!empty.finished);
        assert_eq!(empty.headline(), None);
    }

    #[test]
    fn a7_expanded_state_rule() {
        let mut state = SectionState::new();
        // Unfinished defaults to expanded; finished defaults to collapsed.
        assert!(state.is_expanded(false));
        assert!(!state.is_expanded(true));
        // Toggling while unfinished collapses and the choice sticks.
        state.toggle(false);
        assert!(!state.is_expanded(false));
        state.observe(false);
        assert!(!state.is_expanded(false));
        // After the list becomes finished and idle, the explicit choice is cleared once.
        state.observe(true);
        assert_eq!(state.explicit(), None);
        assert!(!state.is_expanded(true));
        // A later toggle is respected, including across further settled frames.
        state.toggle(true);
        assert!(state.is_expanded(true));
        state.observe(true);
        assert!(state.is_expanded(true));
        // A finished list stays compact by default when a new run begins.
        let mut other = SectionState::new();
        other.observe(true);
        other.observe(false);
        assert!(!other.is_expanded(true));
    }

    #[test]
    fn a7_states_of_two_owners_are_independent() {
        let mut states = SectionStates::default();
        let a: SharedString = "a".into();
        let b: SharedString = "b".into();
        states.states.entry(a.clone()).or_default().toggle(false);
        assert!(!states.states[&a].is_expanded(false));
        assert!(states.states.entry(b).or_default().is_expanded(false));
    }

    #[test]
    fn a8_dismissal_holds_for_the_same_list_and_ends_when_it_changes() {
        let items = ten(Some(2), 2);
        let mut state = SectionState::new();
        assert!(!state.is_dismissed(&items));
        state.dismiss(&items);
        assert!(state.is_dismissed(&items));
        assert!(state.is_dismissed(&items.clone()));
        let mut changed = items.clone();
        changed[4].text = "other".into();
        assert!(!state.is_dismissed(&changed));
        let mut status = items.clone();
        status[2].status = ItemStatus::Completed;
        assert!(!state.is_dismissed(&status));
        assert!(!SectionState::can_dismiss(true));
        assert!(SectionState::can_dismiss(false));
        // An empty list has the empty signature, distinct from non-empty lists.
        assert_ne!(signature(&[]), signature(&items));
    }

    #[test]
    fn fold_toggles_and_epoch() {
        let mut state = SectionState::new();
        state.toggle_fold(Side::Earlier);
        assert!(state.show_earlier() && !state.show_later());
        state.toggle_fold(Side::Earlier);
        state.toggle_fold(Side::Later);
        assert!(!state.show_earlier() && state.show_later());
        let epoch = state.epoch();
        state.toggle(false);
        assert_eq!(state.epoch(), epoch + 1);
    }
}
