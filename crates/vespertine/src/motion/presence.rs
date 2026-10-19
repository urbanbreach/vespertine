//! Exit lifecycle and presence management (ANM-002 R12, ANM-003).
//!
//! [`PresenceLifecycle`] owns the open / closing / unmounted state of one component.
//! [`PresenceManager`] is the framework-neutral presence tracking logic that retained-tree
//! adapters (React `AnimatePresence`, Solid `AnimatePresence`) call: it keeps removed children
//! mounted until their native exit completion (and every registered participant) finishes,
//! rejects stale completion generations, and aggregates exits.

use std::{collections::HashSet, hash::Hash, rc::Rc, time::Duration};

use crate::{AnyElement, Context, IntoElement, div};

use super::PaintScope;

/// The lifecycle state of a component with an exit animation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresenceState {
    /// Mounted and interactive.
    Open,
    /// Being closed: still painted but inert until the exit finishes.
    Closing,
    /// Removed.
    Unmounted,
}

/// Lifecycle ownership of one closable component (ANM-002 R12).
///
/// A closing component stays painted but non-interactive (no hitboxes, focus, pointer or
/// accessibility actions) until its native exit completion arrives, or, as a fallback when no
/// completion is observed, until the exit duration has elapsed. Reopening while closing returns
/// to [`PresenceState::Open`].
#[derive(Clone, Debug)]
pub struct PresenceLifecycle {
    state: PresenceState,
    generation: u64,
    exit_duration: Duration,
    closing_since: Option<Duration>,
}

impl PresenceLifecycle {
    /// An open lifecycle whose fallback unmount fires `exit_duration` after closing.
    pub fn new(exit_duration: Duration) -> Self {
        Self {
            state: PresenceState::Open,
            generation: 0,
            exit_duration,
            closing_since: None,
        }
    }

    /// The current state.
    pub fn state(&self) -> PresenceState {
        self.state
    }

    /// The current completion generation: exit tracks must report this generation for their
    /// completion to unmount the component.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether the component is painted (open or closing).
    pub fn is_painted(&self) -> bool {
        self.state != PresenceState::Unmounted
    }

    /// Whether the component accepts input (open).
    pub fn is_interactive(&self) -> bool {
        self.state == PresenceState::Open
    }

    /// Whether the component is present (open); the value an exit track's `present` flag takes.
    pub fn is_present(&self) -> bool {
        self.state == PresenceState::Open
    }

    /// Starts closing at time `now`. Returns the generation the exit track must carry. Does
    /// nothing (returns the current generation) unless the component is open.
    pub fn close(&mut self, now: Duration) -> u64 {
        if self.state == PresenceState::Open {
            self.state = PresenceState::Closing;
            self.generation += 1;
            self.closing_since = Some(now);
        }
        self.generation
    }

    /// Opens (or reopens while closing, or after unmounting). The generation changes so a
    /// completion of the cancelled exit is rejected.
    pub fn open(&mut self) {
        if self.state != PresenceState::Open {
            self.state = PresenceState::Open;
            self.generation += 1;
            self.closing_since = None;
        }
    }

    /// Delivers a native completion report. Unmounts a closing component when `generation` is
    /// current; stale generations are rejected. Returns whether the component unmounted.
    pub fn complete(&mut self, generation: u64) -> bool {
        if self.state == PresenceState::Closing && generation == self.generation {
            self.state = PresenceState::Unmounted;
            self.closing_since = None;
            true
        } else {
            false
        }
    }

    /// The fallback unmount: unmounts a closing component once the exit duration has elapsed at
    /// `now` without a completion having been observed. Returns whether it unmounted.
    pub fn poll_fallback(&mut self, now: Duration) -> bool {
        match self.closing_since {
            Some(since)
                if self.state == PresenceState::Closing
                    && now.saturating_sub(since) >= self.exit_duration =>
            {
                self.state = PresenceState::Unmounted;
                self.closing_since = None;
                true
            }
            _ => false,
        }
    }

    /// Wraps `child` according to the state: unchanged when open, painted but inert when closing
    /// and absent when unmounted. Siblings are unaffected because layout is unchanged.
    pub fn render(&self, child: impl IntoElement + 'static) -> AnyElement {
        match self.state {
            PresenceState::Open => child.into_any_element(),
            PresenceState::Closing => PaintScope::new(child).inert(true).into_any_element(),
            PresenceState::Unmounted => div().into_any_element(),
        }
    }
}

