//! Action descriptors and the dismissal policy of the native dialog (CMP-003 R10, R12).
//!
//! The model is pure: the view asks it what an activation, an Escape or an outside press means
//! and executes the answer. Dismissal through Escape or an outside press is a cancel; it never
//! runs the confirm action.

use vespertine::SharedString;

/// The role of an action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionKind {
    /// The default action: Enter in a single-line field activates it when it is enabled.
    Primary,
    /// A secondary action.
    Secondary,
    /// A destructive action.
    Destructive,
}

/// One action of a dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DialogAction {
    /// Stable identifier.
    pub id: SharedString,
    /// Button label.
    pub label: SharedString,
    /// Role.
    pub kind: ActionKind,
    /// A disabled action cannot be activated by pointer, keyboard or the default-action key.
    pub disabled: bool,
    /// A busy action exposes its busy state, cannot be activated again and keeps the dialog open.
    pub busy: bool,
}

impl DialogAction {
    /// An enabled, idle action.
    pub fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        kind: ActionKind,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            kind,
            disabled: false,
            busy: false,
        }
    }

    /// Sets the disabled flag.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Sets the busy flag.
    pub fn busy(mut self, busy: bool) -> Self {
        self.busy = busy;
        self
    }

    /// Whether the action can be activated: neither disabled nor busy (R12).
    pub fn is_activatable(&self) -> bool {
        !self.disabled && !self.busy
    }
}

/// What dismissal through Escape and an outside press is allowed to do (R10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DismissalPolicy {
    /// An outside press (a press on the backdrop) dismisses.
    pub outside_press: bool,
    /// Escape dismisses.
    pub escape: bool,
}

impl Default for DismissalPolicy {
    fn default() -> Self {
        Self {
            outside_press: true,
            escape: true,
        }
    }
}

impl DismissalPolicy {
    /// A destructive confirmation: only an explicit action closes it.
    pub fn explicit_only() -> Self {
        Self {
            outside_press: false,
            escape: false,
        }
    }
}

/// What a dismissal attempt does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dismissal {
    /// Run the cancel callback and close.
    Cancel,
    /// Do nothing visible. An Escape that is switched off is still consumed (R10), so it never
    /// reaches the background; `consumed` says whether the event is consumed.
    Ignored {
        /// Whether the event must be consumed.
        consumed: bool,
    },
}

/// What activating an action does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Activation {
    /// The activated action.
    pub id: SharedString,
    /// Its role: primary actions run the confirm callback, the others their own.
    pub kind: ActionKind,
}

/// The dialog's actions and policy.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DialogModel {
    /// The actions in display order.
    pub actions: Vec<DialogAction>,
    /// Dismissal policy.
    pub policy: DismissalPolicy,
}

impl DialogModel {
    /// A model with the given actions and the default policy.
    pub fn new(actions: Vec<DialogAction>) -> Self {
        Self {
            actions,
            policy: DismissalPolicy::default(),
        }
    }

