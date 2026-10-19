//! Shortcut recording, combo normalization, conflict detection and keymap
//! rebuilding (INP-007).
//!
//! A *combo* is the persisted and displayed form of one chord: modifier tokens
//! in the fixed order primary modifier, super, alt, shift, followed by the
//! lower-cased, trimmed key, all joined by hyphens, for example
//! `primary-shift-k`. The primary modifier is the platform's conventional
//! command modifier, so one stored combo means the same chord on every
//! platform: Control on Linux and Windows (where the Super/Windows key
//! contributes its own separate `super` token), Command on macOS (where the
//! Control key takes the `super` token).
//!
//! [`ShortcutSet`] holds the user-visible shortcuts with their defaults and
//! answers conflict queries; [`ShortcutRecorder`] is the entity that records a
//! new combo for one shortcut, intercepting keystrokes before any binding or
//! action runs.

use std::collections::HashSet;

use crate::{
    App, Context, EventEmitter, FocusHandle, Focusable, KeyBinding, Keystroke, Modifiers,
    SharedString, Subscription, Window, global_shortcuts::GlobalShortcutSuspension,
    global_shortcuts::GlobalShortcuts,
};

/// Token for the platform's primary command modifier in a combo.
pub const PRIMARY_TOKEN: &str = "primary";
/// Token for the Super/Windows (Control on macOS) modifier in a combo.
pub const SUPER_TOKEN: &str = "super";
/// Token for the Alt/Option modifier in a combo.
pub const ALT_TOKEN: &str = "alt";
/// Token for the Shift modifier in a combo.
pub const SHIFT_TOKEN: &str = "shift";

/// Keys that never form a combo on their own: pressing them while recording
/// is ignored and recording continues. Compared case-insensitively.
const BARE_MODIFIER_KEYS: [&str; 7] = [
    "control", "ctrl", "alt", "shift", "platform", "super", "function",
];

/// The result of classifying one keystroke received while recording.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordedKeystroke {
    /// Escape: recording is cancelled and the previous combo is kept.
    Cancelled,
    /// A bare modifier key, or an empty or unusable key: recording continues.
    Ignored,
    /// Any other key with the held modifiers: the recording is complete.
    Set {
        /// The normalized combo of the keystroke.
        combo: String,
    },
}

/// Classifies a keystroke received while recording a shortcut.
pub fn classify_recording_keystroke(keystroke: &Keystroke) -> RecordedKeystroke {
    let key = keystroke.key.trim();
    if key.eq_ignore_ascii_case("escape") {
        return RecordedKeystroke::Cancelled;
    }
    if key.is_empty()
        || BARE_MODIFIER_KEYS
            .iter()
            .any(|bare| key.eq_ignore_ascii_case(bare))
    {
        return RecordedKeystroke::Ignored;
    }
    match normalize_combo(keystroke) {
        Some(combo) => RecordedKeystroke::Set { combo },
        None => RecordedKeystroke::Ignored,
    }
}

/// Normalizes a keystroke to its combo: modifier tokens in the order primary,
/// super, alt, shift, then the lower-cased trimmed key, joined by hyphens.
/// Returns `None` when the key is empty after trimming.
pub fn normalize_combo(keystroke: &Keystroke) -> Option<String> {
    let key = keystroke.key.trim().to_lowercase();
    if key.is_empty() {
        return None;
    }
    let Modifiers {
        control,
        alt,
        shift,
        platform,
        ..
    } = keystroke.modifiers;
    let (primary, super_key) = if cfg!(target_os = "macos") {
        (platform, control)
    } else {
        (control, platform)
    };
    let mut combo = String::new();
    for (held, token) in [
        (primary, PRIMARY_TOKEN),
        (super_key, SUPER_TOKEN),
        (alt, ALT_TOKEN),
        (shift, SHIFT_TOKEN),
    ] {
        if held {
            combo.push_str(token);
            combo.push('-');
        }
    }
    combo.push_str(&key);
    Some(combo)
}

/// Parses a combo back into the keystroke it stands for, mapping the primary
/// and super tokens to this platform's modifiers. Returns `None` for an empty
/// combo or one without a key.
pub fn combo_to_keystroke(combo: &str) -> Option<Keystroke> {
    let mut rest = combo.trim();
    let mut primary = false;
    let mut super_key = false;
    let mut alt = false;
    let mut shift = false;
    loop {
        let (flag, token) = if let Some(rest_) = rest.strip_prefix("primary-") {
            (&mut primary, rest_)
        } else if let Some(rest_) = rest.strip_prefix("super-") {
            (&mut super_key, rest_)
        } else if let Some(rest_) = rest.strip_prefix("alt-") {
            (&mut alt, rest_)
        } else if let Some(rest_) = rest.strip_prefix("shift-") {
            (&mut shift, rest_)
        } else {
            break;
        };
        *flag = true;
        rest = token;
    }
    if rest.is_empty() {
        return None;
    }
    let (control, platform) = if cfg!(target_os = "macos") {
        (super_key, primary)
    } else {
        (primary, super_key)
    };
    Some(Keystroke {
        modifiers: Modifiers {
            control,
            alt,
            shift,
            platform,
            function: false,
        },
        key: rest.to_string(),
        key_char: None,
    })
}

