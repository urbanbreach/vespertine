//! Rebindable picker actions and their default Linux key bindings (CMP-006 R12-R14, R16).
//!
//! The bindings are data in the key context [`PICKER_CONTEXT`]; applications rebind them through
//! the key map like any other action. Escape, Enter and the text-editing keys are not here:
//! Escape belongs to the dismissal-layer stack (CMP-002), Enter to the query field's submit
//! action (the picker claims it to apply the once-per-press latch) and text keys reach the field.

use vespertine::{App, Global, KeyBinding, actions};

/// Key context of an open picker.
pub const PICKER_CONTEXT: &str = "Picker";
/// Key context added while a nested choice submenu is open.
pub const CHOICES_CONTEXT: &str = "PickerChoices";

actions!(
    vespertine_picker,
    [
        /// Highlight the previous row (Up, Ctrl-P).
        MoveUp,
        /// Highlight the next row (Down, Ctrl-N).
        MoveDown,
        /// Next scope tab (Tab).
        NextTab,
        /// Previous scope tab (Shift-Tab).
        PreviousTab,
        /// The secondary accept (Ctrl-Enter).
        ConfirmSecondary,
        /// Toggle the favorite state of the highlighted row (Ctrl-Shift-F).
        ToggleFavorite,
        /// Step back out of an open choice submenu (Left).
        BackOut,
        /// Retry the first failed section, else the failed page request (Ctrl-R).
        Retry,
        /// The first numbered accelerator (Ctrl-1): the first tab or visible row.
        Accelerator1,
        /// The second numbered accelerator (Ctrl-2).
        Accelerator2,
        /// The third numbered accelerator (Ctrl-3).
        Accelerator3,
        /// The fourth numbered accelerator (Ctrl-4).
        Accelerator4,
        /// The fifth numbered accelerator (Ctrl-5).
        Accelerator5,
        /// The sixth numbered accelerator (Ctrl-6).
        Accelerator6,
        /// The seventh numbered accelerator (Ctrl-7).
        Accelerator7,
        /// The eighth numbered accelerator (Ctrl-8).
        Accelerator8,
        /// The ninth numbered accelerator (Ctrl-9).
        Accelerator9,
    ]
);

struct BindingsInstalled;
impl Global for BindingsInstalled {}

/// The numbered accelerator bindings.
fn accelerator_bindings() -> Vec<KeyBinding> {
    let context = Some(PICKER_CONTEXT);
    vec![
        KeyBinding::new("ctrl-1", Accelerator1, context),
        KeyBinding::new("ctrl-2", Accelerator2, context),
        KeyBinding::new("ctrl-3", Accelerator3, context),
        KeyBinding::new("ctrl-4", Accelerator4, context),
        KeyBinding::new("ctrl-5", Accelerator5, context),
        KeyBinding::new("ctrl-6", Accelerator6, context),
        KeyBinding::new("ctrl-7", Accelerator7, context),
        KeyBinding::new("ctrl-8", Accelerator8, context),
        KeyBinding::new("ctrl-9", Accelerator9, context),
    ]
}

/// Installs the default bindings once; later calls do nothing. [`Picker`](super::Picker) calls it
/// from its constructor.
pub fn bind_default_keys(cx: &mut App) {
    if cx.has_global::<BindingsInstalled>() {
        return;
    }
    cx.set_global(BindingsInstalled);
    let context = Some(PICKER_CONTEXT);
    let mut bindings = vec![
        KeyBinding::new("up", MoveUp, context),
        KeyBinding::new("ctrl-p", MoveUp, context),
        KeyBinding::new("down", MoveDown, context),
        KeyBinding::new("ctrl-n", MoveDown, context),
        KeyBinding::new("tab", NextTab, context),
        KeyBinding::new("shift-tab", PreviousTab, context),
        KeyBinding::new("ctrl-enter", ConfirmSecondary, context),
        KeyBinding::new("ctrl-shift-f", ToggleFavorite, context),
        KeyBinding::new("ctrl-r", Retry, context),
        KeyBinding::new("left", BackOut, Some(CHOICES_CONTEXT)),
    ];
    bindings.extend(accelerator_bindings());
    cx.bind_keys(bindings);
}
