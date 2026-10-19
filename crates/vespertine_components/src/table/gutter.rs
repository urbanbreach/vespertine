//! The graph gutter of a table: lane model, responsive fit, compact rail, stabilization and
//! morph (CMP-019 R8), and the canvas that paints caller-defined marks aligned with the rows
//! of the list (R8, R9).

use vespertine::{
    App, Bounds, ContentMask, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, ListState, Pixels, Style, Window, point, px, relative, size,
};

/// Distance between neighbouring lane centres.
pub const LANE_SPACING: f32 = 12.0;
/// Radius of a node.
pub const NODE_RADIUS: f32 = 3.0;
/// Padding of the ring around the head marker.
pub const HEAD_RING_PADDING: f32 = 2.0;
/// Stroke width of an edge.
pub const STROKE: f32 = 1.5;
/// Space left of the first lane's node.
pub const SIDE_PADDING: f32 = 13.0;
/// Space right of the last lane's node.
pub const TRAILING_PADDING: f32 = 12.0;
/// Natural widths at most this wide are used as they are.
pub const MIN_COMPACT_WIDTH: f32 = 48.0;
/// The widest share of the container the gutter may take.
pub const MAX_WIDTH_RATIO: f32 = 0.34;
/// Fitted widths snap down to a multiple of this.
pub const RESIZE_STEP: f32 = 2.0;
/// The primary column width below which the gutter turns into a compact rail.
pub const COMPACT_ENTER_PRIMARY: f32 = 160.0;
/// The primary column width from which the gutter leaves the compact rail.
pub const COMPACT_EXIT_PRIMARY: f32 = 184.0;
/// The lane spacing below which the gutter turns into a compact rail.
pub const COMPACT_ENTER_SPACING: f32 = 4.0;
/// The lane spacing from which the gutter leaves the compact rail.
pub const COMPACT_EXIT_SPACING: f32 = 5.0;
/// How far marks that cross a row boundary overlap each neighbor row.
pub const ROW_OVERLAP: f32 = 0.75;
/// The hit radius of a node.
pub const HIT_RADIUS: f32 = 5.5;
/// The stroke of the focused path.
pub const FOCUSED_STROKE: f32 = 2.25;
/// The opacity of paths other than the focused one while a path is focused.
pub const UNFOCUSED_PATH_OPACITY: f32 = 0.24;
/// The opacity of rows not on the focused path while a path is focused.
pub const UNFOCUSED_ROW_OPACITY: f32 = 0.6;
/// The primary column's minimum width (used by the budgets).
pub const PRIMARY_MIN: f32 = 80.0;

/// The width of the gutter that holds no lane spacing: padding on both sides and one node.
pub fn fixed_width() -> f32 {
    SIDE_PADDING + TRAILING_PADDING + 2.0 * NODE_RADIUS
}

/// The natural width of a gutter with `lanes` lanes.
pub fn natural_width(lanes: usize) -> f32 {
    fixed_width() + lanes.saturating_sub(1) as f32 * LANE_SPACING
}

/// The geometry of a gutter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GutterGeometry {
    /// The number of lanes.
    pub lanes: usize,
    /// The gutter width.
    pub width: f32,
    /// The distance between lane centres (0 on the compact rail).
    pub lane_spacing: f32,
    /// Whether all lanes share one x (the compact rail).
    pub compact: bool,
}

impl GutterGeometry {
    /// The natural geometry of `lanes` lanes.
    pub fn natural(lanes: usize) -> Self {
        Self {
            lanes,
            width: natural_width(lanes),
            lane_spacing: LANE_SPACING,
            compact: false,
        }
    }

    /// The compact rail of `lanes` lanes: one x, spacing 0, the single-lane natural width.
    pub fn compact_rail(lanes: usize) -> Self {
        Self {
            lanes,
            width: natural_width(1),
            lane_spacing: 0.0,
            compact: true,
        }
    }

    fn fitted(lanes: usize, width: f32) -> Self {
        let natural = natural_width(lanes);
        if lanes <= 1 || width >= natural {
            return Self::natural(lanes);
        }
        let width = width.clamp(fixed_width(), natural);
        Self {
            lanes,
            width,
            lane_spacing: (width - fixed_width()) / (lanes - 1) as f32,
            compact: false,
        }
    }

