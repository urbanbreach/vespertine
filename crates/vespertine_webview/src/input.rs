//! Pure translation of framework input into helper commands (PLT-016 R1 to R16): modifier masks,
//! key names, wheel conversion, pointer press ownership and text-input state conversions. Nothing
//! here needs a display.

use crate::protocol::Command;
use crate::state::InputState;
use std::ops::Range;
use vespertine::{
    Bounds, Modifiers, MouseButton, NavigationDirection, Pixels, Point, ScrollDelta, point, px,
    size,
};

/// GDK modifier bits (PLT-016 R3).
pub const MOD_SHIFT: u32 = 1;
pub const MOD_CONTROL: u32 = 4;
pub const MOD_ALT: u32 = 8;
pub const MOD_SUPER: u32 = 67_108_864;

/// Pixels per wheel line and the smooth-scroll divisor (PLT-016 R6).
pub const PIXELS_PER_LINE: f32 = 16.0;
pub const WHEEL_DIVISOR: f64 = 40.0;

/// Mouse button numbers (PLT-016 R3).
pub const BUTTON_LEFT: u32 = 1;
pub const BUTTON_MIDDLE: u32 = 2;
pub const BUTTON_RIGHT: u32 = 3;
pub const BUTTON_BACK: u32 = 8;
pub const BUTTON_FORWARD: u32 = 9;

/// The GDK modifier mask of `modifiers`: shift 1, control 4, alt 8, platform/super 67108864.
/// Caps lock and num lock are not forwarded.
pub fn modifier_mask(modifiers: &Modifiers) -> u32 {
    let mut mask = 0;
    if modifiers.shift {
        mask |= MOD_SHIFT;
    }
    if modifiers.control {
        mask |= MOD_CONTROL;
    }
    if modifiers.alt {
        mask |= MOD_ALT;
    }
    if modifiers.platform {
        mask |= MOD_SUPER;
    }
    mask
}

/// The pressed-button state bit of `button`: 1 shifted left by (button + 7); left 256, middle
/// 512, right 1024.
pub fn button_state_bit(button: u32) -> u32 {
    1 << (button + 7)
}

/// The helper's button number of a framework button.
pub fn button_number(button: MouseButton) -> u32 {
    match button {
        MouseButton::Left => BUTTON_LEFT,
        MouseButton::Middle => BUTTON_MIDDLE,
        MouseButton::Right => BUTTON_RIGHT,
        MouseButton::Navigate(NavigationDirection::Back) => BUTTON_BACK,
        MouseButton::Navigate(NavigationDirection::Forward) => BUTTON_FORWARD,
    }
}

/// Whether a button is forwarded to the page (left, middle, right).
pub fn is_forwarded_button(button: u32) -> bool {
    matches!(button, BUTTON_LEFT | BUTTON_MIDDLE | BUTTON_RIGHT)
}

/// Converts a framework wheel delta to the helper's smooth-scroll units: line deltas are first
/// converted at 16 pixels per line, then `dx = -pixels_x / 40`, `dy = -pixels_y / 40`, so
/// scrolling content down by 250 logical pixels gives `dy = +6.25`.
pub fn wheel_deltas(delta: &ScrollDelta) -> (f64, f64) {
    let pixels = match delta {
        ScrollDelta::Pixels(pixels) => (pixels.x.as_f32(), pixels.y.as_f32()),
        ScrollDelta::Lines(lines) => (lines.x * PIXELS_PER_LINE, lines.y * PIXELS_PER_LINE),
    };
    (
        -f64::from(pixels.0) / WHEEL_DIVISOR,
        -f64::from(pixels.1) / WHEEL_DIVISOR,
    )
}

/// Framework key names and the helper key names they map to (PLT-016 R11).
const KEY_NAMES: [(&str, &str); 15] = [
    ("enter", "Return"),
    ("backspace", "BackSpace"),
    ("delete", "Delete"),
    ("escape", "Escape"),
    ("tab", "Tab"),
    ("space", "space"),
    ("insert", "Insert"),
    ("left", "Left"),
    ("right", "Right"),
    ("up", "Up"),
    ("down", "Down"),
    ("home", "Home"),
    ("end", "End"),
    ("pageup", "Page_Up"),
    ("pagedown", "Page_Down"),
];

