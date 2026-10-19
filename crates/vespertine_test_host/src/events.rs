//! Events that native elements hand to the framework adapter, and selection-change signals.

use crate::types::NativeEvent;
use std::{cell::RefCell, collections::VecDeque, rc::Rc};
use vespertine::{App, Global, Subscription};

type SelectionListener = Rc<RefCell<Box<dyn FnMut()>>>;

#[derive(Default)]
struct State {
    queue: VecDeque<NativeEvent>,
    selection_listeners: Vec<(usize, SelectionListener)>,
    next_listener_id: usize,
}

/// The native event queue and selection-change signal of an application. An application global
/// installed by the host; element implementations push into it with [`HostEvents::emit`] and
/// [`HostEvents::notify_selection_changed`].
#[derive(Clone, Default)]
pub struct HostEvents {
    state: Rc<RefCell<State>>,
}

impl Global for HostEvents {}

impl HostEvents {
    /// Queues an event for the framework adapter. Does nothing when the application has no
    /// host event queue.
    pub fn emit(cx: &App, event: NativeEvent) {
        if let Some(events) = cx.try_global::<HostEvents>() {
            events.state.borrow_mut().queue.push_back(event);
        }
    }

    /// Signals that the document selection changed, notifying every selection subscriber.
    pub fn notify_selection_changed(cx: &App) {
        if let Some(events) = cx.try_global::<HostEvents>() {
            events.notify_selection();
        }
    }

    pub(crate) fn notify_selection(&self) {
        let listeners: Vec<SelectionListener> = self
            .state
            .borrow()
            .selection_listeners
            .iter()
            .map(|(_, listener)| listener.clone())
            .collect();
        for listener in listeners {
            (listener.borrow_mut())();
        }
    }

    pub(crate) fn drain(&self) -> Vec<NativeEvent> {
        self.state.borrow_mut().queue.drain(..).collect()
    }

    pub(crate) fn subscribe_selection(&self, listener: impl FnMut() + 'static) -> Subscription {
        let mut state = self.state.borrow_mut();
        let id = state.next_listener_id;
        state.next_listener_id += 1;
        state
            .selection_listeners
            .push((id, Rc::new(RefCell::new(Box::new(listener)))));
        drop(state);
        let state = Rc::downgrade(&self.state);
        Subscription::new(move || {
            if let Some(state) = state.upgrade() {
                state
                    .borrow_mut()
                    .selection_listeners
                    .retain(|(existing, _)| *existing != id);
            }
        })
    }
}
