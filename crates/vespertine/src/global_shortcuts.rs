//! A minimal application-level registry of process-wide ("global") shortcuts.
//!
//! A global shortcut is a chord that triggers an application callback no matter
//! which element (or window) has focus. Delivery of the chord is the job of a
//! platform backend (a desktop portal, a passive key grab, a test driver): when
//! the backend sees an activation it calls [`GlobalShortcuts::trigger`]. This
//! registry only decides *whether* the chord currently fires, so that shortcut
//! recording ([`ShortcutRecorder`](crate::shortcuts::ShortcutRecorder)) can
//! suspend every registration while the user is capturing a new chord and
//! restore them afterwards, whichever way recording ends.
//!
//! Backends that hold OS-level registrations observe changes of the set of
//! active chords with [`GlobalShortcuts::observe_active`] and re-read
//! [`GlobalShortcuts::active_combos`]; suspension therefore also releases the
//! OS registration, so the recorded keystrokes reach the recorder.

use std::{cell::RefCell, rc::Rc};

use crate::{App, AsyncApp, Global, Keystroke, Subscription, subscription::SubscriberSet};

type Handler = Rc<dyn Fn(&mut App)>;

struct Registration {
    id: u64,
    combo: String,
    handler: Handler,
}

#[derive(Default)]
struct State {
    next_id: u64,
    registrations: Vec<Registration>,
    suspensions: usize,
}

/// Registry of process-wide shortcuts, stored as an application global.
///
/// Chords are identified by their normalized combo string (see
/// [`crate::shortcuts::normalize_combo`]), so a stored combo means the same
/// chord on every platform.
#[derive(Clone)]
pub struct GlobalShortcuts {
    state: Rc<RefCell<State>>,
    observers: SubscriberSet<(), Box<dyn FnMut(&mut App) -> bool>>,
    app: AsyncApp,
}

impl Global for GlobalShortcuts {}

/// Keeps a global shortcut registered; dropping it unregisters the shortcut.
#[must_use = "dropping the registration unregisters the shortcut"]
pub struct GlobalShortcutRegistration {
    registry: GlobalShortcuts,
    id: u64,
}

/// Suspends every global shortcut while it is alive; dropping it restores
/// them (unless another suspension is still alive).
#[must_use = "dropping the suspension restores the global shortcuts"]
pub struct GlobalShortcutSuspension {
    registry: GlobalShortcuts,
}

impl GlobalShortcuts {
    /// The registry of this application, created on first use.
    pub fn get(cx: &mut App) -> GlobalShortcuts {
        if let Some(registry) = cx.try_global::<GlobalShortcuts>() {
            return registry.clone();
        }
        let registry = GlobalShortcuts {
            state: Rc::default(),
            observers: SubscriberSet::new(),
            app: cx.to_async(),
        };
        cx.set_global(registry.clone());
        registry
    }

    /// Registers `handler` for the chord `combo`, a normalized combo string.
    /// Several registrations for one chord all run when it triggers.
    pub fn register(
        cx: &mut App,
        combo: impl Into<String>,
        handler: impl Fn(&mut App) + 'static,
    ) -> GlobalShortcutRegistration {
        let registry = Self::get(cx);
        let id = {
            let mut state = registry.state.borrow_mut();
            let id = state.next_id;
            state.next_id += 1;
            state.registrations.push(Registration {
                id,
                combo: combo.into(),
                handler: Rc::new(handler),
            });
            id
        };
        registry.notify(cx);
        GlobalShortcutRegistration { registry, id }
    }

    /// Suspends all registrations until the returned guard is dropped.
    pub fn suspend(cx: &mut App) -> GlobalShortcutSuspension {
        let registry = Self::get(cx);
        {
            let mut state = registry.state.borrow_mut();
            state.suspensions += 1;
        }
        registry.notify(cx);
        GlobalShortcutSuspension { registry }
    }

    /// Whether registrations are currently suspended.
    pub fn is_suspended(cx: &mut App) -> bool {
        Self::get(cx).state.borrow().suspensions > 0
    }

    /// The combos that currently fire when triggered: every registered combo,
    /// or none while suspended. Backends holding OS-level registrations
    /// mirror this set.
    pub fn active_combos(cx: &mut App) -> Vec<String> {
        let registry = Self::get(cx);
        let state = registry.state.borrow();
        if state.suspensions > 0 {
            return Vec::new();
        }
        let mut combos: Vec<String> = Vec::new();
        for registration in &state.registrations {
            if !combos.contains(&registration.combo) {
                combos.push(registration.combo.clone());
            }
        }
        combos
    }

    /// Calls `callback` whenever the set of active combos may have changed
    /// (a registration was added or removed, or suspension began or ended).
    /// A change caused by dropping a registration or a suspension guard is
    /// reported from the foreground executor shortly after the drop, because
    /// guards have no application context to run it with.
    pub fn observe_active(
        cx: &mut App,
        mut callback: impl FnMut(&mut App) + 'static,
    ) -> Subscription {
        let registry = Self::get(cx);
        let (subscription, activate) = registry.observers.insert(
            (),
            Box::new(move |cx| {
                callback(cx);
                true
            }),
        );
        activate();
        subscription
    }

    /// Reports that the user pressed the chord `combo`. Runs the handlers
    /// registered for it and returns whether any ran; nothing runs while the
    /// registry is suspended.
    pub fn trigger(cx: &mut App, combo: &str) -> bool {
        let registry = Self::get(cx);
        let handlers: Vec<Handler> = {
            let state = registry.state.borrow();
            if state.suspensions > 0 {
                return false;
            }
            state
                .registrations
                .iter()
                .filter(|registration| registration.combo == combo)
                .map(|registration| registration.handler.clone())
                .collect()
        };
        for handler in &handlers {
            handler(cx);
        }
        !handlers.is_empty()
    }

    /// Like [`Self::trigger`] for a keystroke, normalizing it to its combo.
    /// Returns `false` for keystrokes that have no combo (bare modifiers).
    pub fn trigger_keystroke(cx: &mut App, keystroke: &Keystroke) -> bool {
        match crate::shortcuts::normalize_combo(keystroke) {
            Some(combo) => Self::trigger(cx, &combo),
            None => false,
        }
    }

    fn notify(&self, cx: &mut App) {
        self.observers.clone().retain(&(), |observer| observer(cx));
    }

    /// Reports a change made by a guard's `Drop`, which has no application
    /// context: the notification runs from the foreground executor.
    fn defer_notification(&self) {
        let registry = self.clone();
        let app = self.app.clone();
        self.app
            .foreground_executor()
            .spawn(async move { app.update(|cx| registry.notify(cx)) })
            .detach();
    }
}

impl Drop for GlobalShortcutRegistration {
    fn drop(&mut self) {
        {
            let mut state = self.registry.state.borrow_mut();
            state.registrations.retain(|entry| entry.id != self.id);
        }
        self.registry.defer_notification();
    }
}

impl Drop for GlobalShortcutSuspension {
    fn drop(&mut self) {
        {
            let mut state = self.registry.state.borrow_mut();
            state.suspensions = state.suspensions.saturating_sub(1);
        }
        self.registry.defer_notification();
    }
}
