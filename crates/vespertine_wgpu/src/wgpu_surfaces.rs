//! Shared-device GPU texture surfaces.
//!
//! A surface primitive carries a shared, type-erased texture that the application created on
//! this renderer's own device. The renderer composites it as one RGBA straight-alpha 2D
//! texture (single sample, mip level zero) sampled with linear filtering and clamp-to-edge
//! addressing.
//!
//! Every surface in a frame owns one parameter block (destination bounds, content-mask bounds
//! and opacity) in a single uniform buffer. Blocks are `min_uniform_buffer_offset_alignment`
//! apart and addressed per draw with a dynamic offset. Queue writes take effect at submission,
//! before any draw of the frame runs, so a single shared slot would leave every surface with
//! the last surface's parameters; the block of a surface is therefore indexed by the surface's
//! position in the scene and never overwritten within a frame.
//!
//! Corner radii are not applied to surfaces: the visible area is the destination rectangle
//! intersected with the content mask.

use bytemuck::{Pod, Zeroable};
use std::num::NonZeroU64;
use std::ops::Range;
use vespertine::{Bounds, PaintSurface, ScaledPixels};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PodBounds {
    origin: [f32; 2],
    size: [f32; 2],
}

impl From<Bounds<ScaledPixels>> for PodBounds {
    fn from(bounds: Bounds<ScaledPixels>) -> Self {
        Self {
            origin: [bounds.origin.x.0, bounds.origin.y.0],
            size: [bounds.size.width.0, bounds.size.height.0],
        }
    }
}

/// One surface's parameter block. Mirrors `SurfaceParams` in `shaders_surface.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SurfaceParams {
    bounds: PodBounds,
    content_mask: PodBounds,
    opacity: f32,
    _pad: [f32; 3],
}

const PARAMS_SIZE: usize = std::mem::size_of::<SurfaceParams>();

/// The bind group layout of a surface draw: the dynamically offset parameter block, the
/// filterable 2D texture and the sampler.
pub(crate) fn create_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("surfaces_layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: NonZeroU64::new(PARAMS_SIZE as u64),
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    })
}

