//! The chart view: readout row, plot canvas and hover (CMP-024 R9, R13-R17).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use vespertine::{
    App, Bounds, ContentMask, Context, Corners, ElementId, FontWeight, Hsla, InteractiveElement,
    IntoElement, LineCap, LineJoin, MouseMoveEvent, ParentElement, PathBuilder, PathStyle, Pixels,
    Point, Render, RenderOnce, SharedString, StatefulInteractiveElement, StrokeOptions, Styled,
    Subscription, TextAlign, TextRun, Window, canvas, div, fill, point, px, size,
};
use vespertine_theme::appearance;
use vespertine_theme::theme::{Appearance, ThemeRegistry, ThemeVariant};

use super::model::{Chart, ChartKind};
use super::palette::SeriesPalette;
use super::plan::{
    AREA_ALPHA, HAlign, HOVER_BAND_RADIUS, HOVER_DOT_RADIUS, Hover, LINE_WIDTH, Label, Mark, Plan,
    Pt, Rect, SCATTER_HOVER_RADIUS, SCATTER_RADIUS, TICK_LINE, TICK_SIZE, VAlign,
};

/// Height of the readout row (minimum, one line).
pub const READOUT_HEIGHT: f32 = 18.0;
/// Gap between the readout row and the plot.
pub const READOUT_GAP: f32 = 8.0;
const READOUT_TEXT: f32 = 12.0;

/// The theme roles a chart paints with.
#[derive(Clone, Debug, PartialEq)]
pub struct ChartColors {
    /// Gridlines.
    pub border: Hsla,
    /// Axis labels and unemphasised readout text.
    pub muted: Hsla,
    /// Readout values.
    pub text: Hsla,
    /// The hovered band.
    pub hover: Hsla,
    /// The hover rule.
    pub strong_border: Hsla,
    /// One color per series.
    pub series: Vec<Hsla>,
}

impl ChartColors {
    /// Resolves the roles in a theme variant. `palette` replaces the variant-derived series
    /// palette; explicit series colors always win.
    pub fn from_variant(
        variant: &ThemeVariant,
        palette: Option<&SeriesPalette>,
        chart: &Chart,
    ) -> Self {
        let default_palette;
        let palette = match palette {
            Some(palette) => palette,
            None => {
                default_palette = SeriesPalette::from_theme(variant);
                &default_palette
            }
        };
        let series = chart
            .series
            .iter()
            .enumerate()
            .map(|(index, series)| {
                series
                    .color
                    .map_or_else(|| palette.color(index), |color| color.resolve(variant))
                    .to_hsla()
            })
            .collect();
        Self {
            border: variant.colors.border.to_hsla(),
            muted: variant.colors.text_muted.to_hsla(),
            text: variant.colors.text.to_hsla(),
            hover: variant.colors.hover.to_hsla(),
            strong_border: variant.colors.border_strong.to_hsla(),
            series,
        }
    }

    /// The roles of the application's active theme (the built-in dark variant before a theme
    /// is installed).
    pub fn active(cx: &App, palette: Option<&SeriesPalette>, chart: &Chart) -> Self {
        match appearance::try_theme_of(cx) {
            Some(theme) => Self::from_variant(&theme.variant, palette, chart),
            None => {
                let registry = ThemeRegistry::builtin();
                Self::from_variant(registry.first_variant(Appearance::Dark), palette, chart)
            }
        }
    }
}

/// One legend entry of the readout.
#[derive(Clone, Debug, PartialEq)]
pub struct LegendEntry {
    /// Series index (its color).
    pub series: usize,
    /// Series name.
    pub name: String,
    /// The formatted value at the hovered category, when the series has one.
    pub value: Option<String>,
}

/// The content of the readout row above the plot (R15).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Readout {
    /// The hovered category label or the scatter "x · y" text.
    pub head: Option<String>,
    /// Legend entries.
    pub entries: Vec<LegendEntry>,
}

