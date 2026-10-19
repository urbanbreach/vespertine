//! Pure decisions and data for the X11 backend (PLT-001 R21-R25): whether composition is
//! available, composited mode versus SHAPE cutouts, the overlay window lifecycle and input
//! shape, and the occlusion rectangles cut out of native children's shapes.
//!
//! The backend does the X requests; everything it needs to decide lives here so it can be
//! tested without a display server.

use crate::{
    Bounds, ComposedLayer, ComposedScene, CompositionError, CompositionLayout, DevicePixels,
    PaintOperation, Pixels, Point, Primitive, ScaledPixels, Size, SurfaceId, point, size,
};

/// The maximum number of occlusion rectangles; beyond it the set collapses to one rectangle.
pub const MAX_OCCLUSION_RECTS: usize = 256;

/// What the X11 backend knows about compositing for a window's screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct X11CompositingFacts {
    /// The window has a 32-bit ARGB visual.
    pub argb_visual: bool,
    /// The server is an XWayland instance (a compositor is then always present).
    pub xwayland: bool,
    /// A compositing manager owns the compositing selection of the window's screen.
    pub compositing_manager: bool,
}

/// How framework content stacked above native children reaches the screen on X11.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum X11CompositionMode {
    /// A transparent override-redirect overlay window blends over the native children; the
    /// opaque coverage of the overlay is additionally cut out of the children's shapes so a
    /// compositor that stacks the overlay window below the parent still shows opaque
    /// controls.
    Composited,
    /// No suitable compositing: the occlusion rectangles of all framework content above a
    /// native child are cut out of that child's bounding and input shapes so the parent's own
    /// pixels (which contain the overlay content) show through. Translucent backdrops and
    /// shadows cannot blend over native pixels in this mode.
    ShapeFallback,
}

impl X11CompositionMode {
    /// Whether occlusion rectangles of this mode only come from fully opaque primitives
    /// (composited mode) or from every primitive that paints pixels (SHAPE fallback).
    pub fn opaque_only(self) -> bool {
        matches!(self, X11CompositionMode::Composited)
    }
}

/// Decides between composited mode and the SHAPE fallback: composited mode needs a 32-bit
/// ARGB visual and either an XWayland server or a compositing manager. The caller makes the
/// decision the first time it is needed and caches it.
pub fn decide_x11_composition_mode(facts: X11CompositingFacts) -> X11CompositionMode {
    if facts.argb_visual && (facts.xwayland || facts.compositing_manager) {
        X11CompositionMode::Composited
    } else {
        X11CompositionMode::ShapeFallback
    }
}

/// Enabling composition on X11 needs the SHAPE extension; without it enabling fails and the
/// window stays single-plane.
pub fn require_x11_shape_extension(shape_extension: bool) -> Result<(), CompositionError> {
    if shape_extension {
        Ok(())
    } else {
        Err(CompositionError::Unsupported(
            "window composition on X11 needs the SHAPE extension, which the X server does not provide"
                .into(),
        ))
    }
}

/// Whether the transparent overlay window must exist: the overlay window is destroyed (not
/// merely unmapped) when the overlay scene and its input region are both empty, keeping its
/// renderer as a spare for the next overlay window.
pub fn overlay_window_needed(
    overlay_scene_has_content: bool,
    input_region_has_content: bool,
) -> bool {
    overlay_scene_has_content || input_region_has_content
}

/// Converts device-space bounds to whole device pixels by flooring the origin and ceiling
/// the opposite corner. Returns `None` for empty results.
pub fn device_rect(bounds: Bounds<ScaledPixels>) -> Option<Bounds<DevicePixels>> {
    let left = bounds.origin.x.0.floor();
    let top = bounds.origin.y.0.floor();
    let right = (bounds.origin.x.0 + bounds.size.width.0).ceil();
    let bottom = (bounds.origin.y.0 + bounds.size.height.0).ceil();
    let (width, height) = (right - left, bottom - top);
    if !(width > 0.0 && height > 0.0) {
        return None;
    }
    Some(Bounds {
        origin: point(DevicePixels(left as i32), DevicePixels(top as i32)),
        size: size(DevicePixels(width as i32), DevicePixels(height as i32)),
    })
}

/// The input shape of an X11 overlay window: the overlay's input region (already empty while
/// capture is inactive, see [`overlay_input_region`](super::overlay_input_region)) converted
/// to device pixels. Empty until set from content, so events outside hitboxes pass through.
pub fn overlay_input_shape(
    region: &[Bounds<Pixels>],
    scale_factor: f32,
) -> Vec<Bounds<DevicePixels>> {
    region
        .iter()
        .filter_map(|bounds| device_rect(bounds.scale(scale_factor)))
        .collect()
}

