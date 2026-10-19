// todo("windows"): remove
#![cfg_attr(windows, allow(dead_code))]

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    AtlasTextureId, AtlasTile, Background, Bounds, ContentMask, Corners, CurvedClipId,
    CurvedClipShape, Edges, Hsla, Pixels, Point, Radians, ScaledPixels, ScopeParams, Size, point,
    scene_order::{OrderIndex, SceneOrderStats},
    scene_sort::{SortScratch, sort_records},
};
use std::{
    fmt::Debug,
    iter::Peekable,
    ops::{Add, Range, Sub},
    slice,
};

#[allow(non_camel_case_types, unused)]
#[expect(missing_docs)]
pub type PathVertex_ScaledPixels = PathVertex<ScaledPixels>;

#[expect(missing_docs)]
pub type DrawOrder = u32;

/// A boolean stored as a `u32` so that GPU-facing structs contain no
/// compiler-inserted padding bytes, which would be undefined behavior to
/// reinterpret as `&[u8]` when writing instance buffers. Guaranteed to be
/// `0` or `1` by construction; shaders read it as a `u32`/`uint`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(transparent)]
pub struct PaddedBool32(u32);

impl From<bool> for PaddedBool32 {
    fn from(value: bool) -> Self {
        PaddedBool32(value as u32)
    }
}

#[derive(Default)]
#[expect(missing_docs)]
pub struct Scene {
    pub(crate) paint_operations: Vec<PaintOperation>,
    order_index: OrderIndex,
    sort_scratch: SortScratch,
    layer_stack: Vec<DrawOrder>,
    pub shadows: Vec<Shadow>,
    pub quads: Vec<Quad>,
    pub paths: Vec<Path<ScaledPixels>>,
    pub underlines: Vec<Underline>,
    pub surfaces: Vec<PaintSurface>,
    pub polychrome_sprites: Vec<PolychromeSprite>,
    pub subpixel_sprites: Vec<SubpixelSprite>,
    pub monochrome_sprites: Vec<MonochromeSprite>,
    /// Backdrop-blur effect records, ordered with the primitives (see [`BackdropBlur`]).
    pub backdrop_blurs: Vec<BackdropBlur>,
    /// Curved overflow clip shapes referenced by [`CurvedClipId`] (index + 1).
    pub curved_clips: Vec<CurvedClipShape>,
    /// Parent chain node (`0` for none) of each entry of `curved_clips`.
    pub curved_clip_parents: Vec<CurvedClipId>,
    curved_clip_stack: Vec<CurvedClipScope>,
}

#[expect(missing_docs)]
impl Scene {
    pub fn clear(&mut self) {
        self.paint_operations.clear();
        self.order_index.begin_frame();
        self.layer_stack.clear();
        self.curved_clip_stack.clear();
        self.curved_clip_parents.clear();
        self.curved_clips.clear();
        self.backdrop_blurs.clear();
        self.surfaces.clear();
        self.polychrome_sprites.clear();
        self.subpixel_sprites.clear();
        self.monochrome_sprites.clear();
        self.underlines.clear();
        self.paths.clear();
        self.quads.clear();
        self.shadows.clear();
    }

    pub fn len(&self) -> usize {
        self.paint_operations.len()
    }

    pub fn push_layer(&mut self, bounds: Bounds<ScaledPixels>) {
        let order = self.order_index.insert(bounds);
        self.layer_stack.push(order);
        self.paint_operations
            .push(PaintOperation::StartLayer(bounds));
    }

    pub fn pop_layer(&mut self) {
        self.layer_stack.pop();
        self.paint_operations.push(PaintOperation::EndLayer);
    }

    pub fn insert_primitive(&mut self, primitive: impl Into<Primitive>) {
        let mut primitive = primitive.into();
        let curved_clip = self.current_curved_clip();
        primitive.set_curved_clip(curved_clip);
        let clipped_bounds = primitive
            .painted_bounds()
            .intersect(&primitive.content_mask().bounds);

        if clipped_bounds.is_empty() {
            return;
        }

        let order = self.next_order(clipped_bounds);
        match &mut primitive {
            Primitive::Shadow(shadow) => {
                shadow.order = order;
                self.shadows.push(*shadow);
            }
            Primitive::Quad(quad) => {
                quad.order = order;
                self.quads.push(*quad);
            }
            Primitive::Path(path) => {
                path.order = order;
                path.id = PathId(self.paths.len());
                self.paths.push(path.clone());
            }
            Primitive::Underline(underline) => {
                underline.order = order;
                self.underlines.push(*underline);
            }
            Primitive::MonochromeSprite(sprite) => {
                sprite.order = order;
                self.monochrome_sprites.push(*sprite);
            }
            Primitive::SubpixelSprite(sprite) => {
                sprite.order = order;
                self.subpixel_sprites.push(*sprite);
            }
            Primitive::PolychromeSprite(sprite) => {
                sprite.order = order;
                self.polychrome_sprites.push(*sprite);
            }
            Primitive::Surface(surface) => {
                surface.order = order;
                self.surfaces.push(surface.clone());
            }
        }
        self.paint_operations
            .push(PaintOperation::Primitive(primitive));
    }

    /// Inserts a backdrop-blur effect record together with its invisible ordering primitive: a
    /// fully transparent drop shadow with zero blur and the effect's bounds, radii and content
    /// mask, which gets the same order as the effect so that it produces a batch boundary
    /// exactly at the effect's order. The order is assigned exactly like an ordinary
    /// primitive's covering the same clipped bounds (the layer's order inside a paint layer).
    /// Effects whose clipped bounds are empty and effects with a non-positive blur radius are
    /// dropped. Replaying the recorded operation reproduces both records.
    pub fn insert_backdrop_blur(&mut self, mut blur: BackdropBlur) {
        let clipped_bounds = blur.bounds.intersect(&blur.content_mask.bounds);
        if clipped_bounds.is_empty() || blur.blur_radius.0.is_nan() || blur.blur_radius.0 <= 0.0 {
            return;
        }
        blur.order = self.next_order(clipped_bounds);
        self.shadows.push(Shadow {
            order: blur.order,
            blur_radius: ScaledPixels(0.0),
            bounds: blur.bounds,
            corner_radii: blur.corner_radii,
            content_mask: blur.content_mask,
            color: Hsla::transparent_black(),
            element_bounds: blur.bounds,
            element_corner_radii: blur.corner_radii,
            inset: 0,
            curved_clip: 0,
        });
        self.backdrop_blurs.push(blur);
        self.paint_operations
            .push(PaintOperation::BackdropBlur(blur));
    }

    /// Replays `range` of `prev_scene`'s paint operations into this scene.
    pub fn replay(&mut self, range: Range<usize>, prev_scene: &Scene) {
        self.replay_translated(range, prev_scene, Point::default());
    }

    /// Replays `range` of `prev_scene`'s paint operations into this scene, moving the curved
    /// overflow clip shapes the replayed range itself opens by `offset` (device pixels).
    ///
    /// Shapes opened before the range, by enclosing elements, are not part of the range:
    /// replayed primitives take the destination's current chain of enclosing shapes. Primitive
    /// geometry is replayed as recorded; callers that translate the primitives translate by
    /// the same `offset` here.
    pub fn replay_translated(
        &mut self,
        range: Range<usize>,
        prev_scene: &Scene,
        offset: Point<ScaledPixels>,
    ) {
        for operation in &prev_scene.paint_operations[range] {
            match operation {
                PaintOperation::Primitive(primitive) => self.insert_primitive(primitive.clone()),
                PaintOperation::StartLayer(bounds) => self.push_layer(*bounds),
                PaintOperation::BackdropBlur(blur) => self.insert_backdrop_blur(*blur),
                PaintOperation::EndLayer => self.pop_layer(),
                PaintOperation::PushCurvedClip(shape) => {
                    self.push_curved_clip(shape.translated(offset.x.0, offset.y.0))
                }
                PaintOperation::PopCurvedClip => self.pop_curved_clip(),
            }
        }
    }

