//! The GPU renderer of shader surfaces (CMP-026 R1, R6, R8, R9, R11, R13).
//!
//! Frames run on their own wgpu device (the platform's Vulkan-capable adapter, low-power
//! preference, limits taken from the adapter). Every program of a frame is compiled before any
//! state is touched, so a compile error leaves the feedback state unchanged. The passes render in
//! 16-bit float; the final composite pass applies the blend mode, bloom glow and edge fade and
//! writes 8-bit straight-alpha pixels that are read back, flipped upright and swizzled to BGRA.
//! Rows are rendered bottom-up (the fragment coordinate origin is the bottom-left, so a buffer
//! reads back exactly what it wrote); the composite pass flips the final image.

use super::blend::{BLOOM_ITERATIONS, BlendMode, clamp_bloom};
use super::inputs::{TimeState, UniformBlock, UniformInputs};
use super::program::{self, MAX_BUFFERS, MAX_TEXTURES, ProgramError};
use std::borrow::Cow;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};

/// The pass format.
pub const PASS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Side of the noise texture.
pub const NOISE_SIZE: u32 = 256;

/// An RGBA8 image of a host asset table.
#[derive(Clone, Debug)]
pub struct TextureImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Straight-alpha RGBA8 pixels, row-major from the top.
    pub rgba: Vec<u8>,
}

impl TextureImage {
    /// An image from RGBA8 bytes; `None` when the length is not `width x height x 4`.
    pub fn from_rgba(width: u32, height: u32, rgba: Vec<u8>) -> Option<Self> {
        (width > 0 && height > 0 && rgba.len() == width as usize * height as usize * 4).then_some(
            Self {
                width,
                height,
                rgba,
            },
        )
    }
}

/// The host's asset table: resolves texture names to images.
pub trait TextureSource: Send + Sync + 'static {
    /// The image named `name`, if the table has one. A missing name keeps its slot and samples
    /// as transparent black.
    fn texture(&self, name: &str) -> Option<Arc<TextureImage>>;
}

/// A table of named textures.
#[derive(Default)]
pub struct TextureTable(Mutex<HashMap<String, Arc<TextureImage>>>);

impl TextureTable {
    /// An empty table.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Adds or replaces a texture.
    pub fn insert(&self, name: impl Into<String>, image: TextureImage) {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(name.into(), Arc::new(image));
    }
}

impl TextureSource for TextureTable {
    fn texture(&self, name: &str) -> Option<Arc<TextureImage>> {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(name)
            .cloned()
    }
}

/// Everything a frame renders from.
#[derive(Clone)]
pub struct ShaderFrame {
    /// The image pass program.
    pub main: Arc<str>,
    /// The buffer pass programs, in order (at most 4).
    pub buffers: Vec<Arc<str>>,
    /// Names of the named float inputs.
    pub input_names: Vec<String>,
    /// Values of the named float inputs.
    pub input_values: Vec<f32>,
    /// Names of the image textures (at most 4).
    pub texture_names: Vec<String>,
    /// The asset table the names resolve against.
    pub textures: Arc<dyn TextureSource>,
    /// Blend mode.
    pub blend: BlendMode,
    /// Bloom strength.
    pub bloom: f32,
    /// Whether time advances (otherwise the first frame's time is kept).
    pub animated: bool,
    /// Pixel size.
    pub size: (u32, u32),
    /// Pixel ratio.
    pub ratio: f32,
    /// The mouse value.
    pub mouse: [f32; 4],
    /// Yaw, pitch, zoom.
    pub camera: [f32; 3],
    /// Seconds since the host view was created.
    pub time: f32,
}

/// The shared device and the pipelines that do not depend on a program.
pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter: String,
    vertex: wgpu::ShaderModule,
    pass_layout: wgpu::BindGroupLayout,
    pass_pipeline_layout: wgpu::PipelineLayout,
    clamp: wgpu::Sampler,
    repeat: wgpu::Sampler,
    bright: wgpu::RenderPipeline,
    blur: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    post_layouts: PostLayouts,
}

struct PostLayouts {
    single: wgpu::BindGroupLayout,
    composite: wgpu::BindGroupLayout,
}

static GPU: OnceLock<Result<Arc<Gpu>, String>> = OnceLock::new();

