//! The native popup lifecycle: unmounted, open, closing (CMP-002 R13, R15, R17).
//!
//! A [`Popup`] is a plain value a view keeps next to its other state. *For logic* (key
//! handling, is-open queries, press tracking) a closing popup reads as closed
//! ([`Popup::as_open`], [`Popup::is_open`]) and its key and press handlers fall through; *for
//! rendering* it is still mounted ([`Popup::get`]) until the reap drops it.
//!
//! Times are milliseconds on the motion clock ([`vespertine::MotionClock`]), so tests freeze and
//! step them deterministically.
//!
//! ```
//! use vespertine_components::floating::Popup;
//!
//! let mut popup: Popup<&str> = Popup::new();
//! popup.open("menu", 0.);
//! assert!(popup.is_open());
//! assert!(popup.begin_close(10.));      // this call started the close
//! assert!(!popup.begin_close(50.));     // a second call is a no-op
//! assert!(popup.get().is_some() && popup.as_open().is_none());
//! assert!(!popup.reap(99.));            // a stale reap leaves it alone
//! assert!(popup.reap(10.));             // the matching reap drops it
//! ```

use super::motion::reap_delay;
use vespertine::{Context, MotionClock};

#[derive(Clone, Debug)]
enum Phase<T> {
    Unmounted,
    Open {
        payload: T,
        opened_at: f64,
    },
    Closing {
        payload: T,
        opened_at: f64,
        since: f64,
    },
}

/// A popup lifecycle value carrying a payload while mounted.
#[derive(Clone, Debug)]
pub struct Popup<T> {
    phase: Phase<T>,
    press_was_open: bool,
}

impl<T> Default for Popup<T> {
    fn default() -> Self {
        Self {
            phase: Phase::Unmounted,
            press_was_open: false,
        }
    }
}

impl<T> Popup<T> {
    /// An unmounted popup.
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens with `payload` at motion time `now_ms`, clearing any closing mark. Opening an
    /// already open popup replaces the payload and keeps the entrance running.
    pub fn open(&mut self, payload: T, now_ms: f64) {
        let opened_at = match &self.phase {
            Phase::Open { opened_at, .. } => *opened_at,
            _ => now_ms,
        };
        self.phase = Phase::Open { payload, opened_at };
    }

    /// Open and not closing: the logic view.
    pub fn is_open(&self) -> bool {
        matches!(self.phase, Phase::Open { .. })
    }

    /// In its exit phase.
    pub fn is_closing(&self) -> bool {
        matches!(self.phase, Phase::Closing { .. })
    }

    /// Open or closing: the rendering view.
    pub fn is_mounted(&self) -> bool {
        !matches!(self.phase, Phase::Unmounted)
    }

    /// Motion time the close began, while closing.
    pub fn closing_since(&self) -> Option<f64> {
        match self.phase {
            Phase::Closing { since, .. } => Some(since),
            _ => None,
        }
    }

    /// Motion time the popup opened, while mounted (the entrance start).
    pub fn opened_at(&self) -> Option<f64> {
        match self.phase {
            Phase::Open { opened_at, .. } | Phase::Closing { opened_at, .. } => Some(opened_at),
            Phase::Unmounted => None,
        }
    }

    /// The payload while mounted (open or closing): what rendering uses.
    pub fn get(&self) -> Option<&T> {
        match &self.phase {
            Phase::Open { payload, .. } | Phase::Closing { payload, .. } => Some(payload),
            Phase::Unmounted => None,
        }
    }

    /// Mutable payload while mounted.
    pub fn get_mut(&mut self) -> Option<&mut T> {
        match &mut self.phase {
            Phase::Open { payload, .. } | Phase::Closing { payload, .. } => Some(payload),
            Phase::Unmounted => None,
        }
    }

    /// The payload only while open: what key handling and queries use (a closing popup reads as
    /// closed and its handlers fall through).
    pub fn as_open(&self) -> Option<&T> {
        match &self.phase {
            Phase::Open { payload, .. } => Some(payload),
            _ => None,
        }
    }

    /// Mutable [`Popup::as_open`].
    pub fn as_open_mut(&mut self) -> Option<&mut T> {
        match &mut self.phase {
            Phase::Open { payload, .. } => Some(payload),
            _ => None,
        }
    }

    /// Enters the closing phase at `now_ms`; true when this call started it, false when the
    /// popup was not open (already closing or unmounted). A second call never restarts the exit.
    pub fn begin_close(&mut self, now_ms: f64) -> bool {
        match std::mem::replace(&mut self.phase, Phase::Unmounted) {
            Phase::Open { payload, opened_at } => {
                self.phase = Phase::Closing {
                    payload,
                    opened_at,
                    since: now_ms,
                };
                true
            }
            other => {
                self.phase = other;
                false
            }
        }
    }