    /// Sets the dismissal policy.
    pub fn policy(mut self, policy: DismissalPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// An action by identifier.
    pub fn action(&self, id: &str) -> Option<&DialogAction> {
        self.actions.iter().find(|action| action.id.as_ref() == id)
    }

    /// Activates an action by pointer or keyboard: `None` when it is disabled, busy or unknown.
    pub fn activate(&self, id: &str) -> Option<Activation> {
        let action = self.action(id)?;
        action.is_activatable().then(|| Activation {
            id: action.id.clone(),
            kind: action.kind,
        })
    }

    /// The default-action key (Enter in a single-line field): the first primary action when it is
    /// enabled, otherwise nothing happens (R12).
    pub fn activate_default(&self) -> Option<Activation> {
        let primary = self
            .actions
            .iter()
            .find(|action| action.kind == ActionKind::Primary)?;
        self.activate(&primary.id)
    }

    /// A busy primary action keeps the dialog open and exposes the busy state.
    pub fn primary_busy(&self) -> bool {
        self.actions
            .iter()
            .any(|action| action.kind == ActionKind::Primary && action.busy)
    }

    /// Escape: a cancel when allowed; consumed either way (R10).
    pub fn escape(&self) -> Dismissal {
        if self.policy.escape {
            Dismissal::Cancel
        } else {
            Dismissal::Ignored { consumed: true }
        }
    }

    /// A press on the backdrop (outside the popup).
    pub fn outside_press(&self) -> Dismissal {
        if self.policy.outside_press {
            Dismissal::Cancel
        } else {
            Dismissal::Ignored { consumed: false }
        }
    }
}

/// Tab inside a modal popup: the next index among `count` focusable descendants, wrapping from
/// the last to the first, and from the first to the last for Shift-Tab (R7). With no current
/// index forward enters at the first and reverse at the last; `None` when there is nothing to
/// focus.
pub fn next_tab_stop(count: usize, current: Option<usize>, reverse: bool) -> Option<usize> {
    if count == 0 {
        return None;
    }
    Some(match (current.filter(|ix| *ix < count), reverse) {
        (Some(ix), false) => (ix + 1) % count,
        (Some(ix), true) => (ix + count - 1) % count,
        (None, false) => 0,
        (None, true) => count - 1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn confirmation(policy: DismissalPolicy) -> DialogModel {
        DialogModel::new(vec![
            DialogAction::new("cancel", "Cancel", ActionKind::Secondary),
            DialogAction::new("delete", "Delete", ActionKind::Primary),
        ])
        .policy(policy)
    }

    #[test]
    fn a_disabled_or_busy_action_cannot_be_activated_by_any_path() {
        let model = DialogModel::new(vec![
            DialogAction::new("ok", "OK", ActionKind::Primary).disabled(true),
            DialogAction::new("other", "Other", ActionKind::Secondary).busy(true),
        ]);
        assert_eq!(model.activate("ok"), None);
        assert_eq!(model.activate("other"), None);
        assert_eq!(
            model.activate_default(),
            None,
            "Enter does nothing when the primary is disabled"
        );
        assert_eq!(model.activate("missing"), None);
    }

    #[test]
    fn enter_activates_the_enabled_primary_action() {
        let model = confirmation(DismissalPolicy::default());
        let activation = model.activate_default().expect("primary");
        assert_eq!(activation.id.as_ref(), "delete");
        assert_eq!(activation.kind, ActionKind::Primary);
    }

    #[test]
    fn a_busy_primary_keeps_the_dialog_open_and_reports_busy() {
        let model = DialogModel::new(vec![
            DialogAction::new("save", "Save", ActionKind::Primary).busy(true),
        ]);
        assert!(model.primary_busy());
        assert_eq!(model.activate("save"), None);
        assert_eq!(model.activate_default(), None);
    }

    #[test]
    fn escape_and_outside_press_cancel_by_default() {
        let model = confirmation(DismissalPolicy::default());
        assert_eq!(model.escape(), Dismissal::Cancel);
        assert_eq!(model.outside_press(), Dismissal::Cancel);
    }

    #[test]
    fn a_destructive_confirmation_ignores_but_consumes_escape_and_ignores_outside_presses() {
        let model = confirmation(DismissalPolicy::explicit_only());
        assert_eq!(model.escape(), Dismissal::Ignored { consumed: true });
        assert_eq!(
            model.outside_press(),
            Dismissal::Ignored { consumed: false }
        );
        assert!(
            model.activate("cancel").is_some(),
            "the explicit buttons still work"
        );
    }

    #[test]
    fn tab_stops_wrap_in_both_directions() {
        assert_eq!(next_tab_stop(3, Some(2), false), Some(0));
        assert_eq!(next_tab_stop(3, Some(0), true), Some(2));
        assert_eq!(next_tab_stop(3, Some(1), false), Some(2));
        assert_eq!(next_tab_stop(3, None, false), Some(0));
        assert_eq!(next_tab_stop(3, None, true), Some(2));
        assert_eq!(next_tab_stop(0, None, false), None);
        assert_eq!(next_tab_stop(1, Some(0), false), Some(0));
    }
}
