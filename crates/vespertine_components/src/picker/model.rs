//! The non-visual state of a picker: query, scope tabs, highlight, list/panel depth, favorites,
//! keyboard mode and the Enter latch (CMP-006 R12, R14, R16, R17).

use super::highlight::Highlight;
use super::results::{Results, RowKey, View};
use crate::floating::stepping::{Edge, StepDirection};
use std::collections::BTreeSet;
use vespertine::SharedString;

/// Once-per-physical-press Enter activation (R14). X11 delivers repeated key-downs that are not
/// flagged as held, so the latch is set on the first key-down and cleared by the matching key-up
/// (and by [`EnterLatch::reset`] on reopen or focus loss).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EnterLatch {
    down: bool,
}

impl EnterLatch {
    /// An open latch.
    pub fn new() -> Self {
        Self::default()
    }

    /// An Enter key-down. Returns whether it activates: only when the latch was not already down
    /// and the event is not flagged as held. Every key-down sets the latch; callers consume the
    /// event either way.
    pub fn key_down(&mut self, held: bool) -> bool {
        let activate = !self.down && !held;
        self.down = true;
        activate
    }

    /// The matching key-up.
    pub fn key_up(&mut self) {
        self.down = false;
    }

    /// Clears the latch (palette key state reset: reopen, focus loss).
    pub fn reset(&mut self) {
        self.down = false;
    }

    /// Whether Enter is currently held down.
    pub fn is_down(&self) -> bool {
        self.down
    }
}

/// One scope tab (R12).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopeTab {
    /// Stable key.
    pub key: SharedString,
    /// Tab label.
    pub label: SharedString,
    /// Section keys shown by a dedicated scope; `None` is the summary ("All") tab.
    pub sections: Option<Vec<SharedString>>,
}

impl ScopeTab {
    /// The summary tab.
    pub fn all(label: impl Into<SharedString>) -> Self {
        Self {
            key: "all".into(),
            label: label.into(),
            sections: None,
        }
    }

    /// A dedicated scope over the given sections.
    pub fn scope(
        key: impl Into<SharedString>,
        label: impl Into<SharedString>,
        sections: Vec<SharedString>,
    ) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            sections: Some(sections),
        }
    }
}

/// How deep the picker currently is (R16).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Depth {
    /// The result list.
    List,
    /// The per-selection settings panel of a row.
    Panel(RowKey),
    /// A nested submenu of choices opened from the panel.
    Choices(RowKey),
}

/// What the list shows again after coming back from a panel.
#[derive(Clone, Debug, PartialEq)]
pub struct ListRestore {
    /// Scroll offset to restore.
    pub scroll_offset: f32,
    /// Highlighted identity to restore.
    pub highlighted: Option<RowKey>,
}

/// The result of an Escape (R16).
#[derive(Clone, Debug, PartialEq)]
pub enum EscapeOutcome {
    /// A nested choice submenu closed; the panel remains.
    ClosedChoices,
    /// The panel closed and the list is back with its saved scroll and highlight.
    ClosedPanel(ListRestore),
    /// Nothing is left to undo: the picker closes.
    Close,
}

/// What reopening does with the previous session (R17).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReopenPolicy {
    /// Clear the query (default).
    pub clear_query: bool,
    /// Clear the highlight (default).
    pub clear_highlight: bool,
}

impl Default for ReopenPolicy {
    fn default() -> Self {
        Self {
            clear_query: true,
            clear_highlight: true,
        }
    }
}

/// The picker state machine.
#[derive(Clone, Debug, Default)]
pub struct PickerModel {
    query: String,
    tabs: Vec<ScopeTab>,
    active_tab: usize,
    highlight: Highlight,
    depth_stack: Vec<Depth>,
    saved_list: Option<ListRestore>,
    favorites: BTreeSet<RowKey>,
    selected: Option<RowKey>,
    keyboard_mode: bool,
    latch: EnterLatch,
    /// The list must scroll to its top (tab or query change).
    scroll_to_top: bool,
}

impl PickerModel {
    /// A picker with the given scope tabs (empty means a single summary view).
    pub fn new(tabs: Vec<ScopeTab>) -> Self {
        Self {
            tabs,
            ..Self::default()
        }
    }

    /// The query text; panel and list switches never change it.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Changes the query: the highlight resets and the list scrolls to the top (R12). Returns
    /// whether the query actually changed.
    pub fn set_query(&mut self, query: &str) -> bool {
        if self.query == query {
            return false;
        }
        self.query = query.to_string();
        self.highlight.reset();
        self.scroll_to_top = true;
        true
    }

    /// Takes the scroll-to-top request raised by a query or tab change.
    pub fn take_scroll_to_top(&mut self) -> bool {
        std::mem::take(&mut self.scroll_to_top)
    }

