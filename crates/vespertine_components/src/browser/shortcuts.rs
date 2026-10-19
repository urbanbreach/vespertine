//! Actions and key bindings of the browser surface (CMP-022 R11).
//!
//! The actions are handled in the key context [`KEY_CONTEXT`]. The default chords use the platform
//! primary modifier (control on Linux). A chord is registered only when the embedder's keymap
//! does not already assign it to another command, so customized application chords win; reload has
//! no built-in chord and is bound only when the embedder configures one that parses.

use vespertine::{Action, App, KeyBinding, Keystroke, actions};

/// The key context name of the surface.
pub const KEY_CONTEXT: &str = "Browser";

actions!(
    browser,
    [
        /// Reloads the page, or retries after an error (R9).
        Reload,
        /// Focuses the address field and selects its text.
        FocusAddress,
        /// Asks the embedder for a new tab.
        NewTab,
        /// Asks the embedder to close this tab.
        CloseTab,
        /// Goes back in the page history.
        Back,
        /// Goes forward in the page history.
        Forward,
    ]
);

/// The chords of the surface's actions, as keystroke text such as `"ctrl-l"`. `None` leaves an
/// action unbound.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserShortcuts {
    /// Focus the address field.
    pub focus_address: Option<String>,
    /// Request a new tab.
    pub new_tab: Option<String>,
    /// Request closing the tab.
    pub close_tab: Option<String>,
    /// History back.
    pub back: Option<String>,
    /// History forward.
    pub forward: Option<String>,
    /// Reload; unbound by default.
    pub reload: Option<String>,
}

impl Default for BrowserShortcuts {
    fn default() -> Self {
        Self {
            focus_address: Some("ctrl-l".into()),
            new_tab: Some("ctrl-t".into()),
            close_tab: Some("ctrl-w".into()),
            back: Some("ctrl-[".into()),
            forward: Some("ctrl-]".into()),
            reload: None,
        }
    }
}

impl BrowserShortcuts {
    /// The focus-address chord as shown in hints, for example `"Ctrl L"`.
    pub fn focus_address_hint(&self) -> Option<String> {
        self.focus_address.as_deref().map(chord_hint)
    }
}

/// Formats a chord for display: `"ctrl-shift-l"` becomes `"Ctrl Shift L"`.
pub fn chord_hint(chord: &str) -> String {
    chord
        .split_whitespace()
        .map(|stroke| {
            stroke
                .split('-')
                .filter(|part| !part.is_empty())
                .map(|part| {
                    let mut chars = part.chars();
                    match chars.next() {
                        Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                        None => String::new(),
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Registers the default chords (skipping the ones the embedder's keymap already assigns).
pub fn bind_keys(cx: &mut App) {
    bind_shortcuts(cx, &BrowserShortcuts::default());
}

/// Registers the chords of `shortcuts`: each one only if it parses and the keymap does not already
/// assign it to another command. Registering the same chord for the same action again is a no-op.
pub fn bind_shortcuts(cx: &mut App, shortcuts: &BrowserShortcuts) {
    bind_one(cx, shortcuts.focus_address.as_deref(), FocusAddress);
    bind_one(cx, shortcuts.new_tab.as_deref(), NewTab);
    bind_one(cx, shortcuts.close_tab.as_deref(), CloseTab);
    bind_one(cx, shortcuts.back.as_deref(), Back);
    bind_one(cx, shortcuts.forward.as_deref(), Forward);
    bind_one(cx, shortcuts.reload.as_deref(), Reload);
}

/// The normalized keystrokes of a chord, or `None` when it does not parse.
fn normalized(chord: &str) -> Option<Vec<String>> {
    let strokes: Option<Vec<String>> = chord
        .split_whitespace()
        .map(|stroke| {
            Keystroke::parse(stroke)
                .ok()
                .map(|keystroke| keystroke.unparse())
        })
        .collect();
    strokes.filter(|strokes| !strokes.is_empty())
}

fn bind_one<A: Action>(cx: &mut App, chord: Option<&str>, action: A) {
    let Some(chord) = chord else {
        return;
    };
    let Some(wanted) = normalized(chord) else {
        return;
    };
    let keymap = cx.key_bindings();
    let (assigned_to_other, already_bound) = {
        let keymap = keymap.borrow();
        let mut other = false;
        let mut same = false;
        for binding in keymap.bindings() {
            let strokes: Vec<String> = binding
                .keystrokes()
                .iter()
                .map(|stroke| stroke.inner().unparse())
                .collect();
            if strokes != wanted {
                continue;
            }
            if binding.action().name() == action.name() {
                same = true;
            } else {
                other = true;
            }
        }
        (other, same)
    };
    if !assigned_to_other && !already_bound {
        cx.bind_keys([KeyBinding::new(chord, action, Some(KEY_CONTEXT))]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords_are_formatted_for_hints() {
        assert_eq!(chord_hint("ctrl-l"), "Ctrl L");
        assert_eq!(chord_hint("ctrl-shift-r"), "Ctrl Shift R");
        assert_eq!(chord_hint("f5"), "F5");
        assert_eq!(
            BrowserShortcuts::default().focus_address_hint().as_deref(),
            Some("Ctrl L")
        );
    }

    #[test]
    fn invalid_chords_do_not_normalize() {
        assert!(normalized("ctrl-l").is_some());
        assert!(normalized("").is_none());
        assert!(normalized("ctrl-a-b").is_none());
    }
}