    /// The x of `lane`'s centre (the first lane is `SIDE_PADDING + NODE_RADIUS` from the left),
    /// rounded to the device pixel grid: `round(x * scale) / scale`.
    pub fn lane_x(&self, lane: usize, scale: f32) -> f32 {
        let spacing = if self.compact { 0.0 } else { self.lane_spacing };
        let x =
            SIDE_PADDING + NODE_RADIUS + lane.min(self.lanes.saturating_sub(1)) as f32 * spacing;
        (x * scale).round() / scale
    }
}

/// The width budget of a gutter of `lanes` lanes in a table `container` pixels wide whose
/// optional columns take `optional_columns` pixels, before snapping:
/// `min(natural, max(48, container - optional - 80), max(48, container x 0.34))`; a natural
/// width of at most 48 px is used as it is.
pub fn fit_width(lanes: usize, container: f32, optional_columns: f32) -> f32 {
    let natural = natural_width(lanes);
    if natural <= MIN_COMPACT_WIDTH {
        return natural;
    }
    let content_budget = MIN_COMPACT_WIDTH.max(container - optional_columns - PRIMARY_MIN);
    let share_budget = MIN_COMPACT_WIDTH.max(container * MAX_WIDTH_RATIO);
    natural.min(content_budget).min(share_budget)
}

/// Snaps a non-natural width down to a multiple of [`RESIZE_STEP`]; natural widths are kept.
pub fn snap_width(width: f32, lanes: usize) -> f32 {
    if width >= natural_width(lanes) {
        width
    } else {
        (width / RESIZE_STEP).floor() * RESIZE_STEP
    }
}

/// Fits a gutter of `lanes` lanes into a table `container` pixels wide whose optional columns
/// take `optional_columns` pixels (R8), given the geometry of the previous frame:
///
/// * the width is [`fit_width`] snapped by [`snap_width`]; a fitted geometry with fewer pixels
///   than natural compresses the lane spacing to `(width - fixed) / (lanes - 1)`; a single
///   lane always uses its natural geometry;
/// * when the lane count is unchanged and the new width differs from the previous by less than
///   2 px the previous geometry is kept (no oscillation while dragging a split divider);
/// * with more than one lane the gutter becomes a compact rail when the primary column
///   (`container - optional - width`) is narrower than 160 px or the lane spacing below 4 px,
///   and leaves it only when the primary column is at least 184 px and the spacing at least 5.
pub fn fit_gutter(
    lanes: usize,
    container: f32,
    optional_columns: f32,
    previous: Option<&GutterGeometry>,
) -> GutterGeometry {
    if lanes <= 1 {
        return GutterGeometry::natural(lanes);
    }
    let width = snap_width(fit_width(lanes, container, optional_columns), lanes);
    let candidate = GutterGeometry::fitted(lanes, width);
    let fitted = match previous {
        Some(previous)
            if previous.lanes == lanes
                && !previous.compact
                && (candidate.width - previous.width).abs() < RESIZE_STEP =>
        {
            *previous
        }
        _ => candidate,
    };
    let primary = container - optional_columns - fitted.width;
    let was_compact = previous.is_some_and(|previous| previous.compact && previous.lanes == lanes);
    let compact = if was_compact {
        !(primary >= COMPACT_EXIT_PRIMARY && fitted.lane_spacing >= COMPACT_EXIT_SPACING)
    } else {
        primary < COMPACT_ENTER_PRIMARY || fitted.lane_spacing < COMPACT_ENTER_SPACING
    };
    if compact {
        GutterGeometry::compact_rail(lanes)
    } else {
        fitted
    }
}

/// The gutter while it morphs from one geometry to another with `progress` in `[0, 1]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MorphedGutter {
    /// The interpolated width.
    pub width: f32,
    /// The interpolated lane spacing.
    pub lane_spacing: f32,
    /// Whether the compact rail topology (all lanes at one x) is drawn.
    pub compact_topology: bool,
}

/// Interpolates width and lane spacing linearly. The compact rail takes over only once
/// `progress` reaches 0.999 when entering, and the full topology returns at the first frame
/// (progress at most 0.001) when leaving.
pub fn morph(from: &GutterGeometry, to: &GutterGeometry, progress: f32) -> MorphedGutter {
    let t = progress.clamp(0.0, 1.0);
    let lerp = |a: f32, b: f32| a + (b - a) * t;
    let compact_topology = if to.compact {
        from.compact || t >= 0.999
    } else {
        false
    };
    MorphedGutter {
        width: lerp(from.width, to.width),
        lane_spacing: lerp(from.lane_spacing, to.lane_spacing),
        compact_topology,
    }
}

