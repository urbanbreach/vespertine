//! Trigger-relative floating layer positioning (LAY-005).
//!
//! A [`FloatingLayer`] owns content that is positioned relative to its *trigger* (the border
//! box of the element that contains the layer) or an explicit window point, painted in a
//! deferred pass, kept inside the window, and reports its final border box through a
//! [`BoundsTracker`]. It is composed from the upstream [`anchored`](crate::anchored) and
//! [`deferred`](crate::deferred) elements plus a zero-size positioning wrapper at the trigger.
//!
//! [`place_floating`] is the pure placement computation; it shares its fitting code with the
//! anchored element, so the computed bounds equal the painted bounds.

use serde_json::{Map, Value};

use crate::{
    Anchor, AnchoredFitMode, AnyElement, App, Bounds, Display, Div, Edges, ElementAdapter,
    ElementFactory, ElementId, FlexDirection, HostId, InteractiveElement, Interactivity,
    IntoElement, ParentElement, Pixels, Point, RenderContext, RenderOnce, STANDARD_EVENTS, Size,
    Stateful, StatefulInteractiveElement, StyleRefinement, Styled, Window, anchored, deferred, div,
    elements::fit_anchored_bounds,
    host_surface::{BoundsTracker, track_bounds},
    point, px, relative, rgb,
};

/// Default snap margin on every window edge, in logical pixels.
pub const DEFAULT_SNAP_MARGIN: Pixels = px(8.);
/// Default deferred paint priority.
pub const DEFAULT_PRIORITY: usize = 1;
/// sRGB hex of the opaque fill a layer without a declared background receives.
pub const FALLBACK_BACKGROUND: u32 = 0x1A1A1A;

/// The trigger side a layer attaches to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FloatingSide {
    /// Above the trigger.
    Top,
    /// Right of the trigger.
    Right,
    /// Below the trigger (default).
    #[default]
    Bottom,
    /// Left of the trigger.
    Left,
}

impl FloatingSide {
    /// Parses `top`, `right`, `bottom` or `left` (case-sensitive).
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "top" => Self::Top,
            "right" => Self::Right,
            "bottom" => Self::Bottom,
            "left" => Self::Left,
            _ => return None,
        })
    }
}

/// Alignment along the trigger edge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FloatingAlign {
    /// Left (top and bottom sides) or top (left and right sides) of the trigger (default).
    #[default]
    Start,
    /// Center of the trigger edge.
    Center,
    /// Right (top and bottom sides) or bottom (left and right sides) of the trigger.
    End,
}

impl FloatingAlign {
    /// Parses `start`, `center` or `end` (case-sensitive).
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "start" => Self::Start,
            "center" => Self::Center,
            "end" => Self::End,
            _ => return None,
        })
    }
}

/// How the layer is fitted into the window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FloatingFit {
    /// Snap to the window edges keeping the snap margin (default).
    #[default]
    Snap,
    /// Upstream anchored fitting: try the opposite content anchors, then snap with no margin.
    Switch,
}

impl FloatingFit {
    /// Parses `snap` or `switch` (case-sensitive).
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "snap" => Self::Snap,
            "switch" => Self::Switch,
            _ => return None,
        })
    }
}

/// Parses an anchor token: `topLeft`, `topCenter`, `topRight`, `rightCenter`, `bottomRight`,
/// `bottomCenter`, `bottomLeft` or `leftCenter` (case-sensitive).
pub fn parse_anchor(value: &str) -> Option<Anchor> {
    Some(match value {
        "topLeft" => Anchor::TopLeft,
        "topCenter" => Anchor::TopCenter,
        "topRight" => Anchor::TopRight,
        "rightCenter" => Anchor::RightCenter,
        "bottomRight" => Anchor::BottomRight,
        "bottomCenter" => Anchor::BottomCenter,
        "bottomLeft" => Anchor::BottomLeft,
        "leftCenter" => Anchor::LeftCenter,
        _ => return None,
    })
}

