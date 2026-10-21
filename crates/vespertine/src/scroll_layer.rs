//! Scroll-layer transport (PRF-004): the types a scene carries so that a renderer can draw the
//! scrolled content of a block as cached opaque tiles, plus the tile cache bookkeeping both the
//! renderer and the tests use.
//!
//! A layer is recorded in *content space*: device pixels measured from the content origin of
//! the block, which does not move when the block scrolls. The recorded paint operations are
//! split in two ordered lists:
//!
//! * the **baked** primitives, which a renderer rasterizes into 512 x 512 device-pixel tiles
//!   (cleared to the opaque background quad of the block first), and
//! * the **overlay**: every path, and every later primitive that overlaps an overlay
//!   primitive. Paths stay out of tiles because their derivative-based antialiasing changes at
//!   odd translations. The overlay is replayed live, in its original order, over the tiles.
//!
//! No overlay primitive precedes an overlapping baked primitive by construction, so drawing
//! the tiles first and the overlay on top reproduces drawing the content directly.

use crate::{
    AtlasTextureId, AtlasTextureKind, AtlasTile, Background, Bounds, ContentMask, Corners,
    DevicePixels, ImageEffects, PaddedBool32, PolychromeSprite, Primitive, Quad, ScaledPixels,
    Scene, ScopeParams, TileId, point, scene::PaintOperation, size,
};
use std::{
    collections::hash_map::DefaultHasher,
    fmt::Write as _,
    hash::Hasher,
    sync::{Arc, Mutex},
};
use vespertine_collections::FxHashMap;

/// The edge of a tile in device pixels.
pub const TILE_SIZE: i32 = 512;

/// Layer identities live in a 24-bit domain.
pub const LAYER_IDENTITY_LIMIT: u32 = 1 << 24;

/// The smallest and largest signed tile coordinate on each axis.
pub const MIN_TILE_COORDINATE: i32 = -2048;
/// See [`MIN_TILE_COORDINATE`].
pub const MAX_TILE_COORDINATE: i32 = 2047;

/// Bytes of one tile texture (512 x 512 x 4).
pub const TILE_BYTES: u64 = (TILE_SIZE as u64) * (TILE_SIZE as u64) * 4;

/// Tile texture memory a window may hold: 64 MiB, that is 64 tiles.
pub const TILE_BUDGET_BYTES: u64 = 64 * 1024 * 1024;

/// Tiles a window may hold at once.
pub const TILE_BUDGET_TILES: usize = (TILE_BUDGET_BYTES / TILE_BYTES) as usize;

/// Freed tile textures a cache keeps for reuse.
pub const TILE_POOL_SIZE: usize = 8;

/// Frames a layer's tiles may go unused before the cache drops them.
pub const TILE_IDLE_FRAMES: u64 = 120;

/// The marker bit of the texture index of tile sprites: tile textures are never atlas textures.
const TILE_TEXTURE_FLAG: u32 = 1 << 31;

/// The identity of a scroll layer, a value below 2^24.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LayerId(u32);

impl LayerId {
    /// The identity `id`, or `None` outside the 24-bit domain.
    pub fn new(id: u32) -> Option<Self> {
        (id < LAYER_IDENTITY_LIMIT).then_some(Self(id))
    }

    /// The numeric value of the identity.
    pub fn get(self) -> u32 {
        self.0
    }
}

/// A signed tile coordinate; each axis ranges over -2048 through 2047.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileCoord {
    x: i16,
    y: i16,
}

impl TileCoord {
    /// The coordinate (`x`, `y`), or `None` when an axis is outside its range.
    pub fn new(x: i32, y: i32) -> Option<Self> {
        let range = MIN_TILE_COORDINATE..=MAX_TILE_COORDINATE;
        (range.contains(&x) && range.contains(&y)).then_some(Self {
            x: x as i16,
            y: y as i16,
        })
    }

    /// The horizontal tile index.
    pub fn x(self) -> i32 {
        i32::from(self.x)
    }