    /// Replays `range` of `source`'s paint operations moved by `offset` device pixels, with
    /// every content mask moved along and intersected with `clip` (when given). Primitives
    /// that end up outside their mask are dropped, as when they are first inserted.
    pub fn replay_moved(
        &mut self,
        range: Range<usize>,
        source: &Scene,
        offset: Point<ScaledPixels>,
        clip: Option<Bounds<ScaledPixels>>,
    ) {
        for operation in &source.paint_operations[range] {
            match operation {
                PaintOperation::Primitive(primitive) => {
                    self.insert_primitive(primitive.translated(offset, clip))
                }
                PaintOperation::StartLayer(bounds) => {
                    let bounds = moved(*bounds, offset);
                    self.push_layer(clip.map_or(bounds, |clip| bounds.intersect(&clip)))
                }
                PaintOperation::BackdropBlur(blur) => {
                    let mut blur = *blur;
                    blur.bounds = moved(blur.bounds, offset);
                    blur.content_mask = moved_mask(blur.content_mask, offset, clip);
                    self.insert_backdrop_blur(blur)
                }
                PaintOperation::EndLayer => self.pop_layer(),
                PaintOperation::PushCurvedClip(shape) => {
                    self.push_curved_clip(shape.translated(offset.x.0, offset.y.0))
                }
                PaintOperation::PopCurvedClip => self.pop_curved_clip(),
            }
        }
    }

    /// Whether any recorded primitive carries scope coordinates that may belong to an
    /// enclosing element (see [`Primitive::has_active_scope`]), or a curved clip that was
    /// opened outside the recording.
    pub fn has_active_scope(&self) -> bool {
        self.paint_operations
            .iter()
            .any(|operation| match operation {
                PaintOperation::Primitive(primitive) => primitive.has_active_scope(),
                _ => false,
            })
    }

    /// The order of the next primitive with the given (nonempty, clipped) bounds: the order of
    /// the innermost explicit layer, otherwise one above every earlier overlapping bounds.
    fn next_order(&mut self, clipped_bounds: Bounds<ScaledPixels>) -> DrawOrder {
        match self.layer_stack.last() {
            Some(order) => *order,
            None => self.order_index.insert(clipped_bounds),
        }
    }

    /// Uses the order stream of `previous_frame` as the baseline for replaying orders in this
    /// scene. Call right after [`Scene::clear`] when two scenes alternate between frames.
    pub fn adopt_order_baseline(&mut self, previous_frame: &Scene) {
        self.order_index.adopt_baseline(&previous_frame.order_index);
    }

    /// Counters describing how the orders of the current frame were assigned.
    pub fn order_stats(&self) -> SceneOrderStats {
        self.order_index.stats()
    }

    /// Number of times sort scratch storage had to grow since this scene was created.
    pub fn sort_scratch_allocations(&self) -> u64 {
        self.sort_scratch.index.allocations()
    }

    /// Opens a curved overflow clip shape: every primitive inserted until the matching
    /// [`Self::pop_curved_clip`] is cut by it, together with every enclosing shape.
    /// Shapes enter the GPU data only when a primitive is painted under them.
    pub fn push_curved_clip(&mut self, shape: CurvedClipShape) {
        self.curved_clip_stack
            .push(CurvedClipScope { shape, node: 0 });
        self.paint_operations
            .push(PaintOperation::PushCurvedClip(shape));
    }

    /// Closes the innermost open curved overflow clip shape.
    pub fn pop_curved_clip(&mut self) {
        self.curved_clip_stack.pop();
        self.paint_operations.push(PaintOperation::PopCurvedClip);
    }

    /// The chain node of the innermost open curved clip shape, materializing the chain on
    /// first use; `0` when no shape is open.
    pub fn current_curved_clip(&mut self) -> CurvedClipId {
        let Some(last) = self.curved_clip_stack.last() else {
            return 0;
        };
        if last.node != 0 {
            return last.node;
        }
        let first_new = self
            .curved_clip_stack
            .partition_point(|scope| scope.node != 0);
        let mut parent = first_new
            .checked_sub(1)
            .map_or(0, |index| self.curved_clip_stack[index].node);
        for scope in &mut self.curved_clip_stack[first_new..] {
            self.curved_clips.push(scope.shape);
            self.curved_clip_parents.push(parent);
            scope.node = self.curved_clips.len() as CurvedClipId;
            parent = scope.node;
        }
        parent
    }

    /// Whether the scene holds nothing drawable: no primitive and no backdrop blur. Layer
    /// (and other scope) markers recorded without any primitive between them never make a
    /// scene drawable, so a scene that only contains empty scopes is empty.
    pub fn is_empty(&self) -> bool {
        let primitive_counts = [
            self.shadows.len(),
            self.quads.len(),
            self.paths.len(),
            self.underlines.len(),
            self.monochrome_sprites.len(),
            self.subpixel_sprites.len(),
            self.polychrome_sprites.len(),
            self.surfaces.len(),
            self.backdrop_blurs.len(),
        ];
        primitive_counts.iter().all(|&count| count == 0)
    }

    /// Replays `range` of `prev_scene`'s paint operations into this scene with its scopes
    /// balanced: scopes (layers) that were opened before the range and are still open at its
    /// start are opened first, in their original nesting order, and scopes the range leaves
    /// open are closed at its end. Fails, without touching the scene, when the operations
    /// cannot be balanced: a scope is closed that was never opened, or closes a scope of
    /// another kind than the innermost open one.
    pub fn replay_balanced(
        &mut self,
        range: Range<usize>,
        prev_scene: &Scene,
    ) -> Result<(), UnbalancedScopes> {
        let operations = &prev_scene.paint_operations;
        if range.start > range.end || range.end > operations.len() {
            return Err(UnbalancedScopes {
                operation_index: range.end.min(operations.len()),
            });
        }
        let mut tracker = ScopeTracker::default();
        for (index, operation) in operations[..range.start].iter().enumerate() {
            tracker.apply(index, operation)?;
        }
        let open = tracker.stack.clone();
        for (offset, operation) in operations[range.clone()].iter().enumerate() {
            tracker.apply(range.start + offset, operation)?;
        }
        self.replay_range_with_scopes(&operations[range], &open, &tracker.stack);
        Ok(())
    }

    /// Splits the paint stream into one scene per target. `ranges` lists, in stream order and
    /// covering `0..self.len()` without gaps, which target (an index below `targets`) each
    /// stretch of paint operations belongs to. Repeated stretches of one target keep their
    /// relative order and every stretch is replayed with balanced scopes (see
    /// [`Scene::replay_balanced`]). The returned scenes are finished.
    pub fn split_ranges(
        &self,
        ranges: &[(usize, Range<usize>)],
        targets: usize,
    ) -> Result<Vec<Scene>, UnbalancedScopes> {
        let mut scenes: Vec<Scene> = (0..targets).map(|_| Scene::default()).collect();
        let mut tracker = ScopeTracker::default();
        for (target, range) in ranges {
            let operations = &self.paint_operations;
            if *target >= targets || range.start > range.end || range.end > operations.len() {
                return Err(UnbalancedScopes {
                    operation_index: range.start,
                });
            }
            let open = tracker.stack.clone();
            for (offset, operation) in operations[range.clone()].iter().enumerate() {
                tracker.apply(range.start + offset, operation)?;
            }
            scenes[*target].replay_range_with_scopes(
                &operations[range.clone()],
                &open,
                &tracker.stack,
            );
        }
        for scene in &mut scenes {
            scene.finish();
        }
        Ok(scenes)
    }

