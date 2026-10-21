//! Adaptive tiled scroll-content reuse, the window side (PRF-004).
//!
//! A scrolling block that scrolled on two consecutive frames records its content once, in
//! content space, as a *layer*: the paint operations go to a [`LayerContent`] the renderer
//! rasterizes into tiles, and the prepaint and paint output other than the scene (hitboxes,
//! listeners, cursor styles, element states, tab stops, dispatch nodes, accessibility
//! operations) is kept as a record nested in the view that owns the block, exactly like a
//! retained row of a virtual list (see `rows.rs`). While the block only scrolls, a frame
//! replays that record moved to the new offset and composites the tiles at the new offset
//! instead of rebuilding the content.
//!
//! The decisions are conservative: the result is always the direct drawing of the content,
//! and a layer that costs as much as direct drawing is demoted.

mod policy;

use super::{
    Window,
    read_tracker::ViewKey,
    retained::{CacheKey, Placement, SpliceMap, ViewKind},
};
use crate::{
    App, Background, Bounds, ContentMask, DevicePixels, ElementId, GlobalElementId, LayerContent,
    LayerFrame, LayerId, Pixels, Point, ScaledPixels, Size, Style, TILE_BUDGET_TILES, TileCoord,
    point, px, size, tile_sprite,
};
use policy::{Backoff, CostHistory, REPAINT_COST, WORK_CLAMP};
use std::{cell::RefCell, rc::Rc, sync::Arc};
use vespertine_collections::{FxHashMap, FxHashSet};

/// Consecutive scrolling frames after which a block becomes eligible (R1).
pub(crate) const PROMOTION_FRAMES: u32 = 2;
/// Frames to wait after content turned out to be ineligible (R8).
pub(crate) const RETRY_FRAMES: u32 = 8;
/// Viewport extents of content painted on each side of the viewport (R1).
const OVERSCAN_VIEWPORTS: i32 = 2;
/// Quarter of a viewport: the margin a composite frame needs around the viewport (R12).
const MARGIN_DIVISOR: i32 = 4;
/// Frames a layer may go without being composited before it is dropped (R10).
const INACTIVE_FRAMES: u64 = 120;

/// Whether a layer last composited in frame `composited` is dropped in `frame` (R10).
pub(crate) fn inactive(frame: u64, composited: u64) -> bool {
    frame.saturating_sub(composited) >= INACTIVE_FRAMES
}

thread_local! {
    /// The scroll states whose exact offset was read while a window rendered (PRF-004 R2).
    static OFFSET_READS: RefCell<OffsetReads> = RefCell::new(OffsetReads::default());
}

#[derive(Default)]
struct OffsetReads {
    active: bool,
    read: FxHashSet<usize>,
}

/// Notes that the exact scroll offset `offset` was read. Only reads made while a window is
/// drawing matter: a block whose offset is read during rendering is repainted on every offset
/// change, so the optimization of PRF-004 is invalid for it.
pub(crate) fn note_offset_read(offset: &Rc<RefCell<Point<Pixels>>>) {
    OFFSET_READS.with(|reads| {
        let mut reads = reads.borrow_mut();
        if reads.active {
            reads.read.insert(Rc::as_ptr(offset) as usize);
        }
    });
}

/// What the scroll block tells the window about itself while it is prepainted.
pub(crate) struct LayerParams<'a> {
    pub id: &'a GlobalElementId,
    /// The border box of the block.
    pub bounds: Bounds<Pixels>,
    /// The scrolling viewport: the block's overflow clip, before the clips of its ancestors.
    pub viewport: Bounds<Pixels>,
    /// The extent of the scrollable content measured from the origin of `bounds`.
    pub extent: Size<Pixels>,
    pub scroll_max: Point<Pixels>,
    pub offset: Rc<RefCell<Point<Pixels>>>,
    pub scrollable: (bool, bool),
    /// The block paints an opaque solid background without rounded corners and without opacity.
    pub background: Option<Background>,
    pub style: &'a Style,
    /// The block reports its children's bounds to a listener after prepainting them.
    pub prepaint_listener: bool,
}

/// Why a layer repaints its content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refresh {
    /// The layer is new.
    Promotion,
    /// What the content read changed, or the owner was rebuilt for a reason nothing tracked
    /// explains.
    Content,
    /// The retained region no longer holds the viewport and its margin.
    Region,
    /// Hover state or a pending input needs the content at current coordinates.
    Input,
}

/// What a block does in the frame being drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum LayerMode {
    /// The content is drawn directly.
    #[default]
    Direct,
    /// The content is painted into the layer.
    Record(Refresh),
    /// The recorded content is composited at the new offset.
    Composite,
}

/// The device-pixel rectangle `left..right` x `top..bottom`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl Rect {
    fn width(&self) -> i32 {
        self.right - self.left
    }

    fn height(&self) -> i32 {
        self.bottom - self.top
    }

    fn contains(&self, other: &Rect) -> bool {
        self.left <= other.left
            && self.top <= other.top
            && self.right >= other.right
            && self.bottom >= other.bottom
    }

    fn bounds(&self) -> Bounds<DevicePixels> {
        Bounds {
            origin: point(DevicePixels(self.left), DevicePixels(self.top)),
            size: size(DevicePixels(self.width()), DevicePixels(self.height())),
        }
    }

    fn moved(&self, dx: i32, dy: i32) -> Rect {
        Rect {
            left: self.left + dx,
            top: self.top + dy,
            right: self.right + dx,
            bottom: self.bottom + dy,
        }
    }

    fn area(&self) -> f64 {
        f64::from(self.width().max(0)) * f64::from(self.height().max(0))
    }
}

impl From<Bounds<DevicePixels>> for Rect {
    fn from(bounds: Bounds<DevicePixels>) -> Self {
        Rect {
            left: bounds.origin.x.0,
            top: bounds.origin.y.0,
            right: bounds.origin.x.0 + bounds.size.width.0,
            bottom: bounds.origin.y.0 + bounds.size.height.0,
        }
    }
}

/// Rounds a device-pixel rectangle outward to whole device pixels.
fn outward(bounds: Bounds<ScaledPixels>) -> Rect {
    Rect {
        left: bounds.origin.x.0.floor() as i32,
        top: bounds.origin.y.0.floor() as i32,
        right: (bounds.origin.x.0 + bounds.size.width.0).ceil() as i32,
        bottom: (bounds.origin.y.0 + bounds.size.height.0).ceil() as i32,
    }
}

fn device(value: Pixels, scale: f32) -> i32 {
    (value.0 * scale).round() as i32
}

/// The geometry a layer is recorded and composited with, in whole device pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Geometry {
    /// The origin of content space in window device pixels while the content is not scrolled.
    origin: (i32, i32),
    /// The snapped scroll offset in device pixels.
    scroll: (i32, i32),
    /// The viewport in content space.
    view: Rect,
    /// The content extent in content space.
    extent: Rect,
}