    /// The vertical tile index.
    pub fn y(self) -> i32 {
        i32::from(self.y)
    }

    /// The 24-bit packing of the coordinate, as stored in a tile sprite's tile id.
    pub fn pack(self) -> u32 {
        let bias = -MIN_TILE_COORDINATE;
        (((self.x() + bias) as u32) << 12) | ((self.y() + bias) as u32)
    }

    /// The coordinate packed by [`TileCoord::pack`].
    pub fn unpack(packed: u32) -> Self {
        let bias = -MIN_TILE_COORDINATE;
        Self {
            x: (((packed >> 12) & 0xfff) as i32 - bias) as i16,
            y: ((packed & 0xfff) as i32 - bias) as i16,
        }
    }

    /// The bounds of the tile in content space (device pixels).
    pub fn bounds(self) -> Bounds<DevicePixels> {
        Bounds {
            origin: point(
                DevicePixels(self.x() * TILE_SIZE),
                DevicePixels(self.y() * TILE_SIZE),
            ),
            size: size(DevicePixels(TILE_SIZE), DevicePixels(TILE_SIZE)),
        }
    }

    /// The tiles that intersect `bounds` (content-space device pixels), row by row, or `None`
    /// when one of them is outside the coordinate range.
    pub fn covering(bounds: Bounds<DevicePixels>) -> Option<Vec<TileCoord>> {
        if bounds.size.width.0 <= 0 || bounds.size.height.0 <= 0 {
            return Some(Vec::new());
        }
        let left = bounds.origin.x.0.div_euclid(TILE_SIZE);
        let top = bounds.origin.y.0.div_euclid(TILE_SIZE);
        let right = (bounds.origin.x.0 + bounds.size.width.0 - 1).div_euclid(TILE_SIZE);
        let bottom = (bounds.origin.y.0 + bounds.size.height.0 - 1).div_euclid(TILE_SIZE);
        let mut tiles = Vec::new();
        for y in top..=bottom {
            for x in left..=right {
                tiles.push(TileCoord::new(x, y)?);
            }
        }
        Some(tiles)
    }
}

/// The texture identity of the tiles of `layer`. It carries a marker bit in the texture index,
/// so a tile texture is never confused with an atlas texture; the tile coordinate travels in
/// the tile id of the sprite (see [`tile_sprite_tile`]).
pub fn tile_texture_id(layer: LayerId) -> AtlasTextureId {
    AtlasTextureId {
        index: TILE_TEXTURE_FLAG | layer.get(),
        kind: AtlasTextureKind::Polychrome,
    }
}

/// Whether `texture` identifies the tiles of a scroll layer rather than an atlas texture.
pub fn is_tile_texture(texture: AtlasTextureId) -> bool {
    texture.index & TILE_TEXTURE_FLAG != 0
}

/// The atlas tile a tile sprite samples: the whole texture of tile `coord` of `layer`.
pub fn tile_sprite_tile(layer: LayerId, coord: TileCoord) -> AtlasTile {
    AtlasTile {
        texture_id: tile_texture_id(layer),
        tile_id: TileId(coord.pack()),
        padding: 0,
        bounds: Bounds {
            origin: point(DevicePixels(0), DevicePixels(0)),
            size: size(DevicePixels(TILE_SIZE), DevicePixels(TILE_SIZE)),
        },
    }
}

/// The layer and tile coordinate a tile sprite refers to, or `None` for an atlas sprite.
pub fn tile_identity(tile: &AtlasTile) -> Option<(LayerId, TileCoord)> {
    is_tile_texture(tile.texture_id).then(|| {
        (
            LayerId(tile.texture_id.index & !TILE_TEXTURE_FLAG),
            TileCoord::unpack(tile.tile_id.0),
        )
    })
}