/// Runs `on_elapsed` on the entity after `duration` of the executor's clock: the delayed fallback
/// of ANM-002 R12. The callback should call [`PresenceLifecycle::poll_fallback`].
pub fn spawn_fallback_timer<T: 'static>(
    duration: Duration,
    cx: &mut Context<T>,
    on_elapsed: impl FnOnce(&mut T, &mut Context<T>) + 'static,
) {
    cx.spawn(async move |this, cx| {
        cx.background_executor().timer(duration).await;
        this.update(cx, |this, cx| on_elapsed(this, cx)).ok();
    })
    .detach();
}

/// A callback an exiting participant calls when it has finished.
pub type SafeToRemove = Rc<dyn Fn()>;

/// The presence context a child sees: whether it is present and, while exiting, how to report it
/// is safe to remove (ANM-003 R7).
#[derive(Clone)]
pub struct PresenceContext {
    /// Whether the child is present (not exiting).
    pub is_present: bool,
    /// The removal callback; `None` outside a presence context.
    pub safe_to_remove: Option<SafeToRemove>,
}

impl Default for PresenceContext {
    /// The values outside any presence context: present, with no removal callback.
    fn default() -> Self {
        Self {
            is_present: true,
            safe_to_remove: None,
        }
    }
}

/// Whether the child is present: `true` outside a presence context.
pub fn use_is_present(context: Option<&PresenceContext>) -> bool {
    context.is_none_or(|context| context.is_present)
}

/// Present flag and removal callback: `(true, None)` outside a presence context.
pub fn use_presence(context: Option<&PresenceContext>) -> (bool, Option<SafeToRemove>) {
    match context {
        Some(context) => (context.is_present, context.safe_to_remove.clone()),
        None => (true, None),
    }
}

/// Identifies a registered presence participant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ParticipantId(pub(crate) u64);

/// Where exiting children appear in the output order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresenceOrder {
    /// Each exiting child stays at its previous sibling position (React `AnimatePresence`).
    Positional,
    /// Current children come first in their current order, followed by the retained exits in
    /// their previous relative order (Solid `AnimatePresence`).
    ExitsAfterCurrent,
}

/// How exits are released.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemovalMode {
    /// All exiting children are removed together once every one of them has completed (React).
    Aggregate,
    /// Each exiting child is removed as soon as it completes (Solid).
    Individual,
}

/// The framework flavor of a [`PresenceManager`]: output order plus removal policy.
///
/// [`PresenceFlavor::REACT`] and [`PresenceFlavor::SOLID`] are the two adapters' behaviors; any
/// other combination can be built with the public fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PresenceFlavor {
    /// Where exits appear in the output.
    pub order: PresenceOrder,
    /// How exits are released.
    pub removal: RemovalMode,
}

impl PresenceFlavor {
    /// React: exits keep their sibling positions and are removed together once all completed.
    pub const REACT: PresenceFlavor = PresenceFlavor {
        order: PresenceOrder::Positional,
        removal: RemovalMode::Aggregate,
    };
    /// Solid: current children precede retained exits, each exit is removed as it completes.
    pub const SOLID: PresenceFlavor = PresenceFlavor {
        order: PresenceOrder::ExitsAfterCurrent,
        removal: RemovalMode::Individual,
    };
}

/// One current child supplied to [`PresenceManager::reconcile`].
#[derive(Clone, Debug)]
pub struct PresenceChild<K, T = ()> {
    /// The child's key (the empty string for absent keys in React).
    pub key: K,
    /// Whether the child has an exit target. A removed child without one is removable at once.
    pub has_exit: bool,
    /// A digest of the motion target, transition and presence-relevant props. A change gives the
    /// child a new completion generation.
    pub signature: u64,
    /// The child's latest props, returned with the render items.
    pub payload: T,
}

/// A child to render, in output order.
#[derive(Clone, Debug, PartialEq)]
pub struct PresenceItem<K, T = ()> {
    /// The child's key.
    pub key: K,
    /// `false` while the child is exiting (its exit target replaces animate).
    pub present: bool,
    /// The completion generation its native track must carry.
    pub generation: u64,
    /// Whether the enter animation is suppressed (`initial: false`, first-render children only).
    pub suppress_enter: bool,
    /// The latest payload (the last seen one for an exiting child).
    pub payload: T,
}

/// What a presence operation asks its caller to do, in order: call user code for the accepted
/// motion completions, then fire the root exit callback, with removals already applied.
#[derive(Clone, Debug, PartialEq)]
pub struct PresenceOutcome<K> {
    /// `(key, generation)` of completions accepted for the current generation: the caller must
    /// run the user completion callbacks first, before the safe removal takes effect.
    pub motion_callbacks: Vec<(K, u64)>,
    /// Keys removed from the render list by this operation.
    pub removed: Vec<K>,
    /// Whether the last exiting child was removed, so the root exit-complete callback fires.
    pub exit_complete: bool,
}

