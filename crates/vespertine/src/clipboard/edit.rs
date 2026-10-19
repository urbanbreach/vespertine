//! The standard cut, copy, paste and select-all actions and their availability.
//!
//! Text editors expose these four actions ([`standard::Cut`], [`standard::Copy`], [`standard::Paste`], [`standard::SelectAll`]) in the
//! `clipboard` action namespace. This module holds the editor-independent contract:
//!
//! * copy and cut are available only with a non-empty selection;
//! * cut and paste are available only when the editor is editable (a read-only editor never
//!   mutates its content or the clipboard on cut, though copy still works);
//! * paste is available when the clipboard holds text or a representation the editor's
//!   [`PastePolicy`] accepts;
//! * select-all is available when the content is non-empty;
//! * triggering an unavailable action is a silent no-op that leaves clipboard and content
//!   unchanged, and pasting an empty clipboard changes nothing.
//!
//! An editor implements [`EditTarget`]; [`perform`] and [`availability`] then implement the
//! contract for it, and [`perform_on_clipboard`] wires it to the application's clipboard.

use serde::{Serialize, de::DeserializeOwned};

use crate::{
    App, ClipboardItem,
    clipboard::policy::{PasteContent, PastePolicy},
};

/// The standard edit actions, in the `clipboard` action namespace.
pub mod standard {
    actions!(
        clipboard,
        [
            /// Cut the selection to the clipboard.
            Cut,
            /// Copy the selection to the clipboard.
            Copy,
            /// Paste the clipboard at the selection.
            Paste,
            /// Select all content.
            SelectAll,
        ]
    );
}

/// One of the four standard edit actions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EditAction {
    /// [`standard::Cut`]
    Cut,
    /// [`standard::Copy`]
    Copy,
    /// [`standard::Paste`]
    Paste,
    /// [`standard::SelectAll`]
    SelectAll,
}

impl From<standard::Cut> for EditAction {
    fn from(_: standard::Cut) -> Self {
        Self::Cut
    }
}

impl From<standard::Copy> for EditAction {
    fn from(_: standard::Copy) -> Self {
        Self::Copy
    }
}

impl From<standard::Paste> for EditAction {
    fn from(_: standard::Paste) -> Self {
        Self::Paste
    }
}

impl From<standard::SelectAll> for EditAction {
    fn from(_: standard::SelectAll) -> Self {
        Self::SelectAll
    }
}

/// The editor facts availability depends on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditState {
    /// Whether the selection is empty (a caret only).
    pub selection_empty: bool,
    /// Whether the editor accepts content changes.
    pub editable: bool,
    /// Whether the content is empty.
    pub content_empty: bool,
}

/// Which of the standard actions are currently available.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditAvailability {
    /// Whether [`standard::Cut`] is available.
    pub cut: bool,
    /// Whether [`standard::Copy`] is available.
    pub copy: bool,
    /// Whether [`standard::Paste`] is available.
    pub paste: bool,
    /// Whether [`standard::SelectAll`] is available.
    pub select_all: bool,
}

impl EditAvailability {
    /// Computes availability from editor state and whether the clipboard holds something
    /// the editor would paste.
    pub fn new(state: EditState, clipboard_pasteable: bool) -> Self {
        Self {
            cut: !state.selection_empty && state.editable,
            copy: !state.selection_empty,
            paste: state.editable && clipboard_pasteable,
            select_all: !state.content_empty,
        }
    }

    /// Whether `action` is available.
    pub fn allows(&self, action: EditAction) -> bool {
        match action {
            EditAction::Cut => self.cut,
            EditAction::Copy => self.copy,
            EditAction::Paste => self.paste,
            EditAction::SelectAll => self.select_all,
        }
    }
}

/// An editor that takes part in the standard edit actions.
pub trait EditTarget {
    /// The type of projection references this editor pastes, see
    /// [`scope`](crate::clipboard::scope). Editors without projections use `()`.
    type Reference: Serialize + DeserializeOwned;