    /// Splits the paint stream at `overlay_start` into the base scene (operations before the
    /// index) and the overlay scene (operations from it on). An index past the end of the
    /// stream clamps to its length, which leaves the overlay empty. Both scenes are finished.
    pub fn split_at_overlay(
        &self,
        overlay_start: usize,
    ) -> Result<(Scene, Scene), UnbalancedScopes> {
        let start = overlay_start.min(self.len());
        let mut scenes = self
            .split_ranges(&[(0, 0..start), (1, start..self.len())], 2)?
            .into_iter();
        let base = scenes.next().unwrap_or_default();
        let overlay = scenes.next().unwrap_or_default();
        Ok((base, overlay))
    }

    /// Replays `operations`, reopening `open` scopes first and closing `still_open` scopes
    /// last. The scopes were validated by a [`ScopeTracker`] pass over the same operations.
    fn replay_range_with_scopes(
        &mut self,
        operations: &[PaintOperation],
        open: &[ScopeMarker],
        still_open: &[ScopeMarker],
    ) {
        for marker in open {
            marker.open(self);
        }
        for operation in operations {
            match operation {
                PaintOperation::Primitive(primitive) => self.insert_primitive(primitive.clone()),
                PaintOperation::StartLayer(bounds) => self.push_layer(*bounds),
                PaintOperation::BackdropBlur(blur) => self.insert_backdrop_blur(*blur),
                PaintOperation::EndLayer => self.pop_layer(),
                PaintOperation::PushCurvedClip(shape) => self.push_curved_clip(*shape),
                PaintOperation::PopCurvedClip => self.pop_curved_clip(),
            }
        }
        for marker in still_open.iter().rev() {
            marker.close(self);
        }
    }

    pub fn finish(&mut self) {
        let SortScratch { index, gather } = &mut self.sort_scratch;
        sort_records(&mut self.shadows, &mut gather.shadows, index);
        sort_records(&mut self.quads, &mut gather.quads, index);
        sort_records(&mut self.paths, &mut gather.paths, index);
        sort_records(&mut self.underlines, &mut gather.underlines, index);
        sort_records(
            &mut self.monochrome_sprites,
            &mut gather.monochrome_sprites,
            index,
        );
        sort_records(
            &mut self.subpixel_sprites,
            &mut gather.subpixel_sprites,
            index,
        );
        sort_records(
            &mut self.polychrome_sprites,
            &mut gather.polychrome_sprites,
            index,
        );
        sort_records(&mut self.surfaces, &mut gather.surfaces, index);
        // Stable: effects with equal order are processed in insertion order.
        self.backdrop_blurs.sort_by_key(|effect| effect.order);
    }

    #[cfg_attr(
        all(
            any(target_os = "linux", target_os = "freebsd"),
            not(any(feature = "x11", feature = "wayland"))
        ),
        allow(dead_code)
    )]
    pub fn batches(&self) -> impl Iterator<Item = PrimitiveBatch> + '_ {
        BatchIterator {
            blurs: &self.backdrop_blurs,
            blur_cursor: 0,
            surfaces_start: 0,
            surfaces_iter: self.surfaces.iter().peekable(),
            polychrome_sprites_start: 0,
            polychrome_sprites_iter: self.polychrome_sprites.iter().peekable(),
            subpixel_sprites_start: 0,
            subpixel_sprites_iter: self.subpixel_sprites.iter().peekable(),
            monochrome_sprites_start: 0,
            monochrome_sprites_iter: self.monochrome_sprites.iter().peekable(),
            underlines_start: 0,
            underlines_iter: self.underlines.iter().peekable(),
            paths_start: 0,
            paths_iter: self.paths.iter().peekable(),
            quads_start: 0,
            quads_iter: self.quads.iter().peekable(),
            shadows_start: 0,
            shadows_iter: self.shadows.iter().peekable(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Default)]
#[cfg_attr(
    all(
        any(target_os = "linux", target_os = "freebsd"),
        not(any(feature = "x11", feature = "wayland"))
    ),
    allow(dead_code)
)]
pub(crate) enum PrimitiveKind {
    Shadow,
    #[default]
    Quad,
    Path,
    Underline,
    MonochromeSprite,
    SubpixelSprite,
    PolychromeSprite,
    Surface,
}

#[derive(Clone, Copy)]
struct CurvedClipScope {
    shape: CurvedClipShape,
    node: CurvedClipId,
}

pub(crate) enum PaintOperation {
    Primitive(Primitive),
    StartLayer(Bounds<ScaledPixels>),
    EndLayer,
    BackdropBlur(BackdropBlur),
    PushCurvedClip(CurvedClipShape),
    PopCurvedClip,
}

/// A paint operation range could not be replayed with balanced scopes: it closes a scope that
/// was never opened, or closes a scope of another kind than the innermost open one. A frame
/// whose paint stream cannot be balanced is not drawn, because drawing it would apply
/// clipping to the wrong primitives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error(
    "paint operation {operation_index} cannot be balanced: it closes a scope that is not the innermost open scope"
)]
pub struct UnbalancedScopes {
    /// Index of the paint operation that cannot be balanced.
    pub operation_index: usize,
}

/// An open scope in the paint stream: it applies to every primitive recorded until it is
/// closed. Replaying a part of a stream reopens the scopes that enclose the part.
#[derive(Clone, Copy, Debug, PartialEq)]
enum ScopeMarker {
    /// A paint layer (see [`Scene::push_layer`]).
    Layer(Bounds<ScaledPixels>),
    /// A curved overflow clip shape (see [`Scene::push_curved_clip`]).
    CurvedClip(CurvedClipShape),
}

/// What a paint operation does to the stack of open scopes.
enum ScopeEvent {
    Open(ScopeMarker),
    Close(ScopeKind),
}

/// The kind of a scope, so that a close can be checked against the scope it closes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScopeKind {
    Layer,
    CurvedClip,
}

impl ScopeMarker {
    fn kind(&self) -> ScopeKind {
        match self {
            ScopeMarker::Layer(_) => ScopeKind::Layer,
            ScopeMarker::CurvedClip(_) => ScopeKind::CurvedClip,
        }
    }

    fn open(&self, scene: &mut Scene) {
        match self {
            ScopeMarker::Layer(bounds) => scene.push_layer(*bounds),
            ScopeMarker::CurvedClip(shape) => scene.push_curved_clip(*shape),
        }
    }

    fn close(&self, scene: &mut Scene) {
        match self {
            ScopeMarker::Layer(_) => scene.pop_layer(),
            ScopeMarker::CurvedClip(_) => scene.pop_curved_clip(),
        }
    }
}

impl PaintOperation {
    fn scope_event(&self) -> Option<ScopeEvent> {
        match self {
            PaintOperation::StartLayer(bounds) => {
                Some(ScopeEvent::Open(ScopeMarker::Layer(*bounds)))
            }
            PaintOperation::EndLayer => Some(ScopeEvent::Close(ScopeKind::Layer)),
            PaintOperation::PushCurvedClip(shape) => {
                Some(ScopeEvent::Open(ScopeMarker::CurvedClip(*shape)))
            }
            PaintOperation::PopCurvedClip => Some(ScopeEvent::Close(ScopeKind::CurvedClip)),
            PaintOperation::Primitive(_) | PaintOperation::BackdropBlur(_) => None,
        }
    }
}

