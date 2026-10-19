//! Reentrant delivery of desktop setting changes (PLT-003).
//!
//! Appearance and titlebar button-layout changes arrive from the desktop
//! portal and are announced to every window. A window's callback may render,
//! query the settings or open and close windows, so no exclusive borrow of the
//! client may be outstanding while callbacks run.

use std::cell::RefCell;

/// Delivers one setting change.
///
/// 1. `update` stores the new value in the client state (so a query made from a
///    notification callback already returns it),
/// 2. `snapshot` clones the currently registered window handles,
/// 3. the client borrow is released,
/// 4. `notify` runs once per snapshotted handle.
///
/// The snapshot holds shared handles rather than references into the registry,
/// so callbacks that open or close windows cannot invalidate the iteration. A
/// window that was closed by an earlier callback is notified harmlessly through
/// its still-valid handle.
pub(crate) fn deliver_setting_change<State, Handle>(
    client: &RefCell<State>,
    update: impl FnOnce(&mut State),
    snapshot: impl FnOnce(&State) -> Vec<Handle>,
    notify: impl FnMut(Handle),
) {
    let handles = {
        let mut state = client.borrow_mut();
        update(&mut state);
        snapshot(&state)
    };
    handles.into_iter().for_each(notify);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    /// A stand-in for a client: a registry of windows plus the common setting.
    struct FakeClient {
        appearance: u32,
        button_layout: &'static str,
        windows: Vec<Rc<FakeWindow>>,
    }

    struct FakeWindow {
        id: u32,
        notified: Cell<u32>,
        seen_value: Cell<u32>,
    }

    fn fake_window(id: u32) -> Rc<FakeWindow> {
        Rc::new(FakeWindow {
            id,
            notified: Cell::new(0),
            seen_value: Cell::new(u32::MAX),
        })
    }

    fn client_with_windows(count: u32) -> Rc<RefCell<FakeClient>> {
        Rc::new(RefCell::new(FakeClient {
            appearance: 0,
            button_layout: "default",
            windows: (0..count).map(fake_window).collect(),
        }))
    }

    fn deliver_appearance(
        client: &Rc<RefCell<FakeClient>>,
        value: u32,
        on_window: impl Fn(&Rc<RefCell<FakeClient>>, &FakeWindow),
    ) {
        deliver_setting_change(
            client,
            |state| state.appearance = value,
            |state| state.windows.clone(),
            |window| {
                window.notified.set(window.notified.get() + 1);
                on_window(client, &window);
            },
        );
    }

    #[test]
    fn callbacks_can_borrow_the_client_mutably_and_see_the_new_value() {
        let client = client_with_windows(3);
        deliver_appearance(&client, 7, |client, window| {
            // Rendering or querying settings from a callback needs the client.
            let state = client.borrow_mut();
            window.seen_value.set(state.appearance);
        });
        for window in &client.borrow().windows {
            assert_eq!(window.notified.get(), 1);
            assert_eq!(window.seen_value.get(), 7);
        }
    }

    #[test]
    fn button_layout_updates_before_notifying() {
        let client = client_with_windows(2);
        let seen = Rc::new(RefCell::new(Vec::new()));
        deliver_setting_change(
            &client,
            |state| state.button_layout = "close:",
            |state| state.windows.clone(),
            |window| {
                seen.borrow_mut()
                    .push((window.id, client.borrow_mut().button_layout));
            },
        );
        assert_eq!(*seen.borrow(), vec![(0, "close:"), (1, "close:")]);
    }

    #[test]
    fn opening_a_window_from_a_callback_does_not_invalidate_iteration() {
        let client = client_with_windows(2);
        let opened = Cell::new(false);
        deliver_appearance(&client, 1, |client, _| {
            if !opened.replace(true) {
                client.borrow_mut().windows.push(fake_window(99));
            }
        });
        let state = client.borrow();
        assert_eq!(state.windows.len(), 3);
        // The snapshot was taken before the callback: the new window is not
        // notified for this change, the original two each are exactly once.
        assert_eq!(state.windows[0].notified.get(), 1);
        assert_eq!(state.windows[1].notified.get(), 1);
        assert_eq!(state.windows[2].notified.get(), 0);
    }

    #[test]
    fn closing_a_window_from_a_callback_is_harmless() {
        let client = client_with_windows(3);
        deliver_appearance(&client, 1, |client, window| {
            if window.id == 0 {
                client.borrow_mut().windows.retain(|w| w.id != 1);
            }
        });
        let state = client.borrow();
        assert_eq!(state.windows.len(), 2);
        assert_eq!(state.windows[0].notified.get(), 1);
        assert_eq!(state.windows[1].id, 2);
        assert_eq!(state.windows[1].notified.get(), 1);
    }
}