    /// The facts availability depends on.
    fn edit_state(&self) -> EditState;

    /// The paste policy of this editor; text first unless the owner prefers media.
    fn paste_policy(&self) -> &PastePolicy;

    /// The editor's scope compatibility predicate for structured paste.
    fn accepts_scope(&self, scope: &str) -> bool;

    /// The selection as a clipboard item: expanded plain text, plus scoped metadata when the
    /// selection contains projections (see
    /// [`ScopedContent::into_clipboard_item`](crate::clipboard::scope::ScopedContent::into_clipboard_item)).
    /// `None` for an empty selection.
    fn selected_item(&self) -> Option<ClipboardItem>;

    /// Deletes the selection. Called by cut after [`selected_item`](Self::selected_item).
    fn delete_selection(&mut self);

    /// Inserts what a paste resolved to at the selection.
    fn insert(&mut self, paste: PasteContent<Self::Reference>);

    /// Selects all content.
    fn select_all(&mut self);
}

/// What performing an action did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditOutcome {
    /// The editor wants this item written to the clipboard (cut or copy).
    WriteToClipboard(ClipboardItem),
    /// The editor's content or selection changed (paste or select-all).
    Changed,
    /// The action was not available; nothing happened.
    Unavailable,
}

/// Availability of every standard action for `editor` given the current clipboard content.
pub fn availability<E: EditTarget>(
    editor: &E,
    clipboard: Option<&ClipboardItem>,
) -> EditAvailability {
    let pasteable = clipboard.is_some_and(|item| {
        editor
            .paste_policy()
            .accepts(item, &|scope| editor.accepts_scope(scope))
    });
    EditAvailability::new(editor.edit_state(), pasteable)
}

/// Performs `action` on `editor`. `clipboard` is the current clipboard content (needed by
/// paste only).
///
/// An unavailable action returns [`EditOutcome::Unavailable`] and changes nothing.
pub fn perform<E: EditTarget>(
    editor: &mut E,
    action: EditAction,
    clipboard: Option<&ClipboardItem>,
) -> EditOutcome {
    if !availability(editor, clipboard).allows(action) {
        return EditOutcome::Unavailable;
    }
    match action {
        EditAction::Copy => match editor.selected_item() {
            Some(item) => EditOutcome::WriteToClipboard(item),
            None => EditOutcome::Unavailable,
        },
        EditAction::Cut => match editor.selected_item() {
            Some(item) => {
                editor.delete_selection();
                EditOutcome::WriteToClipboard(item)
            }
            None => EditOutcome::Unavailable,
        },
        EditAction::Paste => {
            let Some(item) = clipboard else {
                return EditOutcome::Unavailable;
            };
            let paste = editor
                .paste_policy()
                .resolve::<E::Reference>(item, &|scope| editor.accepts_scope(scope));
            match paste {
                Some(paste) => {
                    editor.insert(paste);
                    EditOutcome::Changed
                }
                None => EditOutcome::Unavailable,
            }
        }
        EditAction::SelectAll => {
            editor.select_all();
            EditOutcome::Changed
        }
    }
}

/// Performs `action` on `editor` against the application's system clipboard: paste reads it
/// and cut or copy write it.
///
/// Reading the system clipboard can wait on another process; call from the UI thread only for
/// user-initiated paste, or use [`App::read_from_clipboard_async`] and [`perform`].
pub fn perform_on_clipboard<E: EditTarget>(
    editor: &mut E,
    action: EditAction,
    cx: &mut App,
) -> EditOutcome {
    let clipboard = (action == EditAction::Paste)
        .then(|| cx.read_from_clipboard())
        .flatten();
    let outcome = perform(editor, action, clipboard.as_ref());
    if let EditOutcome::WriteToClipboard(item) = &outcome {
        cx.write_to_clipboard(item.clone());
    }
    outcome
}
