//! Keyboard and paste encoding and the input coalescer (CMP-021 R11-R14).

use vespertine::Modifiers;

/// ESC.
const ESC: u8 = 0x1B;
/// The bracketed-paste start marker, `ESC [ 2 0 0 ~`.
pub const PASTE_START: &[u8] = b"\x1b[200~";
/// The bracketed-paste end marker, `ESC [ 2 0 1 ~`.
pub const PASTE_END: &[u8] = b"\x1b[201~";

/// Translates a key event into the bytes to send to the session (R11); `None` means "not
/// handled" and the event falls through to ordinary actions.
///
/// `key` is the key name (`"enter"`, `"up"`, `"f5"`, `"a"`), `key_char` the typed (shift- and
/// IME-aware) character when present, `app_cursor` whether application-cursor mode is on.
/// Combinations with the platform modifier are never sent to the terminal.
pub fn keystroke_to_bytes(
    key: &str,
    key_char: Option<&str>,
    modifiers: Modifiers,
    app_cursor: bool,
) -> Option<Vec<u8>> {
    if modifiers.platform {
        return None;
    }
    if modifiers.alt {
        let without_alt = Modifiers {
            alt: false,
            ..modifiers
        };
        let mut bytes = keystroke_to_bytes(key, key_char, without_alt, app_cursor)?;
        bytes.insert(0, ESC);
        return Some(bytes);
    }
    if modifiers.control {
        return control_byte(key).map(|byte| vec![byte]);
    }
    plain_bytes(key, key_char, modifiers.shift, app_cursor)
}

/// The control encoding in caret notation (R11).
fn control_byte(key: &str) -> Option<u8> {
    match key {
        "space" | " " => return Some(0),
        "backspace" => return Some(0x08),
        "enter" => return Some(b'\r'),
        _ => {}
    }
    let mut chars = key.chars();
    let (Some(ch), None) = (chars.next(), chars.next()) else {
        return None;
    };
    match ch.to_ascii_lowercase() {
        letter @ 'a'..='z' => Some(letter as u8 - b'a' + 1),
        '@' => Some(0),
        '[' => Some(0x1B),
        '\\' => Some(0x1C),
        ']' => Some(0x1D),
        '^' => Some(0x1E),
        '_' | '/' => Some(0x1F),
        '?' => Some(0x7F),
        _ => None,
    }
}

fn plain_bytes(
    key: &str,
    key_char: Option<&str>,
    shift: bool,
    app_cursor: bool,
) -> Option<Vec<u8>> {
    let cursor_key = |final_byte: u8| {
        Some(if app_cursor {
            vec![ESC, b'O', final_byte]
        } else {
            vec![ESC, b'[', final_byte]
        })
    };
    match key {
        "enter" => return Some(vec![b'\r']),
        "backspace" => return Some(vec![0x7F]),
        "tab" => {
            return Some(if shift {
                vec![ESC, b'[', b'Z']
            } else {
                vec![b'\t']
            });
        }
        "escape" => return Some(vec![ESC]),
        "space" => return Some(vec![b' ']),
        "up" => return cursor_key(b'A'),
        "down" => return cursor_key(b'B'),
        "right" => return cursor_key(b'C'),
        "left" => return cursor_key(b'D'),
        "home" => return cursor_key(b'H'),
        "end" => return cursor_key(b'F'),
        "insert" => return Some(b"\x1b[2~".to_vec()),
        "delete" => return Some(b"\x1b[3~".to_vec()),
        "pageup" => return Some(b"\x1b[5~".to_vec()),
        "pagedown" => return Some(b"\x1b[6~".to_vec()),
        "f1" => return Some(b"\x1bOP".to_vec()),
        "f2" => return Some(b"\x1bOQ".to_vec()),
        "f3" => return Some(b"\x1bOR".to_vec()),
        "f4" => return Some(b"\x1bOS".to_vec()),
        "f5" => return Some(b"\x1b[15~".to_vec()),
        "f6" => return Some(b"\x1b[17~".to_vec()),
        "f7" => return Some(b"\x1b[18~".to_vec()),
        "f8" => return Some(b"\x1b[19~".to_vec()),
        "f9" => return Some(b"\x1b[20~".to_vec()),
        "f10" => return Some(b"\x1b[21~".to_vec()),
        "f11" => return Some(b"\x1b[23~".to_vec()),
        "f12" => return Some(b"\x1b[24~".to_vec()),
        _ => {}
    }
    if let Some(text) = key_char.filter(|text| !text.is_empty()) {
        return Some(text.as_bytes().to_vec());
    }
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(_), None) => Some(key.as_bytes().to_vec()),
        _ => None,
    }
}

/// A clipboard shortcut of the terminal (R12).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardShortcut {
    /// Control+shift+c (the platform modifier + c on macOS).
    Copy,
    /// Control+shift+v (the platform modifier + v on macOS).
    Paste,
}