/// Follows the stack of open scopes through a paint stream.
#[derive(Default)]
struct ScopeTracker {
    stack: Vec<ScopeMarker>,
}

impl ScopeTracker {
    fn apply(&mut self, index: usize, operation: &PaintOperation) -> Result<(), UnbalancedScopes> {
        match operation.scope_event() {
            None => Ok(()),
            Some(ScopeEvent::Open(marker)) => {
                self.stack.push(marker);
                Ok(())
            }
            Some(ScopeEvent::Close(kind)) => match self.stack.last() {
                Some(top) if top.kind() == kind => {
                    self.stack.pop();
                    Ok(())
                }
                _ => Err(UnbalancedScopes {
                    operation_index: index,
                }),
            },
        }
    }
}

#[derive(Clone)]
#[expect(missing_docs)]
pub enum Primitive {
    Shadow(Shadow),
    Quad(Quad),
    Path(Path<ScaledPixels>),
    Underline(Underline),
    MonochromeSprite(MonochromeSprite),
    SubpixelSprite(SubpixelSprite),
    PolychromeSprite(PolychromeSprite),
    Surface(PaintSurface),
}

#[expect(missing_docs)]
impl Primitive {
    pub fn bounds(&self) -> &Bounds<ScaledPixels> {
        match self {
            Primitive::Shadow(shadow) => &shadow.bounds,
            Primitive::Quad(quad) => &quad.bounds,
            Primitive::Path(path) => &path.bounds,
            Primitive::Underline(underline) => &underline.bounds,
            Primitive::MonochromeSprite(sprite) => &sprite.bounds,
            Primitive::SubpixelSprite(sprite) => &sprite.bounds,
            Primitive::PolychromeSprite(sprite) => &sprite.bounds,
            Primitive::Surface(surface) => &surface.bounds,
        }
    }

    /// The device-space rectangle the primitive covers once painted: its bounds, or for a
    /// monochrome sprite with a non-identity transformation (RND-005) the axis-aligned box
    /// of the transformed quad. Ordering and content-mask culling use this rectangle so a
    /// translated or rotated sprite is neither culled nor ordered by where it was recorded.
    pub fn painted_bounds(&self) -> Bounds<ScaledPixels> {
        let Primitive::MonochromeSprite(sprite) = self else {
            return *self.bounds();
        };
        if sprite.transformation == TransformationMatrix::unit() {
            return sprite.bounds;
        }
        let [[a, b], [c, d]] = sprite.transformation.rotation_scale;
        let [tx, ty] = sprite.transformation.translation;
        let (left, top) = (sprite.bounds.origin.x.0, sprite.bounds.origin.y.0);
        let (right, bottom) = (
            left + sprite.bounds.size.width.0,
            top + sprite.bounds.size.height.0,
        );
        let mut min = [f32::INFINITY; 2];
        let mut max = [f32::NEG_INFINITY; 2];
        for (x, y) in [(left, top), (right, top), (left, bottom), (right, bottom)] {
            let transformed = [a * x + b * y + tx, c * x + d * y + ty];
            for axis in 0..2 {
                min[axis] = min[axis].min(transformed[axis]);
                max[axis] = max[axis].max(transformed[axis]);
            }
        }
        Bounds {
            origin: Point::new(ScaledPixels(min[0]), ScaledPixels(min[1])),
            size: Size {
                width: ScaledPixels(max[0] - min[0]),
                height: ScaledPixels(max[1] - min[1]),
            },
        }
    }

    pub fn content_mask(&self) -> &ContentMask<ScaledPixels> {
        match self {
            Primitive::Shadow(shadow) => &shadow.content_mask,
            Primitive::Quad(quad) => &quad.content_mask,
            Primitive::Path(path) => &path.content_mask,
            Primitive::Underline(underline) => &underline.content_mask,
            Primitive::MonochromeSprite(sprite) => &sprite.content_mask,
            Primitive::SubpixelSprite(sprite) => &sprite.content_mask,
            Primitive::PolychromeSprite(sprite) => &sprite.content_mask,
            Primitive::Surface(surface) => &surface.content_mask,
        }
    }

    /// Stamps the curved overflow clip chain that applies to this primitive. Surfaces keep
    /// only the rectangular content mask.
    pub fn set_curved_clip(&mut self, curved_clip: CurvedClipId) {
        match self {
            Primitive::Shadow(shadow) => shadow.curved_clip = curved_clip,
            Primitive::Quad(quad) => quad.scope.curved_clip = curved_clip,
            Primitive::Path(path) => path.curved_clip = curved_clip,
            Primitive::Underline(underline) => underline.curved_clip = curved_clip,
            Primitive::MonochromeSprite(sprite) => sprite.scope.curved_clip = curved_clip,
            Primitive::SubpixelSprite(sprite) => sprite.scope.curved_clip = curved_clip,
            Primitive::PolychromeSprite(sprite) => sprite.scope.curved_clip = curved_clip,
            Primitive::Surface(surface) => surface.curved_clip = curved_clip,
        }
    }
}

/// Moves `bounds` by `offset`.
fn moved(bounds: Bounds<ScaledPixels>, offset: Point<ScaledPixels>) -> Bounds<ScaledPixels> {
    Bounds {
        origin: bounds.origin + offset,
        size: bounds.size,
    }
}

/// Moves a content mask by `offset`, then intersects it with `clip` when there is one.
fn moved_mask(
    mask: ContentMask<ScaledPixels>,
    offset: Point<ScaledPixels>,
    clip: Option<Bounds<ScaledPixels>>,
) -> ContentMask<ScaledPixels> {
    let bounds = moved(mask.bounds, offset);
    ContentMask {
        bounds: clip.map_or(bounds, |clip| bounds.intersect(&clip)),
    }
}

impl ScopeParams {
    /// This scope moved by `offset`, for scopes that are active; inactive scopes are returned
    /// unchanged.
    fn moved(mut self, offset: Point<ScaledPixels>) -> Self {
        let (dx, dy) = (offset.x.0, offset.y.0);
        if self.fade_active() {
            self.fade_edges[0] += dy;
            self.fade_edges[1] += dy;
            self.fade_edges[2] += dx;
            self.fade_edges[3] += dx;
        }
        if self.rounded_clip_active() {
            self.clip_bounds[0] += dx;
            self.clip_bounds[1] += dy;
            self.clip_bounds[2] += dx;
            self.clip_bounds[3] += dy;
        }
        self
    }
}

impl TransformationMatrix {
    /// The matrix of a sprite that was moved by `offset` after its transformation was
    /// recorded: the transformed result moves by the same offset.
    fn moved(mut self, offset: Point<ScaledPixels>) -> Self {
        if self == Self::unit() {
            return self;
        }
        let [[a, b], [c, d]] = self.rotation_scale;
        let (dx, dy) = (offset.x.0, offset.y.0);
        // The sprite's bounds move by `offset`, so the matrix applied to the moved coordinates
        // must produce the original result plus `offset`.
        self.translation[0] += dx - (a * dx + b * dy);
        self.translation[1] += dy - (c * dx + d * dy);
        self
    }
}

impl Path<ScaledPixels> {
    fn moved(&mut self, offset: Point<ScaledPixels>, clip: Option<Bounds<ScaledPixels>>) {
        self.bounds = moved(self.bounds, offset);
        self.content_mask = moved_mask(self.content_mask, offset, clip);
        self.start = self.start + offset;
        self.current = self.current + offset;
        for vertex in &mut self.vertices {
            vertex.xy_position = vertex.xy_position + offset;
            vertex.content_mask = moved_mask(vertex.content_mask, offset, clip);
        }
    }
}