/// The shared GPU, created on first use. The error names the adapter reason when no GPU is
/// available.
pub fn gpu() -> Result<Arc<Gpu>, String> {
    GPU.get_or_init(|| Gpu::new().map(Arc::new)).clone()
}

/// Whether a GPU adapter is available.
pub fn gpu_available() -> bool {
    gpu().is_ok()
}

const POST_WGSL: &str = r#"
@vertex fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    return vec4<f32>(x * 2.0 - 1.0, y * 2.0 - 1.0, 0.0, 1.0);
}

struct Blur { texel: vec2<f32>, dir: vec2<f32> };
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var<uniform> blur: Blur;

fn luma(c: vec3<f32>) -> f32 { return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722)); }

@fragment fn fs_bright(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = pos.xy * blur.texel;
    let c = textureSampleLevel(src, samp, uv, 0.0).rgb;
    let over = smoothstep(0.6, 1.2, luma(c));
    return vec4<f32>(c * over, 1.0);
}

@fragment fn fs_blur(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = pos.xy * blur.texel;
    let step = blur.dir * blur.texel * 1.6;
    var sum = textureSampleLevel(src, samp, uv, 0.0).rgb * 0.227;
    sum += (textureSampleLevel(src, samp, uv + step, 0.0).rgb + textureSampleLevel(src, samp, uv - step, 0.0).rgb) * 0.194;
    sum += (textureSampleLevel(src, samp, uv + step * 2.0, 0.0).rgb + textureSampleLevel(src, samp, uv - step * 2.0, 0.0).rgb) * 0.121;
    sum += (textureSampleLevel(src, samp, uv + step * 3.0, 0.0).rgb + textureSampleLevel(src, samp, uv - step * 3.0, 0.0).rgb) * 0.054;
    sum += (textureSampleLevel(src, samp, uv + step * 4.0, 0.0).rgb + textureSampleLevel(src, samp, uv - step * 4.0, 0.0).rgb) * 0.016;
    return vec4<f32>(sum, 1.0);
}

struct Composite { size: vec2<f32>, bloom: f32, mode: u32 };
@group(0) @binding(0) var hdr: texture_2d<f32>;
@group(0) @binding(1) var glow_tex: texture_2d<f32>;
@group(0) @binding(2) var csamp: sampler;
@group(0) @binding(3) var<uniform> comp: Composite;

fn ease(d: f32, w: f32) -> f32 {
    let s = clamp(d / w, 0.0, 1.0);
    return clamp(s * s * s * (s * (6.0 * s - 15.0) + 10.0), 0.0, 1.0);
}

@fragment fn fs_composite(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let size = comp.size;
    let texel = vec2<i32>(i32(pos.x), i32(size.y) - 1 - i32(pos.y));
    let src = textureLoad(hdr, texel, 0);
    let glow = textureSampleLevel(glow_tex, csamp, vec2<f32>(pos.x / size.x, (size.y - pos.y) / size.y), 0.0).rgb * comp.bloom;
    if comp.mode == 0u {
        return vec4<f32>(clamp(src.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
    }
    let w = max(0.15 * min(size.x, size.y), 1.0);
    let fade = ease(min(pos.x, size.x - pos.x), w) * ease(min(pos.y, size.y - pos.y), w);
    if comp.mode == 1u {
        let col = clamp(src.rgb + glow, vec3<f32>(0.0), vec3<f32>(1.0));
        let a = max(col.r, max(col.g, col.b));
        if a <= 0.0001 {
            return vec4<f32>(0.0);
        }
        return vec4<f32>(col / a, a * fade);
    }
    let a = clamp(src.a + luma(glow), 0.0, 1.0);
    if a <= 0.0001 {
        return vec4<f32>(0.0);
    }
    let col = clamp((clamp(src.rgb, vec3<f32>(0.0), vec3<f32>(1.0)) * src.a + glow) / a, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(col, a * fade);
}
"#;

fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        count: None,
        ty: wgpu::BindingType::Buffer {
            has_dynamic_offset: false,
            min_binding_size: None,
            ty: wgpu::BufferBindingType::Uniform,
        },
        visibility: wgpu::ShaderStages::FRAGMENT,
    }
}

/// A full-screen pipeline drawing `vertex` and the entry point of `fragment` into one unblended
/// color target of `format`.
fn fullscreen_pipeline(
    device: &wgpu::Device,
    label: &str,
    layout: &wgpu::PipelineLayout,
    vertex: &wgpu::ShaderModule,
    fragment: (&wgpu::ShaderModule, &str),
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let (fragment_module, fragment_entry) = fragment;
    let targets = [Some(wgpu::ColorTargetState {
        format,
        blend: None,
        write_mask: wgpu::ColorWrites::ALL,
    })];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        cache: None,
        multiview_mask: None,
        multisample: wgpu::MultisampleState::default(),
        depth_stencil: None,
        primitive: wgpu::PrimitiveState::default(),
        vertex: wgpu::VertexState {
            module: vertex,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: fragment_module,
            entry_point: Some(fragment_entry),
            targets: &targets,
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
    })
}