/// The key name sent to the helper (PLT-016 R11).
///
/// A keystroke with character text of exactly one character, which is not a control character,
/// while neither control nor platform is held, is named by that character. Otherwise the
/// framework key name goes through the table above; any other name passes through unchanged.
pub fn key_name(key: &str, text: Option<&str>, modifiers: &Modifiers) -> String {
    if let Some(text) = text {
        let mut chars = text.chars();
        if let (Some(character), None) = (chars.next(), chars.next())
            && !character.is_control()
            && !modifiers.control
            && !modifiers.platform
        {
            return text.to_owned();
        }
    }
    if let Some((_, name)) = KEY_NAMES.iter().find(|(framework, _)| *framework == key) {
        return (*name).to_owned();
    }
    if let Some(number) = key.strip_prefix('f')
        && let Ok(number) = number.parse::<u32>()
        && (1..=24).contains(&number)
        && key == format!("f{number}")
    {
        return format!("F{number}");
    }
    key.to_owned()
}

/// What a key-down event becomes for a focused, live page (PLT-016 R10 steps 3 to 5; step 1 is
/// the page-provided menu and step 2 the framework actions, both handled before this runs).
#[derive(Clone, Debug, PartialEq)]
pub enum KeyDownAction {
    /// Committed character input.
    Commit(String),
    Copy,
    Cut,
    Paste,
    Key(Command),
}

/// Classifies a key-down event.
///
/// `accepts_text_input` is the stored input-method "focused" flag: the platform text-input path
/// produced text for the key only when the page has an editable focus.
pub fn classify_key_down(
    key: &str,
    key_char: Option<&str>,
    modifiers: &Modifiers,
    accepts_text_input: bool,
) -> KeyDownAction {
    if accepts_text_input
        && let Some(text) = key_char.filter(|text| !text.is_empty())
        && !modifiers.control
        && !modifiers.platform
    {
        return KeyDownAction::Commit(text.to_owned());
    }
    if modifiers.control && !modifiers.alt {
        match key {
            "c" => return KeyDownAction::Copy,
            "x" => return KeyDownAction::Cut,
            "v" => return KeyDownAction::Paste,
            _ => {}
        }
    }
    KeyDownAction::Key(Command::KeyDown {
        key: key_name(key, key_char, modifiers),
        text: key_char.unwrap_or_default().to_owned(),
        mods: modifier_mask(modifiers),
    })
}

/// The key-up command for a keystroke.
pub fn key_up_command(key: &str, key_char: Option<&str>, modifiers: &Modifiers) -> Command {
    Command::KeyUp {
        key: key_name(key, key_char, modifiers),
        text: key_char.unwrap_or_default().to_owned(),
        mods: modifier_mask(modifiers),
    }
}

// ---------------------------------------------------------------------------------------------
// Pointer press ownership
// ---------------------------------------------------------------------------------------------

/// The situation of one pointer event.
#[derive(Copy, Clone, Debug)]
pub struct PointerContext {
    /// The surface presentation is live and a native page exists (PLT-016 R1).
    pub eligible: bool,
    /// A framework drag-and-drop is active.
    pub dragging: bool,
    /// The page area is the topmost pointer target at the pointer position (PLT-015 R19).
    pub hovered: bool,
}

/// The commands produced for a pointer event and whether the page element consumed it.
#[derive(Debug, Default, PartialEq)]
pub struct PointerOutcome {
    pub commands: Vec<Command>,
    pub consumed: bool,
}

/// Tracks the buttons forwarded as "down", the hover state and the last known position.
#[derive(Debug, Default)]
pub struct PointerState {
    held: [bool; 3],
    inside: bool,
    last: (f64, f64),
}

impl PointerState {
    fn index(button: u32) -> Option<usize> {
        is_forwarded_button(button).then(|| button as usize - 1)
    }