/// Renders a combo as a keystroke source string accepted by
/// [`KeyBinding::new`] (for example `ctrl-shift-k`).
pub fn combo_to_binding_source(combo: &str) -> Option<String> {
    let keystroke = combo_to_keystroke(combo)?;
    let mut source = String::new();
    for (held, name) in [
        (keystroke.modifiers.control, "ctrl"),
        (keystroke.modifiers.alt, "alt"),
        (keystroke.modifiers.shift, "shift"),
        (keystroke.modifiers.platform, "super"),
    ] {
        if held {
            source.push_str(name);
            source.push('-');
        }
    }
    source.push_str(&keystroke.key);
    Some(source)
}

/// One user-rebindable shortcut.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shortcut {
    /// Stable identifier of the shortcut.
    pub id: SharedString,
    /// Human-readable name, used in conflict notices.
    pub label: SharedString,
    /// The combo restored by a reset. May be empty (no default chord).
    pub default_combo: String,
    /// The combo currently in effect. Empty when the shortcut is unbound.
    pub combo: String,
    /// Whether the shortcut participates in conflict detection (for example
    /// it is not offered on the current page).
    pub available: bool,
}

impl Shortcut {
    /// A shortcut that starts out at its default combo and is available.
    pub fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        default_combo: impl Into<String>,
    ) -> Self {
        let default_combo = default_combo.into();
        Self {
            id: id.into(),
            label: label.into(),
            combo: default_combo.clone(),
            default_combo,
            available: true,
        }
    }
}

/// Data for the notice shown when a recorded combo is refused because another
/// available shortcut already uses it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShortcutRefusal {
    /// The refused combo.
    pub combo: String,
    /// Identifier of the shortcut that already uses the combo.
    pub existing: SharedString,
    /// Label of the shortcut that already uses the combo.
    pub existing_label: SharedString,
}

/// What applying one keystroke to a [`ShortcutSet`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordingOutcome {
    /// Escape: recording cancelled, the old combo kept.
    Cancelled,
    /// A bare modifier or unusable key: recording continues.
    Ignored,
    /// The combo was stored for the shortcut.
    Set {
        /// The stored combo.
        combo: String,
    },
    /// Another available shortcut already uses the combo; nothing changed.
    Refused(ShortcutRefusal),
}

/// A set of rebindable shortcuts with defaults and conflict queries.
#[derive(Clone, Debug, Default)]
pub struct ShortcutSet {
    shortcuts: Vec<Shortcut>,
}

impl ShortcutSet {
    /// A set holding `shortcuts` in the given order.
    pub fn new(shortcuts: impl IntoIterator<Item = Shortcut>) -> Self {
        Self {
            shortcuts: shortcuts.into_iter().collect(),
        }
    }

    /// All shortcuts in order.
    pub fn shortcuts(&self) -> &[Shortcut] {
        &self.shortcuts
    }

    /// The shortcut with the given identifier.
    pub fn get(&self, id: &str) -> Option<&Shortcut> {
        self.shortcuts.iter().find(|shortcut| shortcut.id == id)
    }

    /// Marks a shortcut as available or not for conflict detection. Returns
    /// whether the shortcut exists.
    pub fn set_available(&mut self, id: &str, available: bool) -> bool {
        match self.shortcuts.iter_mut().find(|shortcut| shortcut.id == id) {
            Some(shortcut) => {
                shortcut.available = available;
                true
            }
            None => false,
        }
    }

    /// The other available shortcut that already uses `combo`, if any. An
    /// empty combo never collides.
    pub fn find_conflict(&self, id: &str, combo: &str) -> Option<&Shortcut> {
        if combo.is_empty() {
            return None;
        }
        self.shortcuts
            .iter()
            .find(|other| other.id != id && other.available && other.combo == combo)
    }

    /// The identifiers of every available shortcut whose non-empty combo
    /// collides with another available shortcut, in set order.
    pub fn conflicts(&self) -> Vec<SharedString> {
        let mut seen = HashSet::new();
        let mut duplicated = HashSet::new();
        for shortcut in self.shortcuts.iter().filter(|s| s.available) {
            if !shortcut.combo.is_empty() && !seen.insert(shortcut.combo.as_str()) {
                duplicated.insert(shortcut.combo.as_str());
            }
        }
        self.shortcuts
            .iter()
            .filter(|s| s.available && duplicated.contains(s.combo.as_str()))
            .map(|s| s.id.clone())
            .collect()
    }

    /// Applies a keystroke recorded for the shortcut `id`: classifies it,
    /// refuses a combo another available shortcut uses (changing nothing),
    /// and stores any other new combo.
    pub fn apply_recorded_keystroke(
        &mut self,
        id: &str,
        keystroke: &Keystroke,
    ) -> RecordingOutcome {
        match classify_recording_keystroke(keystroke) {
            RecordedKeystroke::Cancelled => RecordingOutcome::Cancelled,
            RecordedKeystroke::Ignored => RecordingOutcome::Ignored,
            RecordedKeystroke::Set { combo } => {
                if let Some(existing) = self.find_conflict(id, &combo) {
                    return RecordingOutcome::Refused(ShortcutRefusal {
                        combo,
                        existing: existing.id.clone(),
                        existing_label: existing.label.clone(),
                    });
                }
                if let Some(shortcut) = self.shortcuts.iter_mut().find(|s| s.id == id) {
                    shortcut.combo = combo.clone();
                }
                RecordingOutcome::Set { combo }
            }
        }
    }

