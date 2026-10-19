//! In-window menus built from `App::get_menus()`, shared by the `menus_in_window` example and the
//! APP-003 tests.
//!
//! On Linux the toolkit creates no native menu bar: [`App::set_menus`] only stores the
//! descriptors. This module renders a menu bar and popup from those retrieved descriptors, shows
//! the key equivalents derived from the current key bindings, derives each item's enabled state
//! from the descriptor's disabled flag plus action availability on the focused surface, and
//! dispatches the item's action through the ordinary action system.

use vespertine::{
    Action, AnyElement, App, Context, Entity, FocusHandle, Focusable, Global, KeyBinding, Menu,
    MenuItem, MouseButton, OsAction, OwnedMenu, OwnedMenuItem, SharedString, Window, actions,
    deferred, div, prelude::*, px, rgb,
};

actions!(
    menus_in_window,
    [
        NewDocument,
        Settings,
        Quit,
        Undo,
        Redo,
        Cut,
        Copy,
        Paste,
        SelectAll,
        Minimize,
        Zoom,
        CloseWindow,
    ]
);

/// Every activation, from whichever path, ends up as a line in this log.
#[derive(Default)]
pub struct ActivationLog {
    pub entries: Vec<String>,
}

impl Global for ActivationLog {}

pub fn log(cx: &mut App, entry: impl Into<String>) {
    cx.global_mut::<ActivationLog>().entries.push(entry.into());
}

/// Installs the log, the global quit handler, the fixed shortcuts and the menus.
pub fn install(cx: &mut App) {
    cx.set_global(ActivationLog::default());
    cx.on_action(|_: &Quit, cx| {
        log(cx, "quit");
        cx.quit();
    });
    bind_shortcuts(cx);
    set_menus(cx);
}

/// The shortcuts that back the menu equivalents. Re-run after `clear_key_bindings`.
pub fn bind_shortcuts(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-n", NewDocument, None),
        KeyBinding::new("ctrl-q", Quit, None),
        KeyBinding::new("ctrl-w", CloseWindow, None),
        KeyBinding::new("ctrl-alt-m", Minimize, None),
        KeyBinding::new("ctrl-alt-z", Zoom, None),
        KeyBinding::new("ctrl-z", Undo, Some("Editor")),
        KeyBinding::new("ctrl-shift-z", Redo, Some("Editor")),
        KeyBinding::new("ctrl-x", Cut, Some("Editor")),
        KeyBinding::new("ctrl-c", Copy, Some("Editor || Viewer")),
        KeyBinding::new("ctrl-v", Paste, Some("Editor")),
        KeyBinding::new("ctrl-a", SelectAll, Some("Editor || Viewer")),
    ]);
}