impl Geometry {
    fn new(params: &LayerParams, snapped: Point<Pixels>, scale: f32) -> Geometry {
        let origin = (
            device(params.bounds.origin.x, scale),
            device(params.bounds.origin.y, scale),
        );
        let scroll = (device(snapped.x, scale), device(snapped.y, scale));
        let view_window = outward(params.viewport.scale(scale));
        let view = view_window.moved(-(origin.0 + scroll.0), -(origin.1 + scroll.1));
        let extent_size = params.extent.scale(scale);
        let extent = Rect {
            left: 0,
            top: 0,
            right: extent_size.width.0.ceil() as i32,
            bottom: extent_size.height.0.ceil() as i32,
        };
        Geometry {
            origin,
            scroll,
            view,
            extent,
        }
    }

    /// Where content space starts in window device pixels at the current scroll offset.
    fn to_window(&self) -> (i32, i32) {
        (self.origin.0 + self.scroll.0, self.origin.1 + self.scroll.1)
    }
}

/// The region painted around `view` on the scrolled axes: two viewport extents on each side,
/// clamped to the content (R1); the viewport itself on the other axes.
fn painted_region(view: Rect, extent: Rect, scrollable: (bool, bool)) -> Rect {
    let mut region = view;
    if scrollable.0 {
        let over = view.width() * OVERSCAN_VIEWPORTS;
        region.left = view.left.min((view.left - over).max(extent.left));
        region.right = view.right.max((view.right + over).min(extent.right));
    }
    if scrollable.1 {
        let over = view.height() * OVERSCAN_VIEWPORTS;
        region.top = view.top.min((view.top - over).max(extent.top));
        region.bottom = view.bottom.max((view.bottom + over).min(extent.bottom));
    }
    region
}

/// The viewport with the safety margin of a quarter viewport on the scrolled axes, clamped to
/// the content (R12).
fn padded_view(view: Rect, extent: Rect, scrollable: (bool, bool)) -> Rect {
    let mut padded = view;
    if scrollable.0 {
        let margin = view.width() / MARGIN_DIVISOR;
        padded.left = view.left.min((view.left - margin).max(extent.left));
        padded.right = view.right.max((view.right + margin).min(extent.right));
    }
    if scrollable.1 {
        let margin = view.height() / MARGIN_DIVISOR;
        padded.top = view.top.min((view.top - margin).max(extent.top));
        padded.bottom = view.bottom.max((view.bottom + margin).min(extent.bottom));
    }
    padded
}

/// A layer being used by a block.
pub(crate) struct Layer {
    /// The record nested in the view that owns the block: dependencies, output ranges and the
    /// non-scene output of the content.
    pub key: ViewKey,
    pub id: LayerId,
    pub instance: u64,
    pub generation: u64,
    pub content: Option<Arc<LayerContent>>,
    /// The painted region in content space.
    region: Rect,
    /// The geometry the content was recorded with.
    recorded: Geometry,
    background: Background,
    cache_key: CacheKey,
    scale: f32,
    /// The block's own geometry, for placing the layer without the block being prepainted.
    bounds: Bounds<Pixels>,
    viewport: Bounds<Pixels>,
    extent: Size<Pixels>,
    scroll_max: Point<Pixels>,
    offset: Rc<RefCell<Point<Pixels>>>,
    scrollable: (bool, bool),
    /// The visible clip of the block's content when it was last prepainted.
    clip: Bounds<Pixels>,
    /// The style, kept only to paint classic scrollbars after the content.
    scrollbar_style: Option<Style>,
    /// Scroll offsets of containers nested in the content, as recorded.
    nested: Vec<(Rc<RefCell<Point<Pixels>>>, Point<Pixels>)>,
    /// Hash and generation of the tiles shown so far.
    tiles: FxHashMap<TileCoord, (u64, u64)>,
    /// The time the content was recorded at, for the reads of the owner that built it.
    clock: u64,
    /// The dispatch node the content's nodes attach to, in the frame that recorded it.
    dispatch_active: Option<usize>,
    /// Why the content was last painted.
    refresh: Refresh,
    /// What the layer cost so far (R9).
    cost: CostHistory,
    repainted: bool,
}

impl Layer {
    /// Whether the block's current snapped scroll offset differs from the one the content was
    /// recorded at, so that the content captured stale window coordinates (R7).
    fn displaced(&self) -> bool {
        let offset = *self.offset.borrow();
        let scale = self.scale;
        let max = self.scroll_max;
        let x = offset.x.clamp(-max.x, px(0.));
        let y = offset.y.clamp(-max.y, px(0.));
        let snapped = (
            crate::util::round_to_device_pixel(x.0, scale) as i32,
            crate::util::round_to_device_pixel(y.0, scale) as i32,
        );
        snapped != self.recorded.scroll
    }
}

/// Per-block state, kept while the block exists.
#[derive(Default)]
pub(crate) struct Block {
    seen: u64,
    composited: u64,
    last_offset: Option<Point<Pixels>>,
    /// The scroll state of the block, and whether its exact offset was read while the block
    /// was last prepainted (R2).
    offset_ptr: usize,
    offset_read: bool,
    /// The frame the block was last prepainted in.
    prepainted: u64,
    streak: u32,
    streak_frame: u64,
    retry: u32,
    inhibited: bool,
    pub layer: Option<Layer>,
    mode: LayerMode,
    /// The cooldown after demotions (R10).
    backoff: Backoff,
    /// The geometry of the frame being drawn, fixed at prepaint.
    geometry: Option<Geometry>,
    recording: Recording,
    /// The layer is waiting to be replaced at a new offset by a clean owner.
    pub moved: bool,
}

/// What recording the content of a block collects.
#[derive(Default)]
struct Recording {
    /// The content requested an animation frame while it was built.
    animated: bool,
    /// A text input inside the content is focused.
    text_input: bool,
    /// Scroll containers nested in the content and their offsets as recorded.
    nested: Vec<(Rc<RefCell<Point<Pixels>>>, Point<Pixels>)>,
}

/// The scroll layers of a window.
pub(crate) struct ScrollLayers {
    /// Per-window control: whether scroll layers are used at all.
    pub enabled: bool,
    supported: Option<bool>,
    pub frame: u64,
    blocks: FxHashMap<GlobalElementId, Block>,
    layer_blocks: FxHashMap<LayerId, GlobalElementId>,
    next_id: u32,
    free_ids: Vec<LayerId>,
    next_instance: u64,
    retired: Vec<(LayerId, u64)>,
    /// Blocks that scrolled without notifying their owner view in this frame.
    absorbed: FxHashSet<GlobalElementId>,
    /// The block whose content is being built into a layer right now.
    active: Option<GlobalElementId>,
    /// Tiles composited by this frame's layer frames so far.
    frame_tiles: usize,
    /// Views that requested an animation frame in the preceding frame.
    pub animating: FxHashSet<crate::EntityId>,
    /// Layers repaint at their current offsets in this frame: input is about to be dispatched
    /// (R7).
    force_input: bool,
}

impl Default for ScrollLayers {
    fn default() -> Self {
        Self {
            enabled: true,
            supported: None,
            frame: 0,
            blocks: FxHashMap::default(),
            layer_blocks: FxHashMap::default(),
            next_id: 0,
            free_ids: Vec::new(),
            next_instance: 1,
            retired: Vec::new(),
            absorbed: FxHashSet::default(),
            active: None,
            frame_tiles: 0,
            animating: FxHashSet::default(),
            force_input: false,
        }
    }
}