    /// The scope tabs.
    pub fn tabs(&self) -> &[ScopeTab] {
        &self.tabs
    }

    /// The index of the active tab.
    pub fn active_tab(&self) -> usize {
        self.active_tab
    }

    /// The view of the active tab.
    pub fn view(&self) -> View {
        match self
            .tabs
            .get(self.active_tab)
            .and_then(|tab| tab.sections.clone())
        {
            Some(sections) => View::Scope(sections),
            None => View::Summary,
        }
    }

    fn change_tab(&mut self, index: usize) -> bool {
        if index == self.active_tab || index >= self.tabs.len() {
            return false;
        }
        self.active_tab = index;
        self.highlight.reset();
        self.scroll_to_top = true;
        true
    }

    /// Tab (`forward`) or Shift-Tab: steps through the tabs with wraparound (R12). Returns
    /// whether the active tab changed. The key is consumed either way.
    pub fn step_tab(&mut self, forward: bool) -> bool {
        self.keyboard_mode = true;
        let count = self.tabs.len();
        if count < 2 {
            return false;
        }
        let next = if forward {
            (self.active_tab + 1) % count
        } else {
            (self.active_tab + count - 1) % count
        };
        self.change_tab(next)
    }

    /// The numbered accelerator: selects the tab with the 1-based `number` (R12).
    pub fn select_tab_number(&mut self, number: usize) -> bool {
        number
            .checked_sub(1)
            .is_some_and(|index| self.change_tab(index))
    }

    /// The highlight.
    pub fn highlight(&self) -> &Highlight {
        &self.highlight
    }

    /// Resolves the highlight against the displayed rows and returns its flat index.
    pub fn reconcile(&mut self, results: &Results) -> Option<usize> {
        self.highlight
            .reconcile(&results.keys(), &results.enabled())
    }

    /// Up/Down or Ctrl-P/Ctrl-N: steps the highlight with wraparound and turns keyboard mode on.
    pub fn step(&mut self, results: &Results, direction: StepDirection) -> Option<usize> {
        self.keyboard_mode = true;
        self.highlight
            .step(&results.keys(), &results.enabled(), direction)
    }

    /// Home/End.
    pub fn jump(&mut self, results: &Results, edge: Edge) -> Option<usize> {
        self.keyboard_mode = true;
        self.highlight
            .jump(&results.keys(), &results.enabled(), edge)
    }

    /// Pointer motion over a row: moves the highlight and clears keyboard mode (R13).
    pub fn pointer_over(&mut self, results: &Results, key: &RowKey) -> bool {
        self.keyboard_mode = false;
        self.highlight
            .point_at(&results.keys(), &results.enabled(), key)
            .is_some()
    }

    /// Keyboard mode: focus indicators and the active descendant show (R16, R18).
    pub fn keyboard_mode(&self) -> bool {
        self.keyboard_mode
    }

    /// The Enter latch.
    pub fn latch(&mut self) -> &mut EnterLatch {
        &mut self.latch
    }

    /// Enter pressed: returns the highlighted row key to activate, once per physical press.
    pub fn enter_down(&mut self, results: &Results, held: bool) -> Option<RowKey> {
        if !self.latch.key_down(held) {
            return None;
        }
        self.reconcile(results)?;
        self.highlight.key().cloned()
    }

    /// The favorites set (kept across panel switches, reopening and queries).
    pub fn favorites(&self) -> &BTreeSet<RowKey> {
        &self.favorites
    }

    /// Toggles a row's favorite state (primary modifier + Shift + F on the highlighted row).
    pub fn toggle_favorite(&mut self, key: &RowKey) -> bool {
        if self.favorites.remove(key) {
            false
        } else {
            self.favorites.insert(key.clone());
            true
        }
    }

    /// The selected identity (kept across panel switches and reopening).
    pub fn selected(&self) -> Option<&RowKey> {
        self.selected.as_ref()
    }

    /// Sets the selected identity.
    pub fn set_selected(&mut self, selected: Option<RowKey>) {
        self.selected = selected;
    }

    /// The nth visible row (1-based) for the numbered row accelerators: the key of the nth
    /// enabled row of the flat order.
    pub fn nth_visible(&self, results: &Results, number: usize) -> Option<RowKey> {
        let index = number.checked_sub(1)?;
        results
            .rows()
            .filter(|row| row.enabled)
            .nth(index)
            .map(|row| row.key.clone())
    }

    /// The current depth.
    pub fn depth(&self) -> Depth {
        self.depth_stack.last().cloned().unwrap_or(Depth::List)
    }

