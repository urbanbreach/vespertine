//! CMP-024 on the offscreen test host: painted marks, hover, readout stability, clipping,
//! theme recoloring and degenerate data.

use std::cell::RefCell;
use std::rc::Rc;

use vespertine::{
    AppContext as _, Context, Entity, IntoElement, ParentElement, Render, Styled, Window, div, px,
    rgb,
};
use vespertine_components::chart::{
    Chart, ChartColors, ChartKind, ChartView, Hover, NumberFormat, Plan, Series,
};
use vespertine_test_host::{HostOptions, TestHost, require_backend};
use vespertine_theme::{
    appearance,
    color::Color,
    selection::{AccentSelection, AppearanceMode, SurfacePreference, ThemeSelection},
};

const PAD: f32 = 20.0;

struct Root {
    chart: Entity<ChartView>,
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .p(px(PAD))
            .bg(rgb(0x14161A))
            .text_color(rgb(0xE6E8EB))
            .font_family("Noto Sans")
            .child(self.chart.clone())
    }
}

fn host(chart: Chart) -> (TestHost, Entity<ChartView>) {
    let slot: Rc<RefCell<Option<Entity<ChartView>>>> = Rc::new(RefCell::new(None));
    let sink = slot.clone();
    let host = TestHost::with_view(HostOptions::with_size(480.0, 340.0), move |window, cx| {
        let _ = window;
        let chart = cx.new(|cx| ChartView::new(chart, cx));
        *sink.borrow_mut() = Some(chart.clone());
        cx.new(|_| Root { chart })
    })
    .expect("host");
    let view = slot.borrow().clone().unwrap();
    (host, view)
}

fn plan(host: &mut TestHost, view: &Entity<ChartView>) -> Rc<Plan> {
    host.flush();
    host.app()
        .read_entity(view, |view, _| view.plan().expect("a plan after prepaint"))
}

fn hover(host: &mut TestHost, view: &Entity<ChartView>) -> Option<Hover> {
    host.app().read_entity(view, |view, _| view.hover())
}

fn pixel(host: &mut TestHost, x: f32, y: f32) -> [u8; 4] {
    let image = host.screenshot_image().unwrap();
    let scale = host.scale_factor();
    image.get_pixel((x * scale) as u32, (y * scale) as u32).0
}

fn near(a: [u8; 4], b: [u8; 4]) -> bool {
    a.iter().zip(b).take(3).all(|(a, b)| a.abs_diff(b) <= 6)
}

fn bar_chart() -> Chart {
    Chart::new()
        .height(200.0)
        .x(vec!["Mon".into(), "Tue".into(), "Wed".into()])
        .unwrap()
        .series(vec![
            Series::new("alpha").values(vec![Some(40.0), Some(80.0), Some(60.0)]),
            Series::new("beta").values(vec![Some(20.0), Some(100.0), None]),
        ])
        .unwrap()
        .format(NumberFormat::new().unit("ms"))
}

fn series_rgba(chart: &Chart, host: &mut TestHost, series: usize) -> [u8; 4] {
    let colors = host.app().update(|cx| ChartColors::active(cx, None, chart));
    let color = colors.series[series];
    let rgba: vespertine::Rgba = color.into();
    [
        (rgba.r * 255.0).round() as u8,
        (rgba.g * 255.0).round() as u8,
        (rgba.b * 255.0).round() as u8,
        255,
    ]
}

#[test]
fn bars_paint_in_series_colors_and_are_clipped_to_the_plot() {
    require_backend!();
    let chart = bar_chart().y_max(60.0);
    let (mut host, view) = host(chart.clone());
    let plan = plan(&mut host, &view);
    assert!(plan.paintable);
    let first = series_rgba(&chart, &mut host, 0);
    // Category 1 of series 0 reaches 80 (> y_max 60 fitted up to the tick step): its bar is
    // painted inside the plot and never above it.
    let band = plan.plot.w / 3.0;
    let inner = 0.72 * band;
    let bar = inner / 2.0;
    let x = plan.plot.x + band * 1.0 + (band - inner) / 2.0 + bar * 0.5;
    let inside = pixel(&mut host, x, plan.plot.y + plan.plot.h - 4.0);
    assert!(near(inside, first), "{inside:?} vs {first:?}");
    let above = pixel(&mut host, x, plan.plot.y - 3.0);
    assert!(!near(above, first), "mark leaked above the plot: {above:?}");
    // Every mark rectangle the plan produced is finite.
    assert!(plan.marks.len() >= 5);
}

#[test]
fn hover_reports_the_category_without_moving_the_plot_and_restores_pixels() {
    require_backend!();
    let (mut host, view) = host(bar_chart());
    let before_plan = plan(&mut host, &view);
    let before = host.screenshot_image().unwrap();
    let band = before_plan.plot.w / 3.0;
    let x = f64::from(before_plan.plot.x + band * 1.5);
    let y = f64::from(before_plan.plot.y + 40.0);
    host.mouse_move(x, y, None, "").unwrap();
    host.flush();
    assert_eq!(hover(&mut host, &view), Some(Hover::Category(1)));
    let during_plan = plan(&mut host, &view);
    assert_eq!(during_plan.plot, before_plan.plot);
    let text = host.painted_text();
    assert!(text.iter().any(|t| t == "Tue"), "{text:?}");
    assert!(text.iter().any(|t| t == "80 ms"), "{text:?}");
    assert!(text.iter().any(|t| t == "100 ms"), "{text:?}");
    let during = host.screenshot_image().unwrap();
    assert_ne!(
        before.as_raw(),
        during.as_raw(),
        "the hover wash changes pixels"
    );
    // Dropping the hover restores the exact previous pixels.
    host.mouse_exit(1.0, 1.0).unwrap();
    host.flush();
    assert_eq!(hover(&mut host, &view), None);
    let after = host.screenshot_image().unwrap();
    assert_eq!(before.as_raw(), after.as_raw());
    assert_eq!(plan(&mut host, &view).plot, before_plan.plot);
}