impl ScrollLayers {
    /// Notes that the layer `id` was composited in this frame.
    pub(crate) fn note_composited(&mut self, id: LayerId) {
        let frame = self.frame;
        if let Some(block) = self
            .layer_blocks
            .get(&id)
            .and_then(|block| self.blocks.get_mut(block))
        {
            block.composited = frame;
        }
    }

    /// Whether the block `id` has a layer.
    pub(crate) fn has_layer(&self, id: &GlobalElementId) -> bool {
        self.blocks
            .get(id)
            .is_some_and(|block| block.layer.is_some())
    }

    /// The number of live layers.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn layer_count(&self) -> usize {
        self.blocks
            .values()
            .filter(|block| block.layer.is_some())
            .count()
    }

    fn allocate_id(&mut self) -> Option<LayerId> {
        if let Some(id) = self.free_ids.pop() {
            return Some(id);
        }
        let id = LayerId::new(self.next_id)?;
        self.next_id += 1;
        Some(id)
    }
}

/// What tests observe of a scroll block.
#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ScrollLayerProbe {
    pub layer: bool,
    pub instance: u64,
    pub generation: u64,
    pub composited: bool,
    pub region: Option<(i32, i32, i32, i32)>,
    pub recorded_scroll: Option<(i32, i32)>,
    pub cooldown: u32,
    pub demotions: u32,
    pub retry: u32,
    pub offset_read: bool,
}

impl Window {
    /// Whether scroll layers can be used by the block being drawn.
    fn scroll_layers_usable(&mut self) -> bool {
        if !self.scroll_layers.enabled
            || !self.retention_tracking()
            || self.composition.is_some()
            || self.retention.stack.is_empty()
            || self.scroll_layers.active.is_some()
            || self.in_row_recording()
        {
            return false;
        }
        let platform = &self.platform_window;
        *self
            .scroll_layers
            .supported
            .get_or_insert_with(|| platform.supports_scroll_layers())
    }

    /// Chooses what the block described by `params` does in this frame, given its scroll
    /// offset snapped to the device pixel grid (PRF-004 R1, R2, R8, R9, R12).
    pub(crate) fn scroll_layer_begin(
        &mut self,
        params: &LayerParams,
        snapped: Point<Pixels>,
    ) -> LayerMode {
        if !self.scroll_layers_usable() {
            self.scroll_layer_release(params.id);
            return LayerMode::Direct;
        }
        let frame = self.scroll_layers.frame;
        let scale = self.scale_factor();
        let geometry = Geometry::new(params, snapped, scale);
        let cache_key = self.ambient_cache_key(params.bounds);
        let owner_animating = self
            .retention
            .stack
            .last()
            .and_then(|key| self.retention.views.get(*key))
            .is_some_and(|record| self.scroll_layers.animating.contains(&record.entity_id));

        // Scroll bookkeeping: eligibility needs scrolling on consecutive frames.
        let block = self
            .scroll_layers
            .blocks
            .entry(params.id.clone())
            .or_default();
        block.seen = frame;
        block.prepainted = frame;
        block.offset_ptr = Rc::as_ptr(&params.offset) as usize;
        let offset_read = block.offset_read
            || OFFSET_READS.with(|reads| reads.borrow().read.contains(&block.offset_ptr));
        let previous = block.last_offset.replace(snapped);
        let moved = previous.is_some_and(|previous| previous != snapped);
        let jump = previous.is_some_and(|previous| {
            let delta = snapped - previous;
            (params.scrollable.0 && delta.x.abs().0 * scale >= geometry.view.width() as f32)
                || (params.scrollable.1 && delta.y.abs().0 * scale >= geometry.view.height() as f32)
        });
        if moved {
            block.streak = if block.streak_frame + 1 == frame {
                block.streak + 1
            } else {
                1
            };
            block.streak_frame = frame;
        }
        let waiting = block.retry > 0;
        block.retry = block.retry.saturating_sub(1);
        if jump {
            // A jump of a viewport per frame drops the layer and inhibits promotion until
            // the motion slows down (R8).
            block.inhibited = true;
            block.streak = 0;
        } else if moved && block.inhibited {
            block.inhibited = false;
            block.streak = 1;
        }
        let scrolling_now = block.streak_frame == frame && block.streak >= PROMOTION_FRAMES;
        let promotable =
            scrolling_now && !waiting && !block.inhibited && block.backoff.ready() && !offset_read;
        let has_layer = block.layer.is_some();

        let style_ok = params.background.is_some()
            && !params.prepaint_listener
            && !self.inherited_geometry_unsafe(params);
        let mut mode = LayerMode::Direct;
        let mut teardown = false;
        if jump || !style_ok || owner_animating || offset_read {
            teardown = has_layer;
        } else if has_layer {
            mode = LayerMode::Composite;
        } else if promotable {
            mode = LayerMode::Record(Refresh::Promotion);
        }
        if teardown {
            self.scroll_layer_drop(params.id, false);
        }

        if has_layer && !teardown {
            mode = self.scroll_layer_decide(params, geometry, &cache_key, scale);
        }
        let tiles_needed = TileCoord::covering(geometry.view.bounds()).map(|tiles| tiles.len());
        if mode != LayerMode::Direct {
            // The visible tile set alone must fit the budget; content that cannot be tiled
            // (coordinates out of range) is drawn directly (R10).
            let fits = tiles_needed
                .is_some_and(|needed| self.scroll_layers.frame_tiles + needed <= TILE_BUDGET_TILES);
            if !fits {
                self.scroll_layer_drop(params.id, false);
                if let Some(block) = self.scroll_layers.blocks.get_mut(params.id) {
                    block.retry = RETRY_FRAMES;
                }
                mode = LayerMode::Direct;
            } else if let Some(needed) = tiles_needed {
                self.scroll_layers.frame_tiles += needed;
            }
        }

        if let LayerMode::Record(refresh) = mode {
            self.scroll_layer_start_record(params, snapped, geometry, cache_key, scale, refresh);
        }
        let clip = self.content_mask().bounds;
        if let Some(block) = self.scroll_layers.blocks.get_mut(params.id) {
            block.mode = mode;
            block.geometry = Some(geometry);
            block.moved = false;
            if let Some(layer) = block.layer.as_mut() {
                layer.clip = clip;
                layer.offset = params.offset.clone();
            }
        }
        mode
    }

    /// Whether the state the block inherits from its ancestors keeps its content from being
    /// translated as a whole without changing pixels (PRF-004 R8): an element opacity (the
    /// opaque tiles would not fade), a fade or rounded clip scope or a curved overflow clip
    /// (their coverage applies per primitive, not to the composited tiles), or bounds, clip or
    /// content extent that are not finite or that leave the block nothing to show.
    fn inherited_geometry_unsafe(&self, params: &LayerParams) -> bool {
        let finite = |bounds: &Bounds<Pixels>| {
            [
                bounds.origin.x,
                bounds.origin.y,
                bounds.size.width,
                bounds.size.height,
            ]
            .iter()
            .all(|value| value.0.is_finite())
        };
        let mask = self.content_mask().bounds;
        self.element_opacity != 1.0
            || self.paint_scope.is_active()
            || self.curved_clip_ancestors > 0
            || !(finite(&params.bounds) && finite(&params.viewport) && finite(&mask))
            || !params.extent.width.0.is_finite()
            || !params.extent.height.0.is_finite()
            || mask.is_empty()
    }