fn area(rect: &Bounds<DevicePixels>) -> i64 {
    i64::from(rect.size.width.0) * i64::from(rect.size.height.0)
}

fn contains(outer: &Bounds<DevicePixels>, inner: &Bounds<DevicePixels>) -> bool {
    outer.origin.x.0 <= inner.origin.x.0
        && outer.origin.y.0 <= inner.origin.y.0
        && outer.origin.x.0 + outer.size.width.0 >= inner.origin.x.0 + inner.size.width.0
        && outer.origin.y.0 + outer.size.height.0 >= inner.origin.y.0 + inner.size.height.0
}

/// Collects the rectangles of primitives that occlude what lies below them.
///
/// Rectangles come from quads (only fully opaque ones when `opaque_only`), paths, underlines,
/// monochrome and subpixel text sprites, surfaces and polychrome sprites; shadows never
/// contribute. Every rectangle is intersected with its content mask and, inside a group, with
/// the group's enclosing bounds. A group marker is a container, not an image: only the
/// drawable content inside it contributes, so metadata and empty scope markers never occlude
/// anything.
#[derive(Debug)]
pub struct OcclusionCollector {
    opaque_only: bool,
    groups: Vec<Bounds<ScaledPixels>>,
    rects: Vec<Bounds<DevicePixels>>,
}

impl OcclusionCollector {
    /// A collector; `opaque_only` selects the opaque-only variant used while a transparent
    /// overlay window is in use.
    pub fn new(opaque_only: bool) -> Self {
        OcclusionCollector {
            opaque_only,
            groups: Vec::new(),
            rects: Vec::new(),
        }
    }

    /// Enters a rounded drawable group enclosed by `bounds`.
    pub fn begin_group(&mut self, bounds: Bounds<ScaledPixels>) {
        let enclosing = match self.groups.last() {
            Some(outer) => outer.intersect(&bounds),
            None => bounds,
        };
        self.groups.push(enclosing);
    }

    /// Leaves the innermost group.
    pub fn end_group(&mut self) {
        self.groups.pop();
    }

    /// Adds a primitive's occluding rectangle, if it has one.
    pub fn add_primitive(&mut self, primitive: &Primitive) {
        let bounds = match primitive {
            Primitive::Shadow(_) => return,
            Primitive::Quad(quad) => {
                if self.opaque_only && !quad.background.is_opaque() {
                    return;
                }
                quad.bounds
            }
            Primitive::Path(path) => path.bounds,
            Primitive::Underline(underline) => underline.bounds,
            Primitive::MonochromeSprite(sprite) => sprite.bounds,
            Primitive::SubpixelSprite(sprite) => sprite.bounds,
            Primitive::Surface(surface) => surface.bounds,
            Primitive::PolychromeSprite(sprite) => sprite.bounds,
        };
        let mut clipped = bounds.intersect(&primitive.content_mask().bounds);
        if let Some(group) = self.groups.last() {
            clipped = clipped.intersect(group);
        }
        if let Some(rect) = device_rect(clipped) {
            self.rects.push(rect);
        }
    }

    /// Adds every primitive of a scene's paint stream, descending into its scopes.
    pub fn add_scene(&mut self, scene: &crate::Scene) {
        for operation in &scene.paint_operations {
            match operation {
                PaintOperation::Primitive(primitive) => self.add_primitive(primitive),
                PaintOperation::PushCurvedClip(shape) => {
                    let [left, top, right, bottom] = shape.bounds;
                    self.begin_group(Bounds::from_corners(
                        Point::new(ScaledPixels(left), ScaledPixels(top)),
                        Point::new(ScaledPixels(right), ScaledPixels(bottom)),
                    ));
                }
                PaintOperation::PopCurvedClip => self.end_group(),
                PaintOperation::StartLayer(_)
                | PaintOperation::EndLayer
                | PaintOperation::BackdropBlur(_) => {}
            }
        }
    }