/// A polychrome sprite drawing tile `coord` of `layer` at `bounds` (device pixels, a whole
/// number of them), clipped by `content_mask`.
pub fn tile_sprite(
    layer: LayerId,
    coord: TileCoord,
    bounds: Bounds<ScaledPixels>,
    content_mask: ContentMask<ScaledPixels>,
) -> PolychromeSprite {
    PolychromeSprite {
        order: 0,
        pad: 0,
        grayscale: PaddedBool32::from(false),
        opacity: 1.0,
        bounds,
        content_mask,
        corner_radii: Corners::default(),
        tile: tile_sprite_tile(layer, coord),
        scope: ScopeParams::default(),
        // Whole-device-pixel placement samples texel centers; the nearest-texel flag makes
        // that exact: no resampling touches a tile pixel (PRF-004 R5).
        effects: ImageEffects {
            flags: ImageEffects::NEAREST,
            ..ImageEffects::default()
        },
    }
}

/// Why recorded content cannot be baked into tiles (PRF-004 R8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IneligibleContent {
    /// The content paints a platform surface.
    PlatformSurface,
    /// The content paints a backdrop blur, which samples what is behind it.
    BackdropBlur,
    /// The content opens a curved overflow clip.
    CurvedClip,
    /// A primitive carries fade or rounded-clip scope coordinates of an enclosing element.
    ScopedPrimitive,
}

fn clipped_bounds(primitive: &Primitive) -> Bounds<ScaledPixels> {
    primitive
        .painted_bounds()
        .intersect(&primitive.content_mask().bounds)
}

/// Clears what identifies a primitive in its scene but is not content: its draw order and path
/// id, so that the same content hashes equally wherever it was recorded.
pub(crate) fn strip_identity(primitive: &mut Primitive) {
    match primitive {
        Primitive::Shadow(shadow) => shadow.order = 0,
        Primitive::Quad(quad) => quad.order = 0,
        Primitive::Path(path) => {
            path.order = 0;
            path.id = crate::PathId(0);
        }
        Primitive::Underline(underline) => underline.order = 0,
        Primitive::MonochromeSprite(sprite) => sprite.order = 0,
        Primitive::SubpixelSprite(sprite) => sprite.order = 0,
        Primitive::PolychromeSprite(sprite) => sprite.order = 0,
        Primitive::Surface(surface) => surface.order = 0,
    }
}

/// An atlas tile that stands for "any tile", used where only the meaning of a sprite is compared.
#[cfg(any(test, feature = "test-support"))]
pub(crate) fn blank_tile() -> AtlasTile {
    AtlasTile {
        texture_id: AtlasTextureId {
            index: 0,
            kind: AtlasTextureKind::Polychrome,
        },
        tile_id: TileId(0),
        padding: 0,
        bounds: Bounds::default(),
    }
}

/// The recorded content of a layer, shared between the frames that show it.
pub struct LayerContent {
    baked: Scene,
    overlay: Scene,
    region: Bounds<DevicePixels>,
    baked_count: usize,
    overlay_count: usize,
    tile_hashes: Mutex<FxHashMap<TileCoord, u64>>,
}

impl std::fmt::Debug for LayerContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LayerContent")
            .field("region", &self.region)
            .field("baked", &self.baked_count)
            .field("overlay", &self.overlay_count)
            .finish()
    }
}

/// Vertical bands of 256 device pixels used to find overlapping overlay primitives quickly.
const BAND: f32 = 256.0;

#[derive(Default)]
struct OverlayIndex {
    bands: FxHashMap<i32, Vec<Bounds<ScaledPixels>>>,
}

impl OverlayIndex {
    fn bands_of(bounds: &Bounds<ScaledPixels>) -> std::ops::RangeInclusive<i32> {
        let top = (bounds.origin.y.0 / BAND).floor() as i32;
        let bottom = ((bounds.origin.y.0 + bounds.size.height.0) / BAND).floor() as i32;
        top..=bottom
    }

    fn insert(&mut self, bounds: Bounds<ScaledPixels>) {
        for band in Self::bands_of(&bounds) {
            self.bands.entry(band).or_default().push(bounds);
        }
    }

