//! Keyboard access to context menus (INP-007 R4).
//!
//! Menu and context-menu actions must stay available without a pointer. The
//! keyboard context-menu gesture (Shift+F10) opens the menu anchored at the
//! bottom-left corner of the focused element and moves focus into it. Inside
//! the menu Down and Tab move focus to the next item, Up and Shift+Tab to the
//! previous one, wrapping at both ends; Escape closes the menu and returns
//! focus to the element that opened it.
//!
//! [`KeyboardMenu`] holds the focus behavior of one menu; the view that renders
//! the menu owns it, gives each item element the matching
//! [`KeyboardMenu::item_focus`] handle, and forwards the menu's key-down events
//! to [`KeyboardMenu::handle_key_down`]. [`BoundsTracker`] reports the bounds
//! of the element that will open the menu so the anchor can be computed.

use std::{cell::Cell, rc::Rc};

use crate::{
    App, Bounds, FocusHandle, IntoElement, KeyDownEvent, Keystroke, Pixels, Point, Styled as _,
    WeakFocusHandle, Window, canvas,
};

/// Whether `keystroke` is the keyboard context-menu chord (Shift+F10).
pub fn is_context_menu_keystroke(keystroke: &Keystroke) -> bool {
    let modifiers = keystroke.modifiers;
    keystroke.key == "f10"
        && modifiers.shift
        && !modifiers.control
        && !modifiers.alt
        && !modifiers.platform
}

/// Records the bounds of the element it is embedded in.
///
/// Add [`Self::element`] as a child of the element whose bounds are needed
/// (typically the menu's trigger or the editor that owns the context menu);
/// after the element has painted, [`Self::bounds`] reports its bounds.
#[derive(Clone, Default)]
pub struct BoundsTracker(Rc<Cell<Option<Bounds<Pixels>>>>);

impl BoundsTracker {
    /// A tracker that has not seen any bounds yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// The bounds of the parent element as of its latest paint.
    pub fn bounds(&self) -> Option<Bounds<Pixels>> {
        self.0.get()
    }

    /// The bottom-left corner of the tracked bounds, where a keyboard-opened
    /// menu anchors.
    pub fn bottom_left(&self) -> Option<Point<Pixels>> {
        self.bounds().map(|bounds| bounds.bottom_left())
    }

    /// A zero-cost child element that fills its parent and records the
    /// parent's bounds while painting.
    pub fn element(&self) -> impl IntoElement + use<> {
        let cell = self.0.clone();
        canvas(move |bounds, _, _| cell.set(Some(bounds)), |_, _, _, _| {})
            .absolute()
            .size_full()
    }
}

/// What [`KeyboardMenu::handle_key_down`] did with a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuKey {
    /// The key is not a menu navigation key.
    Ignored,
    /// Focus moved to another item.
    Moved,
    /// The menu closed and focus returns to the element that opened it.
    Closed,
}

/// Focus behavior of one keyboard-operable menu.
pub struct KeyboardMenu {
    items: Vec<FocusHandle>,
    fallback: WeakFocusHandle,
    origin: Option<WeakFocusHandle>,
    anchor: Option<Point<Pixels>>,
    open: bool,
}

impl KeyboardMenu {
    /// A menu with `item_count` items. `fallback` is the focus handle of a
    /// mounted root (the pane or window root) that receives focus when the
    /// element that opened the menu is gone by the time the menu closes.
    pub fn new(item_count: usize, fallback: &FocusHandle, cx: &mut App) -> Self {
        Self {
            items: (0..item_count)
                .map(|_| cx.focus_handle().tab_stop(true))
                .collect(),
            fallback: fallback.downgrade(),
            origin: None,
            anchor: None,
            open: false,
        }
    }

    /// The focus handle of item `ix`; the item's element tracks it.
    pub fn item_focus(&self, ix: usize) -> &FocusHandle {
        &self.items[ix]
    }

    /// Whether the menu is open.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The window position the open menu is anchored at.
    pub fn anchor(&self) -> Option<Point<Pixels>> {
        self.anchor
    }

    /// Opens the menu from the currently focused element, anchored at
    /// `anchor` (the bottom-left corner of that element, see
    /// [`BoundsTracker::bottom_left`]), and moves focus to the first item. The
    /// focused element is remembered as the one Escape returns focus to.
    pub fn open(&mut self, anchor: Point<Pixels>, window: &mut Window, cx: &mut App) {
        self.origin = window.focused(cx).map(|handle| handle.downgrade());
        self.anchor = Some(anchor);
        self.open = true;
        if let Some(first) = self.items.first() {
            window.focus(first, cx);
        }
    }

    /// Closes the menu and requests that focus return to the element that
    /// opened it (or the fallback root when that element is gone).
    pub fn close(&mut self, window: &mut Window) {
        if !self.open {
            return;
        }
        self.open = false;
        self.anchor = None;
        let origin = self.origin.take().unwrap_or_else(|| self.fallback.clone());
        window.restore_focus(&origin, &self.fallback);
    }

    /// Handles a key-down delivered to the open menu. Down and Tab move to
    /// the next item, Up and Shift+Tab to the previous one (cyclically);
    /// Escape closes the menu. Returns what was done; the caller stops
    /// propagation for anything but [`MenuKey::Ignored`].
    pub fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut App,
    ) -> MenuKey {
        if !self.open {
            return MenuKey::Ignored;
        }
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        let plain = !modifiers.control && !modifiers.alt && !modifiers.platform;
        let forward = plain && !modifiers.shift && matches!(keystroke.key.as_str(), "down" | "tab");
        let backward = plain
            && ((!modifiers.shift && keystroke.key == "up")
                || (modifiers.shift && keystroke.key == "tab"));
        if forward {
            let items = &self.items;
            window.focus_next_among(cx, |id, _| items.iter().any(|item| item.id() == id));
            MenuKey::Moved
        } else if backward {
            let items = &self.items;
            window.focus_prev_among(cx, |id, _| items.iter().any(|item| item.id() == id));
            MenuKey::Moved
        } else if keystroke.key == "escape" && !modifiers.modified() {
            self.close(window);
            MenuKey::Closed
        } else {
            MenuKey::Ignored
        }
    }
}