/// Computes the readout of `chart` for the hover state.
pub fn readout(chart: &Chart, hover: Option<Hover>) -> Readout {
    let category = match hover {
        Some(Hover::Category(index)) => Some(index),
        _ => None,
    };
    let head = match hover {
        Some(Hover::Category(index)) => chart.x.get(index).cloned(),
        Some(Hover::Point { series, index }) => chart
            .series
            .get(series)
            .and_then(|s| s.points.get(index))
            .map(|&(x, y)| {
                format!(
                    "{} \u{b7} {}",
                    chart.x_format.format(x),
                    chart.format.format(y)
                )
            }),
        None => None,
    };
    let any_named = chart.series.iter().any(|series| !series.name.is_empty());
    let shown = category.is_some() || (chart.legend && (chart.series.len() > 1) && any_named);
    let entries = if shown {
        chart
            .series
            .iter()
            .enumerate()
            .map(|(index, series)| LegendEntry {
                series: index,
                name: series.name.clone(),
                value: category
                    .and_then(|c| series.value_at(c))
                    .map(|value| chart.format.format(value)),
            })
            .collect()
    } else {
        Vec::new()
    };
    Readout { head, entries }
}

/// A chart view: owns the descriptor, the hover state and the most recent plan.
pub struct ChartView {
    chart: Arc<Chart>,
    palette: Option<SeriesPalette>,
    hover: Option<Hover>,
    plan: Rc<RefCell<Option<Rc<Plan>>>>,
    _theme: Subscription,
}

impl ChartView {
    /// A view of `chart`.
    pub fn new(chart: Chart, cx: &mut Context<Self>) -> Self {
        let this = cx.entity().downgrade();
        let subscription = appearance::observe_changes(cx, move |cx| {
            this.update(cx, |_, cx| cx.notify()).ok();
        });
        Self {
            chart: Arc::new(chart),
            palette: None,
            hover: None,
            plan: Rc::new(RefCell::new(None)),
            _theme: subscription,
        }
    }

    /// Replaces the descriptor; notifies only when it changed.
    pub fn set_chart(&mut self, chart: Chart, cx: &mut Context<Self>) {
        if *self.chart != chart {
            self.chart = Arc::new(chart);
            self.hover = None;
            cx.notify();
        }
    }

    /// Replaces the series palette (`None` restores the theme-derived one).
    pub fn set_palette(&mut self, palette: Option<SeriesPalette>, cx: &mut Context<Self>) {
        if self.palette != palette {
            self.palette = palette;
            cx.notify();
        }
    }

    /// The descriptor.
    pub fn chart(&self) -> &Chart {
        &self.chart
    }

    /// The current hover.
    pub fn hover(&self) -> Option<Hover> {
        self.hover
    }

    /// The most recent plan (recorded in prepaint), if the chart has been laid out.
    pub fn plan(&self) -> Option<Rc<Plan>> {
        self.plan.borrow().clone()
    }

    /// The readout for the current hover.
    pub fn readout(&self) -> Readout {
        readout(&self.chart, self.hover)
    }

    /// Resolves a pointer position in window coordinates against the most recent plan and
    /// updates the hover; redraws only on change.
    pub fn pointer_moved(&mut self, pointer: Point<Pixels>, cx: &mut Context<Self>) {
        let hover = self.plan().and_then(|plan| {
            plan.hover(
                &self.chart,
                Pt::new(f32::from(pointer.x), f32::from(pointer.y)),
            )
        });
        self.set_hover(hover, cx);
    }

    /// Releases the hover (the pointer left the plot).
    pub fn pointer_left(&mut self, cx: &mut Context<Self>) {
        self.set_hover(None, cx);
    }

    fn set_hover(&mut self, hover: Option<Hover>, cx: &mut Context<Self>) {
        if self.hover != hover {
            self.hover = hover;
            cx.notify();
        }
    }
}

fn to_px_rect(rect: Rect) -> Bounds<Pixels> {
    Bounds {
        origin: point(px(rect.x), px(rect.y)),
        size: size(px(rect.w.max(0.0)), px(rect.h.max(0.0))),
    }
}