/// Declarative placement options of a floating layer; see the spec defaults in [`Default`].
#[derive(Clone, Debug, PartialEq)]
pub struct FloatingOptions {
    /// Explicit window position replacing the trigger attach point.
    pub position: Option<Point<Pixels>>,
    /// Trigger side.
    pub side: FloatingSide,
    /// Alignment along the side.
    pub align: FloatingAlign,
    /// Content point override; `None` uses [`default_content_anchor`].
    pub anchor: Option<Anchor>,
    /// Distance moving the content outward along the side.
    pub gap: Pixels,
    /// Offset in window axes.
    pub offset: Point<Pixels>,
    /// Fitting mode.
    pub fit: FloatingFit,
    /// Snap margin on every edge (snap fitting only).
    pub snap_margin: Pixels,
    /// Paint in the deferred pass (otherwise in tree order).
    pub deferred: bool,
    /// Deferred paint priority; higher paints on top.
    pub priority: usize,
    /// Block pointer hit testing beneath the content box.
    pub occlude: bool,
    /// Cover the whole window at (0,0) with the viewport size.
    pub fill_window: bool,
}

impl Default for FloatingOptions {
    fn default() -> Self {
        Self {
            position: None,
            side: FloatingSide::Bottom,
            align: FloatingAlign::Start,
            anchor: None,
            gap: px(0.),
            offset: point(px(0.), px(0.)),
            fit: FloatingFit::Snap,
            snap_margin: DEFAULT_SNAP_MARGIN,
            deferred: true,
            priority: DEFAULT_PRIORITY,
            occlude: true,
            fill_window: false,
        }
    }
}

/// The property names the anchored host reads (see [`FloatingOptions::from_properties`]).
pub const FLOATING_PROPERTIES: &[&str] = &[
    "side",
    "align",
    "position",
    "anchor",
    "offset",
    "gap",
    "fit",
    "snapMargin",
    "fill",
    "occlude",
    "priority",
    "deferred",
];

fn finite_number(value: Option<&Value>) -> Option<f32> {
    value
        .and_then(Value::as_f64)
        .filter(|number| number.is_finite())
        .map(|number| number as f32)
}

impl FloatingOptions {
    /// Builds options from the host's properties (R8): an unknown side, alignment or fitting
    /// falls back to its default, an invalid anchor clears the override, a missing or invalid
    /// gap or offset component is zero, an invalid snap margin or priority is the default, and
    /// a position without two finite coordinates is no position.
    pub fn from_properties(properties: &Map<String, Value>) -> Self {
        let mut options = Self::default();
        if let Some(Value::Object(position)) = properties.get("position") {
            if let (Some(x), Some(y)) = (
                finite_number(position.get("x")),
                finite_number(position.get("y")),
            ) {
                options.position = Some(point(px(x), px(y)));
            }
        }
        if let Some(side) = properties
            .get("side")
            .and_then(Value::as_str)
            .and_then(FloatingSide::parse)
        {
            options.side = side;
        }
        if let Some(align) = properties
            .get("align")
            .and_then(Value::as_str)
            .and_then(FloatingAlign::parse)
        {
            options.align = align;
        }
        options.anchor = properties
            .get("anchor")
            .and_then(Value::as_str)
            .and_then(parse_anchor);
        options.gap = px(finite_number(properties.get("gap")).unwrap_or(0.));
        if let Some(Value::Object(offset)) = properties.get("offset") {
            options.offset = point(
                px(finite_number(offset.get("x")).unwrap_or(0.)),
                px(finite_number(offset.get("y")).unwrap_or(0.)),
            );
        }
        if let Some(fit) = properties
            .get("fit")
            .and_then(Value::as_str)
            .and_then(FloatingFit::parse)
        {
            options.fit = fit;
        }
        if let Some(margin) = finite_number(properties.get("snapMargin")).filter(|m| *m >= 0.) {
            options.snap_margin = px(margin);
        }
        if let Some(deferred) = properties.get("deferred").and_then(Value::as_bool) {
            options.deferred = deferred;
        }
        if let Some(priority) = properties
            .get("priority")
            .and_then(Value::as_f64)
            .filter(|p| p.is_finite() && *p >= 0.)
        {
            options.priority = priority as usize;
        }
        if let Some(occlude) = properties.get("occlude").and_then(Value::as_bool) {
            options.occlude = occlude;
        }
        options.fill_window = properties.get("fill").and_then(Value::as_str) == Some("window");
        options
    }

    /// The content point that is attached to the attach point.
    pub fn content_anchor(&self) -> Anchor {
        self.anchor
            .unwrap_or_else(|| default_content_anchor(self.side, self.align))
    }
}