impl Primitive {
    /// Whether the primitive carries fade or rounded-clip scope coordinates. Their positions
    /// may belong to an enclosing element that does not move with the primitive, so recorded
    /// output containing such primitives is never moved.
    pub fn has_active_scope(&self) -> bool {
        match self {
            Primitive::Quad(quad) => quad.scope.is_active(),
            Primitive::MonochromeSprite(sprite) => sprite.scope.is_active(),
            Primitive::SubpixelSprite(sprite) => sprite.scope.is_active(),
            Primitive::PolychromeSprite(sprite) => sprite.scope.is_active(),
            Primitive::Shadow(_)
            | Primitive::Path(_)
            | Primitive::Underline(_)
            | Primitive::Surface(_) => false,
        }
    }

    /// A copy of the primitive moved by `offset` device pixels. The content mask moves with
    /// it and is then intersected with `clip`, when given.
    pub fn translated(
        &self,
        offset: Point<ScaledPixels>,
        clip: Option<Bounds<ScaledPixels>>,
    ) -> Primitive {
        let mut primitive = self.clone();
        match &mut primitive {
            Primitive::Shadow(shadow) => {
                shadow.bounds = moved(shadow.bounds, offset);
                shadow.element_bounds = moved(shadow.element_bounds, offset);
                shadow.content_mask = moved_mask(shadow.content_mask, offset, clip);
            }
            Primitive::Quad(quad) => {
                quad.bounds = moved(quad.bounds, offset);
                quad.content_mask = moved_mask(quad.content_mask, offset, clip);
                quad.scope = quad.scope.moved(offset);
            }
            Primitive::Path(path) => path.moved(offset, clip),
            Primitive::Underline(underline) => {
                underline.bounds = moved(underline.bounds, offset);
                underline.content_mask = moved_mask(underline.content_mask, offset, clip);
            }
            Primitive::MonochromeSprite(sprite) => {
                sprite.bounds = moved(sprite.bounds, offset);
                sprite.content_mask = moved_mask(sprite.content_mask, offset, clip);
                sprite.transformation = sprite.transformation.moved(offset);
                sprite.scope = sprite.scope.moved(offset);
            }
            Primitive::SubpixelSprite(sprite) => {
                sprite.bounds = moved(sprite.bounds, offset);
                sprite.content_mask = moved_mask(sprite.content_mask, offset, clip);
                sprite.transformation = sprite.transformation.moved(offset);
                sprite.scope = sprite.scope.moved(offset);
            }
            Primitive::PolychromeSprite(sprite) => {
                sprite.bounds = moved(sprite.bounds, offset);
                sprite.content_mask = moved_mask(sprite.content_mask, offset, clip);
                sprite.scope = sprite.scope.moved(offset);
                let effects = &mut sprite.effects;
                if effects.mask_feather > 0.0 {
                    effects.mask_bounds[0] += offset.x.0;
                    effects.mask_bounds[1] += offset.y.0;
                    effects.mask_bottom_reference += offset.y.0;
                }
            }
            Primitive::Surface(surface) => {
                surface.bounds = moved(surface.bounds, offset);
                surface.content_mask = moved_mask(surface.content_mask, offset, clip);
            }
        }
        primitive
    }
}

#[cfg_attr(
    all(
        any(target_os = "linux", target_os = "freebsd"),
        not(any(feature = "x11", feature = "wayland"))
    ),
    allow(dead_code)
)]
struct BatchIterator<'a> {
    shadows_start: usize,
    shadows_iter: Peekable<slice::Iter<'a, Shadow>>,
    quads_start: usize,
    quads_iter: Peekable<slice::Iter<'a, Quad>>,
    paths_start: usize,
    paths_iter: Peekable<slice::Iter<'a, Path<ScaledPixels>>>,
    underlines_start: usize,
    underlines_iter: Peekable<slice::Iter<'a, Underline>>,
    monochrome_sprites_start: usize,
    monochrome_sprites_iter: Peekable<slice::Iter<'a, MonochromeSprite>>,
    subpixel_sprites_start: usize,
    subpixel_sprites_iter: Peekable<slice::Iter<'a, SubpixelSprite>>,
    polychrome_sprites_start: usize,
    polychrome_sprites_iter: Peekable<slice::Iter<'a, PolychromeSprite>>,
    surfaces_start: usize,
    surfaces_iter: Peekable<slice::Iter<'a, PaintSurface>>,
    /// Backdrop-blur effects sorted by order; batches are cut at their orders.
    blurs: &'a [BackdropBlur],
    blur_cursor: usize,
}

impl<'a> Iterator for BatchIterator<'a> {
    type Item = PrimitiveBatch;