fn px_point(point: Pt) -> Point<Pixels> {
    vespertine::point(px(point.x), px(point.y))
}

fn paint_dot(window: &mut Window, center: Pt, radius: f32, color: Hsla) {
    let bounds = Bounds {
        origin: point(px(center.x - radius), px(center.y - radius)),
        size: size(px(radius * 2.0), px(radius * 2.0)),
    };
    window.paint_quad(fill(bounds, color).corner_radii(Corners::all(px(radius))));
}

fn paint_polygon(window: &mut Window, points: &[Pt], color: Hsla) {
    let mut builder = PathBuilder::fill();
    let points: Vec<Point<Pixels>> = points.iter().map(|p| px_point(*p)).collect();
    builder.add_polygon(&points, true);
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

fn paint_polyline(window: &mut Window, points: &[Pt], color: Hsla) {
    let mut builder = PathBuilder::stroke(px(LINE_WIDTH)).with_style(PathStyle::Stroke(
        StrokeOptions::default()
            .with_line_width(LINE_WIDTH)
            .with_line_cap(LineCap::Round)
            .with_line_join(LineJoin::Round),
    ));
    for (index, point) in points.iter().enumerate() {
        if index == 0 {
            builder.move_to(px_point(*point));
        } else {
            builder.line_to(px_point(*point));
        }
    }
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

fn shape(window: &Window, text: &str, size_px: f32, color: Hsla) -> vespertine::ShapedLine {
    let run = TextRun {
        len: text.len(),
        font: window.text_style().font(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
        letter_spacing: Pixels::ZERO,
    };
    window.text_system().shape_line(
        SharedString::from(text.to_string()),
        px(size_px),
        &[run],
        None,
    )
}

fn paint_label(
    window: &mut Window,
    cx: &mut App,
    label: &Label,
    size_px: f32,
    line_height: f32,
    color: Hsla,
) {
    let mut text = label.text.clone();
    let mut line = shape(window, &text, size_px, color);
    if let Some(max) = label.max_width {
        while f32::from(line.width()) > max && text.chars().count() > 1 {
            text.pop();
            let truncated = format!("{}\u{2026}", text.trim_end());
            line = shape(window, &truncated, size_px, color);
        }
    }
    let width = f32::from(line.width());
    let x = match label.h {
        HAlign::Left => label.anchor.x,
        HAlign::Center => label.anchor.x - width / 2.0,
        HAlign::Right => label.anchor.x - width,
    };
    let y = match label.v {
        VAlign::Top => label.anchor.y,
        VAlign::Center => label.anchor.y - line_height / 2.0,
    };
    line.paint(
        point(px(x), px(y)),
        px(line_height),
        TextAlign::Left,
        None,
        window,
        cx,
    )
    .ok();
}

fn paint_plan(
    window: &mut Window,
    cx: &mut App,
    plan: &Plan,
    chart: &Chart,
    colors: &ChartColors,
    hover: Option<Hover>,
    scale: f32,
) {
    if !plan.paintable {
        return;
    }
    let series_color = |series: usize| colors.series.get(series).copied().unwrap_or(colors.text);
    let plot = to_px_rect(plan.plot);
    window.with_content_mask(Some(ContentMask { bounds: plot }), |window| {
        for line in &plan.gridlines {
            let bounds = if (line.from.y - line.to.y).abs() < f32::EPSILON {
                Rect::new(line.from.x, line.from.y - 0.5, line.to.x - line.from.x, 1.0)
            } else {
                Rect::new(line.from.x - 0.5, line.from.y, 1.0, line.to.y - line.from.y)
            };
            window.paint_quad(fill(to_px_rect(bounds), colors.border));
        }
        if let (ChartKind::Bar, Some(Hover::Category(index))) = (plan.kind, hover) {
            window.paint_quad(
                fill(to_px_rect(plan.band_rect(index)), colors.hover)
                    .corner_radii(Corners::all(px(HOVER_BAND_RADIUS))),
            );
        }
        for mark in &plan.marks {
            match mark {
                Mark::Bar {
                    rect,
                    series,
                    radius,
                } => {
                    window.paint_quad(
                        fill(to_px_rect(*rect), series_color(*series))
                            .corner_radii(Corners::all(px(*radius))),
                    );
                }
                Mark::Area { polygon, series } => {
                    let mut color = series_color(*series);
                    color.a *= AREA_ALPHA;
                    paint_polygon(window, polygon, color);
                }
                Mark::Polyline { points, series } => {
                    paint_polyline(window, points, series_color(*series));
                }
                Mark::Dot {
                    center,
                    radius,
                    series,
                } => {
                    paint_dot(window, *center, *radius, series_color(*series));
                }
            }
        }
        if let Some(Hover::Category(index)) = hover {
            let overlay = plan.hover_overlay(chart, index);
            if let Some((x, top, bottom)) = overlay.rule {
                window.paint_quad(fill(
                    to_px_rect(Rect::new(x - 0.5, top, 1.0, bottom - top)),
                    colors.strong_border,
                ));
            }
            for (center, series) in overlay.dots {
                paint_dot(window, center, HOVER_DOT_RADIUS, series_color(series));
            }
        }
        for scatter in &plan.points {
            let hovered = hover
                == Some(Hover::Point {
                    series: scatter.series,
                    index: scatter.index,
                });
            let radius = if hovered {
                SCATTER_HOVER_RADIUS
            } else {
                SCATTER_RADIUS
            };
            let vertices: Vec<Pt> = (0..14)
                .map(|i| {
                    let angle = std::f32::consts::TAU * i as f32 / 14.0;
                    Pt::new(
                        scatter.pos.x + radius * angle.cos(),
                        scatter.pos.y + radius * angle.sin(),
                    )
                })
                .collect();
            paint_polygon(window, &vertices, series_color(scatter.series));
        }
    });
    for label in &plan.labels {
        paint_label(
            window,
            cx,
            label,
            TICK_SIZE * scale,
            TICK_LINE * scale,
            colors.muted,
        );
    }
}

impl Render for ChartView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chart = self.chart.clone();
        let colors = Rc::new(ChartColors::active(cx, self.palette.as_ref(), &chart));
        let readout = self.readout();
        let hover = self.hover;
        let scale = f32::from(window.rem_size()) / 16.0;
        let plan_cell = self.plan.clone();
        let paint_chart = chart.clone();
        let paint_colors = colors.clone();

        let mut row = div()
            .items_center()
            .flex()
            .flex_wrap()
            .flex_row()
            .gap_y(px(2.0))
            .gap_x(px(14.0))
            .min_h(px(READOUT_HEIGHT))
            .mb(px(READOUT_GAP))
            .text_size(px(READOUT_TEXT))
            .line_height(px(READOUT_HEIGHT))
            .text_color(colors.muted);
        if let Some(head) = readout.head {
            row = row.child(
                div()
                    .text_color(colors.text)
                    .font_weight(FontWeight::MEDIUM)
                    .child(head),
            );
        }
        for entry in readout.entries {
            let swatch = colors
                .series
                .get(entry.series)
                .copied()
                .unwrap_or(colors.text);
            row = row.child(
                div()
                    .items_center()
                    .flex()
                    .gap(px(6.0))
                    .flex_row()
                    .child(
                        div()
                            .w(px(8.0))
                            .h(px(8.0))
                            .rounded(px(2.0))
                            .flex_none()
                            .bg(swatch),
                    )
                    .child(entry.name)
                    .children(
                        entry
                            .value
                            .map(|value| div().text_color(colors.text).child(value)),
                    ),
            );
        }

        let plot = canvas(
            move |bounds, window, _| {
                let scale = f32::from(window.rem_size()) / 16.0;
                let rect = Rect::new(
                    f32::from(bounds.origin.x),
                    f32::from(bounds.origin.y),
                    f32::from(bounds.size.width),
                    f32::from(bounds.size.height),
                );
                let measure = |text: &str| {
                    f32::from(shape(window, text, TICK_SIZE * scale, Hsla::default()).width())
                };
                let plan = Rc::new(Plan::build(&paint_chart, rect, scale, &measure));
                *plan_cell.borrow_mut() = Some(plan.clone());
                (plan, paint_chart)
            },
            move |_, (plan, chart), window, cx| {
                paint_plan(window, cx, &plan, &chart, &paint_colors, hover, scale);
            },
        )
        .size_full();

        div().flex().flex_col().w_full().child(row).child(
            div()
                .id("chart-plot")
                .relative()
                .w_full()
                .h(px(chart.height))
                .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                    this.pointer_moved(event.position, cx);
                }))
                .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                    if !*hovered {
                        this.pointer_left(cx);
                    }
                }))
                .child(plot),
        )
    }
}