    /// The final rectangle set: sorted by area, largest first, with rectangles contained in a
    /// kept one dropped; more than [`MAX_OCCLUSION_RECTS`] remaining rectangles collapse to
    /// their single bounding union, which is never smaller than any dropped occluder.
    pub fn finish(mut self) -> Vec<Bounds<DevicePixels>> {
        self.rects.sort_by_key(|rect| std::cmp::Reverse(area(rect)));
        let mut kept: Vec<Bounds<DevicePixels>> = Vec::new();
        for rect in self.rects {
            if !kept.iter().any(|existing| contains(existing, &rect)) {
                kept.push(rect);
            }
        }
        if kept.len() > MAX_OCCLUSION_RECTS {
            let left = kept.iter().map(|rect| rect.origin.x.0).min().unwrap_or(0);
            let top = kept.iter().map(|rect| rect.origin.y.0).min().unwrap_or(0);
            let right = kept
                .iter()
                .map(|rect| rect.origin.x.0 + rect.size.width.0)
                .max()
                .unwrap_or(0);
            let bottom = kept
                .iter()
                .map(|rect| rect.origin.y.0 + rect.size.height.0)
                .max()
                .unwrap_or(0);
            return vec![Bounds {
                origin: Point::new(DevicePixels(left), DevicePixels(top)),
                size: Size {
                    width: DevicePixels(right - left),
                    height: DevicePixels(bottom - top),
                },
            }];
        }
        kept
    }
}

/// The occlusion rectangles of `scenes` taken together, in device pixels.
pub fn collect_occlusion_rects<'a>(
    scenes: impl IntoIterator<Item = &'a crate::Scene>,
    opaque_only: bool,
) -> Vec<Bounds<DevicePixels>> {
    let mut collector = OcclusionCollector::new(opaque_only);
    for scene in scenes {
        collector.add_scene(scene);
    }
    collector.finish()
}

/// The framework layers of `composed` that stack above `native` in tree order. Each native
/// child is cut only by these.
pub fn layers_above<'a>(
    layout: &CompositionLayout,
    composed: &'a ComposedScene,
    native: SurfaceId,
) -> Vec<&'a ComposedLayer> {
    let Some(native_position) = layout.position(native) else {
        return Vec::new();
    };
    composed
        .layers
        .iter()
        .filter(|layer| {
            layout
                .position(layer.surface)
                .is_some_and(|position| position > native_position)
        })
        .collect()
}

/// The occlusion rectangles to cut out of one native child's shapes in `mode`: the layers
/// above it in tree order, filtered to opaque coverage only in composited mode.
pub fn native_cutout(
    layout: &CompositionLayout,
    composed: &ComposedScene,
    native: SurfaceId,
    mode: X11CompositionMode,
) -> Vec<Bounds<DevicePixels>> {
    collect_occlusion_rects(
        layers_above(layout, composed, native)
            .into_iter()
            .map(|layer| &layer.scene),
        mode.opaque_only(),
    )
}

/// Remembers the previous frame's occlusion data so the backend only issues shape requests
/// when it changed.
#[derive(Debug, Default)]
pub struct OcclusionTracker {
    last: Option<Vec<Bounds<DevicePixels>>>,
}

impl OcclusionTracker {
    /// Records `rects` and returns them if they differ from the previous frame's data (the
    /// first call always counts as a change).
    pub fn update(&mut self, rects: Vec<Bounds<DevicePixels>>) -> Option<&[Bounds<DevicePixels>]> {
        if self.last.as_ref() == Some(&rects) {
            return None;
        }
        self.last = Some(rects);
        self.last.as_deref()
    }

    /// Forgets the previous data, for example after the native child was recreated.
    pub fn invalidate(&mut self) {
        self.last = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ContentMask, Corners, Edges, Hsla, Quad, Scene, Shadow, Underline, black, hsla, rgb,
    };

    fn bounds(x: f32, y: f32, w: f32, h: f32) -> Bounds<ScaledPixels> {
        Bounds {
            origin: Point::new(ScaledPixels(x), ScaledPixels(y)),
            size: Size {
                width: ScaledPixels(w),
                height: ScaledPixels(h),
            },
        }
    }

    fn mask(bounds: Bounds<ScaledPixels>) -> ContentMask<ScaledPixels> {
        ContentMask { bounds }
    }

    fn quad(rect: Bounds<ScaledPixels>, background: impl Into<crate::Background>) -> Primitive {
        Primitive::Quad(Quad {
            bounds: rect,
            content_mask: mask(bounds(0.0, 0.0, 10_000.0, 10_000.0)),
            background: background.into(),
            border_color: Hsla::default(),
            corner_radii: Corners::default(),
            border_widths: Edges::default(),
            ..Default::default()
        })
    }

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Bounds<DevicePixels> {
        Bounds {
            origin: point(DevicePixels(x), DevicePixels(y)),
            size: size(DevicePixels(w), DevicePixels(h)),
        }
    }