/// The surface pipeline. `color_target` carries the same blend state as polychrome sprites.
pub(crate) fn create_pipeline(
    device: &wgpu::Device,
    globals_layout: &wgpu::BindGroupLayout,
    surfaces_layout: &wgpu::BindGroupLayout,
    color_target: wgpu::ColorTargetState,
    module: &wgpu::ShaderModule,
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("surfaces_layout_pipeline"),
        bind_group_layouts: &[Some(globals_layout), Some(surfaces_layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("surfaces"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs_surface"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some("fs_surface"),
            targets: &[Some(color_target)],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

/// The parameter-block spacing for a device's uniform offset alignment.
fn params_stride(min_uniform_buffer_offset_alignment: u32) -> usize {
    let alignment = (min_uniform_buffer_offset_alignment as usize).max(1);
    PARAMS_SIZE.next_multiple_of(alignment)
}

struct ParamsBuffer {
    buffer: wgpu::Buffer,
    slots: usize,
}

/// Device-lifetime state of the surface path: the sampler and the growable parameter buffer.
pub(crate) struct SurfaceBatcher {
    sampler: wgpu::Sampler,
    stride: usize,
    params: Option<ParamsBuffer>,
    staging: Vec<u8>,
    warned_invalid_texture: bool,
}

impl SurfaceBatcher {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("surface_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            lod_min_clamp: 0.0,
            lod_max_clamp: 0.0,
            ..Default::default()
        });
        Self {
            sampler,
            stride: params_stride(device.limits().min_uniform_buffer_offset_alignment),
            params: None,
            staging: Vec::new(),
            warned_invalid_texture: false,
        }
    }

    /// Makes the parameter buffer hold at least `slots` blocks. A replaced buffer stays alive
    /// through the bind groups of draws already recorded this frame, which only address
    /// blocks written to it.
    fn ensure_capacity(&mut self, device: &wgpu::Device, slots: usize) {
        if self
            .params
            .as_ref()
            .is_some_and(|params| params.slots >= slots)
        {
            return;
        }
        let slots = slots.next_power_of_two().max(4);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("surface_params"),
            size: (slots * self.stride) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.params = Some(ParamsBuffer { buffer, slots });
    }

    /// Draws `scene_surfaces[range]` into `pass`. `scene_surfaces` is the scene's whole,
    /// ordered surface list: a surface's parameter block is its index in that list, so
    /// batches of one frame never share or overwrite blocks.
    ///
    /// Surfaces whose erased texture is not a `wgpu::Texture`, that the device rejects as
    /// a sampled filterable 2D texture (a texture of another or a lost device, a missing
    /// texture-binding usage, a multisampled or non-filterable format), or that have an
    /// empty destination or clip are skipped.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pipeline: &wgpu::RenderPipeline,
        layout: &wgpu::BindGroupLayout,
        globals: &wgpu::BindGroup,
        scene_surfaces: &[PaintSurface],
        range: Range<usize>,
        pass: &mut wgpu::RenderPass<'_>,
    ) {
        let surfaces = &scene_surfaces[range.clone()];
        if surfaces.is_empty() {
            return;
        }
        self.ensure_capacity(device, scene_surfaces.len());
        let stride = self.stride;

        self.staging.clear();
        self.staging.resize(surfaces.len() * stride, 0);
        for (index, surface) in surfaces.iter().enumerate() {
            let params = SurfaceParams {
                bounds: surface.bounds.into(),
                content_mask: surface.content_mask.bounds.into(),
                opacity: surface.opacity,
                _pad: [0.0; 3],
            };
            let offset = index * stride;
            self.staging[offset..offset + PARAMS_SIZE].copy_from_slice(bytemuck::bytes_of(&params));
        }
        let Some(params_buffer) = &self.params else {
            return;
        };
        queue.write_buffer(
            &params_buffer.buffer,
            (range.start * stride) as u64,
            &self.staging,
        );

        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, globals, &[]);
        for (index, surface) in surfaces.iter().enumerate() {
            if surface.bounds.size.width.0 <= 0.0
                || surface.bounds.size.height.0 <= 0.0
                || surface.content_mask.bounds.size.width.0 <= 0.0
                || surface.content_mask.bounds.size.height.0 <= 0.0
            {
                continue;
            }
            let Some(texture) = surface.texture.downcast_ref::<wgpu::Texture>() else {
                continue;
            };
            let Some(bind_group) = create_bind_group(
                device,
                layout,
                &params_buffer.buffer,
                &self.sampler,
                texture,
                &mut self.warned_invalid_texture,
            ) else {
                continue;
            };
            pass.set_bind_group(1, &bind_group, &[((range.start + index) * stride) as u32]);
            pass.draw(0..4, 0..1);
        }
    }
}

/// Binds one surface's texture. Returns `None` (the surface is skipped) when the device
/// rejects the texture; the rejection is contained in an error scope so it never reaches
/// the device's uncaptured-error handling.
fn create_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    params: &wgpu::Buffer,
    sampler: &wgpu::Sampler,
    texture: &wgpu::Texture,
    warned: &mut bool,
) -> Option<wgpu::BindGroup> {
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let view = texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("surface_view"),
        dimension: Some(wgpu::TextureViewDimension::D2),
        base_mip_level: 0,
        mip_level_count: Some(1),
        base_array_layer: 0,
        array_layer_count: Some(1),
        ..Default::default()
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("surface_bind_group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: params,
                    offset: 0,
                    size: NonZeroU64::new(PARAMS_SIZE as u64),
                }),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    });
    match vespertine::block_on(scope.pop()) {
        None => Some(bind_group),
        Some(error) => {
            if !*warned {
                *warned = true;
                log::warn!("skipping a surface whose texture cannot be sampled: {error}");
            }
            None
        }
    }
}

#[cfg(test)]
mod tests;
