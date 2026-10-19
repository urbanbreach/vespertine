//! The controlled-state helper of the compound parts (CMP-002 R12).
//!
//! An absent controlled value means the state is uncontrolled and lives in the helper; the
//! default only initializes the internal state; the change notification runs only for a
//! requested value that differs from the current one; a controlled request never overwrites the
//! supplied value, the owner decides by passing a new one.

/// A value that is either controlled by its owner or held internally.
#[derive(Clone, Debug)]
pub struct ControlledState<T> {
    controlled: Option<T>,
    internal: T,
}

impl<T: Clone + PartialEq> ControlledState<T> {
    /// `controlled` is the owner-supplied value (`None`: uncontrolled); `default` initializes the
    /// internal state and is ignored once a controlled value exists.
    pub fn new(controlled: Option<T>, default: T) -> Self {
        Self {
            controlled,
            internal: default,
        }
    }

    /// The current value: the controlled one when supplied, otherwise the internal one.
    pub fn value(&self) -> &T {
        self.controlled.as_ref().unwrap_or(&self.internal)
    }

    /// Whether the owner supplies the value.
    pub fn is_controlled(&self) -> bool {
        self.controlled.is_some()
    }

    /// The owner passes its value again (each render); `None` returns to uncontrolled, keeping
    /// the last internal value.
    pub fn set_controlled(&mut self, controlled: Option<T>) {
        self.controlled = controlled;
    }

    /// Requests `next`. Nothing happens when it equals the current value. Otherwise `notify` runs
    /// with the requested value, and uncontrolled state takes it; controlled state keeps the
    /// supplied value. Returns whether `notify` ran.
    pub fn request(&mut self, next: T, notify: impl FnOnce(&T)) -> bool {
        if *self.value() == next {
            return false;
        }
        notify(&next);
        if self.controlled.is_none() {
            self.internal = next;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn undefined_value_is_uncontrolled_and_default_only_initializes() {
        let mut state = ControlledState::new(None, 3);
        assert!(!state.is_controlled());
        assert_eq!(*state.value(), 3);
        let seen = RefCell::new(Vec::new());
        assert!(state.request(5, |v| seen.borrow_mut().push(*v)));
        assert_eq!(*state.value(), 5);
        assert_eq!(*seen.borrow(), vec![5]);
        // The default does not come back.
        state.set_controlled(Some(9));
        assert_eq!(*state.value(), 9);
        state.set_controlled(None);
        assert_eq!(*state.value(), 5);
    }

    #[test]
    fn identical_request_does_not_notify() {
        let mut state = ControlledState::new(None, 1);
        let calls = RefCell::new(0);
        assert!(!state.request(1, |_| *calls.borrow_mut() += 1));
        assert_eq!(*calls.borrow(), 0);
        let mut controlled = ControlledState::new(Some(2), 1);
        assert!(!controlled.request(2, |_| *calls.borrow_mut() += 1));
        assert_eq!(*calls.borrow(), 0);
    }

    #[test]
    fn controlled_request_reports_but_leaves_the_supplied_value() {
        let mut state = ControlledState::new(Some("a"), "default");
        assert!(state.is_controlled());
        let seen = RefCell::new(None);
        assert!(state.request("b", |v| *seen.borrow_mut() = Some(*v)));
        assert_eq!(*seen.borrow(), Some("b"));
        assert_eq!(*state.value(), "a");
        state.set_controlled(Some("b"));
        assert_eq!(*state.value(), "b");
    }
}