    #[test]
    fn composited_mode_needs_argb_and_xwayland_or_a_compositing_manager() {
        use X11CompositionMode::*;
        let facts = |argb_visual, xwayland, compositing_manager| X11CompositingFacts {
            argb_visual,
            xwayland,
            compositing_manager,
        };
        assert_eq!(
            decide_x11_composition_mode(facts(true, true, false)),
            Composited
        );
        assert_eq!(
            decide_x11_composition_mode(facts(true, false, true)),
            Composited
        );
        assert_eq!(
            decide_x11_composition_mode(facts(true, true, true)),
            Composited
        );
        assert_eq!(
            decide_x11_composition_mode(facts(true, false, false)),
            ShapeFallback
        );
        assert_eq!(
            decide_x11_composition_mode(facts(false, true, true)),
            ShapeFallback
        );
        assert_eq!(
            decide_x11_composition_mode(X11CompositingFacts::default()),
            ShapeFallback
        );
    }

    #[test]
    fn missing_shape_extension_is_an_error() {
        assert!(require_x11_shape_extension(true).is_ok());
        let error = require_x11_shape_extension(false).unwrap_err();
        assert!(error.to_string().contains("SHAPE"), "{error}");
    }

    #[test]
    fn overlay_window_exists_only_for_content_or_input() {
        assert!(!overlay_window_needed(false, false));
        assert!(overlay_window_needed(true, false));
        assert!(overlay_window_needed(false, true));
        assert!(overlay_window_needed(true, true));
    }

    #[test]
    fn input_shape_is_empty_outside_hitboxes_and_floor_ceil_inside() {
        assert!(overlay_input_shape(&[], 2.0).is_empty());
        let region = [Bounds {
            origin: Point::new(crate::px(10.25), crate::px(5.0)),
            size: Size {
                width: crate::px(20.0),
                height: crate::px(10.1),
            },
        }];
        // 20.5..60.5 x 10..30.2 at scale 2.
        assert_eq!(overlay_input_shape(&region, 2.0), [rect(20, 10, 41, 21)]);
    }

    #[test]
    fn rectangles_use_floor_origin_and_ceil_far_corner_and_drop_empty() {
        let mut collector = OcclusionCollector::new(false);
        collector.add_primitive(&quad(bounds(1.5, 2.25, 3.0, 3.0), black()));
        collector.add_primitive(&quad(bounds(5.0, 5.0, 0.0, 4.0), black()));
        assert_eq!(collector.finish(), [rect(1, 2, 4, 4)]);
    }

    #[test]
    fn shadows_never_contribute_and_translucent_quads_depend_on_the_variant() {
        let shadow = Primitive::Shadow(Shadow {
            bounds: bounds(0.0, 0.0, 50.0, 50.0),
            content_mask: mask(bounds(0.0, 0.0, 100.0, 100.0)),
            color: black(),
            blur_radius: ScaledPixels(4.0),
            ..Default::default()
        });
        let translucent = quad(bounds(10.0, 10.0, 20.0, 20.0), hsla(0.0, 0.0, 0.0, 0.5));
        let opaque = quad(bounds(40.0, 40.0, 10.0, 10.0), rgb(0x2868dc));
        let gradient = quad(
            bounds(60.0, 60.0, 10.0, 10.0),
            crate::linear_gradient(
                90.0,
                crate::linear_color_stop(rgb(0xff0000), 0.0),
                crate::linear_color_stop(rgb(0x0000ff), 1.0),
            ),
        );
        let translucent_gradient = quad(
            bounds(80.0, 80.0, 10.0, 10.0),
            crate::linear_gradient(
                90.0,
                crate::linear_color_stop(rgb(0xff0000), 0.0),
                crate::linear_color_stop(hsla(0.0, 0.0, 0.0, 0.5), 1.0),
            ),
        );
        for (opaque_only, expected) in [
            (true, vec![rect(40, 40, 10, 10), rect(60, 60, 10, 10)]),
            (
                false,
                vec![
                    rect(10, 10, 20, 20),
                    rect(80, 80, 10, 10),
                    rect(40, 40, 10, 10),
                    rect(60, 60, 10, 10),
                ],
            ),
        ] {
            let mut collector = OcclusionCollector::new(opaque_only);
            for primitive in [
                &shadow,
                &translucent,
                &opaque,
                &gradient,
                &translucent_gradient,
            ] {
                collector.add_primitive(primitive);
            }
            let mut got = collector.finish();
            let mut expected = expected;
            got.sort_by_key(|rect| (rect.origin.x.0, rect.origin.y.0));
            expected.sort_by_key(|rect| (rect.origin.x.0, rect.origin.y.0));
            assert_eq!(got, expected, "opaque_only = {opaque_only}");
        }
    }