/// Application, Edit and Window groups with a separator, a disabled item and role hints.
pub fn menus() -> Vec<Menu> {
    vec![
        Menu::new("App").items([
            MenuItem::action("New Document", NewDocument),
            MenuItem::separator(),
            MenuItem::action("Settings", Settings).disabled(true),
            MenuItem::separator(),
            MenuItem::action("Quit", Quit),
        ]),
        Menu::new("Edit").items([
            MenuItem::os_action("Undo", Undo, OsAction::Undo),
            MenuItem::os_action("Redo", Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", Cut, OsAction::Cut),
            MenuItem::os_action("Copy", Copy, OsAction::Copy),
            MenuItem::os_action("Paste", Paste, OsAction::Paste),
            MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
        ]),
        Menu::new("Window").items([
            MenuItem::action("Minimize", Minimize),
            MenuItem::action("Zoom", Zoom),
            MenuItem::separator(),
            MenuItem::action("Close Window", CloseWindow),
        ]),
    ]
}

/// Replaces the installed menus. Calling this again is how displayed equivalents are refreshed.
pub fn set_menus(cx: &mut App) {
    cx.set_menus(menus());
}

/// What an in-window menu shows for one entry, derived from the retrieved descriptors.
pub enum MenuEntry {
    Separator,
    Item(ItemModel),
    Submenu {
        name: SharedString,
        disabled: bool,
        entries: Vec<MenuEntry>,
    },
}

pub struct ItemModel {
    pub label: SharedString,
    pub equivalent: Option<String>,
    pub enabled: bool,
    pub checked: bool,
    pub action: Box<dyn Action>,
}

/// An action is available when a handler for it exists on the focused element's ancestor chain
/// or as a global handler.
pub fn is_available(action: &dyn Action, window: &Window, cx: &App) -> bool {
    window.is_action_available(action, cx)
        || window
            .available_actions(cx)
            .iter()
            .any(|available| available.as_any().type_id() == action.as_any().type_id())
}

/// The highest-precedence binding for the action in the focused context, as display text.
///
/// Without a focused element only bindings without a context predicate apply.
pub fn displayed_equivalent(action: &dyn Action, window: &Window, cx: &App) -> Option<String> {
    let binding = match window.focused(cx) {
        Some(focus_handle) => {
            window.highest_precedence_binding_for_action_in(action, &focus_handle)
        }
        None => cx
            .key_bindings()
            .borrow()
            .bindings_for_action(action)
            .rev()
            .find(|binding| binding.predicate().is_none())
            .cloned(),
    }?;
    Some(
        binding
            .keystrokes()
            .iter()
            .map(|keystroke| keystroke.to_string())
            .collect::<Vec<_>>()
            .join(" "),
    )
}

pub fn menu_entries(menu: &OwnedMenu, window: &Window, cx: &App) -> Vec<MenuEntry> {
    menu.items
        .iter()
        .filter_map(|item| match item {
            OwnedMenuItem::Action {
                action,
                name,
                disabled,
                checked,
                ..
            } => {
                let enabled = !disabled && !menu.disabled && is_available(&**action, window, cx);
                Some(MenuEntry::Item(ItemModel {
                    action: action.boxed_clone(),
                    checked: *checked,
                    enabled,
                    equivalent: displayed_equivalent(&**action, window, cx),
                    label: name.clone().into(),
                }))
            }
            OwnedMenuItem::Submenu(submenu) => Some(MenuEntry::Submenu {
                name: submenu.name.clone(),
                disabled: submenu.disabled,
                entries: menu_entries(submenu, window, cx),
            }),
            // Platform-provided lists have no effect on Linux.
            OwnedMenuItem::SystemMenu(_) => None,
            OwnedMenuItem::Separator => Some(MenuEntry::Separator),
        })
        .collect()
}

/// Two focusable surfaces: an editor that handles every edit action, and a read-only viewer
/// that only handles copy and select all.
pub struct Surface {
    pub name: &'static str,
    pub editable: bool,
    focus_handle: FocusHandle,
}

impl Surface {
    pub fn new(name: &'static str, editable: bool, cx: &mut Context<Self>) -> Self {
        Self {
            name,
            editable,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl Focusable for Surface {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Surface {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let name = self.name;
        let focused = self.focus_handle.is_focused(window);
        let mut surface = div()
            .id(name)
            .key_context(if self.editable { "Editor" } else { "Viewer" })
            .track_focus(&self.focus_handle)
            .debug_selector(|| format!("surface-{name}"))
            .on_action(cx.listener(move |_, _: &Copy, _, cx| log(cx, format!("{name}: copy"))))
            .on_action(
                cx.listener(move |_, _: &SelectAll, _, cx| log(cx, format!("{name}: select all"))),
            )
            .w(px(180.))
            .h(px(100.))
            .p_2()
            .bg(if focused {
                rgb(0x2d4a6b)
            } else {
                rgb(0x333333)
            })
            .text_color(rgb(0xffffff))
            .child(SharedString::from(format!(
                "{name}{}",
                if self.editable { "" } else { " (read-only)" }
            )));
        if self.editable {
            surface = surface
                .on_action(cx.listener(move |_, _: &Cut, _, cx| log(cx, format!("{name}: cut"))))
                .on_action(
                    cx.listener(move |_, _: &Paste, _, cx| log(cx, format!("{name}: paste"))),
                )
                .on_action(cx.listener(move |_, _: &Undo, _, cx| log(cx, format!("{name}: undo"))))
                .on_action(cx.listener(move |_, _: &Redo, _, cx| log(cx, format!("{name}: redo"))));
        }
        surface
    }
}

/// Root view: a menu bar with a popup rendered from `cx.get_menus()`, above two surfaces.
pub struct MenuDemo {
    pub editor: Entity<Surface>,
    pub other_editor: Entity<Surface>,
    pub viewer: Entity<Surface>,
    pub open_menu: Option<usize>,
}

impl MenuDemo {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| Surface::new("editor", true, cx));
        let other_editor = cx.new(|cx| Surface::new("other editor", true, cx));
        let viewer = cx.new(|cx| Surface::new("viewer", false, cx));
        editor.focus_handle(cx).focus(window, cx);
        Self {
            editor,
            other_editor,
            viewer,
            open_menu: None,
        }
    }

    fn render_menu_bar(
        &mut self,
        menus: &[OwnedMenu],
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut titles = div().flex().flex_row().h(px(28.)).bg(rgb(0x222222));
        for (ix, menu) in menus.iter().enumerate() {
            let name = menu.name.clone();
            titles = titles.child(
                div()
                    .id(("menu-title", ix))
                    .debug_selector(|| format!("menu-title-{name}"))
                    .px_3()
                    .py_1()
                    .text_color(rgb(0xffffff))
                    .when(self.open_menu == Some(ix), |title| title.bg(rgb(0x444444)))
                    .child(name.clone())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_menu = if this.open_menu == Some(ix) {
                            None
                        } else {
                            Some(ix)
                        };
                        cx.notify();
                    })),
            );
        }
        let popup = self
            .open_menu
            .and_then(|ix| menus.get(ix))
            .map(|menu| self.render_popup(menu, window, cx));
        div()
            .relative()
            .child(titles)
            .children(popup)
            .into_any_element()
    }

    fn render_popup(
        &mut self,
        menu: &OwnedMenu,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut popup = div()
            .absolute()
            // The popup covers the surfaces below it: they must not see its mouse events.
            .occlude()
            .top(px(28.))
            .left_0()
            .w(px(240.))
            .flex()
            .flex_col()
            .bg(rgb(0x1a1a1a))
            .border_1()
            .border_color(rgb(0x555555));
        for entry in menu_entries(menu, window, cx) {
            popup = popup.child(self.render_entry(entry, 0, cx));
        }
        // Deferred so the popup paints and hit-tests above the surfaces drawn after the bar.
        deferred(popup).with_priority(1).into_any_element()
    }

    fn render_entry(
        &mut self,
        entry: MenuEntry,
        depth: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match entry {
            MenuEntry::Separator => div().h(px(1.)).my_1().bg(rgb(0x555555)).into_any_element(),
            MenuEntry::Submenu {
                name,
                disabled,
                entries,
            } => {
                let mut group = div().flex().flex_col().child(
                    div()
                        .pl(px(8. + 12. * depth as f32))
                        .text_color(if disabled {
                            rgb(0x777777)
                        } else {
                            rgb(0xffffff)
                        })
                        .child(SharedString::from(format!("{name} >"))),
                );
                for entry in entries {
                    group = group.child(self.render_entry(entry, depth + 1, cx));
                }
                group.into_any_element()
            }
            MenuEntry::Item(item) => {
                let label = item.label.clone();
                let mut row = div()
                    .id(SharedString::from(format!("menu-item-{label}")))
                    .debug_selector(|| format!("menu-item-{label}"))
                    .flex()
                    .flex_row()
                    .justify_between()
                    .pl(px(8. + 12. * depth as f32))
                    .pr_2()
                    .text_color(if item.enabled {
                        rgb(0xffffff)
                    } else {
                        rgb(0x777777)
                    })
                    .child(SharedString::from(format!(
                        "{}{}",
                        if item.checked { "* " } else { "" },
                        item.label
                    )))
                    .child(SharedString::from(
                        item.equivalent.clone().unwrap_or_default(),
                    ));
                if item.enabled {
                    let action = item.action;
                    row = row
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(0x333333)))
                        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_menu = None;
                            // Same path as a shortcut: dispatch to the focused element.
                            window.dispatch_action(action.boxed_clone(), cx);
                            cx.notify();
                        }));
                }
                row.into_any_element()
            }
        }
    }
}

impl Render for MenuDemo {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let menus = cx.get_menus().unwrap_or_default();
        let bar = self.render_menu_bar(&menus, window, cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x111111))
            .on_action(cx.listener(|_, _: &NewDocument, _, cx| log(cx, "new document")))
            .on_action(cx.listener(|_, _: &Minimize, window, cx| {
                log(cx, "minimize");
                window.minimize_window();
            }))
            .on_action(cx.listener(|_, _: &Zoom, window, cx| {
                log(cx, "zoom");
                window.zoom_window();
            }))
            .on_action(cx.listener(|_, _: &CloseWindow, window, cx| {
                log(cx, "close window");
                window.remove_window();
            }))
            .child(bar)
            .child(
                div()
                    .p_2()
                    .gap_2()
                    .flex_row()
                    .flex()
                    .child(self.editor.clone())
                    .child(self.other_editor.clone())
                    .child(self.viewer.clone()),
            )
    }
}