    fn next(&mut self) -> Option<Self::Item> {
        let mut orders_and_kinds = [
            (
                self.shadows_iter.peek().map(|s| s.order),
                PrimitiveKind::Shadow,
            ),
            (self.quads_iter.peek().map(|q| q.order), PrimitiveKind::Quad),
            (self.paths_iter.peek().map(|q| q.order), PrimitiveKind::Path),
            (
                self.underlines_iter.peek().map(|u| u.order),
                PrimitiveKind::Underline,
            ),
            (
                self.monochrome_sprites_iter.peek().map(|s| s.order),
                PrimitiveKind::MonochromeSprite,
            ),
            (
                self.subpixel_sprites_iter.peek().map(|s| s.order),
                PrimitiveKind::SubpixelSprite,
            ),
            (
                self.polychrome_sprites_iter.peek().map(|s| s.order),
                PrimitiveKind::PolychromeSprite,
            ),
            (
                self.surfaces_iter.peek().map(|s| s.order),
                PrimitiveKind::Surface,
            ),
        ];
        orders_and_kinds.sort_by_key(|(order, kind)| (order.unwrap_or(u32::MAX), *kind));

        let first = orders_and_kinds[0];
        let second = orders_and_kinds[1];
        let first_order = first.0?;
        // A batch that starts below an effect's order must not contain primitives at or above
        // it, so the effect can be processed between the two batches.
        while self
            .blurs
            .get(self.blur_cursor)
            .is_some_and(|blur| blur.order <= first_order)
        {
            self.blur_cursor += 1;
        }
        let batch_kind = first.1;
        let batch_limit = (second.0.unwrap_or(u32::MAX), second.1);
        // `Shadow` sorts first, so `(order, kind) < (limit, Shadow)` means `order < limit`.
        let max_order_and_kind = match self.blurs.get(self.blur_cursor) {
            Some(blur) => batch_limit.min((blur.order, PrimitiveKind::Shadow)),
            None => batch_limit,
        };

        match batch_kind {
            PrimitiveKind::Shadow => {
                let shadows_start = self.shadows_start;
                let mut shadows_end = shadows_start + 1;
                self.shadows_iter.next();
                while self
                    .shadows_iter
                    .next_if(|shadow| (shadow.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    shadows_end += 1;
                }
                self.shadows_start = shadows_end;
                Some(PrimitiveBatch::Shadows(shadows_start..shadows_end))
            }
            PrimitiveKind::Quad => {
                let quads_start = self.quads_start;
                let mut quads_end = quads_start + 1;
                self.quads_iter.next();
                while self
                    .quads_iter
                    .next_if(|quad| (quad.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    quads_end += 1;
                }
                self.quads_start = quads_end;
                Some(PrimitiveBatch::Quads(quads_start..quads_end))
            }
            PrimitiveKind::Path => {
                let paths_start = self.paths_start;
                let mut paths_end = paths_start + 1;
                // A batch is composited from one rasterized intermediate. Paths of one draw
                // order are composited as separate sprites, each cut by its own curved clip
                // chain; paths of several orders share one union sprite, so they must share
                // a chain (RND-002 R15, R16). Splitting on every chain change would turn a
                // row of independently clipped panels into one pass per panel.
                let (first_order, first_curved_clip) = self
                    .paths_iter
                    .next()
                    .map_or((0, 0), |path| (path.order, path.curved_clip));
                let (mut same_order, mut same_clip) = (true, true);
                while self
                    .paths_iter
                    .next_if(|path| {
                        let order_matches = same_order && path.order == first_order;
                        let clip_matches = same_clip && path.curved_clip == first_curved_clip;
                        let accepted = (path.order, batch_kind) < max_order_and_kind
                            && (order_matches || clip_matches);
                        if accepted {
                            same_order = order_matches;
                            same_clip = clip_matches;
                        }
                        accepted
                    })
                    .is_some()
                {
                    paths_end += 1;
                }
                self.paths_start = paths_end;
                Some(PrimitiveBatch::Paths(paths_start..paths_end))
            }
            PrimitiveKind::Underline => {
                let underlines_start = self.underlines_start;
                let mut underlines_end = underlines_start + 1;
                self.underlines_iter.next();
                while self
                    .underlines_iter
                    .next_if(|underline| (underline.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    underlines_end += 1;
                }
                self.underlines_start = underlines_end;
                Some(PrimitiveBatch::Underlines(underlines_start..underlines_end))
            }
            PrimitiveKind::MonochromeSprite => {
                let texture_id = self.monochrome_sprites_iter.peek().unwrap().tile.texture_id;
                let sprites_start = self.monochrome_sprites_start;
                let mut sprites_end = sprites_start + 1;
                self.monochrome_sprites_iter.next();
                while self
                    .monochrome_sprites_iter
                    .next_if(|sprite| {
                        (sprite.order, batch_kind) < max_order_and_kind
                            && sprite.tile.texture_id == texture_id
                    })
                    .is_some()
                {
                    sprites_end += 1;
                }
                self.monochrome_sprites_start = sprites_end;
                Some(PrimitiveBatch::MonochromeSprites {
                    texture_id,
                    range: sprites_start..sprites_end,
                })
            }
            PrimitiveKind::SubpixelSprite => {
                let texture_id = self.subpixel_sprites_iter.peek().unwrap().tile.texture_id;
                let sprites_start = self.subpixel_sprites_start;
                let mut sprites_end = sprites_start + 1;
                self.subpixel_sprites_iter.next();
                while self
                    .subpixel_sprites_iter
                    .next_if(|sprite| {
                        (sprite.order, batch_kind) < max_order_and_kind
                            && sprite.tile.texture_id == texture_id
                    })
                    .is_some()
                {
                    sprites_end += 1;
                }
                self.subpixel_sprites_start = sprites_end;
                Some(PrimitiveBatch::SubpixelSprites {
                    texture_id,
                    range: sprites_start..sprites_end,
                })
            }
            PrimitiveKind::PolychromeSprite => {
                let texture_id = self.polychrome_sprites_iter.peek().unwrap().tile.texture_id;
                let sprites_start = self.polychrome_sprites_start;
                let mut sprites_end = sprites_start + 1;
                self.polychrome_sprites_iter.next();
                while self
                    .polychrome_sprites_iter
                    .next_if(|sprite| {
                        (sprite.order, batch_kind) < max_order_and_kind
                            && sprite.tile.texture_id == texture_id
                    })
                    .is_some()
                {
                    sprites_end += 1;
                }
                self.polychrome_sprites_start = sprites_end;
                Some(PrimitiveBatch::PolychromeSprites {
                    texture_id,
                    range: sprites_start..sprites_end,
                })
            }
            PrimitiveKind::Surface => {
                let surfaces_start = self.surfaces_start;
                let mut surfaces_end = surfaces_start + 1;
                self.surfaces_iter.next();
                while self
                    .surfaces_iter
                    .next_if(|surface| (surface.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    surfaces_end += 1;
                }
                self.surfaces_start = surfaces_end;
                Some(PrimitiveBatch::Surfaces(surfaces_start..surfaces_end))
            }
        }
    }
}

#[derive(Debug)]
#[cfg_attr(
    all(
        any(target_os = "linux", target_os = "freebsd"),
        not(any(feature = "x11", feature = "wayland"))
    ),
    allow(dead_code)
)]
#[allow(missing_docs)]
pub enum PrimitiveBatch {
    Shadows(Range<usize>),
    Quads(Range<usize>),
    Paths(Range<usize>),
    Underlines(Range<usize>),
    MonochromeSprites {
        texture_id: AtlasTextureId,
        range: Range<usize>,
    },
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    SubpixelSprites {
        texture_id: AtlasTextureId,
        range: Range<usize>,
    },
    PolychromeSprites {
        texture_id: AtlasTextureId,
        range: Range<usize>,
    },
    Surfaces(Range<usize>),
}

impl PrimitiveBatch {
    #[expect(missing_docs)]
    pub fn label(&self) -> String {
        match self {
            Self::Shadows(range) => format!("shadows ({})", range.len()),
            Self::Quads(range) => format!("quads ({})", range.len()),
            Self::Paths(range) => format!("paths ({})", range.len()),
            Self::Underlines(range) => format!("underlines ({})", range.len()),
            Self::MonochromeSprites { texture_id, range } => {
                format!(
                    "monochrome sprites ({}) on atlas {}",
                    range.len(),
                    texture_id.index
                )
            }
            Self::SubpixelSprites { texture_id, range } => {
                format!(
                    "subpixel sprites ({}) on atlas {}",
                    range.len(),
                    texture_id.index
                )
            }
            Self::PolychromeSprites { texture_id, range } => {
                format!(
                    "polychrome sprites ({}) on atlas {}",
                    range.len(),
                    texture_id.index
                )
            }
            Self::Surfaces(range) => format!("surfaces ({})", range.len()),
        }
    }
}

#[derive(Default, Debug, Copy, Clone)]
#[repr(C)]
#[expect(missing_docs)]
pub struct Quad {
    pub order: DrawOrder,
    pub border_style: BorderStyle,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub background: Background,
    pub border_color: Hsla,
    pub corner_radii: Corners<ScaledPixels>,
    pub border_widths: Edges<ScaledPixels>,
    pub scope: ScopeParams,
}

impl From<Quad> for Primitive {
    fn from(quad: Quad) -> Self {
        Primitive::Quad(quad)
    }
}

#[derive(Debug, Default, Copy, Clone)]
#[repr(C)]
#[expect(missing_docs)]
pub struct Underline {
    pub order: DrawOrder,
    pub curved_clip: CurvedClipId,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
    pub thickness: ScaledPixels,
    pub wavy: PaddedBool32,
}

impl From<Underline> for Primitive {
    fn from(underline: Underline) -> Self {
        Primitive::Underline(underline)
    }
}

#[derive(Debug, Default, Copy, Clone)]
#[repr(C)]
#[expect(missing_docs)]
pub struct Shadow {
    pub order: DrawOrder,
    pub blur_radius: ScaledPixels,
    pub bounds: Bounds<ScaledPixels>,
    pub corner_radii: Corners<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
    pub element_bounds: Bounds<ScaledPixels>,
    pub element_corner_radii: Corners<ScaledPixels>,
    /// 0 = drop shadow (rendered outside the element), 1 = inset shadow (rendered inside).
    pub inset: u32,
    pub curved_clip: CurvedClipId,
}

impl From<Shadow> for Primitive {
    fn from(shadow: Shadow) -> Self {
        Primitive::Shadow(shadow)
    }
}

/// The style of a border.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[repr(C)]
pub enum BorderStyle {
    /// A solid border.
    #[default]
    Solid = 0,
    /// A dashed border.
    Dashed = 1,
}

/// A data type representing a 2 dimensional transformation that can be applied to an element.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct TransformationMatrix {
    /// 2x2 matrix containing rotation and scale,
    /// stored row-major
    pub rotation_scale: [[f32; 2]; 2],
    /// translation vector
    pub translation: [f32; 2],
}

impl Eq for TransformationMatrix {}

impl TransformationMatrix {
    /// The unit matrix, has no effect.
    pub fn unit() -> Self {
        Self {
            rotation_scale: [[1.0, 0.0], [0.0, 1.0]],
            translation: [0.0, 0.0],
        }
    }

