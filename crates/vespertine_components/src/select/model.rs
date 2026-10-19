//! Option descriptors and the keyboard/selection state machine of the native select (CMP-005
//! R9, R10, R12, R14).
//!
//! Options have a stable opaque id that survives reordering, filtering and replacement; the
//! selection callback receives the id and indices are never an identity. Highlight and selection
//! are tracked by id, so replacing the option list keeps the same option highlighted, and when
//! the highlighted option disappears the highlight moves to the selected option, else to none.

use crate::floating::stepping::{Edge, StepDirection, edge_available, step_available};
use vespertine::{Hsla, SharedString};

/// Stable identity of an option.
pub type OptionId = SharedString;

/// The optional leading element of a row (shown in the trigger too when selected).
#[derive(Clone, Debug, PartialEq)]
pub enum Leading {
    /// A glyph or short text.
    Glyph(SharedString),
    /// A color swatch.
    Swatch(Hsla),
}

/// One option of a native select.
#[derive(Clone, Debug, PartialEq)]
pub struct SelectOption {
    /// Stable id.
    pub id: OptionId,
    /// Label (also the value text of the trigger).
    pub label: SharedString,
    /// Secondary text, shown in the row only, quieter than the label.
    pub secondary: Option<SharedString>,
    /// Leading glyph or swatch.
    pub leading: Option<Leading>,
    /// Trailing keyboard-shortcut hint (display only).
    pub shortcut: Option<SharedString>,
    /// Tooltip hint (CMP-004).
    pub tooltip: Option<SharedString>,
    /// A disabled option cannot be highlighted or selected.
    pub disabled: bool,
}

impl SelectOption {
    /// An enabled option with only an id and a label.
    pub fn new(id: impl Into<OptionId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            secondary: None,
            leading: None,
            shortcut: None,
            tooltip: None,
            disabled: false,
        }
    }

    /// Adds secondary text.
    pub fn secondary(mut self, text: impl Into<SharedString>) -> Self {
        self.secondary = Some(text.into());
        self
    }

    /// Adds a leading element.
    pub fn leading(mut self, leading: Leading) -> Self {
        self.leading = Some(leading);
        self
    }

    /// Adds a shortcut hint.
    pub fn shortcut(mut self, shortcut: impl Into<SharedString>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    /// Adds a tooltip hint.
    pub fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    /// Marks the option disabled.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// Index of the option with `id`.
pub fn position_of(options: &[SelectOption], id: &str) -> Option<usize> {
    options.iter().position(|option| option.id.as_ref() == id)
}

/// The accessible name of the trigger: `label: current value` (R10). The value text is the
/// selected option's label; an unmatched selection shows the placeholder, or empty text.
pub fn trigger_name(
    label: &str,
    options: &[SelectOption],
    selected: Option<&str>,
    placeholder: Option<&str>,
) -> String {
    format!("{label}: {}", value_text(options, selected, placeholder))
}

/// The text shown in the trigger's value area.
pub fn value_text(
    options: &[SelectOption],
    selected: Option<&str>,
    placeholder: Option<&str>,
) -> String {
    selected
        .and_then(|id| position_of(options, id))
        .map(|ix| options[ix].label.to_string())
        .or_else(|| placeholder.map(str::to_string))
        .unwrap_or_default()
}

/// Geometry presets of the compact discrete selector (R14).
pub mod compact {
    /// Card width.
    pub const WIDTH: f32 = 256.0;
    /// Row height.
    pub const ROW_HEIGHT: f32 = 32.0;
    /// Gap between rows.
    pub const ROW_GAP: f32 = 2.0;
    /// Card inset on every side.
    pub const INSET: f32 = 4.0;
    /// Rows visible before the list scrolls.
    pub const MAX_VISIBLE_ROWS: usize = 7;
    /// Skeleton rows reserved by an empty or loading list.
    pub const SKELETON_ROWS: usize = 4;

    /// Height of the list: `visible rows × (32 + 2) + 2 × 4`, where visible rows is 4 when the
    /// count is 0 and `min(count, 7)` otherwise.
    pub fn list_height(count: usize) -> f32 {
        let visible = if count == 0 {
            SKELETON_ROWS
        } else {
            count.min(MAX_VISIBLE_ROWS)
        };
        visible as f32 * (ROW_HEIGHT + ROW_GAP) + 2.0 * INSET
    }
}

/// What the keyboard did to the select, for the view to execute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Not a key of the select: pass it on.
    Ignored,
    /// Consumed with no visible effect (a held key, no available option).
    Consumed,
    /// Open the menu; the highlight is already on the selected option.
    Open,
    /// The highlight moved to this option: scroll it into view.
    Highlighted(OptionId),
    /// Close the menu and commit this option: the close begins first, then the callback runs.
    Commit(OptionId),
    /// Close the menu without a change.
    Close,
}