    /// Resets one shortcut to its default combo. Returns whether its combo
    /// changed.
    pub fn reset(&mut self, id: &str) -> bool {
        match self.shortcuts.iter_mut().find(|shortcut| shortcut.id == id) {
            Some(shortcut) if shortcut.combo != shortcut.default_combo => {
                shortcut.combo = shortcut.default_combo.clone();
                true
            }
            _ => false,
        }
    }

    /// Restores every shortcut to its default combo. Returns whether any
    /// combo changed.
    pub fn restore_all(&mut self) -> bool {
        let mut changed = false;
        for shortcut in &mut self.shortcuts {
            if shortcut.combo != shortcut.default_combo {
                shortcut.combo = shortcut.default_combo.clone();
                changed = true;
            }
        }
        changed
    }

    /// Builds the key bindings of the shortcuts that have a combo, one per
    /// shortcut, using `bind` to attach the action to the keystroke source
    /// string of the combo (for example `ctrl-shift-k`). Shortcuts for which
    /// `bind` returns `None` are skipped.
    pub fn bindings(
        &self,
        mut bind: impl FnMut(&Shortcut, &str) -> Option<KeyBinding>,
    ) -> Vec<KeyBinding> {
        self.shortcuts
            .iter()
            .filter(|shortcut| shortcut.available)
            .filter_map(|shortcut| {
                let source = combo_to_binding_source(&shortcut.combo)?;
                bind(shortcut, &source)
            })
            .collect()
    }
}

/// Rebuilds the application keymap: clears every binding, then registers the
/// `embedded_defaults` (defaults of embedded surfaces such as editors or
/// terminals) first and the `configured` user combos after them. Later
/// registrations outrank earlier ones, so a configured application chord
/// always takes precedence over a colliding embedded default.
pub fn rebuild_keymap(
    cx: &mut App,
    embedded_defaults: impl IntoIterator<Item = KeyBinding>,
    configured: impl IntoIterator<Item = KeyBinding>,
) {
    cx.clear_key_bindings();
    cx.bind_keys(embedded_defaults);
    cx.bind_keys(configured);
}

/// Why a recording ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordingEnd {
    /// A combo was recorded and stored.
    Completed,
    /// Escape was pressed.
    Cancelled,
    /// The combo was refused because another shortcut uses it.
    Refused,
    /// The recorder lost focus.
    Blurred,
    /// The page changed ([`ShortcutRecorder::cancel_recording`]).
    PageChanged,
}

/// Events emitted by a [`ShortcutRecorder`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShortcutRecorderEvent {
    /// Recording started for the shortcut.
    RecordingStarted {
        /// The shortcut being recorded.
        shortcut: SharedString,
    },
    /// Recording ended for the shortcut.
    RecordingEnded {
        /// The shortcut that was being recorded.
        shortcut: SharedString,
        /// The route by which recording ended.
        end: RecordingEnd,
    },
    /// A recorded combo was refused; show a notice identifying the shortcut
    /// that already uses it. Nothing was persisted.
    Refused(ShortcutRefusal),
    /// The set of combos changed: the application persists the new combos and
    /// rebuilds its keymap (see [`rebuild_keymap`]).
    KeymapChanged,
}

struct ActiveRecording {
    shortcut: SharedString,
    _interceptor: Subscription,
    _focus_out: Subscription,
    _global_shortcuts: GlobalShortcutSuspension,
}

/// Records new combos for the shortcuts of a [`ShortcutSet`].
///
/// Recording is started per shortcut with [`Self::start_recording`]. While it
/// runs the recorder is focused, every keystroke is intercepted before any
/// key binding or action (a colliding chord is recorded or refused instead of
/// executing its action) and consumed, and all process-wide shortcuts
/// registered with [`GlobalShortcuts`] are suspended. Recording ends when a
/// combo is recorded, on Escape, when the recorder loses focus, when the
/// page changes ([`Self::cancel_recording`]) or when the recorder is released;
/// the global shortcuts are restored on every route.
///
/// The element that represents the recorder in the UI must track
/// [`Self::focus_handle`], otherwise the recorder cannot hold focus.
pub struct ShortcutRecorder {
    shortcuts: ShortcutSet,
    focus_handle: FocusHandle,
    active: Option<ActiveRecording>,
}

impl EventEmitter<ShortcutRecorderEvent> for ShortcutRecorder {}

impl Focusable for ShortcutRecorder {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ShortcutRecorder {
    /// A recorder over `shortcuts`.
    pub fn new(shortcuts: ShortcutSet, cx: &mut Context<Self>) -> Self {
        Self {
            shortcuts,
            focus_handle: cx.focus_handle(),
            active: None,
        }
    }