    /// Opens the settings panel of `key`, remembering the list's scroll offset and highlighted
    /// identity. The query, favorites and selected identity are untouched (R16).
    pub fn open_panel(&mut self, key: RowKey, scroll_offset: f32) {
        if self.depth_stack.is_empty() {
            self.saved_list = Some(ListRestore {
                scroll_offset,
                highlighted: self.highlight.key().cloned(),
            });
        }
        self.depth_stack.clear();
        self.depth_stack.push(Depth::Panel(key));
    }

    /// Opens a nested choice submenu on top of the open panel.
    pub fn open_choices(&mut self) {
        if let Some(Depth::Panel(key)) = self.depth_stack.last().cloned() {
            self.depth_stack.push(Depth::Choices(key));
        }
    }

    fn step_back(&mut self) -> Option<EscapeOutcome> {
        match self.depth_stack.pop()? {
            Depth::Choices(_) => Some(EscapeOutcome::ClosedChoices),
            Depth::Panel(_) => {
                let restore = self.saved_list.take().unwrap_or(ListRestore {
                    scroll_offset: 0.0,
                    highlighted: None,
                });
                if let Some(key) = restore.highlighted.clone() {
                    // The next reconcile finds the identity again.
                    self.highlight = Highlight::seeded(key);
                }
                Some(EscapeOutcome::ClosedPanel(restore))
            }
            Depth::List => None,
        }
    }

    /// Escape (R16, CMP-002 R8): undoes the innermost depth first and closes only when nothing
    /// is left to undo. Escape never turns keyboard mode on.
    pub fn escape(&mut self) -> EscapeOutcome {
        self.step_back().unwrap_or(EscapeOutcome::Close)
    }

    /// Left: steps back out of an open choice submenu; otherwise it is not handled.
    pub fn left(&mut self) -> bool {
        self.keyboard_mode = true;
        if matches!(self.depth(), Depth::Choices(_)) {
            self.step_back();
            true
        } else {
            false
        }
    }