/// Keys the select understands (R12).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectKey {
    /// Up or Ctrl-P.
    Up,
    /// Down or Ctrl-N.
    Down,
    /// Home.
    Home,
    /// End.
    End,
    /// Enter.
    Enter,
    /// Space.
    Space,
    /// Escape.
    Escape,
}

/// Highlight state of an open (or closed) select, tracked by option id.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SelectState {
    highlighted: Option<OptionId>,
    /// Keys that are currently down; a held key never repeats the open or the commit.
    held: Option<SelectKey>,
}

impl SelectState {
    /// A closed select with no highlight.
    pub fn new() -> Self {
        Self::default()
    }

    /// The highlighted option id.
    pub fn highlighted(&self) -> Option<&OptionId> {
        self.highlighted.as_ref()
    }

    /// Opening puts the highlight on the selected option when it is enabled (without advancing),
    /// otherwise on none.
    pub fn open(&mut self, options: &[SelectOption], selected: Option<&str>) {
        self.highlighted = selected
            .and_then(|id| position_of(options, id))
            .filter(|ix| !options[*ix].disabled)
            .map(|ix| options[ix].id.clone());
    }

    /// Closing forgets the highlight.
    pub fn close(&mut self) {
        self.highlighted = None;
    }

    /// Re-resolves the highlight after the option list was replaced: the same id stays
    /// highlighted; when it disappeared the highlight moves to the selected option, else none
    /// (R9).
    pub fn reconcile(&mut self, options: &[SelectOption], selected: Option<&str>) {
        let Some(id) = self.highlighted.clone() else {
            return;
        };
        let still_there = position_of(options, &id).is_some_and(|ix| !options[ix].disabled);
        if !still_there {
            self.open(options, selected);
        }
    }

    /// Pointer motion over an option moves the highlight (real motion only is the caller's
    /// concern, CMP-002 R24). Disabled options never take it.
    pub fn point_at(&mut self, options: &[SelectOption], id: &str) -> bool {
        match position_of(options, id) {
            Some(ix) if !options[ix].disabled => {
                self.highlighted = Some(options[ix].id.clone());
                true
            }
            _ => false,
        }
    }

    /// A key-up releases the held-key latch.
    pub fn key_up(&mut self, key: SelectKey) {
        if self.held == Some(key) {
            self.held = None;
        }
    }

    /// Clears the latch (focus loss).
    pub fn reset_keys(&mut self) {
        self.held = None;
    }

