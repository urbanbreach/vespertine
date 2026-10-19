//! Determinate progress (CMP-023 R4, R5): a bar and a ring in fixed-size slots.

use std::panic::Location;

use vespertine::{
    App, Bounds, ElementId, FontWeight, Hsla, InteractiveElement as _, IntoElement, LineCap,
    LineJoin, ParentElement, PathBuilder, PathStyle, Pixels, Point, RenderOnce, Role, SharedString,
    StatefulInteractiveElement as _, StrokeOptions, Styled, Window, canvas, div, hsla, point, px,
    relative,
};

use super::motion::{arc_points, circle_points, fraction, percent_fraction};
use super::theme::{StatusColors, with_alpha};

/// Default track height of a determinate bar.
pub const BAR_HEIGHT: f32 = 6.0;

/// A determinate bar: a fixed-size slot (track height 6 px by default, fully rounded ends)
/// holding a track and a fill of `fraction x track width`. The slot never depends on the value.
#[derive(IntoElement)]
pub struct ProgressBar {
    id: ElementId,
    value: f64,
    max: f64,
    height: f32,
    label: Option<SharedString>,
    fill: Option<Hsla>,
    track: Option<Hsla>,
}

/// A progress bar of `value` out of `max`. Its accessibility identity is derived from the call
/// site unless [`ProgressBar::id`] is given.
#[track_caller]
pub fn progress_bar(value: f64, max: f64) -> ProgressBar {
    ProgressBar {
        id: ElementId::CodeLocation(*Location::caller()),
        value,
        max,
        height: BAR_HEIGHT,
        label: None,
        fill: None,
        track: None,
    }
}

impl ProgressBar {
    /// Sets the element identity.
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = id.into();
        self
    }

    /// Sets the track height.
    pub fn height(mut self, height: f32) -> Self {
        self.height = height.max(1.0);
        self
    }

    /// Sets the accessible label.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Overrides the fill color (default: the accent).
    pub fn fill(mut self, fill: Hsla) -> Self {
        self.fill = Some(fill);
        self
    }

    /// Overrides the track color (default: the theme's muted fill).
    pub fn track(mut self, track: Hsla) -> Self {
        self.track = Some(track);
        self
    }

    /// The clamped fraction this bar shows.
    pub fn fraction(&self) -> f32 {
        fraction(self.value, self.max)
    }
}

impl RenderOnce for ProgressBar {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = StatusColors::active(cx);
        let fraction = self.fraction();
        let radius = px(self.height / 2.0);
        let mut slot = div()
            .id(self.id)
            .role(Role::ProgressIndicator)
            .aria_min_numeric_value(0.0)
            .aria_max_numeric_value(if self.max.is_finite() { self.max } else { 0.0 })
            .aria_numeric_value(
                f64::from(fraction) * if self.max.is_finite() { self.max } else { 0.0 },
            )
            .w_full()
            .h(px(self.height))
            .flex_none()
            .rounded(radius)
            .overflow_hidden()
            .bg(self.track.unwrap_or(colors.track))
            .child(
                div()
                    .h_full()
                    .w(relative(fraction))
                    .rounded(radius)
                    .bg(self.fill.unwrap_or(colors.accent)),
            );
        if let Some(label) = self.label {
            slot = slot.aria_label(label);
        }
        slot
    }
}

/// How a ring looks.
#[derive(Clone, Copy, Debug, PartialEq)]
struct RingStyle {
    diameter: f32,
    radius: f32,
    stroke: f32,
    track: Option<Hsla>,
    arc: Option<Hsla>,
    labeled: bool,
}

/// A determinate ring painted on a canvas as two stroked polylines in a square slot: a
/// full-circle track and a clockwise arc starting at twelve o'clock.
#[derive(IntoElement)]
pub struct ProgressRing {
    id: ElementId,
    fraction: f32,
    percent: Option<u32>,
    style: RingStyle,
    label: Option<SharedString>,
}

impl ProgressRing {
    /// The compact ring: 16 px slot, radius 6 px, stroke 1.8 px, track the faint-text token at
    /// 25 % opacity, arc in `arc`.
    #[track_caller]
    pub fn compact(fraction: f32, arc: Hsla) -> Self {
        Self {
            id: ElementId::CodeLocation(*Location::caller()),
            fraction: sanitize(fraction),
            percent: None,
            style: RingStyle {
                diameter: 16.0,
                radius: 6.0,
                stroke: 1.8,
                track: None,
                arc: Some(arc),
                labeled: false,
            },
            label: None,
        }
    }