/// The trigger point a layer on `side` with `align` attaches to (R1).
pub fn attach_point(
    trigger: Bounds<Pixels>,
    side: FloatingSide,
    align: FloatingAlign,
) -> Point<Pixels> {
    let (fx, fy) = attach_fractions(side, align);
    point(
        trigger.origin.x + trigger.size.width * fx,
        trigger.origin.y + trigger.size.height * fy,
    )
}

/// The attach point as fractions of the trigger's width and height.
pub fn attach_fractions(side: FloatingSide, align: FloatingAlign) -> (f32, f32) {
    let along = match align {
        FloatingAlign::Start => 0.,
        FloatingAlign::Center => 0.5,
        FloatingAlign::End => 1.,
    };
    match side {
        FloatingSide::Bottom => (along, 1.),
        FloatingSide::Top => (along, 0.),
        FloatingSide::Right => (1., along),
        FloatingSide::Left => (0., along),
    }
}

/// The default content point attached to the trigger point (R1).
pub fn default_content_anchor(side: FloatingSide, align: FloatingAlign) -> Anchor {
    use FloatingAlign::*;
    match (side, align) {
        (FloatingSide::Bottom, Start) => Anchor::TopLeft,
        (FloatingSide::Bottom, Center) => Anchor::TopCenter,
        (FloatingSide::Bottom, End) => Anchor::TopRight,
        (FloatingSide::Top, Start) => Anchor::BottomLeft,
        (FloatingSide::Top, Center) => Anchor::BottomCenter,
        (FloatingSide::Top, End) => Anchor::BottomRight,
        (FloatingSide::Right, Start) => Anchor::TopLeft,
        (FloatingSide::Right, Center) => Anchor::LeftCenter,
        (FloatingSide::Right, End) => Anchor::BottomLeft,
        (FloatingSide::Left, Start) => Anchor::TopRight,
        (FloatingSide::Left, Center) => Anchor::RightCenter,
        (FloatingSide::Left, End) => Anchor::BottomRight,
    }
}

/// The displacement the gap applies: outward along `side` (R1).
pub fn gap_vector(side: FloatingSide, gap: Pixels) -> Point<Pixels> {
    match side {
        FloatingSide::Top => point(px(0.), -gap),
        FloatingSide::Right => point(gap, px(0.)),
        FloatingSide::Bottom => point(px(0.), gap),
        FloatingSide::Left => point(-gap, px(0.)),
    }
}

/// Computes the final border box of a layer (R1-R3, R6).
///
/// * `trigger`: border box of the element owning the layer (ignored for the attach point when
///   `options.position` is set).
/// * `layout_origin`: the layout origin the final offset is rounded relative to; `None` uses the
///   attach point (trigger mode, where the positioning wrapper sits on it) or the trigger origin
///   (explicit position).
/// * `content_size`: measured size of the content.
/// * `viewport` and `client_inset`: window size and the transparent client-decoration border.
///
/// Full-window fill returns `(0, 0, viewport)`.
pub fn place_floating(
    options: &FloatingOptions,
    trigger: Bounds<Pixels>,
    layout_origin: Option<Point<Pixels>>,
    content_size: Size<Pixels>,
    viewport: Size<Pixels>,
    client_inset: Pixels,
) -> Bounds<Pixels> {
    if options.fill_window {
        return Bounds {
            origin: point(px(0.), px(0.)),
            size: viewport,
        };
    }
    let attach = options
        .position
        .unwrap_or_else(|| attach_point(trigger, options.side, options.align));
    let anchor = options.content_anchor();
    let offset = gap_vector(options.side, options.gap) + options.offset;
    let desired = Bounds::from_anchor_and_size(anchor, attach + offset, content_size);
    let fit_mode = match options.fit {
        FloatingFit::Snap => {
            AnchoredFitMode::SnapToWindowWithMargin(Edges::all(options.snap_margin))
        }
        FloatingFit::Switch => AnchoredFitMode::SwitchAnchor,
    };
    let limits = Bounds {
        origin: point(px(0.), px(0.)),
        size: viewport,
    };
    let fitted = fit_anchored_bounds(fit_mode, anchor, attach, desired, limits, client_inset);

    let layout_origin = layout_origin.unwrap_or(if options.position.is_some() {
        trigger.origin
    } else {
        attach
    });
    let delta = fitted.origin - layout_origin;
    Bounds {
        origin: point(
            layout_origin.x + delta.x.round(),
            layout_origin.y + delta.y.round(),
        ),
        size: fitted.size,
    }
}

