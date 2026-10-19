//! The tab chip, its close slot and the strip region wrapper (CMP-017 R6, R7, R9).

use super::model::{FADE_BAND, TAB_HEIGHT, TAB_WIDTH, TabId};
use crate::nav_theme::NavTheme;
use std::time::Duration;
use vespertine::{
    AnyView, App, AppContext, ClickEvent, Context, Div, ElementId, FocusHandle, InteractiveElement,
    IntoElement, MouseButton, ParentElement, Render, Role, ScrollHandle, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Window, div, edge_faded, prelude::FluentBuilder, px, svg,
};

/// Size of the leading icon slot.
pub const ICON_SLOT: f32 = 18.0;
/// Size of the leading icon.
pub const ICON_SIZE: f32 = 12.0;
/// Size of the trailing close slot.
pub const CLOSE_SLOT: f32 = 18.0;
/// Size of the close glyph.
pub const CLOSE_GLYPH: f32 = 12.0;
/// Size of the unsaved indicator.
pub const DIRTY_DOT: f32 = 6.0;
/// Opacity of the leading icon on an unselected tab.
pub const INACTIVE_ICON_ALPHA: f32 = 0.7;
/// Tooltip text and accessible label of the close slot.
pub const CLOSE_LABEL: &str = "Close tab";
/// Suffix of a dirty tab's accessible name.
pub const UNSAVED_SUFFIX: &str = ", unsaved changes";

/// The wash of a selected tab.
pub const SELECTED_WASH: f32 = 0.10;
/// The wash of a hovered unselected tab.
pub const HOVER_WASH: f32 = 0.06;
/// The wash of a hovered close slot.
pub const SLOT_WASH: f32 = 0.12;

/// A tab's leading icon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TabIcon {
    /// A vector icon by asset path.
    Svg(SharedString),
    /// A short text glyph.
    Glyph(SharedString),
}

/// What the owner says about one tab.
#[derive(Clone, Debug, PartialEq)]
pub struct TabSpec {
    /// The stable identity.
    pub id: TabId,
    /// The title shown on the chip (one line, truncated).
    pub title: SharedString,
    /// An optional leading icon.
    pub icon: Option<TabIcon>,
    /// Whether the tab has unsaved changes.
    pub dirty: bool,
    /// A longer accessible name; the title is used when absent.
    pub accessible_name: Option<SharedString>,
    /// An optional detail shown as a tooltip after 350 ms of hover.
    pub detail: Option<SharedString>,
}

impl TabSpec {
    /// A clean tab with only an id and a title.
    pub fn new(id: impl Into<TabId>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            icon: None,
            dirty: false,
            accessible_name: None,
            detail: None,
        }
    }

    /// The same tab with a leading icon.
    pub fn icon(mut self, icon: TabIcon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// The same tab with the dirty flag set.
    pub fn dirty(mut self, dirty: bool) -> Self {
        self.dirty = dirty;
        self
    }

    /// The same tab with a longer accessible name.
    pub fn accessible_name(mut self, name: impl Into<SharedString>) -> Self {
        self.accessible_name = Some(name.into());
        self
    }

    /// The same tab with a detail tooltip.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// The accessible name: the title, or the longer name when supplied, with
    /// ", unsaved changes" appended when dirty (R6).
    pub fn label(&self) -> SharedString {
        let name = self.accessible_name.as_ref().unwrap_or(&self.title);
        if self.dirty {
            format!("{name}{UNSAVED_SUFFIX}").into()
        } else {
            name.clone()
        }
    }
}

/// The opacities of the two stacked layers of the close slot (R6): `(dot, glyph)`. The dirty
/// dot shows while the tab is not hovered; on hover of the whole tab the dot hides and the
/// glyph shows. A clean tab shows the glyph only on hover.
pub fn close_slot_opacity(dirty: bool, tab_hovered: bool) -> (f32, f32) {
    if tab_hovered {
        (0.0, 1.0)
    } else {
        (if dirty { 1.0 } else { 0.0 }, 0.0)
    }
}

/// The group name that ties a chip's hover to its close slot layers.
fn group_name(id: &TabId) -> SharedString {
    format!("tab-chip:{id}").into()
}

struct TextTooltip {
    text: SharedString,
    theme: NavTheme,
}

impl Render for TextTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.))
            .py(px(4.))
            .rounded(px(4.))
            .border_1()
            .border_color(self.theme.popup_border)
            .bg(self.theme.popup_surface)
            .text_color(self.theme.text)
            .text_size(px(11.))
            .child(self.text.clone())
    }
}

pub(crate) fn tooltip(
    text: SharedString,
    theme: &NavTheme,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let theme = theme.clone();
    move |_window, cx| {
        cx.new(|_| TextTooltip {
            text: text.clone(),
            theme: theme.clone(),
        })
        .into()
    }
}

/// The builder of one tab chip (R7): a 112 x 24 px rounded chip with the icon slot, the title
/// and the close slot. The strip adds selection, drag and context handlers to the element
/// [`TabChip::into_element`] returns; the chip itself owns its appearance and the close slot.
pub struct TabChip {
    spec: TabSpec,
    theme: NavTheme,
    selected: bool,
    focus: Option<FocusHandle>,
    on_close: Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>,
}