    #[test]
    fn rectangles_are_clipped_by_their_content_mask() {
        let mut primitive = quad(bounds(0.0, 0.0, 100.0, 100.0), black());
        if let Primitive::Quad(quad) = &mut primitive {
            quad.content_mask = mask(bounds(10.0, 10.0, 20.0, 20.0));
        }
        let mut collector = OcclusionCollector::new(false);
        collector.add_primitive(&primitive);
        assert_eq!(collector.finish(), [rect(10, 10, 20, 20)]);
    }

    #[test]
    fn group_descent_clips_to_the_group_and_metadata_never_occludes() {
        let mut collector = OcclusionCollector::new(false);
        // Invisible metadata: a group with no drawable content contributes nothing, however
        // large its bounds are.
        collector.begin_group(bounds(0.0, 0.0, 500.0, 500.0));
        collector.end_group();
        // Drawable content in a rounded group is bounded by the group's enclosing bounds,
        // not by the group marker itself.
        collector.begin_group(bounds(100.0, 100.0, 50.0, 50.0));
        collector.add_primitive(&quad(bounds(90.0, 90.0, 200.0, 40.0), black()));
        collector.begin_group(bounds(120.0, 0.0, 400.0, 400.0));
        collector.add_primitive(&quad(bounds(0.0, 0.0, 1000.0, 1000.0), black()));
        collector.end_group();
        collector.end_group();
        // Outside the group the full rectangle counts again.
        collector.add_primitive(&quad(bounds(300.0, 300.0, 10.0, 10.0), black()));
        assert_eq!(
            collector.finish(),
            [
                rect(100, 100, 50, 30),
                rect(120, 100, 30, 50),
                rect(300, 300, 10, 10)
            ]
        );
        // The group marker alone, with nothing inside, leaves no cutout at all.
        let mut empty = OcclusionCollector::new(false);
        empty.begin_group(bounds(0.0, 0.0, 500.0, 500.0));
        empty.end_group();
        assert!(empty.finish().is_empty());
    }

    #[test]
    fn empty_scope_markers_leave_no_cutout() {
        let mut scene = Scene::default();
        scene.push_layer(bounds(0.0, 0.0, 800.0, 600.0));
        scene.pop_layer();
        assert!(collect_occlusion_rects([&scene], false).is_empty());
        assert!(collect_occlusion_rects([&scene], true).is_empty());
    }

    #[test]
    fn contained_rectangles_are_dropped_and_larger_ones_come_first() {
        let mut collector = OcclusionCollector::new(false);
        collector.add_primitive(&quad(bounds(10.0, 10.0, 5.0, 5.0), black()));
        collector.add_primitive(&quad(bounds(0.0, 0.0, 100.0, 100.0), black()));
        collector.add_primitive(&quad(bounds(90.0, 90.0, 30.0, 30.0), black()));
        collector.add_primitive(&Primitive::Underline(Underline {
            bounds: bounds(20.0, 20.0, 10.0, 1.0),
            content_mask: mask(bounds(0.0, 0.0, 1000.0, 1000.0)),
            ..Default::default()
        }));
        assert_eq!(
            collector.finish(),
            [rect(0, 0, 100, 100), rect(90, 90, 30, 30)]
        );
    }

    #[test]
    fn more_than_256_rectangles_collapse_to_the_bounding_union() {
        let mut collector = OcclusionCollector::new(false);
        for index in 0..257 {
            collector.add_primitive(&quad(
                bounds(index as f32 * 4.0, (index % 3) as f32 * 5.0, 2.0, 2.0),
                black(),
            ));
        }
        let rects = collector.finish();
        assert_eq!(rects, [rect(0, 0, 256 * 4 + 2, 12)]);

        let mut at_limit = OcclusionCollector::new(false);
        for index in 0..256 {
            at_limit.add_primitive(&quad(bounds(index as f32 * 4.0, 0.0, 2.0, 2.0), black()));
        }
        assert_eq!(at_limit.finish().len(), 256);
    }

    #[test]
    fn occlusion_data_updates_only_when_it_changes() {
        let mut tracker = OcclusionTracker::default();
        assert!(tracker.update(vec![rect(0, 0, 10, 10)]).is_some());
        assert!(tracker.update(vec![rect(0, 0, 10, 10)]).is_none());
        assert_eq!(
            tracker.update(vec![rect(0, 0, 11, 10)]),
            Some([rect(0, 0, 11, 10)].as_slice())
        );
        tracker.invalidate();
        assert!(tracker.update(vec![rect(0, 0, 11, 10)]).is_some());
    }
}