impl<K> Default for PresenceOutcome<K> {
    fn default() -> Self {
        Self {
            motion_callbacks: Vec::new(),
            removed: Vec::new(),
            exit_complete: false,
        }
    }
}

struct Participant {
    id: ParticipantId,
    done: bool,
}

struct Entry<K, T> {
    key: K,
    payload: T,
    present: bool,
    has_exit: bool,
    signature: u64,
    generation: u64,
    suppress_enter: bool,
    native_done: bool,
    participants: Vec<Participant>,
}

impl<K, T> Entry<K, T> {
    fn begin_cycle(&mut self, generation: u64) {
        self.generation = generation;
        self.native_done = false;
        for participant in &mut self.participants {
            participant.done = false;
        }
    }

    fn exit_ready(&self) -> bool {
        (!self.has_exit || self.native_done) && self.participants.iter().all(|p| p.done)
    }
}

/// Presence tracking over keyed children (ANM-003).
///
/// Removed keyed children stay in the output while exiting (placed according to the
/// [`PresenceFlavor`]), current children use their latest payload, a returning key cancels its
/// exit, and completions carry generations so obsolete ones can never remove a newer target.
pub struct PresenceManager<K, T = ()> {
    entries: Vec<Entry<K, T>>,
    initial: bool,
    flavor: PresenceFlavor,
    first_render: bool,
    next_generation: u64,
    next_participant: u64,
    check_queued: bool,
}

impl<K: Clone + Eq + Hash, T: Clone> PresenceManager<K, T> {
    /// A manager. With `initial == false`, children present on the first reconcile have their
    /// enter animation suppressed; children added later animate normally.
    pub fn new(initial: bool, flavor: PresenceFlavor) -> Self {
        Self {
            entries: Vec::new(),
            initial,
            flavor,
            first_render: true,
            next_generation: 1,
            next_participant: 1,
            check_queued: false,
        }
    }

    fn new_generation(&mut self) -> u64 {
        let generation = self.next_generation;
        self.next_generation += 1;
        generation
    }

    /// The items to render, in output order.
    pub fn items(&self) -> Vec<PresenceItem<K, T>> {
        self.entries
            .iter()
            .map(|entry| PresenceItem {
                key: entry.key.clone(),
                present: entry.present,
                generation: entry.generation,
                suppress_enter: entry.suppress_enter,
                payload: entry.payload.clone(),
            })
            .collect()
    }

    /// Whether any child is exiting.
    pub fn has_exiting(&self) -> bool {
        self.entries.iter().any(|entry| !entry.present)
    }

    /// Reconciles the current children (duplicate keys keep the first) and returns the render
    /// items. Newly removed children begin exiting and keep their previous position; a child
    /// whose key returns while exiting cancels its exit.
    pub fn reconcile(&mut self, children: Vec<PresenceChild<K, T>>) -> Vec<PresenceItem<K, T>> {
        let mut seen = HashSet::new();
        let children: Vec<_> = children
            .into_iter()
            .filter(|child| seen.insert(child.key.clone()))
            .collect();

        let mut old: Vec<Option<Entry<K, T>>> = std::mem::take(&mut self.entries)
            .into_iter()
            .map(Some)
            .collect();

        // Current children, reusing existing entries.
        let mut next: Vec<Entry<K, T>> = Vec::with_capacity(children.len());
        for child in children {
            let existing = old
                .iter_mut()
                .find(|slot| slot.as_ref().is_some_and(|entry| entry.key == child.key))
                .and_then(Option::take);
            let entry = match existing {
                Some(mut entry) => {
                    let reentered = !entry.present;
                    let changed = entry.signature != child.signature;
                    entry.present = true;
                    entry.has_exit = child.has_exit;
                    entry.signature = child.signature;
                    entry.payload = child.payload;
                    if reentered || changed {
                        let generation = self.new_generation();
                        entry.begin_cycle(generation);
                    }
                    entry
                }
                None => {
                    let generation = self.new_generation();
                    Entry {
                        key: child.key,
                        payload: child.payload,
                        present: true,
                        has_exit: child.has_exit,
                        signature: child.signature,
                        generation,
                        suppress_enter: self.first_render && !self.initial,
                        native_done: false,
                        participants: Vec::new(),
                    }
                }
            };
            next.push(entry);
        }

        // Removed children begin exiting. React keeps each at its previous sibling position (in
        // increasing order); Solid appends them after all current children in their previous
        // relative order.
        for (index, slot) in old.into_iter().enumerate() {
            let Some(mut entry) = slot else { continue };
            if entry.present {
                entry.present = false;
                let generation = self.new_generation();
                entry.begin_cycle(generation);
            }
            match self.flavor.order {
                PresenceOrder::Positional => next.insert(index.min(next.len()), entry),
                PresenceOrder::ExitsAfterCurrent => next.push(entry),
            }
        }
        self.entries = next;
        self.first_render = false;
        self.items()
    }