    /// Move the origin by a given point
    pub fn translate(mut self, point: Point<ScaledPixels>) -> Self {
        self.compose(Self {
            rotation_scale: [[1.0, 0.0], [0.0, 1.0]],
            translation: [point.x.0, point.y.0],
        })
    }

    /// Clockwise rotation in radians around the origin
    pub fn rotate(self, angle: Radians) -> Self {
        self.compose(Self {
            rotation_scale: [
                [angle.0.cos(), -angle.0.sin()],
                [angle.0.sin(), angle.0.cos()],
            ],
            translation: [0.0, 0.0],
        })
    }

    /// Scale around the origin
    pub fn scale(self, size: Size<f32>) -> Self {
        self.compose(Self {
            rotation_scale: [[size.width, 0.0], [0.0, size.height]],
            translation: [0.0, 0.0],
        })
    }

    /// Perform matrix multiplication with another transformation
    /// to produce a new transformation that is the result of
    /// applying both transformations: first, `other`, then `self`.
    #[inline]
    pub fn compose(self, other: TransformationMatrix) -> TransformationMatrix {
        if other == Self::unit() {
            return self;
        }
        // Perform matrix multiplication
        TransformationMatrix {
            rotation_scale: [
                [
                    self.rotation_scale[0][0] * other.rotation_scale[0][0]
                        + self.rotation_scale[0][1] * other.rotation_scale[1][0],
                    self.rotation_scale[0][0] * other.rotation_scale[0][1]
                        + self.rotation_scale[0][1] * other.rotation_scale[1][1],
                ],
                [
                    self.rotation_scale[1][0] * other.rotation_scale[0][0]
                        + self.rotation_scale[1][1] * other.rotation_scale[1][0],
                    self.rotation_scale[1][0] * other.rotation_scale[0][1]
                        + self.rotation_scale[1][1] * other.rotation_scale[1][1],
                ],
            ],
            translation: [
                self.translation[0]
                    + self.rotation_scale[0][0] * other.translation[0]
                    + self.rotation_scale[0][1] * other.translation[1],
                self.translation[1]
                    + self.rotation_scale[1][0] * other.translation[0]
                    + self.rotation_scale[1][1] * other.translation[1],
            ],
        }
    }

