//! Interactive two-dimensional charts (CMP-024).
//!
//! A [`Chart`] descriptor (bars, horizontal bars, lines, areas and scatter plots with named
//! series, category and numeric axes) is shown by a [`ChartView`] on the framework's canvas.
//! Axis fitting ([`scale`]), the measured layout [`Plan`] (plot rectangle, ticks, label
//! stride, marks) and hover hit testing are pure and share one plot rectangle; the view builds
//! the plan in prepaint and paints from it. Colors come from theme roles and the replaceable
//! [`SeriesPalette`]. Hover reports the hovered category or nearest scatter point in a
//! reserved readout row, so it never changes the layout.

mod format;
mod model;
mod palette;
mod plan;
pub mod scale;
mod view;

pub use format::NumberFormat;
pub use model::{Chart, ChartError, ChartKind, Series};
pub use palette::{SeriesColor, SeriesPalette, ThemeToken};
pub use plan::{
    Gridline, HAlign, Hover, HoverOverlay, Label, Mark, Plan, Pt, Rect, ScatterPoint, VAlign,
    hover_category, hover_point, label_stride,
};
pub use scale::{Domain, category_count, domain, nice_ticks, value_domain, value_range, x_domain};
pub use view::{
    ChartColors, ChartElement, ChartView, LegendEntry, READOUT_HEIGHT, Readout, readout,
};
