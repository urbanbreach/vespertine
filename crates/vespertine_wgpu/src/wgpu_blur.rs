//! GPU side of the ordered backdrop blur (RND-003) with the reusable storage and region-bounded
//! filtering of PRF-009 and the idle reclamation rules of PRF-010 that concern blur resources.

use crate::blur::{
    EFFECT_FREE_FRAME_LIMIT, FilterParams, FloatRect, Kernel, MAX_EFFECTS_PER_FRAME, PixelRect,
    SetShape, TABLE_SLOTS, new_set_shape, pass_scissors, set_is_reusable, snapshot_envelope,
    snapshot_origin,
};
use bytemuck::{Pod, Zeroable};
use std::num::NonZeroU64;
use std::sync::Arc;
use vespertine::BackdropBlur;
use vespertine_collections::FxHashMap;

const BLUR_SHADER: &str = include_str!("blur.wgsl");

/// Bound on cached kernel tables; the cache is cleared when exceeded.
const MAX_CACHED_KERNELS: usize = 32;

const BLIT_SHADER: &str = r#"
@group(0) @binding(0) var source: texture_2d<f32>;

struct BlitVarying {
    @builtin(position) position: vec4<f32>,
}

@vertex
fn vs_blit(@builtin(vertex_index) vertex_id: u32) -> BlitVarying {
    let x = select(-1.0, 3.0, vertex_id == 1u);
    let y = select(-1.0, 3.0, vertex_id == 2u);
    var out = BlitVarying();
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    return out;
}

@fragment
fn fs_blit(input: BlitVarying) -> @location(0) vec4<f32> {
    return textureLoad(source, vec2<i32>(input.position.xy), 0);
}
"#;

/// Uniform block shared by the filter and composite passes (see `blur.wgsl`).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BlurParams {
    step: [f32; 2],
    out_size: [f32; 2],
    radius: u32,
    paired: u32,
    effective_sigma: f32,
    inverse_norm: f32,
    bounds: [f32; 4],
    content_mask: [f32; 4],
    radii: [f32; 4],
    snapshot_origin: [f32; 2],
    snapshot_size: [f32; 2],
    viewport: [f32; 2],
    pad: [f32; 2],
    draw_rect: [f32; 4],
    table: [[f32; 4]; TABLE_SLOTS / 4],
}

const PARAM_BLOCK_SIZE: u64 = std::mem::size_of::<BlurParams>() as u64;

/// Counters exposed for tests and the performance report.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BlurStats {
    /// Blur sets (snapshot plus two filter targets) allocated so far.
    pub set_allocations: u64,
    /// Presentation intermediates allocated so far.
    pub intermediate_allocations: u64,
    /// Effects rendered (snapshot, both filter passes, composite).
    pub effects_rendered: u64,
    /// Effects skipped: over the per-frame budget, empty envelope or empty scissor.
    pub effects_skipped: u64,
    /// Kernel tables computed.
    pub kernel_computations: u64,
    /// Filter-target texels covered by the scissors of all drawn filter passes.
    pub shaded_texels: u64,
    /// Texels of the filter targets of all drawn effects (cost of unrestricted filtering).
    pub full_target_texels: u64,
}

/// Observable state of the retained blur resources.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BlurResourceState {
    /// Shape of the retained set, if any.
    pub set: Option<SetShape>,
    /// Whether a presentation intermediate is retained.
    pub intermediate: bool,
    /// Number of cached kernel tables.
    pub kernels: usize,
    /// Consecutive effect-free rendered frames (saturates at the release limit).
    pub effect_free_frames: u32,
    /// Bytes retained by snapshot, filter targets and presentation intermediate.
    pub retained_bytes: u64,
}

struct BlurSet {
    shape: SetShape,
    snapshot: wgpu::Texture,
    target_a: wgpu::Texture,
    target_b: wgpu::Texture,
    target_a_view: wgpu::TextureView,
    target_b_view: wgpu::TextureView,
    snapshot_group: wgpu::BindGroup,
    a_group: wgpu::BindGroup,
    b_group: wgpu::BindGroup,
}

struct PresentationIntermediate {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    group: wgpu::BindGroup,
}

/// What the main pass must draw after an effect's filters have run.
pub(crate) struct CompositePlan {
    offset: u32,
}

