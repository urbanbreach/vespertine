//! The measured layout plan of a chart (CMP-024 R7-R14): plot rectangle, domains, ticks, label
//! stride, gridlines, labels and data marks. The plan is built in prepaint from the chart's
//! measured bounds and painted from; hover hit testing reads the same plan, so plotting, axis
//! labels and hit testing always agree. Everything here is pure, in logical pixels.

use super::model::{Chart, ChartKind};
use super::scale::{Domain, category_count, value_domain, x_domain};

/// Gap between the plot and its axis labels.
pub const AXIS_GAP: f32 = 8.0;
/// Offset of labels below the plot.
pub const LABEL_OFFSET: f32 = 4.0;
/// Tick label size.
pub const TICK_SIZE: f32 = 11.0;
/// Tick label line height.
pub const TICK_LINE: f32 = 16.0;
/// Category label thinning padding.
pub const THINNING_PADDING: f32 = 12.0;
/// Share of a band occupied by its bars.
pub const BAR_BAND_SHARE: f32 = 0.72;
/// Corner radius of unstacked bars.
pub const BAR_RADIUS: f32 = 2.0;
/// Corner radius of the hover band.
pub const HOVER_BAND_RADIUS: f32 = 4.0;
/// Width of line strokes.
pub const LINE_WIDTH: f32 = 1.75;
/// Radius of the dot of a one-point run.
pub const SINGLE_DOT_RADIUS: f32 = 2.5;
/// Radius of hover dots on lines and areas.
pub const HOVER_DOT_RADIUS: f32 = 3.5;
/// Scatter dot radius.
pub const SCATTER_RADIUS: f32 = 3.0;
/// Hovered scatter dot radius.
pub const SCATTER_HOVER_RADIUS: f32 = 5.0;
/// Pointer distance within which a scatter point is picked.
pub const PICK_RADIUS: f32 = 12.0;
/// Share of the chart width a horizontal-bar category gutter may take.
pub const CATEGORY_GUTTER_SHARE: f32 = 0.35;
/// Alpha factor of area fills.
pub const AREA_ALPHA: f32 = 0.22;
/// A plot at most this wide or high paints nothing.
pub const MIN_PLOT: f32 = 4.0;

/// A point in logical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pt {
    /// Horizontal coordinate.
    pub x: f32,
    /// Vertical coordinate.
    pub y: f32,
}

impl Pt {
    /// A point.
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

/// A rectangle in logical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

impl Rect {
    /// A rectangle.
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// Right edge.
    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    /// Bottom edge.
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    fn is_finite(&self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.w.is_finite() && self.h.is_finite()
    }
}

/// Horizontal anchoring of a label at its anchor point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HAlign {
    /// The anchor is the left edge.
    Left,
    /// The anchor is the horizontal center.
    Center,
    /// The anchor is the right edge.
    Right,
}

/// Vertical anchoring of a label at its anchor point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VAlign {
    /// The anchor is the top edge.
    Top,
    /// The anchor is the vertical center.
    Center,
}

/// A tick or category label.
#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    /// The text.
    pub text: String,
    /// The anchor point.
    pub anchor: Pt,
    /// Horizontal anchoring.
    pub h: HAlign,
    /// Vertical anchoring.
    pub v: VAlign,
    /// Width the text is truncated to (horizontal-bar category gutter).
    pub max_width: Option<f32>,
}

/// A one-pixel gridline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gridline {
    /// One end.
    pub from: Pt,
    /// The other end.
    pub to: Pt,
}

/// A data mark.
#[derive(Clone, Debug, PartialEq)]
pub enum Mark {
    /// A bar.
    Bar {
        /// Its rectangle.
        rect: Rect,
        /// The series.
        series: usize,
        /// Corner radius (0 when stacked).
        radius: f32,
    },
    /// A stroked polyline of a run of at least two points.
    Polyline {
        /// The vertices.
        points: Vec<Pt>,
        /// The series.
        series: usize,
    },
    /// A filled area under a run of at least two points: the run plus the baseline corners.
    Area {
        /// The polygon.
        polygon: Vec<Pt>,
        /// The series.
        series: usize,
    },
    /// A filled dot (a one-point run).
    Dot {
        /// Center.
        center: Pt,
        /// Radius.
        radius: f32,
        /// The series.
        series: usize,
    },
}

/// A scatter point positioned in the plot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScatterPoint {
    /// The series.
    pub series: usize,
    /// Index into the series points.
    pub index: usize,
    /// Screen position.
    pub pos: Pt,
}

/// What the pointer is over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hover {
    /// A category of a bar, line or area chart.
    Category(usize),
    /// The nearest real point of a scatter chart.
    Point {
        /// The series.
        series: usize,
        /// Index into the series points.
        index: usize,
    },
}