    fn held_mask(&self) -> u32 {
        self.held
            .iter()
            .enumerate()
            .filter(|(_, held)| **held)
            .map(|(index, _)| button_state_bit(index as u32 + 1))
            .fold(0, |mask, bit| mask | bit)
    }

    /// Whether any forwarded button is held.
    pub fn any_held(&self) -> bool {
        self.held.iter().any(|held| *held)
    }

    /// Whether `button` is held.
    pub fn is_held(&self, button: u32) -> bool {
        Self::index(button).is_some_and(|index| self.held[index])
    }

    /// A button press at `position` (relative to the page origin).
    ///
    /// Back and forward buttons over the page are consumed and ignored. A forwarded press is
    /// consumed only when eligible, no drag is active and the page area is topmost.
    pub fn press(
        &mut self,
        context: PointerContext,
        button: u32,
        position: (f64, f64),
        mods: u32,
    ) -> PointerOutcome {
        if !context.hovered && !self.any_held() {
            return PointerOutcome::default();
        }
        if matches!(button, BUTTON_BACK | BUTTON_FORWARD) {
            return PointerOutcome {
                commands: Vec::new(),
                consumed: context.hovered,
            };
        }
        let Some(index) = Self::index(button) else {
            return PointerOutcome::default();
        };
        if !context.eligible || context.dragging || !context.hovered {
            return PointerOutcome::default();
        }
        self.held[index] = true;
        self.inside = true;
        self.last = position;
        PointerOutcome {
            commands: vec![Command::Down {
                x: position.0,
                y: position.1,
                button,
                mods,
            }],
            consumed: true,
        }
    }

    /// A button release: forwarded (and the button forgotten) only for a button in the held set.
    pub fn release(&mut self, button: u32, position: (f64, f64), mods: u32) -> PointerOutcome {
        let Some(index) = Self::index(button) else {
            return PointerOutcome::default();
        };
        if !self.held[index] {
            return PointerOutcome::default();
        }
        self.held[index] = false;
        self.last = position;
        PointerOutcome {
            commands: vec![Command::Up {
                x: position.0,
                y: position.1,
                button,
                mods,
            }],
            consumed: true,
        }
    }

    /// A pointer move. While a forwarded button is held moves keep flowing wherever the pointer
    /// is; otherwise they are forwarded only inside the topmost visible page area, and leaving
    /// it (or being covered by an overlay) sends one "leave".
    pub fn moved(
        &mut self,
        context: PointerContext,
        position: (f64, f64),
        mods: u32,
    ) -> Vec<Command> {
        if context.dragging {
            // A framework drag took over: never leave a press stuck in the page.
            let mut commands = self.release_all(position);
            self.last = position;
            if self.inside {
                self.inside = false;
                commands.push(Command::Leave {
                    x: position.0,
                    y: position.1,
                });
            }
            return commands;
        }
        if !context.eligible {
            self.last = position;
            return Vec::new();
        }
        if self.any_held() {
            self.last = position;
            return vec![Command::Move {
                x: position.0,
                y: position.1,
                mods: mods | self.held_mask(),
            }];
        }
        if context.hovered {
            self.inside = true;
            self.last = position;
            return vec![Command::Move {
                x: position.0,
                y: position.1,
                mods,
            }];
        }
        self.last = position;
        if self.inside {
            self.inside = false;
            return vec![Command::Leave {
                x: position.0,
                y: position.1,
            }];
        }
        Vec::new()
    }

    /// The pointer left the window: end hover unless a press keeps the pointer captured.
    pub fn exited(&mut self) -> Vec<Command> {
        if self.inside && !self.any_held() {
            self.inside = false;
            return vec![Command::Leave {
                x: self.last.0,
                y: self.last.1,
            }];
        }
        Vec::new()
    }

