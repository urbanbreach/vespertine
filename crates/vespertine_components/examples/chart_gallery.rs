//! CMP-024 A1: every chart kind (grouped bars, stacked bars, horizontal bars, line, area,
//! scatter) with two series, an empty chart, a constant-value chart and a missing-sample chart.
//! Hover any chart to read its category (or nearest scatter point) in the readout row above it.

use vespertine::{
    App, AppContext as _, Bounds, Context, IntoElement, ParentElement, Render, Styled, Window,
    WindowBounds, WindowOptions, div, px, rgb, size,
};
use vespertine_components::chart::{Chart, ChartElement, ChartKind, NumberFormat, Series};
use vespertine_platform::application;

fn days() -> Vec<String> {
    ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat"]
        .into_iter()
        .map(String::from)
        .collect()
}

fn values(offset: f64) -> Vec<Option<f64>> {
    (0..6)
        .map(|i| Some(20.0 + offset + f64::from(i) * 9.0 + f64::from(i % 3) * 14.0))
        .collect()
}

fn two_series() -> Vec<Series> {
    vec![
        Series::new("latency").values(values(0.0)),
        Series::new("budget").values(values(25.0)),
    ]
}

fn charts() -> Vec<(&'static str, Chart)> {
    let ms = NumberFormat::new().unit("ms");
    let base = |kind| {
        Chart::new()
            .kind(kind)
            .height(130.0)
            .x(days())
            .and_then(|c| c.series(two_series()))
            .expect("valid chart")
            .format(ms.clone())
    };
    let scatter = Chart::new()
        .kind(ChartKind::Scatter)
        .height(130.0)
        .format(ms.clone())
        .x_format(NumberFormat::new().unit("s"))
        .series(vec![
            Series::new("a").points(
                (0..24)
                    .map(|i| (f64::from(i), 20.0 + f64::from((i * 7) % 23) * 3.0))
                    .collect(),
            ),
            Series::new("b").points(
                (0..24)
                    .map(|i| (f64::from(i) + 0.5, 30.0 + f64::from((i * 5) % 17) * 4.0))
                    .collect(),
            ),
        ])
        .expect("valid scatter");
    vec![
        ("grouped bars", base(ChartKind::Bar)),
        ("stacked bars", base(ChartKind::Bar).stacked(true)),
        ("horizontal bars", base(ChartKind::Bar).horizontal(true)),
        ("line", base(ChartKind::Line)),
        ("area", base(ChartKind::Area)),
        ("scatter", scatter),
        ("empty", Chart::new().height(130.0)),
        (
            "constant",
            Chart::new()
                .height(130.0)
                .x(days())
                .and_then(|c| c.series(vec![Series::new("flat").values(vec![Some(5.0); 6])]))
                .expect("valid constant chart"),
        ),
        (
            "missing samples",
            Chart::new()
                .kind(ChartKind::Line)
                .height(130.0)
                .x(days())
                .and_then(|c| {
                    c.series(vec![Series::new("gappy").values(vec![
                        Some(10.0),
                        Some(40.0),
                        None,
                        Some(25.0),
                        Some(f64::NAN),
                        Some(55.0),
                    ])])
                })
                .expect("valid gap chart"),
        ),
    ]
}

struct Gallery;

impl Render for Gallery {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let mut grid = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .gap(px(24.0))
            .p(px(24.0));
        for (index, (title, chart)) in charts().into_iter().enumerate() {
            grid = grid.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .w(px(380.0))
                    .child(div().text_size(px(13.0)).child(title))
                    .child(ChartElement::new(("chart", index), chart)),
            );
        }
        div()
            .size_full()
            .overflow_hidden()
            .bg(rgb(0x14161A))
            .text_color(rgb(0xE6E8EB))
            .child(grid)
    }
}

fn main() {
    application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1260.0), px(760.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| Gallery),
        )
        .expect("open window");
        cx.activate(true);
    });
}