/// A floating layer host: content positioned relative to its trigger or a window point.
///
/// Place the layer as a child of the trigger element. It occupies no flow space there.
/// Styling, accessibility properties and the ordinary interactive handlers (click, hover,
/// file drop, ...) apply to the content box. Use [`FloatingLayer::bounds_tracker`] to read the
/// final border box after placement.
#[derive(IntoElement)]
pub struct FloatingLayer {
    options: FloatingOptions,
    content: Stateful<Div>,
    tracker: BoundsTracker,
    trigger_border: Edges<Pixels>,
}

/// Creates a [`FloatingLayer`] whose content box has the given stable element id.
pub fn floating_layer(id: impl Into<ElementId>) -> FloatingLayer {
    FloatingLayer {
        options: FloatingOptions::default(),
        content: div().id(id),
        tracker: BoundsTracker::new(),
        trigger_border: Edges::default(),
    }
}

impl FloatingLayer {
    /// Replaces all placement options.
    pub fn options(mut self, options: FloatingOptions) -> Self {
        self.options = options;
        self
    }

    /// Positions the layer at an explicit window point instead of the trigger.
    pub fn position(mut self, position: Point<Pixels>) -> Self {
        self.options.position = Some(position);
        self
    }

    /// Sets the trigger side.
    pub fn side(mut self, side: FloatingSide) -> Self {
        self.options.side = side;
        self
    }

    /// Sets the alignment along the side.
    pub fn align(mut self, align: FloatingAlign) -> Self {
        self.options.align = align;
        self
    }

    /// Overrides which content point is attached.
    pub fn anchor(mut self, anchor: Anchor) -> Self {
        self.options.anchor = Some(anchor);
        self
    }

    /// Sets the gap along the side.
    pub fn gap(mut self, gap: Pixels) -> Self {
        self.options.gap = gap;
        self
    }

    /// Sets the offset in window axes.
    pub fn offset(mut self, offset: Point<Pixels>) -> Self {
        self.options.offset = offset;
        self
    }

    /// Sets the fitting mode.
    pub fn fit(mut self, fit: FloatingFit) -> Self {
        self.options.fit = fit;
        self
    }

    /// Sets the snap margin.
    pub fn snap_margin(mut self, margin: Pixels) -> Self {
        self.options.snap_margin = margin;
        self
    }

    /// Enables or disables the deferred paint pass.
    pub fn deferred(mut self, deferred: bool) -> Self {
        self.options.deferred = deferred;
        self
    }

    /// Sets the deferred paint priority.
    pub fn priority(mut self, priority: usize) -> Self {
        self.options.priority = priority;
        self
    }

    /// Enables or disables hit-test occlusion beneath the content box.
    pub fn occlude_below(mut self, occlude: bool) -> Self {
        self.options.occlude = occlude;
        self
    }

    /// Covers the whole window (R6).
    pub fn fill_window(mut self) -> Self {
        self.options.fill_window = true;
        self
    }

    /// The trigger's border widths. Absolute insets resolve against the trigger's padding box,
    /// so a bordered trigger needs them for the layer to attach to its border box (R1). The
    /// declarative host passes the parent's border widths; the default is no border.
    pub fn trigger_border(mut self, border: Edges<Pixels>) -> Self {
        self.trigger_border = border;
        self
    }

    /// Records the final content border box into `tracker` on every paint.
    pub fn bounds_tracker(mut self, tracker: &BoundsTracker) -> Self {
        self.tracker = tracker.clone();
        self
    }
}

impl Styled for FloatingLayer {
    fn style(&mut self) -> &mut StyleRefinement {
        self.content.style()
    }
}

impl InteractiveElement for FloatingLayer {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.content.interactivity()
    }
}

impl StatefulInteractiveElement for FloatingLayer {}

impl ParentElement for FloatingLayer {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.content.extend(elements);
    }
}

impl RenderOnce for FloatingLayer {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let FloatingLayer {
            options,
            mut content,
            tracker,
            trigger_border,
        } = self;

        // Content uses column layout unless the style chose otherwise, and an opaque fallback
        // fill unless a background (including an explicit transparent one) is declared (R5).
        let style = content.style();
        if style.display.is_none() {
            style.display = Some(Display::Flex);
        }
        if style.flex_direction.is_none() {
            style.flex_direction = Some(FlexDirection::Column);
        }
        if style.background.is_none() {
            style.background = Some(rgb(FALLBACK_BACKGROUND).into());
        }
        if options.fill_window {
            let viewport = window.viewport_size();
            content = content.w(viewport.width).h(viewport.height);
        }
        if options.occlude {
            content = content.occlude();
        }