pub(crate) struct BlurRenderer {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    format: wgpu::TextureFormat,
    bytes_per_pixel: u64,
    layout: wgpu::BindGroupLayout,
    blit_layout: wgpu::BindGroupLayout,
    filter_pipeline: wgpu::RenderPipeline,
    composite_pipeline: wgpu::RenderPipeline,
    blit_pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    params_buffer: wgpu::Buffer,
    block_stride: u64,
    set: Option<BlurSet>,
    intermediate: Option<PresentationIntermediate>,
    kernels: FxHashMap<(u32, u32), Arc<Kernel>>,
    /// Parameter blocks of the frame being recorded, `block_stride` bytes each.
    frame_blocks: Vec<u8>,
    frame_effects: usize,
    effect_free_frames: u32,
    stats: BlurStats,
}

impl BlurRenderer {
    pub(crate) fn new(
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        format: wgpu::TextureFormat,
    ) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blur_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: NonZeroU64::new(PARAM_BLOCK_SIZE),
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
        });
        let blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blur_blit_layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blur_shaders"),
            source: wgpu::ShaderSource::Wgsl(BLUR_SHADER.into()),
        });
        let blit_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blur_blit_shader"),
            source: wgpu::ShaderSource::Wgsl(BLIT_SHADER.into()),
        });
        let create = |name: &str,
                      layout: &wgpu::BindGroupLayout,
                      module: &wgpu::ShaderModule,
                      vs: &str,
                      fs: &str,
                      topology: wgpu::PrimitiveTopology| {
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(&format!("{name}_layout")),
                bind_group_layouts: &[Some(layout)],
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(name),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some(vs),
                    buffers: &[],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some(fs),
                    // Replacement: no blending, all four components written.
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let filter_pipeline = create(
            "blur_filter",
            &layout,
            &module,
            "vs_filter",
            "fs_filter",
            wgpu::PrimitiveTopology::TriangleList,
        );
        let composite_pipeline = create(
            "blur_composite",
            &layout,
            &module,
            "vs_composite",
            "fs_composite",
            wgpu::PrimitiveTopology::TriangleStrip,
        );
        let blit_pipeline = create(
            "blur_blit",
            &blit_layout,
            &blit_module,
            "vs_blit",
            "fs_blit",
            wgpu::PrimitiveTopology::TriangleList,
        );

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("blur_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let alignment = u64::from(device.limits().min_uniform_buffer_offset_alignment).max(1);
        let block_stride = PARAM_BLOCK_SIZE.next_multiple_of(alignment);
        // Three blocks per supported effect: horizontal filter, vertical filter, composite.
        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("blur_params"),
            size: block_stride * 3 * MAX_EFFECTS_PER_FRAME as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            device,
            queue,
            format,
            bytes_per_pixel: format.block_copy_size(None).map_or(4, u64::from),
            layout,
            blit_layout,
            filter_pipeline,
            composite_pipeline,
            blit_pipeline,
            sampler,
            params_buffer,
            block_stride,
            set: None,
            intermediate: None,
            kernels: FxHashMap::default(),
            frame_blocks: Vec::new(),
            frame_effects: 0,
            effect_free_frames: 0,
            stats: BlurStats::default(),
        }
    }

    #[cfg(any(test, feature = "bench-support", feature = "test-support"))]
    pub(crate) fn stats(&self) -> BlurStats {
        self.stats
    }

    #[cfg(any(test, feature = "bench-support", feature = "test-support"))]
    pub(crate) fn resource_state(&self) -> BlurResourceState {
        BlurResourceState {
            set: self.set.as_ref().map(|set| set.shape),
            intermediate: self.intermediate.is_some(),
            kernels: self.kernels.len(),
            effect_free_frames: self.effect_free_frames,
            retained_bytes: self.retained_bytes(),
        }
    }

    /// Bytes of all retained scratch textures: snapshot, both filter targets and the
    /// presentation intermediate (DEV-002 R5).
    pub(crate) fn retained_bytes(&self) -> u64 {
        let texture_bytes = |texture: &wgpu::Texture| {
            u64::from(texture.width()) * u64::from(texture.height()) * self.bytes_per_pixel
        };
        let set = self.set.as_ref().map_or(0, |set| {
            texture_bytes(&set.snapshot)
                + texture_bytes(&set.target_a)
                + texture_bytes(&set.target_b)
        });
        let intermediate = self
            .intermediate
            .as_ref()
            .map_or(0, |intermediate| texture_bytes(&intermediate.texture));
        set + intermediate
    }

    /// Releases the blur set, the presentation intermediate and the kernel cache. Work already
    /// submitted keeps them alive through the graphics API.
    pub(crate) fn release(&mut self) {
        self.set = None;
        self.intermediate = None;
        self.kernels.clear();
    }

    pub(crate) fn begin_frame(&mut self) {
        self.frame_blocks.clear();
        self.frame_effects = 0;
    }

    /// Counts an encoded frame for the effect-free retention rule (PRF-009 R3). A scene that
    /// lists any effect, even one hidden or over budget, resets the counter.
    pub(crate) fn end_frame(&mut self, scene_has_effects: bool) {
        if scene_has_effects {
            self.effect_free_frames = 0;
            return;
        }
        self.effect_free_frames = (self.effect_free_frames + 1).min(EFFECT_FREE_FRAME_LIMIT);
        if self.effect_free_frames >= EFFECT_FREE_FRAME_LIMIT {
            self.release();
        }
    }

    /// Explicit idle trim (PRF-010 R5): frees everything unless the last frame had an effect.
    pub(crate) fn trim(&mut self) {
        if self.effect_free_frames > 0 {
            self.release();
        }
    }

    /// Uploads the parameter blocks written by this frame's effects. Must run before the
    /// frame's command buffer is submitted.
    pub(crate) fn flush_frame(&mut self) {
        if !self.frame_blocks.is_empty() {
            self.queue
                .write_buffer(&self.params_buffer, 0, &self.frame_blocks);
        }
    }

    /// Release when the drawable shrank below the retained snapshot, or the format changed.
    pub(crate) fn drawable_changed(&mut self, drawable: (u32, u32)) {
        if self
            .set
            .as_ref()
            .is_some_and(|set| set.shape.width > drawable.0 || set.shape.height > drawable.1)
        {
            self.set = None;
        }
        if self.intermediate.as_ref().is_some_and(|intermediate| {
            (intermediate.texture.width(), intermediate.texture.height()) != drawable
        }) {
            self.intermediate = None;
        }
    }

    fn kernel_for(&mut self, filter: &FilterParams) -> Arc<Kernel> {
        let key = (filter.effective_sigma.to_bits(), filter.radius);
        if let Some(kernel) = self.kernels.get(&key) {
            return kernel.clone();
        }
        if self.kernels.len() >= MAX_CACHED_KERNELS {
            self.kernels.clear();
        }
        let kernel = Arc::new(Kernel::for_filter(filter));
        self.stats.kernel_computations += 1;
        self.kernels.insert(key, kernel.clone());
        kernel
    }

    fn param_group(&self, view: &wgpu::TextureView) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("blur_group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.params_buffer,
                        offset: 0,
                        size: NonZeroU64::new(PARAM_BLOCK_SIZE),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    fn create_texture(
        &self,
        label: &str,
        width: u32,
        height: u32,
        usage: wgpu::TextureUsages,
    ) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage,
            view_formats: &[],
        })
    }

    /// Ensures a set serving `required` at `downsample`, replacing a set that cannot.
    fn ensure_set(&mut self, downsample: u32, required: (u32, u32), drawable: (u32, u32)) {
        if self
            .set
            .as_ref()
            .is_some_and(|set| set_is_reusable(&set.shape, downsample, required, drawable))
        {
            return;
        }
        // The obsolete set is discarded here; alternatives are never accumulated.
        self.set = None;
        let shape = new_set_shape(downsample, required, drawable);
        let (filter_width, filter_height) = shape.filter_size();
        let snapshot = self.create_texture(
            "blur_snapshot",
            shape.width,
            shape.height,
            wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let filter_usage =
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let target_a =
            self.create_texture("blur_target_a", filter_width, filter_height, filter_usage);
        let target_b =
            self.create_texture("blur_target_b", filter_width, filter_height, filter_usage);
        let snapshot_view = snapshot.create_view(&wgpu::TextureViewDescriptor::default());
        let target_a_view = target_a.create_view(&wgpu::TextureViewDescriptor::default());
        let target_b_view = target_b.create_view(&wgpu::TextureViewDescriptor::default());
        let snapshot_group = self.param_group(&snapshot_view);
        let a_group = self.param_group(&target_a_view);
        let b_group = self.param_group(&target_b_view);
        self.stats.set_allocations += 1;
        self.set = Some(BlurSet {
            shape,
            snapshot,
            target_a,
            target_b,
            target_a_view,
            target_b_view,
            snapshot_group,
            a_group,
            b_group,
        });
    }

    fn push_block(&mut self, params: &BlurParams) -> u32 {
        let offset = self.frame_blocks.len();
        let stride = self.block_stride as usize;
        self.frame_blocks.resize(offset + stride, 0);
        self.frame_blocks[offset..offset + PARAM_BLOCK_SIZE as usize]
            .copy_from_slice(bytemuck::bytes_of(params));
        offset as u32
    }

    /// Snapshots the framebuffer and runs both filter passes for one effect. The caller has
    /// already ended the main render pass. Returns the composite to draw in the restarted main
    /// pass, or `None` when the effect is skipped (over budget, empty envelope or region).
    pub(crate) fn render_effect(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::Texture,
        effect: &BackdropBlur,
    ) -> Option<CompositePlan> {
        let index = self.frame_effects;
        self.frame_effects += 1;
        if index >= MAX_EFFECTS_PER_FRAME {
            self.stats.effects_skipped += 1;
            return None;
        }
        let drawable = (source.width(), source.height());
        let filter = FilterParams::new(effect.blur_radius.0);
        let bounds = FloatRect::new(
            [effect.bounds.origin.x.0, effect.bounds.origin.y.0],
            [effect.bounds.size.width.0, effect.bounds.size.height.0],
        );
        let mask = FloatRect::new(
            [
                effect.content_mask.bounds.origin.x.0,
                effect.content_mask.bounds.origin.y.0,
            ],
            [
                effect.content_mask.bounds.size.width.0,
                effect.content_mask.bounds.size.height.0,
            ],
        );
        let visible = bounds.intersect(&mask);
        let Some(envelope) = snapshot_envelope(&visible, filter.envelope_padding(), drawable)
        else {
            self.stats.effects_skipped += 1;
            return None;
        };
        self.ensure_set(
            filter.downsample,
            (envelope.width(), envelope.height()),
            drawable,
        );
        let kernel = self.kernel_for(&filter);
        let (shape, snapshot_size, target_size) = {
            let set = self.set.as_ref()?;
            (
                set.shape,
                (set.shape.width, set.shape.height),
                set.shape.filter_size(),
            )
        };
        let origin = snapshot_origin(&envelope, snapshot_size, drawable);
        let scissors = pass_scissors(
            &visible,
            origin,
            snapshot_size,
            target_size,
            filter.effective_sigma,
        );
        if scissors.vertical.is_empty() {
            self.stats.effects_skipped += 1;
            return None;
        }

        // Pixels the composite covers: the visible rectangle snapped outward to whole pixels.
        let draw_rect = PixelRect {
            x0: visible.x0.floor().max(0.0) as u32,
            y0: visible.y0.floor().max(0.0) as u32,
            x1: (visible.x1.ceil().max(0.0) as u32).min(drawable.0),
            y1: (visible.y1.ceil().max(0.0) as u32).min(drawable.1),
        };
        if draw_rect.is_empty() {
            self.stats.effects_skipped += 1;
            return None;
        }

        let mut table = [[0.0_f32; 4]; TABLE_SLOTS / 4];
        for (slot, value) in kernel.table.iter().enumerate() {
            table[slot / 4][slot % 4] = *value;
        }
        let base = BlurParams {
            step: [0.0; 2],
            out_size: [target_size.0 as f32, target_size.1 as f32],
            radius: kernel.radius,
            paired: 0,
            effective_sigma: kernel.effective_sigma,
            inverse_norm: kernel.inverse_norm,
            bounds: [
                effect.bounds.origin.x.0,
                effect.bounds.origin.y.0,
                effect.bounds.size.width.0,
                effect.bounds.size.height.0,
            ],
            content_mask: [
                effect.content_mask.bounds.origin.x.0,
                effect.content_mask.bounds.origin.y.0,
                effect.content_mask.bounds.size.width.0,
                effect.content_mask.bounds.size.height.0,
            ],
            radii: [
                effect.corner_radii.top_left.0,
                effect.corner_radii.top_right.0,
                effect.corner_radii.bottom_right.0,
                effect.corner_radii.bottom_left.0,
            ],
            snapshot_origin: [origin.0 as f32, origin.1 as f32],
            snapshot_size: [snapshot_size.0 as f32, snapshot_size.1 as f32],
            viewport: [drawable.0 as f32, drawable.1 as f32],
            pad: [0.0; 2],
            draw_rect: [
                draw_rect.x0 as f32,
                draw_rect.y0 as f32,
                draw_rect.x1 as f32,
                draw_rect.y1 as f32,
            ],
            table,
        };
        let uses_table = kernel.uses_table();
        let horizontal = self.push_block(&BlurParams {
            // Taps are `k` source texels times the downsampling factor apart; adjacent taps may
            // only be paired when that stride is a single texel.
            step: [shape.downsample as f32 / snapshot_size.0 as f32, 0.0],
            paired: u32::from(uses_table && shape.downsample == 1),
            ..base
        });
        let vertical = self.push_block(&BlurParams {
            step: [0.0, 1.0 / target_size.1 as f32],
            paired: u32::from(uses_table),
            ..base
        });
        let composite = self.push_block(&base);

        let set = self.set.as_ref()?;
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: source,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: origin.0,
                    y: origin.1,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &set.snapshot,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: snapshot_size.0,
                height: snapshot_size.1,
                depth_or_array_layers: 1,
            },
        );

        for (label, target, group, offset, scissor) in [
            (
                "blur_horizontal",
                &set.target_a_view,
                &set.snapshot_group,
                horizontal,
                scissors.horizontal,
            ),
            (
                "blur_vertical",
                &set.target_b_view,
                &set.a_group,
                vertical,
                scissors.vertical,
            ),
        ] {
            // Both targets are cleared to transparent so skipped areas never retain data from
            // an earlier effect.
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            });
            if !scissor.is_empty() {
                pass.set_pipeline(&self.filter_pipeline);
                pass.set_bind_group(0, group, &[offset]);
                pass.set_scissor_rect(scissor.x0, scissor.y0, scissor.width(), scissor.height());
                pass.draw(0..3, 0..1);
                self.stats.shaded_texels += scissor.area();
            }
        }
        self.stats.full_target_texels += 2 * u64::from(target_size.0) * u64::from(target_size.1);
        self.stats.effects_rendered += 1;
        Some(CompositePlan { offset: composite })
    }

    /// Draws the replacement composite of an effect into the main pass.
    pub(crate) fn composite(&self, pass: &mut wgpu::RenderPass<'_>, plan: &CompositePlan) {
        let Some(set) = self.set.as_ref() else {
            return;
        };
        pass.set_pipeline(&self.composite_pipeline);
        pass.set_bind_group(0, &set.b_group, &[plan.offset]);
        pass.draw(0..4, 0..1);
    }

    /// A copy-capable full-frame target, replaced when the size changes (PRF-009 R8).
    pub(crate) fn presentation_intermediate(
        &mut self,
        size: (u32, u32),
    ) -> (wgpu::Texture, wgpu::TextureView) {
        let current = self.intermediate.as_ref().is_some_and(|intermediate| {
            (intermediate.texture.width(), intermediate.texture.height()) == size
        });
        if !current {
            let texture = self.create_texture(
                "blur_presentation_intermediate",
                size.0,
                size.1,
                wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::TEXTURE_BINDING,
            );
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("blur_blit_group"),
                layout: &self.blit_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                }],
            });
            self.stats.intermediate_allocations += 1;
            self.intermediate = Some(PresentationIntermediate {
                texture,
                view,
                group,
            });
        }
        let intermediate = self
            .intermediate
            .as_ref()
            .expect("presentation intermediate was just created");
        (intermediate.texture.clone(), intermediate.view.clone())
    }

    /// Copies the finished frame from the presentation intermediate to the presentation target
    /// without blending, preserving every channel (including transparent alpha).
    pub(crate) fn present_intermediate(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
    ) {
        let Some(intermediate) = self.intermediate.as_ref() else {
            return;
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("blur_present"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            ..Default::default()
        });
        pass.set_pipeline(&self.blit_pipeline);
        pass.set_bind_group(0, &intermediate.group, &[]);
        pass.draw(0..3, 0..1);
    }
}