    /// The shortcuts and their current combos.
    pub fn shortcuts(&self) -> &ShortcutSet {
        &self.shortcuts
    }

    /// Mutable access to the shortcut set, for example to mark shortcuts
    /// unavailable on the current page. Combo changes made here are not
    /// announced; use the recorder's own operations to change combos.
    pub fn shortcuts_mut(&mut self) -> &mut ShortcutSet {
        &mut self.shortcuts
    }

    /// The focus handle the recorder holds while recording.
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

    /// The shortcut being recorded, if any.
    pub fn recording(&self) -> Option<&SharedString> {
        self.active.as_ref().map(|active| &active.shortcut)
    }

    /// Starts recording for the shortcut `id`, ending a recording in progress
    /// for another shortcut as cancelled. Returns `false` (and records
    /// nothing) when the shortcut is unknown.
    pub fn start_recording(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(shortcut) = self.shortcuts.get(id).map(|shortcut| shortcut.id.clone()) else {
            return false;
        };
        self.end_recording(RecordingEnd::Cancelled, cx);

        let recorder = cx.entity().downgrade();
        let window_id = window.window_handle().window_id();
        let interceptor = cx.intercept_keystrokes(move |event, window, cx| {
            if window.window_handle().window_id() != window_id {
                return;
            }
            let Some(recorder) = recorder.upgrade() else {
                return;
            };
            cx.stop_propagation();
            recorder.update(cx, |this, cx| {
                this.handle_keystroke(&event.keystroke, cx);
            });
        });
        let recorder = cx.entity().downgrade();
        let focus_out = window.on_focus_out(&self.focus_handle, cx, move |_, _, cx| {
            recorder
                .update(cx, |this, cx| this.end_recording(RecordingEnd::Blurred, cx))
                .ok();
        });
        let global_shortcuts = GlobalShortcuts::suspend(cx);

        window.focus(&self.focus_handle, cx);
        self.active = Some(ActiveRecording {
            shortcut: shortcut.clone(),
            _interceptor: interceptor,
            _focus_out: focus_out,
            _global_shortcuts: global_shortcuts,
        });
        cx.emit(ShortcutRecorderEvent::RecordingStarted { shortcut });
        true
    }

    /// Ends recording because the page changed, keeping the old combo.
    pub fn cancel_recording(&mut self, cx: &mut Context<Self>) {
        self.end_recording(RecordingEnd::PageChanged, cx);
    }

    /// Resets one shortcut to its default combo, announcing
    /// [`ShortcutRecorderEvent::KeymapChanged`] when its combo changed.
    pub fn reset(&mut self, id: &str, cx: &mut Context<Self>) -> bool {
        let changed = self.shortcuts.reset(id);
        if changed {
            cx.emit(ShortcutRecorderEvent::KeymapChanged);
        }
        changed
    }

    /// Restores every shortcut to its default combo, announcing
    /// [`ShortcutRecorderEvent::KeymapChanged`] when any combo changed.
    pub fn restore_all(&mut self, cx: &mut Context<Self>) -> bool {
        let changed = self.shortcuts.restore_all();
        if changed {
            cx.emit(ShortcutRecorderEvent::KeymapChanged);
        }
        changed
    }

    fn handle_keystroke(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) {
        let Some(id) = self.recording().cloned() else {
            return;
        };
        match self.shortcuts.apply_recorded_keystroke(&id, keystroke) {
            RecordingOutcome::Ignored => {}
            RecordingOutcome::Cancelled => self.end_recording(RecordingEnd::Cancelled, cx),
            RecordingOutcome::Set { .. } => {
                self.end_recording(RecordingEnd::Completed, cx);
                cx.emit(ShortcutRecorderEvent::KeymapChanged);
            }
            RecordingOutcome::Refused(refusal) => {
                self.end_recording(RecordingEnd::Refused, cx);
                cx.emit(ShortcutRecorderEvent::Refused(refusal));
            }
        }
    }