    /// Decides between compositing and repainting a block that has a layer.
    fn scroll_layer_decide(
        &mut self,
        params: &LayerParams,
        geometry: Geometry,
        cache_key: &CacheKey,
        scale: f32,
    ) -> LayerMode {
        let Some(layer) = self
            .scroll_layers
            .blocks
            .get(params.id)
            .and_then(|block| block.layer.as_ref())
        else {
            return LayerMode::Direct;
        };
        // A resize or scale-factor change drops the cached content (R8).
        if layer.scale != scale
            || layer.extent != params.extent
            || layer.viewport.size != params.viewport.size
        {
            self.scroll_layer_drop(params.id, false);
            let promotable = self
                .scroll_layers
                .blocks
                .get(params.id)
                .is_some_and(|block| block.streak >= PROMOTION_FRAMES && block.backoff.ready());
            return if promotable {
                LayerMode::Record(Refresh::Promotion)
            } else {
                LayerMode::Direct
            };
        }
        let key = layer.key;
        let recorded_view = layer.recorded.view;
        let region = layer.region;
        let ambient_changed = layer.cache_key != *cache_key;
        let owner_stale = self.owner_reads_changed_since(layer.clock);
        let nested_changed = layer
            .nested
            .iter()
            .any(|(offset, recorded)| *offset.borrow() != *recorded);
        let layer_scroll = layer.recorded.scroll;
        let _ = recorded_view;
        let content_changed = ambient_changed
            || owner_stale
            || nested_changed
            || !self.row_reusable(key)
            || self.rows_owner_ambiguous_with(0);
        if self.scroll_layers.force_input && layer_scroll != geometry.scroll {
            // Input is about to be dispatched: the content is painted at current coordinates
            // first (R7).
            return LayerMode::Record(Refresh::Input);
        }
        if content_changed {
            return LayerMode::Record(Refresh::Content);
        }
        if !region.contains(&padded_view(
            geometry.view,
            geometry.extent,
            params.scrollable,
        )) {
            return LayerMode::Record(Refresh::Region);
        }
        let placement = Placement {
            offset: point(
                px((geometry.scroll.0 - layer_scroll.0) as f32 / scale),
                px((geometry.scroll.1 - layer_scroll.1) as f32 / scale),
            ),
            clip: self.content_mask().bounds,
            keep_states: true,
            scale_factor: scale,
        };
        if self.row_hover_changed(key, &placement) {
            return LayerMode::Record(Refresh::Input);
        }
        LayerMode::Composite
    }

    /// Whether anything the owner of the block read changed after `clock`.
    fn owner_reads_changed_since(&self, clock: u64) -> bool {
        let Some(owner) = self
            .retention
            .stack
            .last()
            .and_then(|key| self.retention.views.get(*key))
        else {
            return true;
        };
        let tracker = &self.retention.tracker;
        // Building the owner updates its own entity. That only means the content changed when
        // something notified it for a reason other than a scroll or an animation frame; an
        // owner rebuilt because a scrolled layer could not be placed has the same content.
        let own = owner.entity_id;
        let notified = self.retention.causes.unexplained;
        owner.reads.entities.iter().any(|entity| {
            (notified || *entity != own) && tracker.entity_updated_at(*entity) > clock
        }) || owner
            .reads
            .globals
            .iter()
            .any(|global| tracker.global_value_written_at(*global) > clock)
            || owner
                .reads
                .presence
                .iter()
                .any(|global| tracker.global_presence_written_at(*global) > clock)
            || owner.reads.versioned_changed()
    }

    /// Creates the layer of a block, or keeps the one that repaints.
    fn scroll_layer_start_record(
        &mut self,
        params: &LayerParams,
        snapped: Point<Pixels>,
        geometry: Geometry,
        cache_key: CacheKey,
        scale: f32,
        refresh: Refresh,
    ) {
        if refresh == Refresh::Input {
            self.frame_stats_mut().input_layer_rebuilds += 1;
        }
        let region = painted_region(geometry.view, geometry.extent, params.scrollable);
        let background = params.background.expect("layers need a background");
        let existing = self
            .scroll_layers
            .blocks
            .get(params.id)
            .and_then(|block| block.layer.as_ref().map(|layer| layer.key));
        let key = match existing {
            Some(key) => key,
            None => {
                let mut path = params.id.to_vec();
                path.push(ElementId::Name("scroll-layer".into()));
                let path = GlobalElementId::new(&path);
                let owner = self.current_view();
                let parent = self.retention.stack.last().copied();
                match self.retention.views.lookup(&path) {
                    Some(key) => key,
                    None => self.retention.views.create_layer(path, owner, parent),
                }
            }
        };
        let clock = self.retention.tracker.now();
        let new_layer = existing.is_none();
        let id_instance = if new_layer {
            let Some(id) = self.scroll_layers.allocate_id() else {
                return;
            };
            let instance = self.scroll_layers.next_instance;
            self.scroll_layers.next_instance += 1;
            self.scroll_layers
                .layer_blocks
                .insert(id, params.id.clone());
            Some((id, instance))
        } else {
            None
        };
        let block = self
            .scroll_layers
            .blocks
            .get_mut(params.id)
            .expect("the block exists");
        block.recording = Recording::default();
        match (&mut block.layer, id_instance) {
            (None, Some((id, instance))) => {
                block.layer = Some(Layer {
                    key,
                    id,
                    instance,
                    generation: 0,
                    content: None,
                    region,
                    recorded: geometry,
                    background,
                    cache_key,
                    scale,
                    bounds: params.bounds,
                    viewport: params.viewport,
                    extent: params.extent,
                    scroll_max: params.scroll_max,
                    offset: params.offset.clone(),
                    scrollable: params.scrollable,
                    clip: params.viewport,
                    scrollbar_style: params.style.scrollbar_color.map(|_| params.style.clone()),
                    nested: Vec::new(),
                    tiles: FxHashMap::default(),
                    clock,
                    dispatch_active: None,
                    refresh,
                    cost: CostHistory::default(),
                    repainted: true,
                });
            }
            (Some(layer), _) => {
                layer.region = region;
                layer.recorded = geometry;
                layer.background = background;
                layer.cache_key = cache_key;
                layer.scale = scale;
                layer.bounds = params.bounds;
                layer.viewport = params.viewport;
                layer.extent = params.extent;
                layer.scroll_max = params.scroll_max;
                layer.scrollable = params.scrollable;
                layer.scrollbar_style = params.style.scrollbar_color.map(|_| params.style.clone());
                layer.clock = clock;
                layer.refresh = refresh;
                layer.repainted = true;
            }
            (None, None) => {}
        }
        let _ = snapped;
    }