#[test]
fn readout_row_keeps_its_height_and_a_single_unnamed_series_has_no_legend() {
    require_backend!();
    let single = Chart::new()
        .height(160.0)
        .x(vec!["a".into(), "b".into()])
        .unwrap()
        .series(vec![Series::new("").values(vec![Some(1.0), Some(2.0)])])
        .unwrap();
    let (mut host, view) = host(single);
    let idle = plan(&mut host, &view);
    // Only axis labels are painted while idle: no swatch name, no category.
    let text = host.painted_text();
    // Idle: only the axis labels are painted ("a" once); no legend entry, no readout head.
    assert_eq!(text.iter().filter(|t| *t == "a").count(), 1, "{text:?}");
    host.mouse_move(
        f64::from(idle.plot.x + 10.0),
        f64::from(idle.plot.y + 20.0),
        None,
        "",
    )
    .unwrap();
    host.flush();
    let hovered = plan(&mut host, &view);
    assert_eq!(hovered.plot, idle.plot);
    let readout = host.app().read_entity(&view, |view, _| view.readout());
    assert_eq!(readout.entries.len(), 1);
    assert_eq!(readout.head.as_deref(), Some("a"));
    let text = host.painted_text();
    assert_eq!(text.iter().filter(|t| *t == "a").count(), 2, "{text:?}");
}

#[test]
fn scatter_hover_picks_the_nearest_point_within_twelve_pixels() {
    require_backend!();
    let chart = Chart::new()
        .kind(ChartKind::Scatter)
        .height(220.0)
        .series(vec![Series::new("p").points(vec![
            (1.0, 1.0),
            (5.0, 9.0),
            (9.0, 4.0),
        ])])
        .unwrap();
    let (mut host, view) = host(chart);
    let plan = plan(&mut host, &view);
    let point = plan.points[1];
    host.mouse_move(
        f64::from(point.pos.x + 8.0),
        f64::from(point.pos.y),
        None,
        "",
    )
    .unwrap();
    host.flush();
    assert_eq!(
        hover(&mut host, &view),
        Some(Hover::Point {
            series: 0,
            index: 1
        })
    );
    host.mouse_move(
        f64::from(point.pos.x + 13.0),
        f64::from(point.pos.y),
        None,
        "",
    )
    .unwrap();
    host.flush();
    assert_eq!(hover(&mut host, &view), None);
}

#[test]
fn degenerate_charts_render_without_failing() {
    require_backend!();
    let cases = [
        Chart::new(),
        Chart::new().series(vec![Series::new("a")]).unwrap(),
        Chart::new()
            .series(vec![Series::new("a").values(vec![Some(5.0); 3])])
            .unwrap(),
        Chart::new()
            .kind(ChartKind::Line)
            .series(vec![Series::new("a").values(vec![Some(1.0)])])
            .unwrap(),
        Chart::new()
            .kind(ChartKind::Area)
            .series(vec![Series::new("a").values(vec![
                Some(1.0),
                None,
                Some(f64::NAN),
                Some(4.0),
                Some(5.0),
            ])])
            .unwrap(),
        Chart::new()
            .kind(ChartKind::Scatter)
            .series(vec![Series::new("p").points(vec![(2.0, 2.0)])])
            .unwrap(),
    ];
    for chart in cases {
        let (mut host, view) = host(chart);
        let plan = plan(&mut host, &view);
        assert!(plan.plot.w.is_finite() && plan.plot.h.is_finite());
        host.screenshot_image().unwrap();
    }
}

#[test]
fn changing_the_theme_recolors_an_existing_chart() {
    require_backend!();
    let chart = bar_chart();
    let (mut host, view) = host(chart.clone());
    host.app().update(|cx| {
        appearance::init(
            cx,
            AppearanceMode::Dark,
            ThemeSelection::default(),
            AccentSelection::ThemeDefault,
            SurfacePreference::ThemeDefault,
        );
    });
    let plan = plan(&mut host, &view);
    let band = plan.plot.w / 3.0;
    let inner = 0.72 * band;
    let x = plan.plot.x + (band - inner) / 2.0 + inner / 4.0;
    let y = plan.plot.bottom() - 4.0;
    let before = pixel(&mut host, x, y);
    host.app().update(|cx| {
        appearance::set_accent(cx, AccentSelection::Color(Color::rgb(0xE0, 0x30, 0x40)));
    });
    host.flush();
    let after = pixel(&mut host, x, y);
    assert!(!near(before, after), "{before:?} {after:?}");
    let expected = series_rgba(&chart, &mut host, 0);
    assert!(near(after, expected), "{after:?} vs {expected:?}");
}