    /// A wheel event over the page.
    pub fn scrolled(
        &mut self,
        context: PointerContext,
        position: (f64, f64),
        delta: &ScrollDelta,
        mods: u32,
    ) -> PointerOutcome {
        if !context.eligible || !context.hovered {
            return PointerOutcome::default();
        }
        let (dx, dy) = wheel_deltas(delta);
        self.last = position;
        PointerOutcome {
            commands: vec![Command::Scroll {
                x: position.0,
                y: position.1,
                dx,
                dy,
                mods,
            }],
            consumed: true,
        }
    }

    fn release_all(&mut self, position: (f64, f64)) -> Vec<Command> {
        let mut commands = Vec::new();
        for (index, held) in self.held.iter_mut().enumerate() {
            if *held {
                *held = false;
                commands.push(Command::Up {
                    x: position.0,
                    y: position.1,
                    button: index as u32 + 1,
                    mods: 0,
                });
            }
        }
        commands
    }

    /// The presentation left live (or the page went away): release every held button and send
    /// one pointer-leave so the page never keeps a stuck press or hover (PLT-016 R1).
    pub fn leave_live(&mut self) -> Vec<Command> {
        let position = self.last;
        let mut commands = self.release_all(position);
        if self.inside || !commands.is_empty() {
            commands.push(Command::Leave {
                x: position.0,
                y: position.1,
            });
        }
        self.inside = false;
        commands
    }
}

/// Coordinates relative to the page element (PLT-016 R2): the framework position minus the
/// element origin, not clamped.
pub fn relative_position(position: Point<Pixels>, origin: Point<Pixels>) -> (f64, f64) {
    (
        f64::from(position.x.as_f32()) - f64::from(origin.x.as_f32()),
        f64::from(position.y.as_f32()) - f64::from(origin.y.as_f32()),
    )
}

// ---------------------------------------------------------------------------------------------
// Text-input state conversions (PLT-016 R14)
// ---------------------------------------------------------------------------------------------

/// Length of `text` in UTF-16 code units.
pub fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// Converts a byte offset into `text` to UTF-16 units. An offset that is out of range or not on
/// a character boundary maps to the whole text length.
pub fn byte_offset_to_utf16(text: &str, offset: usize) -> usize {
    if offset > text.len() || !text.is_char_boundary(offset) {
        return utf16_len(text);
    }
    utf16_len(&text[..offset])
}

/// The selected range in UTF-16 units from the cursor and selection byte offsets: range =
/// min..max, reversed when the cursor is before the selection anchor.
pub fn selected_utf16_range(
    text: &str,
    cursor: Option<usize>,
    selection: Option<usize>,
) -> (Range<usize>, bool) {
    let whole = utf16_len(text);
    let cursor = cursor.map_or(whole, |offset| byte_offset_to_utf16(text, offset));
    let selection = selection.map_or(whole, |offset| byte_offset_to_utf16(text, offset));
    (
        cursor.min(selection)..cursor.max(selection),
        cursor < selection,
    )
}

/// The text of a UTF-16 `range` after clamping both ends to the text length, with the clamped
/// range.
pub fn text_in_utf16_range(text: &str, range: Range<usize>) -> (String, Range<usize>) {
    let units: Vec<u16> = text.encode_utf16().collect();
    let start = range.start.min(units.len());
    let end = range.end.min(units.len()).max(start);
    (String::from_utf16_lossy(&units[start..end]), start..end)
}

/// The caret rectangle in logical pixels: the element origin plus the caret (device pixels)
/// divided by the scale. Defaults without a reported caret: x 0, y 0, width 1, height 18.
pub fn caret_bounds(origin: Point<Pixels>, caret: Option<[i32; 4]>, scale: f32) -> Bounds<Pixels> {
    let [x, y, width, height] = caret.unwrap_or([0, 0, 1, 18]);
    let scale = if scale > 0.0 { scale } else { 1.0 };
    Bounds {
        origin: origin + point(px(x as f32 / scale), px(y as f32 / scale)),
        size: size(px(width as f32 / scale), px(height as f32 / scale)),
    }
}