    /// Registers a participant of the child's presence cycle. The child completes only after
    /// every currently registered participant finishes the same cycle. Returns `None` for an
    /// unknown key.
    pub fn register_participant(&mut self, key: &K) -> Option<ParticipantId> {
        let id = ParticipantId(self.next_participant);
        let entry = self.entries.iter_mut().find(|entry| &entry.key == key)?;
        self.next_participant += 1;
        entry.participants.push(Participant { id, done: false });
        Some(id)
    }

    /// Unregisters a participant. Unregistering the last participant of an exiting child queues
    /// a completion check: the caller must run [`PresenceManager::run_queued_check`] afterwards
    /// (the equivalent of a microtask). Returns whether a check was queued.
    pub fn unregister_participant(&mut self, key: &K, id: ParticipantId) -> bool {
        let Some(entry) = self.entries.iter_mut().find(|entry| &entry.key == key) else {
            return false;
        };
        let before = entry.participants.len();
        entry
            .participants
            .retain(|participant| participant.id != id);
        let removed = entry.participants.len() != before;
        if removed && entry.participants.is_empty() && !entry.present {
            self.check_queued = true;
        }
        removed && self.check_queued
    }

    /// A participant reports it finished the presence cycle of `generation`. Calls from
    /// unregistered participants, unknown keys and stale generations are ignored.
    pub fn participant_complete(
        &mut self,
        key: &K,
        id: ParticipantId,
        generation: u64,
    ) -> PresenceOutcome<K> {
        let accepted = self
            .entries
            .iter_mut()
            .find(|entry| &entry.key == key && entry.generation == generation)
            .and_then(|entry| entry.participants.iter_mut().find(|p| p.id == id))
            .map(|participant| participant.done = true)
            .is_some();
        if accepted {
            self.check()
        } else {
            PresenceOutcome::default()
        }
    }

    /// A native motion completion report (ANM-002 R6) for `key` with the track's generation.
    ///
    /// Reports for other generations are rejected without calling user code or releasing
    /// presence. A current report requests the user completion callback and, for an exiting
    /// child, counts as the child's native completion.
    pub fn motion_complete(&mut self, key: &K, generation: u64) -> PresenceOutcome<K> {
        let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| &entry.key == key && entry.generation == generation)
        else {
            return PresenceOutcome::default();
        };
        if !entry.present {
            entry.native_done = true;
        }
        let mut outcome = self.check();
        outcome
            .motion_callbacks
            .insert(0, (key.clone(), generation));
        outcome
    }

    /// Runs the completion check that follows commit effects: exiting children with nothing to
    /// wait for (no exit target and no registered participants) complete immediately.
    pub fn after_effects(&mut self) -> PresenceOutcome<K> {
        self.check()
    }

    /// Runs a completion check queued by [`PresenceManager::unregister_participant`].
    pub fn run_queued_check(&mut self) -> PresenceOutcome<K> {
        if std::mem::take(&mut self.check_queued) {
            self.check()
        } else {
            PresenceOutcome::default()
        }
    }

    fn check(&mut self) -> PresenceOutcome<K> {
        let mut outcome = PresenceOutcome::default();
        let exiting = self.entries.iter().filter(|entry| !entry.present).count();
        if exiting == 0 {
            return outcome;
        }
        let all_ready = self
            .entries
            .iter()
            .filter(|entry| !entry.present)
            .all(Entry::exit_ready);
        let releasable = |entry: &Entry<K, T>| match self.flavor.removal {
            RemovalMode::Aggregate => all_ready,
            RemovalMode::Individual => entry.exit_ready(),
        };
        let mut kept = Vec::with_capacity(self.entries.len());
        for entry in std::mem::take(&mut self.entries) {
            if !entry.present && releasable(&entry) {
                outcome.removed.push(entry.key);
            } else {
                kept.push(entry);
            }
        }
        self.entries = kept;
        outcome.exit_complete = !outcome.removed.is_empty() && !self.has_exiting();
        outcome
    }
}