    /// The mode the block has in this frame, as decided when it was prepainted.
    pub(crate) fn scroll_layer_mode(&self, id: &GlobalElementId) -> LayerMode {
        self.scroll_layers
            .blocks
            .get(id)
            .map_or(LayerMode::Direct, |block| block.mode)
    }

    /// Drops the layer of a block that stopped being a layer candidate (its style, its
    /// ancestors or the window no longer allow it).
    fn scroll_layer_release(&mut self, id: &GlobalElementId) {
        if self.scroll_layers.has_layer(id) {
            self.scroll_layer_drop(id, false);
        }
        if let Some(block) = self.scroll_layers.blocks.get_mut(id) {
            block.mode = LayerMode::Direct;
        }
    }

    /// Drops the layer of a block: its record, its identity and (through the scene) its tiles.
    /// `demote` starts the cooldown of a demotion (R9, R10).
    fn scroll_layer_drop(&mut self, id: &GlobalElementId, demote: bool) {
        let Some(block) = self.scroll_layers.blocks.get_mut(id) else {
            return;
        };
        let Some(layer) = block.layer.take() else {
            return;
        };
        if demote {
            block.backoff.demote();
        }
        self.scroll_layers.layer_blocks.remove(&layer.id);
        self.scroll_layers.retired.push((layer.id, layer.instance));
        self.scroll_layers.free_ids.push(layer.id);
        self.row_drop(layer.key);
    }

    /// Whether the window is recording a retained list row: scroll layers do not nest in rows.
    fn in_row_recording(&self) -> bool {
        self.retention
            .stack
            .last()
            .and_then(|key| self.retention.views.get(*key))
            .is_some_and(|record| matches!(record.kind, ViewKind::Row | ViewKind::Layer))
    }

    /// Notes a scroll container nested in the content being recorded: the layer is repainted
    /// when it scrolls (R2).
    pub(crate) fn scroll_layer_note_nested(&mut self, offset: &Rc<RefCell<Point<Pixels>>>) {
        if let Some(block) = self
            .scroll_layers
            .active
            .as_ref()
            .and_then(|id| self.scroll_layers.blocks.get_mut(id))
        {
            block
                .recording
                .nested
                .push((offset.clone(), *offset.borrow()));
        }
    }

    /// Prepaints the content of a block into its layer: `children` runs under a content mask
    /// that covers the painted region, and its output is recorded.
    pub(crate) fn scroll_layer_record_prepaint<R>(
        &mut self,
        id: &GlobalElementId,
        children: impl FnOnce(&mut Window) -> R,
    ) -> R {
        let Some((key, extent, clip, anchor)) = self.scroll_layer_record_context(id) else {
            return children(self);
        };
        self.layer_begin(key);
        let active = self
            .next_frame
            .dispatch_tree
            .active_node_id()
            .map(crate::DispatchNodeId::index);
        if let Some(layer) = self
            .scroll_layers
            .blocks
            .get_mut(id)
            .and_then(|block| block.layer.as_mut())
        {
            layer.dispatch_active = active;
        }
        let animations = self.animation_requests.get();
        self.scroll_layers.active = Some(id.clone());
        let result = self.row_prepaint(key, extent, clip, anchor, children);
        self.scroll_layers.active = None;
        if self.animation_requests.get() != animations
            && let Some(block) = self.scroll_layers.blocks.get_mut(id)
        {
            block.recording.animated = true;
        }
        result
    }

    /// The record, the recording mask, the visible clip and the anchor of a block's layer.
    fn scroll_layer_record_context(
        &self,
        id: &GlobalElementId,
    ) -> Option<(ViewKey, ContentMask<Pixels>, Bounds<Pixels>, Point<Pixels>)> {
        let block = self.scroll_layers.blocks.get(id)?;
        let layer = block.layer.as_ref()?;
        let geometry = block.geometry?;
        let scale = layer.scale;
        let window = geometry.to_window();
        let region = layer.region.moved(window.0, window.1);
        let extent = ContentMask {
            bounds: Bounds {
                origin: point(
                    px(region.left as f32 / scale),
                    px(region.top as f32 / scale),
                ),
                size: size(
                    px(region.width() as f32 / scale),
                    px(region.height() as f32 / scale),
                ),
            },
        };
        let anchor = point(
            px(geometry.scroll.0 as f32 / scale),
            px(geometry.scroll.1 as f32 / scale),
        );
        Some((layer.key, extent, self.content_mask().bounds, anchor))
    }

    /// Starts the build of the record of a layer.
    fn layer_begin(&mut self, key: ViewKey) {
        let parent = self.retention.stack.last().copied();
        self.retention.views.begin_rebuild(key, parent);
        self.retention.views.reset_reads(key);
        self.begin_view_phase(key);
    }

    /// Paints the content of a block into its layer and shows it: the content is painted
    /// under the recording mask into a scene of its own, which becomes the layer's tiles and
    /// overlay; `after` paints what is drawn over the content live (classic scrollbars).
    pub(crate) fn scroll_layer_record_paint(
        &mut self,
        id: &GlobalElementId,
        children: impl FnOnce(&mut Window),
        after: impl FnOnce(&mut Window),
    ) {
        let Some((key, extent, _, _)) = self.scroll_layer_record_context(id) else {
            children(self);
            after(self);
            return;
        };
        let paint_start = self.paint_index();
        let animations = self.animation_requests.get();
        let handlers = self.next_frame.input_handlers.len();
        self.scroll_layers.active = Some(id.clone());
        self.row_paint(key, extent, children);
        self.scroll_layers.active = None;
        if let Some(block) = self.scroll_layers.blocks.get_mut(id) {
            block.recording.animated |= self.animation_requests.get() != animations;
            block.recording.text_input |= self.next_frame.input_handlers.len() > handlers;
        }
        self.scroll_layer_bake(id, key);
        self.scroll_layer_show(id, key);
        after(self);
        self.scroll_layer_finish_paint(key, paint_start);
        self.scroll_layer_account(id);
    }

    /// Converts the recorded scene of the content into the layer's tiles and overlay, or shows
    /// it directly when it cannot be cached as a whole.
    fn scroll_layer_bake(&mut self, id: &GlobalElementId, key: ViewKey) {
        let Some(block) = self.scroll_layers.blocks.get_mut(id) else {
            return;
        };
        let recording = std::mem::take(&mut block.recording);
        let (animated, text_input) = (recording.animated, recording.text_input);
        let Some(geometry) = block.geometry else {
            return;
        };
        let (scene, replayable) = match self
            .retention
            .views
            .get_mut(key)
            .and_then(|record| record.row.as_deref_mut())
        {
            Some(row) => (std::mem::take(&mut row.scene), row.replayable),
            None => return,
        };
        let layer = block.layer.as_mut().expect("recording has a layer");
        let window = geometry.to_window();
        let to_content = point(
            ScaledPixels(-(window.0 as f32)),
            ScaledPixels(-(window.1 as f32)),
        );
        // A curved overflow clip opened by an ancestor while painting cuts every primitive on its
        // own; the opaque tiles would not reproduce that (the check while prepainting misses
        // views rebuilt inside a replayed owner).
        let curved_clip = self.next_frame.scene.curved_clip_active();
        let content = if animated || text_input || !replayable || curved_clip {
            None
        } else {
            LayerContent::from_recording(&scene, to_content, layer.region.bounds()).ok()
        };
        match content {
            Some(content) => {
                layer.generation += 1;
                layer.content = Some(Arc::new(content));
                layer.nested = recording.nested;
            }
            None => {
                // Not cacheable: the layer ends, the content shows directly (R8).
                layer.content = None;
                let clip = layer.clip;
                let scale = layer.scale;
                if let Some(block) = self.scroll_layers.blocks.get_mut(id) {
                    block.retry = RETRY_FRAMES;
                }
                self.scroll_layer_drop(id, false);
                self.show_recorded_scene(&scene, clip, scale);
                return;
            }
        }
        // The recording mask holds the whole region; keep the scene for nothing else.
        drop(scene);
    }