    fn overlaps(&self, bounds: &Bounds<ScaledPixels>) -> bool {
        Self::bands_of(bounds).any(|band| {
            self.bands
                .get(&band)
                .is_some_and(|list| list.iter().any(|other| other.intersects(bounds)))
        })
    }
}

impl LayerContent {
    /// Builds the content of a layer from the paint operations of the recorded scene `source`,
    /// moved into content space by `to_content` (device pixels). `region` is the painted
    /// region in content space.
    ///
    /// Fails when the content cannot be translated and cached as a whole (PRF-004 R8).
    pub fn from_recording(
        source: &Scene,
        to_content: crate::Point<ScaledPixels>,
        region: Bounds<DevicePixels>,
    ) -> Result<Self, IneligibleContent> {
        let mut baked = Scene::default();
        let mut overlay = Scene::default();
        let mut index = OverlayIndex::default();
        let (mut baked_count, mut overlay_count) = (0, 0);
        for operation in &source.paint_operations {
            match operation {
                PaintOperation::Primitive(primitive) => {
                    if primitive.has_active_scope() {
                        return Err(IneligibleContent::ScopedPrimitive);
                    }
                    if matches!(primitive, Primitive::Surface(_)) {
                        return Err(IneligibleContent::PlatformSurface);
                    }
                    let moved = primitive.translated(to_content, None);
                    let bounds = clipped_bounds(&moved);
                    if bounds.is_empty() {
                        continue;
                    }
                    if matches!(moved, Primitive::Path(_)) || index.overlaps(&bounds) {
                        index.insert(bounds);
                        overlay.insert_primitive(moved);
                        overlay_count += 1;
                    } else {
                        baked.insert_primitive(moved);
                        baked_count += 1;
                    }
                }
                // Paint layers only order content against its surroundings; the order of the
                // operations themselves is kept.
                PaintOperation::StartLayer(_) | PaintOperation::EndLayer => {}
                PaintOperation::BackdropBlur(_) => return Err(IneligibleContent::BackdropBlur),
                PaintOperation::PushCurvedClip(_) | PaintOperation::PopCurvedClip => {
                    return Err(IneligibleContent::CurvedClip);
                }
            }
        }
        baked.finish();
        overlay.finish();
        Ok(Self {
            baked,
            overlay,
            region,
            baked_count,
            overlay_count,
            tile_hashes: Mutex::default(),
        })
    }

    /// The painted region in content space.
    pub fn region(&self) -> Bounds<DevicePixels> {
        self.region
    }

    /// The overlay: paths and every later primitive overlapping an overlay primitive, in
    /// content space, in their original order.
    pub fn overlay(&self) -> &Scene {
        &self.overlay
    }

    /// The baked primitives, in content space, in their original order.
    pub fn baked(&self) -> &Scene {
        &self.baked
    }

    /// The number of baked primitives.
    pub fn baked_len(&self) -> usize {
        self.baked_count
    }

    /// The number of overlay primitives.
    pub fn overlay_len(&self) -> usize {
        self.overlay_count
    }

    /// Whether a baked primitive samples one of the atlas `tiles`.
    pub fn samples_any(&self, tiles: &[AtlasTile]) -> bool {
        let sampled = |tile: &AtlasTile| {
            tiles.iter().any(|updated| {
                updated.texture_id == tile.texture_id && updated.tile_id == tile.tile_id
            })
        };
        let baked = &self.baked;
        baked
            .polychrome_sprites
            .iter()
            .any(|sprite| sampled(&sprite.tile))
            || baked
                .monochrome_sprites
                .iter()
                .any(|sprite| sampled(&sprite.tile))
            || baked
                .subpixel_sprites
                .iter()
                .any(|sprite| sampled(&sprite.tile))
    }

