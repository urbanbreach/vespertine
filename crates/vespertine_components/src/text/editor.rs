//! The behavior the single-line field and the multiline editor share: editing actions over the
//! [`EditCore`], clipboard flows and the native text-input replacement semantics. The two
//! entities implement [`EditorHost`] (the few places they differ) and inherit everything else.

use super::{
    common::{ClipboardContent, EditMenuState, PastePreference},
    core::EditCore,
};
use std::{ops::Range, path::PathBuf};
use vespertine::{
    App, ClipboardItem, Context, EntityInputHandler, FocusHandle, Image, MotionClock,
    UTF16Selection, Window,
};

/// The current motion-clock time in milliseconds: the time base of undo coalescing and blink.
pub fn now_ms(cx: &App) -> f64 {
    MotionClock::now_ms(cx)
}

/// What an editor entity provides to the shared behavior.
pub trait EditorHost: EntityInputHandler + Sized {
    /// The editing core.
    fn core(&self) -> &EditCore;
    /// The editing core, mutably.
    fn core_mut(&mut self) -> &mut EditCore;
    /// The focus handle.
    fn focus_handle_ref(&self) -> &FocusHandle;
    /// Whether the editor is disabled (CMP-007 R2).
    fn is_disabled(&self) -> bool;
    /// How a paste chooses between text and media.
    fn paste_preference(&self) -> PastePreference;
    /// Whether the owner accepts pasted media (paths, images) for the paste enablement.
    fn accepts_media(&self) -> bool {
        self.paste_preference() == PastePreference::MediaFirst
    }
    /// The text may have changed: emit the change, re-arm caret-follow, restart the blink cycle
    /// and repaint.
    fn finish_edit(&mut self, changed: bool, window: &mut Window, cx: &mut Context<Self>);
    /// The selection moved without changing the text.
    fn finish_move(&mut self, window: &mut Window, cx: &mut Context<Self>);
    /// Enter (single line) or the submit chord.
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>);
    /// Escape.
    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>);
    /// Reports pasted paths and images to the owner (CMP-007 R16).
    fn paste_media(&mut self, paths: Vec<PathBuf>, images: Vec<Image>, cx: &mut Context<Self>);

    /// The clipboard item a copy or cut of the selection writes; `None` for an empty selection.
    fn copy_item(&mut self, _cx: &App) -> Option<ClipboardItem> {
        let text = self.core().selected_text();
        (!text.is_empty()).then(|| ClipboardItem::new_string(text.to_owned()))
    }

    /// Pastes classified clipboard content into the editor; returns whether the text changed.
    fn paste_content(
        &mut self,
        content: ClipboardContent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let media_first = self.paste_preference() == PastePreference::MediaFirst;
        if (media_first || content.text.is_none()) && content.has_media() {
            self.paste_media(content.paths, content.images, cx);
            return false;
        }
        let Some(text) = content.text else {
            return false;
        };
        let now = now_ms(cx);
        let changed = self.core_mut().paste(&text, now);
        self.finish_edit(changed, window, cx);
        changed
    }

    // ---- state queries ---------------------------------------------------------------------

    /// The enablement of the context editing actions (CMP-007 R24).
    fn menu_state(&self, cx: &App) -> EditMenuState {
        let core = self.core();
        let editable = !core.is_read_only() && !self.is_disabled();
        let selection = !core.selection().is_empty();
        let clipboard = ClipboardContent::read(cx);
        EditMenuState {
            cut: selection && editable,
            copy: selection,
            paste: editable
                && (clipboard.text.is_some() || (self.accepts_media() && clipboard.has_media())),
            select_all: !core.text().is_empty(),
        }
    }

    // ---- shared action behavior ------------------------------------------------------------

    /// Runs a mutating core operation as an editing action.
    fn edit_with(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        operation: impl FnOnce(&mut EditCore, f64) -> bool,
    ) {
        if self.is_disabled() {
            return;
        }
        let now = now_ms(cx);
        let changed = operation(self.core_mut(), now);
        self.finish_edit(changed, window, cx);
    }

    /// Runs a selection-changing core operation as a movement action.
    fn move_with(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        operation: impl FnOnce(&mut EditCore),
    ) {
        if self.is_disabled() {
            return;
        }
        operation(self.core_mut());
        self.finish_move(window, cx);
    }

    /// Copy (R16): writes the selection; a no-op for an empty selection.
    fn act_copy(&mut self, cx: &mut Context<Self>) {
        if self.is_disabled() {
            return;
        }
        if let Some(item) = self.copy_item(cx) {
            cx.write_to_clipboard(item);
        }
    }

    /// Cut (R16): copy then delete; rejected when read-only.
    fn act_cut(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_disabled() || self.core().is_read_only() || self.core().selection().is_empty() {
            return;
        }
        if let Some(item) = self.copy_item(cx) {
            cx.write_to_clipboard(item);
        }
        self.edit_with(window, cx, |core, now| core.cut_selection(now));
    }

    /// Paste (R16): rejected when read-only; an empty clipboard changes nothing.
    fn act_paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_disabled() || self.core().is_read_only() {
            return;
        }
        let content = ClipboardContent::read(cx);
        self.paste_content(content, window, cx);
    }

    // ---- native text input (R17, R18) ------------------------------------------------------

    /// `text_for_range`.
    fn ime_text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
    ) -> Option<String> {
        let (text, range) = self.core().text_for_range(range);
        *adjusted = Some(range);
        Some(text)
    }

    /// `selected_text_range`.
    fn ime_selected(&self) -> Option<UTF16Selection> {
        let (range, reversed) = self.core().selected_range_utf16();
        Some(UTF16Selection { range, reversed })
    }

    /// `replace_text_in_range`.
    fn ime_replace_text(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_disabled() || self.core().is_read_only() {
            return;
        }
        let now = now_ms(cx);
        let changed = self.core_mut().replace_text(range, text, now);
        self.finish_edit(changed, window, cx);
    }

    /// `replace_and_mark_text_in_range`.
    fn ime_replace_and_mark(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_disabled() || self.core().is_read_only() {
            return;
        }
        let changed = self.core_mut().replace_and_mark(range, text, selected);
        self.finish_edit(changed, window, cx);
    }

    /// `unmark_text`.
    fn ime_unmark(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.core_mut().unmark();
        self.finish_edit(false, window, cx);
    }
}