impl Gpu {
    fn new() -> Result<Self, String> {
        let (device, queue, adapter_name) = vespertine::block_on(async {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::all(),
                flags: wgpu::InstanceFlags::default(),
                backend_options: wgpu::BackendOptions::default(),
                memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
                display: None,
            });
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    compatible_surface: None,
                    force_fallback_adapter: false,
                    apply_limit_buckets: false,
                })
                .await
                .map_err(|error| format!("no GPU is available: {error}"))?;
            let name = adapter.get_info().name;
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("shader_surface_device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::downlevel_defaults()
                        .using_resolution(adapter.limits())
                        .using_alignment(adapter.limits()),
                    memory_hints: wgpu::MemoryHints::MemoryUsage,
                    trace: wgpu::Trace::Off,
                    experimental_features: wgpu::ExperimentalFeatures::disabled(),
                    default_queue: wgpu::QueueDescriptor::default(),
                })
                .await
                .map_err(|error| format!("no GPU is available: {error} ({name})"))?;
            Ok::<_, String>((device, queue, name))
        })?;

        let post = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shader_surface_post"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(POST_WGSL)),
        });
        let mut entries = vec![uniform_entry(0)];
        entries.extend((1..=9).map(texture_entry));
        entries.push(sampler_entry(10));
        entries.push(sampler_entry(11));
        let pass_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shader_surface_pass"),
            entries: &entries,
        });
        let pass_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shader_surface_pass"),
            bind_group_layouts: &[Some(&pass_layout)],
            immediate_size: 0,
        });
        let single = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shader_surface_single"),
            entries: &[texture_entry(0), sampler_entry(1), uniform_entry(2)],
        });
        let composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shader_surface_composite"),
            entries: &[
                texture_entry(0),
                texture_entry(1),
                sampler_entry(2),
                uniform_entry(3),
            ],
        });
        let make = |label: &str,
                    layout: &wgpu::BindGroupLayout,
                    entry: &str,
                    format: wgpu::TextureFormat| {
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[Some(layout)],
                immediate_size: 0,
            });
            fullscreen_pipeline(
                &device,
                label,
                &pipeline_layout,
                &post,
                (&post, entry),
                format,
            )
        };
        let bright = make("shader_surface_bright", &single, "fs_bright", PASS_FORMAT);
        let blur = make("shader_surface_blur", &single, "fs_blur", PASS_FORMAT);
        let composite = make(
            "shader_surface_composite",
            &composite_layout,
            "fs_composite",
            OUTPUT_FORMAT,
        );
        let clamp = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shader_surface_clamp"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let repeat = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shader_surface_repeat"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        Ok(Self {
            device,
            queue,
            adapter: adapter_name,
            vertex: post,
            pass_layout,
            pass_pipeline_layout,
            clamp,
            repeat,
            bright,
            blur,
            composite,
            post_layouts: PostLayouts {
                single,
                composite: composite_layout,
            },
        })
    }

    /// The adapter name.
    pub fn adapter_name(&self) -> &str {
        &self.adapter
    }

    /// Compiles a fragment module into a pass pipeline; a GPU-level validation error is
    /// returned as a message.
    fn pass_pipeline(&self, module: wgpu::naga::Module) -> Result<wgpu::RenderPipeline, String> {
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let fragment = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("shader_surface_program"),
                source: wgpu::ShaderSource::Naga(Cow::Owned(module)),
            });
        let pipeline = fullscreen_pipeline(
            &self.device,
            "shader_surface_program",
            &self.pass_pipeline_layout,
            &self.vertex,
            (&fragment, "main"),
            PASS_FORMAT,
        );
        match vespertine::block_on(scope.pop()) {
            Some(error) => Err(error.to_string()),
            None => Ok(pipeline),
        }
    }
}