    /// The baked primitives touching `coord`, moved into tile-local coordinates and clipped to
    /// the tile, in order.
    fn tile_primitives(&self, coord: TileCoord) -> impl Iterator<Item = Primitive> + '_ {
        let tile = coord.bounds();
        let tile_content: Bounds<ScaledPixels> = Bounds {
            origin: point(
                ScaledPixels(tile.origin.x.0 as f32),
                ScaledPixels(tile.origin.y.0 as f32),
            ),
            size: size(
                ScaledPixels(TILE_SIZE as f32),
                ScaledPixels(TILE_SIZE as f32),
            ),
        };
        let local = Bounds {
            origin: point(ScaledPixels(0.0), ScaledPixels(0.0)),
            size: tile_content.size,
        };
        let offset = point(
            ScaledPixels(-tile_content.origin.x.0),
            ScaledPixels(-tile_content.origin.y.0),
        );
        self.baked
            .paint_operations
            .iter()
            .filter_map(move |operation| match operation {
                PaintOperation::Primitive(primitive)
                    if clipped_bounds(primitive).intersects(&tile_content) =>
                {
                    Some(primitive.translated(offset, Some(local)))
                }
                _ => None,
            })
    }

    /// The content hash of tile `coord`: a hash over the baked primitives that touch it, in
    /// order, in tile-local coordinates (PRF-004 R4). It covers geometry, clipped content
    /// masks, colors, blur extents, sprite transformations and texture identities, so equal
    /// hashes mean equal tile pixels. Memoized: the content never changes.
    pub fn tile_hash(&self, coord: TileCoord) -> u64 {
        if let Some(hash) = self
            .tile_hashes
            .lock()
            .expect("tile hash table")
            .get(&coord)
        {
            return *hash;
        }
        let mut writer = HashWriter(DefaultHasher::new());
        for mut primitive in self.tile_primitives(coord) {
            strip_identity(&mut primitive);
            let _ = write!(writer, "{primitive:?};");
        }
        let hash = writer.0.finish();
        self.tile_hashes
            .lock()
            .expect("tile hash table")
            .insert(coord, hash);
        hash
    }
}

struct HashWriter(DefaultHasher);

impl std::fmt::Write for HashWriter {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        self.0.write(text.as_bytes());
        Ok(())
    }
}

/// One layer as drawn in one frame (PRF-004).
#[derive(Clone, Debug)]
pub struct LayerFrame {
    id: LayerId,
    instance: u64,
    generation: u64,
    background: Background,
    content: Arc<LayerContent>,
    shown: Vec<TileCoord>,
    dirty: Vec<TileCoord>,
}

impl LayerFrame {
    /// A frame of layer `id` (the `instance` distinguishes it from any earlier layer that used
    /// the same identity) showing the tiles `shown`, of which `dirty` need rasterizing.
    pub fn new(
        id: LayerId,
        instance: u64,
        generation: u64,
        background: Background,
        content: Arc<LayerContent>,
        shown: Vec<TileCoord>,
        dirty: Vec<TileCoord>,
    ) -> Self {
        Self {
            id,
            instance,
            generation,
            background,
            content,
            shown,
            dirty,
        }
    }

    /// The frame as a replay of the preceding frame shows it: the same generation and tiles,
    /// nothing newly dirty.
    pub(crate) fn replayed(&self) -> Self {
        Self {
            dirty: Vec::new(),
            ..self.clone()
        }
    }

    /// The identity of the layer.
    pub fn id(&self) -> LayerId {
        self.id
    }

    /// The instance of the layer: unique per layer lifetime, so a recycled identity never
    /// inherits the textures or generations of an earlier layer.
    pub fn instance(&self) -> u64 {
        self.instance
    }

    /// The content generation: incremented on every repaint of the layer.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The opaque background the tiles are cleared to.
    pub fn background(&self) -> Background {
        self.background
    }

    /// The edge of a tile in device pixels.
    pub fn tile_size(&self) -> i32 {
        TILE_SIZE
    }

    /// The recorded content, shared between frames.
    pub fn content(&self) -> &Arc<LayerContent> {
        &self.content
    }

