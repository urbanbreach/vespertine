//! Axis ranges: nice ticks and fitted domains (CMP-024 R3-R5).

use super::model::{Chart, ChartKind};

/// The target tick count of every axis.
pub const TICK_COUNT: usize = 4;
/// The most ticks one axis carries.
pub const MAX_TICKS: usize = 100;

/// A fitted axis domain. `hi > lo` always holds, so coordinate mapping never divides by zero.
#[derive(Clone, Debug, PartialEq)]
pub struct Domain {
    /// Lower bound.
    pub lo: f64,
    /// Upper bound.
    pub hi: f64,
    /// Tick values, ascending.
    pub ticks: Vec<f64>,
}

impl Domain {
    /// The default domain of empty data: `[0, 1]` with ticks `[0, 1]`.
    pub fn empty() -> Self {
        Self {
            lo: 0.0,
            hi: 1.0,
            ticks: vec![0.0, 1.0],
        }
    }

    /// Maps `value` to `[0, 1]` along the domain (`t(v) = (v - lo) / (hi - lo)`); not clamped.
    pub fn t(&self, value: f64) -> f64 {
        (value - self.lo) / (self.hi - self.lo)
    }
}

/// `niceTicks(lo, hi, count)` (R5): the "nice" step (1, 2, 5 or 10 times a power of ten)
/// nearest `(hi - lo) / count`, ticks at its multiples inside `[lo, hi]`, snapped to the step's
/// precision so no floating-point noise remains. Bounds are swapped when reversed; non-finite
/// bounds give no ticks; equal bounds give one tick.
pub fn nice_ticks(lo: f64, hi: f64, count: usize) -> Vec<f64> {
    if !lo.is_finite() || !hi.is_finite() {
        return Vec::new();
    }
    let (lo, hi) = if hi < lo { (hi, lo) } else { (lo, hi) };
    let span = hi - lo;
    if span.abs() < f64::EPSILON {
        return vec![lo];
    }
    let raw = span / count.max(1) as f64;
    let magnitude = 10f64.powf(raw.log10().floor());
    let normalized = raw / magnitude;
    let factor = [(1.5, 1.0), (3.0, 2.0), (7.0, 5.0)]
        .into_iter()
        .find(|&(limit, _)| normalized < limit)
        .map_or(10.0, |(_, factor)| factor);
    let step = magnitude * factor;
    let digits = 10f64.powi((-step.log10().floor()).max(0.0) as i32);
    let start = (lo / step).ceil() * step;
    let mut ticks = Vec::new();
    let mut index = 0u32;
    loop {
        let value = start + f64::from(index) * step;
        if value > hi + step * 1e-9 || ticks.len() >= MAX_TICKS {
            break;
        }
        ticks.push(((value / step).round() * step * digits).round() / digits);
        index += 1;
    }
    ticks
}

/// The domain procedure (R4). `include_zero` is true for every non-scatter kind.
pub fn domain(lo: f64, hi: f64, include_zero: bool) -> Domain {
    let (mut lo, mut hi) = if hi < lo { (hi, lo) } else { (lo, hi) };
    if include_zero && lo.is_finite() && hi.is_finite() {
        lo = lo.min(0.0);
        hi = hi.max(0.0);
    }
    if !lo.is_finite() || !hi.is_finite() {
        return Domain::empty();
    }
    if (hi - lo).abs() < f64::EPSILON {
        hi = lo + lo.abs().max(1.0);
    }
    let first = nice_ticks(lo, hi, TICK_COUNT);
    let step = if first.len() >= 2 {
        first[1] - first[0]
    } else {
        (hi - lo).max(1.0)
    };
    let fitted_lo = (lo / step).floor() * step;
    let mut fitted_hi = (hi / step).ceil() * step;
    if fitted_hi <= fitted_lo {
        fitted_hi = fitted_lo + step;
    }
    Domain {
        lo: fitted_lo,
        hi: fitted_hi,
        ticks: nice_ticks(fitted_lo, fitted_hi, TICK_COUNT),
    }
}

/// Category count `n = max(longest series value count, label count)`.
pub fn category_count(chart: &Chart) -> usize {
    chart
        .series
        .iter()
        .map(|series| series.values.len())
        .max()
        .unwrap_or(0)
        .max(chart.x.len())
}

/// The minimum and maximum of the value axis before fitting (R3): over all present values, or
/// over the per-category positive and negative totals when stacked (both including 0), or over
/// the y of all finite points for scatter. `y_min` and `y_max` replace the computed bounds
/// independently and the pair is swapped when reversed. A bound with no data is infinite, which
/// [`domain`] turns into the default domain.
pub fn value_range(chart: &Chart) -> (f64, f64) {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    if chart.kind == ChartKind::Scatter {
        for series in &chart.series {
            for &(x, y) in &series.points {
                if x.is_finite() && y.is_finite() {
                    lo = lo.min(y);
                    hi = hi.max(y);
                }
            }
        }
    } else if chart.stacked && chart.kind == ChartKind::Bar {
        for category in 0..category_count(chart) {
            let (mut positive, mut negative) = (0.0f64, 0.0f64);
            for series in &chart.series {
                if let Some(value) = series.value_at(category) {
                    if value >= 0.0 {
                        positive += value;
                    } else {
                        negative += value;
                    }
                }
            }
            lo = lo.min(negative);
            hi = hi.max(positive);
        }
    } else {
        for series in &chart.series {
            for value in series.values.iter().flatten().filter(|v| v.is_finite()) {
                lo = lo.min(*value);
                hi = hi.max(*value);
            }
        }
    }
    if let Some(y_min) = chart.y_min.filter(|v| !v.is_nan()) {
        lo = y_min;
    }
    if let Some(y_max) = chart.y_max.filter(|v| !v.is_nan()) {
        hi = y_max;
    }
    if lo > hi && lo.is_finite() && hi.is_finite() {
        std::mem::swap(&mut lo, &mut hi);
    }
    (lo, hi)
}

