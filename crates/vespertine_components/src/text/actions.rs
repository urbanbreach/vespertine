//! Rebindable editing actions and their default Linux key bindings (CMP-007 R8, CMP-008 R6/R7).
//!
//! The actions are shared by [`TextField`](super::TextField) (key context `"TextField"`) and
//! [`TextArea`](super::TextArea) (key context `"TextArea"`). Tab and Shift+Tab are never bound
//! so a plain editor lets them traverse focus; every key the editors do not handle propagates.

use vespertine::{App, Global, KeyBinding, actions};

actions!(
    vespertine_text,
    [
        /// Delete the selection or one grapheme before the caret.
        Backspace,
        /// Delete the selection or one grapheme after the caret.
        Delete,
        /// Move the caret left.
        Left,
        /// Move the caret right.
        Right,
        /// Extend the selection left.
        SelectLeft,
        /// Extend the selection right.
        SelectRight,
        /// Move to the start of the logical line.
        Home,
        /// Move to the end of the logical line.
        End,
        /// Select to the start of the logical line.
        SelectHome,
        /// Select to the end of the logical line.
        SelectEnd,
        /// Move to the previous word start.
        WordLeft,
        /// Move to the next word end.
        WordRight,
        /// Select to the previous word start.
        SelectWordLeft,
        /// Select to the next word end.
        SelectWordRight,
        /// Delete to the previous word start.
        DeleteWordLeft,
        /// Delete to the next word end.
        DeleteWordRight,
        /// Delete to the start of the logical line.
        DeleteToLineStart,
        /// Delete to the end of the logical line.
        DeleteToLineEnd,
        /// Select the whole value.
        SelectAll,
        /// Copy the selection.
        Copy,
        /// Cut the selection.
        Cut,
        /// Paste the clipboard.
        Paste,
        /// Undo.
        Undo,
        /// Redo.
        Redo,
        /// Enter: submit (single line) or newline/submit per the submit mode (multiline).
        Submit,
        /// Escape.
        Cancel,
        /// Move one visual row up (multiline).
        Up,
        /// Move one visual row down (multiline).
        Down,
        /// Extend the selection one visual row up (multiline).
        SelectUp,
        /// Extend the selection one visual row down (multiline).
        SelectDown,
        /// Move to the document start (multiline).
        DocumentStart,
        /// Move to the document end (multiline).
        DocumentEnd,
        /// Select to the document start (multiline).
        SelectDocumentStart,
        /// Select to the document end (multiline).
        SelectDocumentEnd,
        /// Shift+Enter: always insert a newline (multiline).
        Newline,
        /// Ctrl+Enter: submit in "modified Enter submits" mode (multiline).
        ModifiedSubmit,
        /// Tab in a Markdown editor: indent list items (multiline, see CMP-009 R13).
        Indent,
        /// Shift+Tab in a Markdown editor: outdent list items.
        Outdent,
    ]
);

/// The key context of the single-line field.
pub const FIELD_CONTEXT: &str = "TextField";
/// The key context of the multiline editor.
pub const AREA_CONTEXT: &str = "TextArea";

struct BindingsInstalled;
impl Global for BindingsInstalled {}

fn common_bindings(context: &'static str) -> Vec<KeyBinding> {
    let c = Some(context);
    let mut out = vec![
        KeyBinding::new("backspace", Backspace, c),
        KeyBinding::new("shift-backspace", Backspace, c),
        KeyBinding::new("delete", Delete, c),
        KeyBinding::new("left", Left, c),
        KeyBinding::new("right", Right, c),
        KeyBinding::new("shift-left", SelectLeft, c),
        KeyBinding::new("shift-right", SelectRight, c),
        KeyBinding::new("home", Home, c),
        KeyBinding::new("end", End, c),
        KeyBinding::new("shift-home", SelectHome, c),
        KeyBinding::new("shift-end", SelectEnd, c),
        KeyBinding::new("escape", Cancel, c),
        KeyBinding::new("enter", Submit, c),
    ];
    // Both the Control and the platform-modifier (Super) variants of the Ctrl chords.
    for modifier in ["ctrl", "super"] {
        out.extend([
            KeyBinding::new(&format!("{modifier}-left"), WordLeft, c),
            KeyBinding::new(&format!("{modifier}-right"), WordRight, c),
            KeyBinding::new(&format!("{modifier}-shift-left"), SelectWordLeft, c),
            KeyBinding::new(&format!("shift-{modifier}-left"), SelectWordLeft, c),
            KeyBinding::new(&format!("{modifier}-shift-right"), SelectWordRight, c),
            KeyBinding::new(&format!("shift-{modifier}-right"), SelectWordRight, c),
            KeyBinding::new(&format!("{modifier}-backspace"), DeleteWordLeft, c),
            KeyBinding::new(&format!("{modifier}-delete"), DeleteWordRight, c),
            KeyBinding::new(&format!("{modifier}-a"), SelectAll, c),
            KeyBinding::new(&format!("{modifier}-c"), Copy, c),
            KeyBinding::new(&format!("{modifier}-x"), Cut, c),
            KeyBinding::new(&format!("{modifier}-v"), Paste, c),
            KeyBinding::new(&format!("{modifier}-z"), Undo, c),
            KeyBinding::new(&format!("{modifier}-shift-z"), Redo, c),
            KeyBinding::new(&format!("shift-{modifier}-z"), Redo, c),
        ]);
    }
    out
}

fn area_bindings() -> Vec<KeyBinding> {
    let c = Some(AREA_CONTEXT);
    let mut out = common_bindings(AREA_CONTEXT);
    out.extend([
        KeyBinding::new("up", Up, c),
        KeyBinding::new("down", Down, c),
        KeyBinding::new("shift-up", SelectUp, c),
        KeyBinding::new("shift-down", SelectDown, c),
        KeyBinding::new("shift-enter", Newline, c),
    ]);
    for modifier in ["ctrl", "super"] {
        out.extend([
            KeyBinding::new(&format!("{modifier}-home"), DocumentStart, c),
            KeyBinding::new(&format!("{modifier}-end"), DocumentEnd, c),
            KeyBinding::new(&format!("{modifier}-shift-home"), SelectDocumentStart, c),
            KeyBinding::new(&format!("shift-{modifier}-home"), SelectDocumentStart, c),
            KeyBinding::new(&format!("{modifier}-shift-end"), SelectDocumentEnd, c),
            KeyBinding::new(&format!("shift-{modifier}-end"), SelectDocumentEnd, c),
            KeyBinding::new(&format!("{modifier}-enter"), ModifiedSubmit, c),
        ]);
    }
    out
}

/// Installs the default Linux key bindings of the single-line field and the multiline editor
/// once; later calls do nothing. The editors call this from their constructors, so applications
/// only need it to install the bindings before any editor exists.
pub fn bind_default_keys(cx: &mut App) {
    if cx.has_global::<BindingsInstalled>() {
        return;
    }
    cx.set_global(BindingsInstalled);
    cx.bind_keys(common_bindings(FIELD_CONTEXT));
    cx.bind_keys(area_bindings());
}
