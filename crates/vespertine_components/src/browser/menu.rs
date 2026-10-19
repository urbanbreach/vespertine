//! The page-provided menu (CMP-022 R14): its selection logic and the floating card.

use super::theme::BrowserTheme;
use std::rc::Rc;
use vespertine::{
    Anchor, AnchoredPositionMode, App, ElementId, InteractiveElement, IntoElement, ParentElement,
    Role, ScrollHandle, Stateful, StatefulInteractiveElement, Styled, Window, anchored, deferred,
    div, point, prelude::FluentBuilder, px,
};
use vespertine_webview::protocol::Command;
use vespertine_webview::{MenuItem, MenuRequest};

/// Width of the menu card in logical pixels.
pub const MENU_WIDTH: f32 = 240.;
/// Maximum height of the menu card in logical pixels.
pub const MENU_MAX_HEIGHT: f32 = 320.;
/// Opacity of disabled rows.
pub const DISABLED_OPACITY: f32 = 0.4;

/// An open page menu: the request and the active row.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuState {
    request: MenuRequest,
    active: usize,
}

impl MenuState {
    /// The initial active row is the first with `selected` true, otherwise row 0.
    pub fn new(request: MenuRequest) -> Self {
        let active = request
            .items
            .iter()
            .position(|item| item.selected)
            .unwrap_or(0);
        Self { request, active }
    }

    /// The request the menu was opened with.
    pub fn request(&self) -> &MenuRequest {
        &self.request
    }

    /// The rows.
    pub fn items(&self) -> &[MenuItem] {
        &self.request.items
    }

    /// Index of the active row.
    pub fn active(&self) -> usize {
        self.active
    }

    /// Moves the active row down (`forward`) or up with wrap-around, skipping disabled rows (at
    /// most one full turn; nothing moves when every row is disabled).
    pub fn step(&mut self, forward: bool) {
        let count = self.request.items.len();
        if count == 0 {
            return;
        }
        let mut index = self.active;
        for _ in 0..count {
            index = if forward {
                (index + 1) % count
            } else {
                (index + count - 1) % count
            };
            if self.request.items[index].enabled {
                self.active = index;
                return;
            }
        }
    }

    /// Makes row `index` the active one (hovering).
    pub fn set_active(&mut self, index: usize) {
        if index < self.request.items.len() {
            self.active = index;
        }
    }

    /// The row at `index` when it is enabled.
    pub fn enabled_item(&self, index: usize) -> Option<&MenuItem> {
        self.request.items.get(index).filter(|item| item.enabled)
    }
}

/// How an activated row is answered to the helper.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuCommand {
    /// Send this command.
    Send(Command),
    /// Read the clipboard's plain text and send it as a `text` command (nothing without text).
    Paste,
    /// Unknown action: nothing to send.
    None,
}

/// Maps a row's action to what the surface does for it.
pub fn command_for_action(action: &str) -> MenuCommand {
    match action {
        "text" => MenuCommand::Paste,
        "open-link" => MenuCommand::Send(Command::MenuAction("open-link")),
        "copy-link" => MenuCommand::Send(Command::MenuAction("copy-link")),
        "copy" => MenuCommand::Send(Command::MenuAction("copy")),
        "select-all" => MenuCommand::Send(Command::MenuAction("select-all")),
        "back" => MenuCommand::Send(Command::MenuAction("back")),
        "forward" => MenuCommand::Send(Command::MenuAction("forward")),
        "reload" => MenuCommand::Send(Command::MenuAction("reload")),
        other => match other
            .strip_prefix("option:")
            .and_then(|index| index.parse::<usize>().ok())
        {
            Some(index) => MenuCommand::Send(Command::Option(index)),
            None => MenuCommand::None,
        },
    }
}

/// Everything the card needs besides the state.
pub(super) struct MenuCard<'a> {
    pub state: &'a MenuState,
    pub theme: &'a BrowserTheme,
    pub scroll: &'a ScrollHandle,
    pub on_activate: Rc<dyn Fn(usize, &mut Window, &mut App)>,
    pub on_hover: Rc<dyn Fn(usize, &mut Window, &mut App)>,
    pub on_dismiss: Rc<dyn Fn(&mut Window, &mut App)>,
}

