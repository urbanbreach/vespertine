//! Small self-contained widgets of the browser chrome: a tooltip, the globe glyph and the
//! icon-button style of the toolbar (CMP-022 R2). They are intentionally minimal; a shared
//! component library (CMP-001, CMP-004) can replace them without touching the surface logic.

use super::theme::{BrowserMetrics, BrowserTheme};
use vespertine::{
    AnyView, App, AppContext, ClickEvent, Context, CursorStyle, Div, ElementId, Hsla,
    InteractiveElement, IntoElement, ParentElement, Pixels, Render, Role, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px,
};

/// Opacity of a disabled toolbar button.
pub const DISABLED_BUTTON_OPACITY: f32 = 0.35;

/// A text tooltip card.
pub(super) struct TextTooltip {
    text: SharedString,
    theme: BrowserTheme,
}

impl Render for TextTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.))
            .py(px(4.))
            .rounded(px(4.))
            .border_1()
            .border_color(self.theme.border)
            .bg(self.theme.raised)
            .text_color(self.theme.text)
            .text_size(px(11.))
            .child(self.text.clone())
    }
}

/// A tooltip builder for `.tooltip(..)` showing `text`.
pub(super) fn tooltip(
    text: &'static str,
    theme: &BrowserTheme,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let theme = theme.clone();
    move |_window, cx| {
        cx.new(|_| TextTooltip {
            text: text.into(),
            theme: theme.clone(),
        })
        .into()
    }
}

/// A globe glyph of the given pixel size: a circle with a meridian ellipse and an equator.
pub(super) fn globe(size: Pixels, color: Hsla) -> Div {
    let line = px(1.);
    div()
        .relative()
        .flex_shrink_0()
        .size(size)
        .rounded_full()
        .border_1()
        .border_color(color)
        .child(
            div()
                .absolute()
                .top(px(0.))
                .bottom(px(0.))
                .left(size * 0.5 - size * 0.18 - line)
                .w(size * 0.36)
                .rounded_full()
                .border_1()
                .border_color(color),
        )
        .child(
            div()
                .absolute()
                .left(px(0.))
                .right(px(0.))
                .top(size * 0.5 - line)
                .h(line)
                .bg(color),
        )
}

/// A toolbar icon button (R2): square, glyph in the muted colour, wash on hover and a tooltip
/// with its label while enabled; disabled it has opacity 0.35, the default cursor and no click
/// handler. `on_click` is only installed when `enabled`.
pub(super) fn icon_button(
    id: &'static str,
    label: &'static str,
    glyph: &'static str,
    enabled: bool,
    theme: &BrowserTheme,
    metrics: &BrowserMetrics,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let hover = theme.hover;
    let tooltip_theme = theme.clone();
    div()
        .id(ElementId::Name(id.into()))
        .debug_selector(|| id.to_owned())
        .role(Role::Button)
        .aria_label(label)
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .size(metrics.button_size)
        .rounded(metrics.button_radius)
        .text_size(metrics.glyph_size)
        .text_color(theme.muted)
        .child(glyph)
        .when(!enabled, |button| {
            button
                .opacity(DISABLED_BUTTON_OPACITY)
                .cursor(CursorStyle::Arrow)
        })
        .when(enabled, move |button| {
            button
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .tooltip(tooltip(label, &tooltip_theme))
                .on_click(on_click)
        })
}