    /// Replays a recorded scene unmoved, clipped to `clip` (the content as direct drawing
    /// shows it).
    fn show_recorded_scene(&mut self, scene: &crate::Scene, clip: Bounds<Pixels>, scale: f32) {
        let clip = clip.scale(scale);
        self.next_frame.scene.replay_moved(
            0..scene.len(),
            scene,
            point(ScaledPixels(0.0), ScaledPixels(0.0)),
            Some(clip),
        );
    }

    /// Composites the layer: tile sprites for the visible tiles at the current offset, then
    /// the overlay replayed live over them.
    fn scroll_layer_show(&mut self, id: &GlobalElementId, key: ViewKey) {
        let _ = key;
        let frame_number = self.scroll_layers.frame;
        let Some(block) = self.scroll_layers.blocks.get_mut(id) else {
            return;
        };
        let Some(geometry) = block.geometry else {
            return;
        };
        let Some(layer) = block.layer.as_mut() else {
            return;
        };
        let Some(content) = layer.content.clone() else {
            return;
        };
        block.composited = frame_number;
        let scale = layer.scale;
        let clip = layer.clip.scale(scale);
        let window = geometry.to_window();
        let shown = TileCoord::covering(geometry.view.bounds()).unwrap_or_default();
        // Tiles whose content changed since the preceding generation, or that were never
        // rasterized, are dirty; the rest are current (R4).
        let mut dirty = Vec::new();
        for coord in &shown {
            let hash = content.tile_hash(*coord);
            let current = match layer.tiles.get(coord) {
                Some((generation, _)) if *generation == layer.generation => true,
                Some((generation, previous))
                    if *generation + 1 == layer.generation && *previous == hash =>
                {
                    true
                }
                _ => false,
            };
            layer.tiles.insert(*coord, (layer.generation, hash));
            if !current {
                dirty.push(*coord);
            }
        }
        let frame = LayerFrame::new(
            layer.id,
            layer.instance,
            layer.generation,
            layer.background,
            content.clone(),
            shown.clone(),
            dirty.clone(),
        );
        let layer_id = layer.id;
        let repainted = layer.repainted;
        let dirty_count = dirty.len() as u64;
        self.next_frame.scene.layers.push(frame);
        let mask = ContentMask { bounds: clip };
        for coord in &shown {
            let tile = coord.bounds();
            let bounds = Bounds {
                origin: point(
                    ScaledPixels((window.0 + tile.origin.x.0) as f32),
                    ScaledPixels((window.1 + tile.origin.y.0) as f32),
                ),
                size: size(
                    ScaledPixels(tile.size.width.0 as f32),
                    ScaledPixels(tile.size.height.0 as f32),
                ),
            };
            self.next_frame
                .scene
                .insert_primitive(tile_sprite(layer_id, *coord, bounds, mask));
        }
        let overlay = content.overlay();
        self.next_frame.scene.replay_moved(
            0..overlay.len(),
            overlay,
            point(ScaledPixels(window.0 as f32), ScaledPixels(window.1 as f32)),
            Some(clip),
        );
        let stats = self.frame_stats_mut();
        stats.tiles_dirty += dirty_count;
        if repainted {
            stats.layer_repaints += 1;
        } else {
            stats.layer_composites += 1;
        }
    }

    /// Ends the paint range of a layer record after the content was shown: the range covers
    /// the scene output as well, so a parent replayed around the layer skips it all.
    fn scroll_layer_finish_paint(&mut self, key: ViewKey, start: super::PaintIndex) {
        let end = self.paint_index();
        if let Some(record) = self.retention.views.get_mut(key) {
            record.paint = Some(start..end);
        }
    }

    /// Replays the non-scene prepaint output of a layer record at the block's current offset.
    pub(crate) fn scroll_layer_composite_prepaint(&mut self, id: &GlobalElementId, cx: &mut App) {
        let Some((key, placement)) = self.scroll_layer_placement(id) else {
            return;
        };
        self.row_replay_prepaint(key, &placement, cx);
    }

    /// Replays the non-scene paint output of a layer record, composites the tiles and paints
    /// what goes over the content (`after`).
    pub(crate) fn scroll_layer_composite_paint(
        &mut self,
        id: &GlobalElementId,
        cx: &mut App,
        after: impl FnOnce(&mut Window),
    ) {
        let Some((key, placement)) = self.scroll_layer_placement(id) else {
            after(self);
            return;
        };
        let start = self.paint_index();
        self.row_replay_paint(key, &placement, cx);
        if let Some(block) = self.scroll_layers.blocks.get_mut(id)
            && let Some(layer) = block.layer.as_mut()
        {
            layer.repainted = false;
        }
        self.scroll_layer_show(id, key);
        after(self);
        self.scroll_layer_finish_paint(key, start);
        self.scroll_layer_account(id);
    }

    /// The record of a block's layer and where its output goes at the block's current offset.
    fn scroll_layer_placement(&self, id: &GlobalElementId) -> Option<(ViewKey, Placement)> {
        let block = self.scroll_layers.blocks.get(id)?;
        let layer = block.layer.as_ref()?;
        let geometry = block.geometry?;
        let scale = layer.scale;
        let placement = Placement {
            offset: point(
                px((geometry.scroll.0 - layer.recorded.scroll.0) as f32 / scale),
                px((geometry.scroll.1 - layer.recorded.scroll.1) as f32 / scale),
            ),
            clip: layer.clip,
            keep_states: true,
            scale_factor: scale,
        };
        Some((layer.key, placement))
    }

    /// Updates the cost accounting of the layer after a frame (R9) and demotes it when its
    /// content changes too often or it costs as much as drawing the content directly.
    fn scroll_layer_account(&mut self, id: &GlobalElementId) {
        let frame = self.scroll_layers.frame;
        let Some(block) = self.scroll_layers.blocks.get_mut(id) else {
            return;
        };
        let Some(layer) = block.layer.as_mut() else {
            return;
        };
        let repainted = std::mem::replace(&mut layer.repainted, false);
        // A content refresh is a repaint because the content changed, not because the
        // retained region moved with the viewport.
        let refresh = matches!(layer.refresh, Refresh::Content | Refresh::Input);
        let region_area = layer.region.area();
        let view_area = layer.recorded.view.area().max(1.0);
        let work = if repainted {
            (REPAINT_COST * (region_area / view_area) as f32).clamp(0.0, WORK_CLAMP)
        } else {
            0.0
        };
        let demote = layer
            .cost
            .complete_frame(frame, repainted, refresh, work)
            .is_some();
        if demote {
            self.scroll_layer_drop(id, true);
        }
    }

