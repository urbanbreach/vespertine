//! The chart descriptor (CMP-024 R1, R2).

use super::format::NumberFormat;
use super::palette::SeriesColor;

/// Default chart height in logical pixels.
pub const DEFAULT_HEIGHT: f32 = 220.0;
/// Smallest chart height.
pub const MIN_HEIGHT: f32 = 80.0;
/// Largest chart height.
pub const MAX_HEIGHT: f32 = 600.0;
/// Most values plus points one series may carry.
pub const MAX_SAMPLES_PER_SERIES: usize = 20_000;
/// Most categories of a non-scatter chart.
pub const MAX_CATEGORIES: usize = 2_000;

/// What a chart draws.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ChartKind {
    /// Grouped or stacked bars (vertical, or horizontal with [`Chart::horizontal`]).
    #[default]
    Bar,
    /// Polylines.
    Line,
    /// Polylines with filled areas down to the baseline.
    Area,
    /// Points on numeric x and y axes.
    Scatter,
}

/// A descriptor exceeded a validation limit.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ChartError {
    /// A series carries more than [`MAX_SAMPLES_PER_SERIES`] values plus points.
    #[error("series {series} carries {count} samples; the limit is {MAX_SAMPLES_PER_SERIES}")]
    TooManySamples {
        /// Index of the series.
        series: usize,
        /// Its sample count.
        count: usize,
    },
    /// A non-scatter chart has more than [`MAX_CATEGORIES`] categories.
    #[error("the chart has {count} categories; the limit is {MAX_CATEGORIES}")]
    TooManyCategories {
        /// The category count.
        count: usize,
    },
}

/// One named data series.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Series {
    /// Legend name (may be empty).
    pub name: String,
    /// Explicit color; the palette color at the series index (modulo 6) when absent.
    pub color: Option<SeriesColor>,
    /// One optional number per category (bar, line, area); `None` is a gap.
    pub values: Vec<Option<f64>>,
    /// Points of a scatter series.
    pub points: Vec<(f64, f64)>,
}

impl Series {
    /// A series with a name and no data.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }

    /// Sets the name.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Sets an explicit color (an sRGB value, a theme token or a token mix).
    pub fn color(mut self, color: impl Into<SeriesColor>) -> Self {
        self.color = Some(color.into());
        self
    }

    /// Sets the per-category values.
    pub fn values(mut self, values: Vec<Option<f64>>) -> Self {
        self.values = values;
        self
    }

    /// Sets the scatter points.
    pub fn points(mut self, points: Vec<(f64, f64)>) -> Self {
        self.points = points;
        self
    }

    /// The present (finite) value of category `index`, or `None` for a gap, a non-finite
    /// sample or an index beyond the values (R2).
    pub fn value_at(&self, index: usize) -> Option<f64> {
        self.values
            .get(index)
            .copied()
            .flatten()
            .filter(|value| value.is_finite())
    }
}

/// A chart descriptor.
#[derive(Clone, Debug, PartialEq)]
pub struct Chart {
    /// The kind.
    pub kind: ChartKind,
    /// Plot height in logical pixels, clamped to `[80, 600]`.
    pub height: f32,
    /// Category labels (non-scatter kinds).
    pub x: Vec<String>,
    /// The series.
    pub series: Vec<Series>,
    /// Stack bars per category.
    pub stacked: bool,
    /// Lay bars out horizontally.
    pub horizontal: bool,
    /// Replaces the computed lower bound of the value axis.
    pub y_min: Option<f64>,
    /// Replaces the computed upper bound of the value axis.
    pub y_max: Option<f64>,
    /// Format of values: value-axis ticks, readouts and the y part of scatter readouts.
    pub format: NumberFormat,
    /// Format of scatter x values (no prefix).
    pub x_format: NumberFormat,
    /// Whether the legend is shown.
    pub legend: bool,
}

impl Default for Chart {
    fn default() -> Self {
        Self {
            kind: ChartKind::Bar,
            height: DEFAULT_HEIGHT,
            x: Vec::new(),
            series: Vec::new(),
            stacked: false,
            horizontal: false,
            y_min: None,
            y_max: None,
            format: NumberFormat::default(),
            x_format: NumberFormat::default(),
            legend: true,
        }
    }
}