/// The hover decorations of lines and areas.
#[derive(Clone, Debug, PartialEq)]
pub struct HoverOverlay {
    /// The vertical rule: x, top, bottom.
    pub rule: Option<(f32, f32, f32)>,
    /// Dots: center and series.
    pub dots: Vec<(Pt, usize)>,
}

/// The measured plan of one chart.
#[derive(Clone, Debug)]
pub struct Plan {
    /// Measured bounds of the chart.
    pub bounds: Rect,
    /// The plot rectangle.
    pub plot: Rect,
    /// Whether anything paints (the plot is wider and taller than 4 px).
    pub paintable: bool,
    /// Kind.
    pub kind: ChartKind,
    /// Horizontal bars.
    pub horizontal: bool,
    /// Category count.
    pub n: usize,
    /// Value axis domain (the y domain of a scatter).
    pub value_domain: Domain,
    /// The x domain of a scatter chart.
    pub x_domain: Option<Domain>,
    /// Category label stride (1 when every label is drawn).
    pub stride: usize,
    /// Gridlines.
    pub gridlines: Vec<Gridline>,
    /// Axis labels.
    pub labels: Vec<Label>,
    /// Data marks, in paint order.
    pub marks: Vec<Mark>,
    /// Scatter points.
    pub points: Vec<ScatterPoint>,
    /// Left gutter width excluding the axis gap.
    pub gutter: f32,
}

/// `label_stride` (R8): `max(1, ceil((widest + 12) / max(band, 1)))`.
pub fn label_stride(widest_label: f32, band: f32) -> usize {
    (((widest_label + THINNING_PADDING) / band.max(1.0)).ceil() as usize).max(1)
}

/// The hovered category from the pointer's position along the category axis (R13): for bars
/// (or when `n <= 1`) `floor(along * n)`, for lines and areas `round(along * (n - 1))`; valid
/// only inside the plot along that axis.
pub fn hover_category(chart: &Chart, n: usize, plot: Rect, pointer: Pt) -> Option<usize> {
    if n == 0 || chart.kind == ChartKind::Scatter {
        return None;
    }
    let along = if chart.is_horizontal_bars() {
        (pointer.y - plot.y) / plot.h
    } else {
        (pointer.x - plot.x) / plot.w
    };
    if !along.is_finite() || !(0.0..=1.0).contains(&along) {
        return None;
    }
    let index = if chart.kind == ChartKind::Bar || n <= 1 {
        (along * n as f32).floor()
    } else {
        (along * (n - 1) as f32).round()
    };
    (index >= 0.0 && (index as usize) < n).then_some(index as usize)
}

/// The nearest scatter point within [`PICK_RADIUS`] of the pointer; the smallest distance wins
/// and ties go to the earlier series and point.
pub fn hover_point(points: &[ScatterPoint], pointer: Pt) -> Option<ScatterPoint> {
    let mut best: Option<(f32, ScatterPoint)> = None;
    for point in points {
        if !point.pos.is_finite() {
            continue;
        }
        let distance = (point.pos.x - pointer.x).hypot(point.pos.y - pointer.y);
        if distance < PICK_RADIUS && best.is_none_or(|(d, _)| distance < d) {
            best = Some((distance, *point));
        }
    }
    best.map(|(_, point)| point)
}

impl Plan {
    /// Builds the plan of `chart` in `bounds`. `scale` multiplies the 11/16 px text metrics
    /// (the interface size setting); `measure` returns the width of a tick label shaped at the
    /// scaled tick size.
    pub fn build(chart: &Chart, bounds: Rect, scale: f32, measure: &dyn Fn(&str) -> f32) -> Plan {
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        let tick_line = TICK_LINE * scale;
        let n = category_count(chart);
        let value_domain = value_domain(chart);
        let x_domain = (chart.kind == ChartKind::Scatter).then(|| x_domain(chart));
        let horizontal = chart.is_horizontal_bars();

        let value_labels: Vec<String> = value_domain
            .ticks
            .iter()
            .map(|tick| chart.format.format(*tick))
            .collect();
        let gutter = if horizontal {
            let widest = (0..n)
                .filter_map(|i| chart.x.get(i))
                .map(|label| measure(label))
                .fold(0.0f32, f32::max);
            widest.min(bounds.w * CATEGORY_GUTTER_SHARE)
        } else {
            value_labels
                .iter()
                .map(|label| measure(label))
                .fold(0.0f32, f32::max)
        };
        let left = gutter + AXIS_GAP;
        let top = tick_line / 2.0;
        let bottom = tick_line + LABEL_OFFSET;
        let right = if horizontal || chart.kind == ChartKind::Scatter {
            12.0
        } else {
            4.0
        };
        let plot = Rect::new(
            bounds.x + left,
            bounds.y + top,
            bounds.w - left - right,
            bounds.h - top - bottom,
        );
        let paintable = plot.w > MIN_PLOT && plot.h > MIN_PLOT;
        let mut plan = Plan {
            bounds,
            plot,
            paintable,
            kind: chart.kind,
            horizontal,
            n,
            value_domain,
            x_domain,
            stride: 1,
            gridlines: Vec::new(),
            labels: Vec::new(),
            marks: Vec::new(),
            points: Vec::new(),
            gutter,
        };
        if chart.kind == ChartKind::Scatter {
            plan.place_points(chart);
        }
        if !paintable {
            return plan;
        }
        plan.axes(chart, &value_labels, measure, gutter);
        match chart.kind {
            ChartKind::Bar => plan.bars(chart),
            ChartKind::Line | ChartKind::Area => plan.lines(chart),
            ChartKind::Scatter => {}
        }
        plan
    }