    /// The labeled ring for overlay on dimmed imagery: stroke 2.5 px, radius
    /// `diameter / 2 - 2.5`, track white at 22 %, arc white at 95 %, the integer percent (clamped
    /// to 100) centered as semibold 9 px text in white at 95 %.
    #[track_caller]
    pub fn labeled(percent: u32, diameter: f32) -> Self {
        Self {
            id: ElementId::CodeLocation(*Location::caller()),
            fraction: percent_fraction(percent),
            percent: Some(percent.min(100)),
            style: RingStyle {
                diameter,
                radius: diameter / 2.0 - 2.5,
                stroke: 2.5,
                track: Some(hsla(0.0, 0.0, 1.0, 0.22)),
                arc: Some(hsla(0.0, 0.0, 1.0, 0.95)),
                labeled: true,
            },
            label: None,
        }
    }

    /// An unlabeled arc with caller-supplied colors, for rings too small for text.
    #[track_caller]
    pub fn arc(fraction: f32, diameter: f32, track: Hsla, arc: Hsla) -> Self {
        let stroke = 1.8;
        Self {
            id: ElementId::CodeLocation(*Location::caller()),
            fraction: sanitize(fraction),
            percent: None,
            style: RingStyle {
                diameter,
                radius: diameter / 2.0 - stroke,
                stroke,
                track: Some(track),
                arc: Some(arc),
                labeled: false,
            },
            label: None,
        }
    }

    /// Sets the element identity.
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = id.into();
        self
    }

    /// Sets the accessible label.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// The fraction the arc sweeps.
    pub fn fraction(&self) -> f32 {
        self.fraction
    }
}

fn sanitize(fraction: f32) -> f32 {
    if fraction.is_finite() {
        fraction.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn stroke(
    points: &[(f32, f32)],
    origin: Point<Pixels>,
    width: f32,
    color: Hsla,
    window: &mut Window,
) {
    if points.len() < 2 {
        return;
    }
    let mut builder = PathBuilder::stroke(px(width)).with_style(PathStyle::Stroke(
        StrokeOptions::default()
            .with_line_width(width)
            .with_line_cap(LineCap::Round)
            .with_line_join(LineJoin::Round),
    ));
    for (index, (x, y)) in points.iter().enumerate() {
        let at = point(origin.x + px(*x), origin.y + px(*y));
        if index == 0 {
            builder.move_to(at);
        } else {
            builder.line_to(at);
        }
    }
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

impl RenderOnce for ProgressRing {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = StatusColors::active(cx);
        let style = self.style;
        let track = style
            .track
            .unwrap_or_else(|| with_alpha(colors.faint, 0.25));
        let arc = style.arc.unwrap_or(colors.accent);
        let fraction = self.fraction;
        let diameter = style.diameter;
        let plot = canvas(
            |_, _, _| (),
            move |bounds: Bounds<Pixels>, _, window: &mut Window, _| {
                let center = diameter / 2.0;
                let circle = circle_points(center, center, style.radius);
                stroke(&circle, bounds.origin, style.stroke, track, window);
                let arc_vertices = arc_points(center, center, style.radius, fraction);
                stroke(&arc_vertices, bounds.origin, style.stroke, arc, window);
            },
        )
        .absolute()
        .size_full();
        let mut slot = div()
            .id(self.id)
            .role(Role::ProgressIndicator)
            .aria_min_numeric_value(0.0)
            .aria_max_numeric_value(100.0)
            .aria_numeric_value(f64::from(fraction) * 100.0)
            .relative()
            .flex_none()
            .w(px(diameter))
            .h(px(diameter))
            .child(plot);
        if let Some(label) = self.label {
            slot = slot.aria_label(label);
        }
        if let (true, Some(percent)) = (style.labeled, self.percent) {
            slot = slot.child(
                div()
                    .size_full()
                    .absolute()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(9.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(hsla(0.0, 0.0, 1.0, 0.95))
                    .child(percent.to_string()),
            );
        }
        slot
    }
}

/// A compact ring of `fraction` with the arc in `arc`.
#[track_caller]
pub fn progress_ring(fraction: f32, arc: Hsla) -> ProgressRing {
    ProgressRing::compact(fraction, arc)
}