/// The floating menu card, anchored at the page origin plus the request's position. It must be a
/// child of the (relatively positioned) page body so that the position is relative to the page.
pub(super) fn menu_card(card: MenuCard<'_>) -> impl IntoElement {
    let MenuCard {
        state,
        theme,
        scroll,
        on_activate,
        on_hover,
        on_dismiss,
    } = card;
    let rows = state.items().iter().enumerate().map(|(index, item)| {
        let activate = on_activate.clone();
        let hover = on_hover.clone();
        let active = index == state.active();
        let row: Stateful<_> = div()
            .id(ElementId::NamedInteger(
                "browser-menu-row".into(),
                index as u64,
            ))
            .role(Role::MenuItem)
            .aria_label(item.label.clone())
            .flex()
            .items_center()
            .h(px(28.))
            .px(px(10.))
            .flex_shrink_0()
            .rounded(px(4.))
            .text_size(px(12.))
            .text_color(theme.text)
            .when(active && item.enabled, |row| row.bg(theme.hover))
            .when(!item.enabled, |row| row.opacity(DISABLED_OPACITY))
            .when(item.enabled, |row| {
                row.cursor_pointer()
                    .on_mouse_move(move |_, window, cx| hover(index, window, cx))
                    .on_click(move |_, window, cx| activate(index, window, cx))
            })
            .child(item.label.clone());
        row
    });
    let position = point(px(state.request().x as f32), px(state.request().y as f32));
    // The wrapper pins the anchored element's flow position to the body's origin so that the
    // local position is relative to the page.
    div().absolute().top(px(0.)).left(px(0.)).child(
        deferred(
            anchored()
                .position_mode(AnchoredPositionMode::Local)
                .position(position)
                .anchor(Anchor::TopLeft)
                .snap_to_window_with_margin(px(4.))
                .child(
                    div()
                        .id("browser-menu")
                        .debug_selector(|| "browser-menu".to_owned())
                        .role(Role::Menu)
                        .occlude()
                        // A press outside dismisses the menu and never reaches the page (R14).
                        .on_mouse_down_out(move |_, window, cx| {
                            on_dismiss(window, cx);
                            cx.stop_propagation();
                        })
                        .w(px(MENU_WIDTH))
                        .max_h(px(MENU_MAX_HEIGHT))
                        .overflow_y_scroll()
                        .track_scroll(scroll)
                        .flex()
                        .flex_col()
                        .gap(px(1.))
                        .p(px(4.))
                        .rounded(px(8.))
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.raised)
                        .shadow_lg()
                        .children(rows),
                ),
        )
        .priority(1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(label: &str, action: &str, enabled: bool, selected: bool) -> MenuItem {
        MenuItem {
            label: label.into(),
            action: action.into(),
            enabled,
            selected,
        }
    }

    fn state(items: Vec<MenuItem>) -> MenuState {
        MenuState::new(MenuRequest {
            x: 0.,
            y: 0.,
            items,
        })
    }

    #[test]
    fn the_initial_row_is_the_first_selected_one_or_zero() {
        let plain = state(vec![
            item("a", "x", true, false),
            item("b", "y", true, false),
        ]);
        assert_eq!(plain.active(), 0);
        let marked = state(vec![
            item("a", "x", true, false),
            item("b", "y", true, true),
            item("c", "z", true, true),
        ]);
        assert_eq!(marked.active(), 1);
    }

    #[test]
    fn stepping_wraps_and_skips_disabled_rows() {
        let mut menu = state(vec![
            item("a", "a", true, false),
            item("b", "b", false, false),
            item("c", "c", true, false),
        ]);
        menu.step(true);
        assert_eq!(menu.active(), 2);
        menu.step(true);
        assert_eq!(menu.active(), 0);
        menu.step(false);
        assert_eq!(menu.active(), 2);
        let mut disabled = state(vec![
            item("a", "a", false, false),
            item("b", "b", false, false),
        ]);
        disabled.step(true);
        assert_eq!(disabled.active(), 0);
        let mut empty = state(Vec::new());
        empty.step(true);
        assert_eq!(empty.active(), 0);
    }

    #[test]
    fn actions_map_to_commands() {
        assert_eq!(command_for_action("text"), MenuCommand::Paste);
        assert_eq!(
            command_for_action("copy-link"),
            MenuCommand::Send(Command::MenuAction("copy-link"))
        );
        assert_eq!(
            command_for_action("option:2"),
            MenuCommand::Send(Command::Option(2))
        );
        assert_eq!(command_for_action("option:x"), MenuCommand::None);
        assert_eq!(command_for_action("frobnicate"), MenuCommand::None);
    }
}
