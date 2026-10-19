//! Small self-contained widgets of the code card header (CMP-014 R8): vector icons painted as
//! strokes in the given color, and a text tooltip. A shared component library (CMP-001, CMP-004)
//! can replace them without touching the card logic.

use vespertine::{
    AnyView, App, AppContext, Context, Hsla, IntoElement, ParentElement, Pixels, Point, Render,
    SharedString, Styled, Window, canvas, div, point, px,
};
use vespertine::{Bounds, PathBuilder};
use vespertine_theme::document::DocumentTheme;

use crate::doc_theme::{hsla, ink_alpha};

/// The icons of the card header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    /// Two overlapping sheets.
    Copy,
    /// A check mark.
    Check,
    /// Text lines with a return arrow (the wrap toggle).
    Wrap,
    /// A warning triangle.
    Warning,
}

fn polyline(builder: &mut PathBuilder, bounds: Bounds<Pixels>, points: &[(f32, f32)], close: bool) {
    let origin = bounds.origin;
    let size = bounds.size;
    let map = |(x, y): (f32, f32)| -> Point<Pixels> {
        point(origin.x + size.width * x, origin.y + size.height * y)
    };
    let mapped: Vec<Point<Pixels>> = points.iter().copied().map(map).collect();
    builder.add_polygon(&mapped, close);
}

/// An icon of `size` painted in `color`.
pub fn icon(kind: Icon, icon_size: Pixels, color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, (), window, _| {
            let mut builder = PathBuilder::stroke(px((icon_size.as_f32() / 11.).max(1.)));
            match kind {
                Icon::Copy => {
                    polyline(
                        &mut builder,
                        bounds,
                        &[(0.30, 0.30), (0.88, 0.30), (0.88, 0.88), (0.30, 0.88)],
                        true,
                    );
                    polyline(
                        &mut builder,
                        bounds,
                        &[(0.12, 0.64), (0.12, 0.12), (0.64, 0.12)],
                        false,
                    );
                }
                Icon::Check => polyline(
                    &mut builder,
                    bounds,
                    &[(0.14, 0.54), (0.40, 0.78), (0.86, 0.24)],
                    false,
                ),
                Icon::Wrap => {
                    polyline(&mut builder, bounds, &[(0.10, 0.20), (0.90, 0.20)], false);
                    polyline(
                        &mut builder,
                        bounds,
                        &[(0.10, 0.50), (0.74, 0.50), (0.74, 0.74), (0.42, 0.74)],
                        false,
                    );
                    polyline(
                        &mut builder,
                        bounds,
                        &[(0.52, 0.64), (0.40, 0.74), (0.52, 0.84)],
                        false,
                    );
                }
                Icon::Warning => {
                    polyline(
                        &mut builder,
                        bounds,
                        &[(0.50, 0.10), (0.92, 0.86), (0.08, 0.86)],
                        true,
                    );
                    polyline(&mut builder, bounds, &[(0.50, 0.40), (0.50, 0.62)], false);
                    polyline(&mut builder, bounds, &[(0.50, 0.73), (0.50, 0.75)], false);
                }
            }
            if let Ok(path) = builder.build() {
                window.paint_path(path, color);
            }
        },
    )
    .size(icon_size)
    .flex_none()
}

/// A text tooltip card.
pub struct TextTooltip {
    text: SharedString,
    theme: DocumentTheme,
}

impl Render for TextTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.))
            .py(px(4.))
            .rounded(px(4.))
            .border_1()
            .border_color(hsla(self.theme.border))
            .bg(hsla(self.theme.bg).blend(ink_alpha(&self.theme, 0.08)))
            .text_color(hsla(self.theme.text))
            .text_size(px(11.))
            .child(self.text.clone())
    }
}

/// A tooltip builder for `.tooltip(..)` showing `text`.
pub fn tooltip(
    text: impl Into<SharedString>,
    theme: &DocumentTheme,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text = text.into();
    let theme = theme.clone();
    move |_window, cx| {
        cx.new(|_| TextTooltip {
            text: text.clone(),
            theme: theme.clone(),
        })
        .into()
    }
}