    /// Whether a layer of the block would composite in the next frame at an offset other than
    /// the one it was recorded at (the content is "moved", R7).
    pub(crate) fn scroll_layers_moved(&self) -> bool {
        self.scroll_layers
            .blocks
            .values()
            .any(|block| block.layer.as_ref().is_some_and(Layer::displaced))
    }

    /// Starts a frame of the scroll layers: scrolls absorbed by layers move their records,
    /// the cooldowns of demoted blocks advance.
    pub(crate) fn begin_scroll_layers_frame(&mut self) {
        let updated = self.retention.causes.unexplained;
        let layers = &mut self.scroll_layers;
        layers.frame += 1;
        layers.frame_tiles = 0;
        layers.animating = std::mem::take(&mut *self.animation_views.borrow_mut());
        OFFSET_READS.with(|reads| {
            let mut reads = reads.borrow_mut();
            reads.active = true;
            reads.read.clear();
        });
        let mut absorbed: Vec<GlobalElementId> = layers.absorbed.drain().collect();
        if layers.force_input {
            absorbed.extend(layers.blocks.iter().filter_map(|(id, block)| {
                block
                    .layer
                    .as_ref()
                    .filter(|layer| layer.displaced())
                    .map(|_| id.clone())
            }));
        }
        for block in layers.blocks.values_mut() {
            // Frames pass for the backoff of a demoted block; a content update during the
            // cooldown extends it (R10).
            block.backoff.frame(updated, block.layer.is_some());
        }
        for id in absorbed {
            let key = self
                .scroll_layers
                .blocks
                .get(&id)
                .and_then(|block| block.layer.as_ref())
                .map(|layer| layer.key);
            if let Some(key) = key {
                self.retention.views.flag_moved(key);
                if let Some(block) = self.scroll_layers.blocks.get_mut(&id) {
                    block.moved = true;
                }
            }
        }
    }