    /// The tiles composited in this frame.
    pub fn shown(&self) -> &[TileCoord] {
        &self.shown
    }

    /// The shown tiles whose content differs from the preceding generation or that were
    /// never rasterized.
    pub fn dirty(&self) -> &[TileCoord] {
        &self.dirty
    }

    /// The content hash of tile `coord` (see [`LayerContent::tile_hash`]).
    pub fn tile_hash(&self, coord: TileCoord) -> u64 {
        self.content.tile_hash(coord)
    }

    /// The scene a renderer draws to rasterize tile `coord`: the opaque background quad over
    /// the whole tile, then the baked primitives touching it in tile-local coordinates.
    pub fn tile_scene(&self, coord: TileCoord) -> Scene {
        let mut scene = Scene::default();
        let rect = Bounds {
            origin: point(ScaledPixels(0.0), ScaledPixels(0.0)),
            size: size(
                ScaledPixels(TILE_SIZE as f32),
                ScaledPixels(TILE_SIZE as f32),
            ),
        };
        scene.insert_primitive(Quad {
            bounds: rect,
            content_mask: ContentMask { bounds: rect },
            background: self.background,
            ..Quad::default()
        });
        for primitive in self.content.tile_primitives(coord) {
            scene.insert_primitive(primitive);
        }
        scene.finish();
        scene
    }
}

/// The layer frames of one scene.
#[derive(Clone, Debug, Default)]
pub struct SceneLayers {
    frames: Vec<LayerFrame>,
    retired: Vec<(LayerId, u64)>,
}

impl SceneLayers {
    /// The layers composited in the scene.
    pub fn frames(&self) -> &[LayerFrame] {
        &self.frames
    }

    /// Layers that ended since the scene that was presented before this one; their tiles can
    /// be released.
    pub fn retired(&self) -> &[(LayerId, u64)] {
        &self.retired
    }

    /// Adds a layer frame.
    pub fn push(&mut self, frame: LayerFrame) {
        self.frames.push(frame);
    }

    /// Records that the layer `id` of `instance` ended.
    pub fn retire(&mut self, id: LayerId, instance: u64) {
        self.retired.push((id, instance));
    }

    /// The number of frames.
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Whether the scene has no layer frame and no retired layer.
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty() && self.retired.is_empty()
    }

    /// Forgets everything.
    pub fn clear(&mut self) {
        self.frames.clear();
        self.retired.clear();
    }
}

/// Whether two lists of painted primitives (see `Window::painted_content`) draw the same pixels:
/// they hold the same primitives, and every two overlapping primitives keep their relative
/// order. Primitives that do not overlap may be ordered differently (tiles bake them in a
/// different order than direct drawing paints them). Returns the first difference.
#[cfg(any(test, feature = "test-support"))]
pub fn painted_content_agrees(
    expected: &[(String, Bounds<ScaledPixels>)],
    actual: &[(String, Bounds<ScaledPixels>)],
) -> Result<(), String> {
    if expected.len() != actual.len() {
        let mut missing: Vec<&str> = Vec::new();
        let mut unused: Vec<bool> = vec![false; actual.len()];
        for (text, _) in expected {
            match (0..actual.len()).find(|ix| !unused[*ix] && actual[*ix].0 == *text) {
                Some(ix) => unused[ix] = true,
                None => missing.push(text),
            }
        }
        let extra: Vec<&str> = actual
            .iter()
            .zip(&unused)
            .filter(|(_, used)| !**used)
            .map(|((text, _), _)| text.as_str())
            .collect();
        return Err(format!(
            "{} primitives expected, {} found; missing {:?}, extra {:?}",
            expected.len(),
            actual.len(),
            missing.first(),
            extra.first()
        ));
    }
    let mut taken = vec![false; actual.len()];
    let mut position = Vec::with_capacity(expected.len());
    for (text, _) in expected {
        let Some(ix) = (0..actual.len()).find(|ix| !taken[*ix] && actual[*ix].0 == *text) else {
            let stray = actual
                .iter()
                .zip(&taken)
                .find(|((other, _), taken)| !**taken && expected.iter().all(|(e, _)| e != other))
                .map(|((other, _), _)| other.as_str());
            return Err(format!(
                "primitive missing: {text}\nnot expected but painted: {stray:?}"
            ));
        };
        taken[ix] = true;
        position.push(ix);
    }
    for first in 0..expected.len() {
        for second in first + 1..expected.len() {
            if expected[first].1.intersects(&expected[second].1)
                && position[first] > position[second]
            {
                return Err(format!(
                    "overlapping primitives swapped:\n{}\n{}",
                    expected[first].0, expected[second].0
                ));
            }
        }
    }
    Ok(())
}