        let mut layer = anchored().anchor(options.content_anchor());
        if options.fill_window {
            layer = layer.position(point(px(0.), px(0.))).snap_to_window();
        } else {
            if let Some(position) = options.position {
                layer = layer.position(position);
            }
            layer = layer.offset(gap_vector(options.side, options.gap) + options.offset);
            if options.fit == FloatingFit::Snap {
                layer = layer.snap_to_window_with_margin(Edges::all(options.snap_margin));
            }
        }
        let layer = layer.child(track_bounds(&tracker, content));

        let layer = if options.deferred {
            deferred(layer)
                .with_priority(options.priority)
                .into_any_element()
        } else {
            layer.into_any_element()
        };

        if options.position.is_some() || options.fill_window {
            layer
        } else {
            // A zero-size wrapper on the trigger's attach point: the anchored element
            // measures its own origin from it.
            let (fx, fy) = attach_fractions(options.side, options.align);
            let point_wrapper = div()
                .absolute()
                .left(relative(fx))
                .top(relative(fy))
                .w(px(0.))
                .h(px(0.))
                .child(layer);
            if trigger_border == Edges::default() {
                point_wrapper.into_any_element()
            } else {
                // Absolute insets resolve against the trigger's padding box. Negative margins
                // on a full-inset wrapper grow it to the trigger's border box.
                div()
                    .absolute()
                    .left(px(0.))
                    .top(px(0.))
                    .right(px(0.))
                    .bottom(px(0.))
                    .ml(-trigger_border.left)
                    .mt(-trigger_border.top)
                    .mr(-trigger_border.right)
                    .mb(-trigger_border.bottom)
                    .child(point_wrapper)
                    .into_any_element()
            }
        }
    }
}

/// Factory of the built-in `anchored` host type (R12): a [`FloatingLayer`] driven by the host's
/// properties ([`FLOATING_PROPERTIES`]) with the standard events.
pub struct AnchoredFactory;

impl ElementFactory for AnchoredFactory {
    fn type_name(&self) -> &'static str {
        "anchored"
    }

    fn create(&self, _id: HostId) -> Box<dyn ElementAdapter> {
        Box::new(AnchoredAdapter::default())
    }
}

/// The adapter of the `anchored` host type. It only retains the placement properties it was
/// given; placement is recomputed natively every frame.
#[derive(Default)]
pub struct AnchoredAdapter {
    properties: Map<String, Value>,
}

impl ElementAdapter for AnchoredAdapter {
    fn render(
        &mut self,
        mut context: RenderContext,
        _window: &mut Window,
        _cx: &mut App,
    ) -> AnyElement {
        let layer = floating_layer(context.element_id())
            .options(FloatingOptions::from_properties(&self.properties))
            .trigger_border(context.trigger_border)
            .bounds_tracker(&context.bounds);
        let mut layer = context.apply_to(layer);
        layer.extend(std::mem::take(&mut context.children));
        layer.into_any_element()
    }

    fn set_property(&mut self, name: &str, value: &Value) {
        if value.is_null() {
            self.properties.remove(name);
        } else {
            self.properties.insert(name.to_owned(), value.clone());
        }
    }

