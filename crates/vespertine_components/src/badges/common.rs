//! Shared pieces of the CMP-028 components: text tooltips, action buttons and icon buttons.

use std::rc::Rc;

use vespertine::{
    AnyView, App, ClickEvent, Context, ElementId, FontWeight, Hsla, InteractiveElement,
    IntoElement, MouseButton, ParentElement, Render, Role, SharedString, Stateful,
    StatefulInteractiveElement, Styled, Window, div, px, svg,
};

use super::tokens::BadgeTokens;

/// A click handler of a control.
pub type Handler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// A plain text tooltip chip.
struct TextTooltip {
    text: SharedString,
    tokens: BadgeTokens,
}

impl Render for TextTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.))
            .py(px(4.))
            .rounded(px(6.))
            .border_1()
            .border_color(self.tokens.popover_border)
            .bg(self.tokens.popover)
            .text_size(px(12.))
            .text_color(self.tokens.muted)
            .child(self.text.clone())
    }
}

/// A tooltip builder showing `text`.
pub(super) fn text_tooltip(
    text: SharedString,
    tokens: BadgeTokens,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    use vespertine::AppContext as _;
    move |_, cx| {
        let text = text.clone();
        let tokens = tokens.clone();
        cx.new(|_| TextTooltip { text, tokens }).into()
    }
}

/// A square icon button (copy, edit, remove) labelled `label`: `size` px, `radius` px, a `glyph`
/// px icon in `color`. It is a keyboard tab stop activated by click or a whole Enter/Space press
/// (the core keyboard click), with a `focus_ring` border while keyboard-focused. The click stops
/// propagation so a surrounding click or dismiss handler never fires.
#[allow(clippy::too_many_arguments)]
pub(super) fn icon_button(
    id: impl Into<ElementId>,
    label: SharedString,
    icon: &'static str,
    size: f32,
    glyph: f32,
    radius: f32,
    color: Hsla,
    hover_background: Hsla,
    focus_ring: Hsla,
    on_click: Handler,
) -> Stateful<vespertine::Div> {
    focusable_button(div().id(id), label, focus_ring, on_click)
        .flex()
        .items_center()
        .justify_center()
        .flex_none()
        .size(px(size))
        .rounded(px(radius))
        .hover(move |style| style.bg(hover_background))
        .child(
            svg()
                .path(icon)
                .size(px(glyph))
                .flex_none()
                .text_color(color),
        )
}

/// The behavior shared by the badge buttons: role button named `label`, a keyboard tab stop with
/// a focus ring, and `on_click` for a click or a whole Enter/Space press. Neither the press nor
/// the click reaches the surrounding element.
fn focusable_button(
    element: Stateful<vespertine::Div>,
    label: SharedString,
    focus_ring: Hsla,
    on_click: Handler,
) -> Stateful<vespertine::Div> {
    element
        .role(Role::Button)
        .aria_label(label)
        .focusable()
        .tab_stop(true)
        .cursor_pointer()
        .focus_visible(move |style| style.border_1().border_color(focus_ring))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(move |event, window, cx| {
            cx.stop_propagation();
            on_click(event, window, cx);
        })
}

/// A text action: height 22, horizontal padding 10, radius 6, 11 px medium text. Primary actions
/// use the accent background with on-accent text; secondary actions blend from muted to
/// foreground text over a transparent to element-hover background.
pub(super) fn action_button(
    id: impl Into<ElementId>,
    label: SharedString,
    tokens: &BadgeTokens,
    primary: bool,
    on_click: Handler,
) -> Stateful<vespertine::Div> {
    let tokens = tokens.clone();
    let base = focusable_button(div().id(id), label.clone(), tokens.text, on_click)
        .flex()
        .items_center()
        .flex_none()
        .h(px(22.))
        .px(px(10.))
        .rounded(px(6.))
        .text_size(px(11.))
        .font_weight(FontWeight::MEDIUM);
    let styled = if primary {
        let hover = tokens.accent.opacity(0.85);
        base.bg(tokens.accent)
            .text_color(tokens.on_accent)
            .hover(move |style| style.bg(hover))
    } else {
        let (text, hover) = (tokens.text, tokens.element_hover);
        base.text_color(tokens.muted)
            .hover(move |style| style.text_color(text).bg(hover))
    };
    styled.child(label)
}