/// Convenience: the selected range of a stored input state.
pub fn selected_range_of(state: &InputState) -> (Range<usize>, bool) {
    match &state.text {
        Some(text) => selected_utf16_range(text, state.cursor, state.selection),
        None => (0..0, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vespertine::{Modifiers, point, px};

    fn mods(shift: bool, control: bool, alt: bool, platform: bool) -> Modifiers {
        Modifiers {
            shift,
            control,
            alt,
            platform,
            function: false,
        }
    }

    #[test]
    fn modifier_masks() {
        assert_eq!(
            modifier_mask(&mods(true, true, true, true)),
            1 + 4 + 8 + 67_108_864
        );
        assert_eq!(modifier_mask(&Modifiers::default()), 0);
        assert_eq!(button_state_bit(BUTTON_LEFT), 256);
        assert_eq!(button_state_bit(BUTTON_MIDDLE), 512);
        assert_eq!(button_state_bit(BUTTON_RIGHT), 1024);
    }

    #[test]
    fn key_names_follow_the_table() {
        let none = Modifiers::default();
        for (key, expected) in [
            ("enter", "Return"),
            ("backspace", "BackSpace"),
            ("delete", "Delete"),
            ("escape", "Escape"),
            ("tab", "Tab"),
            ("left", "Left"),
            ("right", "Right"),
            ("up", "Up"),
            ("down", "Down"),
            ("home", "Home"),
            ("end", "End"),
            ("pageup", "Page_Up"),
            ("pagedown", "Page_Down"),
            ("space", "space"),
            ("insert", "Insert"),
            ("f1", "F1"),
            ("f5", "F5"),
            ("f12", "F12"),
            ("f24", "F24"),
            ("f25", "f25"),
            ("f0", "f0"),
            ("f", "f"),
            ("fn", "fn"),
            ("KP_Add", "KP_Add"),
        ] {
            assert_eq!(key_name(key, None, &none), expected, "{key}");
        }
        // A single printable character wins, also with shift.
        assert_eq!(
            key_name("a", Some("A"), &mods(true, false, false, false)),
            "A"
        );
        assert_eq!(
            key_name("1", Some("!"), &mods(true, false, false, false)),
            "!"
        );
        // Control (or platform) disables the character path: control+c is named "c".
        assert_eq!(
            key_name("c", Some("c"), &mods(false, true, false, false)),
            "c"
        );
        assert_eq!(key_name("enter", Some("\r"), &none), "Return");
        // Multi-character text is not a single character.
        assert_eq!(key_name("a", Some("ab"), &none), "a");
        assert_eq!(key_name("é", Some("é"), &none), "é");
    }

    #[test]
    fn wheel_conversion() {
        assert_eq!(
            wheel_deltas(&ScrollDelta::Pixels(point(px(0.), px(-250.)))),
            (-0.0, 6.25)
        );
        let (dx, dy) = wheel_deltas(&ScrollDelta::Lines(point(0.0, -1.0)));
        assert_eq!((dx, dy), (0.0, 0.4));
        let (dx, _) = wheel_deltas(&ScrollDelta::Pixels(point(px(40.), px(0.))));
        assert_eq!(dx, -1.0);
    }

    #[test]
    fn key_down_classification() {
        let none = Modifiers::default();
        assert_eq!(
            classify_key_down("a", Some("a"), &none, true),
            KeyDownAction::Commit("a".into())
        );
        // Without an editable focus the same key is a plain key event.
        assert_eq!(
            classify_key_down("a", Some("a"), &none, false),
            KeyDownAction::Key(Command::KeyDown {
                key: "a".into(),
                text: "a".into(),
                mods: 0
            })
        );
        let ctrl = mods(false, true, false, false);
        assert_eq!(
            classify_key_down("c", None, &ctrl, true),
            KeyDownAction::Copy
        );
        assert_eq!(
            classify_key_down("x", None, &ctrl, false),
            KeyDownAction::Cut
        );
        assert_eq!(
            classify_key_down("v", None, &ctrl, false),
            KeyDownAction::Paste
        );
        // Control with alt is not a clipboard chord; control+other is a key.
        assert!(matches!(
            classify_key_down("v", None, &mods(false, true, true, false), false),
            KeyDownAction::Key(_)
        ));
        assert_eq!(
            classify_key_down("enter", None, &none, true),
            KeyDownAction::Key(Command::KeyDown {
                key: "Return".into(),
                text: String::new(),
                mods: 0
            })
        );
        // The platform modifier never maps to the clipboard on Linux.
        assert!(matches!(
            classify_key_down("c", None, &mods(false, false, false, true), false),
            KeyDownAction::Key(_)
        ));
    }

    fn context(eligible: bool, dragging: bool, hovered: bool) -> PointerContext {
        PointerContext {
            eligible,
            dragging,
            hovered,
        }
    }

    #[test]
    fn press_ownership_forwards_matching_events_only() {
        let live = context(true, false, true);
        let mut pointer = PointerState::default();
        let left = pointer.press(live, 1, (1.0, 2.0), 0);
        assert!(left.consumed && matches!(left.commands[..], [Command::Down { button: 1, .. }]));
        let right = pointer.press(live, 3, (1.0, 2.0), 0);
        assert!(right.consumed);
        let up_right = pointer.release(3, (1.0, 2.0), 0);
        assert!(matches!(
            up_right.commands[..],
            [Command::Up { button: 3, .. }]
        ));
        let up_left = pointer.release(1, (1.0, 2.0), 0);
        assert!(matches!(
            up_left.commands[..],
            [Command::Up { button: 1, .. }]
        ));
        // An up without a matching down is dropped and not consumed.
        let stray = pointer.release(2, (0.0, 0.0), 0);
        assert_eq!(stray, PointerOutcome::default());
        // Back and forward are consumed over the page and send nothing.
        let back = pointer.press(live, BUTTON_BACK, (0.0, 0.0), 0);
        assert!(back.consumed && back.commands.is_empty());
        let forward = pointer.press(live, BUTTON_FORWARD, (0.0, 0.0), 0);
        assert!(forward.consumed && forward.commands.is_empty());
        // A down during a framework drag sends nothing and is not consumed.
        let drag = pointer.press(context(true, true, true), 1, (0.0, 0.0), 0);
        assert_eq!(drag, PointerOutcome::default());
        assert!(!pointer.any_held());
    }

    #[test]
    fn moves_follow_capture_and_hover() {
        let mut pointer = PointerState::default();
        let inside = context(true, false, true);
        let outside = context(true, false, false);
        // Outside without a held button: nothing.
        assert!(pointer.moved(outside, (-5.0, 5.0), 0).is_empty());
        // Inside: forwarded.
        assert_eq!(
            pointer.moved(inside, (5.0, 5.0), 0),
            vec![Command::Move {
                x: 5.0,
                y: 5.0,
                mods: 0
            }]
        );
        // Leaving sends exactly one leave.
        assert_eq!(
            pointer.moved(outside, (-5.0, 5.0), 0),
            vec![Command::Leave { x: -5.0, y: 5.0 }]
        );
        assert!(pointer.moved(outside, (-6.0, 5.0), 0).is_empty());
        // While a button is held moves outside are forwarded with the pressed-button bit.
        pointer.press(inside, 1, (5.0, 5.0), 0);
        assert_eq!(
            pointer.moved(outside, (300.0, 400.0), MOD_SHIFT),
            vec![Command::Move {
                x: 300.0,
                y: 400.0,
                mods: 1 + 256
            }]
        );
        pointer.press(inside, 3, (5.0, 5.0), 0);
        assert_eq!(
            pointer.moved(outside, (1.0, 1.0), 0),
            vec![Command::Move {
                x: 1.0,
                y: 1.0,
                mods: 256 + 1024
            }]
        );
    }

    #[test]
    fn a_drag_releases_held_buttons() {
        let mut pointer = PointerState::default();
        pointer.press(context(true, false, true), 1, (5.0, 5.0), 0);
        let commands = pointer.moved(context(true, true, true), (7.0, 7.0), 0);
        assert_eq!(
            commands,
            vec![
                Command::Up {
                    x: 7.0,
                    y: 7.0,
                    button: 1,
                    mods: 0
                },
                Command::Leave { x: 7.0, y: 7.0 }
            ]
        );
        assert!(!pointer.any_held());
    }

    #[test]
    fn leaving_live_releases_once() {
        let mut pointer = PointerState::default();
        pointer.press(context(true, false, true), 1, (5.0, 6.0), 0);
        let commands = pointer.leave_live();
        assert_eq!(
            commands,
            vec![
                Command::Up {
                    x: 5.0,
                    y: 6.0,
                    button: 1,
                    mods: 0
                },
                Command::Leave { x: 5.0, y: 6.0 }
            ]
        );
        assert!(pointer.leave_live().is_empty());
        // Hover without a press: only a leave.
        pointer.moved(context(true, false, true), (1.0, 1.0), 0);
        assert_eq!(
            pointer.leave_live(),
            vec![Command::Leave { x: 1.0, y: 1.0 }]
        );
    }

    #[test]
    fn scroll_is_forwarded_only_over_the_live_page() {
        let mut pointer = PointerState::default();
        let delta = ScrollDelta::Pixels(point(px(0.), px(-250.)));
        let outcome = pointer.scrolled(context(true, false, true), (3.0, 4.0), &delta, 0);
        assert!(outcome.consumed);
        assert_eq!(
            outcome.commands,
            vec![Command::Scroll {
                x: 3.0,
                y: 4.0,
                dx: -0.0,
                dy: 6.25,
                mods: 0
            }]
        );
        assert_eq!(
            pointer.scrolled(context(true, false, false), (3.0, 4.0), &delta, 0),
            PointerOutcome::default()
        );
        assert_eq!(
            pointer.scrolled(context(false, false, true), (3.0, 4.0), &delta, 0),
            PointerOutcome::default()
        );
    }

    #[test]
    fn coordinates_are_relative_to_the_element_origin() {
        // A press at (130, 90) over an element at (40, 80): x 90, y 10; at scale 2 the helper
        // multiplies by the page scale to get device coordinates (180, 20).
        let relative = relative_position(point(px(130.), px(90.)), point(px(40.), px(80.)));
        assert_eq!(relative, (90.0, 10.0));
        assert_eq!((relative.0 * 2.0, relative.1 * 2.0), (180.0, 20.0));
        // Not clamped.
        assert_eq!(
            relative_position(point(px(0.), px(0.)), point(px(40.), px(80.))),
            (-40.0, -80.0)
        );
    }

    #[test]
    fn text_input_conversions() {
        let text = "aé日";
        assert_eq!(selected_utf16_range(text, Some(3), Some(3)), (2..2, false));
        assert_eq!(text_in_utf16_range(text, 1..3), ("é日".to_owned(), 1..3));
        assert_eq!(text_in_utf16_range(text, 1..300), ("é日".to_owned(), 1..3));
        assert_eq!(text_in_utf16_range(text, 5..9), (String::new(), 3..3));
        // Out of range or mid-character offsets map to the whole length.
        assert_eq!(byte_offset_to_utf16(text, 100), 3);
        assert_eq!(byte_offset_to_utf16(text, 2), 3);
        assert_eq!(selected_utf16_range(text, Some(0), Some(100)), (0..3, true));
        assert_eq!(
            selected_utf16_range(text, Some(100), Some(0)),
            (0..3, false)
        );
        // Astral characters count two UTF-16 units.
        assert_eq!(utf16_len("a😀"), 3);
        assert_eq!(byte_offset_to_utf16("a😀b", 5), 3);
        // Caret [20, 40, 2, 18] at scale 2 with the element at (10, 100).
        let bounds = caret_bounds(point(px(10.), px(100.)), Some([20, 40, 2, 18]), 2.0);
        assert_eq!(bounds.origin, point(px(20.), px(120.)));
        assert_eq!(bounds.size, size(px(1.), px(9.)));
        let default = caret_bounds(point(px(10.), px(100.)), None, 1.0);
        assert_eq!(default.size, size(px(1.), px(18.)));
        assert_eq!(selected_range_of(&InputState::default()), (0..0, false));
    }
}
