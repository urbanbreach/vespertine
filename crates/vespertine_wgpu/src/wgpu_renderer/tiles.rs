//! Scroll-layer tiles (PRF-004): 512 x 512 device-pixel textures of the window's render target
//! format, rasterized from the baked content of a layer and drawn as tile sprites.

use super::{WgpuRendererCore, recording::TextureKey};
use anyhow::Result;
use vespertine::{
    AtlasTile, DevicePixels, Scene, Size, TILE_SIZE, TileCache, TileNeed, tile_identity,
};

/// A tile texture and the identity its bind group is cached under. Reusing a pooled texture for
/// another tile keeps the serial: the bind group refers to the texture, not to the tile.
pub(super) struct TileTexture {
    pub(super) texture: wgpu::Texture,
    pub(super) view: wgpu::TextureView,
    pub(super) serial: u64,
}

/// Counters of the tile work of a renderer (test-support access).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TileStats {
    /// Tiles rasterized.
    pub rasterized: u64,
    /// Shown tiles whose cached texture was current.
    pub reused: u64,
    /// Tile textures created (not taken from the pool).
    pub created: u64,
}

/// The tile cache of a renderer.
pub(super) struct TileState {
    pub(super) cache: TileCache<TileTexture>,
    next_serial: u64,
    pub(super) stats: TileStats,
}

impl Default for TileState {
    fn default() -> Self {
        Self {
            cache: TileCache::default(),
            next_serial: 1,
            stats: TileStats::default(),
        }
    }
}

impl WgpuRendererCore {
    fn create_tile_texture(&mut self) -> TileTexture {
        let serial = self.tiles.next_serial;
        self.tiles.next_serial += 1;
        self.tiles.stats.created += 1;
        let texture = self
            .resources
            .device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("scroll_layer_tile"),
                size: wgpu::Extent3d {
                    width: TILE_SIZE as u32,
                    height: TILE_SIZE as u32,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.target_format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        TileTexture {
            texture,
            view,
            serial,
        }
    }

    /// Rasterizes the shown tiles of the scene's layer frames that are missing or changed
    /// (PRF-004 R4). Each tile is drawn by the ordinary pipelines into its own texture; the
    /// frame's globals are uploaded again by the caller afterwards. Cached tiles that sample an
    /// atlas tile in `updated_in_place` are rasterized again; those of layers this frame does
    /// not show are marked as well, as their content is unknown here.
    pub(super) fn rasterize_tiles(
        &mut self,
        scene: &Scene,
        updated_in_place: &[AtlasTile],
        premultiplied_alpha: bool,
    ) -> Result<()> {
        if scene.layers.is_empty() && self.tiles.cache.is_empty() {
            return Ok(());
        }
        if !updated_in_place.is_empty() {
            let frames = scene.layers.frames();
            self.tiles.cache.invalidate_layers(|layer| {
                frames
                    .iter()
                    .find(|frame| frame.id() == layer)
                    .is_none_or(|frame| frame.content().samples_any(updated_in_place))
            });
        }
        self.tiles.cache.begin_frame();
        let tile_size = Size {
            width: DevicePixels(TILE_SIZE),
            height: DevicePixels(TILE_SIZE),
        };
        for frame in scene.layers.frames() {
            for &coord in frame.shown() {
                if self.tiles.cache.need(frame, coord) == TileNeed::Reuse {
                    self.tiles.stats.reused += 1;
                    continue;
                }
                let tile_scene = frame.tile_scene(coord);
                let texture = match self.tiles.cache.take_pooled() {
                    Some(texture) => texture,
                    None => self.create_tile_texture(),
                };
                self.upload_globals(tile_size, premultiplied_alpha);
                self.record_frame(
                    &tile_scene,
                    &texture.texture,
                    &texture.view,
                    wgpu::Color::TRANSPARENT,
                    None,
                    None,
                    premultiplied_alpha,
                )?;
                self.tiles.stats.rasterized += 1;
                self.tiles.cache.store(frame, coord, texture);
            }
        }
        Ok(())
    }

    /// Ends the tile work of a frame once the frame's sprites were recorded: tiles of layers
    /// that ended in this frame (a layer demoted after it was shown is still drawn by the frame
    /// that retires it), idle layers and tiles over the budget are released.
    pub(super) fn finish_tiles(&mut self, scene: &Scene) {
        if scene.layers.is_empty() && self.tiles.cache.is_empty() {
            return;
        }
        self.tiles.cache.end_frame(scene.layers.retired());
    }

    /// The binding key of the tile texture a tile sprite of the frame samples.
    pub(super) fn tile_texture_key(
        &self,
        scene: &Scene,
        first_sprite: usize,
    ) -> Option<TextureKey> {
        let sprite = scene.polychrome_sprites.get(first_sprite)?;
        let (layer, coord) = tile_identity(&sprite.tile)?;
        let tile = self.tiles.cache.texture(layer, coord)?;
        Some(TextureKey::Tile(tile.serial))
    }

    /// The view of the tile texture a tile sprite of the frame samples.
    pub(super) fn tile_texture_view(
        &self,
        scene: &Scene,
        first_sprite: usize,
    ) -> Option<(u64, &wgpu::TextureView)> {
        let sprite = scene.polychrome_sprites.get(first_sprite)?;
        let (layer, coord) = tile_identity(&sprite.tile)?;
        let tile = self.tiles.cache.texture(layer, coord)?;
        Some((tile.serial, &tile.view))
    }
}