    fn end_recording(&mut self, end: RecordingEnd, cx: &mut Context<Self>) {
        if let Some(active) = self.active.take() {
            let shortcut = active.shortcut.clone();
            // Dropping the recording removes the interceptor and focus
            // listener and restores the global shortcuts.
            drop(active);
            cx.emit(ShortcutRecorderEvent::RecordingEnded { shortcut, end });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::FluentBuilder as _;
    use crate::{
        self as vespertine, AppContext as _, Entity, InteractiveElement as _, IntoElement,
        ParentElement as _, Render, Styled as _, TestAppContext, div, px,
    };
    use std::{cell::Cell, cell::RefCell, rc::Rc};

    actions!(
        shortcut_tests,
        [EmbeddedAction, ConfiguredAction, BoundAction]
    );

    fn stroke(source: &str) -> Keystroke {
        Keystroke::parse(source).expect("valid keystroke")
    }

    fn set_of(shortcuts: &[(&str, &str)]) -> ShortcutSet {
        ShortcutSet::new(
            shortcuts
                .iter()
                .map(|(id, combo)| Shortcut::new(*id, format!("Shortcut {id}"), *combo)),
        )
    }

    #[test]
    fn recording_classification() {
        // Escape cancels, regardless of modifiers.
        assert_eq!(
            classify_recording_keystroke(&stroke("escape")),
            RecordedKeystroke::Cancelled
        );
        // Bare modifier keys are ignored in any capitalization.
        for key in [
            "control", "Control", "ALT", "alt", "shift", "Shift", "platform", "super", "function",
            "Function",
        ] {
            let bare = Keystroke {
                modifiers: Modifiers::none(),
                key: key.to_string(),
                key_char: None,
            };
            assert_eq!(
                classify_recording_keystroke(&bare),
                RecordedKeystroke::Ignored,
                "{key}"
            );
        }
        // Empty or whitespace keys are unusable.
        for key in ["", "  "] {
            let unusable = Keystroke {
                modifiers: Modifiers::control(),
                key: key.to_string(),
                key_char: None,
            };
            assert_eq!(
                classify_recording_keystroke(&unusable),
                RecordedKeystroke::Ignored
            );
        }
        // Control+Shift+K gives the primary, shift and k tokens in order.
        assert_eq!(
            classify_recording_keystroke(&stroke("ctrl-shift-k")),
            RecordedKeystroke::Set {
                combo: "primary-shift-k".into()
            }
        );
        // Case and surrounding whitespace in the key are normalized.
        let shouted = Keystroke {
            modifiers: Modifiers {
                control: true,
                shift: true,
                ..Modifiers::none()
            },
            key: " K ".into(),
            key_char: None,
        };
        assert_eq!(
            normalize_combo(&shouted).as_deref(),
            Some("primary-shift-k")
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn combo_token_order_and_super_on_linux() {
        // Control is the primary modifier; Super contributes its own token and
        // never folds into the primary token.
        let all = Keystroke {
            modifiers: Modifiers {
                control: true,
                alt: true,
                shift: true,
                platform: true,
                function: false,
            },
            key: "x".into(),
            key_char: None,
        };
        assert_eq!(
            normalize_combo(&all).as_deref(),
            Some("primary-super-alt-shift-x")
        );
        let super_only = Keystroke {
            modifiers: Modifiers {
                platform: true,
                ..Modifiers::none()
            },
            key: "x".into(),
            key_char: None,
        };
        assert_eq!(normalize_combo(&super_only).as_deref(), Some("super-x"));

        // Round trip through the stored form.
        for combo in [
            "primary-super-alt-shift-x",
            "super-x",
            "alt-f4",
            "primary--",
            "enter",
        ] {
            let keystroke = combo_to_keystroke(combo).expect(combo);
            assert_eq!(normalize_combo(&keystroke).as_deref(), Some(combo));
        }
        assert_eq!(
            combo_to_binding_source("primary-shift-k").as_deref(),
            Some("ctrl-shift-k")
        );
        assert_eq!(combo_to_keystroke(""), None);
        assert_eq!(combo_to_keystroke("primary-"), None);
    }

    #[test]
    fn conflicts_are_reported_and_recording_a_colliding_combo_is_refused() {
        let mut set = set_of(&[("copy", "primary-c"), ("paste", "primary-v"), ("find", "")]);
        assert!(set.conflicts().is_empty());

        // Recording Control+V for "find" collides with "paste": refused, and
        // the set stays unchanged.
        let before = set.clone().shortcuts().to_vec();
        let outcome = set.apply_recorded_keystroke("find", &stroke("ctrl-v"));
        assert_eq!(
            outcome,
            RecordingOutcome::Refused(ShortcutRefusal {
                combo: "primary-v".into(),
                existing: "paste".into(),
                existing_label: "Shortcut paste".into(),
            })
        );
        assert_eq!(set.shortcuts(), before.as_slice());

        // Re-recording a shortcut's own combo is not a conflict.
        assert_eq!(
            set.apply_recorded_keystroke("paste", &stroke("ctrl-v")),
            RecordingOutcome::Set {
                combo: "primary-v".into()
            }
        );

        // A free combo is stored.
        assert_eq!(
            set.apply_recorded_keystroke("find", &stroke("ctrl-f")),
            RecordingOutcome::Set {
                combo: "primary-f".into()
            }
        );
        assert_eq!(set.get("find").map(|s| s.combo.as_str()), Some("primary-f"));

        // Escape and bare modifiers change nothing.
        assert_eq!(
            set.apply_recorded_keystroke("find", &stroke("escape")),
            RecordingOutcome::Cancelled
        );
        assert_eq!(
            set.apply_recorded_keystroke("find", &stroke("shift")),
            RecordingOutcome::Ignored
        );
        assert_eq!(set.get("find").map(|s| s.combo.as_str()), Some("primary-f"));

        // The conflict query lists every available shortcut sharing a
        // non-empty combo; unavailable shortcuts and empty combos are
        // ignored.
        let mut set = set_of(&[
            ("a", "primary-k"),
            ("b", "primary-k"),
            ("c", ""),
            ("d", ""),
            ("e", "alt-x"),
        ]);
        assert_eq!(set.conflicts(), vec![SharedString::from("a"), "b".into()]);
        assert!(set.set_available("b", false));
        assert!(set.conflicts().is_empty());
        assert!(
            set.find_conflict("c", "primary-k")
                .is_some_and(|s| s.id == "a")
        );
    }

    #[test]
    fn reset_one_and_restore_all() {
        let mut set = set_of(&[("a", "primary-a"), ("b", "primary-b"), ("c", "")]);
        set.apply_recorded_keystroke("a", &stroke("ctrl-1"));
        set.apply_recorded_keystroke("b", &stroke("ctrl-2"));
        set.apply_recorded_keystroke("c", &stroke("ctrl-3"));

        assert!(set.reset("a"));
        assert!(!set.reset("a"), "already at default");
        assert!(!set.reset("missing"));
        assert_eq!(set.get("a").map(|s| s.combo.as_str()), Some("primary-a"));
        assert_eq!(set.get("b").map(|s| s.combo.as_str()), Some("primary-2"));

        assert!(set.restore_all());
        assert!(!set.restore_all());
        assert_eq!(set.get("b").map(|s| s.combo.as_str()), Some("primary-b"));
        assert_eq!(set.get("c").map(|s| s.combo.as_str()), Some(""));
    }

    struct KeymapHost {
        embedded: Rc<Cell<usize>>,
        configured: Rc<Cell<usize>>,
        focus: crate::FocusHandle,
    }

    impl Render for KeymapHost {
        fn render(
            &mut self,
            _: &mut crate::Window,
            _: &mut crate::Context<Self>,
        ) -> impl IntoElement {
            let embedded = self.embedded.clone();
            let configured = self.configured.clone();
            div()
                .size_full()
                .track_focus(&self.focus)
                .on_action(move |_: &EmbeddedAction, _, _| embedded.set(embedded.get() + 1))
                .on_action(move |_: &ConfiguredAction, _, _| configured.set(configured.get() + 1))
        }
    }

    fn focus_and_draw(cx: &mut crate::VisualTestContext, handle: &crate::FocusHandle) {
        cx.update(|window, cx| {
            window.focus(handle, cx);
            window.activate_window();
            window.draw(cx).clear(cx);
        });
    }

    #[crate::test]
    fn configured_combos_outrank_embedded_defaults_after_rebuild(cx: &mut TestAppContext) {
        let embedded = Rc::new(Cell::new(0));
        let configured = Rc::new(Cell::new(0));
        let (view, cx) = cx.add_window_view({
            let (embedded, configured) = (embedded.clone(), configured.clone());
            move |_, cx| KeymapHost {
                embedded,
                configured,
                focus: cx.focus_handle(),
            }
        });
        let focus = view.read_with(cx, |host, _| host.focus.clone());
        focus_and_draw(cx, &focus);

        // The configured chord collides with an embedded surface's default.
        let set = set_of(&[("configured", "primary-k")]);
        let embedded_bindings = || [KeyBinding::new("ctrl-k", EmbeddedAction, None)];
        let configured_bindings =
            || set.bindings(|_, source| Some(KeyBinding::new(source, ConfiguredAction, None)));
        assert_eq!(configured_bindings().len(), 1);

        cx.update(|_, cx| rebuild_keymap(cx, embedded_bindings(), configured_bindings()));
        cx.simulate_keystrokes("ctrl-k");
        assert_eq!((embedded.get(), configured.get()), (0, 1));

        // Rebuilding again is idempotent: clear, then re-register in the same
        // order, so nothing accumulates and the order still holds.
        cx.update(|_, cx| rebuild_keymap(cx, embedded_bindings(), configured_bindings()));
        cx.simulate_keystrokes("ctrl-k");
        assert_eq!((embedded.get(), configured.get()), (0, 2));

        // Without the configured combo only the embedded default remains.
        cx.update(|_, cx| rebuild_keymap(cx, embedded_bindings(), []));
        cx.simulate_keystrokes("ctrl-k");
        assert_eq!((embedded.get(), configured.get()), (1, 2));

        // Registering the other way around (a rebuild bug) would let the
        // embedded default win; the helper's order is what prevents it.
        cx.update(|_, cx| {
            rebuild_keymap(
                cx,
                [],
                embedded_bindings().into_iter().chain(configured_bindings()),
            )
        });
        cx.simulate_keystrokes("ctrl-k");
        assert_eq!((embedded.get(), configured.get()), (1, 3));
        cx.update(|_, cx| rebuild_keymap(cx, configured_bindings(), embedded_bindings()));
        cx.simulate_keystrokes("ctrl-k");
        assert_eq!((embedded.get(), configured.get()), (2, 3));
    }

    struct RecorderHost {
        recorder: Option<Entity<ShortcutRecorder>>,
        other: crate::FocusHandle,
        root: crate::FocusHandle,
        action_runs: Rc<Cell<usize>>,
        raw_key_downs: Rc<Cell<usize>>,
    }

    impl Render for RecorderHost {
        fn render(
            &mut self,
            _: &mut crate::Window,
            cx: &mut crate::Context<Self>,
        ) -> impl IntoElement {
            let runs = self.action_runs.clone();
            let raw_key_downs = self.raw_key_downs.clone();
            let recorder_focus = self
                .recorder
                .as_ref()
                .map(|recorder| recorder.read(cx).focus_handle().clone());
            div()
                .size_full()
                .track_focus(&self.root)
                .on_action(move |_: &BoundAction, _, _| runs.set(runs.get() + 1))
                .on_key_down(move |_, _, _| raw_key_downs.set(raw_key_downs.get() + 1))
                .child(
                    div()
                        .id("recorder")
                        .size(px(10.))
                        .when_some(recorder_focus, |this, focus| this.track_focus(&focus)),
                )
                .child(div().id("other").size(px(10.)).track_focus(&self.other))
        }
    }

    struct RecorderFixture {
        view: Entity<RecorderHost>,
        events: Rc<RefCell<Vec<ShortcutRecorderEvent>>>,
        action_runs: Rc<Cell<usize>>,
        raw_key_downs: Rc<Cell<usize>>,
        global_runs: Rc<Cell<usize>>,
        _registration: crate::global_shortcuts::GlobalShortcutRegistration,
        _subscription: Subscription,
    }

    const CHORD: &str = "ctrl-shift-k";
    const CHORD_COMBO: &str = "primary-shift-k";

    fn recorder_fixture(
        cx: &mut TestAppContext,
    ) -> (RecorderFixture, &mut crate::VisualTestContext) {
        let action_runs = Rc::new(Cell::new(0));
        let raw_key_downs = Rc::new(Cell::new(0));
        let global_runs = Rc::new(Cell::new(0));
        let events = Rc::new(RefCell::new(Vec::new()));
        cx.update(|cx| cx.bind_keys([KeyBinding::new(CHORD, BoundAction, None)]));
        let (view, cx) = cx.add_window_view({
            let action_runs = action_runs.clone();
            let raw_key_downs = raw_key_downs.clone();
            move |window, cx| {
                let recorder = cx.new(|cx| {
                    ShortcutRecorder::new(
                        set_of(&[("find", ""), ("paste", "primary-v"), ("copy", "primary-c")]),
                        cx,
                    )
                });
                let _ = window;
                RecorderHost {
                    recorder: Some(recorder),
                    other: cx.focus_handle(),
                    root: cx.focus_handle(),
                    action_runs,
                    raw_key_downs,
                }
            }
        });
        let root = view.read_with(cx, |host, _| host.root.clone());
        focus_and_draw(cx, &root);
        let recorder = view.read_with(cx, |host, _| host.recorder.clone().expect("recorder"));
        let subscription = cx.update(|_, cx| {
            let events = events.clone();
            cx.subscribe(&recorder, move |_, event: &ShortcutRecorderEvent, _| {
                events.borrow_mut().push(event.clone())
            })
        });
        let registration = cx.update(|_, cx| {
            let global_runs = global_runs.clone();
            GlobalShortcuts::register(cx, CHORD_COMBO, move |_| {
                global_runs.set(global_runs.get() + 1)
            })
        });
        (
            RecorderFixture {
                view,
                events,
                action_runs,
                raw_key_downs,
                global_runs,
                _registration: registration,
                _subscription: subscription,
            },
            cx,
        )
    }

    fn start_recording(
        fixture: &RecorderFixture,
        cx: &mut crate::VisualTestContext,
        id: &str,
    ) -> bool {
        let recorder = fixture
            .view
            .read_with(cx, |host, _| host.recorder.clone().expect("recorder"));
        let started = recorder.update_in(cx, |recorder, window, cx| {
            recorder.start_recording(id, window, cx)
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        started
    }

    fn fire_global(cx: &mut crate::VisualTestContext) -> bool {
        cx.update(|_, cx| GlobalShortcuts::trigger(cx, CHORD_COMBO))
    }

    #[crate::test]
    fn recording_intercepts_bound_chords_and_suspends_global_shortcuts(cx: &mut TestAppContext) {
        // Completed, cancelled, blurred and released: whichever way
        // recording ends, the chord's action and its global shortcut fire
        // again, and neither fires while recording.
        for route in ["completed", "cancelled", "blurred", "released"] {
            let (fixture, cx) = recorder_fixture(cx);
            // Before recording, the global shortcut fires and so does the action.
            assert!(fire_global(cx), "{route}");
            assert_eq!(fixture.global_runs.get(), 1);
            cx.simulate_keystrokes(CHORD);
            assert_eq!(fixture.action_runs.get(), 1);

            assert!(start_recording(&fixture, cx, "find"), "{route}");
            let recorder_focus = fixture.view.read_with(cx, |host, cx| {
                host.recorder
                    .as_ref()
                    .expect("recorder")
                    .read(cx)
                    .focus_handle()
                    .clone()
            });
            cx.update(|window, _| assert!(recorder_focus.is_focused(window)));
            assert!(!fire_global(cx), "{route}: suspended while recording");
            assert_eq!(fixture.global_runs.get(), 1);

            match route {
                "completed" => {
                    // The colliding chord is recorded instead of running.
                    let raw_before = fixture.raw_key_downs.get();
                    cx.simulate_keystrokes(CHORD);
                    assert_eq!(fixture.action_runs.get(), 1, "action must not run");
                    assert_eq!(
                        fixture.raw_key_downs.get(),
                        raw_before,
                        "raw key listeners must not see consumed keystrokes"
                    );
                    let events = fixture.events.borrow().clone();
                    assert_eq!(
                        events,
                        vec![
                            ShortcutRecorderEvent::RecordingStarted {
                                shortcut: "find".into()
                            },
                            ShortcutRecorderEvent::RecordingEnded {
                                shortcut: "find".into(),
                                end: RecordingEnd::Completed
                            },
                            ShortcutRecorderEvent::KeymapChanged,
                        ]
                    );
                    let combo = fixture.view.read_with(cx, |host, cx| {
                        host.recorder
                            .as_ref()
                            .expect("recorder")
                            .read(cx)
                            .shortcuts()
                            .get("find")
                            .expect("find")
                            .combo
                            .clone()
                    });
                    assert_eq!(combo, CHORD_COMBO);
                }
                "cancelled" => {
                    cx.simulate_keystrokes("escape");
                    assert!(fixture.events.borrow().contains(
                        &ShortcutRecorderEvent::RecordingEnded {
                            shortcut: "find".into(),
                            end: RecordingEnd::Cancelled,
                        }
                    ));
                }
                "blurred" => {
                    let other = fixture.view.read_with(cx, |host, _| host.other.clone());
                    focus_and_draw(cx, &other);
                    assert!(fixture.events.borrow().contains(
                        &ShortcutRecorderEvent::RecordingEnded {
                            shortcut: "find".into(),
                            end: RecordingEnd::Blurred,
                        }
                    ));
                }
                _ => {
                    fixture.view.update(cx, |host, _| host.recorder = None);
                    cx.run_until_parked();
                }
            }
            cx.run_until_parked();
            assert!(
                cx.update(|_, cx| !GlobalShortcuts::is_suspended(cx)),
                "{route}"
            );

            // After the recording ended the chord is live again.
            assert!(fire_global(cx), "{route}: restored after recording");
            assert_eq!(fixture.global_runs.get(), 2);
            let runs_before = fixture.action_runs.get();
            let root = fixture.view.read_with(cx, |host, _| host.root.clone());
            focus_and_draw(cx, &root);
            cx.simulate_keystrokes(CHORD);
            assert_eq!(fixture.action_runs.get(), runs_before + 1, "{route}");
        }
    }

    #[crate::test]
    fn recording_ignores_bare_modifiers_and_refuses_collisions(cx: &mut TestAppContext) {
        let (fixture, cx) = recorder_fixture(cx);
        assert!(!start_recording(&fixture, cx, "unknown"));
        assert!(start_recording(&fixture, cx, "find"));

        // A bare modifier is consumed but recording continues.
        cx.simulate_modifiers_change(Modifiers::shift());
        cx.simulate_modifiers_change(Modifiers::none());
        let recorder = fixture
            .view
            .read_with(cx, |host, _| host.recorder.clone().expect("recorder"));
        assert_eq!(
            recorder.read_with(cx, |r, _| r.recording().cloned()),
            Some("find".into())
        );

        // Ctrl+V is used by "paste": refused, nothing stored, notice emitted.
        cx.simulate_keystrokes("ctrl-v");
        assert_eq!(recorder.read_with(cx, |r, _| r.recording().cloned()), None);
        let events = fixture.events.borrow().clone();
        assert!(
            events.contains(&ShortcutRecorderEvent::Refused(ShortcutRefusal {
                combo: "primary-v".into(),
                existing: "paste".into(),
                existing_label: "Shortcut paste".into(),
            }))
        );
        assert!(!events.contains(&ShortcutRecorderEvent::KeymapChanged));
        assert_eq!(
            recorder.read_with(cx, |r, _| r
                .shortcuts()
                .get("find")
                .map(|s| s.combo.clone())),
            Some(String::new())
        );

        // Page changes end recording as cancelled, and reset/restore announce
        // keymap changes only when something changed.
        assert!(start_recording(&fixture, cx, "find"));
        recorder.update(cx, |r, cx| r.cancel_recording(cx));
        assert!(
            fixture
                .events
                .borrow()
                .contains(&ShortcutRecorderEvent::RecordingEnded {
                    shortcut: "find".into(),
                    end: RecordingEnd::PageChanged,
                })
        );
        assert!(cx.update(|_, cx| !GlobalShortcuts::is_suspended(cx)));
        fixture.events.borrow_mut().clear();
        assert!(!recorder.update(cx, |r, cx| r.reset("find", cx)));
        assert!(fixture.events.borrow().is_empty());
        assert!(start_recording(&fixture, cx, "find"));
        cx.simulate_keystrokes("ctrl-f");
        fixture.events.borrow_mut().clear();
        assert!(recorder.update(cx, |r, cx| r.reset("find", cx)));
        assert_eq!(
            fixture.events.borrow().as_slice(),
            &[ShortcutRecorderEvent::KeymapChanged]
        );
        assert!(!recorder.update(cx, |r, cx| r.restore_all(cx)));
    }
}