/// A chart as an element: keeps its [`ChartView`] (and so its hover) across frames under `id`.
#[derive(IntoElement)]
pub struct ChartElement {
    id: ElementId,
    chart: Chart,
    palette: Option<SeriesPalette>,
}

impl ChartElement {
    /// A chart element with a stable identity.
    pub fn new(id: impl Into<ElementId>, chart: Chart) -> Self {
        Self {
            id: id.into(),
            chart,
            palette: None,
        }
    }

    /// Replaces the series palette.
    pub fn palette(mut self, palette: SeriesPalette) -> Self {
        self.palette = Some(palette);
        self
    }
}

impl RenderOnce for ChartElement {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let view = window.use_keyed_state(self.id, cx, {
            let chart = self.chart.clone();
            move |_, cx| ChartView::new(chart, cx)
        });
        view.update(cx, |view, cx| {
            view.set_chart(self.chart, cx);
            view.set_palette(self.palette, cx);
        });
        view
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chart::model::Series;

    fn two_series() -> Chart {
        Chart::new()
            .x(vec!["Mon".into(), "Tue".into()])
            .unwrap()
            .series(vec![
                Series::new("a").values(vec![Some(1.0), Some(2.5)]),
                Series::new("b").values(vec![Some(3.0), None]),
            ])
            .unwrap()
    }