    /// Maps a value to its vertical coordinate (`y(v) = top + height * (1 - t(v))`).
    pub fn value_y(&self, value: f64) -> f32 {
        self.plot.y + self.plot.h * (1.0 - self.value_domain.t(value) as f32)
    }

    /// Maps a value to its horizontal coordinate (horizontal bars).
    pub fn value_x(&self, value: f64) -> f32 {
        self.plot.x + self.plot.w * self.value_domain.t(value) as f32
    }

    /// Maps a scatter x value to its horizontal coordinate.
    pub fn scatter_x(&self, value: f64) -> f32 {
        match &self.x_domain {
            Some(domain) => self.plot.x + self.plot.w * domain.t(value) as f32,
            None => self.plot.x,
        }
    }

    /// The x coordinate of category `i` on a line or area chart.
    pub fn category_x(&self, i: usize) -> f32 {
        if self.n <= 1 {
            self.plot.x + self.plot.w / 2.0
        } else {
            self.plot.x + self.plot.w * i as f32 / (self.n - 1) as f32
        }
    }

    /// The band of category `i` across the plot (the hover wash of bars).
    pub fn band_rect(&self, i: usize) -> Rect {
        if self.n == 0 {
            return Rect::default();
        }
        if self.horizontal {
            let band = self.plot.h / self.n as f32;
            Rect::new(
                self.plot.x,
                self.plot.y + band * i as f32,
                self.plot.w,
                band,
            )
        } else {
            let band = self.plot.w / self.n as f32;
            Rect::new(
                self.plot.x + band * i as f32,
                self.plot.y,
                band,
                self.plot.h,
            )
        }
    }

    /// Resolves the pointer to a hover target.
    pub fn hover(&self, chart: &Chart, pointer: Pt) -> Option<Hover> {
        if !pointer.is_finite() {
            return None;
        }
        if self.kind == ChartKind::Scatter {
            hover_point(&self.points, pointer).map(|point| Hover::Point {
                series: point.series,
                index: point.index,
            })
        } else {
            hover_category(chart, self.n, self.plot, pointer).map(Hover::Category)
        }
    }

    /// The hover rule and dots of a line or area chart at category `c`.
    pub fn hover_overlay(&self, chart: &Chart, category: usize) -> HoverOverlay {
        if !matches!(self.kind, ChartKind::Line | ChartKind::Area) || category >= self.n {
            return HoverOverlay {
                rule: None,
                dots: Vec::new(),
            };
        }
        let x = self.category_x(category);
        let dots = chart
            .series
            .iter()
            .enumerate()
            .filter_map(|(series, s)| {
                s.value_at(category)
                    .map(|value| (Pt::new(x, self.value_y(value)), series))
            })
            .collect();
        HoverOverlay {
            rule: Some((x, self.plot.y, self.plot.bottom())),
            dots,
        }
    }

    fn place_points(&mut self, chart: &Chart) {
        let plot = self.plot;
        let y_domain = self.value_domain.clone();
        let x_domain = self.x_domain.clone().unwrap_or_else(Domain::empty);
        for (series, s) in chart.series.iter().enumerate() {
            for (index, &(x, y)) in s.points.iter().enumerate() {
                if !x.is_finite() || !y.is_finite() {
                    continue;
                }
                let pos = Pt::new(
                    plot.x + plot.w * x_domain.t(x) as f32,
                    plot.y + plot.h * (1.0 - y_domain.t(y) as f32),
                );
                if pos.is_finite() {
                    self.points.push(ScatterPoint { series, index, pos });
                }
            }
        }
    }