/// The fitted value-axis domain of a chart (R3, R4).
pub fn value_domain(chart: &Chart) -> Domain {
    let (lo, hi) = value_range(chart);
    domain(lo, hi, chart.kind != ChartKind::Scatter)
}

/// The fitted x-axis domain of a scatter chart: `domain(minX, maxX, false)` over the finite
/// points.
pub fn x_domain(chart: &Chart) -> Domain {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for series in &chart.series {
        for &(x, y) in &series.points {
            if x.is_finite() && y.is_finite() {
                lo = lo.min(x);
                hi = hi.max(x);
            }
        }
    }
    domain(lo, hi, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chart::model::Series;

    fn bars(series: Vec<Series>) -> Chart {
        Chart::new().series(series).unwrap()
    }

    #[test]
    fn a2_nice_ticks() {
        assert_eq!(
            nice_ticks(0.0, 100.0, 4),
            [0.0, 20.0, 40.0, 60.0, 80.0, 100.0]
        );
        assert_eq!(nice_ticks(0.0, 1.0, 4), [0.0, 0.2, 0.4, 0.6, 0.8, 1.0]);
        assert_eq!(nice_ticks(3.0, 3.0, 4), [3.0]);
        assert!(nice_ticks(f64::NAN, 1.0, 4).is_empty());
        assert!(nice_ticks(0.0, f64::INFINITY, 4).is_empty());
        assert_eq!(nice_ticks(100.0, 0.0, 4), nice_ticks(0.0, 100.0, 4));
    }

    #[test]
    fn a3_domains() {
        let empty = domain(f64::INFINITY, f64::NEG_INFINITY, true);
        assert_eq!((empty.lo, empty.hi), (0.0, 1.0));
        assert_eq!(empty.ticks, [0.0, 1.0]);
        let constant = value_domain(&bars(vec![Series::new("a").values(vec![
            Some(5.0),
            Some(5.0),
            Some(5.0),
        ])]));
        assert_eq!((constant.lo, constant.hi), (0.0, 5.0));
        assert_eq!(constant.ticks, [0.0, 1.0, 2.0, 3.0, 4.0, 5.0]);
        // Scatter y constant 5: hi becomes 10 before fitting, then the domain is [5, 10].
        let scatter = Chart::new()
            .kind(ChartKind::Scatter)
            .series(vec![Series::new("p").points(vec![(1.0, 5.0), (2.0, 5.0)])])
            .unwrap();
        let fitted = value_domain(&scatter);
        assert_eq!((fitted.lo, fitted.hi), (5.0, 10.0));
        let spread = value_domain(&bars(vec![
            Series::new("a").values(vec![Some(3.0), Some(97.0)]),
        ]));
        assert_eq!((spread.lo, spread.hi), (0.0, 100.0));
        assert_eq!(spread.ticks, [0.0, 20.0, 40.0, 60.0, 80.0, 100.0]);
        let capped = Chart::new()
            .y_max(50.0)
            .series(vec![Series::new("a").values(vec![Some(3.0), Some(97.0)])])
            .unwrap();
        let capped = value_domain(&capped);
        assert_eq!((capped.lo, capped.hi), (0.0, 50.0));
        assert_eq!(capped.ticks, [0.0, 10.0, 20.0, 30.0, 40.0, 50.0]);
    }

    #[test]
    fn swapped_bounds_and_missing_samples() {
        let chart = Chart::new()
            .y_min(10.0)
            .y_max(-5.0)
            .series(vec![Series::new("a").values(vec![
                Some(1.0),
                None,
                Some(f64::NAN),
            ])])
            .unwrap();
        assert_eq!(value_range(&chart), (-5.0, 10.0));
        let empty = value_domain(&Chart::new());
        assert_eq!((empty.lo, empty.hi), (0.0, 1.0));
    }

    #[test]
    fn a4_stacked_totals() {
        let chart = Chart::new()
            .stacked(true)
            .series(vec![
                Series::new("a").values(vec![Some(3.0), Some(-2.0), Some(4.0)]),
                Series::new("b").values(vec![Some(1.0), Some(-1.0), Some(2.0)]),
            ])
            .unwrap();
        assert_eq!(value_range(&chart), (-3.0, 6.0));
    }

    #[test]
    fn single_point_and_tiny_ranges_stay_finite() {
        for (lo, hi) in [(5.0, 5.0), (0.0, 0.0), (1e-300, 1e-300), (0.5, 0.5 + 1e-9)] {
            let fitted = domain(lo, hi, false);
            assert!(fitted.hi > fitted.lo, "{lo} {hi}");
            assert!(fitted.lo.is_finite() && fitted.hi.is_finite());
        }
    }
}