/// What a tile cache decided about one shown tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileNeed {
    /// The cached texture shows the tile's current content.
    Reuse,
    /// The tile must be rasterized again.
    Rasterize,
}

struct TileEntry<T> {
    texture: T,
    hash: u64,
    generation: u64,
    used: u64,
    /// Pixels the texture sampled changed after it was rasterized.
    stale: bool,
}

/// The tile cache a renderer keeps, generic in its texture type so that the bookkeeping is
/// testable without a GPU (PRF-004 R4, R10, R11).
///
/// Tiles are keyed by the live layer identity plus the signed tile coordinate. A layer whose
/// identity was recycled (a different instance) starts from nothing. Tiles needed by the
/// current frame are never evicted; at most [`TILE_POOL_SIZE`] freed textures are pooled.
pub struct TileCache<T> {
    entries: FxHashMap<(LayerId, TileCoord), TileEntry<T>>,
    instances: FxHashMap<LayerId, u64>,
    pool: Vec<T>,
    frame: u64,
    budget: usize,
}

impl<T> Default for TileCache<T> {
    fn default() -> Self {
        Self::new(TILE_BUDGET_TILES)
    }
}

impl<T> TileCache<T> {
    /// A cache holding at most `budget` tiles besides those the current frame needs.
    pub fn new(budget: usize) -> Self {
        Self {
            entries: FxHashMap::default(),
            instances: FxHashMap::default(),
            pool: Vec::new(),
            frame: 0,
            budget,
        }
    }

    /// Starts a frame.
    pub fn begin_frame(&mut self) {
        self.frame += 1;
    }

    fn release(&mut self, texture: T) {
        if self.pool.len() < TILE_POOL_SIZE {
            self.pool.push(texture);
        }
    }

    /// Drops every tile of the layer `id` of `instance`, if that is the live instance.
    pub fn retire(&mut self, id: LayerId, instance: u64) {
        if self.instances.get(&id) == Some(&instance) {
            self.drop_layer(id);
        }
    }

    fn drop_layer(&mut self, id: LayerId) {
        self.instances.remove(&id);
        let keys: Vec<_> = self
            .entries
            .keys()
            .filter(|(layer, _)| *layer == id)
            .copied()
            .collect();
        for key in keys {
            if let Some(entry) = self.entries.remove(&key) {
                self.release(entry.texture);
            }
        }
    }

    /// Notes that layer `id` of `instance` is drawn: a different instance under the same
    /// identity replaces the old layer's tiles.
    pub fn observe(&mut self, id: LayerId, instance: u64) {
        if self
            .instances
            .get(&id)
            .is_some_and(|live| *live != instance)
        {
            self.drop_layer(id);
        }
        self.instances.insert(id, instance);
    }