impl Chart {
    /// An empty bar chart. An empty chart renders its axes with the default domain.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the kind.
    pub fn kind(mut self, kind: ChartKind) -> Self {
        self.kind = kind;
        self
    }

    /// Sets the plot height; clamped to `[80, 600]` (a non-finite height is the default).
    pub fn height(mut self, height: f32) -> Self {
        self.height = if height.is_finite() {
            height.clamp(MIN_HEIGHT, MAX_HEIGHT)
        } else {
            DEFAULT_HEIGHT
        };
        self
    }

    /// Sets the category labels; fails when the chart would exceed [`MAX_CATEGORIES`].
    pub fn x(mut self, labels: Vec<String>) -> Result<Self, ChartError> {
        self.x = labels;
        self.validate()?;
        Ok(self)
    }

    /// Sets the series; fails when a limit is exceeded.
    pub fn series(mut self, series: Vec<Series>) -> Result<Self, ChartError> {
        self.series = series;
        self.validate()?;
        Ok(self)
    }

    /// Stacks bars.
    pub fn stacked(mut self, stacked: bool) -> Self {
        self.stacked = stacked;
        self
    }

    /// Lays bars out horizontally.
    pub fn horizontal(mut self, horizontal: bool) -> Self {
        self.horizontal = horizontal;
        self
    }

    /// Replaces the computed lower bound of the value axis.
    pub fn y_min(mut self, y_min: f64) -> Self {
        self.y_min = Some(y_min);
        self
    }

    /// Replaces the computed upper bound of the value axis.
    pub fn y_max(mut self, y_max: f64) -> Self {
        self.y_max = Some(y_max);
        self
    }

    /// Sets the value format.
    pub fn format(mut self, format: NumberFormat) -> Self {
        self.format = format;
        self
    }

    /// Sets the scatter x format (its prefix is ignored).
    pub fn x_format(mut self, mut format: NumberFormat) -> Self {
        format.prefix.clear();
        self.x_format = format;
        self
    }

    /// Shows or hides the legend.
    pub fn legend(mut self, legend: bool) -> Self {
        self.legend = legend;
        self
    }

    /// Checks the limits of R1: at most 20,000 values plus points per series and at most 2,000
    /// categories for non-scatter kinds.
    pub fn validate(&self) -> Result<(), ChartError> {
        for (index, series) in self.series.iter().enumerate() {
            let count = series.values.len() + series.points.len();
            if count > MAX_SAMPLES_PER_SERIES {
                return Err(ChartError::TooManySamples {
                    series: index,
                    count,
                });
            }
        }
        if self.kind != ChartKind::Scatter {
            let count = super::scale::category_count(self);
            if count > MAX_CATEGORIES {
                return Err(ChartError::TooManyCategories { count });
            }
        }
        Ok(())
    }

    /// Whether the plot is a horizontal bar plot.
    pub fn is_horizontal_bars(&self) -> bool {
        self.kind == ChartKind::Bar && self.horizontal
    }

    /// Whether bars stack (bars only).
    pub fn is_stacked(&self) -> bool {
        self.kind == ChartKind::Bar && self.stacked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_are_enforced_by_builders() {
        let big = Series::new("a").values(vec![Some(1.0); MAX_SAMPLES_PER_SERIES + 1]);
        assert!(matches!(
            Chart::new().kind(ChartKind::Line).series(vec![big]),
            Err(ChartError::TooManySamples { series: 0, .. })
        ));
        let many = Series::new("a").values(vec![Some(1.0); MAX_CATEGORIES + 1]);
        assert!(matches!(
            Chart::new().series(vec![many]),
            Err(ChartError::TooManyCategories { .. })
        ));
        let labels = vec![String::new(); MAX_CATEGORIES + 1];
        assert!(Chart::new().x(labels.clone()).is_err());
        // Scatter charts have no category limit.
        let points = Series::new("p").points(vec![(0.0, 0.0); 3000]);
        assert!(
            Chart::new()
                .kind(ChartKind::Scatter)
                .x(labels)
                .and_then(|chart| chart.series(vec![points]))
                .is_ok()
        );
    }

    #[test]
    fn height_clamps_and_non_finite_falls_back() {
        assert_eq!(Chart::new().height(10.0).height, 80.0);
        assert_eq!(Chart::new().height(9999.0).height, 600.0);
        assert_eq!(Chart::new().height(f32::NAN).height, 220.0);
    }
}