    /// Ends the frame: layers whose record was swept or that were not composited for 120
    /// frames are dropped, retired identities travel with the scene.
    pub(crate) fn finish_scroll_layers_frame(&mut self) {
        let frame = self.scroll_layers.frame;
        self.scroll_layers.force_input = false;
        // Blocks whose exact offset was read while they were prepainted (R2) stop using layers.
        let read: Vec<GlobalElementId> = OFFSET_READS.with(|reads| {
            let mut reads = reads.borrow_mut();
            reads.active = false;
            let read = std::mem::take(&mut reads.read);
            self.scroll_layers
                .blocks
                .iter_mut()
                .filter(|(_, block)| block.prepainted == frame)
                .filter_map(|(id, block)| {
                    block.offset_read = read.contains(&block.offset_ptr);
                    (block.offset_read && block.layer.is_some()).then(|| id.clone())
                })
                .collect()
        });
        for id in read {
            self.scroll_layer_drop(&id, false);
        }
        let gone: Vec<GlobalElementId> = self
            .scroll_layers
            .blocks
            .iter()
            .filter(|(_, block)| {
                block.layer.as_ref().is_some_and(|layer| {
                    self.retention.views.get(layer.key).is_none()
                        || inactive(frame, block.composited)
                })
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in gone {
            self.scroll_layer_drop(&id, false);
        }
        self.scroll_layers.blocks.retain(|_, block| {
            block.layer.is_some()
                || block.backoff.pending()
                || frame.saturating_sub(block.seen) < INACTIVE_FRAMES
        });
        let retired = std::mem::take(&mut self.scroll_layers.retired);
        for (id, instance) in retired {
            self.next_frame.scene.layers.retire(id, instance);
        }
    }

    /// Records that the block scrolled without notifying its owner view: its layer moves in
    /// the next frame (PRF-004 R2). Returns `false` when the block has no layer to move, or
    /// when something read the exact scroll offset.
    pub(crate) fn scroll_layer_absorb_scroll(&mut self, id: &GlobalElementId) -> bool {
        if !self
            .scroll_layers
            .blocks
            .get(id)
            .is_some_and(|block| block.layer.is_some() && !block.offset_read)
        {
            return false;
        }
        self.scroll_layers.absorbed.insert(id.clone());
        self.invalidator.set_dirty(true);
        true
    }

    /// Draws a frame in which every layer that composited at a new offset paints its content at
    /// current coordinates, so that input dispatched next observes the geometry the user sees
    /// (PRF-004 R7).
    pub(crate) fn repaint_scroll_layers_for_input(&mut self, cx: &mut App) {
        if !self.invalidator.not_drawing() {
            return;
        }
        self.scroll_layers.force_input = true;
        self.invalidator.set_dirty(true);
        self.draw(cx).clear(cx);
        self.scroll_layers.force_input = false;
    }

    /// Whether the layer record `key` can be placed at its block's new offset while its owner
    /// is replayed around it: nothing it depends on changed, the retained region still holds
    /// the viewport and its margin, no pointer hover changed under the moved content and the
    /// block did not jump. Fixes the geometry the placement uses.
    pub(crate) fn scroll_layer_can_splice(&mut self, key: ViewKey) -> bool {
        let Some(id) = self.scroll_layer_of(key) else {
            return false;
        };
        if !self.scroll_layers.enabled || !self.retention_reuse_allowed() {
            return false;
        }
        let Some(geometry) = self.scroll_layer_current_geometry(&id) else {
            return false;
        };
        let Some(block) = self.scroll_layers.blocks.get(&id) else {
            return false;
        };
        let Some(layer) = block.layer.as_ref() else {
            return false;
        };
        let scale = layer.scale;
        let previous = block.last_offset.map_or(layer.recorded.scroll, |offset| {
            (device(offset.x, scale), device(offset.y, scale))
        });
        let jump = (layer.scrollable.0
            && (geometry.scroll.0 - previous.0).abs() >= geometry.view.width())
            || (layer.scrollable.1
                && (geometry.scroll.1 - previous.1).abs() >= geometry.view.height());
        let nested_changed = layer
            .nested
            .iter()
            .any(|(offset, recorded)| *offset.borrow() != *recorded);
        let contained = layer.region.contains(&padded_view(
            geometry.view,
            geometry.extent,
            layer.scrollable,
        ));
        let placement = Placement {
            offset: point(
                px((geometry.scroll.0 - layer.recorded.scroll.0) as f32 / scale),
                px((geometry.scroll.1 - layer.recorded.scroll.1) as f32 / scale),
            ),
            clip: layer.clip,
            keep_states: true,
            scale_factor: scale,
        };
        let layer_key = layer.key;
        let forced = self.scroll_layers.force_input && layer.recorded.scroll != geometry.scroll;
        if jump || nested_changed || !contained || forced {
            return false;
        }
        if !self.row_reusable(layer_key) || self.row_hover_changed(layer_key, &placement) {
            return false;
        }
        if let Some(block) = self.scroll_layers.blocks.get_mut(&id) {
            block.geometry = Some(geometry);
        }
        true
    }

    /// Replays the layer of a block that scrolled while the views around it stayed clean, in
    /// place of the stretch of their recorded output the layer occupies (they are replayed
    /// around it, like around a rebuilt child view).
    pub(crate) fn scroll_layer_splice_prepaint(
        &mut self,
        key: ViewKey,
        map: &SpliceMap,
        attachment: Option<crate::DispatchNodeId>,
        cx: &mut App,
    ) -> bool {
        let Some(id) = self.scroll_layer_of(key) else {
            return false;
        };
        // The record stays nested in the view that owns the block, which may itself be nested
        // in the view being replayed: a scroll the layer cannot absorb must reach that owner.
        let Some(owner) = self
            .retention
            .views
            .get(key)
            .and_then(|record| record.parent)
        else {
            return false;
        };
        let frame = self.scroll_layers.frame;
        let Some(block) = self.scroll_layers.blocks.get_mut(&id) else {
            return false;
        };
        let (Some(geometry), Some(layer)) = (block.geometry, block.layer.as_ref()) else {
            return false;
        };
        let scale = layer.scale;
        let old_parent = layer.dispatch_active.map(crate::DispatchNodeId::from_index);
        // The scroll bookkeeping of a block that is not prepainted this frame.
        let snapped = point(
            px(geometry.scroll.0 as f32 / scale),
            px(geometry.scroll.1 as f32 / scale),
        );
        let previous = block.last_offset.replace(snapped);
        if previous.is_some_and(|previous| previous != snapped) {
            block.streak = if block.streak_frame + 1 == frame {
                block.streak + 1
            } else {
                1
            };
            block.streak_frame = frame;
        }
        block.seen = frame;
        block.mode = LayerMode::Composite;
        block.moved = false;
        if let Some(record) = self.retention.views.get_mut(key) {
            record.moved = false;
        }
        // The content attaches where it attached when it was recorded.
        let new_parent = old_parent.and_then(|old| map.resolve(old)).or(attachment);
        if let Some(parent) = new_parent {
            self.next_frame.dispatch_tree.set_active_node(parent);
        }
        self.retention.stack.push(owner);
        self.scroll_layer_composite_prepaint(&id, cx);
        self.retention.stack.pop();
        if let Some(attachment) = attachment {
            self.next_frame.dispatch_tree.set_active_node(attachment);
        }
        true
    }

    /// The paint-phase counterpart of [`Self::scroll_layer_splice_prepaint`].
    pub(crate) fn scroll_layer_splice_paint(&mut self, key: ViewKey, cx: &mut App) {
        let Some(id) = self.scroll_layer_of(key) else {
            return;
        };
        let (scrollbar, bounds, scroll_max, offset) = {
            let Some(layer) = self
                .scroll_layers
                .blocks
                .get(&id)
                .and_then(|block| block.layer.as_ref())
            else {
                return;
            };
            (
                layer.scrollbar_style.clone(),
                layer.bounds,
                layer.scroll_max,
                *layer.offset.borrow(),
            )
        };
        self.scroll_layer_composite_paint(&id, cx, |window| {
            if let Some(style) = scrollbar {
                crate::elements::paint_classic_scrollbars(
                    &style, bounds, scroll_max, offset, window,
                );
            }
        });
    }

    /// The block of a layer record.
    fn scroll_layer_of(&self, key: ViewKey) -> Option<GlobalElementId> {
        self.scroll_layers
            .blocks
            .iter()
            .find(|(_, block)| block.layer.as_ref().is_some_and(|layer| layer.key == key))
            .map(|(id, _)| id.clone())
    }

    /// The geometry of a block's layer at the block's current scroll offset, clamped to the
    /// scrollable range like the block does when it is prepainted.
    fn scroll_layer_current_geometry(&mut self, id: &GlobalElementId) -> Option<Geometry> {
        let layer = self
            .scroll_layers
            .blocks
            .get(id)
            .and_then(|block| block.layer.as_ref())?;
        let scale = layer.scale;
        let (max, offset_state, bounds, viewport, extent, recorded) = (
            layer.scroll_max,
            layer.offset.clone(),
            layer.bounds,
            layer.viewport,
            layer.extent,
            layer.recorded,
        );
        let mut offset = offset_state.borrow_mut();
        offset.x = offset.x.clamp(-max.x, px(0.));
        offset.y = offset.y.clamp(-max.y, px(0.));
        let snapped = point(self.pixel_snap(offset.x), self.pixel_snap(offset.y));
        drop(offset);
        let scroll = (device(snapped.x, scale), device(snapped.y, scale));
        let view_window = outward(viewport.scale(scale));
        let origin = recorded.origin;
        let _ = (bounds, extent);
        Some(Geometry {
            origin,
            scroll,
            view: view_window.moved(-(origin.0 + scroll.0), -(origin.1 + scroll.1)),
            extent: recorded.extent,
        })
    }

    /// Converts a stored position of the layer's content to window coordinates (R7).
    #[cfg(any(test, feature = "test-support"))]
    pub fn scroll_layer_count(&self) -> usize {
        self.scroll_layers.layer_count()
    }

    /// Enables or disables scroll layers for this window (enabled by default): a test and
    /// measurement switch (PRF-004), so that runs with and without layers can be compared with
    /// everything else equal. Disabling drops every layer; content is then drawn directly, with
    /// the same snapped scroll offsets.
    pub fn set_scroll_layers_enabled(&mut self, enabled: bool) {
        self.scroll_layers.enabled = enabled;
        if !enabled {
            let ids: Vec<GlobalElementId> = self.scroll_layers.blocks.keys().cloned().collect();
            for id in ids {
                self.scroll_layer_drop(&id, false);
            }
            self.refresh();
        }
    }

    /// Whether scroll layers are enabled for this window.
    pub fn scroll_layers_enabled(&self) -> bool {
        self.scroll_layers.enabled
    }

    /// The state of the scroll block with a layer or a backoff, for tests that drive exactly one
    /// block.
    #[cfg(test)]
    pub(crate) fn scroll_layer_probe(&self) -> ScrollLayerProbe {
        let block = self.scroll_layers.blocks.values().next();
        let layer = block.and_then(|block| block.layer.as_ref());
        ScrollLayerProbe {
            layer: layer.is_some(),
            instance: layer.map_or(0, |layer| layer.instance),
            generation: layer.map_or(0, |layer| layer.generation),
            composited: block.is_some_and(|block| block.composited == self.scroll_layers.frame),
            region: layer.map(|layer| {
                (
                    layer.region.left,
                    layer.region.top,
                    layer.region.right,
                    layer.region.bottom,
                )
            }),
            recorded_scroll: layer.map(|layer| layer.recorded.scroll),
            cooldown: block.map_or(0, |block| block.backoff.cooldown),
            demotions: block.map_or(0, |block| block.backoff.demotions),
            retry: block.map_or(0, |block| block.retry),
            offset_read: block.is_some_and(|block| block.offset_read),
        }
    }
}
