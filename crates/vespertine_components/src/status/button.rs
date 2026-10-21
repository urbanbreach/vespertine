//! A small focusable button shared by the status kit: retry, toggle, fold and dismiss
//! actions are ordinary buttons reachable by keyboard and pointer (CMP-023 R1, R12).

use std::rc::Rc;
use std::time::Duration;

use vespertine::{
    AnyView, App, AppContext as _, Div, ElementId, Hsla, InteractiveElement, IntoElement, Outline,
    ParentElement, Render, Role, SharedString, Stateful, StatefulInteractiveElement, Styled,
    Window, div, px,
};

/// Handler of a button activation.
pub type PressHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// A text-only tooltip view.
pub struct TextTooltip {
    text: SharedString,
    background: Hsla,
    color: Hsla,
}

impl Render for TextTooltip {
    fn render(&mut self, _: &mut Window, _: &mut vespertine::Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.0))
            .py(px(4.0))
            .rounded(px(6.0))
            .bg(self.background)
            .text_color(self.color)
            .text_size(px(12.0))
            .child(self.text.clone())
    }
}

/// Delay before a tooltip appears.
pub const TOOLTIP_DELAY: Duration = Duration::from_millis(350);

/// A focusable button element: role button with an accessible `label`, tab stop, activation by
/// click or a whole Enter/Space press (the core keyboard click), and an optional tooltip after
/// 350 ms.
pub fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    tooltip: Option<(SharedString, Hsla, Hsla)>,
    focus_ring: Hsla,
    on_press: PressHandler,
) -> Stateful<Div> {
    let id: ElementId = id.into();
    let label: SharedString = label.into();
    let mut element = div()
        .id(id)
        .role(Role::Button)
        .aria_label(label)
        .focusable()
        .tab_stop(true)
        .cursor_pointer()
        // An inset outline: the ring covers the button's edge pixels without taking layout
        // space or moving the content.
        .focus_visible(move |style| style.outline(Outline::new(px(1.), focus_ring, px(-1.))))
        .on_click(move |_, window, cx| on_press(window, cx));
    if let Some((text, background, color)) = tooltip {
        element = element
            .tooltip(move |_, cx| {
                AnyView::from(cx.new(|_| TextTooltip {
                    text: text.clone(),
                    background,
                    color,
                }))
            })
            .tooltip_show_delay(TOOLTIP_DELAY);
    }
    element
}