    /// The reap that belongs to the close begun at `begun_at`: drops the state and returns true
    /// only when the popup is still in exactly that close. A popup reopened or re-closed since
    /// is left alone.
    pub fn reap(&mut self, begun_at: f64) -> bool {
        if self.closing_since() == Some(begun_at) {
            self.phase = Phase::Unmounted;
            true
        } else {
            false
        }
    }

    /// Records, at trigger press time, whether the popup was mounted (open or still in its exit
    /// phase). Call it for presses on the trigger that owns this popup.
    pub fn note_trigger_press(&mut self) {
        self.press_was_open = self.is_mounted();
    }

    /// Consumes the press note. The click that follows a press that found its popup mounted
    /// leaves the popup closed; every click is preceded by a press, so the note is never stale.
    pub fn take_press_was_open(&mut self) -> bool {
        std::mem::take(&mut self.press_was_open)
    }
}

/// Starts the exit phase of the popup that `popup_of` selects in `this` and schedules its reap.
///
/// Returns whether this call started the close. The reap fires after the exit span plus 20 ms
/// (immediately, but after the current event, with reduced motion) and is harmless when the
/// popup was reopened or re-closed in the meantime.
pub fn close_popup<V: 'static, T: 'static>(
    this: &mut V,
    cx: &mut Context<V>,
    popup_of: fn(&mut V) -> &mut Popup<T>,
) -> bool {
    let now = MotionClock::now_ms(cx);
    if !popup_of(this).begin_close(now) {
        return false;
    }
    cx.notify();
    schedule_reap(cx, now, popup_of);
    true
}

/// Schedules the reap of the close begun at `begun_at` (see [`close_popup`]).
pub fn schedule_reap<V: 'static, T: 'static>(
    cx: &mut Context<V>,
    begun_at: f64,
    popup_of: fn(&mut V) -> &mut Popup<T>,
) {
    let delay = reap_delay(cx.reduce_motion());
    cx.spawn(async move |this, cx| {
        cx.background_executor().timer(delay).await;
        this.update(cx, |this, cx| {
            if popup_of(this).reap(begun_at) {
                cx.notify();
            }
        })
        .ok();
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_close_reap_cycle() {
        let mut popup = Popup::new();
        assert!(!popup.is_mounted());
        popup.open(1, 0.);
        assert!(popup.is_open() && popup.is_mounted() && !popup.is_closing());
        assert_eq!(popup.get(), Some(&1));
        assert_eq!(popup.as_open(), Some(&1));
        assert!(popup.begin_close(5.));
        assert!(popup.is_closing() && !popup.is_open());
        assert_eq!(popup.closing_since(), Some(5.));
        assert_eq!(popup.get(), Some(&1), "rendering still mounts it");
        assert_eq!(popup.as_open(), None, "logic reads it as closed");
        assert!(popup.reap(5.));
        assert!(!popup.is_mounted());
        assert!(!popup.begin_close(6.), "nothing to close when unmounted");
    }

    #[test]
    fn begin_close_is_idempotent_and_keeps_the_original_start() {
        let mut popup = Popup::new();
        popup.open((), 0.);
        assert!(popup.begin_close(10.));
        assert!(!popup.begin_close(60.));
        assert_eq!(popup.closing_since(), Some(10.));
    }

    #[test]
    fn stale_reap_leaves_a_reopened_or_reclosed_popup_alone() {
        let mut popup = Popup::new();
        popup.open("a", 0.);
        popup.begin_close(10.);
        popup.open("b", 40.);
        assert!(!popup.reap(10.), "reopened since");
        assert!(popup.is_open());
        assert_eq!(popup.opened_at(), Some(40.));
        popup.begin_close(70.);
        assert!(!popup.reap(10.), "re-closed since");
        assert!(popup.is_closing());
        assert!(popup.reap(70.));
    }

    #[test]
    fn opening_an_open_popup_keeps_the_entrance() {
        let mut popup = Popup::new();
        popup.open(1, 3.);
        popup.open(2, 90.);
        assert_eq!(popup.opened_at(), Some(3.));
        assert_eq!(popup.get(), Some(&2));
    }

    #[test]
    fn press_note_counts_open_and_closing_and_is_consumed_once() {
        let mut popup = Popup::new();
        popup.note_trigger_press();
        assert!(!popup.take_press_was_open(), "nothing was mounted");
        popup.open(7, 0.);
        popup.note_trigger_press();
        assert!(popup.take_press_was_open());
        assert!(!popup.take_press_was_open(), "consumed");
        // An outside press already began the close: the popup is still mounted for the note.
        popup.begin_close(1.);
        popup.note_trigger_press();
        assert!(popup.take_press_was_open());
    }
}
