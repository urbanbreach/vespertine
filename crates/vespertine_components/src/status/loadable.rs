//! The loadable-state wrapper (CMP-023 R1): idle, loading, ready or error with an inline retry
//! action inside a slot whose outer size is fixed by the caller.

use std::rc::Rc;

use vespertine::{
    AnyElement, App, ElementId, IntoElement, ParentElement, Pixels, RenderOnce, SharedString,
    Styled, Window, div, px,
};

use super::button::button;
use super::skeleton::{skeleton_block, skeleton_rows};
use super::theme::StatusColors;

/// The state of a loadable value.
#[derive(Clone, Debug, PartialEq)]
pub enum Loadable<T> {
    /// Nothing requested yet: the placeholder shows.
    Idle,
    /// Work in flight: the placeholder shows.
    Loading,
    /// The content.
    Ready(T),
    /// A failure: the message and an inline retry action show.
    Error(SharedString),
}

/// Which of the four presentations a state selects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presentation {
    /// The placeholder (idle).
    Placeholder,
    /// The placeholder (loading).
    Loading,
    /// The content.
    Content,
    /// The error message and retry action.
    Error,
}

impl<T> Loadable<T> {
    /// The presentation of this state.
    pub fn presentation(&self) -> Presentation {
        match self {
            Self::Idle => Presentation::Placeholder,
            Self::Loading => Presentation::Loading,
            Self::Ready(_) => Presentation::Content,
            Self::Error(_) => Presentation::Error,
        }
    }
}

type Placeholder = Box<dyn FnOnce(&mut Window, &mut App) -> AnyElement>;
type ContentBuilder<T> = Box<dyn FnOnce(T, &mut Window, &mut App) -> AnyElement>;
type ErrorBuilder = Box<dyn FnOnce(&SharedString, &mut Window, &mut App) -> AnyElement>;
type RetryHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// The loadable wrapper. It renders exactly one presentation inside a slot whose size the
/// caller fixes (explicit width and height, or a minimum height): the slot's outer size is
/// identical in all four states. The wrapper never changes state by itself; the owner moves it
/// to loading and back after a retry.
#[derive(IntoElement)]
pub struct LoadableSlot<T: 'static> {
    id: ElementId,
    state: Loadable<T>,
    width: Option<Pixels>,
    height: Option<Pixels>,
    min_height: Option<Pixels>,
    placeholder: Option<Placeholder>,
    content: Option<ContentBuilder<T>>,
    error: Option<ErrorBuilder>,
    on_retry: Option<RetryHandler>,
    retry_label: SharedString,
    loading_label: SharedString,
}

/// A loadable slot for `state`.
pub fn loadable<T: 'static>(id: impl Into<ElementId>, state: Loadable<T>) -> LoadableSlot<T> {
    LoadableSlot {
        id: id.into(),
        state,
        width: None,
        height: None,
        min_height: None,
        placeholder: None,
        content: None,
        error: None,
        on_retry: None,
        retry_label: "Retry".into(),
        loading_label: "Loading".into(),
    }
}

impl<T: 'static> LoadableSlot<T> {
    /// Fixes the slot width.
    pub fn width(mut self, width: Pixels) -> Self {
        self.width = Some(width);
        self
    }

    /// Fixes the slot height.
    pub fn height(mut self, height: Pixels) -> Self {
        self.height = Some(height);
        self
    }

    /// Sets a minimum slot height (the slot may grow only with its content).
    pub fn min_height(mut self, height: Pixels) -> Self {
        self.min_height = Some(height);
        self
    }

    /// Replaces the default placeholder (three skeleton rows) shown while idle and loading.
    pub fn placeholder(
        mut self,
        build: impl FnOnce(&mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        self.placeholder = Some(Box::new(build));
        self
    }

    /// Builds the content of the ready state.
    pub fn content(
        mut self,
        build: impl FnOnce(T, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        self.content = Some(Box::new(build));
        self
    }

    /// Replaces the default error message presentation (the retry action is always added).
    pub fn error(
        mut self,
        build: impl FnOnce(&SharedString, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        self.error = Some(Box::new(build));
        self
    }

    /// Called when the retry action is activated by pointer or keyboard.
    pub fn on_retry(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_retry = Some(Rc::new(handler));
        self
    }

    /// Overrides the accessible name of the retry action (default "Retry").
    pub fn retry_label(mut self, label: impl Into<SharedString>) -> Self {
        self.retry_label = label.into();
        self
    }

    /// Sets the accessible label of the default placeholder ("Loading <thing>").
    pub fn loading_label(mut self, label: impl Into<SharedString>) -> Self {
        self.loading_label = label.into();
        self
    }
}

impl<T: 'static> RenderOnce for LoadableSlot<T> {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = StatusColors::active(cx);
        let mut slot = div()
            .relative()
            .flex()
            .flex_col()
            .overflow_hidden()
            .min_w(px(0.0));
        if let Some(width) = self.width {
            slot = slot.w(width).flex_none();
        } else {
            slot = slot.w_full();
        }
        if let Some(height) = self.height {
            slot = slot.h(height).flex_none();
        }
        if let Some(min_height) = self.min_height {
            slot = slot.min_h(min_height);
        }
        let id = self.id;
        match self.state {
            Loadable::Idle | Loadable::Loading => {
                let body = match self.placeholder {
                    Some(build) => build(window, cx),
                    None => {
                        skeleton_block((id, "skeleton"), self.loading_label, skeleton_rows(3, 12.0))
                            .into_any_element()
                    }
                };
                slot.child(body)
            }
            Loadable::Ready(value) => {
                let body = match self.content {
                    Some(build) => build(value, window, cx),
                    None => div().into_any_element(),
                };
                slot.child(body)
            }
            Loadable::Error(message) => {
                let message_element = match self.error {
                    Some(build) => build(&message, window, cx),
                    None => div()
                        .text_size(px(12.0))
                        .text_color(colors.danger)
                        .child(message)
                        .into_any_element(),
                };
                let on_retry = self.on_retry;
                let press: super::button::PressHandler = Rc::new(move |window, cx| {
                    if let Some(handler) = &on_retry {
                        handler(window, cx);
                    }
                });
                let retry = button(
                    (id, "retry"),
                    self.retry_label.clone(),
                    None,
                    colors.accent,
                    press,
                )
                .px(px(10.0))
                .h(px(24.0))
                .flex()
                .items_center()
                .rounded(px(6.0))
                .bg(colors.hover)
                .text_size(px(12.0))
                .text_color(colors.text)
                .child(self.retry_label);
                slot.gap(px(8.0)).child(message_element).child(retry)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_select_their_presentation() {
        assert_eq!(
            Loadable::<u8>::Idle.presentation(),
            Presentation::Placeholder
        );
        assert_eq!(
            Loadable::<u8>::Loading.presentation(),
            Presentation::Loading
        );
        assert_eq!(Loadable::Ready(1u8).presentation(), Presentation::Content);
        assert_eq!(
            Loadable::<u8>::Error("x".into()).presentation(),
            Presentation::Error
        );
    }
}