struct Target {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl Target {
    fn new(
        gpu: &Gpu,
        label: &str,
        size: (u32, u32),
        format: wgpu::TextureFormat,
        extra: wgpu::TextureUsages,
    ) -> Self {
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            dimension: wgpu::TextureDimension::D2,
            format,
            mip_level_count: 1,
            sample_count: 1,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | extra,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self { texture, view }
    }
}

/// A feedback buffer: the current and previous frame output.
struct Buffer {
    current: Target,
    previous: Target,
}

struct Feedback {
    size: (u32, u32),
    buffers: Vec<Buffer>,
    hdr: Target,
    half: [Target; 2],
    half_size: (u32, u32),
    output: Target,
    readback: wgpu::Buffer,
    padded_row: u32,
}

struct ImageSlot {
    source: Option<Arc<TextureImage>>,
    view: wgpu::TextureView,
    _texture: wgpu::Texture,
}

/// The retained GPU state of one surface: compiled programs, feedback targets, uniforms,
/// textures and the time bookkeeping. It is dropped (every target released) when the surface is
/// removed or replaced.
pub struct GpuState {
    gpu: Arc<Gpu>,
    time: TimeState,
    uniforms: wgpu::Buffer,
    blur_params: [wgpu::Buffer; 2],
    bright_params: wgpu::Buffer,
    composite_params: wgpu::Buffer,
    noise: ImageSlot,
    black: ImageSlot,
    images: Vec<ImageSlot>,
    programs: HashMap<u64, Result<Arc<wgpu::RenderPipeline>, ProgramError>>,
    feedback: Option<Feedback>,
    frames: u64,
}

fn hash_program(text: &str, names: &[String]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    names.hash(&mut hasher);
    hasher.finish()
}

/// Uploads an image. With `flip`, the rows are reversed so that the image's first row is the
/// top of the picture at texture coordinate `v = 1` (the fragment coordinate origin of the
/// programs is the bottom-left, so images appear upright).
fn upload(
    gpu: &Gpu,
    label: &str,
    image: &TextureImage,
    mips: bool,
    flip: bool,
) -> (wgpu::Texture, wgpu::TextureView) {
    let levels = if mips {
        (32 - image.width.max(image.height).leading_zeros()).max(1)
    } else {
        1
    };
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: image.width,
            height: image.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: levels,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: OUTPUT_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let (mut width, mut height) = (image.width, image.height);
    let mut pixels: Cow<[u8]> = if flip {
        let row = image.width as usize * 4;
        Cow::Owned(
            image
                .rgba
                .chunks_exact(row)
                .rev()
                .flatten()
                .copied()
                .collect(),
        )
    } else {
        Cow::Borrowed(image.rgba.as_slice())
    };
    for level in 0..levels {
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: level,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        if level + 1 < levels {
            let (next_w, next_h) = ((width / 2).max(1), (height / 2).max(1));
            pixels = Cow::Owned(downsample(&pixels, width, height, next_w, next_h));
            (width, height) = (next_w, next_h);
        }
    }
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

/// A box-filter reduction to the next mip level.
pub fn downsample(pixels: &[u8], width: u32, height: u32, next_w: u32, next_h: u32) -> Vec<u8> {
    let mut out = vec![0u8; next_w as usize * next_h as usize * 4];
    for y in 0..next_h {
        for x in 0..next_w {
            let mut sum = [0u32; 4];
            let mut count = 0;
            for dy in 0..2 {
                for dx in 0..2 {
                    let (sx, sy) = ((x * 2 + dx).min(width - 1), (y * 2 + dy).min(height - 1));
                    let at = (sy * width + sx) as usize * 4;
                    for c in 0..4 {
                        sum[c] += pixels[at + c] as u32;
                    }
                    count += 1;
                }
            }
            let at = (y * next_w + x) as usize * 4;
            for c in 0..4 {
                out[at + c] = ((sum[c] + count / 2) / count) as u8;
            }
        }
    }
    out
}

/// The 256 x 256 tiling texture of independent random RGBA values (a fixed xorshift stream, so
/// every surface sees the same noise).
pub fn noise_image() -> TextureImage {
    let mut state = 0x9E37_79B9u32;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    let mut rgba = Vec::with_capacity((NOISE_SIZE * NOISE_SIZE * 4) as usize);
    for _ in 0..NOISE_SIZE * NOISE_SIZE {
        rgba.extend_from_slice(&next().to_le_bytes());
    }
    TextureImage {
        width: NOISE_SIZE,
        height: NOISE_SIZE,
        rgba,
    }
}

impl GpuState {
    /// A fresh state on the shared GPU.
    pub fn new() -> Result<Self, String> {
        let gpu = gpu()?;
        let buffer = |label: &str, size: u64| {
            gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let noise_image = noise_image();
        let (noise_texture, noise_view) =
            upload(&gpu, "shader_surface_noise", &noise_image, false, false);
        let black_image = TextureImage {
            width: 1,
            height: 1,
            rgba: vec![0; 4],
        };
        let (black_texture, black_view) =
            upload(&gpu, "shader_surface_black", &black_image, false, false);
        Ok(Self {
            uniforms: buffer(
                "shader_surface_uniforms",
                std::mem::size_of::<UniformBlock>() as u64,
            ),
            blur_params: [buffer("blur_h", 16), buffer("blur_v", 16)],
            bright_params: buffer("bright", 16),
            composite_params: buffer("composite", 16),
            noise: ImageSlot {
                source: None,
                view: noise_view,
                _texture: noise_texture,
            },
            black: ImageSlot {
                source: None,
                view: black_view,
                _texture: black_texture,
            },
            images: Vec::new(),
            programs: HashMap::new(),
            feedback: None,
            time: TimeState::default(),
            frames: 0,
            gpu,
        })
    }

    /// Frames completed successfully.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Whether feedback targets exist, and their pixel size.
    pub fn feedback_size(&self) -> Option<(u32, u32)> {
        self.feedback.as_ref().map(|f| f.size)
    }

    /// Compiles `text` (cached by text and input names); the error carries the line in the
    /// author's text.
    fn compile(
        &mut self,
        text: &str,
        names: &[String],
    ) -> Result<Arc<wgpu::RenderPipeline>, ProgramError> {
        let key = hash_program(text, names);
        if !self.programs.contains_key(&key) {
            let result = program::assemble(text, names)
                .and_then(|assembled| program::compile(&assembled))
                .and_then(|module| {
                    self.gpu
                        .pass_pipeline(module)
                        .map(Arc::new)
                        .map_err(|message| ProgramError {
                            line: None,
                            message,
                        })
                });
            self.programs.insert(key, result);
        }
        self.programs[&key].clone()
    }

    fn ensure_feedback(&mut self, size: (u32, u32), buffers: usize) {
        if self
            .feedback
            .as_ref()
            .is_some_and(|f| f.size == size && f.buffers.len() == buffers)
        {
            return;
        }
        let gpu = &self.gpu;
        let half_size = ((size.0 / 2).max(1), (size.1 / 2).max(1));
        let padded_row = (size.0 * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let readback = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shader_surface_readback"),
            size: padded_row as u64 * size.1 as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let none = wgpu::TextureUsages::empty();
        let readable = wgpu::TextureUsages::COPY_SRC;
        self.feedback = Some(Feedback {
            size,
            buffers: (0..buffers)
                .map(|i| Buffer {
                    current: Target::new(gpu, &format!("buffer{i}_a"), size, PASS_FORMAT, readable),
                    previous: Target::new(
                        gpu,
                        &format!("buffer{i}_b"),
                        size,
                        PASS_FORMAT,
                        readable,
                    ),
                })
                .collect(),
            hdr: Target::new(gpu, "shader_surface_hdr", size, PASS_FORMAT, none),
            half: [
                Target::new(gpu, "bloom_a", half_size, PASS_FORMAT, none),
                Target::new(gpu, "bloom_b", half_size, PASS_FORMAT, none),
            ],
            half_size,
            output: Target::new(
                gpu,
                "shader_surface_output",
                size,
                OUTPUT_FORMAT,
                wgpu::TextureUsages::COPY_SRC,
            ),
            readback,
            padded_row,
        });
    }

    fn sync_textures(&mut self, names: &[String], source: &dyn TextureSource) {
        let gpu = self.gpu.clone();
        let mut old = std::mem::take(&mut self.images).into_iter();
        let mut next = Vec::with_capacity(MAX_TEXTURES);
        for index in 0..MAX_TEXTURES {
            let wanted = names.get(index).and_then(|name| source.texture(name));
            let existing = old.next();
            next.push(match existing {
                Some(slot)
                    if match (&slot.source, &wanted) {
                        (Some(have), Some(want)) => Arc::ptr_eq(have, want),
                        (None, None) => true,
                        _ => false,
                    } =>
                {
                    slot
                }
                _ => match wanted {
                    Some(image) => {
                        let (texture, view) =
                            upload(&gpu, "shader_surface_texture", &image, true, true);
                        ImageSlot {
                            source: Some(image),
                            view,
                            _texture: texture,
                        }
                    }
                    // A missing name keeps its slot and samples as transparent black.
                    None => {
                        let image = TextureImage {
                            width: 1,
                            height: 1,
                            rgba: vec![0; 4],
                        };
                        let (texture, view) =
                            upload(&gpu, "shader_surface_missing_texture", &image, false, false);
                        ImageSlot {
                            source: None,
                            view,
                            _texture: texture,
                        }
                    }
                },
            });
        }
        self.images = next;
    }

    /// Renders one frame: compiles every program first (a failure leaves all state as it was),
    /// then runs the buffer passes, the image pass, the optional bloom and the composite, and
    /// reads the result back as straight-alpha BGRA, top row first.
    pub fn render(&mut self, frame: &ShaderFrame) -> Result<Vec<u8>, String> {
        if frame.buffers.len() > MAX_BUFFERS {
            return Err(format!(
                "{} buffers exceed the limit of {MAX_BUFFERS}",
                frame.buffers.len()
            ));
        }
        if frame.texture_names.len() > MAX_TEXTURES {
            return Err(format!(
                "{} textures exceed the limit of {MAX_TEXTURES}",
                frame.texture_names.len()
            ));
        }
        program::check_input_names(&frame.input_names).map_err(|e| e.to_string())?;

        // 1. Compile everything before touching any state.
        let mut pipelines = Vec::new();
        for (index, text) in frame.buffers.iter().enumerate() {
            pipelines.push(
                self.compile(text, &frame.input_names)
                    .map_err(|e| format!("buffer {index}: {e}"))?,
            );
        }
        pipelines.push(
            self.compile(&frame.main, &frame.input_names)
                .map_err(|e| e.to_string())?,
        );

        let scope = self
            .gpu
            .device
            .push_error_scope(wgpu::ErrorFilter::Validation);
        // 2. State: feedback targets, textures, uniforms.
        let buffers = frame.buffers.len();
        self.ensure_feedback(frame.size, buffers);
        self.sync_textures(&frame.texture_names, frame.textures.as_ref());
        let time = self.time.begin_frame(frame.time, frame.animated);
        let mut channel_sizes = [(1.0, 1.0); 4];
        for size in channel_sizes.iter_mut().take(buffers) {
            *size = (frame.size.0 as f32, frame.size.1 as f32);
        }
        let mut values = frame.input_values.clone();
        values.resize(frame.input_names.len(), 0.0);
        let block = UniformInputs {
            size: frame.size,
            ratio: frame.ratio,
            time,
            mouse: frame.mouse,
            camera: frame.camera,
            inputs: values,
            channel_sizes,
        }
        .to_block();
        let gpu = self.gpu.clone();
        gpu.queue
            .write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&block));
        let bloom = clamp_bloom(frame.bloom);
        let feedback = self.feedback.as_ref().expect("ensured above");
        let texel = |size: (u32, u32), dir: [f32; 2]| -> [f32; 4] {
            [1.0 / size.0 as f32, 1.0 / size.1 as f32, dir[0], dir[1]]
        };
        gpu.queue.write_buffer(
            &self.bright_params,
            0,
            bytemuck::cast_slice(&texel(feedback.half_size, [0.0, 0.0])),
        );
        gpu.queue.write_buffer(
            &self.blur_params[0],
            0,
            bytemuck::cast_slice(&texel(feedback.half_size, [1.0, 0.0])),
        );
        gpu.queue.write_buffer(
            &self.blur_params[1],
            0,
            bytemuck::cast_slice(&texel(feedback.half_size, [0.0, 1.0])),
        );
        let mode = match frame.blend {
            BlendMode::Opaque => 0u32,
            BlendMode::Light => 1,
            BlendMode::Alpha => 2,
        };
        let composite: [u32; 4] = [
            (frame.size.0 as f32).to_bits(),
            (frame.size.1 as f32).to_bits(),
            bloom.to_bits(),
            mode,
        ];
        gpu.queue
            .write_buffer(&self.composite_params, 0, bytemuck::cast_slice(&composite));

        // 3. Passes.
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("shader_surface_frame"),
            });
        let black = &self.black.view;
        for (pass_index, pipeline) in pipelines.iter().enumerate() {
            let channel = |j: usize| -> &wgpu::TextureView {
                if j < buffers {
                    let buffer = &feedback.buffers[j];
                    // An earlier pass shows this frame's output; this and later passes (feedback)
                    // show the previous frame's.
                    if j < pass_index {
                        &buffer.current.view
                    } else {
                        &buffer.previous.view
                    }
                } else {
                    black
                }
            };
            let entries = [
                bind_buffer(0, &self.uniforms),
                bind_view(1, channel(0)),
                bind_view(2, channel(1)),
                bind_view(3, channel(2)),
                bind_view(4, channel(3)),
                bind_view(5, self.image_view(0)),
                bind_view(6, self.image_view(1)),
                bind_view(7, self.image_view(2)),
                bind_view(8, self.image_view(3)),
                bind_view(9, &self.noise.view),
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: wgpu::BindingResource::Sampler(&gpu.clamp),
                },
                wgpu::BindGroupEntry {
                    binding: 11,
                    resource: wgpu::BindingResource::Sampler(&gpu.repeat),
                },
            ];
            let bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("shader_surface_pass"),
                layout: &gpu.pass_layout,
                entries: &entries,
            });
            let target = if pass_index < buffers {
                &feedback.buffers[pass_index].current.view
            } else {
                &feedback.hdr.view
            };
            run_pass(&mut encoder, pipeline, &bind_group, target);
        }
        let single = |gpu: &Gpu, src: &wgpu::TextureView, params: &wgpu::Buffer| {
            gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("shader_surface_post"),
                layout: &gpu.post_layouts.single,
                entries: &[
                    bind_view(0, src),
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&gpu.clamp),
                    },
                    bind_buffer(2, params),
                ],
            })
        };
        let glow = if bloom > 0.0 {
            run_pass(
                &mut encoder,
                &gpu.bright,
                &single(&gpu, &feedback.hdr.view, &self.bright_params),
                &feedback.half[0].view,
            );
            for _ in 0..BLOOM_ITERATIONS {
                run_pass(
                    &mut encoder,
                    &gpu.blur,
                    &single(&gpu, &feedback.half[0].view, &self.blur_params[0]),
                    &feedback.half[1].view,
                );
                run_pass(
                    &mut encoder,
                    &gpu.blur,
                    &single(&gpu, &feedback.half[1].view, &self.blur_params[1]),
                    &feedback.half[0].view,
                );
            }
            &feedback.half[0].view
        } else {
            black
        };
        let composite_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shader_surface_composite"),
            layout: &gpu.post_layouts.composite,
            entries: &[
                bind_view(0, &feedback.hdr.view),
                bind_view(1, glow),
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&gpu.clamp),
                },
                bind_buffer(3, &self.composite_params),
            ],
        });
        run_pass(
            &mut encoder,
            &gpu.composite,
            &composite_group,
            &feedback.output.view,
        );
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &feedback.output.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &feedback.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(feedback.padded_row),
                    rows_per_image: Some(frame.size.1),
                },
            },
            wgpu::Extent3d {
                width: frame.size.0,
                height: frame.size.1,
                depth_or_array_layers: 1,
            },
        );
        let submission = gpu.queue.submit(std::iter::once(encoder.finish()));
        let (sender, receiver) = std::sync::mpsc::channel();
        feedback
            .readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        gpu.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: Some(std::time::Duration::from_secs(30)),
            })
            .map_err(|error| format!("waiting for the GPU failed: {error}"))?;
        receiver
            .recv_timeout(std::time::Duration::from_secs(30))
            .map_err(|error| format!("reading the frame back failed: {error}"))?
            .map_err(|error| format!("mapping the readback buffer failed: {error}"))?;
        if let Some(error) = vespertine::block_on(scope.pop()) {
            feedback.readback.unmap();
            return Err(format!("GPU error: {error}"));
        }
        let mapped = feedback
            .readback
            .slice(..)
            .get_mapped_range()
            .map_err(|e| e.to_string())?;
        let row = frame.size.0 as usize * 4;
        let mut bgra = Vec::with_capacity(row * frame.size.1 as usize);
        for line in mapped
            .chunks_exact(feedback.padded_row as usize)
            .take(frame.size.1 as usize)
        {
            bgra.extend_from_slice(&line[..row]);
        }
        drop(mapped);
        feedback.readback.unmap();
        for pixel in bgra.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }

        // 4. Success: the buffers swap roles.
        for buffer in &mut self.feedback.as_mut().expect("ensured above").buffers {
            std::mem::swap(&mut buffer.current, &mut buffer.previous);
        }
        self.frames += 1;
        Ok(bgra)
    }

    fn image_view(&self, index: usize) -> &wgpu::TextureView {
        self.images
            .get(index)
            .map_or(&self.black.view, |slot| &slot.view)
    }

    /// The previous-frame output of buffer `index`, read back as RGBA f32 (for tests): the
    /// value a buffer pass would see through its own feedback channel on the next frame.
    pub fn read_previous(&self, index: usize) -> Option<Vec<[f32; 4]>> {
        let feedback = self.feedback.as_ref()?;
        let buffer = feedback.buffers.get(index)?;
        // After a frame the roles were swapped: `previous` holds the last frame's output.
        let (width, height) = feedback.size;
        let padded = (width * 8).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let readback = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shader_surface_feedback_readback"),
            size: padded as u64 * height as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &buffer.previous.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        let submission = self.gpu.queue.submit(std::iter::once(encoder.finish()));
        let (sender, receiver) = std::sync::mpsc::channel();
        readback.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = sender.send(r);
        });
        self.gpu
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: Some(std::time::Duration::from_secs(30)),
            })
            .ok()?;
        receiver.recv().ok()?.ok()?;
        let mapped = readback.slice(..).get_mapped_range().ok()?;
        let mut out = Vec::new();
        for line in mapped.chunks_exact(padded as usize).take(height as usize) {
            for pixel in line[..width as usize * 8].chunks_exact(8) {
                let half =
                    |i: usize| half_to_f32(u16::from_le_bytes([pixel[i * 2], pixel[i * 2 + 1]]));
                out.push([half(0), half(1), half(2), half(3)]);
            }
        }
        Some(out)
    }
}

/// Converts an IEEE half float to `f32`.
pub fn half_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = (bits >> 10) & 0x1F;
    let fraction = (bits & 0x3FF) as f32;
    match exponent {
        0 => sign * fraction * 2f32.powi(-24),
        31 => {
            if fraction == 0.0 {
                sign * f32::INFINITY
            } else {
                f32::NAN
            }
        }
        _ => sign * (1.0 + fraction / 1024.0) * 2f32.powi(exponent as i32 - 15),
    }
}

fn bind_view(binding: u32, view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(view),
    }
}

fn bind_buffer(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: buffer.as_entire_binding(),
    }
}

fn run_pass(
    encoder: &mut wgpu::CommandEncoder,
    pipeline: &wgpu::RenderPipeline,
    bind_group: &wgpu::BindGroup,
    target: &wgpu::TextureView,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("shader_surface_pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                store: wgpu::StoreOp::Store,
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
            },
        })],
        multiview_mask: None,
        occlusion_query_set: None,
        timestamp_writes: None,
        depth_stencil_attachment: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind_group, &[]);
    pass.draw(0..3, 0..1);
}
