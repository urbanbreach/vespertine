use crate::{
    AnyElement, App, Bounds, ContentMask, EdgeFade, Element, GlobalElementId, InspectorElementId,
    IntoElement, LayoutId, Pixels, ScrollHandle, Window,
};

/// Wraps `child` so that everything painted inside it fades toward the edges of the
/// wrapper's own paint-time bounds (RND-001). `top` and `bottom` enable the vertical edges
/// with the common `band`; use the builder methods for the horizontal edges, per-edge bands
/// and overflow gating.
///
/// The wrapper is layout-transparent: its layout is exactly the child's layout. It modulates
/// alpha per fragment (quads, glyphs, emoji, images and vector icons) and never paints a
/// backdrop-colored cover, so translucent windows keep showing the real content behind.
/// Paths, shadows and decorations fade conservatively from their logical bounds; see
/// [`Window::with_edge_fade`].
pub fn edge_faded(band: Pixels, top: bool, bottom: bool, child: impl IntoElement) -> EdgeFaded {
    EdgeFaded {
        child: child.into_any_element(),
        band,
        top,
        bottom,
        left: false,
        right: false,
        band_top: None,
        band_bottom: None,
        band_left: None,
        band_right: None,
        overflow_y: None,
        overflow_x: None,
        overflow_y_with: None,
    }
}

type OverflowPredicate = Box<dyn Fn(&mut Window, &mut App) -> (bool, bool)>;

/// The element returned by [`edge_faded`].
pub struct EdgeFaded {
    child: AnyElement,
    band: Pixels,
    top: bool,
    bottom: bool,
    left: bool,
    right: bool,
    band_top: Option<Pixels>,
    band_bottom: Option<Pixels>,
    band_left: Option<Pixels>,
    band_right: Option<Pixels>,
    overflow_y: Option<ScrollHandle>,
    overflow_x: Option<ScrollHandle>,
    overflow_y_with: Option<OverflowPredicate>,
}

/// Hysteresis, in logical pixels, of the scroll-offset gating of faded edges.
const OVERFLOW_HYSTERESIS: f32 = 1.0;

/// Which ends of a scroll axis have hidden overflow: `scrolled` is the distance scrolled
/// from the start, `max` the maximum scroll offset. The start end needs more than one logical
/// pixel scrolled, the far end more than one logical pixel remaining.
pub fn overflow_ends(scrolled: Pixels, max: Pixels) -> (bool, bool) {
    (
        scrolled.0 > OVERFLOW_HYSTERESIS,
        scrolled.0 < max.0 - OVERFLOW_HYSTERESIS,
    )
}

impl EdgeFaded {
    /// Also fade the left edge.
    pub fn fade_left(mut self, enabled: bool) -> Self {
        self.left = enabled;
        self
    }

    /// Also fade the right edge.
    pub fn fade_right(mut self, enabled: bool) -> Self {
        self.right = enabled;
        self
    }

    /// Overrides the band of the top edge.
    pub fn band_top(mut self, band: Pixels) -> Self {
        self.band_top = Some(band);
        self
    }

    /// Overrides the band of the bottom edge.
    pub fn band_bottom(mut self, band: Pixels) -> Self {
        self.band_bottom = Some(band);
        self
    }

    /// Overrides the band of the left edge.
    pub fn band_left(mut self, band: Pixels) -> Self {
        self.band_left = Some(band);
        self
    }

    /// Overrides the band of the right edge.
    pub fn band_right(mut self, band: Pixels) -> Self {
        self.band_right = Some(band);
        self
    }

    /// Gates the vertical edges on hidden overflow of `handle`, evaluated at paint time after
    /// the child's prepaint has clamped the scroll offset: the top edge stays enabled only
    /// while scrolled more than one logical pixel, the bottom edge only while more than one
    /// logical pixel of content remains.
    pub fn fade_overflow_y(mut self, handle: &ScrollHandle) -> Self {
        self.overflow_y = Some(handle.clone());
        self
    }

    /// Gates the horizontal edges on hidden overflow of `handle` like
    /// [`Self::fade_overflow_y`].
    pub fn fade_overflow_x(mut self, handle: &ScrollHandle) -> Self {
        self.overflow_x = Some(handle.clone());
        self
    }

    /// Gates the vertical edges with a paint-time predicate returning
    /// `(top overflow, bottom overflow)`, combined with the enable flags by logical AND.
    pub fn fade_overflow_y_with(
        mut self,
        predicate: impl Fn(&mut Window, &mut App) -> (bool, bool) + 'static,
    ) -> Self {
        self.overflow_y_with = Some(Box::new(predicate));
        self
    }

    fn resolve(&self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) -> EdgeFade {
        let mut fade = EdgeFade::new(bounds, self.band);
        fade.band_top = self.band_top;
        fade.band_bottom = self.band_bottom;
        fade.band_left = self.band_left;
        fade.band_right = self.band_right;
        fade.top = self.top;
        fade.bottom = self.bottom;
        fade.left = self.left;
        fade.right = self.right;
        if let Some(handle) = &self.overflow_y {
            let (top, bottom) = overflow_ends(-handle.offset().y, handle.max_offset().y);
            fade.top &= top;
            fade.bottom &= bottom;
        }
        if let Some(predicate) = &self.overflow_y_with {
            let (top, bottom) = predicate(window, cx);
            fade.top &= top;
            fade.bottom &= bottom;
        }
        if let Some(handle) = &self.overflow_x {
            let (left, right) = overflow_ends(-handle.offset().x, handle.max_offset().x);
            fade.left &= left;
            fade.right &= right;
        }
        fade
    }
}

impl Element for EdgeFaded {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<crate::ElementId> {
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
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
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
        // Gating is evaluated here, after the child's prepaint clamped scroll offsets.
        let fade = self.resolve(bounds, window, cx);
        let child = &mut self.child;
        window.with_edge_fade(Some(fade), |window| child.paint(window, cx));
    }
}

impl IntoElement for EdgeFaded {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// Wraps `child` so that its entire painted subtree (text, fills, images, custom canvas
/// content) is cut along the wrapper's own paint-time bounds with the logical corner
/// `radius` (RND-002), per fragment and antialiased, without corner-cover shapes.
///
/// The wrapper is layout-transparent. During prepaint it narrows the rectangular content mask
/// to its bounds, so hitboxes beyond the bounds are clipped like paint; the rounded shape
/// itself never cuts hit testing. See [`Window::with_rounded_clip`] for the exceptions
/// (plain solid quads in an all-solid draw batch, paths, decorations, shadows).
pub fn rounded_clip(radius: Pixels, child: impl IntoElement) -> RoundedClip {
    RoundedClip {
        child: child.into_any_element(),
        radius,
    }
}

/// The element returned by [`rounded_clip`].
pub struct RoundedClip {
    child: AnyElement,
    radius: Pixels,
}

impl Element for RoundedClip {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<crate::ElementId> {
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
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let child = &mut self.child;
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            child.prepaint(window, cx);
        });
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
        let child = &mut self.child;
        window.with_rounded_clip(bounds, self.radius, |window| child.paint(window, cx));
    }
}

impl IntoElement for RoundedClip {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}