/// Attaches the editing actions of both editors to a key-context element. `$ty` is the entity
/// type implementing [`EditorHost`].
#[macro_export]
#[doc(hidden)]
macro_rules! editor_action_listeners {
    ($element:expr, $cx:expr, $ty:ty) => {{
        use $crate::text::actions::*;
        use $crate::text::editor::EditorHost as _;
        $element
            .on_action($cx.listener(|this: &mut $ty, _: &Backspace, window, cx| {
                this.edit_with(window, cx, |core, now| core.delete_backward(now))
            }))
            .on_action($cx.listener(|this: &mut $ty, _: &Delete, window, cx| {
                this.edit_with(window, cx, |core, now| core.delete_forward(now))
            }))
            .on_action(
                $cx.listener(|this: &mut $ty, _: &DeleteWordLeft, window, cx| {
                    this.edit_with(window, cx, |core, now| core.delete_word_backward(now))
                }),
            )
            .on_action(
                $cx.listener(|this: &mut $ty, _: &DeleteWordRight, window, cx| {
                    this.edit_with(window, cx, |core, now| core.delete_word_forward(now))
                }),
            )
            .on_action(
                $cx.listener(|this: &mut $ty, _: &DeleteToLineStart, window, cx| {
                    this.edit_with(window, cx, |core, now| core.delete_to_line_start(now))
                }),
            )
            .on_action(
                $cx.listener(|this: &mut $ty, _: &DeleteToLineEnd, window, cx| {
                    this.edit_with(window, cx, |core, now| core.delete_to_line_end(now))
                }),
            )
            .on_action($cx.listener(|this: &mut $ty, _: &Left, window, cx| {
                this.move_with(window, cx, |core| core.move_left(false))
            }))
            .on_action($cx.listener(|this: &mut $ty, _: &Right, window, cx| {
                this.move_with(window, cx, |core| core.move_right(false))
            }))
            .on_action($cx.listener(|this: &mut $ty, _: &SelectLeft, window, cx| {
                this.move_with(window, cx, |core| core.move_left(true))
            }))
            .on_action($cx.listener(|this: &mut $ty, _: &SelectRight, window, cx| {
                this.move_with(window, cx, |core| core.move_right(true))
            }))
            .on_action($cx.listener(|this: &mut $ty, _: &Home, window, cx| {
                this.move_with(window, cx, |core| core.move_line_start(false))
            }))
            .on_action($cx.listener(|this: &mut $ty, _: &End, window, cx| {
                this.move_with(window, cx, |core| core.move_line_end(false))
            }))
            .on_action($cx.listener(|this: &mut $ty, _: &SelectHome, window, cx| {
                this.move_with(window, cx, |core| core.move_line_start(true))
            }))
            .on_action($cx.listener(|this: &mut $ty, _: &SelectEnd, window, cx| {
                this.move_with(window, cx, |core| core.move_line_end(true))
            }))
            .on_action($cx.listener(|this: &mut $ty, _: &WordLeft, window, cx| {
                this.move_with(window, cx, |core| core.move_word_left(false))
            }))
            .on_action($cx.listener(|this: &mut $ty, _: &WordRight, window, cx| {
                this.move_with(window, cx, |core| core.move_word_right(false))
            }))
            .on_action(
                $cx.listener(|this: &mut $ty, _: &SelectWordLeft, window, cx| {
                    this.move_with(window, cx, |core| core.move_word_left(true))
                }),
            )
            .on_action(
                $cx.listener(|this: &mut $ty, _: &SelectWordRight, window, cx| {
                    this.move_with(window, cx, |core| core.move_word_right(true))
                }),
            )
            .on_action($cx.listener(|this: &mut $ty, _: &SelectAll, window, cx| {
                this.move_with(window, cx, |core| core.select_all())
            }))
            .on_action($cx.listener(|this: &mut $ty, _: &Copy, _window, cx| this.act_copy(cx)))
            .on_action($cx.listener(|this: &mut $ty, _: &Cut, window, cx| this.act_cut(window, cx)))
            .on_action(
                $cx.listener(|this: &mut $ty, _: &Paste, window, cx| this.act_paste(window, cx)),
            )
            .on_action($cx.listener(|this: &mut $ty, _: &Undo, window, cx| {
                this.edit_with(window, cx, |core, _| core.undo())
            }))
            .on_action($cx.listener(|this: &mut $ty, _: &Redo, window, cx| {
                this.edit_with(window, cx, |core, _| core.redo())
            }))
            .on_action(
                $cx.listener(|this: &mut $ty, _: &Submit, window, cx| this.submit(window, cx)),
            )
            .on_action(
                $cx.listener(|this: &mut $ty, _: &Cancel, window, cx| this.cancel(window, cx)),
            )
    }};
}