    /// Reopening applies the owner's policy (R17): the default clears the query and the highlight
    /// and keeps favorites and the selected identity. The depth always returns to the list and
    /// the Enter latch is reset.
    pub fn reopen(&mut self, policy: ReopenPolicy) {
        if policy.clear_query {
            self.query.clear();
        }
        if policy.clear_highlight {
            self.highlight.reset();
        }
        self.depth_stack.clear();
        self.saved_list = None;
        self.keyboard_mode = false;
        self.latch.reset();
        self.active_tab = 0;
        self.scroll_to_top = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::picker::results::{Paging, Row, SectionSpec};

    fn results(count: usize) -> Results {
        let rows = (0..count)
            .map(|ix| Row::new(format!("k{ix}"), format!("row {ix}")))
            .collect();
        Results::build(
            &[SectionSpec::new("s", rows)],
            &View::Summary,
            &Paging::default(),
            "",
            "",
        )
    }

    #[test]
    fn one_activation_per_physical_press() {
        let list = results(2);
        let mut model = PickerModel::default();
        assert_eq!(model.enter_down(&list, false).as_deref(), Some("k0"));
        // An X11 repeat that is not flagged as held is consumed without activating.
        assert_eq!(model.enter_down(&list, false), None);
        assert_eq!(model.enter_down(&list, true), None);
        model.latch().key_up();
        assert_eq!(model.enter_down(&list, false).as_deref(), Some("k0"));
        // A reset clears the latch (reopen, focus loss).
        model.latch().reset();
        assert_eq!(model.enter_down(&list, false).as_deref(), Some("k0"));
        model.latch().reset();
        assert_eq!(
            model.enter_down(&list, true),
            None,
            "a held first key-down never activates"
        );
    }

    #[test]
    fn tabs_wrap_and_numbers_select_and_reset_highlight_and_scroll() {
        let list = results(3);
        let mut model = PickerModel::new(vec![
            ScopeTab::all("All"),
            ScopeTab::scope("files", "Files", vec!["s".into()]),
            ScopeTab::scope("cmds", "Commands", vec!["c".into()]),
        ]);
        model.step(&list, StepDirection::Forward);
        assert_eq!(model.highlight().last_index(), 1);
        assert!(model.step_tab(true));
        assert_eq!(model.active_tab(), 1);
        assert_eq!(model.view(), View::Scope(vec!["s".into()]));
        assert!(model.take_scroll_to_top());
        assert_eq!(model.highlight().key(), None, "the highlight reset");
        assert!(model.step_tab(true));
        assert!(model.step_tab(true));
        assert_eq!(
            model.active_tab(),
            0,
            "Tab wraps from the last tab to the first"
        );
        assert!(model.step_tab(false));
        assert_eq!(
            model.active_tab(),
            2,
            "Shift-Tab wraps from the first to the last"
        );
        assert!(model.select_tab_number(2));
        assert_eq!(model.active_tab(), 1);
        assert!(!model.select_tab_number(2), "the same tab is not a change");
        assert!(!model.select_tab_number(9));
        assert!(!model.select_tab_number(0));
    }

    #[test]
    fn a_new_query_resets_the_highlight_and_scrolls_to_the_top() {
        let list = results(3);
        let mut model = PickerModel::default();
        model.step(&list, StepDirection::Forward);
        assert!(model.set_query("ab"));
        assert!(model.take_scroll_to_top());
        assert!(!model.take_scroll_to_top());
        assert_eq!(model.reconcile(&list), Some(0));
        assert!(!model.set_query("ab"));
    }

    #[test]
    fn nth_visible_skips_disabled_rows() {
        let mut rows: Vec<Row> = (0..3).map(|ix| Row::new(format!("k{ix}"), "r")).collect();
        rows[0].enabled = false;
        let list = Results::build(
            &[SectionSpec::new("s", rows)],
            &View::Summary,
            &Paging::default(),
            "",
            "",
        );
        let model = PickerModel::default();
        assert_eq!(model.nth_visible(&list, 1).as_deref(), Some("k1"));
        assert_eq!(model.nth_visible(&list, 2).as_deref(), Some("k2"));
        assert_eq!(model.nth_visible(&list, 3), None);
        assert_eq!(model.nth_visible(&list, 0), None);
    }

    #[test]
    fn panel_switching_keeps_query_favorites_selection_and_restores_the_list() {
        let list = results(5);
        let mut model = PickerModel::default();
        model.set_query("row");
        model.toggle_favorite(&"k2".into());
        model.set_selected(Some("k3".into()));
        model.step(&list, StepDirection::Forward);
        model.step(&list, StepDirection::Forward);
        assert_eq!(model.highlight().key().map(|k| k.as_ref()), Some("k2"));

        model.open_panel("k2".into(), 120.0);
        assert_eq!(model.depth(), Depth::Panel("k2".into()));
        model.open_choices();
        assert_eq!(model.depth(), Depth::Choices("k2".into()));

        // Escape steps back one depth at a time: choices, panel, then close.
        assert_eq!(model.escape(), EscapeOutcome::ClosedChoices);
        let outcome = model.escape();
        assert_eq!(
            outcome,
            EscapeOutcome::ClosedPanel(ListRestore {
                scroll_offset: 120.0,
                highlighted: Some("k2".into())
            })
        );
        assert_eq!(model.depth(), Depth::List);
        assert_eq!(model.escape(), EscapeOutcome::Close);

        assert_eq!(model.query(), "row");
        assert!(model.favorites().contains("k2"));
        assert_eq!(model.selected().map(|k| k.as_ref()), Some("k3"));
        assert_eq!(
            model.reconcile(&list),
            Some(2),
            "the highlighted identity is restored"
        );
    }

    #[test]
    fn left_steps_back_out_of_choices_only() {
        let mut model = PickerModel::default();
        assert!(!model.left());
        model.open_panel("k".into(), 0.0);
        assert!(!model.left(), "Left does nothing in a plain panel");
        model.open_choices();
        assert!(model.left());
        assert_eq!(model.depth(), Depth::Panel("k".into()));
    }

    #[test]
    fn escape_does_not_turn_keyboard_mode_on_but_navigation_keys_do() {
        let list = results(3);
        let mut model = PickerModel::default();
        model.escape();
        assert!(!model.keyboard_mode());
        model.step(&list, StepDirection::Forward);
        assert!(model.keyboard_mode());
        model.open_panel("k0".into(), 0.0);
        model.pointer_over(&list, &"k1".into());
        assert!(
            !model.keyboard_mode(),
            "pointer motion clears keyboard mode"
        );
        model.escape();
        assert!(!model.keyboard_mode());
    }

    #[test]
    fn reopening_clears_query_and_highlight_but_keeps_favorites_and_selection() {
        let list = results(3);
        let mut model = PickerModel::default();
        model.set_query("x");
        model.toggle_favorite(&"k1".into());
        model.set_selected(Some("k2".into()));
        model.step(&list, StepDirection::Forward);
        model.latch().key_down(false);
        model.reopen(ReopenPolicy::default());
        assert_eq!(model.query(), "");
        assert_eq!(model.highlight().key(), None);
        assert!(model.favorites().contains("k1"));
        assert_eq!(model.selected().map(|k| k.as_ref()), Some("k2"));
        assert!(!model.latch().is_down());

        model.set_query("y");
        model.reopen(ReopenPolicy {
            clear_query: false,
            clear_highlight: false,
        });
        assert_eq!(model.query(), "y");
    }
}