    fn axes(
        &mut self,
        chart: &Chart,
        value_labels: &[String],
        measure: &dyn Fn(&str) -> f32,
        _gutter: f32,
    ) {
        let plot = self.plot;
        let label_top = plot.bottom() + LABEL_OFFSET;
        if self.horizontal {
            for (tick, text) in self
                .value_domain
                .ticks
                .clone()
                .into_iter()
                .zip(value_labels)
            {
                let x = self.value_x(tick);
                self.gridlines.push(Gridline {
                    from: Pt::new(x, plot.y),
                    to: Pt::new(x, plot.bottom()),
                });
                self.labels.push(Label {
                    text: text.clone(),
                    anchor: Pt::new(x, label_top),
                    h: HAlign::Center,
                    v: VAlign::Top,
                    max_width: None,
                });
            }
            if self.n > 0 {
                let band = plot.h / self.n as f32;
                for i in 0..self.n {
                    if let Some(text) = chart.x.get(i) {
                        self.labels.push(Label {
                            text: text.clone(),
                            anchor: Pt::new(plot.x - AXIS_GAP, plot.y + band * (i as f32 + 0.5)),
                            h: HAlign::Right,
                            v: VAlign::Center,
                            max_width: Some(self.gutter),
                        });
                    }
                }
            }
            return;
        }
        for (tick, text) in self
            .value_domain
            .ticks
            .clone()
            .into_iter()
            .zip(value_labels)
        {
            let y = self.value_y(tick);
            self.gridlines.push(Gridline {
                from: Pt::new(plot.x, y),
                to: Pt::new(plot.right(), y),
            });
            self.labels.push(Label {
                text: text.clone(),
                anchor: Pt::new(plot.x - AXIS_GAP, y),
                h: HAlign::Right,
                v: VAlign::Center,
                max_width: None,
            });
        }
        if chart.kind == ChartKind::Scatter {
            if let Some(domain) = self.x_domain.clone() {
                for tick in domain.ticks {
                    let x = self.scatter_x(tick);
                    self.gridlines.push(Gridline {
                        from: Pt::new(x, plot.y),
                        to: Pt::new(x, plot.bottom()),
                    });
                    self.labels.push(Label {
                        text: chart.x_format.format(tick),
                        anchor: Pt::new(x, label_top),
                        h: HAlign::Center,
                        v: VAlign::Top,
                        max_width: None,
                    });
                }
            }
            return;
        }
        if self.n == 0 {
            return;
        }
        let band = plot.w / self.n as f32;
        let widest = (0..self.n)
            .filter_map(|i| chart.x.get(i))
            .map(|label| measure(label))
            .fold(0.0f32, f32::max);
        self.stride = label_stride(widest, band);
        for i in (0..self.n).step_by(self.stride) {
            let Some(text) = chart.x.get(i) else { continue };
            let (x, h) = if chart.kind == ChartKind::Bar {
                (plot.x + band * (i as f32 + 0.5), HAlign::Center)
            } else {
                let x = self.category_x(i);
                if self.n > 1 && i == 0 {
                    (x, HAlign::Left)
                } else if self.n > 1 && i == self.n - 1 {
                    (x, HAlign::Right)
                } else {
                    (x, HAlign::Center)
                }
            };
            self.labels.push(Label {
                text: text.clone(),
                anchor: Pt::new(x, label_top),
                h,
                v: VAlign::Top,
                max_width: None,
            });
        }
    }

    fn bars(&mut self, chart: &Chart) {
        if self.n == 0 {
            return;
        }
        let plot = self.plot;
        let band = if self.horizontal { plot.h } else { plot.w } / self.n as f32;
        let inner = BAR_BAND_SHARE * band;
        let stacked = chart.is_stacked();
        let groups = if stacked {
            1
        } else {
            chart.series.len().max(1)
        };
        let bar = (inner / groups as f32).max(1.0);
        let gap = if groups > 1 && !stacked { 1.0 } else { 0.0 };
        let (lo, hi) = (self.value_domain.lo, self.value_domain.hi);
        let baseline = 0.0f64.clamp(lo, hi);
        for i in 0..self.n {
            let start = band * i as f32 + (band - inner) / 2.0;
            let (mut positive, mut negative) = (0.0f64, 0.0f64);
            for (s, series) in chart.series.iter().enumerate() {
                let Some(value) = series.value_at(i) else {
                    continue;
                };
                let offset = if stacked { 0.0 } else { bar * s as f32 };
                let thickness = (bar - gap).max(0.0);
                let (from, to) = if stacked {
                    if value >= 0.0 {
                        let from = positive;
                        positive += value;
                        (from, positive)
                    } else {
                        let from = negative;
                        negative += value;
                        (from, negative)
                    }
                } else {
                    (baseline, value)
                };
                let rect = if self.horizontal {
                    let (a, b) = (self.value_x(from), self.value_x(to));
                    Rect::new(a.min(b), plot.y + start + offset, (b - a).abs(), thickness)
                } else {
                    let (a, b) = (self.value_y(from), self.value_y(to));
                    Rect::new(plot.x + start + offset, a.min(b), thickness, (b - a).abs())
                };
                if rect.is_finite() {
                    self.marks.push(Mark::Bar {
                        rect,
                        series: s,
                        radius: if stacked { 0.0 } else { BAR_RADIUS },
                    });
                }
            }
        }
    }