    /// Apply transformation to a point, mainly useful for debugging
    pub fn apply(&self, point: Point<Pixels>) -> Point<Pixels> {
        let input = [point.x.0, point.y.0];
        let mut output = self.translation;
        for (i, output_cell) in output.iter_mut().enumerate() {
            for (k, input_cell) in input.iter().enumerate() {
                *output_cell += self.rotation_scale[i][k] * *input_cell;
            }
        }
        Point::new(output[0].into(), output[1].into())
    }
}

impl Default for TransformationMatrix {
    fn default() -> Self {
        Self::unit()
    }
}

#[derive(Copy, Clone, Debug)]
#[repr(C)]
#[expect(missing_docs)]
pub struct MonochromeSprite {
    pub order: DrawOrder,
    /// Gaussian blur sigma in device pixels applied while sampling the mask at composite
    /// time (RND-005); `0` samples the mask as is.
    pub blur: f32,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
    pub tile: AtlasTile,
    pub transformation: TransformationMatrix,
    pub scope: ScopeParams,
}

impl From<MonochromeSprite> for Primitive {
    fn from(sprite: MonochromeSprite) -> Self {
        Primitive::MonochromeSprite(sprite)
    }
}

#[derive(Copy, Clone, Debug)]
#[repr(C)]
#[expect(missing_docs)]
pub struct SubpixelSprite {
    pub order: DrawOrder,
    pub pad: u32, // align to 8 bytes
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
    pub tile: AtlasTile,
    pub transformation: TransformationMatrix,
    pub scope: ScopeParams,
}

impl From<SubpixelSprite> for Primitive {
    fn from(sprite: SubpixelSprite) -> Self {
        Primitive::SubpixelSprite(sprite)
    }
}

/// Per-draw image effects of a [`PolychromeSprite`], all in device pixels; the all-zero
/// block is the neutral state (ordinary linear image painting).
///
/// Only the wgpu (Linux) renderer evaluates the mask and the nearest flag. The Metal and
/// Direct3D renderers carry the block to keep the record layout in step but ignore it: they
/// paint images without the mask and always filter linearly.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
#[repr(C)]
pub struct ImageEffects {
    /// Feathered inverse rounded-rectangle mask bounds: origin x, origin y, width, height
    /// (RND-004). The mask is disabled while `mask_feather` is not positive.
    pub mask_bounds: [f32; 4],
    /// Corner radius of the mask rectangle.
    pub mask_radius: f32,
    /// Feather distance of the mask; zero or negative disables the whole mask.
    pub mask_feather: f32,
    /// Clearance distance added to the cutout.
    pub mask_clearance: f32,
    /// Bottom reference y coordinate of the bottom ramp.
    pub mask_bottom_reference: f32,
    /// Bottom feather distance; zero or negative disables the bottom ramp.
    pub mask_bottom_feather: f32,
    /// Bit 0: select the texel containing the coordinate instead of filtering (RND-007).
    pub flags: u32,
    /// Reserved; keeps the block at 48 bytes.
    pub reserved: [u32; 2],
}

impl ImageEffects {
    /// Flag bit selecting nearest-neighbor texel selection.
    pub const NEAREST: u32 = 1;
}

#[derive(Copy, Clone, Debug)]
#[repr(C)]
#[expect(missing_docs)]
pub struct PolychromeSprite {
    pub order: DrawOrder,
    pub pad: u32,
    pub grayscale: PaddedBool32,
    pub opacity: f32,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub corner_radii: Corners<ScaledPixels>,
    pub tile: AtlasTile,
    pub scope: ScopeParams,
    pub effects: ImageEffects,
}

impl From<PolychromeSprite> for Primitive {
    fn from(sprite: PolychromeSprite) -> Self {
        Primitive::PolychromeSprite(sprite)
    }
}

/// A surface primitive: an axis-aligned destination rectangle showing a texture, clipped by
/// the content mask. Corner radii are not applied.
#[derive(Clone, Debug)]
#[allow(missing_docs)]
pub struct PaintSurface {
    pub order: DrawOrder,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    /// Element opacity in effect while painting; multiplies the sampled alpha.
    /// Ignored by the macOS video-buffer path.
    pub opacity: f32,
    /// Chain of curved overflow clips applying to the surface (stamped by the scene).
    pub curved_clip: CurvedClipId,
    #[cfg(target_os = "macos")]
    pub image_buffer: objc2_core_foundation::CFRetained<objc2_core_video::CVPixelBuffer>,
    /// Shared erased GPU texture; the scene keeps it alive until dropped. Its concrete type
    /// is owned by the renderer, which skips textures of any other type.
    #[cfg(not(target_os = "macos"))]
    pub texture: std::sync::Arc<dyn std::any::Any + Send + Sync>,
}

impl From<PaintSurface> for Primitive {
    fn from(surface: PaintSurface) -> Self {
        Primitive::Surface(surface)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[expect(missing_docs)]
pub struct PathId(pub usize);

/// A line made up of a series of vertices and control points.
#[derive(Clone, Debug)]
#[expect(missing_docs)]
pub struct Path<P: Clone + Debug + Default + PartialEq> {
    pub id: PathId,
    pub order: DrawOrder,
    pub bounds: Bounds<P>,
    pub content_mask: ContentMask<P>,
    pub vertices: Vec<PathVertex<P>>,
    pub color: Background,
    start: Point<P>,
    current: Point<P>,
    contour_count: usize,
    /// Curved overflow clip chain stamped by the scene.
    pub curved_clip: CurvedClipId,
}

impl Path<Pixels> {
    /// Create a new path with the given starting point.
    pub fn new(start: Point<Pixels>) -> Self {
        Self {
            id: PathId(0),
            order: DrawOrder::default(),
            vertices: Vec::new(),
            start,
            current: start,
            bounds: Bounds {
                origin: start,
                size: Default::default(),
            },
            content_mask: Default::default(),
            color: Default::default(),
            contour_count: 0,
            curved_clip: 0,
        }
    }

    /// A path of already tessellated triangle vertices starting at `start`; the bounds are
    /// those of `start` and the vertices.
    pub(crate) fn from_vertices(start: Point<Pixels>, vertices: Vec<PathVertex<Pixels>>) -> Self {
        let mut min = start;
        let mut max = start;
        for vertex in &vertices {
            let p = vertex.xy_position;
            min = point(min.x.min(p.x), min.y.min(p.y));
            max = point(max.x.max(p.x), max.y.max(p.y));
        }
        let mut path = Self::new(start);
        path.bounds = Bounds::from_corners(min, max);
        path.vertices = vertices;
        path
    }

    /// Scale this path by the given factor.
    pub fn scale(&self, factor: f32) -> Path<ScaledPixels> {
        Path {
            id: self.id,
            order: self.order,
            bounds: self.bounds.scale(factor),
            content_mask: self.content_mask.scale(factor),
            vertices: self
                .vertices
                .iter()
                .map(|vertex| vertex.scale(factor))
                .collect(),
            start: self.start.map(|start| start.scale(factor)),
            current: self.current.scale(factor),
            contour_count: self.contour_count,
            curved_clip: self.curved_clip,
            color: self.color,
        }
    }

    /// Move the start, current point to the given point.
    pub fn move_to(&mut self, to: Point<Pixels>) {
        self.contour_count += 1;
        self.start = to;
        self.current = to;
    }

    /// Draw a straight line from the current point to the given point.
    pub fn line_to(&mut self, to: Point<Pixels>) {
        self.contour_count += 1;
        if self.contour_count > 1 {
            self.push_triangle(
                (self.start, self.current, to),
                (point(0., 1.), point(0., 1.), point(0., 1.)),
            );
        }
        self.current = to;
    }

    /// Draw a curve from the current point to the given point, using the given control point.
    pub fn curve_to(&mut self, to: Point<Pixels>, ctrl: Point<Pixels>) {
        self.contour_count += 1;
        if self.contour_count > 1 {
            self.push_triangle(
                (self.start, self.current, to),
                (point(0., 1.), point(0., 1.), point(0., 1.)),
            );
        }

        self.push_triangle(
            (self.current, ctrl, to),
            (point(0., 0.), point(0.5, 0.), point(1., 1.)),
        );
        self.current = to;
    }

    /// Push a triangle to the Path.
    pub fn push_triangle(
        &mut self,
        xy: (Point<Pixels>, Point<Pixels>, Point<Pixels>),
        st: (Point<f32>, Point<f32>, Point<f32>),
    ) {
        self.bounds = self
            .bounds
            .union(&Bounds {
                origin: xy.0,
                size: Default::default(),
            })
            .union(&Bounds {
                origin: xy.1,
                size: Default::default(),
            })
            .union(&Bounds {
                origin: xy.2,
                size: Default::default(),
            });

        self.vertices.push(PathVertex {
            xy_position: xy.0,
            st_position: st.0,
            content_mask: Default::default(),
        });
        self.vertices.push(PathVertex {
            xy_position: xy.1,
            st_position: st.1,
            content_mask: Default::default(),
        });
        self.vertices.push(PathVertex {
            xy_position: xy.2,
            st_position: st.2,
            content_mask: Default::default(),
        });
    }
}

impl<T> Path<T>
where
    T: Clone + Debug + Default + PartialEq + PartialOrd + Add<T, Output = T> + Sub<Output = T>,
{
    #[allow(unused)]
    #[expect(missing_docs)]
    pub fn clipped_bounds(&self) -> Bounds<T> {
        self.bounds.intersect(&self.content_mask.bounds)
    }
}

impl From<Path<ScaledPixels>> for Primitive {
    fn from(path: Path<ScaledPixels>) -> Self {
        Primitive::Path(path)
    }
}

#[derive(Clone, Debug)]
#[repr(C)]
#[expect(missing_docs)]
pub struct PathVertex<P: Clone + Debug + Default + PartialEq> {
    pub xy_position: Point<P>,
    pub st_position: Point<f32>,
    pub content_mask: ContentMask<P>,
}

#[expect(missing_docs)]
impl PathVertex<Pixels> {
    pub fn scale(&self, factor: f32) -> PathVertex<ScaledPixels> {
        PathVertex {
            xy_position: self.xy_position.scale(factor),
            st_position: self.st_position,
            content_mask: self.content_mask.scale(factor),
        }
    }
}

/// An ordered backdrop-blur effect: the renderer snapshots the framebuffer immediately before
/// this record's order, Gaussian-filters the snapshot and writes the result back inside the
/// rounded bounds before drawing continues.
///
/// All geometry is in device pixels; `blur_radius` is the Gaussian standard deviation in device
/// pixels. Effects are not primitives: the painting operation inserts an invisible ordinary
/// primitive with the same bounds immediately before the effect to produce a batch boundary.
#[derive(Debug, Copy, Clone, PartialEq)]
#[repr(C)]
pub struct BackdropBlur {
    /// Paint order (assigned on insertion).
    pub order: DrawOrder,
    /// Effect rectangle.
    pub bounds: Bounds<ScaledPixels>,
    /// Per-corner radii of the effect's rounded rectangle.
    pub corner_radii: Corners<ScaledPixels>,
    /// Gaussian standard deviation in device pixels.
    pub blur_radius: ScaledPixels,
    /// Rectangular content mask clipping both the snapshot envelope and the composite.
    pub content_mask: ContentMask<ScaledPixels>,
}

impl Scene {
    /// The order of the first primitive of `batch`. Pending backdrop-blur effects whose order is
    /// at or below this value are processed immediately before the batch is drawn.
    pub fn batch_first_order(&self, batch: &PrimitiveBatch) -> DrawOrder {
        match batch {
            PrimitiveBatch::Shadows(range) => self.shadows[range.start].order,
            PrimitiveBatch::Quads(range) => self.quads[range.start].order,
            PrimitiveBatch::Paths(range) => self.paths[range.start].order,
            PrimitiveBatch::Underlines(range) => self.underlines[range.start].order,
            PrimitiveBatch::MonochromeSprites { range, .. } => {
                self.monochrome_sprites[range.start].order
            }
            PrimitiveBatch::SubpixelSprites { range, .. } => {
                self.subpixel_sprites[range.start].order
            }
            PrimitiveBatch::PolychromeSprites { range, .. } => {
                self.polychrome_sprites[range.start].order
            }
            PrimitiveBatch::Surfaces(range) => self.surfaces[range.start].order,
        }
    }
}