    #[test]
    fn a9_readout_shows_category_swatches_names_and_values() {
        let chart = two_series();
        let idle = readout(&chart, None);
        assert_eq!(idle.head, None);
        assert_eq!(idle.entries.len(), 2);
        assert!(idle.entries.iter().all(|entry| entry.value.is_none()));
        let hovered = readout(&chart, Some(Hover::Category(1)));
        assert_eq!(hovered.head.as_deref(), Some("Tue"));
        assert_eq!(hovered.entries[0].value.as_deref(), Some("2.50"));
        // A gap shows no value for its series.
        assert_eq!(hovered.entries[1].value, None);
    }

    #[test]
    fn a9_single_unnamed_series_shows_no_legend_until_hovered() {
        let chart = Chart::new()
            .x(vec!["a".into()])
            .unwrap()
            .series(vec![Series::new("").values(vec![Some(1.0)])])
            .unwrap();
        assert!(readout(&chart, None).entries.is_empty());
        assert_eq!(readout(&chart, Some(Hover::Category(0))).entries.len(), 1);
        // Hovering shows entries even when the legend flag is off.
        let hidden = two_series().legend(false);
        assert!(readout(&hidden, None).entries.is_empty());
        assert_eq!(readout(&hidden, Some(Hover::Category(0))).entries.len(), 2);
    }

    #[test]
    fn scatter_readout_joins_x_and_y_with_a_middle_dot() {
        let chart = Chart::new()
            .kind(ChartKind::Scatter)
            .x_format(crate::chart::NumberFormat::new().unit("s"))
            .format(crate::chart::NumberFormat::new().unit("ms"))
            .series(vec![Series::new("p").points(vec![(1.5, 25.0)])])
            .unwrap();
        let hovered = readout(
            &chart,
            Some(Hover::Point {
                series: 0,
                index: 0,
            }),
        );
        assert_eq!(hovered.head.as_deref(), Some("1.50 s \u{b7} 25 ms"));
    }
}