    fn supported_properties(&self) -> &'static [&'static str] {
        FLOATING_PROPERTIES
    }

    fn supported_events(&self) -> &'static [&'static str] {
        &STANDARD_EVENTS
    }

    fn destroy(&mut self, _cx: &mut App) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::size;
    use serde_json::json;

    fn trigger() -> Bounds<Pixels> {
        Bounds::new(point(px(100.), px(60.)), size(px(80.), px(30.)))
    }

    fn props(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn attach_and_content_points_for_all_twelve_combinations() {
        use FloatingAlign::*;
        use FloatingSide::*;
        // (side, align, attach point, content point) on the trigger (100,60,80,30).
        let table = [
            (Bottom, Start, (100., 90.), Anchor::TopLeft),
            (Bottom, Center, (140., 90.), Anchor::TopCenter),
            (Bottom, End, (180., 90.), Anchor::TopRight),
            (Top, Start, (100., 60.), Anchor::BottomLeft),
            (Top, Center, (140., 60.), Anchor::BottomCenter),
            (Top, End, (180., 60.), Anchor::BottomRight),
            (Right, Start, (180., 60.), Anchor::TopLeft),
            (Right, Center, (180., 75.), Anchor::LeftCenter),
            (Right, End, (180., 90.), Anchor::BottomLeft),
            (Left, Start, (100., 60.), Anchor::TopRight),
            (Left, Center, (100., 75.), Anchor::RightCenter),
            (Left, End, (100., 90.), Anchor::BottomRight),
        ];
        for (side, align, (x, y), anchor) in table {
            assert_eq!(
                attach_point(trigger(), side, align),
                point(px(x), px(y)),
                "{side:?} {align:?}"
            );
            assert_eq!(
                default_content_anchor(side, align),
                anchor,
                "{side:?} {align:?}"
            );
        }
    }

    #[test]
    fn gap_moves_content_outward_for_each_side() {
        let gap = px(6.);
        assert_eq!(gap_vector(FloatingSide::Top, gap), point(px(0.), px(-6.)));
        assert_eq!(gap_vector(FloatingSide::Right, gap), point(px(6.), px(0.)));
        assert_eq!(gap_vector(FloatingSide::Bottom, gap), point(px(0.), px(6.)));
        assert_eq!(gap_vector(FloatingSide::Left, gap), point(px(-6.), px(0.)));

        let viewport = size(px(600.), px(400.));
        let content = size(px(50.), px(40.));
        let trigger = Bounds::new(point(px(200.), px(150.)), size(px(80.), px(30.)));
        for side in [
            FloatingSide::Top,
            FloatingSide::Right,
            FloatingSide::Bottom,
            FloatingSide::Left,
        ] {
            let flush = FloatingOptions {
                side,
                ..Default::default()
            };
            let gapped = FloatingOptions {
                side,
                gap,
                ..Default::default()
            };
            let a = place_floating(&flush, trigger, None, content, viewport, px(0.));
            let b = place_floating(&gapped, trigger, None, content, viewport, px(0.));
            assert_eq!(b.origin - a.origin, gap_vector(side, gap), "{side:?}");
        }
    }

    #[test]
    fn spec_scene_places_layer_at_80_96() {
        let options = FloatingOptions {
            side: FloatingSide::Bottom,
            align: FloatingAlign::End,
            gap: px(6.),
            ..Default::default()
        };
        let bounds = place_floating(
            &options,
            trigger(),
            None,
            size(px(100.), px(50.)),
            size(px(320.), px(200.)),
            px(0.),
        );
        assert_eq!(
            bounds,
            Bounds::new(point(px(80.), px(96.)), size(px(100.), px(50.)))
        );
    }

    #[test]
    fn explicit_position_anchor_gap_and_offset() {
        let options = FloatingOptions {
            position: Some(point(px(200.), px(100.))),
            side: FloatingSide::Right,
            anchor: Some(Anchor::BottomRight),
            gap: px(4.),
            offset: point(px(-10.), px(3.)),
            ..Default::default()
        };
        let bounds = place_floating(
            &options,
            trigger(),
            None,
            size(px(60.), px(30.)),
            size(px(800.), px(600.)),
            px(0.),
        );
        // attach (200,100) + gap right 4 + offset (-10,3) = (194,103) is the bottom-right.
        assert_eq!(
            bounds,
            Bounds::new(point(px(134.), px(73.)), size(px(60.), px(30.)))
        );
    }

    #[test]
    fn snap_fitting_margin_on_every_edge() {
        let viewport = size(px(320.), px(200.));
        let content = size(px(100.), px(50.));
        let at = |x: f32, y: f32| FloatingOptions {
            position: Some(point(px(x), px(y))),
            ..Default::default()
        };
        let place = |options: &FloatingOptions, inset: f32| {
            place_floating(options, trigger(), None, content, viewport, px(inset))
        };
        // Right overflow: moves left by the overflow plus margin.
        assert_eq!(place(&at(300., 10.), 0.).origin, point(px(212.), px(10.)));
        // Bottom overflow.
        assert_eq!(place(&at(10., 190.), 0.).origin, point(px(10.), px(142.)));
        // Left clamp to the margin.
        assert_eq!(place(&at(-30., 10.), 0.).origin, point(px(8.), px(10.)));
        // Top clamp to the margin.
        assert_eq!(place(&at(10., -30.), 0.).origin, point(px(10.), px(8.)));
        // Client inset adds to the margin on each edge.
        assert_eq!(place(&at(300., 190.), 4.).origin, point(px(208.), px(138.)));
        assert_eq!(place(&at(-30., -30.), 4.).origin, point(px(12.), px(12.)));
        // Explicit margin.
        let margin = FloatingOptions {
            snap_margin: px(0.),
            ..at(300., 10.)
        };
        assert_eq!(place(&margin, 0.).origin, point(px(220.), px(10.)));
    }

    #[test]
    fn layer_wider_than_window_aligns_to_left_margin() {
        let options = FloatingOptions {
            position: Some(point(px(50.), px(20.))),
            ..Default::default()
        };
        let bounds = place_floating(
            &options,
            trigger(),
            None,
            size(px(500.), px(50.)),
            size(px(320.), px(200.)),
            px(0.),
        );
        assert_eq!(bounds.origin.x, px(8.));
    }

    #[test]
    fn switch_fitting_uses_upstream_anchor_switching() {
        let options = FloatingOptions {
            position: Some(point(px(300.), px(20.))),
            fit: FloatingFit::Switch,
            ..Default::default()
        };
        // Top-left at x=300 overflows 320; switching to top-right fits without a margin.
        let bounds = place_floating(
            &options,
            trigger(),
            None,
            size(px(100.), px(50.)),
            size(px(320.), px(200.)),
            px(0.),
        );
        assert_eq!(bounds.origin, point(px(200.), px(20.)));
    }

    #[test]
    fn fractional_offsets_round_per_axis() {
        let options = FloatingOptions {
            position: Some(point(px(10.4), px(20.6))),
            ..Default::default()
        };
        let bounds = place_floating(
            &options,
            trigger(),
            Some(point(px(0.), px(0.))),
            size(px(10.), px(10.)),
            size(px(320.), px(200.)),
            px(0.),
        );
        assert_eq!(bounds.origin, point(px(10.), px(21.)));
    }

    #[test]
    fn fill_window_forces_origin_and_viewport_size() {
        let options = FloatingOptions {
            fill_window: true,
            position: Some(point(px(40.), px(40.))),
            gap: px(9.),
            offset: point(px(5.), px(5.)),
            ..Default::default()
        };
        let viewport = size(px(333.), px(222.));
        let bounds = place_floating(
            &options,
            trigger(),
            None,
            size(px(10.), px(10.)),
            viewport,
            px(6.),
        );
        assert_eq!(bounds, Bounds::new(point(px(0.), px(0.)), viewport));
    }

    #[test]
    fn unknown_property_values_fall_back_to_defaults() {
        let options = FloatingOptions::from_properties(&props(json!({
            "side": "diagonal",
            "align": "Middle",
            "anchor": "nowhere",
            "gap": "wide",
            "offset": { "x": "a", "y": 3 },
            "fit": "magic",
            "snapMargin": -2,
            "deferred": "yes",
            "priority": -1,
            "occlude": 1,
            "fill": "screen",
            "position": { "x": 1 },
        })));
        let expected = FloatingOptions {
            offset: point(px(0.), px(3.)),
            ..Default::default()
        };
        assert_eq!(options, expected);
    }

    #[test]
    fn valid_properties_are_parsed() {
        let options = FloatingOptions::from_properties(&props(json!({
            "position": { "x": 10, "y": 20.5 },
            "side": "left",
            "align": "end",
            "anchor": "bottomCenter",
            "gap": 6,
            "offset": { "x": 1, "y": 2 },
            "fit": "switch",
            "snapMargin": 12,
            "deferred": false,
            "priority": 7,
            "occlude": false,
            "fill": "window",
        })));
        assert_eq!(
            options,
            FloatingOptions {
                position: Some(point(px(10.), px(20.5))),
                side: FloatingSide::Left,
                align: FloatingAlign::End,
                anchor: Some(Anchor::BottomCenter),
                gap: px(6.),
                offset: point(px(1.), px(2.)),
                fit: FloatingFit::Switch,
                snap_margin: px(12.),
                deferred: false,
                priority: 7,
                occlude: false,
                fill_window: true,
            }
        );
        assert_eq!(options.content_anchor(), Anchor::BottomCenter);
        assert_eq!(FloatingOptions::default().content_anchor(), Anchor::TopLeft);
    }
}

#[cfg(test)]
mod window_tests;