/// Recognizes the copy and paste shortcuts. Plain control+c and control+v are never
/// intercepted: they are encoded as control bytes.
pub fn clipboard_shortcut(key: &str, modifiers: Modifiers) -> Option<ClipboardShortcut> {
    let chord = if cfg!(target_os = "macos") {
        modifiers.platform && !modifiers.control && !modifiers.alt && !modifiers.shift
    } else {
        modifiers.control && modifiers.shift && !modifiers.alt && !modifiers.platform
    };
    if !chord {
        return None;
    }
    match key.to_ascii_lowercase().as_str() {
        "c" => Some(ClipboardShortcut::Copy),
        "v" => Some(ClipboardShortcut::Paste),
        _ => None,
    }
}

/// The bytes for pasting `text` (R13): every bracketed-paste end marker is removed (so a paste
/// cannot terminate the bracket early and inject commands); in bracketed-paste mode the result
/// is wrapped in the start and end markers.
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let mut body = text.as_bytes().to_vec();
    // Removing a marker can splice its neighbours into a new one, so repeat until stable.
    while let Some(position) = body
        .windows(PASTE_END.len())
        .position(|window| window == PASTE_END)
    {
        body.drain(position..position + PASTE_END.len());
    }
    if !bracketed {
        return body;
    }
    let mut bytes = Vec::with_capacity(body.len() + PASTE_START.len() + PASTE_END.len());
    bytes.extend_from_slice(PASTE_START);
    bytes.extend_from_slice(&body);
    bytes.extend_from_slice(PASTE_END);
    bytes
}

/// A per-tab buffer of bytes destined for the session (R14).
///
/// [`push`](Self::push) returns `true` exactly when a flush must be scheduled: bytes were
/// pushed into an empty buffer. Pushes while a flush is pending only append.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InputCoalescer {
    buffer: Vec<u8>,
}

impl InputCoalescer {
    /// Appends `bytes`; `true` when the caller must schedule a flush (the buffer was empty and
    /// `bytes` is not). Pushing nothing never schedules.
    pub fn push(&mut self, bytes: &[u8]) -> bool {
        if bytes.is_empty() {
            return false;
        }
        let schedule = self.buffer.is_empty();
        self.buffer.extend_from_slice(bytes);
        schedule
    }