    fn lines(&mut self, chart: &Chart) {
        let base_y = self.value_y(0.0f64.clamp(self.value_domain.lo, self.value_domain.hi));
        let area = chart.kind == ChartKind::Area;
        for (s, series) in chart.series.iter().enumerate() {
            let mut run: Vec<Pt> = Vec::new();
            let finish = |run: &mut Vec<Pt>, marks: &mut Vec<Mark>| {
                match run.len() {
                    0 => {}
                    1 => marks.push(Mark::Dot {
                        center: run[0],
                        radius: SINGLE_DOT_RADIUS,
                        series: s,
                    }),
                    _ => {
                        if area {
                            let mut polygon = run.clone();
                            polygon.push(Pt::new(run[run.len() - 1].x, base_y));
                            polygon.push(Pt::new(run[0].x, base_y));
                            marks.push(Mark::Area { polygon, series: s });
                        }
                        marks.push(Mark::Polyline {
                            points: run.clone(),
                            series: s,
                        });
                    }
                }
                run.clear();
            };
            for i in 0..self.n {
                match series.value_at(i) {
                    Some(value) => {
                        let point = Pt::new(self.category_x(i), self.value_y(value));
                        if point.is_finite() {
                            run.push(point);
                        }
                    }
                    None => finish(&mut run, &mut self.marks),
                }
            }
            finish(&mut run, &mut self.marks);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chart::model::Series;

    fn width(label: &str) -> f32 {
        label.chars().count() as f32 * 6.0
    }

    fn bounds() -> Rect {
        Rect::new(10.0, 20.0, 400.0, 220.0)
    }

    fn some(values: &[f64]) -> Vec<Option<f64>> {
        values.iter().map(|v| Some(*v)).collect()
    }

    #[test]
    fn a4_stacked_rectangles() {
        let chart = Chart::new()
            .stacked(true)
            .series(vec![
                Series::new("a").values(some(&[3.0, -2.0, 4.0])),
                Series::new("b").values(some(&[1.0, -1.0, 2.0])),
            ])
            .unwrap();
        let plan = Plan::build(&chart, bounds(), 1.0, &width);
        let bars: Vec<(&Rect, usize)> = plan
            .marks
            .iter()
            .filter_map(|mark| match mark {
                Mark::Bar {
                    rect,
                    series,
                    radius,
                } => {
                    assert_eq!(*radius, 0.0);
                    Some((rect, *series))
                }
                _ => None,
            })
            .collect();
        let extent = |rect: &Rect| {
            (
                plan.value_domain.lo
                    + (plan.plot.bottom() - rect.bottom()) as f64 / plan.plot.h as f64
                        * (plan.value_domain.hi - plan.value_domain.lo),
                plan.value_domain.lo
                    + (plan.plot.bottom() - rect.y) as f64 / plan.plot.h as f64
                        * (plan.value_domain.hi - plan.value_domain.lo),
            )
        };
        let close =
            |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3;
        // Category 0: 0..3 then 3..4; category 1: -2..0 then -3..-2.
        assert!(close(extent(bars[0].0), (0.0, 3.0)));
        assert!(close(extent(bars[1].0), (3.0, 4.0)));
        assert!(close(extent(bars[2].0), (-2.0, 0.0)));
        assert!(close(extent(bars[3].0), (-3.0, -2.0)));
        assert_eq!(bars[0].1, 0);
        assert_eq!(bars[1].1, 1);
    }

    #[test]
    fn a4_grouped_bars_start_at_the_zero_baseline() {
        let chart = Chart::new()
            .series(vec![
                Series::new("a").values(some(&[3.0, -2.0])),
                Series::new("b").values(some(&[1.0, 5.0])),
            ])
            .unwrap();
        let plan = Plan::build(&chart, bounds(), 1.0, &width);
        let zero = plan.value_y(0.0);
        for mark in &plan.marks {
            let Mark::Bar { rect, radius, .. } = mark else {
                panic!()
            };
            assert_eq!(*radius, 2.0);
            assert!((rect.y - zero).abs() < 1e-3 || (rect.bottom() - zero).abs() < 1e-3);
        }
        // Two groups: 1 px gap between grouped bars.
        let Mark::Bar { rect: first, .. } = &plan.marks[0] else {
            panic!()
        };
        let Mark::Bar { rect: second, .. } = &plan.marks[1] else {
            panic!()
        };
        assert!((second.x - first.x - (first.w + 1.0)).abs() < 1e-3);
    }

    #[test]
    fn a5_horizontal_mapping() {
        let chart = Chart::new()
            .horizontal(true)
            .y_max(100.0)
            .x(vec!["a".into(), "b".into(), "c".into(), "d".into()])
            .unwrap()
            .series(vec![
                Series::new("s").values(some(&[25.0, 50.0, 75.0, 100.0])),
            ])
            .unwrap();
        let mut plan = Plan::build(
            &chart,
            Rect::new(0.0, 0.0, 700.0, 300.0 + 28.0),
            1.0,
            &width,
        );
        // Force a known plot to check the formulas of the spec example.
        plan.plot = Rect::new(100.0, 0.0, 600.0, 300.0);
        plan.marks.clear();
        plan.bars(&chart);
        assert_eq!(plan.value_domain.hi, 100.0);
        let Mark::Bar { rect, .. } = &plan.marks[0] else {
            panic!()
        };
        assert!((rect.right() - 250.0).abs() < 1e-3);
        let Mark::Bar { rect, .. } = &plan.marks[1] else {
            panic!()
        };
        assert!((rect.y - 85.5).abs() < 1e-3);
        assert!((rect.h - 54.0).abs() < 1e-3);
        // Category labels are right-aligned at plotLeft - 8.
        let plan = Plan::build(&chart, Rect::new(0.0, 0.0, 700.0, 328.0), 1.0, &width);
        for label in plan.labels.iter().filter(|l| l.h == HAlign::Right) {
            assert_eq!(label.anchor.x, plan.plot.x - 8.0);
        }
    }

    #[test]
    fn a6_missing_data_runs() {
        let chart = |kind, values: Vec<Option<f64>>| {
            Chart::new()
                .kind(kind)
                .series(vec![Series::new("s").values(values)])
                .unwrap()
        };
        let gaps = chart(
            ChartKind::Line,
            vec![Some(1.0), None, Some(3.0), Some(f64::NAN), Some(5.0)],
        );
        let plan = Plan::build(&gaps, bounds(), 1.0, &width);
        let dots = plan
            .marks
            .iter()
            .filter(|m| matches!(m, Mark::Dot { .. }))
            .count();
        assert_eq!(dots, 3);
        assert!(
            !plan
                .marks
                .iter()
                .any(|m| matches!(m, Mark::Polyline { .. }))
        );
        let runs = chart(
            ChartKind::Area,
            vec![Some(1.0), Some(2.0), None, Some(4.0), Some(5.0)],
        );
        let plan = Plan::build(&runs, bounds(), 1.0, &width);
        let lines: Vec<_> = plan
            .marks
            .iter()
            .filter_map(|m| match m {
                Mark::Polyline { points, .. } => Some(points.len()),
                _ => None,
            })
            .collect();
        assert_eq!(lines, [2, 2]);
        assert_eq!(
            plan.marks
                .iter()
                .filter(|m| matches!(m, Mark::Area { .. }))
                .count(),
            2
        );
        // Single-point runs have no area fill.
        let single = Plan::build(
            &chart(ChartKind::Area, vec![Some(1.0), None, Some(2.0)]),
            bounds(),
            1.0,
            &width,
        );
        assert!(!single.marks.iter().any(|m| matches!(m, Mark::Area { .. })));
        // Bars skip missing slots.
        let bars = Plan::build(
            &chart(ChartKind::Bar, vec![Some(1.0), None, Some(3.0)]),
            bounds(),
            1.0,
            &width,
        );
        assert_eq!(bars.marks.len(), 2);
    }

    fn plot(width: f32) -> Rect {
        Rect::new(0.0, 0.0, width, 100.0)
    }

    #[test]
    fn a7_bar_and_line_hover() {
        let bars = Chart::new()
            .x((0..4).map(|i| i.to_string()).collect())
            .unwrap();
        for (x, expected) in [(0.0, 0), (99.0, 0), (100.0, 1), (199.0, 1), (399.0, 3)] {
            assert_eq!(
                hover_category(&bars, 4, plot(400.0), Pt::new(x, 10.0)),
                Some(expected)
            );
        }
        assert_eq!(
            hover_category(&bars, 4, plot(400.0), Pt::new(401.0, 0.0)),
            None
        );
        assert_eq!(
            hover_category(&bars, 4, plot(400.0), Pt::new(-1.0, 0.0)),
            None
        );
        let line = Chart::new().kind(ChartKind::Line);
        // n = 5 over 400 px: vertices at 0, 100, ... 400; offsets round to the nearest.
        for (x, expected) in [
            (0.0, 0),
            (49.0, 0),
            (51.0, 1),
            (149.0, 1),
            (151.0, 2),
            (400.0, 4),
        ] {
            assert_eq!(
                hover_category(&line, 5, plot(400.0), Pt::new(x, 5.0)),
                Some(expected)
            );
        }
        assert_eq!(
            hover_category(&line, 5, plot(400.0), Pt::new(400.5, 5.0)),
            None
        );
        assert_eq!(
            hover_category(&line, 5, plot(400.0), Pt::new(-0.5, 5.0)),
            None
        );
        // Horizontal bars use the y axis.
        let horizontal = Chart::new().horizontal(true);
        let area = Rect::new(0.0, 0.0, 100.0, 400.0);
        assert_eq!(
            hover_category(&horizontal, 4, area, Pt::new(5.0, 250.0)),
            Some(2)
        );
        assert_eq!(
            hover_category(&horizontal, 0, area, Pt::new(5.0, 250.0)),
            None
        );
    }

    #[test]
    fn a7_scatter_pick() {
        let point = |series, index, x, y| ScatterPoint {
            series,
            index,
            pos: Pt::new(x, y),
        };
        let points = [point(0, 0, 100.0, 100.0), point(1, 0, 100.0, 108.0)];
        let picked = hover_point(&points, Pt::new(100.0, 111.0)).unwrap();
        assert_eq!((picked.series, picked.index), (1, 0));
        assert!(hover_point(&points[..1], Pt::new(100.0, 113.0)).is_none());
        assert!(hover_point(&points[..1], Pt::new(100.0, 111.0)).is_some());
        // The nearer of two candidates wins; a tie goes to the earlier one.
        let picked = hover_point(&points, Pt::new(100.0, 103.0)).unwrap();
        assert_eq!(picked.series, 0);
        let tie = [point(0, 0, 90.0, 100.0), point(1, 0, 110.0, 100.0)];
        assert_eq!(hover_point(&tie, Pt::new(100.0, 100.0)).unwrap().series, 0);
        let bad = [point(0, 0, f32::NAN, 0.0)];
        assert!(hover_point(&bad, Pt::new(0.0, 0.0)).is_none());
    }

    #[test]
    fn a8_layout_insets_and_stride() {
        let chart = Chart::new()
            .series(vec![Series::new("a").values(some(&[5.0, 50.0, 100.0]))])
            .unwrap();
        // Tick labels measured 20 and 31 px wide: the left inset is 31 + 8 = 39.
        let measure = |text: &str| if text.len() > 2 { 31.0 } else { 20.0 };
        let plan = Plan::build(&chart, Rect::new(0.0, 0.0, 300.0, 200.0), 1.0, &measure);
        assert_eq!(plan.plot.x, 39.0);
        assert_eq!(plan.plot.y, 8.0);
        assert_eq!(plan.plot.bottom(), 200.0 - 20.0);
        assert_eq!(plan.plot.right(), 296.0);
        // Horizontal bars cap the gutter at 35% of the chart width.
        let horizontal = Chart::new()
            .horizontal(true)
            .x(vec!["a very long category label".into()])
            .unwrap()
            .series(vec![Series::new("s").values(some(&[1.0]))])
            .unwrap();
        let plan = Plan::build(&horizontal, Rect::new(0.0, 0.0, 300.0, 200.0), 1.0, &|_| {
            500.0
        });
        assert_eq!(plan.plot.x, 105.0 + 8.0);
        assert_eq!(plan.plot.right(), 288.0);
        // 12 labels of 40 px in a 300 px plot: stride ceil(52 / 25) = 3.
        assert_eq!(label_stride(40.0, 25.0), 3);
        let labels: Vec<String> = (0..12).map(|i| format!("l{i}")).collect();
        let chart = Chart::new()
            .x(labels)
            .unwrap()
            .series(vec![Series::new("a").values(some(&[1.0; 12]))])
            .unwrap();
        // Make the gutter zero-width so the plot is 300 wide: bounds 300 + 8 + 4.
        let plan = Plan::build(&chart, Rect::new(0.0, 0.0, 312.0, 200.0), 1.0, &|text| {
            if text.starts_with('l') { 40.0 } else { 0.0 }
        });
        assert_eq!(plan.plot.w, 300.0);
        assert_eq!(plan.stride, 3);
        let drawn = plan
            .labels
            .iter()
            .filter(|l| l.text.starts_with('l'))
            .count();
        assert_eq!(drawn, 4);
    }

    #[test]
    fn a8_tiny_plot_paints_nothing_but_records_hover_geometry() {
        let chart = Chart::new()
            .kind(ChartKind::Scatter)
            .series(vec![Series::new("p").points(vec![(1.0, 1.0), (2.0, 2.0)])])
            .unwrap();
        let plan = Plan::build(&chart, Rect::new(0.0, 0.0, 30.0, 30.0), 1.0, &|_| 0.0);
        assert!(!plan.paintable);
        assert!(plan.marks.is_empty() && plan.gridlines.is_empty() && plan.labels.is_empty());
        assert_eq!(plan.points.len(), 2);
        assert_eq!(plan.n, 0);
    }

    #[test]
    fn a11_degenerate_inputs_stay_finite() {
        let cases = [
            Chart::new(),
            Chart::new().series(vec![Series::new("a")]).unwrap(),
            Chart::new()
                .series(vec![Series::new("a").values(some(&[5.0, 5.0, 5.0]))])
                .unwrap(),
            Chart::new()
                .kind(ChartKind::Line)
                .series(vec![Series::new("a").values(some(&[7.0]))])
                .unwrap(),
            Chart::new()
                .kind(ChartKind::Area)
                .series(vec![
                    Series::new("a").values(some(&[f64::INFINITY, f64::NAN])),
                ])
                .unwrap(),
            Chart::new()
                .kind(ChartKind::Scatter)
                .series(vec![Series::new("p").points(vec![(3.0, 3.0)])])
                .unwrap(),
            Chart::new()
                .kind(ChartKind::Scatter)
                .series(vec![
                    Series::new("p").points(vec![(f64::NAN, 1.0), (1.0, f64::INFINITY)]),
                ])
                .unwrap(),
            Chart::new()
                .horizontal(true)
                .series(vec![Series::new("a").values(some(&[1.0]))])
                .unwrap(),
        ];
        for chart in cases {
            let plan = Plan::build(&chart, bounds(), 1.0, &width);
            assert!(plan.plot.is_finite());
            for line in &plan.gridlines {
                assert!(line.from.is_finite() && line.to.is_finite());
            }
            for label in &plan.labels {
                assert!(label.anchor.is_finite());
            }
            for mark in &plan.marks {
                match mark {
                    Mark::Bar { rect, .. } => assert!(rect.is_finite()),
                    Mark::Polyline { points, .. } => assert!(points.iter().all(|p| p.is_finite())),
                    Mark::Area { polygon, .. } => assert!(polygon.iter().all(|p| p.is_finite())),
                    Mark::Dot { center, radius, .. } => {
                        assert!(center.is_finite() && radius.is_finite())
                    }
                }
            }
            assert!(plan.points.iter().all(|p| p.pos.is_finite()));
        }
    }

    #[test]
    fn line_labels_align_first_start_and_last_end() {
        let chart = Chart::new()
            .kind(ChartKind::Line)
            .x(vec!["a".into(), "b".into(), "c".into()])
            .unwrap()
            .series(vec![Series::new("s").values(some(&[1.0, 2.0, 3.0]))])
            .unwrap();
        let plan = Plan::build(&chart, bounds(), 1.0, &|_| 6.0);
        let cat: Vec<&Label> = plan.labels.iter().filter(|l| l.v == VAlign::Top).collect();
        assert_eq!(cat[0].h, HAlign::Left);
        assert_eq!(cat[0].anchor.x, plan.plot.x);
        assert_eq!(cat[1].h, HAlign::Center);
        assert_eq!(cat[2].h, HAlign::Right);
        assert_eq!(cat[2].anchor.x, plan.plot.right());
        assert_eq!(cat[0].anchor.y, plan.plot.bottom() + 4.0);
    }

    #[test]
    fn hover_overlay_marks_present_series_only() {
        let chart = Chart::new()
            .kind(ChartKind::Line)
            .series(vec![
                Series::new("a").values(vec![Some(1.0), Some(2.0)]),
                Series::new("b").values(vec![Some(3.0), None]),
            ])
            .unwrap();
        let plan = Plan::build(&chart, bounds(), 1.0, &width);
        let overlay = plan.hover_overlay(&chart, 1);
        assert_eq!(overlay.dots.len(), 1);
        assert_eq!(overlay.dots[0].1, 0);
        let (x, top, bottom) = overlay.rule.unwrap();
        assert_eq!(x, plan.plot.right());
        assert_eq!((top, bottom), (plan.plot.y, plan.plot.bottom()));
        assert_eq!(plan.hover_overlay(&chart, 9).rule, None);
    }
}
