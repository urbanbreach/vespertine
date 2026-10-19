//! Key classification for lists with keyboard navigation (CMP-002 R22).

use vespertine::Modifiers;

/// What a key means to a menu, select, combobox or picker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListKey {
    /// Up, or Ctrl-P.
    Up,
    /// Down, or Ctrl-N.
    Down,
    /// Home.
    Home,
    /// End.
    End,
    /// Enter: accept.
    Enter,
    /// Ctrl-Enter (or the platform primary modifier with Enter): the secondary accept.
    SecondaryEnter,
    /// Escape.
    Escape,
    /// Backspace.
    Backspace,
    /// Everything else is passed on.
    Other,
}

/// Classifies a keystroke given its key name (`"up"`, `"enter"`, `"n"`, ...) and modifiers.
///
/// `claim_ctrl_np` states whether Ctrl-N and Ctrl-P may be claimed as Down and Up: pass `false`
/// when either chord is a text-editing binding in the active key context (the claim is only
/// window-frame-wide where neither chord is).
pub fn classify_key(key: &str, modifiers: Modifiers, claim_ctrl_np: bool) -> ListKey {
    let only_control = modifiers.control
        && !modifiers.alt
        && !modifiers.shift
        && !modifiers.platform
        && !modifiers.function;
    if key == "enter" {
        return if modifiers.secondary() || modifiers.control {
            ListKey::SecondaryEnter
        } else if !modifiers.modified() {
            ListKey::Enter
        } else {
            ListKey::Other
        };
    }
    if only_control && claim_ctrl_np {
        match key {
            "n" => return ListKey::Down,
            "p" => return ListKey::Up,
            _ => {}
        }
    }
    if modifiers.modified() {
        return ListKey::Other;
    }
    match key {
        "up" => ListKey::Up,
        "down" => ListKey::Down,
        "home" => ListKey::Home,
        "end" => ListKey::End,
        "escape" => ListKey::Escape,
        "backspace" => ListKey::Backspace,
        _ => ListKey::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctrl() -> Modifiers {
        Modifiers {
            control: true,
            ..Default::default()
        }
    }

    #[test]
    fn plain_keys() {
        let none = Modifiers::default();
        assert_eq!(classify_key("up", none, true), ListKey::Up);
        assert_eq!(classify_key("down", none, true), ListKey::Down);
        assert_eq!(classify_key("home", none, true), ListKey::Home);
        assert_eq!(classify_key("end", none, true), ListKey::End);
        assert_eq!(classify_key("enter", none, true), ListKey::Enter);
        assert_eq!(classify_key("escape", none, true), ListKey::Escape);
        assert_eq!(classify_key("backspace", none, true), ListKey::Backspace);
        assert_eq!(classify_key("a", none, true), ListKey::Other);
    }

    #[test]
    fn control_chords() {
        assert_eq!(classify_key("n", ctrl(), true), ListKey::Down);
        assert_eq!(classify_key("p", ctrl(), true), ListKey::Up);
        assert_eq!(classify_key("enter", ctrl(), true), ListKey::SecondaryEnter);
        assert_eq!(
            classify_key("enter", Modifiers::secondary_key(), true),
            ListKey::SecondaryEnter
        );
        // Text-editing contexts keep the chords.
        assert_eq!(classify_key("n", ctrl(), false), ListKey::Other);
        assert_eq!(classify_key("p", ctrl(), false), ListKey::Other);
        // Other modifiers do not make a chord.
        let shifted = Modifiers {
            control: true,
            shift: true,
            ..Default::default()
        };
        assert_eq!(classify_key("n", shifted, true), ListKey::Other);
        assert_eq!(classify_key("up", shifted, true), ListKey::Other);
    }
}