    /// Decides whether `coord` of `frame` can use its cached texture, and marks it used. A
    /// tile is reused when its cache entry is of the frame's generation, or when the tile was
    /// not reported dirty and the entry is of the preceding generation with the same content
    /// hash. A cache entry that missed an intervening generation conservatively rasterizes
    /// again.
    pub fn need(&mut self, frame: &LayerFrame, coord: TileCoord) -> TileNeed {
        self.observe(frame.id(), frame.instance());
        let used = self.frame;
        let Some(entry) = self.entries.get_mut(&(frame.id(), coord)) else {
            return TileNeed::Rasterize;
        };
        entry.used = used;
        if entry.stale {
            return TileNeed::Rasterize;
        }
        if entry.generation == frame.generation() {
            return TileNeed::Reuse;
        }
        if frame.dirty().contains(&coord) {
            return TileNeed::Rasterize;
        }
        if entry.generation + 1 == frame.generation() && entry.hash == frame.tile_hash(coord) {
            entry.generation = frame.generation();
            return TileNeed::Reuse;
        }
        TileNeed::Rasterize
    }

    /// Marks every cached tile of the layers `affected` selects as needing rasterization:
    /// pixels its content samples changed outside of the layer, for instance an image updated
    /// in place in the atlas.
    pub fn invalidate_layers(&mut self, affected: impl Fn(LayerId) -> bool) {
        for ((layer, _), entry) in &mut self.entries {
            if affected(*layer) {
                entry.stale = true;
            }
        }
    }

    /// The cached texture of a tile.
    pub fn texture(&self, id: LayerId, coord: TileCoord) -> Option<&T> {
        self.entries.get(&(id, coord)).map(|entry| &entry.texture)
    }

    /// A pooled texture for a tile that is about to be rasterized, if one is free.
    pub fn take_pooled(&mut self) -> Option<T> {
        self.pool.pop()
    }

    /// Stores the freshly rasterized texture of a tile of `frame`.
    pub fn store(&mut self, frame: &LayerFrame, coord: TileCoord, texture: T) {
        self.observe(frame.id(), frame.instance());
        let hash = frame.tile_hash(coord);
        let used = self.frame;
        if let Some(old) = self.entries.insert(
            (frame.id(), coord),
            TileEntry {
                texture,
                hash,
                generation: frame.generation(),
                used,
                stale: false,
            },
        ) {
            self.release(old.texture);
        }
    }

    /// Ends the frame: drops the tiles of retired layers and of layers unused for
    /// [`TILE_IDLE_FRAMES`], then evicts the least recently used tiles that this frame did
    /// not use until at most the budget remains. Tiles used by the frame are never evicted.
    pub fn end_frame(&mut self, retired: &[(LayerId, u64)]) {
        for (id, instance) in retired {
            self.retire(*id, *instance);
        }
        let frame = self.frame;
        let idle: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, entry)| frame.saturating_sub(entry.used) >= TILE_IDLE_FRAMES)
            .map(|(key, _)| *key)
            .collect();
        for key in idle {
            if let Some(entry) = self.entries.remove(&key) {
                self.release(entry.texture);
            }
        }
        if self.entries.len() > self.budget {
            let mut unused: Vec<_> = self
                .entries
                .iter()
                .filter(|(_, entry)| entry.used != frame)
                .map(|(key, entry)| (entry.used, *key))
                .collect();
            unused.sort();
            for (_, key) in unused {
                if self.entries.len() <= self.budget {
                    break;
                }
                if let Some(entry) = self.entries.remove(&key) {
                    self.release(entry.texture);
                }
            }
        }
        let live: std::collections::HashSet<LayerId> =
            self.entries.keys().map(|(layer, _)| *layer).collect();
        self.instances.retain(|id, _| live.contains(id));
    }

    /// The number of cached tiles.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no tile is cached.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The number of freed textures held for reuse.
    pub fn pooled(&self) -> usize {
        self.pool.len()
    }

    /// Bytes of the cached tiles.
    pub fn bytes(&self) -> u64 {
        self.entries.len() as u64 * TILE_BYTES
    }

    /// Drops everything, e.g. after a device loss.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.instances.clear();
        self.pool.clear();
    }

    /// The texture's recorded generation, for tests and diagnostics.
    pub fn generation_of(&self, id: LayerId, coord: TileCoord) -> Option<u64> {
        self.entries.get(&(id, coord)).map(|entry| entry.generation)
    }
}