    /// Takes everything buffered, in order, leaving the buffer empty.
    pub fn take(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.buffer)
    }

    /// Whether nothing is buffered.
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    /// Drops everything buffered (the tab exited).
    pub fn clear(&mut self) {
        self.buffer.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(control: bool, alt: bool, shift: bool, platform: bool) -> Modifiers {
        Modifiers {
            control,
            alt,
            shift,
            platform,
            function: false,
        }
    }

    fn plain(key: &str, key_char: Option<&str>) -> Option<Vec<u8>> {
        keystroke_to_bytes(key, key_char, Modifiers::default(), false)
    }

    fn ctrl(key: &str) -> Option<Vec<u8>> {
        keystroke_to_bytes(key, None, mods(true, false, false, false), false)
    }

    #[test]
    fn plain_keys_encode_per_the_table() {
        assert_eq!(plain("a", Some("A")), Some(b"A".to_vec()));
        assert_eq!(plain("a", Some("a")), Some(b"a".to_vec()));
        assert_eq!(plain("a", None), Some(b"a".to_vec()));
        assert_eq!(plain("enter", Some("\n")), Some(vec![0x0D]));
        assert_eq!(plain("backspace", None), Some(vec![0x7F]));
        assert_eq!(plain("tab", None), Some(vec![0x09]));
        assert_eq!(
            keystroke_to_bytes("tab", None, mods(false, false, true, false), false),
            Some(b"\x1b[Z".to_vec())
        );
        assert_eq!(plain("escape", None), Some(vec![0x1B]));
        assert_eq!(plain("space", Some(" ")), Some(vec![b' ']));
        assert_eq!(plain("delete", None), Some(b"\x1b[3~".to_vec()));
        assert_eq!(plain("insert", None), Some(b"\x1b[2~".to_vec()));
        assert_eq!(plain("pageup", None), Some(b"\x1b[5~".to_vec()));
        assert_eq!(plain("pagedown", None), Some(b"\x1b[6~".to_vec()));
        assert_eq!(plain("f1", None), Some(b"\x1bOP".to_vec()));
        assert_eq!(plain("f4", None), Some(b"\x1bOS".to_vec()));
        assert_eq!(plain("f5", None), Some(b"\x1b[15~".to_vec()));
        assert_eq!(plain("f12", None), Some(b"\x1b[24~".to_vec()));
    }

    #[test]
    fn cursor_keys_follow_application_cursor_mode() {
        let key = |name: &str, app| keystroke_to_bytes(name, None, Modifiers::default(), app);
        assert_eq!(key("up", false), Some(b"\x1b[A".to_vec()));
        assert_eq!(key("up", true), Some(b"\x1bOA".to_vec()));
        assert_eq!(key("end", true), Some(b"\x1bOF".to_vec()));
        assert_eq!(key("home", false), Some(b"\x1b[H".to_vec()));
        assert_eq!(key("left", false), Some(b"\x1b[D".to_vec()));
        assert_eq!(key("right", true), Some(b"\x1bOC".to_vec()));
        assert_eq!(key("down", false), Some(b"\x1b[B".to_vec()));
    }

    #[test]
    fn control_encoding_is_caret_notation() {
        assert_eq!(ctrl("c"), Some(vec![0x03]));
        assert_eq!(ctrl("z"), Some(vec![0x1A]));
        assert_eq!(ctrl("a"), Some(vec![0x01]));
        assert_eq!(ctrl("space"), Some(vec![0x00]));
        assert_eq!(ctrl("@"), Some(vec![0x00]));
        assert_eq!(ctrl("["), Some(vec![0x1B]));
        assert_eq!(ctrl("\\"), Some(vec![0x1C]));
        assert_eq!(ctrl("]"), Some(vec![0x1D]));
        assert_eq!(ctrl("^"), Some(vec![0x1E]));
        assert_eq!(ctrl("_"), Some(vec![0x1F]));
        assert_eq!(ctrl("/"), Some(vec![0x1F]));
        assert_eq!(ctrl("?"), Some(vec![0x7F]));
        assert_eq!(ctrl("backspace"), Some(vec![0x08]));
        assert_eq!(ctrl("enter"), Some(vec![0x0D]));
        assert_eq!(ctrl("1"), None);
        assert_eq!(ctrl("up"), None);
    }

    #[test]
    fn alt_prefixes_escape() {
        let alt = |key: &str, control| {
            keystroke_to_bytes(key, None, mods(control, true, false, false), false)
        };
        assert_eq!(alt("b", false), Some(vec![0x1B, b'b']));
        assert_eq!(alt("c", true), Some(vec![0x1B, 0x03]));
        assert_eq!(alt("1", true), None);
        assert_eq!(alt("up", false), Some(b"\x1b\x1b[A".to_vec()));
    }

    #[test]
    fn platform_and_unknown_keys_are_not_handled() {
        for key in ["a", "c", "enter", "up"] {
            assert_eq!(
                keystroke_to_bytes(key, Some(key), mods(false, false, false, true), false),
                None
            );
            assert_eq!(
                keystroke_to_bytes(key, None, mods(true, true, true, true), false),
                None
            );
        }
        assert_eq!(plain("capslock", None), None);
        assert_eq!(plain("capslock", Some("")), None);
        assert_eq!(plain("unknown", None), None);
        assert_eq!(plain("é", None), Some("é".as_bytes().to_vec()));
    }

    #[test]
    fn clipboard_shortcuts_need_the_full_chord() {
        if cfg!(target_os = "macos") {
            return;
        }
        let chord = mods(true, false, true, false);
        assert_eq!(
            clipboard_shortcut("c", chord),
            Some(ClipboardShortcut::Copy)
        );
        assert_eq!(
            clipboard_shortcut("v", chord),
            Some(ClipboardShortcut::Paste)
        );
        assert_eq!(
            clipboard_shortcut("C", chord),
            Some(ClipboardShortcut::Copy)
        );
        assert_eq!(
            clipboard_shortcut("c", mods(true, false, false, false)),
            None
        );
        assert_eq!(clipboard_shortcut("x", chord), None);
    }

    #[test]
    fn paste_strips_the_end_marker_and_brackets() {
        assert_eq!(paste_bytes("hi", false), b"hi");
        assert_eq!(paste_bytes("hi", true), b"\x1b[200~hi\x1b[201~");
        assert_eq!(
            paste_bytes("a\x1b[201~rm -rf", true),
            b"\x1b[200~arm -rf\x1b[201~"
        );
        assert_eq!(paste_bytes("a\x1b[201~b", false), b"ab");
        // Removing one marker must not splice a new one together.
        assert_eq!(
            paste_bytes("\x1b[20\x1b[201~1~x", true),
            b"\x1b[200~x\x1b[201~"
        );
    }

    #[test]
    fn coalescer_schedules_once_per_burst() {
        let mut input = InputCoalescer::default();
        assert!(input.is_empty());
        assert!(!input.push(b""));
        assert!(input.push(b"a"));
        assert!(!input.push(b"b"));
        assert!(!input.push(b""));
        assert_eq!(input.take(), b"ab");
        assert!(input.is_empty());
        assert!(input.push(b"c"));
        assert_eq!(input.take(), b"c");
    }
}