/// A tab chip for `spec`; `on_close` runs when the close slot is activated.
pub fn tab_chip(
    spec: &TabSpec,
    theme: &NavTheme,
    on_close: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> TabChip {
    TabChip {
        spec: spec.clone(),
        theme: theme.clone(),
        selected: false,
        focus: None,
        on_close: Box::new(on_close),
    }
}

impl TabChip {
    /// Draws the chip selected.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Makes the chip a keyboard tab stop tracking `focus`.
    pub fn track_focus(mut self, focus: &FocusHandle) -> Self {
        self.focus = Some(focus.clone());
        self
    }

    /// The chip element. Its debug selector is `tab:<id>`, its close slot `tab-close:<id>`.
    pub fn into_element(self) -> Stateful<Div> {
        let Self {
            spec,
            theme,
            selected,
            focus,
            on_close,
        } = self;
        let group = group_name(&spec.id);
        let id = spec.id.clone();
        let label = spec.label();
        let (dot, glyph) = close_slot_opacity(spec.dirty, false);
        let text_color = if selected { theme.text } else { theme.muted };

        let icon = spec.icon.clone().map(|icon| {
            let color = if selected {
                theme.muted
            } else {
                vespertine::Hsla {
                    a: theme.muted.a * INACTIVE_ICON_ALPHA,
                    ..theme.muted
                }
            };
            div()
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .size(px(ICON_SLOT))
                .text_size(px(ICON_SIZE))
                .text_color(color)
                .map(|slot| match icon {
                    TabIcon::Svg(path) => {
                        slot.child(svg().path(path).size(px(ICON_SIZE)).text_color(color))
                    }
                    TabIcon::Glyph(glyph) => slot.child(glyph),
                })
        });

        let close_slot = div()
            .id(ElementId::Name(format!("tab-close:{id}").into()))
            .debug_selector({
                let id = id.clone();
                move || format!("tab-close:{id}")
            })
            .role(Role::Button)
            .aria_label(CLOSE_LABEL)
            .relative()
            .flex_none()
            .size(px(CLOSE_SLOT))
            .rounded(px(4.))
            .hover({
                let wash = theme.wash(SLOT_WASH);
                move |style| style.bg(wash)
            })
            .tooltip(tooltip(CLOSE_LABEL.into(), &theme))
            // The slot claims the primary press: the tab behind it never sees it, so pressing
            // the slot neither starts a drag nor selects the tab.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(on_close)
            .child(
                div()
                    .absolute()
                    .top(px((CLOSE_SLOT - DIRTY_DOT) / 2.))
                    .left(px((CLOSE_SLOT - DIRTY_DOT) / 2.))
                    .size(px(DIRTY_DOT))
                    .rounded_full()
                    .bg(theme.muted)
                    .opacity(dot)
                    .group_hover(group.clone(), |style| {
                        style.opacity(close_slot_opacity(true, true).0)
                    }),
            )
            .child(
                div()
                    .absolute()
                    .size(px(CLOSE_SLOT))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(CLOSE_GLYPH))
                    .text_color(theme.muted)
                    .opacity(glyph)
                    .group_hover(group.clone(), |style| {
                        style.opacity(close_slot_opacity(false, true).1)
                    })
                    .child("\u{00d7}"),
            );

        let hover_wash = theme.wash(HOVER_WASH);
        div()
            .id(ElementId::Name(format!("tab:{id}").into()))
            .debug_selector({
                let id = id.clone();
                move || format!("tab:{id}")
            })
            .group(group)
            .role(Role::Tab)
            .aria_label(label)
            .aria_selected(selected)
            .relative()
            .flex()
            .flex_none()
            .flex_row()
            .items_center()
            .gap(px(3.))
            .w(px(TAB_WIDTH))
            .h(px(TAB_HEIGHT))
            .px(px(4.))
            .rounded(px(6.))
            .text_size(px(11.5))
            .text_color(text_color)
            .cursor_pointer()
            // The chip blocks pointer input for window-chrome drag regions, but never wheel
            // input, so an overflowing strip scrolls with the pointer over any chip.
            .block_mouse_except_scroll()
            // Pressing a chip never takes focus away from the tab content: reordering or
            // switching must not unfocus it. Keyboard users reach chips through the tab order.
            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
            .on_mouse_down(MouseButton::Middle, |_, window, _| window.prevent_default())
            .on_mouse_down(MouseButton::Right, |_, window, _| window.prevent_default())
            .when(selected, |chip| chip.bg(theme.wash(SELECTED_WASH)))
            .when(!selected, |chip| {
                chip.hover(move |style| style.bg(hover_wash))
            })
            .when_some(focus, |chip, focus| {
                chip.track_focus(&focus.tab_stop(true)).focus_visible({
                    let accent = theme.accent;
                    move |style| style.border_2().border_color(accent)
                })
            })
            .when_some(spec.detail.clone(), |chip, detail| {
                chip.tooltip_show_delay(Duration::from_millis(super::model::TOOLTIP_DELAY_MS))
                    .tooltip(tooltip(detail, &theme))
            })
            .children(icon)
            .child(
                div()
                    .id(ElementId::Name(format!("tab-title:{id}").into()))
                    .debug_selector(move || format!("tab-title:{id}"))
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(spec.title),
            )
            .child(close_slot)
    }
}

/// The strip region wrapper (R9): `strip` with a [`FADE_BAND`] px edge fade of its content on
/// every hidden edge of `scroll` (hidden when scrolled more than 1 px, or with more than 1 px
/// left to scroll). The fade is a scoped edge fade (RND-001): chips under it stay interactive
/// and no backdrop-coloured cover is painted. `theme` is accepted for owners that prefer
/// theme-aware wrappers; the scoped fade needs no colour.
pub fn tab_strip_region(
    strip: impl IntoElement,
    scroll: &ScrollHandle,
    _theme: &NavTheme,
) -> impl IntoElement {
    edge_faded(px(FADE_BAND), false, false, strip)
        .fade_left(true)
        .fade_right(true)
        .fade_overflow_x(scroll)
}