    /// Handles a key-down. `open` states whether the menu is open (a closing menu reads as
    /// closed, CMP-002 R15); `held` is the event's held flag. Enter and Space latch: a repeat of
    /// the same key, flagged or not, neither reopens nor commits twice (CMP-001 R2).
    pub fn key_down(
        &mut self,
        key: SelectKey,
        held: bool,
        open: bool,
        options: &[SelectOption],
        selected: Option<&str>,
    ) -> Outcome {
        let repeat = held || self.held == Some(key);
        if matches!(key, SelectKey::Enter | SelectKey::Space) {
            self.held = Some(key);
        }
        let available = |ix: usize| !options[ix].disabled;
        if !open {
            return match key {
                SelectKey::Up | SelectKey::Down => {
                    self.open(options, selected);
                    Outcome::Open
                }
                SelectKey::Enter | SelectKey::Space if repeat => Outcome::Consumed,
                SelectKey::Enter | SelectKey::Space => {
                    self.open(options, selected);
                    Outcome::Open
                }
                SelectKey::Home | SelectKey::End | SelectKey::Escape => Outcome::Ignored,
            };
        }
        let current = self
            .highlighted
            .as_deref()
            .and_then(|id| position_of(options, id));
        let target = match key {
            SelectKey::Up => step_available(options.len(), current, StepDirection::Backward, available),
            SelectKey::Down => step_available(options.len(), current, StepDirection::Forward, available),
            SelectKey::Home => edge_available(options.len(), Edge::First, available),
            SelectKey::End => edge_available(options.len(), Edge::Last, available),
            SelectKey::Enter | SelectKey::Space => {
                if repeat {
                    return Outcome::Consumed;
                }
                return match current.filter(|ix| available(*ix)) {
                    Some(ix) => Outcome::Commit(options[ix].id.clone()),
                    None => Outcome::Consumed,
                };
            }
            SelectKey::Escape => return Outcome::Close,
        };
        match target {
            Some(ix) => {
                self.highlighted = Some(options[ix].id.clone());
                Outcome::Highlighted(options[ix].id.clone())
            }
            None => Outcome::Consumed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use SelectKey::*;

    fn options() -> Vec<SelectOption> {
        vec![
            SelectOption::new("a", "Alpha"),
            SelectOption::new("b", "Beta").disabled(true),
            SelectOption::new("c", "Gamma"),
            SelectOption::new("d", "Delta"),
        ]
    }

    #[test]
    fn closed_up_and_down_open_with_the_highlight_on_the_selected_option() {
        let options = options();
        let mut state = SelectState::new();
        assert_eq!(state.key_down(Down, false, false, &options, Some("c")), Outcome::Open);
        assert_eq!(state.highlighted().map(|id| id.as_ref()), Some("c"));
        let mut state = SelectState::new();
        assert_eq!(state.key_down(Up, false, false, &options, None), Outcome::Open);
        assert_eq!(state.highlighted(), None);
        assert_eq!(state.key_down(Home, false, false, &options, None), Outcome::Ignored);
        assert_eq!(state.key_down(Escape, false, false, &options, None), Outcome::Ignored);
    }

    #[test]
    fn open_stepping_wraps_skips_disabled_and_home_end_jump() {
        let options = options();
        let mut state = SelectState::new();
        state.open(&options, Some("a"));
        assert_eq!(state.key_down(Down, false, true, &options, Some("a")), Outcome::Highlighted("c".into()));
        assert_eq!(state.key_down(Down, false, true, &options, Some("a")), Outcome::Highlighted("d".into()));
        assert_eq!(state.key_down(Down, false, true, &options, Some("a")), Outcome::Highlighted("a".into()));
        assert_eq!(state.key_down(Up, false, true, &options, Some("a")), Outcome::Highlighted("d".into()));
        assert_eq!(state.key_down(Home, false, true, &options, Some("a")), Outcome::Highlighted("a".into()));
        assert_eq!(state.key_down(End, false, true, &options, Some("a")), Outcome::Highlighted("d".into()));
    }

    #[test]
    fn enter_commits_the_stable_id_even_after_the_options_were_reordered() {
        let mut options = options();
        let mut state = SelectState::new();
        state.open(&options, Some("a"));
        state.key_down(Down, false, true, &options, Some("a"));
        assert_eq!(state.highlighted().map(|id| id.as_ref()), Some("c"));
        // The owner reorders the options while the menu is open.
        options.reverse();
        state.reconcile(&options, Some("a"));
        assert_eq!(state.highlighted().map(|id| id.as_ref()), Some("c"));
        assert_eq!(state.key_down(Enter, false, true, &options, Some("a")), Outcome::Commit("c".into()));
    }

    #[test]
    fn a_held_enter_neither_opens_nor_commits_twice() {
        let options = options();
        let mut state = SelectState::new();
        assert_eq!(state.key_down(Enter, false, false, &options, Some("a")), Outcome::Open);
        // An unflagged repeat of the same press while the menu is open does not commit.
        assert_eq!(state.key_down(Enter, false, true, &options, Some("a")), Outcome::Consumed);
        assert_eq!(state.key_down(Enter, true, true, &options, Some("a")), Outcome::Consumed);
        state.key_up(Enter);
        assert_eq!(state.key_down(Enter, false, true, &options, Some("a")), Outcome::Commit("a".into()));
        assert_eq!(state.key_down(Enter, false, true, &options, Some("a")), Outcome::Consumed);
        state.key_up(Enter);
        assert_eq!(state.key_down(Space, false, true, &options, Some("a")), Outcome::Commit("a".into()));
    }

    #[test]
    fn escape_closes_and_a_disabled_highlight_never_commits() {
        let options = options();
        let mut state = SelectState::new();
        state.open(&options, Some("b"));
        assert_eq!(state.highlighted(), None, "a disabled selection is not highlighted");
        assert_eq!(state.key_down(Enter, false, true, &options, Some("b")), Outcome::Consumed);
        assert_eq!(state.key_down(Escape, false, true, &options, Some("b")), Outcome::Close);
        let all_disabled = vec![SelectOption::new("x", "X").disabled(true)];
        assert_eq!(state.key_down(Down, false, true, &all_disabled, None), Outcome::Consumed);
    }

    #[test]
    fn a_vanished_highlight_moves_to_the_selected_option_else_none() {
        let options = options();
        let mut state = SelectState::new();
        state.open(&options, Some("a"));
        state.key_down(Down, false, true, &options, Some("a"));
        let replaced = vec![SelectOption::new("a", "Alpha"), SelectOption::new("z", "Zed")];
        state.reconcile(&replaced, Some("a"));
        assert_eq!(state.highlighted().map(|id| id.as_ref()), Some("a"));
        let none_selected = vec![SelectOption::new("q", "Q")];
        state.reconcile(&none_selected, Some("a"));
        assert_eq!(state.highlighted(), None);
    }

    #[test]
    fn trigger_name_and_value_text() {
        let options = options();
        assert_eq!(trigger_name("Greek", &options, Some("c"), Some("Pick")), "Greek: Gamma");
        assert_eq!(trigger_name("Greek", &options, Some("zz"), Some("Pick")), "Greek: Pick");
        assert_eq!(trigger_name("Greek", &options, None, None), "Greek: ");
    }

    #[test]
    fn compact_selector_heights_follow_the_formula() {
        assert_eq!(compact::list_height(0), 144.0);
        assert_eq!(compact::list_height(3), 110.0);
        assert_eq!(compact::list_height(12), 246.0);
        assert_eq!(compact::list_height(7), 246.0);
    }
}