/// Whether a pointer at `(x, y)` hits the node centred at `(center_x, center_y)`: within
/// [`HIT_RADIUS`].
pub fn hit_node(center_x: f32, center_y: f32, x: f32, y: f32) -> bool {
    (x - center_x).hypot(y - center_y) <= HIT_RADIUS
}

/// The opacity of a path while path `focused` is highlighted: the focused path is drawn at full
/// opacity (stroke [`FOCUSED_STROKE`]), the others at [`UNFOCUSED_PATH_OPACITY`]; without a
/// focused path everything is drawn at full opacity.
pub fn path_opacity(focused: Option<usize>, path: usize) -> f32 {
    match focused {
        Some(focused) if focused != path => UNFOCUSED_PATH_OPACITY,
        _ => 1.0,
    }
}

/// The opacity of a row while a path is focused: rows on the path stay at full opacity, the
/// others drop to [`UNFOCUSED_ROW_OPACITY`].
pub fn row_opacity(focus_active: bool, on_focused_path: bool) -> f32 {
    if focus_active && !on_focused_path {
        UNFOCUSED_ROW_OPACITY
    } else {
        1.0
    }
}

/// What the gutter canvas hands to the caller's paint hook.
#[derive(Clone, Debug)]
pub struct GutterPaint {
    /// The gutter's bounds, equal to the list viewport vertically.
    pub bounds: Bounds<Pixels>,
    /// The rows whose rectangles intersect the viewport.
    pub rows: std::ops::Range<usize>,
    /// The uniform row height.
    pub row_height: Pixels,
    /// The vertical scroll offset of the list, read from the list in the same frame.
    pub scroll_offset: Pixels,
    /// The gutter geometry.
    pub geometry: GutterGeometry,
    /// The window scale factor.
    pub scale: f32,
}

impl GutterPaint {
    /// The window y of the top of row `ix`.
    pub fn row_top(&self, ix: usize) -> Pixels {
        self.bounds.top() + self.row_height * ix as f32 - self.scroll_offset
    }

    /// The x of `lane`'s centre in window coordinates, on the device pixel grid.
    pub fn lane_x(&self, lane: usize) -> Pixels {
        self.bounds.left() + px(self.geometry.lane_x(lane, self.scale))
    }
}

type PaintHook = Box<dyn Fn(&GutterPaint, &mut Window, &mut App)>;

/// A canvas element of a given width that fills the height of its parent and paints
/// caller-defined marks for the visible rows of `list`, clipped to the list viewport. The
/// scroll offset is read from the list state at paint time, after the list's own prepaint
/// clamped it, so the marks never lag the rows by a frame.
pub struct GraphGutter {
    list: ListState,
    row_height: Pixels,
    geometry: GutterGeometry,
    paint: PaintHook,
}

/// Creates a [`GraphGutter`].
pub fn graph_gutter(
    list: ListState,
    row_height: Pixels,
    geometry: GutterGeometry,
    paint: impl Fn(&GutterPaint, &mut Window, &mut App) + 'static,
) -> GraphGutter {
    GraphGutter {
        list,
        row_height,
        geometry,
        paint: Box::new(paint),
    }
}

impl IntoElement for GraphGutter {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for GraphGutter {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = px(self.geometry.width).into();
        style.size.height = relative(1.).into();
        style.flex_shrink = 0.0;
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        // Paint-time read of the list's own scroll offset (R9).
        let scroll_offset = (-self.list.scroll_px_offset_for_scrollbar().y).max(px(0.));
        let viewport = self.list.viewport_bounds();
        let clip = if viewport.size.height > px(0.) {
            Bounds::new(
                point(bounds.left(), viewport.top()),
                size(bounds.size.width, viewport.size.height),
            )
        } else {
            bounds
        };
        let row_height = self.row_height;
        let first = (f32::from(scroll_offset) / f32::from(row_height))
            .floor()
            .max(0.0) as usize;
        let count = self.list.item_count();
        let visible = (f32::from(clip.size.height) / f32::from(row_height)).ceil() as usize + 2;
        let rows = first.min(count)..(first + visible).min(count);
        let paint = GutterPaint {
            bounds: Bounds::new(
                point(bounds.left(), clip.top()),
                size(bounds.size.width, clip.size.height),
            ),
            rows,
            row_height,
            scroll_offset,
            geometry: self.geometry,
            scale: window.scale_factor(),
        };
        let hook = &self.paint;
        window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
            hook(&paint, window, cx);
        });
    }
}
