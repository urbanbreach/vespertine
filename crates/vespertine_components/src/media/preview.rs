//! The inline image preview (CMP-020 R8).
//!
//! A centered button that takes the full available width but at most the image's natural width,
//! at most 480 px tall, keeping the image's aspect ratio with contain fitting. Loading and
//! failure show the framework's image placeholder in the same reserved box, so the box does not
//! change size between loading and loaded when the natural size is known. A click stops
//! propagation and opens the viewer through the host's callback.

use super::decode::DecodedMedia;
use std::rc::Rc;
use vespertine::{
    App, ClickEvent, ElementId, IMAGE_LOAD_FAILED_LABEL, ImageSource, InteractiveElement,
    IntoElement, ObjectFit, ParentElement, Pixels, RenderOnce, Role, SharedString, Size,
    StatefulInteractiveElement, Styled, StyledImage, Window, div, image_placeholder, img, px,
};

/// Maximum height of the preview.
pub const PREVIEW_MAX_HEIGHT: f32 = 480.0;
/// Default accessible label of the preview button.
pub const PREVIEW_LABEL: &str = "Enlarge image";
/// Label shown in the loading state.
pub const PREVIEW_LOADING_LABEL: &str = "Loading image";
/// Height reserved for a preview whose natural size is unknown.
pub const UNKNOWN_SIZE_HEIGHT: f32 = 120.0;

/// What the preview shows.
#[derive(Clone, Debug)]
pub enum PreviewMedia {
    /// The image is still loading; `natural_size` reserves the box when the host knows it.
    Loading {
        /// Natural size known in advance.
        natural_size: Option<Size<Pixels>>,
    },
    /// The decoded image.
    Ready(DecodedMedia),
    /// Decoding or loading failed.
    Failed {
        /// The error message.
        message: SharedString,
        /// Natural size known in advance.
        natural_size: Option<Size<Pixels>>,
    },
}

impl PreviewMedia {
    fn natural(&self) -> Option<(f32, f32)> {
        let size = match self {
            PreviewMedia::Loading { natural_size } | PreviewMedia::Failed { natural_size, .. } => {
                (*natural_size)?
            }
            PreviewMedia::Ready(media) => media.natural_size(),
        };
        let (w, h) = (f32::from(size.width), f32::from(size.height));
        (w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0).then_some((w, h))
    }
}

/// The inline preview element, see [`preview_element`].
#[derive(IntoElement)]
pub struct PreviewElement {
    media: PreviewMedia,
    id: ElementId,
    on_click: Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>,
    label: SharedString,
}

/// The inline preview of `media`; `on_click` opens the full view.
pub fn preview_element(
    media: PreviewMedia,
    id: impl Into<ElementId>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> PreviewElement {
    PreviewElement {
        media,
        id: id.into(),
        on_click: Rc::new(on_click),
        label: SharedString::new_static(PREVIEW_LABEL),
    }
}

impl PreviewElement {
    /// Overrides the accessible label (default "Enlarge image").
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = label.into();
        self
    }
}

impl RenderOnce for PreviewElement {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let natural = self.media.natural();
        let content = match &self.media {
            PreviewMedia::Ready(media) => img(ImageSource::Render(media.image().clone()))
                .size_full()
                .object_fit(ObjectFit::Contain)
                .into_any_element(),
            PreviewMedia::Loading { .. } => image_placeholder(
                ElementId::from((self.id.clone(), "loading")),
                PREVIEW_LOADING_LABEL,
                None,
            ),
            PreviewMedia::Failed { message, .. } => image_placeholder(
                ElementId::from((self.id.clone(), "failed")),
                IMAGE_LOAD_FAILED_LABEL,
                Some(message.clone()),
            ),
        };
        let on_click = self.on_click;
        let selector = format!("preview:{}", self.id);
        let mut button = div()
            .id(self.id)
            .debug_selector(move || selector)
            .relative()
            .flex_shrink_1()
            .max_h(px(PREVIEW_MAX_HEIGHT))
            .cursor_pointer()
            .role(Role::Button)
            .aria_label(self.label)
            .on_click(move |event, window, cx| {
                cx.stop_propagation();
                on_click(event, window, cx);
            });
        button = match natural {
            Some((w, h)) => button.flex_basis(px(w)).aspect_ratio(w / h),
            None => button.w_full().h(px(UNKNOWN_SIZE_HEIGHT)),
        };
        div()
            .w_full()
            .flex()
            .flex_row()
            .justify_center()
            .items_start()
            .child(button.child(div().absolute().size_full().child(content)))
    }
}
